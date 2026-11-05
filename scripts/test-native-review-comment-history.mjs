import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const ts = createRequire(path.join(root, "packages/ui/package.json"))("typescript");
const code = ts.transpileModule(fs.readFileSync(path.join(root, "packages/ui/src/native-review-comment-history.ts"), "utf8"), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
const module = { exports: {} };
vm.runInNewContext(code, { module, exports: module.exports });
const { commentHistory } = module.exports;
const original = { actionId: "review-action:one", version: 1, action: "comment", body: "Texte d’origine", actorPrincipalId: "principal:alice", createdAt: "2026-09-20T10:00:00Z", anchor: { path: "src/one.ts", startLine: 7, endLine: 7 } };
const corrected = revision => ({ evidenceObjectId: `blake3:correction${revision}`, record: {
  schemaVersion: "layrs.review-comment-correction.v1", correctionId: `correction:${revision}`, reviewId: "review:one", version: 1,
  actionId: original.actionId, revision, body: `Révision ${revision}`, reason: `Motif ${revision}`, actorPrincipalId: original.actorPrincipalId,
  correctedAt: "2026-09-20T11:00:00Z", originalActionObjectId: "blake3:original", previousCorrectionObjectId: revision === 1 ? null : `blake3:correction${revision - 1}`,
} });
test("current text follows the revision chain while original, author, dates and anchor survive", () => {
  const corrections = [corrected(2), corrected(1)];
  const before = JSON.stringify({ original, corrections });
  const history = commentHistory("review:one", original, corrections);
  assert.equal(history.body, "Révision 2"); assert.equal(history.revision, 2);
  assert.equal(history.corrections[0].record.reason, "Motif 1");
  assert.equal(JSON.stringify({ original, corrections }), before, "rendering must never overwrite original or input ordering");
});
test("another comment's correction cannot replace this one's text", () => {
  const other = corrected(1); other.record.actionId = "review-action:other";
  const history = commentHistory("review:one", original, [other]);
  assert.equal(history.body, original.body); assert.equal(history.revision, 0);
});
test("wrong version, review, author or action kind is rejected instead of silently reanchored", () => {
  for (const patch of [{ version: 2 }, { reviewId: "review:other" }, { actorPrincipalId: "principal:admin" }, { schemaVersion: "future" }]) {
    const change = corrected(1); Object.assign(change.record, patch);
    assert.throws(() => commentHistory("review:one", original, [change]));
  }
  assert.throws(() => commentHistory("review:one", { ...original, action: "approve" }, [corrected(1)]));
});
test("missing, duplicate or broken links cannot be presented as a complete audit trail", () => {
  assert.throws(() => commentHistory("review:one", original, [corrected(2)]));
  assert.throws(() => commentHistory("review:one", original, [corrected(1), corrected(1)]));
  const broken = corrected(2); broken.record.previousCorrectionObjectId = "blake3:elsewhere";
  assert.throws(() => commentHistory("review:one", original, [corrected(1), broken]));
  const otherOriginal = corrected(2); otherOriginal.record.originalActionObjectId = "blake3:elsewhere";
  assert.throws(() => commentHistory("review:one", original, [corrected(1), otherOriginal]));
});
test("rendering refuses more than the admitted 100 revisions per comment", () => {
  const changes = Array.from({ length: 101 }, (_, index) => corrected(index + 1));
  assert.throws(() => commentHistory("review:one", original, changes));
});
