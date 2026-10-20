use crate::{
    confirm,
    store::{absolute, Store},
    INTERRUPTED,
};
use std::ffi::OsString;
use std::fs;
use std::io::{self, IsTerminal};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

#[derive(Clone, Copy, PartialEq)]
enum Interactive {
    Never,
    Auto,
    Once,
    Always,
}

struct Options {
    force: bool,
    interactive: Interactive,
    recursive: bool,
    dir: bool,
    verbose: bool,
    one_fs: bool,
    preserve_root: bool,
    preserve_all: bool,
    days: i64,
    legacy: bool,
    paths: Vec<PathBuf>,
}

const HELP: &str = "Usage: rm [OPTION]... [FILE]...
Recoverably remove files with srm (permanent rm remains available as /bin/rm).
  -f, --force             ignore missing operands/files, never prompt
  -i                     prompt before every removal
  -I                     prompt once if recursive or more than three operands
      --interactive[=WHEN]  never, once, always (default when omitted: always)
  -r, -R, --recursive     remove directories recursively
  -d, --dir              remove empty directories
  -v, --verbose          explain each removal
      --one-file-system  skip directories on different filesystems
      --preserve-root[=all]  protect / (default), optionally operand mount points
      --no-preserve-root disable the root check (trash containment checks still apply)
      --                 end options, including for names starting with '-'
      --help             display this help
      --version          display version
Manage trash using srm list / restore / clean / empty / doctor.
SRM_HOME overrides the private trash location. Deletions expire after 7 days.
Recovery is an intentional semantic difference from GNU rm. See README.md.";

const HELP_ZH: &str = "用法：srm [选项]... [文件]...
将文件移入可恢复存储；系统 /usr/bin/rm 仍可用于原生永久删除。
  -f, --force             忽略不存在的文件，不询问
  -i                     每次删除前询问
  -I                     递归或超过三个操作数时询问一次
      --interactive[=WHEN]  never（不询问）、once（一次）、always（逐项）
  -r, -R, --recursive     递归处理目录
  -d, --dir              允许删除空目录
  -v, --verbose          显示操作详情
      --one-file-system  跳过不同文件系统的子目录
      --preserve-root[=all]  保护根目录，all 额外保护挂载点操作数
      --no-preserve-root 关闭根目录特判，回收站保护仍然有效
      --                 后续全部作为文件名
      --help             显示帮助
      --version          显示版本
管理：srm list / restore / clean / empty / doctor
语言：srm --lang auto|zh|en --rm --help
默认保留时间来自配置；到期不会自行销毁，需执行或启用清理。
可恢复删除与原生 rm 的永久删除语义不同。";

