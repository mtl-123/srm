use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    #[default]
    Local,
    RemoteAsync,
    RemoteSync,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub language: String,
    pub backend: Backend,
    pub local_store: Option<PathBuf>,
    pub volume_stores: Vec<PathBuf>,
    pub expire_days: i64,
    pub batch_size: usize,
    pub directory_strategy: DirectoryStrategy,
    pub remote: Option<Remote>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            backend: Backend::Local,
            local_store: None,
            volume_stores: Vec::new(),
            expire_days: 7,
            batch_size: 256,
            directory_strategy: DirectoryStrategy::Checked,
            remote: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DirectoryStrategy {
    #[default]
    Checked,
    Rename,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Remote {
    pub host: String,
    pub user: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub directory: String,
    pub namespace: String,
    #[serde(default)]
    pub identity_file: Option<PathBuf>,
    #[serde(default)]
    pub known_hosts_file: Option<PathBuf>,
    #[serde(default)]
    pub release_local_after_upload: bool,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}
fn default_port() -> u16 {
    22
}
fn default_timeout() -> u64 {
    86400
}

impl Remote {
    pub fn validate(&self) -> io::Result<()> {
        let token = |s: &str| {
            !s.is_empty()
                && !s.starts_with('-')
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        };
        if !token(&self.user)
            || !token(&self.namespace)
            || self.namespace == "."
            || self.namespace == ".."
            || self.host.is_empty()
            || self.host.starts_with('-')
            || !self
                .host
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b".:-[]_".contains(&c))
            || self.port == 0
            || !Path::new(&self.directory).is_absolute()
            || self.directory.as_bytes().contains(&0)
            || Path::new(&self.directory)
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            || self.timeout_seconds == 0
            || self.timeout_seconds > 604800
        {
            return Err(io::Error::other(
                "invalid remote host/user/namespace/directory/timeout configuration",
            ));
        }
        for p in [&self.identity_file, &self.known_hosts_file]
            .into_iter()
            .flatten()
        {
            if !p.is_absolute() {
                return Err(io::Error::other("SSH file paths must be absolute"));
            }
            if unsafe { libc::geteuid() } == 0 {
                trusted_root_path(p)?;
            }
        }
        Ok(())
    }
}

pub fn root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

pub fn path() -> io::Result<PathBuf> {
    if let Some(p) = CONFIG_PATH.get() {
        return Ok(p.clone());
    }
    if root() {
        return Ok(PathBuf::from("/etc/safe-srm/config.json"));
    }
    if let Some(p) = std::env::var_os("SRM_CONFIG") {
        return Ok(p.into());
    }
    if let Some(p) = std::env::var_os("XDG_CONFIG_HOME").filter(|p| Path::new(p).is_absolute()) {
        return Ok(PathBuf::from(p).join("safe-srm/config.json"));
    }
    Ok(
        PathBuf::from(std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is not set"))?)
            .join(".config/safe-srm/config.json"),
    )
}

pub fn trusted_root_path(path: &Path) -> io::Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(io::Error::other("root paths must be absolute"));
    }
    for part in path.ancestors() {
        match fs::symlink_metadata(part) {
            Ok(m) if m.uid() == 0 && m.mode() & 0o022 == 0 && !m.file_type().is_symlink() => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            _ => {
                return Err(io::Error::other(format!(
                    "root refuses untrusted path: {}",
                    part.display()
                )))
            }
        }
    }
    Ok(())
}

pub fn load() -> io::Result<Settings> {
    let path = path()?;
    if !path.is_absolute() {
        return Err(io::Error::other("configuration path must be absolute"));
    }
    if root() {
        trusted_root_path(&path)?;
    }
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(e) => return Err(e),
    };
    let m = file.metadata()?;
    if !m.is_file() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o022 != 0 {
        return Err(io::Error::other(
            "config must be user-owned and not writable by group/others",
        ));
    }
    let settings: Settings = serde_json::from_reader(file)?;
    if !matches!(settings.language.as_str(), "auto" | "zh" | "en") {
        return Err(io::Error::other("config language must be auto, zh or en"));
    }
    for p in &settings.volume_stores {
        if !p.is_absolute() {
            return Err(io::Error::other(
                "volume_stores must contain absolute paths",
            ));
        }
        if root() {
            trusted_root_path(p)?;
        }
    }
    if !(0..=36500).contains(&settings.expire_days) || !(1..=4096).contains(&settings.batch_size) {
        return Err(io::Error::other("invalid expire_days or batch_size"));
    }
    if settings.backend != Backend::Local && settings.remote.is_none() {
        return Err(io::Error::other(
            "remote backend requires remote configuration",
        ));
    }
    if let Some(remote) = &settings.remote {
        remote.validate()?;
    }
    Ok(settings)
}

pub fn initialize() -> io::Result<PathBuf> {
    let path = path()?;
    if root() {
        trusted_root_path(&path)?;
    }
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| io::Error::other("invalid config path"))?,
    )?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)?;
    let machine = fs::read_to_string("/etc/machine-id").unwrap_or_else(|_| "my-machine".into());
    let settings = Settings {
        remote: Some(Remote {
            host: "storage.example.invalid".into(),
            user: "backup".into(),
            port: 22,
            directory: "/srv/safe-srm".into(),
            namespace: format!("{}-{}", machine.trim(), unsafe { libc::geteuid() }),
            identity_file: None,
            known_hosts_file: None,
            release_local_after_upload: false,
            timeout_seconds: 86400,
        }),
        ..Settings::default()
    };
    file.write_all(&serde_json::to_vec_pretty(&settings)?)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(path)
}

pub fn save_language(language: &str) -> io::Result<PathBuf> {
    use fs2::FileExt;
    let path = path()?;
    if !path.is_absolute() {
        return Err(io::Error::other("configuration path must be absolute"));
    }
    if root() {
        trusted_root_path(&path)?;
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("invalid config path"))?;
    fs::create_dir_all(parent)?;
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path.with_extension("json.lock"))?;
    let m = lock.metadata()?;
    if !m.is_file() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
        return Err(io::Error::other("unsafe config lock file"));
    }
    lock.lock_exclusive()?;
    // Validate before editing; never overwrite a damaged or untrusted config.
    let settings = load()?;
    let mut value = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
    {
        Ok(file) => serde_json::from_reader(file)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => serde_json::to_value(settings)?,
        Err(e) => return Err(e),
    };
    value
        .as_object_mut()
        .ok_or_else(|| io::Error::other("config must be an object"))?
        .insert(
            "language".into(),
            serde_json::Value::String(language.into()),
        );
    let temp = parent.join(format!(
        ".srm-config-{}.tmp",
        crate::store::random_suffix()?
    ));
    let result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)?;
        serde_json::to_writer_pretty(&mut file, &value)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temp, &path)?;
        fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result?;
    Ok(path)
}
