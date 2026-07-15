const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const compiledRoot = path.resolve(__dirname, "../../../tmp/lens-sdk-tests");
const { contractFixture } = require(path.join(compiledRoot, "test/contract.fixture.js"));
const { createLensManifest, lensPinFromManifest, supportsLensCapability } = require(
  path.join(compiledRoot, "src/manifest.js")
);
const { toCanonicalLensReconcileResult } = require(path.join(compiledRoot, "src/reconcile.js"));
const { createLensRegistry } = require(path.join(compiledRoot, "src/runtime.js"));

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
  fileMethods: ["existing"],
  blocks: [
    {
      blockId: "b1",
      base: "base",
      ours: "ours",
      theirs: "theirs",
      supportedResolutions: ["manual"]
    }
  ],
  segments: [],
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
