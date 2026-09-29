import fs from "node:fs";
import assert from "node:assert/strict";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);

let Ajv2020;
try {
  Ajv2020 = require("ajv/dist/2020.js").default;
} catch (error) {
  throw new Error("Ajv 2020 is required; run `pnpm install` before Sync V3 contract validation", { cause: error });
}

const here = path.dirname(fileURLToPath(import.meta.url));
const scmDir = path.resolve(here, "../scm-v1");
const examplesDir = path.join(here, "examples");

// Small dependency-free BLAKE3 oracle, shared in behavior with the SCM golden
// verifier so contract vectors do not trust an implementation under test.
const IV = Uint32Array.from([0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]);
const PERMUTATION = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];
const CHUNK_START = 1, CHUNK_END = 2, PARENT = 4, ROOT = 8;
const rotr = (v, n) => ((v >>> n) | (v << (32 - n))) >>> 0;
const add = (a, b) => (a + b) >>> 0;
function g(s, a, b, c, d, x, y) {
  s[a] = add(add(s[a], s[b]), x); s[d] = rotr(s[d] ^ s[a], 16);
  s[c] = add(s[c], s[d]); s[b] = rotr(s[b] ^ s[c], 12);
  s[a] = add(add(s[a], s[b]), y); s[d] = rotr(s[d] ^ s[a], 8);
  s[c] = add(s[c], s[d]); s[b] = rotr(s[b] ^ s[c], 7);
}
function round(s, m, p) {
  g(s,0,4,8,12,m[p[0]],m[p[1]]); g(s,1,5,9,13,m[p[2]],m[p[3]]);
  g(s,2,6,10,14,m[p[4]],m[p[5]]); g(s,3,7,11,15,m[p[6]],m[p[7]]);
  g(s,0,5,10,15,m[p[8]],m[p[9]]); g(s,1,6,11,12,m[p[10]],m[p[11]]);
  g(s,2,7,8,13,m[p[12]],m[p[13]]); g(s,3,4,9,14,m[p[14]],m[p[15]]);
}
function words(bytes) {
  const block = new Uint8Array(64); block.set(bytes);
  const view = new DataView(block.buffer);
  return Uint32Array.from({ length: 16 }, (_, i) => view.getUint32(i * 4, true));
}
function compress(cv, block, counter, length, flags) {
  const s = new Uint32Array(16); s.set(cv); s.set(IV.subarray(0, 4), 8);
  s[12] = counter >>> 0; s[13] = Math.floor(counter / 0x100000000) >>> 0;
  s[14] = length >>> 0; s[15] = flags >>> 0;
  let p = Array.from({ length: 16 }, (_, i) => i);
  for (let i = 0; i < 7; i += 1) { round(s, block, p); p = PERMUTATION.map((x) => p[x]); }
  const out = new Uint32Array(16);
  for (let i = 0; i < 8; i += 1) { out[i] = (s[i] ^ s[i + 8]) >>> 0; out[i + 8] = (s[i + 8] ^ cv[i]) >>> 0; }
  return out;
}
const outputCv = (o) => compress(o.cv, o.block, o.counter, o.length, o.flags).slice(0, 8);
function chunkOutput(bytes, counter) {
  let cv = IV.slice(); const count = Math.max(1, Math.ceil(bytes.length / 64));
  for (let i = 0; i < count - 1; i += 1) cv = compress(cv, words(bytes.subarray(i * 64, i * 64 + 64)), counter, 64, i === 0 ? CHUNK_START : 0).slice(0, 8);
  const i = count - 1, last = bytes.subarray(i * 64);
  return { cv, block: words(last), counter, length: last.length, flags: CHUNK_END | (i === 0 ? CHUNK_START : 0) };
}
function parentOutput(left, right) { const block = new Uint32Array(16); block.set(left); block.set(right, 8); return { cv: IV.slice(), block, counter: 0, length: 64, flags: PARENT }; }
function blake3(bytes) {
  const count = Math.max(1, Math.ceil(bytes.length / 1024)), stack = [];
  for (let i = 0; i < count - 1; i += 1) {
    let cv = outputCv(chunkOutput(bytes.subarray(i * 1024, i * 1024 + 1024), i)), total = i + 1;
    while ((total & 1) === 0) { cv = outputCv(parentOutput(stack.pop(), cv)); total >>>= 1; }
    stack.push(cv);
  }
  let out = chunkOutput(bytes.subarray((count - 1) * 1024), count - 1);
  while (stack.length) out = parentOutput(stack.pop(), outputCv(out));
  const root = compress(out.cv, out.block, 0, out.length, out.flags | ROOT), digest = Buffer.alloc(32);
  for (let i = 0; i < 8; i += 1) digest.writeUInt32LE(root[i], i * 4);
  return digest.toString("hex");
}
function canonicalJson(value) {
  if (value === null || typeof value === "boolean" || typeof value === "string" || typeof value === "number") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
}
const identityId = (domain, payload) => `blake3:${blake3(Buffer.concat([Buffer.from(domain, "ascii"), Buffer.from([0]), payload]))}`;

