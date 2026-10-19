# 中文与英文帮助

```bash
srm --lang zh                        # 保存默认中文
srm --lang en                        # 保存默认英文
srm --lang auto                      # 保存自动选择
srm --lang zh --help
srm --lang en --help
srm --lang auto --help
srm --lang zh restore --help
srm --lang en remote restore --help
srm --lang zh del --help
srm --lang zh --language-info
```

语言选项必须写在子命令或文件操作数之前，可与前置 `--config` 任意排序：

```bash
srm --config /absolute/config.json --lang zh list --help
sudo srm --lang zh --help
```

也可为当前 shell 设置偏好：

```bash
export SRM_LANG=zh   # 或 en / auto
```

单独执行 `srm --lang zh/en/auto` 会原子写入当前用户配置的 `language` 字段；其他设置保持不变。
配置不存在时创建，已有配置损坏或权限不安全时拒绝覆盖。无需编辑 shell 环境变量。
普通用户使用自己的 config.json；`sudo srm --lang ...` 修改 root 的 `/etc/safe-srm/config.json`。
指定 `--config` 时保存到指定实例的配置。已有环境变量 `SRM_LANG` 会覆盖持久偏好，可用 `unset SRM_LANG` 取消。
带子命令/`--help`/`--language-info` 的 `--lang` 只影响本次运行，不保存。

优先级：命令行 `--lang` > `SRM_LANG` > 配置 `language` > 自动选择。
自动选择遵循 LC_ALL、LC_MESSAGES/LANG，以及适用时的 LANGUAGE 首选语言；
编码依据 LC_ALL、LC_CTYPE、LANG。中文 UTF-8 环境显示中文，其余自动显示英文。
即使指定 zh，编码为 C、POSIX、GBK 或无法确认 UTF-8 时也回退英文。
回退不会把已保存的 zh 改成 en，下次 UTF-8 会话仍显示中文。
`--language-info` 输出 requested、selected、编码/消息 locale 和 reason，便于排查。

例如系统当前为 C，但终端实际支持 UTF-8，可由用户显式设置：

```bash
LANG=C.UTF-8 srm --lang zh --help
# 若 LC_ALL 已设置，它优先于 LANG，需要一并指定：
LC_ALL=C.UTF-8 srm --lang zh --help
```

工具依据环境声明选择文本，不修改系统 locale，也无法检测终端字体是否包含汉字。
出现方框或乱码时使用 `--lang en`，并检查终端编码与字体。

范围：主帮助、所有管理子命令帮助、删除帮助、Zsh 补全描述已支持中英文。
运行时日志、第三方工具/操作系统错误、部分交互提示暂保持英文；JSON 字段、ID、参数名称保持稳定。
这不是全量运行时消息翻译。语言选择不会改变删除、恢复、权限检查或确认行为。
Bash 补全补的是固定命令/参数，因此候选拼写保持英文；支持 --lang 的值补全。

`srm -- --lang=zh` 中 -- 后面的内容仍是文件名，不作为语言选项解析。
兼容入口以 rm 名称调用时，不增加 --lang 参数；可通过 SRM_LANG 指定帮助语言。
