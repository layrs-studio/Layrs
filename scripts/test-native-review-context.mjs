import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const ts = createRequire(path.join(root, "packages/ui/package.json"))("typescript");
const code = ts.transpileModule(fs.readFileSync(path.join(root, "packages/ui/src/native-review-context.ts"), "utf8"), { compilerOptions: { module: ts.ModuleKind.CommonJS } }).outputText;
const module = { exports: {} };
vm.runInNewContext(code, { module, exports: module.exports });
const { validReviewContext, exactReviewVersion } = module.exports;
test("an old notification selects its immutable version even after newer proposals", () => {
  const initial = { reviewId: "review:one", version: 1, proposedStepId: "step:first" };
  const latest = { ...initial, version: 2, proposedStepId: "step:second" };
  assert.equal(exactReviewVersion([initial, latest], { reviewId: "review:one", version: 1 }), initial);
  assert.equal(exactReviewVersion([initial, latest], { reviewId: "review:one", version: 2 }), latest);
});
test("missing, ambiguous or another review's version never falls back to latest", () => {
  const version = { reviewId: "review:one", version: 2 };
  assert.throws(() => exactReviewVersion([version], { ...version, version: 1 }));
  assert.throws(() => exactReviewVersion([version], { ...version, reviewId: "review:other" }));
  assert.throws(() => exactReviewVersion([version, version], version));
  assert.throws(() => exactReviewVersion([], version));
});
test("invalid notification contexts are rejected before offering navigation", () => {
  for (const value of [null, {}, { reviewId: "../escape", version: 1 }, { reviewId: "review:", version: 1 },
    { reviewId: "review:a", version: 0 }, { reviewId: "review:a", version: 1.5 }, { reviewId: "review:a", version: 10002 },
    { reviewId: "review:a", version: "1" }]) assert.equal(validReviewContext(value), false);
  assert.equal(validReviewContext({ reviewId: "review:a", version: 10001 }), true);
});
