const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const compiledRoot = path.resolve(__dirname, "../../../tmp/lens-sdk-tests");
const { contractFixture } = require(path.join(compiledRoot, "test/contract.fixture.js"));
const { createLensManifest, lensPinFromManifest, supportsLensCapability } = require(
  path.join(compiledRoot, "src/manifest.js")
);
const { toCanonicalLensReconcileResult, toLensReconcileResponse } = require(
  path.join(compiledRoot, "src/reconcile.js")
);
const { createLensRegistry } = require(path.join(compiledRoot, "src/runtime.js"));
const {
  assertSchemaSubsetSupported,
  assertValidDefinition,
  validationErrorsForDefinition
} = require("./schema-subset-validator.cjs");

const fixturePath = path.resolve(__dirname, "../../../tests/fixtures/lenses/v1/contract.golden.json");
const schemaPath = path.resolve(__dirname, "../../../schemas/lenses/v1/lens-contract.schema.json");
const expected = JSON.parse(fs.readFileSync(fixturePath, "utf8"));
const schema = JSON.parse(fs.readFileSync(schemaPath, "utf8"));

assert.deepEqual(JSON.parse(JSON.stringify(contractFixture)), expected);
assert.equal(schema.$id, "https://schemas.layrs.dev/lenses/v1/lens-contract.schema.json");
for (const definition of [
  "lensManifest",
  "previewWindow",
  "diffWindow",
  "reconcileRequest",
  "reconcileResponse"
]) {
  assert.ok(schema.$defs[definition], `missing schema definition ${definition}`);
}
assertSchemaSubsetSupported(schema);
for (const [definition, value] of [
  ["lensManifest", expected.manifest],
  ["previewWindow", expected.previewWindow],
  ["diffWindow", expected.diffWindow],
  ["reconcileRequest", expected.reconcileInput],
  ["reconcileResponse", expected.reconcileResults.autoResolved],
  ["reconcileResponse", expected.reconcileResults.conflicted],
  ["reconcileResponse", expected.reconcileResults.unsupported]
]) {
  assertValidDefinition(schema, definition, value);
}

const invalidAutoResponse = clone(expected.reconcileResults.autoResolved);
invalidAutoResponse.result.resolutionMethods = [{ method: "manual", scope: "file" }];
assert.notEqual(validationErrorsForDefinition(schema, "reconcileResponse", invalidAutoResponse).length, 0);
const invalidConflictResponse = clone(expected.reconcileResults.conflicted);
invalidConflictResponse.result.segments[1].text = "not allowed";
assert.notEqual(
  validationErrorsForDefinition(schema, "reconcileResponse", invalidConflictResponse).length,
  0
);
const invalidUnsupportedResponse = clone(expected.reconcileResults.unsupported);
invalidUnsupportedResponse.result.resolutionMethods = [{ method: "manual", scope: "file" }];
assert.notEqual(
  validationErrorsForDefinition(schema, "reconcileResponse", invalidUnsupportedResponse).length,
  0
);

const manifest = createLensManifest({
  id: "layrs.test.declared",
  name: "Declared",
  component: "DeclaredViewer",
  previewKinds: ["text"],
  diffKinds: ["textLines"],
  supportedMediaTypes: ["text/x-declared"],
  fileExtensions: ["declared"],
  capabilities: ["view", "preview"]
});
assert.equal(manifest.contractVersion, "layrs.lens.v1");
assert.deepEqual(manifest.viewer.reconcileStatuses, []);
assert.deepEqual(manifest.resolutionMethods, []);
assert.equal(supportsLensCapability(manifest, "preview"), true);
assert.equal(supportsLensCapability(manifest, "reconcile"), false);
assert.equal(lensPinFromManifest(manifest), undefined);

assert.throws(
  () => createLensManifest({ ...manifestInput("layrs.test.bundle"), bundle: expected.manifest.bundle }),
  /requires an explicit digest/
);
assert.throws(
  () => createLensManifest({ ...manifestInput("layrs.test.digest"), digest: "sha256:abc" }),
  /64 lowercase hexadecimal/
);
assert.throws(
  () => lensPinFromManifest({ ...manifest, digest: "present-but-invalid" }),
  /64 lowercase hexadecimal/
);

const declaredLens = {
  manifest,
  pathRegex: [/\.forced$/],
  viewer: {}
};
const textLens = {
  manifest: createLensManifest({
    ...manifestInput("layrs.text"),
    supportedMediaTypes: ["text/plain"],
    fileExtensions: ["txt"]
  }),
  viewer: {}
};
const rawLens = {
  manifest: createLensManifest({
    ...manifestInput("layrs.raw"),
    supportedMediaTypes: ["application/octet-stream"],
    fileExtensions: []
  }),
  viewer: {}
};
const registry = createLensRegistry([declaredLens, textLens, rawLens]);

