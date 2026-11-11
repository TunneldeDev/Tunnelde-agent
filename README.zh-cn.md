# Tunnelde Agent

[Documentation / 文档](docs/README.md)

[English](README.en-us.md)

[Tunnelde](https://github.com/TunneldeDev/Tunnelde) 的可选原生 MCP stdio 桥接程序和持久远端监督进程。
普通 SSH、SFTP、终端、本地 API 对话以及通过 SSH 直接运行 AI agent 不需要它。
辅助程序不包含 Node；外部运行环境仍需用户明确授权。

## 下载与兼容性

从本仓库 [Releases](https://github.com/TunneldeDev/Tunnelde-agent/releases) 下载对应系统和架构的
二进制及 `SHA256SUMS`，核对 SHA-256 后在 Tunnelde 中选择。macOS/Linux 下载后需执行
`chmod +x <文件>`。持久远端运行时应选择远端机器的系统与架构。

本仓库保留桌面仓库中辅助程序的 Git 历史，迁移来源为桌面 dev
`1b95417252173e731c60b3d64c1327124d09dad7`，导入源码树为
`a12394357c54329e77e5659181939bfbdee03de3`。继承的基线版本为 **0.26.1**，
后续独立于桌面软件版本。迁移保留原命令参数、运行记录格式和协议行为。
不要替换正在运行的监督进程二进制。

## MCP stdio

在 Tunnelde MCP 中启动服务并创建有作用域的令牌。AI 客户端 stdio 命令设置为辅助程序的绝对路径，
参数为 `--mcp-stdio`；环境变量 `TUNNELDE_MCP_URL` 指向插件的数字回环 HTTP 地址，
`TUNNELDE_MCP_TOKEN` 保存令牌。桌面程序和插件需要保持运行。
辅助程序拒绝非回环地址、URL 内嵌凭据、重定向和代理。令牌通过 HTTP 头发送，避免写入命令参数。
只有用户明确选择 Node 后，才可改用插件提供的 Node 桥接程序。

## 持久远端运行

常规远端运行使用对话中的机器选择器及 SSH stdio，无需此程序。
详见[常规助手设置](https://github.com/TunneldeDev/Tunnelde/blob/dev/docs/zh-cn/guide/ai-assistants.md)。
高级持久监督进程需要用户选择 SSH 主机、全新的绝对部署目录、本机上的对应辅助程序，以及明确的远端运行环境路径。
宿主在上传插件与辅助程序前请求确认，不安装 Node，不修改系统服务。
POSIX 目录权限为 0700，配置为 0600；Windows 继承账户目录 ACL，应选择私有用户目录。

监督进程不注册为系统服务。凭据通过 SSH stdin 接收，仅保留在内存中；对话状态、有界输出、
事件和待审批内容保存在部署目录，需按私有应用数据管理。桌面断开时审批继续等待；重连后先检查任务，
再恢复。停止或限制插件会记录无法访问的 runner，并在后续重连时重试；停止请求不代表远端进程已退出。
关闭桌面程序会保留持久远端任务。

插件和 agent CLI 是以账户系统权限运行的受信任代码。工具 API 的路径与审批校验不等同于操作系统沙箱。
Claude Code 和 Pi 的 ReadOnly 执行会被拒绝；Codex 使用自身只读沙箱。
需要宿主强制只读文件工具时使用 API 配置。

## 构建与测试

只读协议协商见[辅助程序能力查询](docs/zh-cn/development/capabilities.md)。

```sh
cargo fmt -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --locked
node --test tests/*.test.mjs scripts/*.test.mjs
```

集成测试明确使用开发环境的 Node 启动隔离插件样例，发布的辅助程序不探测或安装 Node。
可通过 `TUNNELDE_TEST_HELPER` 指定待测试的二进制。优化构建使用 `--release`，输出位于 `target/release/`。

## 贡献与发布

通过 PR 向 `main` 贡献。Linux、Windows、Intel macOS CI 和恢复测试必须通过；macOS CI 还交叉编译
Apple Silicon。按功能 squash 合并。同步修改 Cargo 清单和锁文件版本，合并审查通过的 PR 后推送 `vX.Y.Z`。
发布流程检查标签属于主分支且版本一致，为五个平台构建并测试，核对哈希后才公开。
已公开资产不可替换；失败草稿可使用同一不可变标签重试。桌面版本发布不会触发本仓库发布。
协议修改需要兼容性审查和行为测试。

采用 GPL-3.0-only 许可证，见 [LICENSE](LICENSE)。
