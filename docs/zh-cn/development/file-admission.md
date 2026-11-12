# 文件准入账本（实验性）

`files.admission-ledger.v1` 是通过 `--capabilities` 发现的独立文件领域协议，与
runner 准入和辅助程序发行版本分开。目前只记录准入／取消，**不提供文件执行器、
发布回执或清理权限**。桌面应用尚未接入，不能恢复旧 SFTP 未知请求。

## 授权与传输

明确选择辅助程序，并准备私有的绝对状态目录。执行
`files-admission --directory <状态目录>`，通过 stdin 发送一个 UTF-8 JSON 对象并
关闭 stdin。输入／输出分别不超过 4096 字节，输入须在十秒内完成，随后才会打开
存储。令牌仅通过 stdin 传输，不能进入 argv／环境变量。重复／未知字段、额外 CLI
参数和无效请求均被拒绝，不回显请求，也不创建存储。

初始化要求明确声明使用本地存储：

```json
{"method":"initialize","schemaVersion":1,"storage":"local"}
```

响应包含 `schemaVersion: 1`、`protocol: "files.admission-ledger.v1"`、随机
`authorityId`、`observation: null`、`executionSupported: false` 和
`cleanupAuthorized: false`。重复初始化返回同一账本代次。已有的空、外来、截断或
损坏数据库均被拒绝并保留，不会重置；首次初始化中断后也可能需要诊断。

后续请求必须包含以下全部字段：

| 字段 | 契约 |
|---|---|
| `method` | `reserve`、`cancel` 或 `status` |
| `schemaVersion` | 整数 1 |
| `authorityId` | 先前协商的账本代次 UUID |
| `operationId` | 原操作 UUID；重试不能更换 |
| `generation` | 原所有者／运行代次 UUID |
| `bindingSha256` | 主机、账户、作用域绑定摘要 |
| `intentSha256` | 不可变操作意图摘要 |
| `token` | 私有随机 256 位令牌，小写十六进制 |

UUID 使用规范的小写连字符形式，且非全零。摘要／令牌正好 64 个小写十六进制字符，
全零令牌被拒绝。存储仅包含带领域前缀的令牌摘要；响应不包含令牌或令牌摘要。
本协议不接受用户文件路径或正文。

后续宿主适配器必须以独立版本的规范编码，将摘要绑定至经过认证的 SSH 主机密钥／
账户、授权的规范作用域、目标父目录／对象身份、基线和正文摘要。本基础协议尚未
定义该编码，也不验证声称的 SSH 身份。授权／绑定契约和经过验收的服务端执行器
是启用任何修改适配器的前提。

## 状态机与持久化

| 已有状态 | reserve | cancel | status |
|---|---|---|---|
| Missing | Reserved，修订 1 | Cancelled，修订 1 | Missing，修订 0；不创建记录 |
| Reserved，修订 1 | 原记录 | Cancelled，修订 2 | 原记录 |
| Cancelled，修订 1 或 2 | 原取消记录 | 原取消记录 | 原记录 |

独立进程通过保留的控制文件锁串行处理，等待锁最多五秒。精确重复请求返回原状态。
同一操作 ID 的代次／绑定／意图／令牌变化，或账本代次不同，均返回
`files.admission.identityMismatch`。先取消后准入会留下持久取消占位；它只能约束
未来遵守本账本的执行器，不能约束独立 SFTP 通道。

`observation` 返回操作 ID、代次、绑定／意图摘要、修订和状态。
**Missing、Reserved、Cancelled 都不是文件发布回执**，两个否定权限字段始终为 false。
查询不能重放、抹掉未知结果、授权清理或创建记录。stdout 丢失、超时或非零退出均表示
未收到确认，应在新进程中使用原身份查询。

本地 SQLite 存储采用 immediate 事务、DELETE 回滚日志、synchronous EXTRA 和
4096 字节页。最多 1024 个操作记录、4 MiB 数据库页。满额时拒绝新操作，不驱逐记录；
已有查询／取消仍可进行，但取决于磁盘 I/O 成功。没有过期、重置、删除或压缩命令。
提交前进程丢失通过 SQLite 回滚恢复；持久提交发生在 stdout 确认之前。

不含敏感数据的 `files.admission.*` 错误包括 `invalidArguments`、`invalidRequest`、
`inputTimeout`、`notInitialized`、`unsafeStorage`、`storageChanged`、
`storageUnavailable`、`corrupt`、`capacity`、`busy`、`identityMismatch`、
`invalidResponse`、`responseUnavailable`。失败后保留存储；错误不能作为操作缺失或
已取消的证据。

## 支持边界与后续门槛

使用文件锁和 SQLite 持久化语义正常的 Windows／Linux／macOS 私有可信
**本地文件系统**。Unix 对象须属于有效用户，不授予组／其他用户权限；Windows
调用方须提供账户私有 ACL。控制对象的符号链接／重解析点，以及多硬链接控制文件／
数据库会被拒绝。Unix 系统祖先目录的别名会在保留并核对所选根目录身份后规范化；
所选根目录本身是符号链接时仍会被拒绝。目录／锁／数据库／回滚日志须保存在一起；仍可能存在迟到命令时，
不能重命名、复制、截断或删除。网络／共享／同步存储、不可信控制目录写入者、不诚实
硬件，以及超出 SQLite／OS／文件系统契约的物理断电保证均不支持。

控制锁不锁定用户数据，不能提供针对外部写入者的文件系统 CAS。
`意图 fsync -> rename -> 回执 fsync` 仍存在发布后的崩溃歧义窗口。执行、权威回执
重建、重启后排除旧执行器、宿主原代次恢复、桌面显式安装授权和安全移动仍是独立
门槛；旧 SFTP 未知结果缺少原操作权威证明时，继续保留未知。