fn parse(args: Vec<OsString>, legacy: bool) -> Result<Option<Options>, String> {
    let mut o = Options {
        force: false,
        interactive: Interactive::Auto,
        recursive: legacy,
        dir: false,
        verbose: false,
        one_fs: false,
        preserve_root: true,
        preserve_all: false,
        days: crate::config::load()
            .map_err(|e| e.to_string())?
            .expire_days,
        legacy,
        paths: vec![],
    };
    let mut operands = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        let b = arg.as_bytes();
        if operands || !b.starts_with(b"-") || b == b"-" {
            o.paths.push(arg.into());
            if std::env::var_os("POSIXLY_CORRECT").is_some() {
                operands = true;
            }
            continue;
        }
        if b == b"--" {
            operands = true;
            continue;
        }
        if legacy && b.starts_with(b"-d") && !b.starts_with(b"--") {
            let v = if b.len() == 2 {
                iter.next().ok_or("missing expiration days")?
            } else {
                OsString::from(std::str::from_utf8(&b[2..]).map_err(|_| "invalid expiration days")?)
            };
            o.days = v
                .to_str()
                .and_then(|s| s.parse().ok())
                .filter(|d| (0..=36500).contains(d))
                .ok_or("expiration days must be between 0 and 36500")?;
            continue;
        }
        if b.starts_with(b"--") {
            let text = arg.to_str().ok_or("invalid option encoding")?;
            let (raw, value) = text[2..]
                .split_once('=')
                .map_or((&text[2..], None), |(k, v)| (k, Some(v)));
            let names = [
                "force",
                "interactive",
                "recursive",
                "dir",
                "verbose",
                "one-file-system",
                "preserve-root",
                "no-preserve-root",
                "help",
                "version",
                "expire-days",
            ];
            let name = if names.contains(&raw) {
                raw
            } else {
                let matching: Vec<_> = names
                    .iter()
                    .filter(|n| n.starts_with(raw) && (legacy || **n != "expire-days"))
                    .collect();
                if matching.len() != 1 {
                    return Err(format!("unrecognized or ambiguous option '--{raw}'"));
                }
                *matching[0]
            };
            if value.is_some() && !matches!(name, "interactive" | "preserve-root" | "expire-days") {
                return Err(format!("option '--{name}' does not allow an argument"));
            }
            match name {
                "help" => {
                    println!("{}", crate::i18n::text(HELP, HELP_ZH));
                    if legacy {
                        println!("{}", crate::i18n::text(
                            "Legacy del/delete mode: directories are recursive by default; -d/--expire-days DAYS sets retention (0..36500). -d does NOT mean empty-directory mode here.",
                            "del/delete 旧式模式：默认递归处理目录；-d/--expire-days 天数 设置保留期限（0..36500）。此模式的 -d 不是空目录选项。"));
                    }
                    return Ok(None);
                }
                "version" => {
                    println!("rm (safe-srm) {}", env!("CARGO_PKG_VERSION"));
                    return Ok(None);
                }
                "force" => {
                    o.force = true;
                    o.interactive = Interactive::Never;
                }
                "interactive" => {
                    o.interactive = match value.unwrap_or("always") {
                        "never" | "no" | "none" => Interactive::Never,
                        "once" => Interactive::Once,
                        "always" | "yes" => Interactive::Always,
                        v => return Err(format!("invalid interactive mode '{v}'")),
                    };
                    if o.interactive != Interactive::Never {
                        o.force = false;
                    }
                }
                "recursive" => o.recursive = true,
                "dir" => o.dir = true,
                "verbose" => o.verbose = true,
                "one-file-system" => o.one_fs = true,
                "preserve-root" => {
                    if value.is_some_and(|v| v != "all") {
                        return Err("--preserve-root only accepts 'all'".into());
                    }
                    o.preserve_root = true;
                    o.preserve_all = value == Some("all");
                }
                "no-preserve-root" => {
                    o.preserve_root = false;
                    o.preserve_all = false;
                }
                "expire-days" if legacy => {
                    let v = value
                        .map(OsString::from)
                        .or_else(|| iter.next())
                        .ok_or("missing expiration days")?;
                    o.days = v
                        .to_str()
                        .and_then(|s| s.parse().ok())
                        .filter(|d| (0..=36500).contains(d))
                        .ok_or("expiration days must be between 0 and 36500")?;
                }
                _ => return Err(format!("unrecognized option '--{raw}'")),
            }
        } else {
            for c in &b[1..] {
                match c {
                    b'f' => {
                        o.force = true;
                        o.interactive = Interactive::Never;
                    }
                    b'i' => {
                        o.force = false;
                        o.interactive = Interactive::Always;
                    }
                    b'I' => {
                        o.force = false;
                        o.interactive = Interactive::Once;
                    }
                    b'r' | b'R' => o.recursive = true,
                    b'd' => o.dir = true,
                    b'v' => o.verbose = true,
                    _ => return Err(format!("invalid option -- '{}'", char::from(*c))),
                }
            }
        }
    }
    if o.paths.is_empty() && !o.force {
        return Err("missing operand".into());
    }
    if legacy
        && o.paths.len() > 1
        && o.paths[0]
            .to_str()
            .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    {
        eprintln!("{}", crate::i18n::text(
            "Note: a bare number is a filename. For 3-day retention use: srm del -d 3 FILE. No numeric filename is reinterpreted.",
            "提示：单独的数字会被当成文件名。保留 3 天请用：srm del -d 3 文件。程序不会把数字文件名自动改解释为天数。"));
    }
    Ok(Some(o))
}