function verifyIdentityVectors() {
  const byName = new Map(identityVectors.hashVectors.map((vector) => [vector.name, vector]));
  for (const vector of identityVectors.hashVectors) {
    let payload;
    if (vector.kind === "evidence") {
      payload = Buffer.from(vector.inputBase64, "base64");
      assert.equal(payload.length, vector.expectedByteLength, `${vector.name}: byteLength`);
    } else {
      const identity = structuredClone(vector.input); delete identity.productObjectId;
      payload = Buffer.from(canonicalJson(identity), "utf8");
      if (!process.argv.includes("--print-identities")) assert.equal(`${vector.domain}\0${payload}`, vector.expectedIdentityUtf8, `${vector.name}: identity bytes`);
      assert.equal(vector.input.kind, "layer", `${vector.name}: variant kind`);
      assert.equal(vector.input.productId, vector.input.record.layerId, `${vector.name}: logical/record ID`);
      assert.deepEqual(vector.input.dependsOnStepIds, [], `${vector.name}: exact Step dependencies`);
      assert.deepEqual(vector.input.dependsOnProductIds, vector.input.record.parentLayerIds, `${vector.name}: exact Product dependencies`);
      const validate = ajv.getSchema("https://layrs.dev/schemas/sync-v3/products.schema.json");
      assert(validate(vector.input), `${vector.name}: Product schema: ${ajv.errorsText(validate.errors)}`);
    }
    const actual = identityId(vector.domain, payload);
    if (process.argv.includes("--print-identities")) console.log(JSON.stringify({ name: vector.name, expectedIdentityUtf8: vector.kind === "product" ? `${vector.domain}\0${payload}` : undefined, expectedObjectId: actual }));
    else {
      assert.equal(actual, vector.expectedObjectId, `${vector.name}: identity`);
      if (vector.kind === "product") assert.equal(vector.input.productObjectId, actual, `${vector.name}: claim`);
    }
  }
  if (process.argv.includes("--print-identities")) return;
  for (const reject of identityVectors.rejectVectors) {
    const source = byName.get(reject.source); assert(source, reject.name);
    if (reject.kind === "wrongProductClaim") assert.notEqual(reject.claimedObjectId, source.expectedObjectId, reject.name);
    else if (reject.kind === "wrongObjectKind") assert.notEqual(reject.descriptorKind, source.kind, reject.name);
    else if (reject.kind === "nonJcsProduct") {
      const full = structuredClone(source.input); full.productObjectId = source.expectedObjectId;
      const wire = Buffer.from(JSON.stringify(full, null, 2), "utf8");
      assert.notEqual(wire.toString("utf8"), canonicalJson(full), reject.name);
      assert.deepEqual(JSON.parse(wire), full, `${reject.name}: same JSON value`);
    } else throw new Error(`unknown identity rejection ${reject.kind}`);
  }
}

const readJson = (file) => JSON.parse(fs.readFileSync(file, "utf8"));
const identityVectors = readJson(path.join(here, "identity-vectors.json"));
const receiptDigestVectors = readJson(path.join(here, "receipt-digest-vectors.json"));
const schemaFiles = [scmDir, here].flatMap((directory) =>
  fs.readdirSync(directory)
    .filter((name) => name.endsWith(".schema.json"))
    .sort()
    .map((name) => path.join(directory, name)),
);

