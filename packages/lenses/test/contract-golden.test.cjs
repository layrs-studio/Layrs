const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");

const compiledRoot = path.resolve(__dirname, "../../../tmp/lenses-tests/lenses/src");
const {
  builtinManifestDigestPayload,
  codeManifest,
  imageManifest,
  rawManifest,
  textManifest
} = require(path.join(compiledRoot, "manifests.js"));
const builtinManifests = [codeManifest, imageManifest, textManifest, rawManifest];
const { prepareRawReconcile, resolveRawConflict } = require(path.join(compiledRoot, "raw/reconcile.js"));
const {
  declaredResolutionMethods,
  surfaceResolutionMethods
} = require(path.join(compiledRoot, "shared/resolutionMethods.js"));
const { prepareTextReconcile, resolveTextBlockChoice } = require(
  path.join(compiledRoot, "text/reconcile.js")
);

const goldenPath = path.resolve(__dirname, "builtin-manifests.golden.json");
const golden = JSON.parse(fs.readFileSync(goldenPath, "utf8"));

assert.deepEqual(JSON.parse(JSON.stringify(builtinManifests)), golden);
for (const manifest of builtinManifests) {
  const digest = `sha256:${crypto.createHash("sha256").update(builtinManifestDigestPayload(manifest)).digest("hex")}`;
  assert.equal(manifest.contractVersion, "layrs.lens.v1");
  assert.equal(manifest.version, "1.0.0");
  assert.equal(manifest.digest, digest);
  assert.match(manifest.digest, /^sha256:[a-f0-9]{64}$/);
  assert.deepEqual(manifest.viewer.reconcileStatuses, []);
}

assert.deepEqual(declaredResolutionMethods(textManifest, "block"), ["existing", "incoming", "both", "manual"]);
assert.deepEqual(declaredResolutionMethods(textManifest, "file"), []);
assert.deepEqual(declaredResolutionMethods(rawManifest, "file"), ["existing", "incoming"]);
assert.deepEqual(declaredResolutionMethods(rawManifest, "block"), []);
assert.equal(codeManifest.analyzer.capabilities.includes("reconcile"), false);
assert.equal(imageManifest.analyzer.capabilities.includes("reconcile"), false);
assert.deepEqual(codeManifest.resolutionMethods, []);
assert.deepEqual(imageManifest.resolutionMethods, []);

const textAuto = prepareTextReconcile(
  reconcileInput("a\nb\nc\nd\n", "a\nexisting-b\nc\nd\n", "a\nb\nincoming-c\nd\n", "text/plain")
);
assert.equal(textAuto.status, "auto_resolved");
assert.equal(decode(textAuto.resolved.bytes), "a\nexisting-b\nincoming-c\nd\n");

const textConflict = prepareTextReconcile(
  reconcileInput("a\nx\nb\n", "a\nexisting-x\nb\n", "a\nincoming-x\nb\n", "text/plain")
);
assert.equal(textConflict.status, "conflicted");
assert.equal(textConflict.blocks.length, 1);
assert.deepEqual(textConflict.blocks[0].resolutionMethods, [
  { method: "existing", scope: "block" },
  { method: "incoming", scope: "block" },
  { method: "both", scope: "block" },
  { method: "manual", scope: "block" }
]);
assert.match(decode(textConflict.conflict.bytes), /<<<<<<< target:existing/);

const invalidText = prepareTextReconcile({
  ...reconcileInput("base\n", "existing\n", "incoming\n", "text/plain"),
  theirs: byteSide(new Uint8Array([0xff]))
});
assert.equal(invalidText.status, "unsupported");
assert.throws(() => resolveTextBlockChoice("existing", "incoming", "base"), /not supported/);
assert.throws(() => resolveTextBlockChoice("existing", "incoming", "manual"), /replacement text/);

const rawAuto = prepareRawReconcile(reconcileInput("base", "base", "incoming", "application/octet-stream"));
assert.equal(rawAuto.status, "auto_resolved");
assert.equal(decode(rawAuto.resolved.bytes), "incoming");

const rawConflictInput = reconcileInput("base", "existing", "incoming", "application/octet-stream");
const rawConflict = prepareRawReconcile(rawConflictInput);
assert.equal(rawConflict.status, "conflicted");
assert.deepEqual(rawConflict.resolutionMethods, [
  { method: "existing", scope: "file" },
  { method: "incoming", scope: "file" }
]);
assert.equal(decode(resolveRawConflict(rawConflictInput, "incoming").bytes), "incoming");
assert.throws(() => resolveRawConflict(rawConflictInput, "both"), /not supported/);

const malformedRaw = prepareRawReconcile({
  ...rawConflictInput,
  ours: { exists: true, size: 8 }
});
assert.equal(malformedRaw.status, "unsupported");

assert.deepEqual(
  surfaceResolutionMethods(textManifest, "block", { supportedMethods: ["incoming", "base"] }),
  ["incoming"]
);
assert.throws(
  () => surfaceResolutionMethods(textManifest, "block", {
    resolutionMethods: [{ method: "existing", scope: "file" }]
  }),
  /does not declare/
);

console.log("built-in Lens contract tests: ok");

function reconcileInput(base, existing, incoming, mediaType) {
  return {
    path: "docs/example.txt",
    mediaType,
    base: textSide(base),
    ours: textSide(existing),
    theirs: textSide(incoming),
    oursLabel: "existing",
    theirsLabel: "incoming"
  };
}

function textSide(value) {
  return byteSide(new TextEncoder().encode(value));
}

function byteSide(bytes) {
  return { exists: true, bytes, size: bytes.length };
}

function decode(bytes) {
  return new TextDecoder().decode(bytes);
}