fn writable(path: &Path) -> bool {
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::faccessat(libc::AT_FDCWD, c.as_ptr(), libc::W_OK, libc::AT_EACCESS) == 0 }
}

fn remove(
    path: &Path,
    o: &Options,
    store: &mut Store,
    top_dev: u64,
    depth: usize,
) -> io::Result<bool> {
    if INTERRUPTED.load(Ordering::Relaxed) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "interrupted"));
    }
    if depth > 512 {
        return Err(io::Error::other("directory depth exceeds safety limit"));
    }
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e)
            if o.force
                && matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
        {
            return Ok(true)
        }
        Err(e) => return Err(e),
    };
    store.check_operand(path)?;
    if meta.is_dir() {
        if !o.recursive && !o.dir {
            return Err(io::Error::other("is a directory (use -r or -d)"));
        }
        if o.one_fs && meta.dev() != top_dev {
            return Err(io::Error::other(
                "skipping directory on a different filesystem",
            ));
        }
        if !o.recursive && fs::read_dir(path)?.next().is_some() {
            return Err(io::Error::other("directory not empty"));
        }
        // Selective prompts and mount boundaries require per-entry processing.
        if o.interactive == Interactive::Always
            || o.one_fs
            || (!store.fast_directories() && !bulk_removable(path)?)
            || store.has_mount_at_or_below(path)
            || (o.interactive == Interactive::Auto && io::stdin().is_terminal())
        {
            let mut entries = fs::read_dir(path)?.peekable();
            if entries.peek().is_some()
                && o.interactive == Interactive::Always
                && !confirm(&format!("descend into directory '{}' ?", path.display()))
            {
                return Ok(false);
            }
            let mut failed = false;
            for entry in entries {
                let entry = entry?;
                match remove(&entry.path(), o, store, top_dev, depth + 1) {
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("srm: cannot remove '{}': {e}", entry.path().display());
                        failed = true;
                    }
                }
            }
            if fs::read_dir(path)?.next().is_some() {
                return if failed {
                    Err(io::Error::other("directory has unremoved entries"))
                } else {
                    Ok(false)
                };
            }
            if failed {
                return Err(io::Error::other("some entries could not be removed"));
            }
        }
    }
    let prompt = o.interactive == Interactive::Always
        || (o.interactive == Interactive::Auto
            && io::stdin().is_terminal()
            && !meta.file_type().is_symlink()
            && !writable(path));
    if prompt && !confirm(&format!("remove '{}' ?", path.display())) {
        return Ok(false);
    }
    let id = store.trash(path, o.days)?;
    if o.verbose {
        println!("removed '{}' (trash ID {id})", path.display());
    } else if o.legacy {
        println!("{} -> {id}", path.display());
    }
    Ok(true)
}

fn bulk_removable(path: &Path) -> io::Result<bool> {
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if INTERRUPTED.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "interrupted"));
        }
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return Ok(false),
        };
        let parent = fs::symlink_metadata(&dir)?;
        let can_write = writable(&dir);
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => return Ok(false),
            };
            if !can_write {
                return Ok(false);
            }
            let m = match fs::symlink_metadata(entry.path()) {
                Ok(m) => m,
                Err(_) => return Ok(false),
            };
            let uid = unsafe { libc::geteuid() };
            if parent.mode() & 0o1000 != 0 && uid != 0 && uid != parent.uid() && uid != m.uid() {
                return Ok(false);
            }
            if m.is_dir() {
                stack.push(entry.path());
            }
        }
    }
    Ok(true)
}

