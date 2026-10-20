# 安装与回收目录故障排查

## sudo: srm: command not found

用户安装只提供 ~/.local/bin/srm。sudo 通常使用 secure_path，不包含用户的可写目录。
不要把用户可写目录加入 sudo 的 PATH。
在 musl 静态安装包解压目录执行：

```bash
./srm --version
sudo bash scripts/install-system.sh --binary ./srm
sudo /usr/local/bin/srm --version
```

若 sudo 的 PATH 包含 /usr/local/bin，之后也可直接 sudo srm。
普通用户与 root 回收数据分开，恢复 root 数据用 sudo /usr/local/bin/srm res ID。
不要 sudo 用户安装脚本，也不要修改 /usr/bin/rm。

## 回收目录要求 0700

v2.1.0 用户安装器曾把补全辅助文件放入默认回收目录，并可能创建成 0755。
v2.1.1 将辅助文件迁到 ~/.local/lib/safe-srm，安装后检查真实存储环境。
v2.1.2 正常运行会自动把当前用户拥有的真实管理目录从 0755/0775 等收紧到 0700，通常无需再执行修复命令。
操作通过打开的目录描述符修改权限，不跟随末级符号链接，不改属主、不递归修改回收文件。
若所有者本身缺少访问权限（例如 0500），仍拒绝自动增加权限；不同用户拥有的目录仍拒绝使用。
修复旧环境先检查：

```bash
id
stat -c '%F %U %u %a %n' "$HOME/.local/share/safe-srm"
namei -l "$HOME/.local/share/safe-srm"
```

用 v2.1.1 的二进制，以目录所有者运行：

```bash
./srm doctor --repair-permissions
bash scripts/install.sh --binary ./srm --activate-bash
```

v2.1.2 日常删除直接执行 `srm del -d 3 文件` 即可自动处理本人 0775 管理目录。
`srm del 3 文件` 的 3 是文件名，不是保留天数。程序会提示正确的 -d 用法，但保留数字文件名的语义。

修复仅针对中央回收目录及 files/meta/locks/staging 管理目录，使用不跟随符号链接的目录文件描述符。
仅把本人拥有、无组/其他用户写权限的真实目录收紧为 0700。
不递归 chmod，不改回收文件内容/权限，不更改属主，不处理不可信的符号链接或可写祖先。
自定义配置使用 `./srm --config /absolute/config.json doctor --repair-permissions`。
若显示错误属主（例如 root），先核实归属和来源，不要执行 chown -R 或 chmod -R。
对 0777、符号链接等场景修复会拒绝，需要管理员检查，而不是放宽安全验证。

## 发布前验收

CI 运行默认 HOME/XDG 路径的安装→首次删除→恢复、重复安装、用户/root 隔离，
以及旧版 0755 迁移、错误属主、符号链接、可写目录拒绝和不兼容二进制预检。
同时保留跨盘、磁盘满、中断、远程、语言和补全测试。
这些测试不代表覆盖所有机器、文件系统或竞争条件；新增真实故障必须成为固定回归用例。
