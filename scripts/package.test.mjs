import assert from "node:assert/strict";
import {
  readFileSync,
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  rmSync,
} from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { packageName, targets, versionOf, digest } from "./package.mjs";

test("all supported binaries have unambiguous versioned platform names", () => {
  const names = Object.keys(targets).map((platform) =>
    packageName(platform, "0.26.1"),
  );
  assert.equal(new Set(names).size, 5);
  assert(names.includes("tunnelde-agent-0.26.1-windows-x86_64.exe"));
  assert(names.includes("tunnelde-agent-0.26.1-macos-aarch64"));
  for (const platform of ["__proto__", "../linux-x86_64", "windows-arm64"])
    assert.throws(() => packageName(platform, "0.26.1"));
  for (const version of ["1.2.3-beta.1", "01.2.3", "1.2", "../../x"])
    assert.throws(() => packageName("linux-x86_64", version));
});
test("package entrypoint accepts the platform from the environment", (t) => {
  const root = mkdtempSync(join(tmpdir(), "tunnelde-helper-package-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  writeFileSync(join(root, "Cargo.toml"), '[package]\nversion = "0.26.1"\n');
  const platform = "windows-x86_64";
  const directory = join(root, "target", targets[platform].triple, "release");
  mkdirSync(directory, { recursive: true });
  writeFileSync(join(directory, "tunnelde-agent.exe"), "fixture-binary");
  const result = spawnSync(
    process.execPath,
    [fileURLToPath(new URL("./package.mjs", import.meta.url))],
    {
      cwd: root,
      encoding: "utf8",
      env: { ...process.env, PLATFORM: platform },
      windowsHide: true,
    },
  );
  assert.equal(result.status, 0, result.stderr);
  const name = packageName(platform, "0.26.1");
  assert.equal(
    readFileSync(join(root, "artifacts", name), "utf8"),
    "fixture-binary",
  );
  assert.equal(
    readFileSync(join(root, "artifacts", name + ".sha256"), "utf8"),
    `${digest(Buffer.from("fixture-binary"))}  ${name}\n`,
  );
});
test("helper package and lock versions remain independent and consistent", () => {
  const version = versionOf(
    readFileSync(new URL("../Cargo.toml", import.meta.url), "utf8"),
  );
  const lock = readFileSync(new URL("../Cargo.lock", import.meta.url), "utf8");
  assert.equal(
    /\[\[package\]\]\r?\nname = "tunnelde-agent"\r?\nversion = "([^"]+)"/.exec(
      lock,
    )?.[1],
    version,
  );
  assert.equal(
    digest(Buffer.from("abc")),
    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
  );
});
