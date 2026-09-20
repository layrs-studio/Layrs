import assert from "node:assert/strict";
import { test } from "node:test";
import { gzipSync } from "node:zlib";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readDistributionBytes, readDistribution, extractDistribution, sha256, validateUpgradePair } from "./native-upgrade-input.mjs";

function header(name, bytes, type = "0", link = "", declaredSize = bytes.length) {
  const h = Buffer.alloc(512); h.write(name); h.write("0000700\0", 100);
  h.write(declaredSize.toString(8).padStart(11, "0") + "\0", 124);
  h[156] = type.charCodeAt(0); h.write(link, 157); h.fill(32, 148, 156);
  h.write(h.reduce((sum, byte) => sum + byte, 0).toString(8).padStart(6, "0") + "\0 ", 148);
  return Buffer.concat([h, bytes, Buffer.alloc((512 - bytes.length % 512) % 512)]);
}
function archive(extra = [], mutate = (m) => m, text = "old") {
  const cli = Buffer.from(text), manifest = mutate({ schema: "layrs.native-distribution.v1", protocol: "V3", localStore: "native-format-2", target: "x86_64-unknown-linux-gnu", profile: "release", source: { treeSha256: sha256(cli) }, files: [{ path: "layrs", bytes: cli.length, sha256: sha256(cli) }] });
  return gzipSync(Buffer.concat([header("pkg/", Buffer.alloc(0), "5"), header("pkg/layrs", cli), header("pkg/manifest.json", Buffer.from(JSON.stringify(manifest))), ...extra, Buffer.alloc(1024)]));
}
const read = (bytes) => readDistributionBytes(bytes, sha256(bytes));
test("valid distinct archives are verified and extracted without execution", () => {
  const old = read(archive()), next = read(archive([], (m) => m, "new"));
  validateUpgradePair(old, next, "linux");
  const temp = mkdtempSync(join(tmpdir(), "layrs-upgrade-input-"));
  try {
    const destination = join(temp, "new"); extractDistribution(next, destination);
    assert.equal(readFileSync(join(destination, "layrs"), "utf8"), "new");
    assert.throws(() => extractDistribution(next, destination));
    const path = join(temp, "archive.tar.gz"); writeFileSync(path, archive());
    assert.equal(readDistribution(path, sha256(archive())).manifest.protocol, "V3");
  } finally { rmSync(temp, { recursive: true, force: true }); }
});
test("wrong digest, malformed gzip/header and oversized declared file fail", () => {
  const bytes = archive(); assert.throws(() => readDistributionBytes(bytes, "0".repeat(64)), /checksum/);
  assert.throws(() => readDistributionBytes(bytes, "xyz")); assert.throws(() => read(Buffer.from("not gzip")));
  assert.throws(() => read(archive([header("pkg/huge", Buffer.alloc(0), "0", "", 300 * 1024 * 1024)])), /size/);
  const broken = header("pkg/bad", Buffer.from("x")); broken[1] ^= 1;
  assert.throws(() => read(archive([broken])), /checksum/);
});
test("traversal, absolute paths, links, devices, PAX overrides and duplicates fail", () => {
  for (const [name, type, link] of [["../escape", "0", ""], ["/absolute", "0", ""], ["pkg/../escape", "0", ""], ["pkg/C:escape", "0", ""], ["pkg/CON", "0", ""], ["pkg/link", "2", "../../user"], ["pkg/link", "1", "pkg/layrs"], ["pkg/device", "3", ""], ["pkg/pax", "x", ""], ["pkg/LAYRS", "0", ""], ["other/file", "0", ""]]) {
    assert.throws(() => read(archive([header(name, Buffer.alloc(0), type, link)])), name);
  }
});
test("manifest hashes, inventory, format, platform and identical builds fail closed", () => {
  assert.throws(() => read(archive([], (m) => ({ ...m, localStore: "native-format-999" }))));
  assert.throws(() => read(archive([], (m) => { m.files[0].sha256 = "0".repeat(64); return m; })));
  assert.throws(() => read(archive([header("pkg/unlisted", Buffer.from("x"))])), /unlisted/);
  const old = read(archive()); assert.throws(() => validateUpgradePair(old, old, "linux"), /identical archives/);
  const next = read(archive([], (m) => m, "new")); assert.throws(() => validateUpgradePair(old, next, "win32"));
  assert.throws(() => validateUpgradePair(old, next, "darwin"));
});
