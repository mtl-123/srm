# srm — Linux 可恢复删除工具

[![CI and Linux release](https://github.com/mtl-123/srm/actions/workflows/srm-ci.yml/badge.svg)](https://github.com/mtl-123/srm/actions/workflows/srm-ci.yml)

当前版本：2.1.2。[下载二进制与安装包](https://github.com/mtl-123/srm/releases)。推荐 x86_64-unknown-linux-musl 静态包。
默认使用独立命令 `srm`，不替换系统 `rm`。安装/权限故障见 [排查与修复](docs/TROUBLESHOOTING.md)。

保留两个原项目的简写用法，推荐日常使用：

| 简写 | 完整命令 | 功能 |
|---|---|---|
| `srm del` | `srm delete` | 文件/目录移入回收站 |
| `srm res ID` | `srm restore ID` | 恢复 |
| `srm ls` | `srm list` | 查看回收站 |
| `srm cln` | `srm clean` | 永久清理过期项 |
| `srm empty` | 同名命令 | 确认后永久清空 |

`srm del -d 30 文件` 表示保留 30 天；直接使用 `srm -d 目录` 时，`-d` 与 rm 一样表示允许空目录。两种语法均保留。

设置并保存默认语言：`srm --lang zh` / `srm --lang en`；`srm --lang auto` 恢复自动选择。
临时查看帮助：`srm --lang zh --help` / `srm --lang en --help`；非 UTF-8 环境回退英文但保留偏好。
使用 `srm --language-info` 查看选择原因，详见 [语言设置](docs/LANGUAGE.md)。

支持 Bash/Zsh Tab 补全；Zsh 提供中文功能说明和“恢复 ID — 原路径”提示。
用户安装加 `--activate-bash` 或 `--activate-zsh` 自动启用，详见 [Tab 补全](docs/TAB-COMPLETION.md)。

新增可配置的本地/远程保存、后台 SSH 上传、按磁盘回收目录、独立小文件分组日志，以及 root/sudo 系统安装。
详见 [远程保存、性能与 sudo](docs/REMOTE-AND-PERFORMANCE.md)，配置范例在 [config/examples](config/examples/)。
默认保持本地模式；远程模式推荐 `remote_async`，配置服务器并验证 SSH 主机密钥后启用。
普通用户配置位于 `~/.config/safe-srm/config.json`；root 独立使用 `/etc/safe-srm/config.json` 和 `/var/lib/safe-srm/root`。

整合 safe-srm 的短 ID、详细审计、列表与进度能力，以及 safe-rm-rs 的用户隔离思路，重写删除与恢复流程。
目标是兼容 GNU rm 的常用参数、交互和退出码，同时把删除对象保留在私有回收站。
**可恢复删除不等同于 GNU rm 的永久删除；具体边界见下文，不承诺所有场景完全等价。**

## Linux 使用

运行环境：Linux（需 `/proc`、`renameat2` 和 `syncfs`）、GNU coreutils 的 `/bin/cp`。
源码构建需要 Rust 1.85+、C 编译工具；本次验证环境为 WSL2 Debian 13、Rust 1.96.1、GNU rm 9.7。
跨盘复制使用 GNU cp；本工具不调用 shell 拼接文件名。空格、换行、中文和非 UTF-8 文件名均支持。

```bash
cargo build --release --locked
bash scripts/install.sh --activate-bash
source "$HOME/.config/safe-srm/env.sh"

srm file.txt
srm -rf directory/
srm -i important.txt
srm -I a b c d
srm -- -filename

srm list
srm restore f_12345678      # 使用列表中的真实 ID 或唯一前缀
srm clean --dry-run
srm clean                 # 永久清理已过期项
/bin/rm -- disposable.txt # 明确调用系统原生永久删除
```

安装器写入 `~/.local/lib/safe-srm/srm`、`~/.local/bin/srm`。
`--replace-rm` 添加 `~/.local/bin/rm`，激活脚本同时提供 rm 函数；`--activate-bash` 备份并追加 `.bashrc` 的激活入口。
不覆盖 `/bin/rm` 或 `/usr/bin/rm`。其他用户及 sudo 的命令查找可能仍使用系统 rm。
系统脚本若明确调用绝对路径，也继续使用系统 rm；继承用户 PATH 的脚本则可能调用安全替代版。
`command rm` 仍可能命中 PATH 中的替代版，退出通道应使用 `/bin/rm`。
现有 shell 需 source 激活文件或重新打开终端。

仅安装独立 srm，不接管 rm：

```bash
bash scripts/install.sh
```

使用打包二进制：解压后执行 `bash scripts/install.sh --binary ./srm --activate-bash`。默认独立使用 srm，不接管系统 rm。
指定其他构建输出：`bash scripts/install.sh --binary /absolute/path/to/srm`。
卸载命令入口：`bash scripts/uninstall.sh`，再执行 `unset -f rm; hash -r`。
卸载保留二进制和回收站，便于继续恢复。安装器拒绝覆盖无关的同名命令。

## rm 参数兼容

| 参数 | 行为 |
|---|---|
| `-f`, `--force` | 忽略不存在的路径和空参数列表，不询问；不会跳过权限失败或清空回收站 |
| `-i` | 每次移除前询问，递归时支持保留部分子项 |
| `-I` | 递归删除或超过三个操作数时询问一次 |
| `--interactive[=never/once/always]` | 指定交互方式；省略值时为 always |
| `-r`, `-R`, `--recursive` | 递归删除；未指定时拒绝目录 |
| `-d`, `--dir` | 删除空目录 |
| `-v`, `--verbose` | 显示删除对象和回收 ID；默认 rm 模式成功时安静 |
| `--one-file-system` | 递归遍历时跳过不同设备的目录，并报告失败 |
| `--preserve-root[=all]` | 默认保护 `/`；all 还保护与父目录设备不同的操作数 |
| `--no-preserve-root` | 关闭根目录特判；回收站及其祖先保护仍生效 |
| `--` | 后续所有参数都作为文件名 |

支持组合短参数、操作数之间的选项，以及无歧义的长选项缩写。
设置 `POSIXLY_CORRECT` 后遇到第一个操作数即停止解析选项。
命令后面的 `.`、`..` 始终拒绝；正常名称里的 `..` 不再误判。
成功或用户拒绝交互时返回 0，失败返回非零，SIGINT 返回 130。
以 `rm` 名称调用时，`list`、`restore` 等也是普通文件名，不会意外进入管理命令。
也可用 `srm --rm ...` 强制进入 rm 模式。

## 回收站与管理

默认路径为 `$XDG_DATA_HOME/safe-srm`，未设置有效的绝对 XDG 路径时为 `~/.local/share/safe-srm`。
设置绝对路径 `SRM_HOME` 可覆盖，例如 `export SRM_HOME=/data/my-private-trash`。
每个路径仅供其所有者使用；发现不私有或由其他用户拥有的回收站会报错，不会擅自改权限。
目录为 0700，日志、锁文件、元数据为 0600。保留的文件本身保持原权限，外层私有目录阻止其他用户访问。

```text
safe-srm/
  files/         按 ID 保存的原始对象
  meta/          JSON 操作日志与原路径字节
  audit.jsonl    结构化审计日志
  lock           跨进程互斥锁
  log-date       日志保留检查标记
```

```bash
srm del --expire-days 30 file directory # 旧式命令默认允许目录
srm del -d 30 file                      # 保留旧式 -d 天数；rm 模式 -d 是空目录
srm list --json                         # 包含状态、原路径字节、ID 和大小
srm list -v --expired
srm list --measure                      # 显式扫描目录大小；默认不为统计大小多扫一遍
srm restore ID
srm restore ID -t /path/new-name
srm restore ID1 ID2 -t /existing/directory
srm restore -f ID                       # 先把目标现有对象移入回收站，再恢复
srm clean --dry-run
srm clean --all                         # 永久清理所有已完成条目，无确认
srm empty                              # 交互确认永久清理
srm empty -y
srm doctor                             # 检查未完成、损坏或孤立数据，不删除数据
srm recover ID                         # 将已知具有完整备份的中断记录恢复为可恢复状态
```

默认保留 7 天。过期只是满足清理条件，程序不会后台自动销毁文件。
日志保留 30 天，每个 UTC 日期首次调用时流式清理，避免每次命令都重写整份日志。
日志记录操作开始、成功、失败、原路径字节、ID、UID 和恢复目标。
短 ID 为类型前缀加 16 位随机十六进制字符，支持唯一前缀匹配，不存在批量重复使用旧 ID 集合的问题。

## 可靠性与性能

- 同文件系统使用不覆盖目标的原子 rename，1 TiB 稀疏文件无需额外分配同等空间。
- 递归删除会检查目录可访问性和挂载边界；有交互、权限问题或挂载子树时逐项处理。
- 跨文件系统先由 `cp --archive --reflink=auto --sparse=always` 完整复制，再校验源对象元数据快照、同步目标文件系统，最后删除源对象。
- 跨盘保留目录内硬链接关系、软链接、权限、时间戳及文件系统支持的扩展属性；终端显示复制等待进度，不伪造百分比或吞吐率。
- 写入操作日志先于移动；JSON 使用临时文件、fsync、原子替换，目录变更也同步落盘。
- 用户级锁串行化同一回收站的操作。Ctrl+C 可中断等待锁或复制；已完成的删除仍可恢复。
- 复制失败、ENOSPC、SIGINT 时保留源数据和记录。复制期间中断不执行破坏性“回滚”。
- `clean` 按文件类型正确删除，成功后才移除元数据；失败或中断时保留记录。
- 相对链接、悬空链接不再被当作文件缺失；恢复软链接不会 chmod 链接目标。
- 覆盖恢复会为被覆盖内容建立另一条回收记录。损坏元数据不会被 list 自动删除。

## 中断和恢复

`doctor` 会显示原路径、备份路径、状态和恢复暂存路径。
`recover ID` 只处理已知完整的备份：已完成 rename 的 prepared、copied/removing_source，或备份仍存在的 restoring。
它不删除残余源文件或恢复目标；随后执行 restore，若路径冲突仍会询问或先回收冲突内容。
copying 状态的备份可能不完整，必须检查原文件，工具不会把它误认成完整备份。
已授权但未完成的 purging 状态可重试 clean。异常阶段的源文件、备份和暂存文件保留供人工检查。
这提供可追查的失败处理，不承诺跨文件系统操作具有单个文件系统事务的原子性。

## 与 GNU rm 的明确差异

1. 删除会保留数据，不立即释放空间，也不会立即改变其他硬链接看到的数据。仍在写入的打开文件可能继续改变回收站内容；回收站不是独立备份。
2. 出于安全考虑，拒绝 `rm -r directory-symlink/`。本机 GNU rm 9.7 会遍历目标内容；本工具要求去掉软链接尾部斜杠。
3. 回收站、其内容和祖先不可删除，即使 `-f` 或 `--no-preserve-root`。
4. 跨盘可恢复删除需要读取源对象并分配存储空间；原生 rm 可以删除的一些不可读文件，本工具可能因无法备份而拒绝。
5. 跨盘复制的源树变更检查不是文件系统快照。正在被其他程序持续修改的目录、恶意并发路径替换、特殊设备节点、NFS/网络文件系统及内核不可变属性不承诺与 GNU rm 完全一致。
6. 交互提示文字、详细输出、locale 下的回答解析与 GNU rm 不逐字一致；yes/y 使用英文。管理子命令使用 clap 的错误退出码。
7. 递归交互/跨挂载删除可能产生多条恢复记录，而不是一条完整目录记录。内置目录遍历深度限制为 512。
8. 跨盘 hardlink 与源树以外文件的链接关系无法跨设备保留；底层文件系统不支持的属性也不能保证恢复。

不建议把恢复行为当成数据库备份或磁盘空间回收接口。需要永久删除和立即释放空间时明确使用 `/bin/rm`。

## 定时清理（可选）

```bash
mkdir -p ~/.config/systemd/user
cp systemd/safe-srm-clean.{service,timer} ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now safe-srm-clean.timer
```

默认每天清理过期项，安装器不自动启用。
使用自定义 SRM_HOME 时，为 user service 添加对应的 `Environment=SRM_HOME=/absolute/path`；shell 的 export 不一定传给 systemd 用户服务。

## 构建、测试与发布

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
python3 tests/integration.py target/debug/srm
# 可选：在私有挂载命名空间中检查真实挂载边界及磁盘不足；测试操作以指定普通用户运行
sudo unshare --mount --propagation private python3 tests/mounts.py "$(realpath target/debug/srm)" "$USER"
cargo build --release --locked
bash scripts/package.sh target/release/srm linux-x86_64-gnu
# 可选 UPX，生成独立压缩发布目录并执行 upx -t
bash scripts/package.sh target/release/srm linux-x86_64-gnu-upx --upx
```

集成测试在新建的临时目录里与 `/bin/rm` 对照，并检查真实恢复、权限、硬链接、xattr、并发、稀疏文件和中断处理。
测试根目录会打印并保留证据，其中包含稀疏文件；不要将其复制成实分配文件。
本地 Debian 13 的 GNU 构建不应标为通用静态二进制；不同发行版需考虑 glibc 版本。
CI 分别构建 GNU 和 musl 目标；musl 包仍依赖运行环境中的 GNU `/bin/cp`。

## 从旧版迁移

v2 使用新的用户数据位置和带阶段状态的元数据格式，不会扫描或删除旧版回收站。
旧 safe-srm 的数据在旧可执行文件旁 `.srm`，safe-rm-rs 的数据在 `~/.srm`。
升级前用对应旧程序恢复需要的数据，并保留旧二进制及目录；不要直接把旧 `.meta` 文件复制进 v2。
旧版损坏或丢失元数据的条目需按其原始文件检查，不应通过 empty 自动处理。

MIT License。两个旧项目的共同功能已集中到本仓库，safe-rm-rs 可保留为历史参考。
