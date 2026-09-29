#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const vectors = JSON.parse(fs.readFileSync(path.join(here, "golden-vectors.json"), "utf8"));
const common = JSON.parse(fs.readFileSync(path.join(here, "common.schema.json"), "utf8"));

function jsonPointer(document, fragment) {
  if (fragment === "" || fragment === "#") return document;
  assert(fragment.startsWith("#/"), `unsupported JSON Schema fragment ${fragment}`);
  return fragment.slice(2).split("/").reduce((value, token) => {
    const key = decodeURIComponent(token).replaceAll("~1", "/").replaceAll("~0", "~");
    assert(value !== undefined && Object.hasOwn(value, key), `missing JSON pointer ${fragment}`);
    return value[key];
  }, document);
}

function verifySchemaRefs() {
  const names = fs.readdirSync(here).filter((name) => name.endsWith(".schema.json"));
  const documents = new Map(names.map((name) => [name, JSON.parse(fs.readFileSync(path.join(here, name), "utf8"))]));
  let refCount = 0;
  function visit(value, sourceName) {
    if (Array.isArray(value)) {
      for (const item of value) visit(item, sourceName);
      return;
    }
    if (value === null || typeof value !== "object") return;
    if (typeof value.$ref === "string") {
      refCount += 1;
      const [targetName, rawFragment = ""] = value.$ref.split("#", 2);
      assert(!targetName.includes(":"), `remote ref is not vendored: ${value.$ref}`);
      const resolvedName = targetName === "" ? sourceName : targetName;
      assert(documents.has(resolvedName), `${sourceName}: missing schema ${resolvedName}`);
      jsonPointer(documents.get(resolvedName), rawFragment === "" ? "" : `#${rawFragment}`);
    }
    for (const child of Object.values(value)) visit(child, sourceName);
  }
  for (const [name, document] of documents) visit(document, name);
  return { schemaCount: documents.size, refCount };
}

const IV = Uint32Array.from([
  0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
  0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
]);
const PERMUTATION = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];
const CHUNK_START = 1;
const CHUNK_END = 2;
const PARENT = 4;
const ROOT = 8;

function rotr(value, count) {
  return ((value >>> count) | (value << (32 - count))) >>> 0;
}

function add(left, right) {
  return (left + right) >>> 0;
}

function g(state, a, b, c, d, mx, my) {
  state[a] = add(add(state[a], state[b]), mx);
  state[d] = rotr(state[d] ^ state[a], 16);
  state[c] = add(state[c], state[d]);
  state[b] = rotr(state[b] ^ state[c], 12);
  state[a] = add(add(state[a], state[b]), my);
  state[d] = rotr(state[d] ^ state[a], 8);
  state[c] = add(state[c], state[d]);
  state[b] = rotr(state[b] ^ state[c], 7);
}

function round(state, message, schedule) {
  g(state, 0, 4, 8, 12, message[schedule[0]], message[schedule[1]]);
  g(state, 1, 5, 9, 13, message[schedule[2]], message[schedule[3]]);
  g(state, 2, 6, 10, 14, message[schedule[4]], message[schedule[5]]);
  g(state, 3, 7, 11, 15, message[schedule[6]], message[schedule[7]]);
  g(state, 0, 5, 10, 15, message[schedule[8]], message[schedule[9]]);
  g(state, 1, 6, 11, 12, message[schedule[10]], message[schedule[11]]);
  g(state, 2, 7, 8, 13, message[schedule[12]], message[schedule[13]]);
  g(state, 3, 4, 9, 14, message[schedule[14]], message[schedule[15]]);
}

function wordsFromBlock(bytes) {
  const block = new Uint8Array(64);
  block.set(bytes);
  const view = new DataView(block.buffer);
  return Uint32Array.from({ length: 16 }, (_, index) => view.getUint32(index * 4, true));
}

function compress(cv, blockWords, counter, blockLength, flags) {
  const state = new Uint32Array(16);
  state.set(cv, 0);
  state.set(IV.subarray(0, 4), 8);
  state[12] = counter >>> 0;
  state[13] = Math.floor(counter / 0x100000000) >>> 0;
  state[14] = blockLength >>> 0;
  state[15] = flags >>> 0;
  let schedule = Array.from({ length: 16 }, (_, index) => index);
  for (let index = 0; index < 7; index += 1) {
    round(state, blockWords, schedule);
    schedule = PERMUTATION.map((position) => schedule[position]);
  }
  const output = new Uint32Array(16);
  for (let index = 0; index < 8; index += 1) {
    output[index] = (state[index] ^ state[index + 8]) >>> 0;
    output[index + 8] = (state[index + 8] ^ cv[index]) >>> 0;
  }
  return output;
}

