import test from "node:test";
import process from "node:process";
import { setTimeout } from "node:timers";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, writeFileSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const directory = dirname(fileURLToPath(import.meta.url));
const helper = resolve(
  directory,
  "../target/debug",
  process.platform === "win32" ? "tunnelde-agent.exe" : "tunnelde-agent",
);
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

test(
  "durable runner keeps RPC, scoped tools and approvals across disconnected clients",
  { timeout: 30000 },
  async () => {
    const root = mkdtempSync(join(tmpdir(), "tunnelde-runner-test-"));
    const id = "test-runner";
    const config = join(root, "config.json");
    writeFileSync(config, JSON.stringify({ root }));
    const run = (command, extra = [], input) => {
      const result = spawnSync(helper, [command, "--directory", root, "--id", id, ...extra], {
        input,
        encoding: "utf8",
        timeout: 5000,
        windowsHide: true,
      });
      assert.equal(result.status, 0, result.stderr || result.error?.message);
      return JSON.parse(result.stdout);
    };
    const poll = () => run("poll");
    const send = (id, input) =>
      run(
        "send",
        [],
        JSON.stringify({
          jsonrpc: "2.0",
          id,
          method: "command.run",
          params: { command: "test", input },
        }) + "\n",
      );
    const wait = async (predicate) => {
      for (let count = 0; count < 100; count++) {
        const result = poll();
        const found = predicate(result);
        if (found) return found;
        await pause(70);
      }
      assert.fail("Runner did not produce the expected event");
    };
    const response = (id) =>
      wait(
        (result) =>
          result.events.find(
            (event) => event.message.type === "response" && event.message.message.id === id,
          )?.message.message,
      );
    try {
      const start = run(
        "start",
        [
          "--config",
          config,
          "--executable",
          process.execPath,
          "--entry",
          join(directory, "fixture-plugin.mjs"),
        ],
        JSON.stringify({ secrets: { test: "memory-only-test-secret" } }) + "\n",
      );
      assert.equal(start.runnerId, id);
      await wait((result) =>
        result.events.some(
          (event) => event.message.type === "event" && event.message.event === "ready",
        ),
      );
      send(1, { message: "works after launcher exits" });
      assert.equal((await response(1)).result.echo.message, "works after launcher exits");
      send(2, { kind: "secret", key: "test" });
      assert.equal((await response(2)).result.present, true);
      // The bootstrap credential is never persisted to config, inbox, storage or process arguments.
      assert.ok(!readFileSync(config, "utf8").includes("memory-only-test-secret"));
      assert.ok(
        !readFileSync(join(root, id, "events.jsonl"), "utf8").includes("memory-only-test-secret"),
      );
      assert.ok(!readdirSync(join(root, id, "inbox")).length);

      send(3, { kind: "confirm" });
      const approval = await wait((result) => result.pendingApprovals[0]);
      const latest = poll().nextSeq;
      assert.equal(
        run("poll", ["--after", String(latest)]).pendingApprovals[0].approvalId,
        approval.approvalId,
      );
      run(
        "send",
        [],
        JSON.stringify({ type: "approval", approvalId: approval.approvalId, approved: true }) +
          "\n",
      );
      assert.equal((await response(3)).result, true);

      const file = join(root, "owned.txt");
      writeFileSync(file, "before");
      const request = (mode, method, params) => ({
        mode,
        method,
        params,
        scope: { localRoots: [root] },
      });
      send(4, {
        kind: "tool",
        request: request("readOnly", "files.local.readText", { path: file }),
      });
      const read = (await response(4)).result;
      assert.equal(read.content, "before");
      assert.match(read.sha256, /^[0-9a-f]{64}$/);
      send(5, {
        kind: "tool",
        request: request("readOnly", "files.local.writeText", { path: file, content: "bad" }),
      });
      assert.match((await response(5)).error.message, /ReadOnly/);
      assert.equal(readFileSync(file, "utf8"), "before");
      send(6, {
        kind: "tool",
        request: request("full", "files.local.readText", {
          path: resolve(root, "..", "outside.txt"),
        }),
      });
      assert.match((await response(6)).error.message, /outside/);
      send(7, {
        kind: "tool",
        request: request("full", "files.local.writeText", {
          path: file,
          content: "after",
          expectedHash: read.sha256,
        }),
      });
      assert.equal((await response(7)).error, undefined);
      assert.equal(readFileSync(file, "utf8"), "after");
      send(8, {
        kind: "tool",
        request: request("full", "files.local.writeText", {
          path: file,
          content: "stale",
          expectedHash: read.sha256,
        }),
      });
      assert.match((await response(8)).error.message, /changed/);
      send(9, { kind: "stream" });
      const streamed = (await response(9)).result;
      assert.equal(
        streamed.events
          .filter((event) => event.stream === "stdout")
          .map((event) => event.data)
          .join(""),
        "hello 你好",
      );
      assert.equal(
        streamed.events
          .filter((event) => event.stream === "stderr")
          .map((event) => event.data)
          .join(""),
        "stderr",
      );
      send(10, { kind: "blocked-input" });
      const blocked = (await response(10)).result;
      assert.match(blocked.error, /timed out/);
      assert.equal(blocked.cancelled, true);
      send(11, { kind: "fragmented-output" });
      assert.equal((await response(11)).result.text, "你好".repeat(12000));
    } finally {
      run("stop");
      await wait((result) => result.state.status === "stopped");
      rmSync(root, { recursive: true, force: true });
    }
  },
);

