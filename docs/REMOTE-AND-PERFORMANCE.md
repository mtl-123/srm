# 本地、远程保存、性能与 sudo

v2.1 支持在配置文件中选择本地回收、后台远程保存或同步远程保存。
远程地址、用户、端口、目录、密钥路径和命名空间均可配置，没有内置生产服务器地址。

## 配置文件

普通用户：`~/.config/safe-srm/config.json`，支持 XDG_CONFIG_HOME、SRM_CONFIG 或 `srm --config /absolute/path`。
root：`/etc/safe-srm/config.json`，忽略继承的 HOME、SRM_CONFIG、SRM_HOME 和 XDG_DATA_HOME。
root 的显式 `--config` 文件及祖先目录必须由 root 拥有、不可由其他用户写入、不能包含符号链接。
不支持 setuid 安装；使用 sudo 和 root 拥有的系统二进制。

```bash
srm config --init        # 仅首次创建；不会覆盖现有文件
srm config              # 显示实际加载的配置
sudo /usr/local/bin/srm config
```

配置范例在 `config/examples/`。创建的默认配置是 local 模式，不会连接示例地址。

| 配置项 | 含义 |
|---|---|
| backend | local、remote_async 或 remote_sync |
| local_store | 中央日志和默认本地回收目录；null 使用用户默认路径 |
| volume_stores | 其他文件系统上的私有回收目录列表；按源文件设备选择同盘目录 |
| expire_days | 默认保留天数；0 至 36500 |
| batch_size | 独立文件批处理上限，默认 256 |
| directory_strategy | checked 保留递归权限检查；rename 显式启用整目录快速移动 |
| remote.host / user / port | SSH 服务器地址、账号、端口 |
| remote.directory / namespace | 服务器上的绝对存储目录及本机/用户命名空间 |
| remote.identity_file | SSH 私钥文件的绝对路径；null 使用 SSH 默认认证配置 |
| remote.known_hosts_file | 主机密钥文件；null 使用 SSH 默认文件。始终严格验证主机密钥 |
| remote.release_local_after_upload | true：远程确认后释放本地缓存；false：同时保留本地副本 |
| remote.timeout_seconds | 每次传输/归档命令的超时，默认 86400 秒，可根据数据量和带宽调整 |

配置文件不能被组或其他用户写入。root 不读取用户提供的配置或 SSH 私钥路径。
修改配置只影响新删除；已有队列保留原远程目标，避免改配置时把数据悄悄发送到另一台机器。

## 后台远程模式（推荐）

1. 先持久化操作日志，在本机可靠保存对象。
2. 同盘对象优先重命名，命令返回；启动一个后台上传进程。
3. 通过 SSH 流式传输 tar 归档，支持目录内硬链接、稀疏文件、软链接、ACL 和扩展属性。
4. 客户端与接收端计算 SHA-256；服务器同步归档与元数据到磁盘。
5. 客户端确认 tar 成功、源对象未变化及摘要一致后，再提交远程对象。
6. 只有远程提交得到确认，才可按配置释放本地数据。断线、超时、认证失败均保留本地副本。

上传和缓存回收使用单项锁，传输和批量物理删除期间释放中央锁，其他条目的删除可继续执行。
同一条目正在上传时恢复会报告忙，避免上传与恢复互相破坏。可以停止上传服务/进程后重试恢复。
后台进程退出、机器重启或网络失败后，队列仍在本地元数据中。上传 timer 每分钟重试。
当前失败重试会重新生成归档并重传，不是字节级断点续传。

本机远程功能依赖 OpenSSH 客户端、GNU tar、Python 3；远端需要 SSH 和 Python 3，无需安装 Rust 或 srm。
在启用前由管理员核对并登记 SSH 主机密钥，再执行：

```bash
srm remote init          # 创建并检查远端私有目录
srm remote status        # 本地队列、尝试次数、错误、远程对象编号
srm remote sync          # 前台执行队列，适合排错或 systemd
srm remote list          # 远程已提交的归档目录
srm restore ID           # 本地缓存已释放时，会下载、校验、恢复
srm remote restore OBJECT -t /existing/recovery-directory
```

远程恢复先校验摘要，再检查归档路径及链接结构，最后在私有暂存目录中解包。
恢复过程不自动删除远端历史归档。没有本地记录时，也能从远程目录选择对象恢复，必须显式指定目标。
需要永久删除远端历史归档或已知编号的不完整上传时：

```bash
srm remote delete OBJECT          # 默认询问
srm remote delete OBJECT --yes
```

