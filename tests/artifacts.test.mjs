import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { mkdtempSync, mkdirSync, readdirSync, readFileSync, writeFileSync, rmSync, symlinkSync, linkSync, lstatSync, chmodSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const helper = process.env.TUNNELDE_TEST_HELPER || resolve(here, "../target/debug", process.platform === "win32" ? "tunnelde-agent.exe" : "tunnelde-agent");
function fixture() {
  const root = mkdtempSync(join(tmpdir(), "tunnelde-artifacts-")), id = "task", token = randomUUID(), observation = randomUUID();
  const args = operation => [operation, "--directory", root, "--id", id, "--admission", token, ...(operation === "inspect-artifacts" ? ["--observation", observation] : [])];
  const run = values => spawnSync(helper, values, { encoding: "utf8", windowsHide: true, timeout: 15000 });
  const call = () => { const result = run(args("inspect-artifacts")); assert.equal(result.status, 0, result.stderr || result.error?.message); assert.equal(result.stderr, ""); assert(Buffer.byteLength(result.stdout) <= 16384); return JSON.parse(result.stdout); };
  const reserve = () => { const result = run(args("reserve-fenced")); assert.equal(result.status, 0, result.stderr); };
  const journal = join(root, ".admission-v1", id + ".jsonl"), task = join(root, id);
  return { root, id, token, observation, args, run, call, reserve, journal, task, clean: () => rmSync(root, { recursive: true, force: true }) };
}

function inventory(root) {
  return readdirSync(root).sort().map(name => {
    const path = join(root, name), stat = lstatSync(path);
    return [name, stat.mode, stat.mtimeMs, stat.isSymbolicLink() ? "link" : stat.isDirectory() ? inventory(path) : readFileSync(path).toString("base64")];
  });
}

test("capability discovery explicitly advertises artifact metadata observation", () => {
  const result = spawnSync(helper, ["--capabilities"], { encoding: "utf8", windowsHide: true });
  assert.equal(result.status, 0, result.stderr);
  assert(JSON.parse(result.stdout).features.includes("runner.artifact-observation.v1"));
});

test("observation never creates missing admission or task data", () => {
  const f = fixture(); try {
    let before = inventory(f.root);
    assert.deepEqual(f.call().snapshot, { status: "unavailable", reason: "admissionUnavailable" });
    assert.deepEqual(inventory(f.root), before);
    f.reserve(); before = inventory(f.root);
    assert.deepEqual(f.call(), { schemaVersion: 1, runnerId: f.id, token: f.token, observationId: f.observation, admission: "reserved", consistency: "pointInTime", cleanupAuthorized: false, snapshot: { status: "missing" } });
    assert.deepEqual(inventory(f.root), before);
  } finally { f.clean(); }
});

test("only fixed names, types, byte sizes and bounded unknown counts escape", () => {
  const f = fixture(); try {
    f.reserve(); mkdirSync(f.task, { mode: 0o700 });
    for (const name of ["pid", "status.json", "storage.json", "events.jsonl"]) writeFileSync(join(f.task, name), "private-secret-contents", { mode: 0o600 });
    mkdirSync(join(f.task, "inbox"), { mode: 0o700 });
    writeFileSync(join(f.task, "inbox", "private-message.json"), "private-secret", { mode: 0o600 });
    writeFileSync(join(f.task, "private-secret-filename"), "private-secret", { mode: 0o600 });
    const before = inventory(f.root), receipt = f.call();
    assert.equal(receipt.admission, "reserved"); // not a process-state interpretation
    assert.deepEqual(receipt.snapshot, { status: "present", entries: [
      { name: "pid", kind: "file", sizeBytes: "23" },
      { name: "status.json", kind: "file", sizeBytes: "23" },
      { name: "events.jsonl", kind: "file", sizeBytes: "23" },
      { name: "storage.json", kind: "file", sizeBytes: "23" },
      { name: "inbox", kind: "directory", sizeBytes: null }
    ], otherEntries: 1 });
    assert(!JSON.stringify(receipt).includes("private-secret"));
    assert.deepEqual(inventory(f.root), before);
  } finally { f.clean(); }
});

test("missing, corrupt, partial, oversized or mismatched journals stay unavailable and intact", () => {
  const f = fixture(); try {
    f.reserve(); const valid = readFileSync(f.journal, "utf8");
    for (const bytes of ["", valid.trimEnd(), valid + valid, "x".repeat(1025), valid.replace(f.token, randomUUID()), valid.replace('"schemaVersion":1', '"schemaVersion":2')]) {
      writeFileSync(f.journal, bytes); const before = inventory(f.root);
      assert.deepEqual(f.call().snapshot, { status: "unavailable", reason: "admissionUnavailable" });
      assert.deepEqual(inventory(f.root), before);
    }
    rmSync(f.journal); const before = inventory(f.root);
    assert.equal(f.call().snapshot.reason, "admissionUnavailable"); assert.deepEqual(inventory(f.root), before);
  } finally { f.clean(); }
});

test("strict duplicate/unknown options and noncanonical request identities fail before observation", () => {
  const f = fixture(); try {
    const original = f.args("inspect-artifacts");
    for (const values of [original.slice(0, -2), [...original, "--extra", "secret"], [...original, "--id", "task"], original.map(value => value === f.observation ? "00000000-0000-0000-0000-000000000000" : value), original.map(value => value === f.token ? "bad" : value), original.map(value => value === "--observation" ? "--admission" : value), original.map(value => value === f.id ? "../task" : value)]) {
      const result = f.run(values); assert.notEqual(result.status, 0); assert.equal(result.stdout, ""); assert(!result.stderr.includes("secret")); assert.deepEqual(readdirSync(f.root), []);
    }
  } finally { f.clean(); }
});

test("enumeration caps all entries and never descends or leaks unknown names", () => {
  const f = fixture(); try {
    f.reserve(); mkdirSync(f.task, { mode: 0o700 });
    for (let n = 0; n < 256; n++) writeFileSync(join(f.task, `private-${n}`), "");
    assert.deepEqual(f.call().snapshot, { status: "present", entries: [], otherEntries: 256 });
    writeFileSync(join(f.task, "overflow"), ""); const before = inventory(f.root);
    assert.deepEqual(f.call().snapshot, { status: "unavailable", reason: "enumerationLimit" });
    assert.deepEqual(inventory(f.root), before);
  } finally { f.clean(); }
});

test("redirected task/admission namespaces and hardlinked journals cannot confer authority", () => {
  const f = fixture(), outside = mkdtempSync(join(tmpdir(), "tunnelde-artifact-outside-")); try {
    f.reserve(); writeFileSync(join(outside, "storage.json"), "private-secret", { mode: 0o600 });
    symlinkSync(outside, f.task, process.platform === "win32" ? "junction" : "dir");
    assert.deepEqual(f.call().snapshot, { status: "unavailable", reason: "unsafeDirectory" });
    rmSync(f.task, { recursive: true });
    const backup = join(f.root, "journal-backup"); linkSync(f.journal, backup);
    assert.equal(f.call().snapshot.reason, "admissionUnavailable");
    assert.equal(readFileSync(join(outside, "storage.json"), "utf8"), "private-secret");
  } finally { f.clean(); rmSync(outside, { recursive: true, force: true }); }
});

test("leaf directory links are metadata only and never traversed", () => {
  const f = fixture(), outside = mkdtempSync(join(tmpdir(), "tunnelde-artifact-leaf-")); try {
    f.reserve(); mkdirSync(f.task, { mode: 0o700 });
    writeFileSync(join(outside, "private-name"), "private-secret");
    symlinkSync(outside, join(f.task, "inbox"), process.platform === "win32" ? "junction" : "dir");
    assert.deepEqual(f.call().snapshot, { status: "present", entries: [{ name: "inbox", kind: "link", sizeBytes: null }], otherEntries: 0 });
  } finally { f.clean(); rmSync(outside, { recursive: true, force: true }); }
});

test("nonprivate task namespace is not inspected on POSIX", { skip: process.platform === "win32" }, () => {
  const f = fixture(); try {
    f.reserve(); mkdirSync(f.task, { mode: 0o700 }); chmodSync(f.task, 0o755);
    assert.equal(f.call().snapshot.reason, "unsafeDirectory");
  } finally { f.clean(); }
});
