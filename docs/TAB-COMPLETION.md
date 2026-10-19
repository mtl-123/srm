# Tab 补全与功能提示

用户和系统安装器都会安装 srm 的 Bash/Zsh 补全文件，不注册 rm 补全，不修改系统 rm。

## Bash

```bash
bash scripts/install.sh --binary ./srm --activate-bash
source ~/.config/safe-srm/env.sh
```

交互式 Bash 会加载补全。部分补全唯一时按一次 Tab；多个候选通常按两次 Tab 展开。
如果只想手动加载补全：

```bash
source ~/.local/share/bash-completion/completions/srm
```

系统安装后，启用了 bash-completion 的 shell 会自动加载
`/usr/share/bash-completion/completions/srm`。未启用框架时可直接 source 此文件，root 亦同。

## Zsh：候选项旁显示中文功能说明

描述语言跟随 `--lang`、`SRM_LANG` 和系统 locale；英文或非 UTF-8 环境显示英文。
详见 [语言设置](LANGUAGE.md)。

```bash
bash scripts/install.sh --binary ./srm --activate-zsh
source ~/.config/safe-srm/completion.zsh
```

系统安装把 `_srm` 放在 `/usr/local/share/zsh/site-functions`。
自定义 Zsh 环境需要把该目录加入 fpath，再执行 `autoload -Uz compinit; compinit`。
已有补全框架时无需重复初始化，执行 `autoload -Uz _srm; compdef _srm srm` 即可注册。

## 支持的内容

| 输入后按 Tab | 提示内容 |
|---|---|
| `srm ` | 功能命令、常用选项、本地文件 |
| `srm remote ` | init/status/list/sync/restore/delete/retarget |
| `srm list --` | json/expired/verbose/measure/help |
| `srm clean --` | all/dry-run/help |
| `srm restore ` | 本地记录中的恢复 ID；Zsh 同时显示原路径 |
| `srm restore ID -t ` | 目标路径，支持空格文件名 |
| `srm del -d ` | 常用保留天数 |
| `srm --rm --interactive=` | never/once/always |
| `srm --config ` | 配置文件路径；其后的补全使用该配置 |
| `srm -- ` | 仅补全文件名 |

Bash 原生补全显示可插入的候选词，不像 Zsh 一样原生提供每项独立说明栏。
需要完整功能说明时用 `srm --help`、`srm restore --help`、`srm remote --help`。
如特别需要“Tab 菜单中直接显示功能描述”，推荐 Zsh。

补全不会执行 srm 命令、不会联网、不会自动 sudo、不会写回收记录。
恢复 ID 使用 Python 3 只读扫描，限制记录数量和本地扫描时间，最多返回 100 个匹配项。
没有 Python 3 时，命令、参数和路径补全仍可用，只有动态 ID 不可用。
网络文件系统上的元数据读取仍可能受文件系统 IO 延迟影响；不是硬实时超时保证。

`sudo srm` 的补全转发依赖 shell 的 sudo 补全框架；普通用户的 Tab 不会自动提权读取 root 回收站。
需要补全 root 恢复 ID 时，在 root shell 中加载补全后使用 `srm restore`。
远程归档 ID 不在按 Tab 时连接服务器获取，先运行 `srm remote list` 查看。