function outputCv(output) {
  return compress(
    output.inputCv,
    output.blockWords,
    output.counter,
    output.blockLength,
    output.flags,
  ).slice(0, 8);
}

function chunkOutput(bytes, chunkCounter) {
  let cv = IV.slice();
  const blockCount = Math.max(1, Math.ceil(bytes.length / 64));
  for (let index = 0; index < blockCount - 1; index += 1) {
    const block = bytes.subarray(index * 64, index * 64 + 64);
    const flags = index === 0 ? CHUNK_START : 0;
    cv = compress(cv, wordsFromBlock(block), chunkCounter, 64, flags).slice(0, 8);
  }
  const lastIndex = blockCount - 1;
  const last = bytes.subarray(lastIndex * 64);
  return {
    inputCv: cv,
    blockWords: wordsFromBlock(last),
    counter: chunkCounter,
    blockLength: last.length,
    flags: CHUNK_END | (lastIndex === 0 ? CHUNK_START : 0),
  };
}

function parentOutput(left, right) {
  const words = new Uint32Array(16);
  words.set(left, 0);
  words.set(right, 8);
  return { inputCv: IV.slice(), blockWords: words, counter: 0, blockLength: 64, flags: PARENT };
}

function blake3(bytes) {
  const chunkCount = Math.max(1, Math.ceil(bytes.length / 1024));
  const stack = [];
  for (let index = 0; index < chunkCount - 1; index += 1) {
    let cv = outputCv(chunkOutput(bytes.subarray(index * 1024, index * 1024 + 1024), index));
    let totalChunks = index + 1;
    while ((totalChunks & 1) === 0) {
      cv = outputCv(parentOutput(stack.pop(), cv));
      totalChunks >>>= 1;
    }
    stack.push(cv);
  }
  const lastOffset = (chunkCount - 1) * 1024;
  let output = chunkOutput(bytes.subarray(lastOffset), chunkCount - 1);
  while (stack.length > 0) output = parentOutput(stack.pop(), outputCv(output));
  const rootWords = compress(output.inputCv, output.blockWords, 0, output.blockLength, output.flags | ROOT);
  const digest = Buffer.alloc(32);
  for (let index = 0; index < 8; index += 1) digest.writeUInt32LE(rootWords[index], index * 4);
  return digest.toString("hex");
}

function compareUtf8(left, right) {
  return Buffer.compare(Buffer.from(left, "utf8"), Buffer.from(right, "utf8"));
}

