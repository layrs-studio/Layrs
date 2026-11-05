import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import { createRequire } from "node:module";

const ts = createRequire(new URL("../../package.json", import.meta.url))("typescript");
const module = { exports: {} };
const code = ts.transpileModule(fs.readFileSync(new URL("../app/gitBridge.ts", import.meta.url), "utf8"), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
vm.runInNewContext(code, { module, exports: module.exports });
const { gitReferences, gitExportMode, gitImportRequest, gitExportRequest } = module.exports;
const plain = value => JSON.parse(JSON.stringify(value));
const target = { path: "C:/native", worktreeId: "worktree:a", operationId: "operation:a", viewId: "blake3:reviewed" };
const pin = `sha256:${"a".repeat(64)}`;
const step = `blake3:${"b".repeat(64)}`;

test("Git reference list normalization preserves explicit full names and ordering", () => {
  assert.deepEqual(plain(gitReferences(" refs/heads/main\r\n\nrefs/tags/v1 \n")), ["refs/heads/main", "refs/tags/v1"]);
  assert.deepEqual(plain(gitExportMode("selected_history", "refs/tags/v1\nrefs/heads/main", "", "")), { kind: "selected_history", references: ["refs/tags/v1", "refs/heads/main"] });
  assert.throws(() => gitExportMode("selected_history", " \n", "", ""));
});

test("snapshot export requires an exact immutable Step and explicit destination reference", () => {
  assert.deepEqual(plain(gitExportMode("snapshot", "", ` ${step} `, " refs/heads/reviewed ")), { kind: "snapshot", stepId: step, reference: "refs/heads/reviewed" });
  for (const invalid of ["@layer:main", "blake3:ab", "", "blake3:" + "B".repeat(64)]) assert.throws(() => gitExportMode("snapshot", "", invalid, "refs/heads/main"));
  assert.throws(() => gitExportMode("snapshot", "", step, " "));
  assert.deepEqual(plain(gitExportMode("history", "ignored", "", "")), { kind: "history" });
});

test("import confirmation carries the reviewed pin and rejects checkpoint or target drift", () => {
  const review = { reviewId: "review:1", review: { target, checkpoint: "C:/input.scan", digest: pin } };
  const args = ["C:/input.scan", pin, target.worktreeId, target.operationId];
  assert.deepEqual(plain(gitImportRequest(review, ...args)), { action: "applyImport", reviewId: "review:1", checkpoint: args[0], digest: pin });
  for (const [index, changed] of [[0, "C:/other.scan"], [1, `sha256:${"c".repeat(64)}`], [2, "worktree:other"], [3, "operation:later"]]) {
    const stale = [...args]; stale[index] = changed;
    assert.throws(() => gitImportRequest(review, ...stale));
  }
});

test("export confirmation binds destination, exact selection, Worktree and Operation", () => {
  const mode = { kind: "selected_history", references: ["refs/heads/main"] };
  const review = { reviewId: "review:2", review: { target, destination: "C:/out.git", mode } };
  const args = ["C:/out.git", mode, target.worktreeId, target.operationId, false];
  assert.deepEqual(plain(gitExportRequest(review, ...args)), { action: "export", reviewId: "review:2", destination: args[0], mode });
  for (const [index, changed] of [[0, "C:/other.git"], [1, { kind: "history" }], [1, { kind: "selected_history", references: ["refs/heads/other"] }], [2, "worktree:other"], [3, "operation:later"]]) {
    const stale = [...args]; stale[index] = changed;
    assert.throws(() => gitExportRequest(review, ...stale));
  }
  assert.equal(gitExportRequest(review, ...args.slice(0, 4), true).action, "resumeExport");
});

test("native snapshot review key order preserves exact export and resume confirmation", () => {
  const mode = gitExportMode("snapshot", "", step, "refs/heads/main");
  const nativeMode = { kind: "snapshot", reference: "refs/heads/main", stepId: step };
  assert.notEqual(JSON.stringify(nativeMode), JSON.stringify(mode));
  const review = { reviewId: "review:3", review: { target, destination: "C:/out.git", mode: nativeMode } };
  for (const resume of [false, true]) {
    const request = gitExportRequest(review, "C:/out.git", mode, target.worktreeId, target.operationId, resume);
    assert.equal(request.mode, mode);
    assert.equal(request.action, resume ? "resumeExport" : "export");
    for (const changed of [{ ...mode, stepId: `blake3:${"c".repeat(64)}` }, { ...mode, reference: "refs/heads/other" }, { kind: "history" }]) {
      assert.throws(() => gitExportRequest(review, "C:/out.git", changed, target.worktreeId, target.operationId, resume));
    }
  }
});

test("selected history compares ordered reference values and counts independently of object key order", () => {
  const mode = { kind: "selected_history", references: ["refs/heads/main", "refs/tags/v1"] };
  const review = { reviewId: "review:4", review: { target, destination: "C:/out.git", mode: { references: [...mode.references], kind: mode.kind } } };
  assert.equal(gitExportRequest(review, "C:/out.git", mode, target.worktreeId, target.operationId, false).mode, mode);
  for (const references of [[...mode.references].reverse(), mode.references.slice(0, 1), [...mode.references, "refs/tags/v2"], ["refs/heads/other", mode.references[1]], [mode.references[0], mode.references[0]]]) {
    assert.throws(() => gitExportRequest(review, "C:/out.git", { ...mode, references }, target.worktreeId, target.operationId, false));
  }
});

test("export confirmation refuses unknown variants and malformed or extra mode fields even when both operands match", () => {
  for (const mode of [null, [], {}, { kind: "future" }, { kind: "history", reference: "refs/heads/main" }, { kind: "selected_history" }, { kind: "selected_history", references: [] }, { kind: "selected_history", references: [null] }, { kind: "selected_history", references: [""] }, { kind: "selected_history", references: new Array(1) }, { kind: "selected_history", references: ["refs/heads/main"], stepId: step }, { kind: "snapshot", stepId: "@layer:main", reference: "refs/heads/main" }, { kind: "snapshot", stepId: step }, { kind: "snapshot", stepId: step, reference: "" }, { kind: "snapshot", stepId: step, reference: "refs/heads/main", references: [] }]) {
    const review = { reviewId: "review:5", review: { target, destination: "C:/out.git", mode } };
    assert.throws(() => gitExportRequest(review, "C:/out.git", mode, target.worktreeId, target.operationId, false));
  }
  assert.throws(() => gitExportMode("future", "", step, "refs/heads/main"));
});
