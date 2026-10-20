use std::sync::OnceLock;

static LANGUAGE: OnceLock<Selection> = OnceLock::new();

#[derive(serde::Serialize)]
pub struct Selection {
    pub requested: String,
    pub selected: &'static str,
    pub encoding_locale: String,
    pub message_locale: String,
    pub reason: &'static str,
}

fn environment(names: &[&str]) -> String {
    names
        .iter()
        .find_map(|n| std::env::var(n).ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "C".into())
}

pub fn initialize(explicit: Option<&str>) -> Result<(), String> {
    let requested = explicit.map(str::to_owned).unwrap_or_else(|| {
        std::env::var("SRM_LANG").unwrap_or_else(|_| {
            crate::config::load()
                .map(|s| s.language)
                .unwrap_or_else(|_| "auto".into())
        })
    });
    let requested = match requested.to_ascii_lowercase().as_str() {
        "zh" | "zh-cn" | "zh_cn" => "zh",
        "en" | "en-us" | "en_us" => "en",
        "auto" => "auto",
        _ => return Err("invalid language; use --lang auto|zh|en (or SRM_LANG)".into()),
    }
    .to_string();
    let encoding_locale = environment(&["LC_ALL", "LC_CTYPE", "LANG"]);
    let messages = environment(&["LC_ALL", "LC_MESSAGES", "LANG"]);
    let message_locale = if messages == "C" || messages == "POSIX" {
        messages
    } else {
        std::env::var("LANGUAGE")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| s.split(':').next().unwrap_or("en").to_owned())
            .unwrap_or(messages)
    };
    let encoding = encoding_locale.to_ascii_lowercase().replace('-', "");
    let utf8 = encoding.contains("utf8");
    let chinese = message_locale.to_ascii_lowercase().starts_with("zh");
    let (selected, reason) = if requested == "en" {
        ("en", "explicit_english")
    } else if !utf8 {
        ("en", "non_utf8_or_unknown_encoding")
    } else if requested == "zh" {
        ("zh", "explicit_chinese_utf8")
    } else if chinese {
        ("zh", "chinese_utf8_locale")
    } else {
        ("en", "non_chinese_locale")
    };
    let _ = LANGUAGE.set(Selection {
        requested,
        selected,
        encoding_locale,
        message_locale,
        reason,
    });
    Ok(())
}

pub fn selection() -> &'static Selection {
    LANGUAGE.get().expect("language initialized")
}
pub fn zh() -> bool {
    selection().selected == "zh"
}
pub fn text(english: &'static str, chinese: &'static str) -> &'static str {
    if zh() {
        chinese
    } else {
        english
    }
}

