# 持久任务启动登记

通过能力查询确认 `runner.admission-fence.v1`、`runner.initialize-config.v1`
和任务协议 1 后才能使用。此实验性增量协议先关闭启动许可，再将不存在的任务确认
为已取消。旧传输模式保持原有语义，不能根据版本号推断支持。

1. 选择唯一任务 ID 和规范形式的非空 UUID 令牌，在发起任何修改前持久化。
2. 执行 `reserve-fenced --directory <部署目录> --id <任务ID> --admission <令牌>`，
   要求成功回执绑定相同任务和令牌，状态为 `reserved`、`notFound`。
3. 只执行一次 `start --directory <部署目录> --id <任务ID> --admission <令牌>
   --bootstrap initialize-fenced-v1 --executable <运行时> --entry <子程序>`。
   输入大小、完整文档和 EOF 要求与[初始化配置](runner-initialization.md)相同。
4. 取消或启动结果未知时，用相同目录、ID 和令牌执行 `stop-fenced`，再通过
   `status-fenced` 核对。不得重放启动。

控制命令只接受上述三对参数。成功时退出码为 0，输出单个 JSON 文档，stdout/stderr
合计不得超过 4096 字节。宿主时限 15 秒，helper 等待锁最多 8 秒。

```json
{"schemaVersion":1,"runnerId":"task","token":"217636b5-c7e8-4822-9bdf-717d400fd26e","admission":"cancelled","status":"notFound"}
```

五个字段全部必填。重复或未知字段、版本错误、任务或令牌不符、缺失或非零退出、
输出超限、无效状态均不能作为证明。登记状态为 `reserved`、`launched`、`cancelled`；
进程状态为 `starting`、`running`、`stopping`、`stopped`、`failed`、`notFound`、
`unresponsive`。`reserved` 只允许搭配 `notFound`。取消启动许可不代表进程已退出，
仍运行的任务需要 STOP 和后续核对。

helper 在私有 `.admission-v1/` 中用保留的系统文件锁串行处理合作命令。每次确认或
创建进程前，先刷新有界追加日志。启动只能消费已有且匹配的登记，不能自行创建登记。
读取启动输入期间可以取消，迟到的登记不能恢复已取消的许可。损坏、不完整日志、
异常对象类型、链接或不支持的锁均保留未知状态并拒绝继续。

每个部署最多保留 4096 份任务日志，每份不超过 1024 字节，另有一个锁文件。达到
上限后拒绝新登记，已有任务仍可取消。迟到命令仍可能存在时，不得删除或过期这些
回执；部署退出需要独立协议，不能用定时器或递归清理代替。

该契约要求合作命令共享同一私有部署，文件系统遵守系统锁和刷新语义；不保证抵抗
同用户任意替换命名空间、磁盘回滚或任意断电。控制数据缺失会阻止启动，无法核实
或损坏的状态留待人工检查。它不确认历史 SFTP 写入结束，也不授权删除残留。
