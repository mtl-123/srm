# GitHub 自动构建与发布

本仓库的 `.github/workflows/srm-ci.yml` 提供：

- 提交 main/master、PR 或手动运行：检查格式、Clippy、脚本语法，构建 GNU 与 musl x86_64 Release。
- 两个目标都运行功能、语言、Tab 补全、root、真实 SSH、挂载边界及磁盘不足测试。
- musl 构建额外验证 ELF 无动态解释器及共享库依赖。
- 每次成功构建上传单独二进制、完整安装包和 SHA-256，Actions artifact 保留 14 天。
- 推送 `vX.Y.Z` 标签：验证标签与 Cargo.toml 一致，所有测试通过后自动创建正式 GitHub Release。
- 已存在的 Release 不覆盖，避免重复运行替换已发布二进制。

不需要手工配置 PAT：Release 作业使用 GitHub 自动提供的 GITHUB_TOKEN，仅该作业申请 contents:write。
如组织策略禁用了 Actions 或禁止写仓库，需要由管理员放行；本地没有修改这些仓库设置。

## 首次提交

工作目录必须是 `srm` 仓库（origin: git@github.com:mtl-123/srm.git），不是上一级包含历史项目的目录。
`safe-rm-rs` 保留为历史对照，未修改。不要把它或上一级 artifacts 一并提交。

```bash
git status --short
git add .
git diff --cached --stat
git commit -m "Integrate recoverable deletion, remote archives and Linux release CI"
git push origin HEAD
```

新仓库只包含源码，不包含历史仓库误提交的 `target/` 编译缓存或其 Git 历史。
`dist/` 中的本地安装包保留，不能作为源码提交；构建发布文件进入 Release。

## 发布版本

先确认该版本标签不存在、Cargo.toml/Cargo.lock 中版本一致，且普通分支 CI 已通过。

```bash
git tag -a v2.1.0 -m "safe-srm 2.1.0"
git push origin v2.1.0
```

在 GitHub Actions 查看运行结果。全部通过后会自动发布二进制、安装包与校验文件。
版本已使用时应递增版本，不要移动旧标签或覆盖旧 Release。
手动 workflow_dispatch 只构建和上传 artifact，不自动创建版本标签。

推荐下载名称含 `x86_64-unknown-linux-musl` 的通用静态包。
GNU 包在 ubuntu-24.04 runner 上构建，不能宣称兼容比构建环境更旧的 glibc。
musl 包仍有内核、文件系统及可选外部工具要求，详见 LINUX-COMPATIBILITY.md。

本地验证不等于 GitHub 托管 runner 已运行；首次推送后应检查真实 CI 结果。
