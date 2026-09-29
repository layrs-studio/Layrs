const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");

const compiledRoot = path.resolve(__dirname, "../../../tmp/lens-sdk-tests");
const { contractFixture } = require(path.join(compiledRoot, "test/contract.fixture.js"));
const {
  createLensManifest,
  lensPinFromManifest,
  supportsLensCapability,
  LoadedLensReconcileImplementation,
  VerifiedLensReconcileExecution
} = require(
  path.join(compiledRoot, "src/manifest.js")
);
const {
  assertLensReconcileSideContract,
  toCanonicalLensReconcileResult,
  toLensReconcileResponse
} = require(
  path.join(compiledRoot, "src/reconcile.js")
);
const { createLensRegistry } = require(path.join(compiledRoot, "src/runtime.js"));
const { parseLensRegistryResponse } = require(path.join(compiledRoot, "src/registry.js"));
const { LensSelectionError, selectLensManifest } = require(
  path.join(compiledRoot, "src/selection.js")
);
const { assertLensManifest } = require(path.join(compiledRoot, "src/manifestValidation.js"));
const { assertLensPin } = require(path.join(compiledRoot, "src/validation.js"));
const { formatJson } = require("../scripts/format-contract-schema.cjs");
const {
  assertSchemaSubsetSupported,
  assertValidDefinition,
  validationErrorsForDefinition
} = require("./schema-subset-validator.cjs");

const fixturePath = path.resolve(__dirname, "../../../tests/fixtures/lenses/v1/contract.golden.json");
const schemaPath = path.resolve(__dirname, "../../../schemas/lenses/v1/lens-contract.schema.json");
const negativeVectorsPath = path.resolve(__dirname, "contract-negative.vectors.json");
const selectionVectorsPath = path.resolve(__dirname, "selection.vectors.json");
const bundleVectorsPath = path.resolve(__dirname, "bundle-validation.vectors.json");
const expected = JSON.parse(fs.readFileSync(fixturePath, "utf8"));
const schemaText = fs.readFileSync(schemaPath, "utf8");
const schema = JSON.parse(schemaText);
const negativeVectors = JSON.parse(fs.readFileSync(negativeVectorsPath, "utf8"));
const selectionVectors = JSON.parse(fs.readFileSync(selectionVectorsPath, "utf8"));
const bundleVectors = JSON.parse(fs.readFileSync(bundleVectorsPath, "utf8"));

assert.deepEqual(JSON.parse(JSON.stringify(contractFixture)), expected);
assert.equal(schemaText, formatJson(schema), "Lens schema uses the reproducible formatter");
assert.ok(schemaText.split(/\r?\n/).length - 1 < 900, "Lens schema stays below 900 lines");
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

const emptyPresentRequest = clone(expected.reconcileInput);
emptyPresentRequest.base = { exists: true, bytes: [], size: 0 };
assertValidDefinition(schema, "reconcileRequest", emptyPresentRequest);

const missingPresentBytesRequest = clone(expected.reconcileInput);
delete missingPresentBytesRequest.base.bytes;
assertSchemaRejects("reconcileRequest", missingPresentBytesRequest, "present side without bytes");
for (const [label, side] of [
  ["absent side with bytes", { exists: false, bytes: [1], size: 1 }],
  ["absent side with size", { exists: false, bytes: [], size: 1 }],
  [
    "absent side with hash",
    {
      exists: false,
      bytes: [],
      contentHash: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      size: 0
    }
  ]
]) {
  const invalid = clone(expected.reconcileInput);
  invalid.base = side;
  assertSchemaRejects("reconcileRequest", invalid, label);
}
const missingPresentResultBytes = clone(expected.reconcileResults.autoResolved);
delete missingPresentResultBytes.result.resolved.bytes;
assertSchemaRejects("reconcileResponse", missingPresentResultBytes, "present result without bytes");
const absentResultWithBytes = clone(expected.reconcileResults.autoResolved);
absentResultWithBytes.result.resolved = { exists: false, bytes: [1] };
assertSchemaRejects("reconcileResponse", absentResultWithBytes, "absent result with bytes");

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
  /requires an explicit identity digest/
);

