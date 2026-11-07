import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const targets = Object.freeze({
  "windows-x86_64": { triple: "x86_64-pc-windows-msvc", extension: ".exe" },
  "macos-x86_64": { triple: "x86_64-apple-darwin", extension: "" },
  "macos-aarch64": { triple: "aarch64-apple-darwin", extension: "" },
  "linux-x86_64": { triple: "x86_64-unknown-linux-gnu", extension: "" },
  "linux-aarch64": { triple: "aarch64-unknown-linux-gnu", extension: "" },
});
export function versionOf(manifest) {
  const version = /^version = "([^"]+)"$/m.exec(manifest)?.[1];
  assert.match(
    version ?? "",
    /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/,
    "Expected a stable helper version",
  );
  return version;
}
export function packageName(platform, version) {
  assert(Object.hasOwn(targets, platform), "Unknown helper platform");
  assert.equal(versionOf(`[package]\nversion = "${version}"`), version);
  return `tunnelde-agent-${version}-${platform}${targets[platform].extension}`;
}
export const digest = (bytes) =>
  createHash("sha256").update(bytes).digest("hex");
if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const platform = process.argv[2] || process.env.PLATFORM,
    version = versionOf(readFileSync("Cargo.toml", "utf8"));
  const name = packageName(platform, version),
    target = targets[platform];
  const source = join(
    "target",
    target.triple,
    "release",
    "tunnelde-agent" + target.extension,
  );
  mkdirSync("artifacts", { recursive: true });
  copyFileSync(source, join("artifacts", name));
  writeFileSync(
    join("artifacts", name + ".sha256"),
    `${digest(readFileSync(source))}  ${name}\n`,
  );
}
