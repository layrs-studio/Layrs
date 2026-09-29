// Narrow reader for our flat, regular-file tar.gz distributions. No external
// extractor, executable package script, links, PAX overrides or special files.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { lstatSync, readFileSync, mkdirSync, writeFileSync, chmodSync } from "node:fs";
import { join } from "node:path";
import { gunzipSync } from "node:zlib";

export const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const MAX_ARCHIVE = 256 * 1024 * 1024, MAX_EXPANDED = 512 * 1024 * 1024;
const safeName = (name) => typeof name === "string" && /^[A-Za-z0-9_.-]{1,100}$/.test(name)
  && name !== "." && name !== ".." && !name.endsWith(".")
  && !/^(con|prn|aux|nul|com[0-9]|lpt[0-9])(?:\.|$)/i.test(name);
function number(field) {
  const value = field.toString("ascii").replace(/\0/g, "").trim();
  assert.match(value, /^[0-7]{1,12}$/, "unsupported tar number");
  return parseInt(value, 8);
}
function string(field) {
  const nul = field.indexOf(0);
  const tail = nul < 0 ? Buffer.alloc(0) : field.subarray(nul);
  assert.ok(tail.every((b) => b === 0), "tar text contains trailing data");
  return field.subarray(0, nul < 0 ? field.length : nul).toString("utf8");
}
export function readDistributionBytes(archive, expectedSha256) {
  assert.match(expectedSha256, /^[a-f0-9]{64}$/, "explicit lowercase archive SHA256 required");
  assert.ok(archive.length > 0 && archive.length <= MAX_ARCHIVE, "archive size limit");
  assert.equal(sha256(archive), expectedSha256, "archive checksum mismatch");
  const tar = gunzipSync(archive, { maxOutputLength: MAX_EXPANDED });
  assert.equal(tar.length % 512, 0, "truncated tar");
  const files = new Map(); let root; let offset = 0; let entries = 0; let ended = false;
  for (; offset + 512 <= tar.length;) {
    const header = tar.subarray(offset, offset + 512); offset += 512;
    if (header.every((b) => b === 0)) {
      assert.ok(tar.length - offset >= 512 && tar.subarray(offset).every((b) => b === 0), "invalid tar end");
      ended = true; break;
    }
    assert.ok(++entries <= 128, "tar entry limit");
    const checksum = header.reduce((sum, b, i) => sum + (i >= 148 && i < 156 ? 32 : b), 0);
    assert.equal(number(header.subarray(148, 156)), checksum, "tar header checksum mismatch");
    assert.equal(string(header.subarray(345, 500)), "", "tar prefixes unsupported");
    assert.equal(string(header.subarray(157, 257)), "", "tar links unsupported");
    const name = string(header.subarray(0, 100));
    const type = header[156];
    const size = number(header.subarray(124, 136));
    assert.ok(size <= MAX_ARCHIVE && offset + Math.ceil(size / 512) * 512 <= tar.length, "tar size limit or truncation");
    const parts = name.split("/");
    assert.ok(parts.length === 2 && safeName(parts[0]), "flat distribution root required");
    if (root === undefined) root = parts[0];
    assert.equal(parts[0], root, "multiple archive roots");
    if (type === 53) {
      assert.equal(parts[1], ""); assert.equal(size, 0); assert.equal(entries, 1, "root directory must appear once first");
    } else {
      assert.ok(type === 48 || type === 0, "only regular tar files accepted");
      assert.ok(safeName(parts[1]) && !files.has(parts[1].toLowerCase()), "unsafe or duplicate artifact");
      files.set(parts[1].toLowerCase(), { name: parts[1], bytes: tar.subarray(offset, offset + size) });
    }
    offset += Math.ceil(size / 512) * 512;
  }
  assert.ok(ended && files.has("manifest.json"), "missing tar terminator or manifest");
  const manifestBytes = files.get("manifest.json").bytes;
  assert.ok(manifestBytes.length <= 64 * 1024, "manifest size limit");
  const manifest = JSON.parse(manifestBytes.toString("utf8"));
  assert.equal(manifest.schema, "layrs.native-distribution.v1");
  assert.equal(manifest.protocol, "V3"); assert.equal(manifest.localStore, "native-format-2");
  assert.match(manifest.source?.treeSha256 ?? "", /^[a-f0-9]{64}$/, "source fingerprint required");
  assert.ok(Array.isArray(manifest.files) && manifest.files.length > 0 && manifest.files.length < 128);
  const declared = new Set(["manifest.json"]);
  for (const entry of manifest.files) {
    assert.ok(safeName(entry.path) && !declared.has(entry.path.toLowerCase()), "invalid manifest path");
    declared.add(entry.path.toLowerCase());
    const file = files.get(entry.path.toLowerCase());
    assert.ok(file && file.name === entry.path, "missing artifact or case mismatch");
    assert.equal(entry.bytes, file.bytes.length); assert.equal(entry.sha256, sha256(file.bytes), "artifact checksum mismatch");
  }
  assert.equal(files.size, declared.size, "unlisted archive file");
  return { root, files, manifest, archiveSha256: expectedSha256, manifestSha256: sha256(manifestBytes) };
}
export function readDistribution(path, digest) {
  const stat = lstatSync(path);
  assert.ok(stat.isFile() && !stat.isSymbolicLink() && stat.size <= MAX_ARCHIVE, "archive must be a bounded regular file");
  return readDistributionBytes(readFileSync(path), digest);
}
export function validateUpgradePair(oldPackage, newPackage, platform = process.platform) {
  assert.notEqual(oldPackage.archiveSha256, newPackage.archiveSha256, "identical archives do not qualify an upgrade");
  assert.notEqual(oldPackage.manifest.source?.treeSha256, newPackage.manifest.source?.treeSha256, "different source snapshots required");
  const target = platform === "win32" ? "x86_64-pc-windows-msvc" : platform === "linux" ? "x86_64-unknown-linux-gnu" : undefined;
  assert.ok(target, "only Windows/Linux x64 supported");
  assert.equal(oldPackage.manifest.target, target); assert.equal(newPackage.manifest.target, target);
  for (const pkg of [oldPackage, newPackage]) {
    assert.equal(pkg.manifest.profile, "release");
    assert.ok(pkg.files.has(platform === "win32" ? "layrs.exe" : "layrs"), "missing CLI");
  }
  const name = platform === "win32" ? "layrs.exe" : "layrs";
  assert.notEqual(sha256(oldPackage.files.get(name).bytes), sha256(newPackage.files.get(name).bytes), "identical CLI binaries do not qualify an upgrade");
}
export function extractDistribution(pkg, destination) {
  mkdirSync(destination); // Must be new; never overwrite an installation.
  for (const { name, bytes } of pkg.files.values()) {
    const path = join(destination, name);
    writeFileSync(path, bytes, { flag: "wx", mode: 0o700 });
    chmodSync(path, 0o700);
  }
}