pub fn command(mut cmd: clap::Command, scope: &str) -> clap::Command {
    let about = match scope {
        "" => text(
            "Recoverable Linux deletion with private trash and audit journal",
            "Linux 可恢复删除工具：私有回收站与操作审计",
        ),
        "list" => text(
            "List recoverable entries without changing data",
            "查看回收站，不修改数据",
        ),
        "restore" => text(
            "Restore by ID; existing targets are trashed before replacement",
            "按 ID 恢复；覆盖前先回收原目标",
        ),
        "clean" => text("Permanently remove expired entries", "永久清理已过期条目"),
        "empty" => text(
            "Permanently empty completed entries after confirmation",
            "确认后永久清空已完成的回收条目",
        ),
        "doctor" => text(
            "Inspect incomplete or damaged journals without deleting data",
            "检查中断或损坏记录，不删除数据",
        ),
        "recover" => text(
            "Reconcile an interrupted journal with a complete backup",
            "修复具有完整备份的中断记录",
        ),
        "config" => text(
            "Show settings or create an editable example",
            "查看配置或创建可编辑的配置示例",
        ),
        "remote" => text("Manage configured SSH storage", "管理已配置的 SSH 远程存储"),
        "remote init" => text(
            "Connect and initialize the configured remote directory",
            "连接并初始化配置中指定的远程目录",
        ),
        "remote status" => text(
            "Show the local upload queue and errors",
            "查看本地上传队列及错误",
        ),
        "remote list" => text(
            "Connect and list committed remote archives",
            "连接远端，列出已提交归档",
        ),
        "remote sync" => text("Upload queued entries now", "立即上传队列中的条目"),
        "remote retarget" => text(
            "Apply current remote settings to pending uploads",
            "将当前远程配置应用于待上传条目",
        ),
        "remote delete" => text(
            "Permanently delete a remote archive",
            "永久删除指定远程归档",
        ),
        "remote restore" => text(
            "Download, verify and restore a remote archive",
            "下载、校验并恢复远程归档",
        ),
        _ => "",
    };
    cmd = cmd.about(about).disable_help_subcommand(true);
    if zh() {
        cmd = cmd.help_template("{about-with-newline}\n用法：{usage}\n\n{all-args}{after-help}");
    }
    cmd = cmd
        .subcommand_help_heading(text("Commands", "命令"))
        .next_help_heading(text("Options", "选项"));
    let args: Vec<_> = cmd
        .get_arguments()
        .map(|a| a.get_id().to_string())
        .collect();
    for id in args {
        let help = match id.as_str() {
            "expired" => text("Only show expired entries", "只显示已过期条目"),
            "repair_permissions" => text(
                "Tighten owned management directories to 0700; never change ownership or payloads",
                "仅收紧本人管理目录为 0700，不改变属主或回收文件权限",
            ),
            "verbose" => text("Show full metadata", "显示详细元数据"),
            "json" => text(
                "Output JSON (field names are not translated)",
                "输出 JSON（字段名称保持不变）",
            ),
            "measure" => text(
                "Scan directories to calculate size; may be slow",
                "扫描目录计算大小，可能耗时",
            ),
            "ids" | "id" => text(
                "Recovery ID or unique prefix from srm list",
                "srm list 中的恢复 ID 或唯一前缀",
            ),
            "force" => text(
                "Trash an existing target and restore without prompting",
                "不询问，先回收现有目标再恢复",
            ),
            "target" => text("Restore destination path", "恢复目标路径"),
            "all" => text(
                "Permanently clean all completed entries, not just expired ones",
                "永久清理所有已完成条目，不限已过期条目",
            ),
            "dry_run" => text("Preview only; do not delete anything", "仅预览，不执行删除"),
            "yes" => text("Skip confirmation", "跳过确认"),
            "init" => text(
                "Create a config example; never overwrite an existing file",
                "创建配置示例，不覆盖已有文件",
            ),
            "object" => text("Remote archive object identifier", "远程归档对象编号"),
            _ => "",
        };
        let positional = cmd
            .get_arguments()
            .find(|a| a.get_id().as_str() == id)
            .is_some_and(|a| a.is_positional());
        cmd = cmd.mut_arg(id, |a| {
            a.help(help).help_heading(if positional {
                text("Arguments", "参数")
            } else {
                text("Options", "选项")
            })
        });
    }
    // Explicit help flags let both languages describe every subcommand's help.
    cmd = cmd.disable_help_flag(true).arg(
        clap::Arg::new("help")
            .short('h')
            .long("help")
            .action(clap::ArgAction::Help)
            .help(text("Print help", "显示帮助"))
            .help_heading(text("Options", "选项")),
    );
    let children: Vec<_> = cmd
        .get_subcommands()
        .map(|c| c.get_name().to_owned())
        .collect();
    for name in children {
        let child_scope = if scope.is_empty() {
            name.clone()
        } else {
            format!("{scope} {name}")
        };
        cmd = cmd.mut_subcommand(name, |c| command(c, &child_scope));
    }
    if scope.is_empty() {
        cmd = cmd.after_help(text(
            "Remove: srm [rm OPTIONS] FILE...\nExplicit rm mode: srm --rm [OPTIONS] FILE...\nLegacy recursive mode: srm del [-d DAYS] FILE...\nLanguage: srm --lang auto|zh|en COMMAND --help\nLanguage diagnostics: srm --lang zh --language-info\nEnvironment: SRM_LANG=auto|zh|en; non-UTF-8 locales fall back to English.\nUse -- before filenames that begin with '-' or match command names.",
            "安全删除：srm [rm 选项] 文件...\n显式 rm 模式：srm --rm [选项] 文件...\n旧式递归模式：srm del [-d 保留天数] 文件...\n语言切换：srm --lang auto|zh|en 命令 --help\n语言诊断：srm --lang zh --language-info\n环境变量：SRM_LANG=auto|zh|en；非 UTF-8 环境回退英文。\n文件名以 '-' 开头或与命令重名时，先加 --。"));
    }
    cmd
}
