# 辅助程序能力查询

对用户明确选择的辅助程序执行 `--capabilities`，不附加其他参数。
程序输出一个 UTF-8 JSON 对象及换行，以状态 0 退出。
查询不读取 stdin、凭据或任务文件，不创建文件、启动运行环境或访问网络。
附加参数会在任务参数解析前被拒绝，错误信息不回显参数内容。

```json
{
  "schemaVersion": 1,
  "helper": "tunnelde-agent",
  "helperVersion": "0.27.0",
  "runnerProtocols": [1],
  "features": ["runner.control.v1", "runner.initialize-config.v1"]
}
```

`helperVersion` 仅供展示；`schemaVersion` 是查询信封版本，`runnerProtocols`
列出支持的任务协议。`runner.control.v1` 表示既有 start/send/poll/stop/status
协议，仍使用配置文件路径；它不表示支持 stdin 配置、残留核查或清理。
`runner.initialize-config.v1` 独立声明支持[初始化配置](runner-initialization.md)。
内部 `serve` 不是宿主调用入口。

调用方必须将 stdout/stderr 合计限制为 4096 字节，并为查询设置超时。
确认辅助程序身份和信封版本后才能解释能力。版本 1 必须包含上述五个字段及其类型。
字符串使用非空白可打印 ASCII：辅助程序版本为 1–64 字节；能力名为 1–64 字节，
只使用小写字母、数字、点和连字符。列表最多 32 个不重复条目；协议是正 32 位整数。
为兼容后续扩展，可忽略有界的未知字段及能力，但不能把它们当作操作授权。
重复的已知 JSON 键无效。

旧程序可能拒绝此命令。非零退出、缺少退出状态、超时、传输失败、格式错误和不支持的
信封版本是不同结果，均不能证明具体旧版本或某种能力可用。既有停止和状态恢复不依赖
查询成功。不得自动下载或选择其他辅助程序或运行环境。查询不能确认未知写入或延迟
启动已经结束，不能证明残留文件归属，也不授权删除。