const ajv = new Ajv2020({
  allErrors: true,
  strict: true,
  strictRequired: false,
  strictTypes: false,
  validateFormats: true,
});
ajv.addKeyword({ keyword: "x-layrs-unicode-normalization", schemaType: "string", valid: true });
ajv.addFormat("date-time", {
  type: "string",
  validate: (value) => /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.test(value)
    && Number.isFinite(Date.parse(value)),
});
ajv.addFormat("uri", {
  type: "string",
  validate: (value) => {
    try {
      const url = new URL(value);
      return url.protocol === "https:" || url.protocol === "http:";
    } catch {
      return false;
    }
  },
});

for (const file of schemaFiles) {
  ajv.addSchema(readJson(file));
}

const syncSchemas = fs.readdirSync(here)
  .filter((name) => name.endsWith(".schema.json"))
  .sort();
for (const name of syncSchemas) {
  ajv.getSchema(readJson(path.join(here, name)).$id);
}

verifyIdentityVectors();
if (process.argv.includes("--print-identities")) process.exit(0);

const cases = [
  ["authorization-snapshot.json", "authorization.schema.json"],
  ["bootstrap.json", "bootstrap.schema.json"],
  ["event-page-policy-response.json", "event-page.schema.json"],
  ["event-page-request.json", "event-page.schema.json"],
  ["event-page-response.json", "event-page.schema.json"],
  ["fetch-plan-request.json", "fetch-plan.schema.json"],
  ["fetch-plan-response.json", "fetch-plan.schema.json"],
  ["layer-product.json", "products.schema.json"],
  ["object-plan-request.json", "object-plan.schema.json"],
  ["object-plan-response.json", "object-plan.schema.json"],
  ["policy-root-transaction.json", "transaction.schema.json"],
  ["policy-set.json", "authorization.schema.json"],
  ["receipt.json", "receipt.schema.json"],
  ["rejected-receipt.json", "receipt.schema.json"],
  ["transaction.json", "transaction.schema.json"],
  ["transaction-error.json", "transaction-error.schema.json"],
];

for (const [exampleName, schemaName] of cases) {
  const schema = readJson(path.join(here, schemaName));
  const validate = ajv.getSchema(schema.$id);
  if (!validate) throw new Error(`Schema was not compiled: ${schemaName}`);
  const valid = validate(readJson(path.join(examplesDir, exampleName)));
  if (!valid) {
    throw new Error(`${exampleName} failed ${schemaName}:\n${ajv.errorsText(validate.errors, { separator: "\n" })}`);
  }
}

const transactionValidator = ajv.getSchema(readJson(path.join(here, "transaction.schema.json")).$id);
const missingPolicyEpoch = readJson(path.join(examplesDir, "policy-root-transaction.json"));
delete missingPolicyEpoch.policyRootMutation.expectedAuthorizationEpoch;
assert.equal(transactionValidator(missingPolicyEpoch), false, "PolicyRoot CAS requires expectedAuthorizationEpoch");
const legacyChoose = readJson(path.join(examplesDir, "transaction.json"));
legacyChoose.viewMutation.layerMutations[0] = {
  layerId: "layer:main",
  mode: "choose",
  decisionProductId: "decision:legacy-ambiguous",
};
assert.equal(transactionValidator(legacyChoose), false, "legacy decisionProductId must fail closed");
const exactChoose = readJson(path.join(examplesDir, "transaction.json"));
exactChoose.viewMutation.layerMutations[0] = {
  layerId: "layer:main",
  mode: "choose",
  decisionProductObjectId: "blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
};
assert.equal(transactionValidator(exactChoose), true, `exact Decision Product object shape: ${ajv.errorsText(transactionValidator.errors)}`);
assert.deepEqual(
  exactChoose.viewMutation.expectedPolicyVersionObjectIds,
  [...new Set(exactChoose.viewMutation.expectedPolicyVersionObjectIds)].sort(),
  "expected Policy Product IDs are a canonical exact set",
);

