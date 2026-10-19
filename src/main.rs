mod config;
mod i18n;
mod remote;
mod rm;
mod store;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn confirm(message: &str) -> bool {
    eprint!("srm: {message} [y/N] ");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    io::stdin().read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

#[derive(Parser)]
#[command(
    name = "srm",
    version,
    about = "Recoverable Linux rm with private trash and audit journal"
)]
struct Management {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(
        visible_alias = "ls",
        about = "List recoverable items without discarding damaged metadata"
    )]
    List {
        #[arg(long)]
        expired: bool,
        #[arg(short, long)]
        verbose: bool,
        #[arg(long)]
        json: bool,
        #[arg(long, help = "Scan directories to calculate their size (can be slow)")]
        measure: bool,
    },
    #[command(
        visible_alias = "res",
        about = "Restore IDs; overwritten targets are themselves trashed"
    )]
    Restore {
        #[arg(required = true)]
        ids: Vec<String>,
        #[arg(short, long)]
        force: bool,
        #[arg(short, long)]
        target: Option<PathBuf>,
    },
    #[command(visible_alias = "cln", about = "Permanently remove expired items")]
    Clean {
        #[arg(short, long)]
        all: bool,
        #[arg(long)]
        dry_run: bool,
    },
    #[command(about = "Permanently empty completed entries (pending journals are retained)")]
    Empty {
        #[arg(short, long)]
        yes: bool,
    },
    #[command(about = "Inspect pending/damaged journals without deleting data")]
    Doctor,
    #[command(about = "Reconcile an interrupted journal that has a known complete backup")]
    Recover { id: String },
    #[command(about = "Show configuration, or create a private editable example")]
    Config {
        #[arg(long)]
        init: bool,
    },
    #[command(about = "Manage the configured SSH archive destination")]
    Remote {
        #[command(subcommand)]
        command: RemoteCommand,
    },
}

#[derive(Subcommand)]
enum RemoteCommand {
    Init,
    Status,
    List,
    Sync,
    #[command(about = "Apply current remote configuration to queued, not-yet-verified entries")]
    Retarget {
        #[arg(short, long)]
        yes: bool,
    },
    #[command(about = "Permanently delete a remote object, including incomplete uploads")]
    Delete {
        object: String,
        #[arg(short, long)]
        yes: bool,
    },
    Restore {
        object: String,
        #[arg(short, long)]
        target: PathBuf,
    },
}

