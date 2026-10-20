use crate::{
    config::{self, Backend, Settings},
    confirm,
    remote::{self, Receipt},
    INTERRUPTED,
};
use chrono::Utc;
use fs2::FileExt;
use indicatif::ProgressBar;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::ffi::{CString, OsString};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
enum State {
    Prepared,
    Copying,
    Copied,
    RemovingSource,
    Trashed,
    Restoring,
    Purging,
    RemoteOnly,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Record {
    format: u32,
    id: String,
    original_bytes: Vec<u8>,
    original_path: String,
    deleted_at: i64,
    expire_days: i64,
    kind: String,
    size_bytes: Option<u64>,
    device: u64,
    inode: u64,
    state: State,
    restore_target_bytes: Option<Vec<u8>>,
    #[serde(default)]
    remote: Option<Receipt>,
    #[serde(default)]
    journal_group: Option<String>,
    #[serde(default)]
    local_parent_bytes: Option<Vec<u8>>,
}

impl Record {
    fn original(&self) -> PathBuf {
        OsString::from_vec(self.original_bytes.clone()).into()
    }
    fn expired(&self) -> bool {
        Utc::now().timestamp()
            >= self
                .deleted_at
                .saturating_add(self.expire_days.saturating_mul(86400))
    }
}

pub struct Store {
    base: PathBuf,
    files: PathBuf,
    meta: PathBuf,
    mounts: Vec<PathBuf>,
    settings: Settings,
    volume_roots: Vec<PathBuf>,
    _lock: File,
}

pub fn absolute(input: &Path) -> io::Result<PathBuf> {
    if input.as_os_str().is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "empty file name"));
    }
    let path = if input.is_absolute() {
        input.to_path_buf()
    } else {
        std::env::current_dir()?.join(input)
    };
    if path.as_os_str().as_bytes().iter().all(|c| *c == b'/') {
        return Ok(PathBuf::from("/"));
    }
    // Resolve parent symlinks and '..', but never dereference the removed entry.
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("invalid file name"))?;
    let parent = fs::canonicalize(path.parent().unwrap_or(Path::new("/")))?;
    let result = parent.join(name);
    if input.as_os_str().as_bytes().ends_with(b"/")
        && fs::symlink_metadata(&result)?.file_type().is_symlink()
    {
        return Err(io::Error::other("refusing a symlink operand with trailing slash; remove the trailing slash to trash the link"));
    }
    if input.as_os_str().as_bytes().ends_with(b"/") && !fs::symlink_metadata(&result)?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "not a directory",
        ));
    }
    Ok(result)
}

fn private_dir(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            DirBuilder::new().recursive(true).mode(0o700).create(path)?
        }
        Err(e) => return Err(e),
    }
    let m = fs::symlink_metadata(path)?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } {
        return Err(io::Error::other(format!(
            "{}: unsafe store directory: type={:?}, owner uid={}, current uid={}; refusing to follow links or change ownership",
            path.display(), m.file_type(), m.uid(), unsafe { libc::geteuid() }
        )));
    }
    if m.mode() & 0o777 != 0o700 {
        return Err(io::Error::other(format!(
            "{}: store directory mode is {:04o}, required 0700; run srm doctor --repair-permissions as the directory owner",
            path.display(), m.mode() & 0o777
        )));
    }
    Ok(())
}

fn private_file(path: &Path, append: bool) -> io::Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(!append)
        .append(append)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let m = file.metadata()?;
    if !m.is_file() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
        return Err(io::Error::other(
            "journal file must be private and owned by this user",
        ));
    }
    Ok(file)
}

fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut file = private_file(&tmp, false)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&tmp, path)?;
    sync_dir(path.parent().unwrap())
}

fn exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

pub(crate) fn rename_new(src: &Path, dst: &Path) -> io::Result<()> {
    let src = CString::new(src.as_os_str().as_bytes()).map_err(io::Error::other)?;
    let dst = CString::new(dst.as_os_str().as_bytes()).map_err(io::Error::other)?;
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            src.as_ptr(),
            libc::AT_FDCWD,
            dst.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn remove_entry(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn interrupted() -> io::Result<()> {
    if INTERRUPTED.load(Ordering::Relaxed) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "interrupted; journal and copied data retained",
        ))
    } else {
        Ok(())
    }
}