const productValidator = ajv.getSchema(readJson(path.join(here, "products.schema.json")).$id);
const versionedPolicy = {
  kind: "policy",
  productId: "policy:default",
  productObjectId: `blake3:${"1".repeat(64)}`,
  dependsOnStepIds: [],
  dependsOnProductIds: [],
  previousProductObjectId: null,
  record: {
    schemaVersion: "layrs.policy.v1",
    policyId: "policy:default",
    versionId: "policy-version:default-v1",
    spaceId: "space:demo",
    rulesObjectId: `blake3:${"2".repeat(64)}`,
    createdAt: "2026-07-19T12:00:00Z",
  },
};
assert.equal(productValidator(versionedPolicy), true, `versioned Policy shape: ${ajv.errorsText(productValidator.errors)}`);
const policyWithoutPrevious = structuredClone(versionedPolicy);
delete policyWithoutPrevious.previousProductObjectId;
assert.equal(productValidator(policyWithoutPrevious), false, "versioned Policy requires previousProductObjectId");

const versionedGateDefinition = {
  kind: "gateDefinition",
  productId: "gate-definition:ci",
  productObjectId: `blake3:${"3".repeat(64)}`,
  dependsOnStepIds: [],
  dependsOnProductIds: [],
  previousProductObjectId: null,
  record: {
    schemaVersion: "layrs.gate-definition.v1",
    gateDefinitionId: "gate-definition:ci",
    spaceId: "space:demo",
    name: "CI",
    recipeObjectIds: [],
    policyVersionObjectIds: [],
    createdAt: "2026-07-19T12:00:00Z",
  },
};
assert.equal(productValidator(versionedGateDefinition), true, `versioned GateDefinition shape: ${ajv.errorsText(productValidator.errors)}`);

const singletonMarker = {
  kind: "marker",
  productId: "marker:checkpoint",
  productObjectId: `blake3:${"4".repeat(64)}`,
  dependsOnStepIds: [`blake3:${"5".repeat(64)}`],
  dependsOnProductIds: [],
  record: {
    schemaVersion: "layrs.marker.v1",
    markerId: "marker:checkpoint",
    spaceId: "space:demo",
    targetStepId: `blake3:${"5".repeat(64)}`,
    kind: "checkpoint",
    name: "checkpoint",
    title: "",
    notes: "",
    createdBy: null,
    createdAt: "2026-07-19T12:00:00Z",
    proofIds: [],
    metadata: [],
  },
};
assert.equal(productValidator(singletonMarker), true, `singleton Marker shape: ${ajv.errorsText(productValidator.errors)}`);
singletonMarker.previousProductObjectId = null;
assert.equal(productValidator(singletonMarker), false, "singleton Products forbid previousProductObjectId");

const duplicateLogicalPolicySet = readJson(path.join(examplesDir, "policy-set.json"));
duplicateLogicalPolicySet.policyVersions.push({
  ...structuredClone(duplicateLogicalPolicySet.policyVersions[0]),
  policyVersionId: "policy-version:default-v2",
  policyProductObjectId: `blake3:${"6".repeat(64)}`,
});
assert.notEqual(
  new Set(duplicateLogicalPolicySet.policyVersions.map((entry) => entry.policyId)).size,
  duplicateLogicalPolicySet.policyVersions.length,
  "a PolicySet cannot select two versions of one logical Policy",
);

const inlineWorktree = readJson(path.join(examplesDir, "transaction.json"));
const localView = structuredClone(readJson(path.join(examplesDir, "bootstrap.json")).currentRepositoryView.record);
localView.worktrees = [{
  worktreeId: "worktree:local-only",
  activeLayerId: null,
  baseStepId: null,
  materializedTreeId: null,
  status: "ready",
}];
inlineWorktree.viewMutation.proposedView = { kind: "inline", view: localView };
assert.equal(transactionValidator(inlineWorktree), false, "inline Sync V3 RepositoryView must reject Worktrees");

