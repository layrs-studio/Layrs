const assert = require("node:assert/strict");
const fs = require("node:fs");
const Module = require("node:module");
const path = require("node:path");

const compiledRoot = path.resolve(__dirname, "../../../tmp/lenses-tests/lenses/src");
const compiledSdkEntry = path.resolve(__dirname, "../../../tmp/lenses-tests/lens-sdk/src/index.js");
const originalResolveFilename = Module._resolveFilename;
Module._resolveFilename = function resolveCompiledWorkspacePackage(request, parent, isMain, options) {
  if (request === "@layrs/lens-sdk") {
    return compiledSdkEntry;
  }
  return originalResolveFilename.call(this, request, parent, isMain, options);
};
const {
  BUILTIN_LENS_IMPLEMENTATION_DIGESTS,
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
Module._resolveFilename = originalResolveFilename;
const {
  builtinImplementationDigests,
  implementationDigest,
  readImplementationEntries
} = require("../scripts/implementation-digests.cjs");

const goldenPath = path.resolve(__dirname, "builtin-manifests.golden.json");
const negativeVectorsPath = path.resolve(__dirname, "../../lens-sdk/test/contract-negative.vectors.json");
const golden = JSON.parse(fs.readFileSync(goldenPath, "utf8"));
const negativeVectors = JSON.parse(fs.readFileSync(negativeVectorsPath, "utf8"));

assert.deepEqual(JSON.parse(JSON.stringify(builtinManifests)), golden);
const implementationEntries = readImplementationEntries();
const computedDigests = builtinImplementationDigests(implementationEntries);
assert.deepEqual(BUILTIN_LENS_IMPLEMENTATION_DIGESTS, computedDigests);
for (const manifest of builtinManifests) {
  assert.equal(manifest.contractVersion, "layrs.lens.v1");
  assert.equal(manifest.version, "1.0.0");
  assert.equal(manifest.digest, computedDigests[manifest.id]);
  assert.match(manifest.digest, /^sha256:[a-f0-9]{64}$/);
  assert.notEqual(manifest.digest, `sha256:${"0".repeat(64)}`);
  assert.deepEqual(manifest.viewer.reconcileStatuses, []);
}
const changedImplementation = implementationEntries.map((entry, index) =>
  index === 0 ? { ...entry, bytes: Buffer.concat([entry.bytes, Buffer.from("\n// changed")]) } : entry
);
for (const manifest of builtinManifests) {
  assert.notEqual(
    implementationDigest(manifest.id, changedImplementation),
    manifest.digest,
    `${manifest.id} identity must change when shipped implementation bytes change`
  );
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

for (const vector of negativeVectors.reconcileSides) {
  const invalidSide = runtimeSide(vector.value);
  assert.equal(
    prepareRawReconcile({ ...rawConflictInput, ours: invalidSide }).status,
    "unsupported",
    `Raw: ${vector.name}`
  );
  assert.equal(
    prepareTextReconcile({
      ...reconcileInput("base", "existing", "incoming", "text/plain"),
      ours: invalidSide
    }).status,
    "unsupported",
    `Text: ${vector.name}`
  );
}

for (const [name, prepare, mediaType] of [
  ["Raw", prepareRawReconcile, "application/octet-stream"],
  ["Text", prepareTextReconcile, "text/plain"]
]) {
  const emptyPresent = byteSide(new Uint8Array());
  const emptyResult = prepare(tombstoneInput(mediaType, emptyPresent, emptyPresent, emptyPresent));
  assert.equal(emptyResult.status, "auto_resolved", `${name}: empty present file is valid`);
  assert.equal(emptyResult.resolved.exists, true, `${name}: empty file remains present`);
  assert.equal(emptyResult.resolved.bytes.length, 0);

  const absent = absentSide();
  const deletedResult = prepare(tombstoneInput(mediaType, textSide("base"), absent, absent));
  assert.equal(deletedResult.status, "auto_resolved", `${name}: matching tombstones resolve`);
  assert.equal(deletedResult.resolved.exists, false, `${name}: tombstone remains absent`);

  const createdEmpty = prepare(tombstoneInput(mediaType, absent, emptyPresent, absent));
  assert.equal(createdEmpty.status, "auto_resolved", `${name}: empty creation resolves`);
  assert.equal(createdEmpty.resolved.exists, true, `${name}: empty creation is not a tombstone`);
}

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

function absentSide() {
  return { exists: false, bytes: new Uint8Array(), size: 0 };
}

function runtimeSide(value) {
  return {
    ...value,
    ...(Object.hasOwn(value, "bytes") ? { bytes: Uint8Array.from(value.bytes) } : {})
  };
}

function tombstoneInput(mediaType, base, ours, theirs) {
  return {
    path: "docs/tombstone.txt",
    mediaType,
    base,
    ours,
    theirs,
    oursLabel: "existing",
    theirsLabel: "incoming"
  };
}

function decode(bytes) {
  return new TextDecoder().decode(bytes);
}