for (const vector of bundleVectors.paths) {
  const candidate = clone(expected.manifest);
  candidate.bundle.entrypoints.reconcile.path = vector.value;
  assert.equal(validationPasses(() => assertLensManifest(candidate)), vector.valid, vector.name);
  assert.equal(
    validationErrorsForDefinition(schema, "lensManifest", candidate).length === 0,
    vector.valid,
    `${vector.name}: schema`
  );
  assert.equal(
    validationPasses(() => parseLensRegistryResponse({ items: [candidate], errors: [] })),
    vector.valid,
    `${vector.name}: canonical Studio registry parser`
  );
}
for (const vector of bundleVectors.quotas) {
  const candidate = clone(expected.manifest);
  candidate.bundle.entrypoints.reconcile.resources.maxMemoryBytes = vector.value;
  assert.equal(validationPasses(() => assertLensManifest(candidate)), vector.valid, vector.name);
  assert.equal(
    validationErrorsForDefinition(schema, "lensManifest", candidate).length === 0,
    vector.valid,
    `${vector.name}: schema`
  );
  assert.equal(
    validationPasses(() => parseLensRegistryResponse({ items: [candidate] })),
    vector.valid,
    `${vector.name}: canonical Studio registry parser`
  );
}
assert.throws(
  () =>
    createLensManifest({
      ...manifestInput("layrs.test.empty-bundle"),
      digest: expected.manifest.digest,
      bundle: { ...expected.manifest.bundle, entrypoints: {} }
    }),
  /at least one entrypoint/
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

for (const vector of negativeVectors.reconcileSides) {
  assert.throws(
    () => assertLensReconcileSideContract(vector.value),
    undefined,
    vector.name
  );
}
assert.doesNotThrow(() => assertLensReconcileSideContract({ exists: true, bytes: [], size: 0 }));
for (const vector of negativeVectors.reconcileContents) {
  const content = {
    ...vector.value,
    ...(Object.hasOwn(vector.value, "bytes")
      ? { bytes: Uint8Array.from(vector.value.bytes) }
      : {})
  };
  assert.throws(
    () =>
      toCanonicalLensReconcileResult(
        reconcileResult({ status: "auto_resolved", resolved: content })
      ),
    undefined,
    vector.name
  );
}
for (const vector of negativeVectors.pins) {
  assert.throws(() => assertLensPin(vector.value), undefined, vector.name);
}

for (const vector of selectionVectors) {
  const manifests = selectionManifests();
  if (vector.duplicateFromLensId) {
    const source = manifests.find((candidate) => candidate.id === vector.duplicateFromLensId);
    assert.ok(source, `${vector.name}: duplicate source exists`);
    const duplicate = clone(source);
    duplicate.id = vector.duplicateLensId;
    duplicate.viewer.viewerId = `layrs.viewer.${vector.duplicateLensId}`;
    manifests.push(duplicate);
  }

  if (vector.expectedError) {
    assert.throws(
      () => selectLensManifest(manifests, vector),
      (error) => error instanceof LensSelectionError && error.code === vector.expectedError,
      vector.name
    );
    continue;
  }
  const selected = selectLensManifest(manifests, vector);
  assert.equal(selected?.manifest.id, vector.expectedLensId, vector.name);
  assert.equal(selected?.reason, vector.expectedReason, vector.name);
}

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

function selectionManifests() {
  return [
    selectionManifest("layrs.code", ["application/json"], ["rs"]),
    selectionManifest("layrs.image", ["image/png"], ["png"]),
    selectionManifest("layrs.text", ["text/plain"], ["md"]),
    selectionManifest("layrs.raw", ["application/octet-stream"], [])
  ];
}

function selectionManifest(id, supportedMediaTypes, fileExtensions) {
  return createLensManifest({
    ...manifestInput(id),
    supportedMediaTypes,
    fileExtensions
  });
}

function assertSchemaRejects(definition, value, label) {
  assert.notEqual(
    validationErrorsForDefinition(schema, definition, value).length,
    0,
    label
  );
}

function clone(value) {
  return JSON.parse(JSON.stringify(value));
}

function validationPasses(callback) {
  try {
    callback();
    return true;
  } catch {
    return false;
  }
}

async function testExecutionProof() {
  const implementationBytes = new TextEncoder().encode("immutable reconcile implementation");
  const executableManifest = clone(expected.manifest);
  executableManifest.bundle.entrypoints.reconcile.digest =
    `sha256:${crypto.createHash("sha256").update(implementationBytes).digest("hex")}`;

  const loaded = await LoadedLensReconcileImplementation.load(
    executableManifest,
    implementationBytes
  );
  const verifiedExecution = loaded.execution();
  implementationBytes[0] ^= 0xff;
  assert.equal(
    new TextDecoder().decode(loaded.implementationBytes()),
    "immutable reconcile implementation"
  );
  await assert.rejects(
    LoadedLensReconcileImplementation.load(executableManifest, implementationBytes),
    /implementation bytes do not match/
  );
  assert.throws(
    () =>
      new VerifiedLensReconcileExecution(
        Symbol("forged"),
        executableManifest,
        expected.reconcileInput.lens,
        executableManifest.bundle.entrypoints.reconcile.digest
      ),
    /only be created by the implementation loader/
  );
  assert.throws(
    () =>
      toLensReconcileResponse(
        { ...verifiedExecution },
        expected.reconcileInput.lens,
        reconcileResult()
      ),
    /was not created by the implementation loader/
  );

  const matchingResponse = toLensReconcileResponse(
    verifiedExecution,
    expected.reconcileInput.lens,
    reconcileResult()
  );
  assert.deepEqual(matchingResponse.lens, expected.reconcileInput.lens);
  for (const vector of negativeVectors.responsePinMismatches) {
    const requested = { ...expected.reconcileInput.lens, ...vector.override };
    assert.throws(
      () => toLensReconcileResponse(verifiedExecution, requested, reconcileResult()),
      /Requested Lens pin does not match/,
      vector.name
    );
  }
}

testExecutionProof().catch((error) => {
  process.nextTick(() => {
    throw error;
  });
});
