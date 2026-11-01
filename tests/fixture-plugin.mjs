import { createInterface } from "node:readline";
import process from "node:process";
import { setTimeout } from "node:timers";
import { readFileSync } from "node:fs";

const configPath = process.argv[process.argv.indexOf("--config") + 1];
const config = JSON.parse(readFileSync(configPath, "utf8"));
const pending = new Map();
let sequence = 100;
function send(message) {
  process.stdout.write(JSON.stringify(message) + "\n");
}
function call(method, params) {
  const id = ++sequence;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    send({ jsonrpc: "2.0", id, method, params });
  });
}
createInterface({ input: process.stdin }).on("line", async (line) => {
  const message = JSON.parse(line);
  if (!message.method) {
    const request = pending.get(message.id);
    pending.delete(message.id);
    if (request) {
      if (message.error) request.reject(new Error(message.error.message));
      else request.resolve(message.result);
    }
    return;
  }
  try {
    let result;
    if (message.method === "initialize") {
      result = { ok: true };
      await call("events.publish", { event: "ready", payload: { ready: true } });
    } else {
      const input = message.params.input;
      if (input.kind === "fragmented-output") {
        const record = JSON.stringify({
          jsonrpc: "2.0",
          id: message.id,
          result: { text: "你好".repeat(12000) },
        });
        const middle = Math.floor(record.length / 2);
        process.stdout.write(record.slice(0, middle));
        await new Promise((resolve) => setTimeout(resolve, 180));
        process.stdout.write(record.slice(middle) + "\n");
        return;
      }
      if (input.kind === "exit") process.exit(0);
      if (input.kind === "blocked-input") {
        const task = await call("tasks.start", {
          executable: process.execPath,
          args: ["-e", "setInterval(() => {}, 1000)"],
          cwd: config.root,
        });
        let error;
        try {
          for (let index = 0; index < 32; index++)
            await call("tasks.write", { taskId: task.taskId, data: "x".repeat(256 * 1024) });
        } catch (failure) {
          error = failure.message;
        }
        const cancelled = await call("tasks.cancel", { taskId: task.taskId });
        send({ jsonrpc: "2.0", id: message.id, result: { error, cancelled } });
        return;
      }
      if (input.kind === "confirm")
        result = await call("ui.confirm", { message: "Approve the test operation?" });
      else if (input.kind === "tool") result = await call("tools.invoke", input.request);
      else if (input.kind === "secret")
        result = {
          present: (await call("secrets.get", { key: input.key })) === "memory-only-test-secret",
        };
      else if (input.kind === "stream") {
        const task = await call("tasks.start", {
          executable: process.execPath,
          args: ["-e", "process.stdout.write('hello 你好');process.stderr.write('stderr');"],
          cwd: config.root,
        });
        let output;
        do {
          await new Promise((resolve) => setTimeout(resolve, 100));
          output = await call("tasks.readOutput", { taskId: task.taskId });
        } while (output.status === "running");
        result = output;
      } else result = { echo: input };
    }
    send({ jsonrpc: "2.0", id: message.id, result });
  } catch (error) {
    send({ jsonrpc: "2.0", id: message.id, error: { code: -32000, message: error.message } });
  }
});
