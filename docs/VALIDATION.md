# v2.1.0 验证记录

## v2.1 新增验证（2026-10-19）

- GNU 与 musl Release 均运行本地集成/对照套件，包括独立文件批次恢复与清理、配置验证、按卷同盘重命名；最终结果见工作区 artifacts/v21-*-integration.log。
- 真实 SSH 使用临时 loopback sshd、临时密钥和主机密钥文件；没有连接任何生产远端。
- 验证异步上传确认、本地缓存释放、远程恢复、无本地记录的远程目录恢复、离线保留、同步失败保留原路径、摘要篡改拒绝、root 远程属主/权限恢复。
- sudo 测试验证 root 文件删除与恢复、0600/UID 0 保持，以及拒绝用户可写祖先下的 root 配置。
- 测试服务器仅监听 127.0.0.1，结束后停止；WSL 为测试创建了无登录权限的 sshd 系统账号，未启用生产 sshd 服务。

同一 WSL 文件系统的单次性能样本（创建样本时间不计入）：

| 场景 | v2.0 | v2.1 |
|---|---:|---:|
| 5,000 个独立小文件 | 281.883 秒 | 2.034 秒 |
| 50,000 文件目录，checked | 0.217 秒 | 0.150 秒 |
| 50,000 文件目录，显式 rename 策略 | 未提供 | 0.081 秒 |
| 同盘已分配 1 GiB 文件 | 0.082 秒 | 0.068 秒 |

这是路径移入可恢复存储的耗时，不是物理清理、网络吞吐量或所有硬件上的性能承诺。
版本对比使用 v2.0 musl Release 和 v2.1 GNU Release，编译目标不同，数字不应解释为严格控制变量的性能研究。
rename 策略跳过递归权限扫描，明确不同于 GNU rm 的逐项权限检查；默认仍为 checked。
原始结果位于工作区 artifacts/v21-benchmark.json，脚本为 tests/benchmark.py。

以下保留 v2.0 的历史验证记录。

## v2.0 历史记录

日期：2026-10-19（Asia/Shanghai）。本记录描述本地结果，GitHub Actions 尚未触发。

## 环境与产物

- WSL2 Debian 13，Linux 6.18.40.1-microsoft-standard-WSL2，x86_64。
- GNU rm / cp 9.7；普通用户执行功能测试。
- GNU 构建使用 Debian Rust 1.96.1；musl 构建使用官方 Rust 1.96.1 和对应标准库。
- GNU release：1,123,304 字节；musl release：1,241,808 字节。
- musl ELF 无 INTERP、无 NEEDED 动态依赖；跨盘功能仍需外部 GNU `/bin/cp`。
- 未执行 UPX 压缩；未声称已验证其他 CPU 架构或所有 Linux 发行版。

## 验证结果

| 检查 | 结果 |
|---|---|
| cargo fmt --check | 通过 |
| cargo clippy --locked --all-targets -- -D warnings | 通过，无警告 |
| GNU release 集成套件 | 87/87 通过 |
| musl release 集成套件 | 87/87 通过 |
| GNU rm 对照部分 | 每个套件中 59 个场景，比较退出码及源目录最终内容 |
| 普通文件、目录、相对/绝对/悬空软链接恢复 | 通过 |
| 普通文件、目录和链接过期清理 | 通过 |
| 覆盖恢复时回收冲突对象 | 通过 |
| 同盘 1 TiB 稀疏文件移动与恢复 | 通过；实际分配 0 字节，不代表真实 TB 复制吞吐量 |
| 跨盘目录、目录内硬链接、权限、时间戳和 user xattr | 通过 |
| 12 个并发删除进程 | 通过，ID 唯一，记录一致 |
| 等待锁/跨盘复制阶段 SIGINT | 通过，退出码 130，源数据保留 |
| 元数据写入失败、损坏记录、未完成复制记录 | 源数据/备份保留；不会静默清理 |
| 安装器重复安装、Bash 激活、卸载保留数据 | 通过 |
| 安装器遇到无关同名 rm | 拒绝覆盖，通过 |

另在私有 mount namespace 内，以普通用户身份执行真实挂载测试：

- GNU rm 和 srm 的 `--one-file-system` 均保留不同设备挂载子树并返回失败。
- GNU rm 和 srm 的 `--preserve-root=all` 均拒绝指定的挂载根目录。
- GNU rm 和 srm 均正确处理同设备 bind mount：处理内容，保留不可移除的挂载点并返回失败。
- 1 MiB tmpfs 回收站复制 4 MiB 数据触发 ENOSPC；源文件摘要不变，复制阶段记录保留。

## 有意保留的差异与未覆盖项

拒绝带尾斜杠的目录软链接，不复制 GNU rm 9.7 遍历其目标内容的行为；单独有测试验证。
回收语义、空间释放、打开文件/硬链接、跨盘权限要求、深度限制与 GNU rm 存在差异，详见 README。

未验证真实断电、硬件 I/O 故障、敌对并发路径替换、NFS、全部特殊设备节点、所有 ACL 类型、SELinux、真实 TB 吞吐量及所有 locale 的交互回答。
进程中断测试与日志阶段模拟不能等同于断电恢复保证。数据持续被其他进程修改时也不具备文件系统快照语义。

## 复现

```bash
cargo build --release --locked
python3 tests/integration.py target/release/srm
sudo unshare --mount --propagation private python3 tests/mounts.py "$(realpath target/release/srm)" "$USER"
```

工具链已由 rustup 管理时，musl 构建可使用：

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --locked --target x86_64-unknown-linux-musl
python3 tests/integration.py target/x86_64-unknown-linux-musl/release/srm
```

musl 需要系统安装 musl-tools；不要将 Debian 自行构建的 rustc 与官方预编译标准库混用。
