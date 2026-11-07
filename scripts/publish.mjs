import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { targets, packageName, versionOf, digest } from "./package.mjs";

export async function publish({
  github,
  context,
  tag,
  directory = "artifacts",
}) {
  const version = versionOf(readFileSync("Cargo.toml", "utf8"));
  assert.equal(tag, `v${version}`);
  const names = Object.keys(targets)
    .map((platform) => packageName(platform, version))
    .sort();
  const expected = names.flatMap((name) => [name, name + ".sha256"]).sort();
  assert.deepEqual(
    readdirSync(directory).sort(),
    expected,
    "Missing or unexpected build artifacts",
  );
  const contents = names.map((name) => {
    const data = readFileSync(join(directory, name));
    assert(data.length > 0);
    const checksum = digest(data);
    assert.equal(
      readFileSync(join(directory, name + ".sha256"), "utf8"),
      `${checksum}  ${name}\n`,
    );
    return { name, data, checksum };
  });
  const sums = Buffer.from(
    contents.map((x) => `${x.checksum}  ${x.name}\n`).join(""),
  );
  contents.push({ name: "SHA256SUMS", data: sums, checksum: digest(sums) });
  let release;
  try {
    ({ data: release } = await github.rest.repos.getReleaseByTag({
      ...context.repo,
      tag,
    }));
  } catch (error) {
    if (error.status !== 404) throw error;
  }
  if (release) assert(release.draft, "Published helper releases are immutable");
  else
    ({ data: release } = await github.rest.repos.createRelease({
      ...context.repo,
      tag_name: tag,
      name: tag,
      draft: true,
      prerelease: false,
      generate_release_notes: true,
    }));
  const old = await github.paginate(github.rest.repos.listReleaseAssets, {
    ...context.repo,
    release_id: release.id,
    per_page: 100,
  });
  assert(
    old.every((asset) => contents.some((x) => x.name === asset.name)),
    "Unexpected draft asset",
  );
  // Validate every existing asset before uploading anything to a resumed draft.
  for (const existing of old) {
    const item = contents.find((item) => item.name === existing.name);
    assert.equal(
      existing.digest,
      "sha256:" + item.checksum,
      "Draft asset differs; refusing replacement",
    );
  }
  for (const item of contents) {
    const existing = old.find((asset) => asset.name === item.name);
    if (existing)
      assert.equal(
        existing.digest,
        "sha256:" + item.checksum,
        "Draft asset differs; refusing replacement",
      );
    else
      await github.rest.repos.uploadReleaseAsset({
        ...context.repo,
        release_id: release.id,
        name: item.name,
        data: item.data,
        headers: {
          "content-type": "application/octet-stream",
          "content-length": item.data.length,
        },
      });
  }
  const final = await github.paginate(github.rest.repos.listReleaseAssets, {
    ...context.repo,
    release_id: release.id,
    per_page: 100,
  });
  assert.equal(final.length, contents.length);
  for (const item of contents)
    assert.equal(
      final.find((a) => a.name === item.name)?.digest,
      "sha256:" + item.checksum,
    );
  const latest = await github.rest.repos
    .getLatestRelease(context.repo)
    .catch((error) => {
      if (error.status === 404) return null;
      throw error;
    });
  const compare = (a, b) => {
    const x = a.replace(/^v/, "").split(".").map(BigInt),
      y = b.replace(/^v/, "").split(".").map(BigInt);
    for (let i = 0; i < 3; i++) {
      if (x[i] !== y[i]) return x[i] > y[i] ? 1 : -1;
    }
    return 0;
  };
  await github.rest.repos.updateRelease({
    ...context.repo,
    release_id: release.id,
    draft: false,
    make_latest:
      !latest || compare(tag, latest.data.tag_name) > 0 ? "true" : "false",
  });
}