test(
  "unexpected plugin exit is failed and missing runners are not running",
  { timeout: 15000 },
  async () => {
    const root = mkdtempSync(join(tmpdir(), "tunnelde-runner-exit-"));
    const id = "exit-test";
    const config = join(root, "config.json");
    writeFileSync(config, JSON.stringify({ root }));
    const run = (command, extra = [], input) => {
      const result = spawnSync(helper, [command, "--directory", root, "--id", id, ...extra], {
        input,
        encoding: "utf8",
        timeout: 5000,
        windowsHide: true,
      });
      assert.equal(result.status, 0, result.stderr || result.error?.message);
      return JSON.parse(result.stdout);
    };
    try {
      assert.equal(run("status").status, "notFound");
      assert.equal(run("poll").state.status, "notFound");
      run(
        "start",
        [
          "--config",
          config,
          "--executable",
          process.execPath,
          "--entry",
          join(directory, "fixture-plugin.mjs"),
        ],
        "{}\n",
      );
      run(
        "send",
        [],
        JSON.stringify({
          jsonrpc: "2.0",
          id: 10,
          method: "command.run",
          params: { command: "test", input: { kind: "exit" } },
        }) + "\n",
      );
      let status;
      for (let count = 0; count < 70; count++) {
        status = run("status");
        if (status.status === "failed") break;
        await pause(70);
      }
      assert.equal(status.status, "failed");
      assert.match(status.error, /unexpectedly/);
    } finally {
      run("stop");
      for (let count = 0; count < 70; count++) {
        if (["failed", "stopped"].includes(run("status").status)) break;
        await pause(70);
      }
      rmSync(root, { recursive: true, force: true });
    }
  },
);

test("native MCP bridge rejects non-loopback and embedded credentials", () => {
  for (const url of [
    "http://example.com/mcp",
    "https://127.0.0.1/mcp",
    "http://user:pass@127.0.0.1/mcp",
  ]) {
    const result = spawnSync(helper, ["--mcp-stdio"], {
      encoding: "utf8",
      input: "",
      env: { ...process.env, TUNNELDE_MCP_URL: url, TUNNELDE_MCP_TOKEN: "test" },
      windowsHide: true,
    });
    assert.equal(result.status, 1);
    assert.ok(!result.stderr.includes("user:pass"));
  }
});