// Copy the complete entry before touching the source. GNU cp handles sparse files,
// hard links within trees, timestamps, modes, ACLs, xattrs, and symlinks correctly.
fn copy_complete(src: &Path, dst: &Path) -> io::Result<()> {
    interrupted()?;
    if exists(dst)? {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "copy destination exists",
        ));
    }
    let before = snapshot(src)?;
    let pb = if io::stderr().is_terminal() {
        let p = ProgressBar::new_spinner();
        p.set_message(format!("Copying {} across filesystems", src.display()));
        p.enable_steady_tick(Duration::from_millis(120));
        Some(p)
    } else {
        None
    };
    let mut child = Command::new("/bin/cp")
        .args([
            "--archive",
            "--reflink=auto",
            "--sparse=always",
            "--no-target-directory",
            "--",
        ])
        .arg(src)
        .arg(dst)
        .spawn()?;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if INTERRUPTED.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(p) = &pb {
                p.finish_and_clear();
            }
            return interrupted();
        }
        std::thread::sleep(Duration::from_millis(30));
    };
    if let Some(p) = pb {
        p.finish_and_clear();
    }
    if !status.success() {
        return Err(io::Error::other(
            "cross-device copy failed; source retained, inspect srm doctor",
        ));
    }
    interrupted()?;
    if snapshot(src)? != before {
        return Err(io::Error::other(
            "source tree changed during copy; source and copy retained",
        ));
    }
    // Directory-only fsync is insufficient for copied file data. syncfs flushes
    // the destination filesystem, including data and extended attributes.
    let parent = File::open(dst.parent().unwrap())?;
    if unsafe { libc::syncfs(parent.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[derive(PartialEq)]
pub(crate) struct Stamp {
    dev: u64,
    ino: u64,
    len: u64,
    pub(crate) mode: u32,
    pub(crate) uid: u32,
    gid: u32,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

pub(crate) fn snapshot(root: &Path) -> io::Result<std::collections::BTreeMap<PathBuf, Stamp>> {
    let mut entries = std::collections::BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        interrupted()?;
        let m = fs::symlink_metadata(&path)?;
        if m.is_dir() {
            for child in fs::read_dir(&path)? {
                stack.push(child?.path());
            }
        }
        entries.insert(
            path,
            Stamp {
                dev: m.dev(),
                ino: m.ino(),
                len: m.len(),
                mode: m.mode(),
                uid: m.uid(),
                gid: m.gid(),
                mtime: (m.mtime(), m.mtime_nsec()),
                ctime: (m.ctime(), m.ctime_nsec()),
            },
        );
    }
    Ok(entries)
}

fn measure(path: &Path) -> io::Result<u64> {
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        interrupted()?;
        let m = fs::symlink_metadata(&p)?;
        if m.is_dir() {
            for entry in fs::read_dir(p)? {
                stack.push(entry?.path());
            }
        } else {
            total = total.saturating_add(m.len());
        }
    }
    Ok(total)
}

impl Store {
    fn base_path(settings: &Settings) -> io::Result<PathBuf> {
        let base = if let Some(p) = &settings.local_store {
            p.clone()
        } else if config::root() {
            PathBuf::from("/var/lib/safe-srm/root")
        } else if let Some(p) = std::env::var_os("SRM_HOME") {
            PathBuf::from(p)
        } else if let Some(p) =
            std::env::var_os("XDG_DATA_HOME").filter(|p| Path::new(p).is_absolute())
        {
            PathBuf::from(p).join("safe-srm")
        } else {
            PathBuf::from(
                std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is not set"))?,
            )
            .join(".local/share/safe-srm")
        };
        if !base.is_absolute() {
            return Err(io::Error::other("trash location must be an absolute path"));
        }
        Ok(base)
    }

    pub fn repair_permissions() -> io::Result<()> {
        let settings = config::load()?;
        let base = Self::base_path(&settings)?;
        // Open every component without following links. Only known management
        // directories are changed; payloads, journals and ownership are untouched.
        let mut paths = vec![base.clone()];
        for name in ["files", "meta", "locks", "staging"] {
            paths.push(base.join(name));
        }
        let mut directories = Vec::new();
        for path in paths {
            let mut dir = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
                .open("/")?;
            let mut missing = false;
            for component in path.components() {
                let std::path::Component::Normal(name) = component else {
                    if matches!(component, std::path::Component::RootDir) {
                        continue;
                    }
                    return Err(io::Error::other(
                        "repair requires an absolute path without '.' or '..'",
                    ));
                };
                let name = CString::new(name.as_bytes()).map_err(io::Error::other)?;
                let fd = unsafe {
                    libc::openat(
                        dir.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                };
                if fd < 0 {
                    let e = io::Error::last_os_error();
                    if e.kind() == io::ErrorKind::NotFound {
                        missing = true;
                        break;
                    }
                    return Err(e);
                }
                use std::os::fd::FromRawFd;
                dir = unsafe { File::from_raw_fd(fd) };
                let m = dir.metadata()?;
                if m.uid() != 0 && m.uid() != unsafe { libc::geteuid() } {
                    return Err(io::Error::other("repair refuses foreign-owned ancestor"));
                }
                if m.mode() & 0o022 != 0 {
                    return Err(io::Error::other(
                        "repair refuses group/world-writable path components",
                    ));
                }
            }
            if missing {
                continue;
            }
            if dir.metadata()?.uid() != unsafe { libc::geteuid() } {
                return Err(io::Error::other(format!(
                    "{}: wrong owner; repair never changes ownership",
                    path.display()
                )));
            }
            directories.push((path, dir));
        }
        for (path, dir) in directories {
            use std::os::unix::fs::PermissionsExt;
            dir.set_permissions(fs::Permissions::from_mode(0o700))?;
            dir.sync_all()?;
            println!("secured directory: {} (0700)", path.display());
        }
        Ok(())
    }

    pub fn open() -> io::Result<Self> {
        let settings = config::load()?;
        let base = Self::base_path(&settings)?;
        if config::root() {
            config::trusted_root_path(&base)?;
        }
        private_dir(&base)?;
        let base = fs::canonicalize(base)?;
        let lock = private_file(&base.join("lock"), true)?;
        // Poll to allow Ctrl+C while another srm is performing a large copy.
        loop {
            match lock.try_lock_exclusive() {
                Ok(()) => break,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    interrupted()?;
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(e),
            }
        }
        let files = base.join("files");
        let meta = base.join("meta");
        private_dir(&files)?;
        private_dir(&meta)?;
        let mut mounts = vec![];
        for line in fs::read("/proc/self/mountinfo")?.split(|b| *b == b'\n') {
            if let Some(raw) = line.split(|b| *b == b' ').nth(4) {
                let mut decoded = vec![];
                let mut i = 0;
                while i < raw.len() {
                    if raw[i] == b'\\'
                        && i + 3 < raw.len()
                        && raw[i + 1..i + 4].iter().all(|b| (b'0'..=b'7').contains(b))
                    {
                        decoded.push(
                            (raw[i + 1] - b'0') * 64 + (raw[i + 2] - b'0') * 8 + raw[i + 3] - b'0',
                        );
                        i += 4;
                    } else {
                        decoded.push(raw[i]);
                        i += 1;
                    }
                }
                mounts.push(PathBuf::from(OsString::from_vec(decoded)));
            }
        }
        let mut result = Self {
            base,
            files,
            meta,
            mounts,
            settings,
            volume_roots: Vec::new(),
            _lock: lock,
        };
        let registry = result.base.join("volumes.json");
        if exists(&registry)? {
            let raw: Vec<Vec<u8>> = serde_json::from_reader(
                OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(registry)?,
            )?;
            result.volume_roots = raw
                .into_iter()
                .map(|p| PathBuf::from(OsString::from_vec(p)))
                .collect();
            if result.volume_roots.iter().any(|p| !p.is_absolute()) {
                return Err(io::Error::other("invalid volume registry"));
            }
        }
        result.rotate_log()?;
        Ok(result)
    }

    pub fn check_operand(&self, path: &Path) -> io::Result<()> {
        if path.starts_with(&self.base)
            || self.base.starts_with(path)
            || self
                .volume_roots
                .iter()
                .chain(self.settings.volume_stores.iter())
                .any(|root| path.starts_with(root) || root.starts_with(path))
        {
            return Err(io::Error::other(
                "refusing to move the trash, its ancestors, or entries inside it",
            ));
        }
        Ok(())
    }

    pub fn has_mount_at_or_below(&self, path: &Path) -> bool {
        self.mounts.iter().any(|p| p.starts_with(path))
    }

    fn backup_directory(&mut self, source: &Path) -> io::Result<PathBuf> {
        let device = fs::symlink_metadata(source)?.dev();
        if fs::metadata(&self.files)?.dev() == device {
            return Ok(self.files.clone());
        }
        for root in self.settings.volume_stores.clone() {
            let parent = root.ancestors().find(|p| p.exists());
            if parent
                .and_then(|p| fs::metadata(p).ok())
                .is_none_or(|m| m.dev() != device)
            {
                continue;
            }
            private_dir(&root)?;
            let root = fs::canonicalize(root)?;
            let files = root.join("files");
            private_dir(&files)?;
            if !self.volume_roots.contains(&root) {
                self.volume_roots.push(root);
                let raw: Vec<_> = self
                    .volume_roots
                    .iter()
                    .map(|p| p.as_os_str().as_bytes().to_vec())
                    .collect();
                atomic_write(&self.base.join("volumes.json"), &serde_json::to_vec(&raw)?)?;
            }
            self.check_operand(source)?;
            return Ok(files);
        }
        Ok(self.files.clone())
    }

    fn backup(&self, r: &Record) -> io::Result<PathBuf> {
        if let Some(raw) = &r.local_parent_bytes {
            let parent = PathBuf::from(OsString::from_vec(raw.clone()));
            if !self
                .volume_roots
                .iter()
                .any(|root| parent == root.join("files"))
            {
                return Err(io::Error::other("unregistered local volume in metadata"));
            }
            return Ok(parent.join(&r.id));
        }
        Ok(self.files.join(&r.id))
    }

    pub fn fast_directories(&self) -> bool {
        self.settings.directory_strategy == config::DirectoryStrategy::Rename
    }
    pub fn batch_size(&self) -> usize {
        self.settings.batch_size
    }
    pub fn can_batch(&self, paths: &[PathBuf]) -> bool {
        if self.settings.backend == Backend::RemoteSync || paths.len() < 2 {
            return false;
        }
        let Ok(m) = fs::metadata(&self.files) else {
            return false;
        };
        let mut unique = std::collections::HashSet::new();
        paths.iter().all(|p| {
            unique.insert(p)
                && self.check_operand(p).is_ok()
                && fs::symlink_metadata(p).is_ok_and(|s| !s.is_dir() && s.dev() == m.dev())
        })
    }

    pub fn trash_batch(
        &mut self,
        paths: &[PathBuf],
        days: i64,
    ) -> io::Result<Vec<Result<String, String>>> {
        let group = format!("batch_{}", random_suffix()?);
        let journal = self.meta.join(format!("{group}.json"));
        if exists(&journal)? {
            return Err(io::Error::other("batch ID collision; retry"));
        }
        let mut records = Vec::new();
        let mut ids = std::collections::HashSet::new();
        for path in paths {
            self.check_operand(path)?;
            let m = fs::symlink_metadata(path)?;
            let symlink = m.file_type().is_symlink();
            let id = format!("{}_{}", if symlink { "l" } else { "f" }, random_suffix()?);
            if !ids.insert(id.clone())
                || exists(&self.files.join(&id))?
                || exists(&self.meta.join(format!("{id}.json")))?
            {
                return Err(io::Error::other("ID collision; retry"));
            }
            records.push(Record {
                format: 2,
                id,
                original_bytes: path.as_os_str().as_bytes().to_vec(),
                original_path: path.to_string_lossy().into(),
                deleted_at: Utc::now().timestamp(),
                expire_days: days,
                kind: if symlink {
                    "symlink".into()
                } else {
                    "file".into()
                },
                size_bytes: Some(m.len()),
                device: m.dev(),
                inode: m.ino(),
                state: State::Prepared,
                restore_target_bytes: None,
                journal_group: Some(group.clone()),
                local_parent_bytes: None,
                remote: if self.settings.backend == Backend::RemoteAsync {
                    Some(Receipt::pending(self.settings.remote.clone().unwrap()))
                } else {
                    None
                },
            });
        }
        // One durable prepare record for the whole bounded group, before any rename.
        atomic_write(&journal, &serde_json::to_vec(&records)?)?;
        self.audit_batch("batch_delete_begin", &records)?;
        let mut results = Vec::new();
        let mut parents = std::collections::BTreeSet::new();
        for record in &mut records {
            let source = record.original();
            let result = (|| -> io::Result<()> {
                interrupted()?;
                let current = fs::symlink_metadata(&source)?;
                if current.dev() != record.device || current.ino() != record.inode {
                    return Err(io::Error::other("source changed before rename"));
                }
                rename_new(&source, &self.files.join(&record.id))?;
                parents.insert(source.parent().unwrap().to_path_buf());
                record.state = State::Trashed;
                Ok(())
            })();
            results.push(result.map(|_| record.id.clone()).map_err(|e| e.to_string()));
        }
        for parent in parents {
            sync_dir(&parent)?;
        }
        sync_dir(&self.files)?;
        // Prepared-but-unmoved entries are safe to drop only after the results
        // are known. A crash before this update leaves recoverable prepare records.
        records.retain(|r| r.state == State::Trashed);
        if records.is_empty() {
            fs::remove_file(&journal)?;
            sync_dir(&self.meta)?;
        } else {
            atomic_write(&journal, &serde_json::to_vec(&records)?)?;
        }
        self.audit_batch("batch_delete_complete", &records)?;
        Ok(results)
    }

    fn audit_batch(&self, action: &str, records: &[Record]) -> io::Result<()> {
        let mut log = private_file(&self.base.join("audit.jsonl"), true)?;
        let items: Vec<_> = records
            .iter()
            .map(|r| {
                json!({"id": r.id, "original_path": r.original_path,
            "original_bytes": r.original_bytes, "state": r.state})
            })
            .collect();
        serde_json::to_writer(
            &mut log,
            &json!({"action": action, "items": items, "uid": unsafe { libc::geteuid() },
            "timestamp": Utc::now().to_rfc3339(), "unix_time": Utc::now().timestamp()}),
        )?;
        log.write_all(b"\n")?;
        log.sync_all()
    }

    fn item_lock(&self, id: &str) -> io::Result<File> {
        let dir = self.base.join("locks");
        private_dir(&dir)?;
        let file = private_file(&dir.join(id), true)?;
        file.try_lock_exclusive()
            .map_err(|e| io::Error::other(format!("item is busy (possibly uploading): {e}")))?;
        Ok(file)
    }

    pub fn launch_worker(&self) -> io::Result<()> {
        if self.settings.backend != Backend::RemoteAsync {
            return Ok(());
        }
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;
        let log = private_file(&self.base.join("upload.log"), true)?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("--config")
            .arg(config::path()?)
            .args(["remote", "sync"])
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .process_group(0);
        // Explicitly carry only the non-privileged store override. Root always
        // selects its store from trusted configuration and ignores inherited HOME.
        if !config::root() {
            command.env("SRM_HOME", &self.base);
        }
        command.spawn()?;
        Ok(())
    }

    fn upload_record(&mut self, record: &mut Record) -> io::Result<()> {
        let _lease = self.item_lock(&record.id)?;
        let source = self.backup(record)?;
        if !exists(&source)? {
            return Err(io::Error::other(
                "local backup missing; remote upload aborted",
            ));
        }
        let receipt = record
            .remote
            .as_mut()
            .ok_or_else(|| io::Error::other("entry is local-only"))?;
        let old_object = receipt.object.clone();
        receipt.object = format!("{}-{}", record.id, random_suffix()?);
        receipt.attempts += 1;
        receipt.last_error = None;
        let target = receipt.target.clone();
        let object = receipt.object.clone();
        self.save(record)?;
        self.audit("upload_begin", record)?;
        let metadata = serde_json::to_value(&record)?;
        FileExt::unlock(&self._lock)?;
        let result = (|| {
            if !old_object.is_empty() {
                remote::request(&target, "delete", Some(&old_object), None)?;
            }
            remote::upload(&target, &object, &source, &metadata)
        })();
        FileExt::lock_exclusive(&self._lock)?;
        match result {
            Ok(hash) => {
                let remote = record.remote.as_mut().unwrap();
                remote.sha256 = hash;
                remote.uploaded = true;
                remote.last_error = None;
                self.save(record)?;
                self.audit("upload_verified", record)?;
                if target.release_local_after_upload {
                    // Persist remote availability before removing the local cache.
                    record.state = State::RemoteOnly;
                    record.remote.as_mut().unwrap().evicted = true;
                    self.save(record)?;
                    FileExt::unlock(&self._lock)?;
                    let removed = remove_entry(&source);
                    FileExt::lock_exclusive(&self._lock)?;
                    removed?;
                    sync_dir(source.parent().unwrap())?;
                    self.audit("local_cache_released", record)?;
                }
                Ok(())
            }
            Err(e) => {
                record.remote.as_mut().unwrap().last_error = Some(e.to_string());
                self.save(record)?;
                self.audit("upload_failed_local_retained", record)?;
                Err(e)
            }
        }
    }

    pub fn sync_remote(&mut self) -> io::Result<bool> {
        if self.settings.backend == Backend::Local {
            return Ok(true);
        }
        let worker = private_file(&self.base.join("worker.lock"), true)?;
        if worker.try_lock_exclusive().is_err() {
            return Ok(true);
        }
        let (records, damaged) = self.records()?;
        let mut success = !damaged;
        for mut r in records {
            interrupted()?;
            // Network IO releases the global lock; another process may have
            // restored a queued entry since the queue snapshot was taken.
            r = match self.resolve(&r.id) {
                Ok(current) => current,
                Err(_) => continue,
            };
            if r.state == State::RemoteOnly && r.remote.as_ref().is_some_and(|r| r.uploaded) {
                let path = self.backup(&r)?;
                if exists(&path)? {
                    let _lease = self.item_lock(&r.id)?;
                    FileExt::unlock(&self._lock)?;
                    let removed = remove_entry(&path);
                    FileExt::lock_exclusive(&self._lock)?;
                    if let Err(e) = removed {
                        eprintln!("srm: remote backup verified but cache cleanup failed: {e}");
                        success = false;
                    } else {
                        sync_dir(path.parent().unwrap())?;
                    }
                }
                continue;
            }
            if r.state != State::Trashed || r.remote.as_ref().is_none_or(|x| x.uploaded) {
                continue;
            }
            match self.upload_record(&mut r) {
                Ok(()) => println!("remote verified {}", r.id),
                Err(e) => {
                    eprintln!(
                        "srm: archive/cache operation {} failed; inspect remote status: {e}",
                        r.id
                    );
                    success = false;
                }
            }
        }
        Ok(success)
    }

    pub fn remote_status(&self) -> io::Result<()> {
        let (records, damaged) = self.records()?;
        let rows: Vec<_> = records.iter().filter(|r| r.remote.is_some()).map(|r|
            json!({"id": r.id, "original_path": r.original_path, "state": r.state, "remote": r.remote})).collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        if damaged {
            return Err(io::Error::other("damaged journals present"));
        }
        Ok(())
    }

    pub fn retarget_pending(&mut self) -> io::Result<()> {
        let target = self
            .settings
            .remote
            .clone()
            .ok_or_else(|| io::Error::other("no remote configuration"))?;
        target.validate()?;
        let (records, damaged) = self.records()?;
        if damaged {
            return Err(io::Error::other(
                "repair damaged journals before retargeting",
            ));
        }
        for mut record in records {
            if record.state != State::Trashed || record.remote.as_ref().is_none_or(|r| r.uploaded) {
                continue;
            }
            let _lease = self.item_lock(&record.id)?;
            if !exists(&self.backup(&record)?)? {
                return Err(io::Error::other("local backup missing; not retargeting"));
            }
            record.remote = Some(Receipt::pending(target.clone()));
            self.save(&record)?;
            self.audit("remote_retarget", &record)?;
            println!(
                "retargeted {} (old remote partial objects, if any, retained)",
                record.id
            );
        }
        Ok(())
    }

    pub fn import_remote(&mut self, object: &str) -> io::Result<String> {
        let target = self
            .settings
            .remote
            .clone()
            .ok_or_else(|| io::Error::other("no remote configuration"))?;
        let manifest = remote::request(&target, "inspect", Some(object), None)?;
        let hash = manifest["sha256"]
            .as_str()
            .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| io::Error::other("invalid remote digest"))?;
        let mut record: Record = serde_json::from_value(manifest["record"].clone())?;
        if !valid_id(&record.id)
            || record.id.len() != 18
            || record.format != 2
            || !record.original().is_absolute()
            || record.original_bytes.contains(&0)
        {
            return Err(io::Error::other("invalid remote metadata"));
        }
        if self.records()?.0.iter().any(|r| r.id == record.id)
            || exists(&self.files.join(&record.id))?
        {
            return Err(io::Error::other(
                "ID already exists locally; restore the local ID instead",
            ));
        }
        record.journal_group = None;
        record.local_parent_bytes = None;
        record.state = State::RemoteOnly;
        record.restore_target_bytes = None;
        let mut receipt = Receipt::pending(target);
        receipt.object = object.into();
        receipt.sha256 = hash.into();
        receipt.uploaded = true;
        receipt.evicted = true;
        record.remote = Some(receipt);
        self.save(&record)?;
        self.audit("remote_import", &record)?;
        Ok(record.id)
    }

    fn save(&self, record: &Record) -> io::Result<()> {
        if let Some(group) = &record.journal_group {
            let path = self.meta.join(format!("{group}.json"));
            let mut records: Vec<Record> = serde_json::from_reader(
                OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&path)?,
            )?;
            let entry = records
                .iter_mut()
                .find(|r| r.id == record.id)
                .ok_or_else(|| io::Error::other("batch journal entry missing"))?;
            *entry = record.clone();
            return atomic_write(&path, &serde_json::to_vec(&records)?);
        }
        atomic_write(
            &self.meta.join(format!("{}.json", record.id)),
            &serde_json::to_vec_pretty(record)?,
        )
    }

    fn forget(&self, record: &Record) -> io::Result<()> {
        if let Some(group) = &record.journal_group {
            let path = self.meta.join(format!("{group}.json"));
            let mut records: Vec<Record> = serde_json::from_reader(
                OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&path)?,
            )?;
            records.retain(|r| r.id != record.id);
            if !records.is_empty() {
                return atomic_write(&path, &serde_json::to_vec(&records)?);
            }
            fs::remove_file(path)?;
        } else {
            fs::remove_file(self.meta.join(format!("{}.json", record.id)))?;
        }
        sync_dir(&self.meta)
    }

    fn audit(&self, action: &str, record: &Record) -> io::Result<()> {
        let mut log = private_file(&self.base.join("audit.jsonl"), true)?;
        serde_json::to_writer(
            &mut log,
            &json!({"timestamp": Utc::now().to_rfc3339(), "unix_time": Utc::now().timestamp(),
            "action": action, "id": record.id, "original_path": record.original_path,
            "original_bytes": record.original_bytes, "state": record.state,
            "restore_target_bytes": record.restore_target_bytes, "uid": unsafe { libc::geteuid() },
            "sudo_uid": std::env::var("SUDO_UID").ok(), "remote": record.remote}),
        )?;
        log.write_all(b"\n")?;
        log.sync_data()
    }

    fn rotate_log(&mut self) -> io::Result<()> {
        let marker = self.base.join("log-date");
        let today = Utc::now().format("%Y-%m-%d").to_string();
        if fs::read_to_string(&marker).ok().as_deref() == Some(&today) {
            return Ok(());
        }
        let path = self.base.join("audit.jsonl");
        if exists(&path)? {
            let input = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)?;
            let temp = self.base.join("audit.rotate");
            let mut output = private_file(&temp, false)?;
            for line in BufReader::new(input).lines() {
                let line = line?;
                let old = serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .and_then(|v| v["unix_time"].as_i64())
                    .is_some_and(|t| t < Utc::now().timestamp() - 30 * 86400);
                if !old {
                    writeln!(output, "{line}")?;
                }
            }
            output.sync_all()?;
            fs::rename(temp, path)?;
        }
        atomic_write(&marker, today.as_bytes())
    }

    fn records(&self) -> io::Result<(Vec<Record>, bool)> {
        let mut records = vec![];
        let mut damaged = false;
        for entry in fs::read_dir(&self.meta)? {
            let path = entry?.path();
            if path.extension().is_none_or(|s| s != "json") {
                eprintln!("srm: uncommitted journal retained: {}", path.display());
                damaged = true;
                continue;
            }
            let read = (|| -> io::Result<Vec<Record>> {
                let file = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&path)?;
                let value: serde_json::Value = serde_json::from_reader(file)?;
                let batch = value.is_array();
                let entries: Vec<Record> = if batch {
                    serde_json::from_value(value)?
                } else {
                    vec![serde_json::from_value(value)?]
                };
                for r in &entries {
                    let expected = if batch {
                        r.journal_group.as_deref().filter(|s| valid_group(s))
                    } else {
                        Some(r.id.as_str())
                    };
                    if r.format != 2
                        || !valid_id(&r.id)
                        || expected.is_none()
                        || path.file_stem().and_then(|s| s.to_str()) != expected
                        || !r.original().is_absolute()
                        || r.original_bytes.contains(&0)
                    {
                        return Err(io::Error::other("invalid journal fields"));
                    }
                }
                Ok(entries)
            })();
            match read {
                Ok(entries) => records.extend(entries),
                Err(e) => {
                    eprintln!("srm: damaged journal retained {}: {e}", path.display());
                    damaged = true;
                }
            }
        }
        records.sort_by(|a, b| a.deleted_at.cmp(&b.deleted_at).then(a.id.cmp(&b.id)));
        Ok((records, damaged))
    }

    pub fn trash(&mut self, source: &Path, days: i64) -> io::Result<String> {
        interrupted()?;
        self.check_operand(source)?;
        let backup_parent = self.backup_directory(source)?;
        let m = fs::symlink_metadata(source)?;
        let kind = if m.is_dir() {
            "directory"
        } else if m.file_type().is_symlink() {
            "symlink"
        } else {
            "file"
        };
        let prefix = match kind {
            "directory" => "d",
            "symlink" => "l",
            _ => "f",
        };
        let mut random = [0u8; 8];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        let id = format!("{prefix}_{:016x}", u64::from_ne_bytes(random));
        if exists(&self.meta.join(format!("{id}.json")))? || exists(&backup_parent.join(&id))? {
            return Err(io::Error::other("ID collision; retry the command"));
        }
        let mut record = Record {
            format: 2,
            id: id.clone(),
            original_bytes: source.as_os_str().as_bytes().to_vec(),
            original_path: source.to_string_lossy().into_owned(),
            deleted_at: Utc::now().timestamp(),
            expire_days: days,
            kind: kind.into(),
            size_bytes: (!m.is_dir()).then_some(m.len()),
            device: m.dev(),
            inode: m.ino(),
            state: State::Prepared,
            restore_target_bytes: None,
            remote: if self.settings.backend != Backend::Local {
                Some(Receipt::pending(self.settings.remote.clone().unwrap()))
            } else {
                None
            },
            journal_group: None,
            local_parent_bytes: (backup_parent != self.files)
                .then(|| backup_parent.as_os_str().as_bytes().to_vec()),
        };
        self.save(&record)?;
        self.audit("delete_begin", &record)?;
        let destination = backup_parent.join(&id);
        if self.settings.backend == Backend::RemoteSync {
            // Synchronous policy keeps the original pathname until the remote
            // archive is durably acknowledged. It necessarily pays copy/IO cost.
            let before = snapshot(source)?;
            record.state = State::Copying;
            self.save(&record)?;
            copy_complete(source, &destination)?;
            record.state = State::Trashed;
            self.save(&record)?;
            if let Err(e) = self.upload_record(&mut record) {
                self.audit("remote_sync_failed_source_retained", &record)?;
                return Err(e);
            }
            if snapshot(source)? != before {
                self.audit("remote_sync_source_changed_retained", &record)?;
                return Err(io::Error::other(
                    "source changed while awaiting remote acknowledgement; original retained",
                ));
            }
            remove_entry(source)?;
            sync_dir(source.parent().unwrap())?;
            self.audit("delete_complete", &record)?;
            return Ok(id);
        }
        match rename_new(source, &destination) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
                record.state = State::Copying;
                self.save(&record)?;
                copy_complete(source, &destination)?;
                record.state = State::Copied;
                self.save(&record)?;
                interrupted()?;
                let current = fs::symlink_metadata(source)?;
                if current.dev() != m.dev()
                    || current.ino() != m.ino()
                    || current.mtime() != m.mtime()
                    || current.mtime_nsec() != m.mtime_nsec()
                {
                    return Err(io::Error::other(
                        "source changed during copy; both copies retained, inspect srm doctor",
                    ));
                }
                record.state = State::RemovingSource;
                self.save(&record)?;
                remove_entry(source)?;
            }
            Err(e) => {
                self.audit("delete_failed", &record)?;
                self.forget(&record)?;
                return Err(e);
            }
        }
        sync_dir(source.parent().unwrap())?;
        sync_dir(&backup_parent)?;
        record.state = State::Trashed;
        self.save(&record)?;
        self.audit("delete_complete", &record)?;
        Ok(id)
    }

    fn resolve(&self, query: &str) -> io::Result<Record> {
        if !valid_id(query) {
            return Err(io::Error::other("invalid ID (use srm list)"));
        }
        let (records, _) = self.records()?;
        let exact = records.iter().find(|r| r.id == query);
        if let Some(r) = exact {
            return Ok(r.clone());
        }
        let matches: Vec<_> = records
            .into_iter()
            .filter(|r| r.id.starts_with(query))
            .collect();
        if matches.len() != 1 {
            return Err(io::Error::other(
                "ID is missing or ambiguous; use more characters",
            ));
        }
        Ok(matches[0].clone())
    }

    pub fn restore(
        &mut self,
        id: &str,
        target: Option<&Path>,
        force: bool,
    ) -> io::Result<Option<PathBuf>> {
        let mut record = self.resolve(id)?;
        let _lease = self.item_lock(&record.id)?;
        if record.state == State::RemoteOnly {
            let receipt = record
                .remote
                .as_ref()
                .ok_or_else(|| io::Error::other("missing remote receipt"))?;
            let staging = self.base.join("staging");
            private_dir(&staging)?;
            // A crash during eviction can leave an incomplete local cache.
            let old = self.backup(&record)?;
            let mut caches = vec![old.clone()];
            if old != self.files.join(&record.id) {
                caches.push(self.files.join(&record.id));
            }
            for cache in caches {
                if exists(&cache)? {
                    rename_new(
                        &cache,
                        &cache.with_file_name(format!(
                            ".quarantine-{}-{}",
                            record.id,
                            random_suffix()?
                        )),
                    )?;
                    sync_dir(cache.parent().unwrap())?;
                }
            }
            FileExt::unlock(&self._lock)?;
            let fetched = remote::fetch(receipt, &record.id, &self.files, &staging);
            FileExt::lock_exclusive(&self._lock)?;
            fetched?;
            record.state = State::Trashed;
            record.local_parent_bytes = None;
            record.remote.as_mut().unwrap().evicted = false;
            self.save(&record)?;
        }
        if record.state != State::Trashed {
            return Err(io::Error::other(
                "incomplete operation; inspect srm doctor before recovery",
            ));
        }
        let source = self.backup(&record)?;
        if !exists(&source)? {
            return Err(io::Error::other("trash entry missing; journal retained"));
        }
        let original = record.original();
        let dest = target.map_or_else(
            || original.clone(),
            |p| {
                if p.is_dir() {
                    p.join(original.file_name().unwrap())
                } else {
                    p.to_path_buf()
                }
            },
        );
        let dest = if dest.is_absolute() {
            dest
        } else {
            std::env::current_dir()?.join(dest)
        };
        // Parent creation is useful after a recursive interactive delete. Check
        // containment before creating anything and again after resolving parents.
        if dest.starts_with(&self.base) {
            return Err(io::Error::other("restore target is inside trash"));
        }
        fs::create_dir_all(
            dest.parent()
                .ok_or_else(|| io::Error::other("invalid restore target"))?,
        )?;
        let destination = absolute(&dest)?;
        self.check_operand(&destination)?;
        if exists(&destination)? {
            if !force
                && !confirm(&format!(
                    "replace '{}' (existing entry will be trashed)?",
                    destination.display()
                ))
            {
                return Ok(None);
            }
            self.trash(&destination, 7)?;
        }
        interrupted()?;
        record.state = State::Restoring;
        record.restore_target_bytes = Some(destination.as_os_str().as_bytes().to_vec());
        self.save(&record)?;
        self.audit("restore_begin", &record)?;
        match rename_new(&source, &destination) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
                // Copy into a staging sibling and atomically publish only after it
                // is complete. A failed/interrupted copy never consumes the backup.
                let stage = destination
                    .parent()
                    .unwrap()
                    .join(format!(".srm-restore-{}", record.id));
                copy_complete(&source, &stage)?;
                interrupted()?;
                rename_new(&stage, &destination)?;
                sync_dir(destination.parent().unwrap())?;
                remove_entry(&source)?;
            }
            Err(e) => {
                record.state = State::Trashed;
                record.restore_target_bytes = None;
                self.save(&record)?;
                return Err(e);
            }
        }
        sync_dir(destination.parent().unwrap())?;
        sync_dir(source.parent().unwrap())?;
        self.audit("restore_complete", &record)?;
        self.forget(&record)?;
        // rename/cp preserve metadata. Never chmod a symlink or its target.
        Ok(Some(destination))
    }

    pub fn list(
        &self,
        expired: bool,
        verbose: bool,
        json_output: bool,
        scan: bool,
    ) -> io::Result<()> {
        let (records, damaged) = self.records()?;
        let mut rows = vec![];
        for mut r in records {
            if expired && !r.expired() {
                continue;
            }
            let path = self.backup(&r)?;
            let present = exists(&path)?;
            if scan && present {
                r.size_bytes = Some(measure(&path)?);
            }
            let row = json!({"id": r.id, "original_path": r.original_path, "original_bytes": r.original_bytes,
                "kind": r.kind, "state": r.state, "deleted_at": r.deleted_at, "expire_days": r.expire_days,
                "expired": r.expired(), "size_bytes": r.size_bytes, "present": present, "trash_path": path,
                "remote": r.remote, "journal_group": r.journal_group});
            if !json_output {
                println!(
                    "{}\t{}\t{}\t{}",
                    r.id,
                    if r.expired() { "expired" } else { "active" },
                    r.size_bytes.map_or_else(
                        || "unmeasured".into(),
                        |s| bytesize::ByteSize(s).to_string()
                    ),
                    r.original_path
                );
                if verbose || r.state != State::Trashed || !present {
                    println!("  {row}");
                }
            }
            rows.push(row);
        }
        if json_output {
            println!("{}", serde_json::to_string_pretty(&rows)?);
        }
        if damaged {
            return Err(io::Error::other("damaged journals found; run srm doctor"));
        }
        Ok(())
    }

    fn clean_group(
        &mut self,
        group: &str,
        ids: &std::collections::HashSet<String>,
    ) -> io::Result<bool> {
        let journal = self.meta.join(format!("{group}.json"));
        let mut records: Vec<Record> = serde_json::from_reader(
            OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&journal)?,
        )?;
        let mut leases = Vec::new();
        let mut selected = Vec::new();
        let mut success = true;
        for r in &mut records {
            if !ids.contains(&r.id) {
                continue;
            }
            leases.push(self.item_lock(&r.id)?);
            let path = self.backup(r)?;
            if !exists(&path)? && r.state != State::Purging {
                eprintln!("srm: missing backup {}, journal retained", r.id);
                success = false;
                continue;
            }
            r.state = State::Purging;
            selected.push(r.clone());
        }
        atomic_write(&journal, &serde_json::to_vec(&records)?)?;
        self.audit_batch("batch_purge_begin", &selected)?;
        FileExt::unlock(&self._lock)?;
        let mut removed = std::collections::HashSet::new();
        let mut parents = std::collections::BTreeSet::new();
        for r in &selected {
            if INTERRUPTED.load(Ordering::Relaxed) {
                success = false;
                break;
            }
            let path = self.backup(r)?;
            match exists(&path)
                .and_then(|present| if present { remove_entry(&path) } else { Ok(()) })
            {
                Ok(()) => {
                    removed.insert(r.id.clone());
                    parents.insert(path.parent().unwrap().to_path_buf());
                }
                Err(e) => {
                    eprintln!("srm: cannot purge {}: {e}", r.id);
                    success = false;
                }
            }
        }
        let synced = parents.iter().try_for_each(|p| sync_dir(p));
        FileExt::lock_exclusive(&self._lock)?;
        synced?;
        // Other records in this group can have changed while IO ran unlocked.
        let mut fresh: Vec<Record> = serde_json::from_reader(
            OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&journal)?,
        )?;
        fresh.retain(|r| !removed.contains(&r.id));
        if fresh.is_empty() {
            fs::remove_file(&journal)?;
            sync_dir(&self.meta)?;
        } else {
            atomic_write(&journal, &serde_json::to_vec(&fresh)?)?;
        }
        selected.retain(|r| removed.contains(&r.id));
        self.audit_batch("batch_purge_complete", &selected)?;
        for r in selected {
            println!("permanently removed {}", r.id);
        }
        Ok(success)
    }

    pub fn clean(&mut self, all: bool, dry_run: bool) -> io::Result<bool> {
        let (records, damaged) = self.records()?;
        let mut success = !damaged;
        let mut groups: std::collections::BTreeMap<String, std::collections::HashSet<String>> =
            std::collections::BTreeMap::new();
        if !dry_run {
            for r in &records {
                if r.remote.is_none()
                    && matches!(r.state, State::Trashed | State::Purging)
                    && (all || r.expired() || r.state == State::Purging)
                {
                    if let Some(group) = &r.journal_group {
                        groups
                            .entry(group.clone())
                            .or_default()
                            .insert(r.id.clone());
                    }
                }
            }
        }
        for (group, ids) in &groups {
            match self.clean_group(group, ids) {
                Ok(ok) => success &= ok,
                Err(e) => {
                    eprintln!("srm: batch cleanup retained journal {group}: {e}");
                    success = false;
                }
            }
        }
        for mut r in records {
            interrupted()?;
            if r.journal_group
                .as_ref()
                .is_some_and(|g| groups.get(g).is_some_and(|ids| ids.contains(&r.id)))
            {
                continue;
            }
            r = match self.resolve(&r.id) {
                Ok(current) => current,
                Err(_) => continue,
            };
            if !matches!(r.state, State::Trashed | State::Purging | State::RemoteOnly) {
                eprintln!("srm: retaining incomplete journal {}; run srm doctor", r.id);
                success = false;
                continue;
            }
            if r.state != State::Purging && !all && !r.expired() {
                continue;
            }
            if r.remote.as_ref().is_some_and(|r| !r.uploaded) {
                eprintln!("srm: retaining {} until remote upload is verified", r.id);
                success = false;
                continue;
            }
            if dry_run {
                println!("would permanently remove {} ({})", r.id, r.original_path);
                continue;
            }
            let attempt = (|| -> io::Result<()> {
                let _lease = self.item_lock(&r.id)?;
                let path = self.backup(&r)?;
                if !exists(&path)? && r.state != State::Purging && r.remote.is_none() {
                    return Err(io::Error::other("trash entry missing; journal retained"));
                }
                r.state = State::Purging;
                self.save(&r)?;
                self.audit("purge_begin", &r)?;
                FileExt::unlock(&self._lock)?;
                let purged = (|| -> io::Result<()> {
                    if let Some(remote) = &r.remote {
                        remote::request(&remote.target, "delete", Some(&remote.object), None)?;
                    }
                    if exists(&path)? {
                        remove_entry(&path)?;
                        sync_dir(path.parent().unwrap())?;
                    }
                    Ok(())
                })();
                FileExt::lock_exclusive(&self._lock)?;
                purged?;
                self.audit("purge_complete", &r)?;
                self.forget(&r)?;
                Ok(())
            })();
            match attempt {
                Ok(()) => println!("permanently removed {}", r.id),
                Err(e) => {
                    eprintln!("srm: cannot clean {}: {e}", r.id);
                    success = false;
                }
            }
        }
        Ok(success)
    }

    pub fn recover(&mut self, id: &str) -> io::Result<()> {
        let mut r = self.resolve(id)?;
        let _lease = self.item_lock(&r.id)?;
        let backup = self.backup(&r)?;
        match r.state {
            State::Trashed => return Ok(()),
            State::Prepared if exists(&backup)? && !exists(&r.original())? => {
                let m = fs::symlink_metadata(&backup)?;
                if m.dev() != r.device || m.ino() != r.inode {
                    return Err(io::Error::other("backup identity does not match journal"));
                }
            }
            State::Copied | State::RemovingSource if exists(&backup)? => {}
            State::Restoring if exists(&backup)? => {}
            _ => return Err(io::Error::other(
                "cannot automatically reconcile this phase; inspect srm doctor (no data changed)",
            )),
        }
        // Only phases known to contain a complete backup can become restorable.
        // Any remaining original or published restore target is kept intact.
        r.state = State::Trashed;
        r.restore_target_bytes = None;
        self.save(&r)?;
        self.audit("journal_recovered", &r)
    }

    pub fn doctor(&self) -> io::Result<bool> {
        let (records, damaged) = self.records()?;
        let mut healthy = !damaged;
        let ids: std::collections::HashSet<_> = records.iter().map(|r| r.id.as_str()).collect();
        for r in &records {
            let backup = self.backup(r)?;
            if r.state != State::RemoteOnly && (r.state != State::Trashed || !exists(&backup)?) {
                healthy = false;
                println!(
                    "{}: {:?}\n  source: {}\n  backup: {}\n  journal: {}",
                    r.id,
                    r.state,
                    r.original_path,
                    backup.display(),
                    self.meta
                        .join(format!(
                            "{}.json",
                            r.journal_group.as_ref().unwrap_or(&r.id)
                        ))
                        .display()
                );
                if let Some(bytes) = &r.restore_target_bytes {
                    let target = PathBuf::from(OsString::from_vec(bytes.clone()));
                    println!(
                        "  restore target: {}\n  restore staging: {}",
                        target.display(),
                        target
                            .parent()
                            .unwrap_or(Path::new("/"))
                            .join(format!(".srm-restore-{}", r.id))
                            .display()
                    );
                }
            }
        }
        let directories = std::iter::once(self.files.clone())
            .chain(self.volume_roots.iter().map(|root| root.join("files")));
        for directory in directories {
            let entries = match fs::read_dir(&directory) {
                Ok(e) => e,
                Err(e) => {
                    println!("volume unavailable {}: {e}", directory.display());
                    healthy = false;
                    continue;
                }
            };
            for entry in entries {
                let entry = entry?;
                if !entry
                    .file_name()
                    .to_str()
                    .is_some_and(|id| ids.contains(id))
                {
                    println!("orphan retained: {}", entry.path().display());
                    healthy = false;
                }
            }
        }
        if healthy {
            println!("trash journals are consistent: {} entries", records.len());
        } else {
            println!(
                "No data was changed. Inspect source, backup and journal before manual recovery."
            );
        }
        Ok(healthy)
    }
}

pub(crate) fn valid_id(id: &str) -> bool {
    id.len() >= 3
        && id.len() <= 18
        && matches!(id.as_bytes()[0], b'f' | b'd' | b'l')
        && id.as_bytes()[1] == b'_'
        && id.as_bytes()[2..].iter().all(|b| b.is_ascii_hexdigit())
}

fn valid_group(s: &str) -> bool {
    s.len() == 22
        && s.starts_with("batch_")
        && s.as_bytes()[6..].iter().all(|b| b.is_ascii_hexdigit())
}

pub(crate) fn random_suffix() -> io::Result<String> {
    let mut random = [0u8; 8];
    File::open("/dev/urandom")?.read_exact(&mut random)?;
    Ok(format!("{:016x}", u64::from_ne_bytes(random)))
}

pub(crate) fn sync_fs(path: &Path) -> io::Result<()> {
    let file = File::open(path)?;
    if unsafe { libc::syncfs(file.as_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
