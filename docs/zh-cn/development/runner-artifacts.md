# 任务残留元数据观察

[开发文档](README.md) · [English](../../en-us/development/runner-artifacts.md)

`runner.artifact-observation.v1` 声明只读 `inspect-artifacts` 命令。
调用前必须同时协商 `runner.initialize-config.v1`、`runner.admission-fence.v1`
和任务协议 1，不能只凭版本号启用。这是新增源码能力，已发布的 0.27.0
二进制尚不包含该能力。原有停止和状态操作保持兼容。

命令严格接受四组参数：`--directory` 为绝对路径的私有部署目录，`--id` 为
原任务安全标识，`--admission` 为原登记令牌，`--observation` 为本次新生成的
请求 UUID。UUID 必须是带连字符、规范小写且非零的格式。未知或重复参数会被
拒绝，诊断不回显参数值。

观察只打开既有登记锁及完整、匹配的日志，不创建部署、任务、锁、日志或登记。
枚举期间持有合作锁，返回前重新核对保留的目录句柄和登记日志身份。登记缺失、
损坏或不匹配会返回不可用。该操作不解释进程是否结束。

schema 1 回执绑定 `runnerId`、`token` 和 `observationId`，包含可空 `admission`、
`consistency: "pointInTime"`、`cleanupAuthorized: false` 及 `snapshot`：

- `missing`：未观察到任务目录，已读取匹配登记。
- `present`：`entries` 仅包含观察到的 pid、heartbeat、STOP、status.json、
  events.jsonl、events.previous.jsonl、approvals.json、storage.json、inbox。
  每项仅有 name、kind（file/directory/link/other）及 sizeBytes（普通文件为
  u64 规范十进制字符串，其他类型为 null）。`otherEntries` 只计数其他直接子项，
  不返回其名字。
- `unavailable`：原因限定为 admissionUnavailable、unsafeDirectory、
  namespaceChanged、enumerationLimit 或 metadataUnavailable，不回传路径、
  系统原始错误或文件内容。

最多枚举 256 个直接子项，不递归，不读取配置、状态、日志、消息或存储正文。
部署或任务目录是符号链接/重解析点时拒绝；已知叶节点链接只记录类型，不跟随。
POSIX 上部署和任务目录必须为私有权限。输出不超过 16384 字节；消费者应限制
stdout/stderr 合计大小，并设置 12 秒时限，包含既有最长 8 秒的合作锁等待。

这不是原子快照，观察期间和返回后都可能发生变化。路径元数据配合保留句柄
核对不能锁住任意写入者，也不能保证对象不被替换。名称、大小、目录身份或
登记状态一致，都不证明残留归属、未知写入结束或可以安全删除。登记日志是
独立的合作协议，不是文件归属日志。该操作不产生清理或修改回执，也不能替
helper 从未负责的历史 SFTP 操作确认完成。

宿主必须在能力查询及本次观察之后分别重新核对整条恢复记录与原 SSH 目标，
失败结果也不能省略核对。结果不持久化，过期结果应丢弃。不能缓存能力作为
授权、重放启动/写入，或据此移除未知副作用的恢复证据。
