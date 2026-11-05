import test from "node:test";
import assert from "node:assert/strict";
import { loadUiTypeScript } from "./lib/load-ui-typescript.mjs";
const proposal = loadUiTypeScript("native-review-proposal-drafts.ts");
const comments = loadUiTypeScript("native-review-drafts.ts");
const { readProposalDraft: read, saveProposalDraft: write, removeProposalDraft: remove, proposalDraftTargets: targets, proposalRequest: request, PROPOSAL_DRAFT_STORAGE_KEY: KEY } = proposal;
const now = 1800000000000;
const scope = { origin: "https://example.test", principalId: "principal:alice", spaceId: "space:one", reviewId: null, expectedVersion: 0 };
const input = { title: "Proposition éclair", layerId: "layer:main", stepId: `blake3:${"a".repeat(64)}`, participants: "principal:alice, principal:bob" };
let sequence = 0;
const next = () => `proposal-test-key-${String(++sequence).padStart(8, "0")}`;
const save = (storage, context = scope, value = input, time = now) => write(storage, context, value, time, next);
const plain = value => JSON.parse(JSON.stringify(value));
class Storage {
  values = new Map(); writes = 0;
  getItem(key) { return this.values.get(key) ?? null; }
  setItem(key, value) { this.values.set(key, value); this.writes++; }
  removeItem(key) { this.values.delete(key); this.writes++; }
}

test("proposal title, exact Layer/Step and participants survive reopening without automatic publication", () => {
  const storage = new Storage(); const saved = save(storage);
  const reopened = read(storage, scope, now);
  assert.deepEqual(plain(reopened), { ...input, idempotencyKey: saved.idempotencyKey, updatedAt: now });
  assert.equal(storage.writes, 1);
  assert.deepEqual(plain(request(scope, reopened, reopened.idempotencyKey)), {
    layerId: input.layerId, proposedStepId: input.stepId, title: input.title, participants: ["principal:alice", "principal:bob"], expectedVersion: 0, idempotencyKey: saved.idempotencyKey,
  });
});

test("account, server, Space, review target and expected version isolate proposal drafts", () => {
  const storage = new Storage(); save(storage);
  for (const patch of [{ principalId: "principal:bob" }, { origin: "https://other.test" }, { spaceId: "space:two" }, { reviewId: "review:one", expectedVersion: 1 }, { reviewId: "review:one", expectedVersion: 2 }]) {
    const other = { ...scope, ...patch };
    assert.equal(read(storage, other, now), null); save(storage, other, { ...input, title: JSON.stringify(patch) });
    assert.equal(read(storage, scope, now).title, input.title);
  }
});

test("loss of a response keeps exactly the same request and idempotency key across a reload", () => {
  const storage = new Storage(); const saved = save(storage);
  const firstRequest = plain(request(scope, saved, saved.idempotencyKey));
  // A missing response has no successful acknowledgement and performs no deletion.
  const restored = read(storage, scope, now + 1);
  const retry = save(storage, scope, restored, now + 2);
  assert.deepEqual(plain(request(scope, retry, retry.idempotencyKey)), firstRequest);
  assert.equal(remove(storage, scope, retry.idempotencyKey, now + 3), true);
  assert.equal(read(storage, scope, now + 3), null);
});

test("an update accepted before a lost response can be resumed with its old expected version", () => {
  const storage = new Storage(); const old = { ...scope, reviewId: "review:one", expectedVersion: 1 };
  const saved = save(storage, old); const first = plain(request(old, saved, saved.idempotencyKey));
  const latest = { ...old, expectedVersion: 2 };
  assert.equal(read(storage, latest, now), null, "old input must not populate the current version silently");
  assert.deepEqual(plain(targets(storage, scope, "review:one", now)), [{ reviewId: "review:one", expectedVersion: 1 }]);
  const resumed = read(storage, old, now);
  assert.deepEqual(plain(request(old, resumed, resumed.idempotencyKey)), first);
  save(storage, latest, { ...input, title: "Independent draft of a later version" });
  remove(storage, old, resumed.idempotencyKey, now);
  assert.equal(read(storage, latest, now).title, "Independent draft of a later version");
});

test("old draft discovery reveals only target metadata for this account, Space and review", () => {
  const storage = new Storage();
  for (const context of [{ ...scope, reviewId: "review:one", expectedVersion: 1 }, { ...scope, reviewId: "review:one", expectedVersion: 2, principalId: "principal:bob" }, { ...scope, reviewId: "review:one", expectedVersion: 3, spaceId: "space:two" }, { ...scope, reviewId: "review:other", expectedVersion: 4 }]) save(storage, context);
  assert.deepEqual(plain(targets(storage, scope, "review:one", now)), [{ reviewId: "review:one", expectedVersion: 1 }]);
  assert.deepEqual(plain(targets(storage, { ...scope, origin: "https://other.test" }, "review:one", now)), []);
});

