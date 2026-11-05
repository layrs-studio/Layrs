// Device-local storage contract. No network, credentials or browser needed.
import test from "node:test";
import assert from "node:assert/strict";
import { loadUiTypeScript } from "./lib/load-ui-typescript.mjs";
const module = { exports: loadUiTypeScript("native-review-drafts.ts") };
const { readReviewDraft: read, saveReviewDraft: write, removeReviewDraft: remove, reviewDraftScope: canonical,
  REVIEW_DRAFT_STORAGE_KEY: KEY, REVIEW_DRAFT_MAX_BYTES: MAX, REVIEW_DRAFT_TOTAL_BYTES: TOTAL,
  REVIEW_DRAFT_TTL_MS: TTL, REVIEW_DRAFT_MAX_ENTRIES: COUNT } = module.exports;
const plain = value => JSON.parse(JSON.stringify(value));
const now = 1800000000000;
const scope = { origin: "https://example.test", principalId: "principal:alice", spaceId: "space:one", reviewId: "review:one", version: 1 };
const input = { body: "À revoir 😀\nDeuxième ligne", path: "src/équipe.ts", line: 42, replyTo: "review-action:original" };
let sequence = 0;
const newKey = () => `draft-test-key-${String(++sequence).padStart(8, "0")}`;
const save = (storage, context = scope, value = input, time = now) => write(storage, context, value, time, newKey);
class Storage {
  value = null;
  writes = 0;
  getItem(key) { assert.equal(key, KEY); return this.value; }
  setItem(key, value) { assert.equal(key, KEY); this.value = value; this.writes++; }
  removeItem(key) { assert.equal(key, KEY); this.value = null; this.writes++; }
}

test("the pre-extraction comment envelope keeps its text and retry key without migration", () => {
  const storage = new Storage();
  storage.value = JSON.stringify({ schema: KEY, entries: [{ scope, draft: { ...input, idempotencyKey: "9ac0e513-c73d-4f0e-97bf-b060240cfbea", updatedAt: now } }] });
  const previous = storage.value;
  assert.equal(read(storage, scope, now).idempotencyKey, "9ac0e513-c73d-4f0e-97bf-b060240cfbea");
  assert.equal(storage.value, previous, "compatibility read does not rewrite the old store");
  assert.equal(save(storage).idempotencyKey, "9ac0e513-c73d-4f0e-97bf-b060240cfbea");
});

test("a Unicode reply draft round-trips without any transport or automatic restoration", () => {
  const storage = new Storage();
  const draft = save(storage);
  assert.deepEqual(plain(read(storage, scope, now)), { ...input, updatedAt: now, idempotencyKey: draft.idempotencyKey });
  assert.equal(storage.writes, 1, "reading a recovery candidate never writes or submits it");
  assert.deepEqual(plain(canonical({ ...scope, origin: "https://EXAMPLE.test:443/" })), scope);
});

test("account, origin, Space, review and version are independent isolation boundaries", () => {
  const storage = new Storage();
  save(storage);
  for (const change of [{ principalId: "principal:bob" }, { origin: "https://other.test" }, { spaceId: "space:two" }, { reviewId: "review:two" }, { version: 2 }]) {
    const other = { ...scope, ...change };
    assert.equal(read(storage, other, now), null);
    save(storage, other, { ...input, body: JSON.stringify(change) });
    assert.equal(read(storage, scope, now).body, input.body);
    assert.equal(read(storage, other, now).body, JSON.stringify(change));
  }
  assert.equal(JSON.parse(storage.value).entries.length, 6);
});

test("only whitelisted draft fields persist, never extra session properties", () => {
  const storage = new Storage();
  save(storage, { ...scope, token: "sentinel-scope-secret" }, { ...input, accessToken: "sentinel-input-secret", password: "sentinel-password" });
  assert.ok(!storage.value.includes("sentinel"));
  assert.deepEqual(Object.keys(JSON.parse(storage.value).entries[0].draft).sort(), ["body", "idempotencyKey", "line", "path", "replyTo", "updatedAt"]);
});

test("identical retries retain their key; changed payloads cannot reuse it", () => {
  const storage = new Storage();
  const initial = save(storage);
  const repeated = save(storage, scope, input, now + 10);
  assert.equal(initial.idempotencyKey, repeated.idempotencyKey);
  for (const patch of [{ body: "new" }, { path: "other.ts" }, { line: 7 }, { replyTo: null }]) {
    const changed = save(storage, scope, { ...input, ...patch });
    assert.notEqual(changed.idempotencyKey, initial.idempotencyKey);
  }
});

