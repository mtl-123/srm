use crate::{config::Remote, store, INTERRUPTED};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

const HELPER: &str = include_str!("../remote/receiver.py");

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Receipt {
    pub target: Remote,
    #[serde(default)]
    pub object: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub uploaded: bool,
    #[serde(default)]
    pub evicted: bool,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl Receipt {
    pub fn pending(target: Remote) -> Self {
        Self {
            target,
            object: String::new(),
            sha256: String::new(),
            uploaded: false,
            evicted: false,
            attempts: 0,
            last_error: None,
        }
    }
}

fn quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn ssh(
    target: &Remote,
    action: &str,
    object: Option<&str>,
    digest: Option<&str>,
) -> io::Result<Command> {
    target.validate()?;
    let mut command = Command::new("/usr/bin/ssh");
    command
        .args([
            "-T",
            "-oBatchMode=yes",
            "-oStrictHostKeyChecking=yes",
            "-oConnectTimeout=10",
            "-oServerAliveInterval=15",
            "-oServerAliveCountMax=3",
            "-p",
        ])
        .arg(target.port.to_string());
    if let Some(path) = &target.identity_file {
        command.arg("-i").arg(path).arg("-oIdentitiesOnly=yes");
    }
    if let Some(path) = &target.known_hosts_file {
        command
            .arg("-o")
            .arg(format!("UserKnownHostsFile={}", path.display()));
    }
    let mut arguments = vec![
        "python3".to_string(),
        "-c".into(),
        HELPER.into(),
        action.into(),
        target.directory.clone(),
        target.namespace.clone(),
    ];
    if let Some(object) = object {
        if !valid_object(object) {
            return Err(io::Error::other("invalid remote object ID"));
        }
        arguments.push(object.into());
    }
    if let Some(digest) = digest {
        arguments.push(digest.into());
    }
    command
        .arg("--")
        .arg(format!("{}@{}", target.user, target.host))
        .arg(
            arguments
                .iter()
                .map(|s| quoted(s))
                .collect::<Vec<_>>()
                .join(" "),
        );
    Ok(command)
}

// Child guard reaps killed processes on every error path. The watchdog can break
// blocked pipe I/O after Ctrl+C or a configured deadline, without waiting for SSH.
struct Process {
    child: Arc<Mutex<Child>>,
    done: Arc<AtomicBool>,
    watcher: Option<std::thread::JoinHandle<()>>,
}
impl Process {
    fn spawn(command: &mut Command, timeout: u64) -> io::Result<Self> {
        let child = Arc::new(Mutex::new(command.spawn()?));
        let guarded = child.clone();
        let done = Arc::new(AtomicBool::new(false));
        let finished = done.clone();
        let watcher = std::thread::spawn(move || {
            let start = std::time::Instant::now();
            while !finished.load(Ordering::Acquire) {
                if INTERRUPTED.load(Ordering::Relaxed) || start.elapsed().as_secs() >= timeout {
                    let mut child = guarded.lock().unwrap();
                    if matches!(child.try_wait(), Ok(None)) {
                        let _ = child.kill();
                    }
                    break;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
        });
        Ok(Self {
            child,
            done,
            watcher: Some(watcher),
        })
    }
    fn wait(&mut self) -> io::Result<()> {
        let status = loop {
            if let Some(status) = self.child.lock().unwrap().try_wait()? {
                break status;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        self.done.store(true, Ordering::Release);
        if let Some(w) = self.watcher.take() {
            let _ = w.join();
        }
        if !status.success() {
            return Err(io::Error::other(
                "remote transport or archive command failed (source retained)",
            ));
        }
        Ok(())
    }
    fn stdin(&self) -> std::process::ChildStdin {
        self.child.lock().unwrap().stdin.take().unwrap()
    }
    fn stdout(&self) -> std::process::ChildStdout {
        self.child.lock().unwrap().stdout.take().unwrap()
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        if let Some(w) = self.watcher.take() {
            let _ = w.join();
        }
        let mut child = self.child.lock().unwrap();
        if matches!(child.try_wait(), Ok(None)) {
            let _ = child.kill();
        }
        let _ = child.wait();
    }
}

pub fn request(
    target: &Remote,
    action: &str,
    object: Option<&str>,
    digest: Option<&str>,
) -> io::Result<Value> {
    let mut command = ssh(target, action, object, digest)?;
    command.stdin(Stdio::null()).stdout(Stdio::piped());
    let mut process = Process::spawn(&mut command, target.timeout_seconds)?;
    let mut output = Vec::new();
    process
        .stdout()
        .take(64 * 1024 * 1024)
        .read_to_end(&mut output)?;
    process.wait()?;
    serde_json::from_slice(&output).map_err(io::Error::other)
}

pub fn upload(
    target: &Remote,
    object: &str,
    source: &Path,
    metadata: &Value,
) -> io::Result<String> {
    let before = store::snapshot(source)?;
    if target.release_local_after_upload
        && !crate::config::root()
        && before.values().any(|s| s.uid != unsafe { libc::geteuid() })
    {
        return Err(io::Error::other(
            "cannot evict another user's file: restoration of its owner requires sudo",
        ));
    }
    if before
        .values()
        .any(|s| s.mode & libc::S_IFMT == libc::S_IFSOCK)
    {
        return Err(io::Error::other(
            "Unix sockets cannot be archived; local data retained",
        ));
    }
    let mut tar_cmd = Command::new("/bin/tar");
    tar_cmd
        .args([
            "--create",
            "--file=-",
            "--sparse",
            "--acls",
            "--xattrs",
            "--selinux",
            "--numeric-owner",
            "--directory",
        ])
        .arg(source.parent().unwrap())
        .arg("--")
        .arg(source.file_name().unwrap())
        .stdout(Stdio::piped())
        .stdin(Stdio::null());
    let mut tar = Process::spawn(&mut tar_cmd, target.timeout_seconds)?;
    let mut ssh_cmd = ssh(target, "receive", Some(object), None)?;
    ssh_cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut remote = Process::spawn(&mut ssh_cmd, target.timeout_seconds)?;
    let mut input = remote.stdin();
    serde_json::to_writer(&mut input, metadata)?;
    input.write_all(b"\n")?;
    let mut output = tar.stdout();
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 256 * 1024];
    loop {
        if INTERRUPTED.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "upload interrupted",
            ));
        }
        let n = output.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
        input.write_all(&buffer[..n])?;
    }
    drop(input);
    tar.wait()?;
    let mut reply = Vec::new();
    remote
        .stdout()
        .take(4 * 1024 * 1024)
        .read_to_end(&mut reply)?;
    remote.wait()?;
    let receipt: Value = serde_json::from_slice(&reply)?;
    let hash = format!("{:x}", digest.finalize());
    if receipt["sha256"].as_str() != Some(&hash) || store::snapshot(source)? != before {
        return Err(io::Error::other(
            "archive digest mismatch or source changed; local data retained",
        ));
    }
    let committed = request(target, "commit", Some(object), Some(&hash))?;
    if committed["sha256"].as_str() != Some(&hash) {
        return Err(io::Error::other("remote commit was not acknowledged"));
    }
    Ok(hash)
}

pub fn fetch(receipt: &Receipt, id: &str, files: &Path, staging: &Path) -> io::Result<()> {
    if !receipt.uploaded || receipt.sha256.len() != 64 {
        return Err(io::Error::other("no verified remote archive"));
    }
    let work = staging.join(format!("fetch-{}", store::random_suffix()?));
    fs::create_dir(&work)?;
    let archive = work.join("archive.tar");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&archive)?;
    let mut command = ssh(&receipt.target, "send", Some(&receipt.object), None)?;
    command.stdin(Stdio::null()).stdout(Stdio::piped());
    let mut process = Process::spawn(&mut command, receipt.target.timeout_seconds)?;
    let mut output = process.stdout();
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 256 * 1024];
    loop {
        if INTERRUPTED.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "fetch interrupted",
            ));
        }
        let n = output.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
        file.write_all(&buffer[..n])?;
    }
    process.wait()?;
    file.sync_all()?;
    if format!("{:x}", digest.finalize()) != receipt.sha256 {
        return Err(io::Error::other(
            "remote archive checksum mismatch; nothing restored",
        ));
    }
    let mut validate = Command::new("/usr/bin/python3");
    validate
        .args(["-c", HELPER, "validate-local"])
        .arg(&archive)
        .arg(id);
    Process::spawn(&mut validate, receipt.target.timeout_seconds)?.wait()?;
    let output_dir = work.join("unpacked");
    fs::create_dir(&output_dir)?;
    let mut command = Command::new("/bin/tar");
    command
        .args([
            "--extract",
            "--same-owner",
            "--acls",
            "--xattrs",
            "--xattrs-include=*",
            "--selinux",
            "--numeric-owner",
            "--same-permissions",
            "--file",
        ])
        .arg(&archive)
        .arg("--directory")
        .arg(&output_dir);
    Process::spawn(&mut command, receipt.target.timeout_seconds)?.wait()?;
    let entries = fs::read_dir(&output_dir)?.collect::<Result<Vec<_>, _>>()?;
    if entries.len() != 1 || entries[0].file_name() != id {
        return Err(io::Error::other(
            "unexpected archive contents; staging retained",
        ));
    }
    store::sync_fs(&output_dir)?;
    store::rename_new(&output_dir.join(id), &files.join(id))?;
    File::open(files)?.sync_all()?;
    fs::remove_dir_all(work)?;
    Ok(())
}

pub fn valid_object(s: &str) -> bool {
    s.len() == 35
        && std::str::from_utf8(&s.as_bytes()[..18]).is_ok_and(store::valid_id)
        && s.as_bytes()[18] == b'-'
        && s.as_bytes()[19..].iter().all(|b| b.is_ascii_hexdigit())
}