test("late successful acknowledgement cannot delete a newer edit", () => {
  const storage = new Storage(); const first = save(storage); const changed = save(storage, scope, { ...input, title: "New text" });
  assert.notEqual(first.idempotencyKey, changed.idempotencyKey);
  assert.equal(remove(storage, scope, first.idempotencyKey, now), false);
  assert.equal(read(storage, scope, now).title, "New text");
});

test("a response-loss retry retains the key actually sent even when local storage then fails", () => {
  const saved = save(new Storage());
  const first = proposal.prepareProposalAttempt("account-project-version-context", scope, input, saved.idempotencyKey, null, next);
  const retry = proposal.prepareProposalAttempt("account-project-version-context", scope, input, null, first, next);
  assert.equal(retry.key, saved.idempotencyKey);
  assert.deepEqual(plain(retry.request), plain(first.request));
  const changed = proposal.prepareProposalAttempt("account-project-version-context", scope, { ...input, title: "Changed" }, null, first, next);
  assert.notEqual(changed.key, first.key);
  const otherContext = proposal.prepareProposalAttempt("other-account-context", scope, input, null, first, next);
  assert.notEqual(otherContext.key, first.key);
});

test("partial input can be saved, but submission validates exact IDs and bounded participants", () => {
  const storage = new Storage(); const partial = save(storage, scope, { title: "WIP", layerId: "lay", stepId: "blake", participants: "princip" });
  assert.equal(read(storage, scope, now).layerId, "lay");
  assert.throws(() => request(scope, partial, partial.idempotencyKey));
  for (const patch of [{ participants: Array.from({ length: 101 }, (_, n) => `principal:p${n}`).join(",") }, { title: "é".repeat(513) }, { stepId: "HEAD" }, { participants: "principal:alice, not-a-principal" }]) assert.throws(() => request(scope, { ...input, ...patch }, next()));
});

test("closed proposal schema never serializes extra session fields or overwrites comment drafts", () => {
  const storage = new Storage();
  const commentScope = { ...scope, reviewId: "review:one", version: 1 };
  comments.saveReviewDraft(storage, commentScope, { body: "Separate comment", path: "", line: 1, replyTo: null }, now, next);
  const commentRaw = storage.getItem(comments.REVIEW_DRAFT_STORAGE_KEY);
  save(storage, { ...scope, bearer: "sentinel-secret" }, { ...input, session: "sentinel-session" });
  assert.ok(!storage.getItem(KEY).includes("sentinel"));
  assert.equal(storage.getItem(comments.REVIEW_DRAFT_STORAGE_KEY), commentRaw);
  assert.equal(comments.readReviewDraft(storage, commentScope, now).body, "Separate comment");
});

test("schema, count, byte and expiration bounds are identical to comment drafts", () => {
  const storage = new Storage(); save(storage);
  assert.equal(read(storage, scope, now + 30 * 86400000 + 1), null);
  const original = storage.getItem(KEY);
  assert.throws(() => save(storage, scope, { ...input, participants: "é".repeat(32768) }));
  assert.equal(storage.getItem(KEY), original);
  for (let version = 1; version < 20; version++) save(storage, { ...scope, reviewId: "review:one", expectedVersion: version });
  const full = storage.getItem(KEY);
  assert.throws(() => save(storage, { ...scope, reviewId: "review:one", expectedVersion: 20 }));
  assert.equal(storage.getItem(KEY), full);
  const malformed = JSON.parse(full); malformed.entries[0].draft.token = "not allowed"; storage.setItem(KEY, JSON.stringify(malformed));
  const corrupt = storage.getItem(KEY); assert.throws(() => save(storage)); assert.equal(storage.getItem(KEY), corrupt);
});

test("invalid target bindings cannot turn an update into a new review", () => {
  for (const patch of [{ reviewId: null, expectedVersion: 1 }, { reviewId: "review:one", expectedVersion: 0 }, { reviewId: "review:one", expectedVersion: 10001 }, { reviewId: "../other", expectedVersion: 1 }]) {
    assert.throws(() => save(new Storage(), { ...scope, ...patch }));
    assert.throws(() => request({ ...scope, ...patch }, input, next()));
  }
});