pub fn run(args: Vec<OsString>, legacy: bool) -> Result<i32, Box<dyn std::error::Error>> {
    let Some(o) = parse(args, legacy).map_err(io::Error::other)? else {
        return Ok(0);
    };
    if o.paths.is_empty() {
        return Ok(0);
    }
    if o.interactive == Interactive::Once
        && (o.recursive || o.paths.len() > 3)
        && !confirm(&format!(
            "remove {} argument(s){}?",
            o.paths.len(),
            if o.recursive { " recursively" } else { "" }
        ))
    {
        return Ok(0);
    }
    let mut store = Store::open()?;
    let may_batch = o.interactive != Interactive::Always
        && !o.preserve_all
        && o.paths.iter().all(|p| {
            !matches!(
                p.as_os_str()
                    .as_bytes()
                    .rsplit(|b| *b == b'/')
                    .find(|p| !p.is_empty())
                    .unwrap_or_default(),
                b"." | b".."
            )
        })
        && !(o.interactive == Interactive::Auto && io::stdin().is_terminal());
    if may_batch {
        let paths = o
            .paths
            .iter()
            .map(|p| absolute(p))
            .collect::<io::Result<Vec<_>>>();
        if let Ok(paths) = paths {
            if store.can_batch(&paths) {
                let mut failed = false;
                for chunk in paths.chunks(store.batch_size()) {
                    let results = store.trash_batch(chunk, o.days)?;
                    for (path, result) in chunk.iter().zip(results) {
                        match result {
                            Ok(id) if o.verbose || o.legacy => {
                                println!("removed '{}' (trash ID {id})", path.display())
                            }
                            Ok(_) => {}
                            Err(e) => {
                                eprintln!("srm: cannot remove '{}': {e}", path.display());
                                failed = true;
                            }
                        }
                    }
                    if INTERRUPTED.load(Ordering::Relaxed) {
                        return Ok(130);
                    }
                }
                if let Err(e) = store.launch_worker() {
                    eprintln!("srm: upload queue retained: {e}");
                }
                return Ok(i32::from(failed));
            }
        }
    }
    let root = fs::metadata("/")?;
    let mut failed = false;
    for input in &o.paths {
        if INTERRUPTED.load(Ordering::Relaxed) {
            return Ok(130);
        }
        let attempt = (|| -> io::Result<()> {
            let bytes = input.as_os_str().as_bytes();
            let last = bytes
                .rsplit(|b| *b == b'/')
                .find(|p| !p.is_empty())
                .unwrap_or_default();
            if matches!(last, b"." | b"..") {
                return Err(io::Error::other("refusing to remove '.' or '..'"));
            }
            let path = match absolute(input) {
                Ok(p) => p,
                Err(e)
                    if o.force
                        && matches!(
                            e.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                        ) =>
                {
                    return Ok(())
                }
                Err(e) => return Err(e),
            };
            let meta = match fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(e)
                    if o.force
                        && matches!(
                            e.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                        ) =>
                {
                    return Ok(())
                }
                Err(e) => return Err(e),
            };
            if o.preserve_root
                && meta.is_dir()
                && meta.dev() == root.dev()
                && meta.ino() == root.ino()
            {
                return Err(io::Error::other("refusing to remove root directory"));
            }
            if o.preserve_all
                && !meta.file_type().is_symlink()
                && fs::metadata(path.parent().unwrap_or(Path::new("/")))?.dev() != meta.dev()
            {
                return Err(io::Error::other(
                    "operand is on a different filesystem from its parent",
                ));
            }
            remove(&path, &o, &mut store, meta.dev(), 0)?;
            Ok(())
        })();
        if let Err(e) = attempt {
            eprintln!("srm: cannot remove '{}': {e}", input.display());
            failed = true;
        }
    }
    if let Err(e) = store.launch_worker() {
        eprintln!("srm: automatic upload worker could not start; queue retained: {e}");
    }
    Ok(if INTERRUPTED.load(Ordering::Relaxed) {
        130
    } else {
        i32::from(failed)
    })
}