对仍有本地记录的项目，`srm clean`/`empty` 会清除其已确认的远程归档及本地副本。
尚未成功上传的本地数据会被保留，即使已经过期；不会因为远程离线就静默永久删除。
已恢复后留在服务器上的历史归档需要通过 remote delete 管理；当前没有自动清理这类历史对象的远端策略。
崩溃可能留下 `.part`、`.pending` 或未被本地记录引用的远程归档，它们不会被当作已提交备份。
正常重试会清理已知旧对象；更换远端目标时会保留旧机器上的对象，需另行管理。

修正配置后，显式把待上传项目切换到当前目标：

```bash
srm remote retarget       # 默认询问；不会更改已成功上传的条目
srm remote sync
```

## 同步远程模式

`backend: remote_sync` 先建立本地副本，再等待远程确认，最后删除原路径。
上传失败时原路径仍在。上传期间源对象发生变化时也会拒绝删除原路径。
这种模式需要本地复制空间与传输等待时间，不适合要求即时返回的超大文件任务。
持续并发写入不具备文件系统快照语义；必须避免在需要精确备份时继续写入目标。

## 小文件和大文件如何加速

默认 checked 模式仍检查目录权限，因此目录中条目越多，扫描时间越长。
选择 `directory_strategy: rename` 后，在无逐项交互、无挂载子树等需要递归处理的条件下，整目录直接重命名；不会为统计大小扫描每个子文件。
**该策略可能整体移动含不可删除子项的目录，与 GNU rm 的逐项权限检查不同，必须显式选择。**

多个同盘独立文件使用分组的持久化预备日志，先把恢复信息落盘，再批量重命名，最后一次性提交状态。
每个文件仍有独立 ID；崩溃时可以从批次记录定位对象。对应的本地批次清理也批量提交日志，避免逐文件反复同步整份批次元数据。
逐项交互、重复参数、跨盘混合参数等情况会保留逐项处理；不为了性能静默改变交互规则。

大文件同盘移动不会读写全部内容。其他磁盘可以配置：

```json
{
  "backend": "local",
  "volume_stores": ["/data/.safe-srm-1000", "/archive/.safe-srm-1000"],
  "directory_strategy": "checked"
}
```

这些目录必须由执行用户拥有且为 0700；管理员可提前创建。每个目录使用独立的 `files/` 子目录。
中央元数据记录实际位置，配置变更后仍可通过保存的卷目录注册表恢复旧条目。
回收目录应在工作目录之外，不能放在准备整体删除的目录里面。
没有可用的同盘回收目录时，跨盘仍需复制，无法保证大文件即时返回。

对于数百万个文件，优先删除整个目录；shell 的通配符展开可能触及 Linux 参数长度限制。
需要逐项筛选时可用 `find ... -print0 | xargs -0 -r srm --rm`，每次调用内部再分组。
快速移走路径、远程上传完成、真正释放磁盘空间是三个不同的时间点。网络传输与物理清理速度受带宽、文件系统和设备限制。

## sudo 与 root

```bash
sudo bash scripts/install-system.sh --replace-rm --enable-upload-timer
sudo /usr/local/bin/srm --rm /root/example-file
sudo /usr/local/bin/srm list
sudo /usr/local/bin/srm restore ID
```

系统程序放在 `/usr/local/lib/safe-srm/srm`，由 root 拥有；入口是 `/usr/local/bin/srm` 和可选的 `/usr/local/bin/rm`。
不覆盖 `/usr/bin/rm`、不修改 sudoers，也不绕过 sudo 的身份认证。
当 sudo 的 secure_path 优先包含 `/usr/local/bin` 时，`sudo rm` 会使用此程序；其他系统使用上面的绝对路径即可。
root 默认回收站是 `/var/lib/safe-srm/root`，普通用户默认仍为自己的 XDG 数据目录。
root 和用户分别配置远程认证。namespace 只是目录分类，不是权限边界；若要隔离 root 备份，应使用独立远程账号或服务器端访问控制。
root 文件的属主、权限和扩展属性由归档与特权恢复保留。普通用户不会通过这个程序获得额外权限。
只读挂载、不可变属性、内核或文件系统拒绝的操作仍会失败，sudo 不代表强行绕过这些限制。

## 自动重试

普通用户安装时增加 `--enable-upload-timer`，root 系统安装也支持同名参数。
它仅重试已有上传队列，不会自动开启远程模式，不会启用过期永久清理。

```bash
systemctl --user status safe-srm-upload.timer
sudo systemctl status safe-srm-upload.timer
```

上传日志位于中央回收站的 `upload.log`，或对应 systemd journal。
对使用自定义 `--config`/SRM_HOME 的实例，应为服务明确设置对应参数，避免服务读取默认实例。
如果将 backend 改为 local，自动上传会停止；已有队列保留。