const bootstrapValidator = ajv.getSchema(readJson(path.join(here, "bootstrap.schema.json")).$id);
const bootstrapWorktree = readJson(path.join(examplesDir, "bootstrap.json"));
bootstrapWorktree.currentRepositoryView.record.worktrees = localView.worktrees;
assert.equal(bootstrapValidator(bootstrapWorktree), false, "bootstrap RepositoryView must reject Worktrees");
const restrictedBootstrap = readJson(path.join(examplesDir, "bootstrap.json"));
restrictedBootstrap.contentAccess = {schemaVersion:"layrs.content-access.v1",mode:"restricted"};
assert.equal(bootstrapValidator(restrictedBootstrap), true, "restricted bootstrap preserves canonical records");
restrictedBootstrap.contentAccess.mode = "full";
assert.equal(bootstrapValidator(restrictedBootstrap), false, "full access must omit the extension");
const contentPolicyValidator = ajv.getSchema(readJson(path.join(here,"content-policy-v2.schema.json")).$id);
const contentPolicy = {schemaVersion:"layrs.space-policy-rules.v2",rules:[],contentPolicies:[{principalIds:["principal:reader"],default:"deny",rules:[{effect:"allow",selector:{kind:"subtree",path:"public"}}]}]};
assert.equal(contentPolicyValidator(contentPolicy), true, "closed v2 content policy document");
contentPolicy.contentPolicies[0].rules[0].selector.kind = "regex";
assert.equal(contentPolicyValidator(contentPolicy), false, "no regex selector interpretation");

const uploadValidator = ajv.getSchema(readJson(path.join(here, "object-plan.schema.json")).$id);
const sensitiveUpload = readJson(path.join(examplesDir, "object-plan-response.json"));
sensitiveUpload.uploadCapabilities[0].uploadUrl = `https://layrs.example/v3/transfers/uploads/${sensitiveUpload.missingObjects[0].objectId}?token=secret`;
assert.equal(uploadValidator(sensitiveUpload), false, "transfer URL must not expose IDs or tokens");
for (const invalidUrl of [
  "https://exa mple/v3/transfers/uploads",
  "https://:/v3/transfers/uploads",
  "https://user@layrs.example/v3/transfers/uploads",
]) {
  const invalidUpload = readJson(path.join(examplesDir, "object-plan-response.json"));
  invalidUpload.uploadCapabilities[0].uploadUrl = invalidUrl;
  assert.equal(uploadValidator(invalidUpload), false, `invalid transfer authority must fail: ${invalidUrl}`);
}

const rejected = readJson(path.join(examplesDir, "rejected-receipt.json"));
assert.equal(rejected.status, "rejected");
assert.deepEqual(rejected.acceptedStepIds, []);
assert.deepEqual(rejected.acceptedProducts, []);
assert.deepEqual(rejected.eventIds, []);
assert.equal(rejected.eventCursor, null);
assert.ok(rejected.authorizationSnapshotId, "semantic rejection retains AuthorizationSnapshot");

const receiptValidator = ajv.getSchema(readJson(path.join(here, "receipt.schema.json")).$id);
const operationalAsReceipt = readJson(path.join(examplesDir, "rejected-receipt.json"));
operationalAsReceipt.rejections[0].code = "idempotency_digest_mismatch";
assert.equal(receiptValidator(operationalAsReceipt), false, "idempotency mismatch is not a semantic receipt rejection");

assert.equal(receiptDigestVectors.schemaVersion, "layrs.sync.receipt-digest-vectors.v1");
for (const vector of receiptDigestVectors.vectors) {
  assert.equal(vector.domain, "layrs.sync.v3/receipt", `${vector.name}: domain`);
  const canonical = Buffer.from(canonicalJson(readJson(path.join(examplesDir, vector.fixture))), "utf8");
  assert.equal(canonical.length, vector.canonicalByteLength, `${vector.name}: canonical byte length`);
  assert.equal(identityId(vector.domain, canonical), vector.expectedDigest, `${vector.name}: receipt storage digest`);
}

console.log(`Sync V3 contracts OK: ${syncSchemas.length} schemas, ${cases.length} fixtures, ${identityVectors.hashVectors.length} identity vectors, ${identityVectors.rejectVectors.length} identity rejections, ${receiptDigestVectors.vectors.length} receipt digest vector`);