function canonicalJson(value) {
  if (value === null || typeof value === "boolean" || typeof value === "string") {
    return JSON.stringify(value);
  }
  if (typeof value === "number") {
    assert(Number.isFinite(value), "canonical JSON rejects non-finite numbers");
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  // RFC 8785 follows ECMAScript/UTF-16 property ordering, not UTF-8 ordering.
  const keys = Object.keys(value).sort();
  return `{${keys.map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
}

function stepCanonical(input) {
  assert.equal(new Set(input.parentStepIds).size, input.parentStepIds.length, "duplicate Step parent");
  const parents = [...input.parentStepIds].sort(compareUtf8);
  return [
    `parent-count\t${parents.length}`,
    ...parents.map((parent) => `parent\t${parent}`),
    `root-tree\t${input.rootTreeId}`,
    "",
  ].join("\n");
}

function identityPayload(vector) {
  if (vector.identityKind === "rawUtf8") return Buffer.from(vector.input, "utf8");
  if (vector.identityKind === "stepV1") return Buffer.from(stepCanonical(vector.input), "utf8");
  if (vector.identityKind === "treeV1") {
    const identity = structuredClone(vector.input);
    delete identity[vector.idField];
    identity.entries.sort((left, right) => compareUtf8(left.name, right.name));
    return Buffer.from(canonicalJson(identity), "utf8");
  }
  if (vector.identityKind === "jcsV1") {
    const identity = structuredClone(vector.input);
    delete identity[vector.idField];
    return Buffer.from(canonicalJson(identity), "utf8");
  }
  throw new Error(`unknown identity kind ${vector.identityKind}`);
}

function domainSeparatedBytes(domain, payload) {
  assert(/^[\x21-\x7e]+$/.test(domain), `identity domain must be non-empty ASCII: ${domain}`);
  return Buffer.concat([Buffer.from(domain, "ascii"), Buffer.from([0]), payload]);
}

function contentId(domain, payload) {
  return `blake3:${blake3(domainSeparatedBytes(domain, payload))}`;
}

function identityBytes(vector) {
  return domainSeparatedBytes(vector.domain, identityPayload(vector));
}

function isAntichain(value) {
  const parents = new Map(value.parents.map((entry) => [entry.stepId, entry.parentStepIds]));
  const heads = new Set(value.heads);
  for (const head of heads) {
    const pending = [...(parents.get(head) ?? [])];
    const seen = new Set();
    while (pending.length > 0) {
      const ancestor = pending.pop();
      if (seen.has(ancestor)) continue;
      if (heads.has(ancestor)) return false;
      seen.add(ancestor);
      pending.push(...(parents.get(ancestor) ?? []));
    }
  }
  return true;
}

function rejects(vector) {
  if (vector.kind === "duplicateStepParents") {
    return new Set(vector.value).size !== vector.value.length;
  }
  if (vector.kind === "layerHeadAntichain") return !isAntichain(vector.value);
  if (vector.kind === "fileChunkLayout") {
    let offset = 0;
    for (const chunk of vector.value.chunks) {
      if (chunk.offset !== offset || chunk.byteLength <= 0) return true;
      offset += chunk.byteLength;
    }
    return offset !== vector.value.byteLength;
  }
  if (vector.kind === "fastCdcParameters") {
    const { minBytes, targetBytes, maxBytes } = vector.value;
    return !(minBytes <= targetBytes && targetBytes <= maxBytes);
  }
  throw new Error(`unknown rejection kind ${vector.kind}`);
}

function verifyObjectVectors() {
  const chunks = new Map();
  for (const vector of vectors.objectVectors.filter((item) => item.kind === "chunk")) {
    const value = vector.value;
    assert.equal(value.schemaVersion, "layrs.chunk.v1", vector.name);
    assert.equal(value.content.encoding, "base64", vector.name);
    const raw = Buffer.from(value.content.bytes, "base64");
    assert.equal(raw.toString("base64"), value.content.bytes, `${vector.name}: canonical base64`);
    assert.equal(raw.length, value.byteLength, `${vector.name}: byteLength`);
    assert.equal(contentId("layrs-chunk-v1", raw), value.chunkId, `${vector.name}: ChunkId`);
    chunks.set(value.chunkId, raw);
  }
  const manifests = [
    ...vectors.objectVectors.filter((item) => item.kind === "fileManifest")
      .map((item) => ({ name: item.name, value: item.value })),
    ...vectors.hashVectors.filter((item) => item.domain === "layrs-file-manifest-v1")
      .map((item) => ({ name: item.name, value: item.input })),
  ];
  const manifestsByFile = new Map();
  for (const vector of manifests) {
    const value = vector.value;
    assert.equal(value.schemaVersion, "layrs.file-manifest.v1", vector.name);
    let offset = 0;
    const parts = [];
    for (const chunk of value.chunks) {
      assert.equal(chunk.offset, offset, `${vector.name}: contiguous offset`);
      const raw = chunks.get(chunk.chunkId);
      assert(raw, `${vector.name}: known ChunkId`);
      assert.equal(raw.length, chunk.byteLength, `${vector.name}: chunk byteLength`);
      parts.push(raw);
      offset += raw.length;
    }
    assert.equal(offset, value.byteLength, `${vector.name}: file byteLength`);
    assert.equal(contentId("layrs-file-v1", Buffer.concat(parts)), value.fileId, `${vector.name}: FileId`);
    const manifestPayload = structuredClone(value);
    delete manifestPayload.manifestId;
    assert.equal(
      contentId("layrs-file-manifest-v1", Buffer.from(canonicalJson(manifestPayload), "utf8")),
      value.manifestId,
      `${vector.name}: FileManifestId`,
    );
    const ids = manifestsByFile.get(value.fileId) ?? new Set();
    ids.add(value.manifestId);
    manifestsByFile.set(value.fileId, ids);
  }
  assert(
    [...manifestsByFile.values()].some((ids) => ids.size >= 2),
    "goldens must prove that one FileId may have several FileManifestIds",
  );
}

async function verifyDraft202012Fixtures() {
  let Ajv2020;
  try {
    Ajv2020 = require("ajv/dist/2020.js").default;
  } catch (error) {
    throw new Error("Ajv 2020 is required; run `pnpm install` before contract validation", { cause: error });
  }
  // strictRequired/strictTypes are disabled because conditional/allOf
  // subschemas may rely on declarations in their enclosing/ref'd schema;
  // both constructs are valid Draft 2020-12.
  const ajv = new Ajv2020({
    allErrors: true,
    strict: true,
    strictRequired: false,
    strictTypes: false,
    validateFormats: false,
  });
  ajv.addKeyword({
    keyword: "x-layrs-unicode-normalization",
    schemaType: "string",
    validate(normalization, data) {
      return typeof data !== "string" || data.normalize(normalization) === data;
    },
  });
  const schemaNames = fs.readdirSync(here).filter((name) => name.endsWith(".schema.json"));
  for (const name of schemaNames) {
    ajv.addSchema(JSON.parse(fs.readFileSync(path.join(here, name), "utf8")));
  }
  for (const name of schemaNames) {
    const document = JSON.parse(fs.readFileSync(path.join(here, name), "utf8"));
    assert(ajv.getSchema(document.$id), `${name}: Draft 2020-12 schema did not compile`);
  }
  const fixtures = [
    ...vectors.hashVectors.filter((item) => item.schemaRef)
      .map((item) => ({ name: item.name, schemaRef: item.schemaRef, value: item.input })),
    ...vectors.objectVectors
      .map((item) => ({ name: item.name, schemaRef: item.schemaRef, value: item.value })),
  ];
  for (const fixture of fixtures) {
    const validate = ajv.getSchema(fixture.schemaRef)
      ?? ajv.compile({ $ref: fixture.schemaRef });
    if (!validate(fixture.value)) {
      assert.fail(`${fixture.name}: Draft 2020-12 fixture invalid: ${ajv.errorsText(validate.errors)}`);
    }
  }
  return { schemaCount: schemaNames.length, fixtureCount: fixtures.length };
}

assert.equal(blake3(Buffer.alloc(0)), "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262");
assert.equal(blake3(Buffer.from("hello")), "ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f");

for (const vector of vectors.hashVectors) {
  const bytes = identityBytes(vector);
  const actual = `blake3:${blake3(bytes)}`;
  if (process.argv.includes("--print")) {
    console.log(JSON.stringify({
      name: vector.name,
      expectedIdentityUtf8: bytes.toString("utf8"),
      expectedObjectId: actual,
    }));
  } else {
    assert.equal(bytes.toString("utf8"), vector.expectedIdentityUtf8, `${vector.name}: identity bytes`);
    assert.equal(actual, vector.expectedObjectId, `${vector.name}: ObjectId`);
    if (vector.idField) {
      assert.equal(vector.input[vector.idField], vector.expectedObjectId, `${vector.name}: claimed id`);
    }
  }
}

if (process.argv.includes("--print")) {
  for (const vector of vectors.objectVectors.filter((item) => item.kind === "fileManifest")) {
    const payload = structuredClone(vector.value);
    delete payload.manifestId;
    const bytes = domainSeparatedBytes(
      "layrs-file-manifest-v1",
      Buffer.from(canonicalJson(payload), "utf8"),
    );
    console.log(JSON.stringify({
      name: vector.name,
      expectedIdentityUtf8: bytes.toString("utf8"),
      expectedObjectId: `blake3:${blake3(bytes)}`,
    }));
  }
  process.exit(0);
}

for (const vector of vectors.sortVectors) {
  assert.deepEqual([...vector.input].sort(compareUtf8), vector.expected, vector.name);
}

const repoPath = new RegExp(common.$defs.RepoPath.pattern, "u");
for (const vector of vectors.repoPathVectors) {
  const valid = repoPath.test(vector.value) && vector.value.normalize("NFC") === vector.value;
  assert.equal(valid, vector.valid, `RepoPath ${JSON.stringify(vector.value)}`);
}

for (const vector of vectors.rejectVectors) {
  assert(rejects(vector), `${vector.name} must reject with ${vector.expectedCode}`);
}

verifyObjectVectors();

const refs = verifySchemaRefs();
const draft = await verifyDraft202012Fixtures();
console.log(`verified ${vectors.hashVectors.length} domain-separated hashes, ${vectors.objectVectors.length} representation objects, ${draft.fixtureCount} Draft 2020-12 fixtures, ${vectors.sortVectors.length} sorts, ${vectors.repoPathVectors.length} paths, ${vectors.rejectVectors.length} rejections, ${refs.refCount} refs across ${draft.schemaCount} schemas`);
