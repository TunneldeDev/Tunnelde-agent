import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { targets, packageName, versionOf, digest } from "./package.mjs";
import { readFileSync } from "node:fs";
import { publish } from "./publish.mjs";

function fixture(t) {
  const directory = mkdtempSync(join(tmpdir(), "tunnelde-agent-publish-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const version = versionOf(
    readFileSync(new URL("../Cargo.toml", import.meta.url), "utf8"),
  );
  for (const platform of Object.keys(targets)) {
    const name = packageName(platform, version),
      data = Buffer.from(platform);
    writeFileSync(join(directory, name), data);
    writeFileSync(
      join(directory, name + ".sha256"),
      `${digest(data)}  ${name}\n`,
    );
  }
  const assets = [],
    mutations = [],
    repo = { owner: "test", repo: "agent" };
  const methods = {
    getReleaseByTag: async () => {
      throw { status: 404 };
    },
    createRelease: async (args) => {
      mutations.push(args);
      return { data: { id: 7, draft: true } };
    },
    listReleaseAssets: async () => ({ data: assets }),
    uploadReleaseAsset: async (args) => {
      mutations.push(args);
      assets.push({
        id: assets.length + 1,
        name: args.name,
        digest: "sha256:" + digest(args.data),
      });
    },
    getLatestRelease: async () => ({ data: { tag_name: "v99.0.0" } }),
    updateRelease: async (args) => {
      mutations.push(args);
      return { data: args };
    },
  };
  const github = {
    rest: { repos: methods },
    paginate: async (fn, args) => (await fn(args)).data,
  };
  return {
    directory,
    version,
    tag: `v${version}`,
    context: { repo },
    github,
    methods,
    mutations,
    assets,
  };
}
test("publisher verifies the full artifact set before publishing and retains newer Latest", async (t) => {
  const f = fixture(t);
  await publish(f);
  assert.equal(f.assets.length, 6);
  assert(f.assets.some((a) => a.name === "SHA256SUMS"));
  assert.equal(f.mutations.at(-1).draft, false);
  assert.equal(f.mutations.at(-1).make_latest, "false");
});
test("invalid checksums or unknown files fail before creating a release", async (t) => {
  for (const failure of ["checksum", "unknown"]) {
    const f = fixture(t);
    if (failure === "unknown")
      writeFileSync(join(f.directory, "unexpected"), "x");
    else
      writeFileSync(
        join(f.directory, packageName("linux-x86_64", f.version) + ".sha256"),
        "bad",
      );
    await assert.rejects(publish(f));
    assert.equal(f.mutations.length, 0);
  }
});
test("published releases and changed draft assets cannot be replaced", async (t) => {
  for (const draft of [false, true]) {
    const f = fixture(t);
    f.methods.getReleaseByTag = async () => ({ data: { id: 7, draft } });
    f.assets.push({
      id: 1,
      name: packageName("windows-x86_64", f.version),
      digest: "sha256:changed",
    });
    await assert.rejects(
      publish(f),
      draft ? /Draft asset differs/ : /immutable/,
    );
    assert.equal(f.mutations.length, 0);
  }
});
test("retry reuses identical draft assets and verifies hashes again before publishing", async (t) => {
  const f = fixture(t);
  f.methods.getReleaseByTag = async () => ({ data: { id: 7, draft: true } });
  const name = packageName("windows-x86_64", f.version);
  f.assets.push({
    id: 1,
    name,
    digest: "sha256:" + digest(Buffer.from("windows-x86_64")),
  });
  await publish(f);
  assert.equal(f.assets.length, 6);
  assert.equal(f.mutations.filter((m) => m.name === name).length, 0);
  assert.equal(f.mutations.at(-1).draft, false);
});
