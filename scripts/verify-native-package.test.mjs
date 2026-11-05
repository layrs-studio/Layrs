import test from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { sourceTreeDigest } from "./package-source.mjs";

const verifier = fileURLToPath(new URL("./verify-native-package.mjs", import.meta.url));
test("package verification rejects corruption, unexpected entries and traversal", () => {
  const root = mkdtempSync(join(tmpdir(), "layrs-package-check-"));
  try {
    const bytes = Buffer.from("a binary candidate\0\xff");
    const file = { path: "layrs", bytes: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") };
    const manifest = { schema: "layrs.native-distribution.v1", target: "fixture", profile: "dev", signature: "unsigned", files: [file] };
    const save = () => writeFileSync(join(root, "manifest.json"), JSON.stringify(manifest));
    const verify = () => spawnSync(process.execPath, [verifier, root], { encoding: "utf8" });
    save(); writeFileSync(join(root, "layrs"), bytes);
    assert.equal(verify().status, 0);
    writeFileSync(join(root, "layrs"), Buffer.alloc(bytes.length));
    assert.match(verify().stderr, /Checksum mismatch/);
    writeFileSync(join(root, "layrs"), bytes);
    writeFileSync(join(root, "injected"), "unexpected");
    assert.match(verify().stderr, /Unexpected package entry/);
    rmSync(join(root, "injected"));
    for (const path of ["../outside", "..\\outside", "C:outside", "..", "manifest.json"]) {
      file.path = path; save();
      assert.match(verify().stderr, /Unsafe or duplicate/);
    }
    file.path = "layrs"; manifest.files.push({ ...file }); save();
    assert.match(verify().stderr, /Unsafe or duplicate/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test("source provenance changes with source bytes, not generated output", () => {
  const root = mkdtempSync(join(tmpdir(), "layrs-source-check-"));
  try {
    writeFileSync(join(root, "source.rs"), "first");
    const before = sourceTreeDigest(root, ["source.rs"]);
    writeFileSync(join(root, "unrelated.log"), "build diagnostics");
    assert.equal(sourceTreeDigest(root, ["source.rs"]), before);
    writeFileSync(join(root, "source.rs"), "other");
    assert.notEqual(sourceTreeDigest(root, ["source.rs"]), before);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