fn run() -> Result<i32, Box<dyn std::error::Error>> {
    let mut args: Vec<OsString> = std::env::args_os().collect();
    let invoked_as_rm = Path::new(&args[0]).file_name().is_some_and(|n| n == "rm");
    args.remove(0);
    if invoked_as_rm {
        i18n::initialize(None)?;
        return rm::run(args, false);
    }
    let mut language = None;
    loop {
        if args.first().is_some_and(|a| a == "--lang") {
            if args.len() < 2 {
                return Err("--lang requires auto, zh or en".into());
            }
            language = Some(
                args.remove(1)
                    .into_string()
                    .map_err(|_| "invalid language encoding")?,
            );
            args.remove(0);
        } else if args
            .first()
            .and_then(|a| a.to_str())
            .is_some_and(|a| a.starts_with("--lang="))
        {
            language = Some(args.remove(0).to_str().unwrap()[7..].to_owned());
        } else if args.first().is_some_and(|a| a == "--config") {
            if args.len() < 2 {
                return Err("--config requires an absolute path".into());
            }
            let path = PathBuf::from(args.remove(1));
            args.remove(0);
            let _ = config::CONFIG_PATH.set(path);
        } else {
            break;
        }
    }
    i18n::initialize(language.as_deref())?;
    if language.is_some() && args.is_empty() {
        let requested = &i18n::selection().requested;
        let path = config::save_language(requested)?;
        println!(
            "{}: {} ({})",
            i18n::text("Saved default language", "已保存默认语言"),
            requested,
            path.display()
        );
        if requested == "zh" && !i18n::zh() {
            println!("Chinese preference saved. This non-UTF-8 environment displays English; UTF-8 sessions will use Chinese.");
        }
        return Ok(0);
    }
    if args == [OsString::from("--language-info")] {
        println!("{}", serde_json::to_string_pretty(i18n::selection())?);
        return Ok(0);
    }
    if args.first().is_some_and(|a| a == "help") {
        args.remove(0);
        args.push("--help".into());
    }
    if args.first().is_some_and(|a| a == "--rm" || a == "rm") {
        args.remove(0);
        return rm::run(args, false);
    }
    if args.first().is_some_and(|a| a == "delete" || a == "del") {
        args.remove(0);
        return rm::run(args, true);
    }
    let management = args.first().and_then(|a| a.to_str()).is_some_and(|a| {
        matches!(
            a,
            "list"
                | "ls"
                | "restore"
                | "res"
                | "clean"
                | "cln"
                | "empty"
                | "doctor"
                | "recover"
                | "config"
                | "remote"
                | "help"
        )
    });
    if !management {
        if args.is_empty() || args == [OsString::from("--help")] || args == [OsString::from("-h")] {
            i18n::command(Management::command(), "").print_help()?;
            println!();
            return Ok(0);
        }
        if args == [OsString::from("--version")] || args == [OsString::from("-V")] {
            println!("srm {}", env!("CARGO_PKG_VERSION"));
            return Ok(0);
        }
        return rm::run(args, false);
    }
    let matches = i18n::command(Management::command(), "")
        .get_matches_from(std::iter::once(OsString::from("srm")).chain(args));
    let cli = Management::from_arg_matches(&matches)?;
    if let Command::Config { init } = cli.command {
        if init {
            println!("created {}", config::initialize()?.display());
        } else {
            println!("{}", serde_json::to_string_pretty(&config::load()?)?);
        }
        return Ok(0);
    }
    let mut store = store::Store::open()?;
    let mut failed = false;
    match cli.command {
        Command::Config { .. } => unreachable!(),
        Command::Remote { command } => match command {
            RemoteCommand::Sync => failed = !store.sync_remote()?,
            RemoteCommand::Retarget { yes } => {
                if yes || confirm("retarget pending uploads using the current configuration?") {
                    store.retarget_pending()?;
                }
            }
            RemoteCommand::Delete { object, yes } => {
                if yes || confirm("permanently delete this remote archive?") {
                    let target = config::load()?.remote.ok_or("no remote configuration")?;
                    println!(
                        "{}",
                        remote::request(&target, "delete", Some(&object), None)?
                    );
                }
            }
            RemoteCommand::Status => store.remote_status()?,
            RemoteCommand::Init | RemoteCommand::List => {
                let settings = config::load()?;
                let remote = settings.remote.ok_or("no remote configuration")?;
                let action = if matches!(command, RemoteCommand::Init) {
                    "init"
                } else {
                    "list"
                };
                println!(
                    "{}",
                    serde_json::to_string_pretty(&remote::request(&remote, action, None, None)?)?
                );
            }
            RemoteCommand::Restore { object, target } => {
                let id = store.import_remote(&object)?;
                if let Some(path) = store.restore(&id, Some(&target), false)? {
                    println!("restored {id} -> {}", path.display());
                }
            }
        },
        Command::List {
            expired,
            verbose,
            json,
            measure,
        } => store.list(expired, verbose, json, measure)?,
        Command::Doctor => failed = !store.doctor()?,
        Command::Recover { id } => {
            store.recover(&id)?;
            println!("journal {id} is restorable; use srm restore {id}");
        }
        Command::Restore { ids, force, target } => {
            if ids.len() > 1 && target.as_ref().is_some_and(|p| !p.is_dir()) {
                return Err("restoring multiple IDs requires an existing target directory".into());
            }
            for id in ids {
                if INTERRUPTED.load(Ordering::Relaxed) {
                    failed = true;
                    break;
                }
                match store.restore(&id, target.as_deref(), force) {
                    Ok(Some(path)) => println!("restored {id} -> {}", path.display()),
                    Ok(None) => {}
                    Err(e) => {
                        eprintln!("srm: {id}: {e}");
                        failed = true;
                    }
                }
            }
        }
        Command::Clean { all, dry_run } => failed = !store.clean(all, dry_run)?,
        Command::Empty { yes } => {
            if yes || confirm("permanently empty the trash?") {
                failed = !store.clean(true, false)?;
            }
        }
    }
    Ok(if INTERRUPTED.load(Ordering::Relaxed) {
        130
    } else {
        i32::from(failed)
    })
}

fn main() {
    if unsafe { libc::geteuid() != libc::getuid() } {
        eprintln!("srm: setuid installation is unsupported; use sudo with a root-owned system installation");
        std::process::exit(1);
    }
    if let Err(e) = ctrlc::set_handler(|| {
        INTERRUPTED.store(true, Ordering::Relaxed);
    }) {
        eprintln!("srm: cannot install interrupt handler: {e}");
        std::process::exit(1);
    }
    let code = match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("srm: {e}");
            if INTERRUPTED.load(Ordering::Relaxed) {
                130
            } else {
                1
            }
        }
    };
    std::process::exit(code);
}