assert.equal(registry.resolve({ mediaType: "text/x-declared; charset=utf-8" }).lens, declaredLens);
assert.equal(registry.resolve({ path: "file.declared" }).lens, declaredLens);
assert.equal(registry.resolve({ lensId: "layrs.test.declared" }).lens, declaredLens);
assert.equal(registry.resolve({ path: "file.forced" }), undefined);
assert.equal(registry.resolve({ bytes: new TextEncoder().encode("text") }), undefined);
assert.equal(registry.resolve({ path: "unknown.bin" }), undefined);

const canonicalConflict = toCanonicalLensReconcileResult({
  status: "conflicted",
  summary: "legacy conflict",
  conflict: { exists: true, bytes: new Uint8Array([1, 2, 3]) },
  resolutionMethods: [{ method: "existing", scope: "file" }],
  fileMethods: ["ours"],
  blocks: [
    {
      blockId: "b1",
      base: "base",
      existing: "ours",
      ours: "ours",
      incoming: "theirs",
      theirs: "theirs",
      resolutionMethods: [{ method: "manual", scope: "block" }],
      supportedResolutions: ["manual"]
    }
  ],
  segments: [{ kind: "block", blockId: "b1" }],
  fields: {}
});
assert.deepEqual(canonicalConflict.resolutionMethods, [{ method: "existing", scope: "file" }]);
assert.deepEqual(canonicalConflict.blocks[0].resolutionMethods, [{ method: "manual", scope: "block" }]);
assert.deepEqual(canonicalConflict.conflict.bytes, [1, 2, 3]);
assert.throws(
  () =>
    toCanonicalLensReconcileResult({
      status: "auto_resolved",
      summary: "missing content",
      blocks: [],
      segments: [],
      fields: {}
    }),
  /requires resolved content/
);
assert.throws(
  () =>
    toCanonicalLensReconcileResult(
      reconcileResult({
        status: "auto_resolved",
        resolved: { exists: true, bytes: new Uint8Array([1]) },
        conflict: { exists: true, bytes: new Uint8Array([2]) }
      })
    ),
  /cannot carry conflict content/
);
assert.throws(
  () =>
    toCanonicalLensReconcileResult(
      reconcileResult({
        status: "conflicted",
        conflict: { exists: true, bytes: new Uint8Array() },
        fileMethods: ["banana"]
      })
    ),
  /unsupported legacy resolution method banana/
);
assert.throws(
  () =>
    toCanonicalLensReconcileResult(
      reconcileResult({
        status: "conflicted",
        conflict: { exists: true, bytes: new Uint8Array() },
        resolutionMethods: [{ method: "existing", scope: "file" }],
        fileMethods: ["incoming"]
      })
    ),
  /contradictory canonical and legacy resolution methods/
);
assert.throws(
  () =>
    toCanonicalLensReconcileResult(
      reconcileResult({
        status: "conflicted",
        conflict: { exists: true, bytes: new Uint8Array() },
        blocks: [conflictBlock({ existing: "canonical", ours: "legacy" })],
        segments: [{ kind: "block", blockId: "b1" }]
      })
    ),
  /contradictory existing\/ours content/
);
assert.throws(
  () =>
    toCanonicalLensReconcileResult(
      reconcileResult({
        status: "conflicted",
        conflict: { exists: true, bytes: new Uint8Array() },
        blocks: [conflictBlock({ incoming: "canonical", theirs: "legacy" })],
        segments: [{ kind: "block", blockId: "b1" }]
      })
    ),
  /contradictory incoming\/theirs content/
);
assert.throws(
  () =>
    toCanonicalLensReconcileResult(
      reconcileResult({
        status: "unsupported",
        fileMethods: ["manual"]
      })
    ),
  /cannot carry resolution methods/
);
assert.throws(
  () =>
    toLensReconcileResponse(
      { lensId: "layrs.test", version: "1.0.0", digest: "sha256:invalid" },
      reconcileResult()
    ),
  /Lens pin digest/
);

console.log("lens-sdk contract tests: ok");

function manifestInput(id) {
  return {
    id,
    name: id,
    component: "TestViewer",
    previewKinds: ["text"],
    diffKinds: ["textLines"],
    supportedMediaTypes: [],
    fileExtensions: [],
    capabilities: ["view"]
  };
}

function reconcileResult(overrides = {}) {
  return {
    status: "unsupported",
    summary: "test result",
    blocks: [],
    segments: [],
    fields: {},
    ...overrides
  };
}

function conflictBlock(overrides = {}) {
  return {
    blockId: "b1",
    base: "base",
    existing: "existing",
    incoming: "incoming",
    resolutionMethods: [{ method: "manual", scope: "block" }],
    ...overrides
  };
}

function clone(value) {
  return JSON.parse(JSON.stringify(value));
}
