# 运行器初始化配置

选择此传输方式前，通过[能力查询](capabilities.md)确认
`runner.initialize-config.v1`。它是原有 `--config` 契约之外的明确选项，
不依赖版本号推测，也不会自动回退。

```text
tunnelde-agent start --directory <已存在的绝对目录> --id <唯一标识> --bootstrap initialize-v1 --executable <所选运行时> --entry <所选子程序>
```

向 stdin 写入一个完整的 UTF-8 JSON 文档并关闭输入：

```json
{"schemaVersion":1,"configuration":{},"secrets":{"provider.main":"仅为示例"}}
```

配置必须是序列化后至多 200 KiB 的对象；秘密必须是序列化后至多 256 KiB 的字符串映射。
总输入上限为 512 KiB，须在 10 秒内结束。未知信封字段、重复已知字段、不兼容版本、
无效 UTF-8、尾随第二个文档，以及同时使用 `--config` 和 `--bootstrap` 均被拒绝。
错误不回显输入值。完整输入通过校验前，不会创建任务目录。任务目录采用排他创建，
Unix 上在创建时即指定 0700 权限。

监督进程以 `--initialize-config` 启动所选子程序，并通过 JSON-RPC initialize 参数
传递 `remote: true`、`configurationTransport: "initialize-v1"`、`configuration`
和 `secrets`。子程序必须明确支持该契约，并在使用配置前验证自身插件身份和配置。
配置和秘密不进入命令参数或辅助程序配置文件。此模式下 `storage.get` 默认读取空存储，
不再自动以启动配置播种；`secrets.get` 仍可从内存读取秘密。

主动调用 `storage.set`、子程序代码、运行时行为及系统崩溃处理仍可能保存数据。
该传输方式不承诺任意配置永不落盘。监督进程还会保存普通任务日志和审批消息，
插件应避免向这些消息发布秘密。

原文件模式、MCP 桥接、状态查询和停止仍受支持。启动成功响应只确认已向监督进程
提交启动，不代表插件已经初始化成功。响应丢失时结果未知；调用方应检查和停止同一任务，
不能重复启动。传递初始化输入失败时，会尽力终止自己持有的监督进程，同时保留任务文件
供恢复检查，不隐含递归清理。