test("successful old requests cannot delete newer edits or another account's draft", () => {
  const storage = new Storage();
  const sent = save(storage);
  const newer = save(storage, scope, { ...input, body: "edited while sending" });
  const bob = { ...scope, principalId: "principal:bob" };
  save(storage, bob);
  const before = storage.value;
  assert.equal(remove(storage, scope, sent.idempotencyKey, now), false);
  assert.equal(storage.value, before);
  assert.equal(remove(storage, scope, newer.idempotencyKey, now), true);
  assert.equal(read(storage, scope, now), null);
  assert.equal(read(storage, bob, now).body, input.body);
  assert.equal(remove(storage, bob, undefined, now), true);
  assert.equal(storage.value, null);
});

test("expired and implausibly future records are not offered and reads preserve storage", () => {
  const storage = new Storage();
  save(storage);
  const before = storage.value;
  assert.ok(read(storage, scope, now + TTL));
  assert.equal(read(storage, scope, now + TTL + 1), null);
  assert.equal(read(storage, scope, now - 300001), null);
  assert.equal(storage.value, before);
  save(storage, { ...scope, version: 2 }, input, now + TTL + 1);
  assert.equal(JSON.parse(storage.value).entries.length, 1);
});

test("malformed scopes cannot share or escape an authenticated context", () => {
  for (const patch of [{ origin: "http://example.test" }, { origin: "https://name:password@example.test" }, { origin: "https://example.test/other" },
    { origin: "https://example.test/?q=a" }, { origin: "https://example.test/#fragment" }, { principalId: "" }, { principalId: "principal:a/b" },
    { reviewId: "space:one" }, { spaceId: "../space" }, { version: 0 }, { version: 10002 }, { version: 1.5 }]) {
    const storage = new Storage();
    assert.throws(() => save(storage, { ...scope, ...patch }));
    assert.equal(storage.value, null);
  }
});

test("corrupt, future schema, duplicate and unknown-field storage is never silently overwritten", () => {
  const storage = new Storage();
  save(storage);
  const valid = JSON.parse(storage.value);
  const variants = ["{", "null", JSON.stringify({ ...valid, schema: "future" }), JSON.stringify({ ...valid, token: "unknown" }),
    JSON.stringify({ ...valid, entries: [valid.entries[0], valid.entries[0]] }),
    JSON.stringify({ ...valid, entries: [{ ...valid.entries[0], scope: { ...scope, token: "unknown" } }] }),
    JSON.stringify({ ...valid, entries: [{ ...valid.entries[0], draft: { ...valid.entries[0].draft, token: "unknown" } }] }),
    " ".repeat(TOTAL + 1)];
  for (const malformed of variants) {
    storage.value = malformed;
    assert.throws(() => read(storage, scope, now));
    assert.throws(() => save(storage));
    assert.equal(storage.value, malformed);
  }
});

test("UTF-8 entry limit counts bytes and failure preserves the previous draft", () => {
  const storage = new Storage();
  save(storage);
  const before = storage.value;
  assert.throws(() => save(storage, scope, { ...input, body: "é".repeat(MAX / 2) }));
  assert.throws(() => save(storage, scope, { ...input, path: "x".repeat(4097) }));
  assert.throws(() => save(storage, scope, { ...input, line: 2000001 }));
  assert.equal(storage.value, before);
});

test("entry and total byte budgets refuse growth without evicting other drafts", () => {
  const storage = new Storage();
  for (let index = 0; index < COUNT; index++) save(storage, { ...scope, version: index + 1 });
  const before = storage.value;
  assert.throws(() => save(storage, { ...scope, version: COUNT + 1 }));
  assert.equal(storage.value, before);
  const large = new Storage();
  for (let version = 1; version <= 4; version++) save(large, { ...scope, version }, { ...input, body: "x".repeat(60000) });
  const bytesBefore = large.value;
  assert.throws(() => save(large, { ...scope, version: 5 }, { ...input, body: "x".repeat(60000) }));
  assert.equal(large.value, bytesBefore);
});

test("unavailable browser storage propagates a recoverable failure without deleting data", () => {
  const storage = new Storage();
  save(storage);
  const before = storage.value;
  storage.setItem = () => { throw new Error("QuotaExceededError"); };
  assert.throws(() => save(storage, scope, { ...input, body: "new unsaved text" }), /QuotaExceededError/);
  assert.equal(storage.value, before);
  assert.equal(read(storage, scope, now).body, input.body);
});
