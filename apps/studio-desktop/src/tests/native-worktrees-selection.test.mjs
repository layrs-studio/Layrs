import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import { createRequire } from "node:module";
import { loadUiTypeScript } from "../../../../scripts/lib/load-ui-typescript.mjs";

const ts = createRequire(new URL("../../package.json", import.meta.url))("typescript");
const selection = loadUiTypeScript("native-projection-selection.ts");
const files = loadUiTypeScript("native-projection-store-model.ts");
const namespace = loadUiTypeScript("native-project-files-model.ts");
function load(name) {
  const module = { exports: {} };
  const code = ts.transpileModule(fs.readFileSync(new URL(`../app/${name}.ts`, import.meta.url), "utf8"), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
  vm.runInNewContext(code, { module, exports: module.exports, require: name => {
    if (name === "@layrs/ui") return selection;
    throw new Error(`Unexpected test import: ${name}`);
  } });
  return module.exports;
}
const { createWorktreeRequest, currentWorktree, worktreeDestination } = load("worktrees");
const { selectionFromRules, cloneSelectedRequest, expandSelectionRequest } = load("selection");
const plain = value => JSON.parse(JSON.stringify(value));
const hash = char => `blake3:${char.repeat(64)}`;
const status = () => ({
  identity: { storeId: "store:a", worktreeId: "worktree:a", spaceId: "space:a", origin: "https://example.test", principalId: "principal:a" },
  projection: { generation: 4, digest: "a".repeat(64), snapshot: {
    schemaVersion: "layrs.projection-snapshot.v1", baseStepId: hash("1"), baseRootTreeId: hash("2"), observedPolicySetId: hash("3"), observedAuthorizationEpoch: 1, pathSemantics: "portable_windows",
    entries: [
      { path: "chosen.txt", target: { kind: "file", fileId: hash("4") }, executable: false, writable: true, state: "materialized" },
      { path: "excluded.txt", target: { kind: "file", fileId: hash("5") }, executable: false, writable: true, state: "unavailable" },
      { path: "secret.txt", target: { kind: "file", fileId: hash("6") }, executable: false, writable: false, state: "reserved" },
      { path: "unsupported", target: { kind: "symlink", symlinkId: hash("7") }, executable: false, writable: true, state: "unavailable" },
    ],
  } },
  selection: { kind: "selected", paths: ["chosen.txt", "unsupported"], subtrees: [] },
  needsRepair: false, capabilities: { capture: true, namespace: true, publish: true },
});

test("create pins an explicit Step and Layer instead of a moving reference", () => {
  assert.deepEqual(plain(createWorktreeRequest("C:/second", "layer:topic", hash("a"))), { action: "createWorktree", path: "C:/second", layer: "layer:topic", step: hash("a") });
  for (const args of [["", "layer:a", hash("a")], ["C:/second", "", hash("a")], ["C:/second", "layer:a", "@layer:main"], ["C:/second", "layer:a", "blake3:a"]]) assert.throws(() => createWorktreeRequest(...args));
});

test("opening uses the target Worktree's own View and sends unfinished creation to recovery", () => {
  const state = { worktree: { worktreeId: "worktree:second", checkoutRoot: "C:/second" }, view: { worktrees: [
    { worktreeId: "worktree:first", status: "ready", baseStepId: hash("a") },
    { worktreeId: "worktree:second", status: "stale", baseStepId: hash("b") },
  ] } };
  assert.equal(currentWorktree(state).baseStepId, hash("b"));
  assert.deepEqual(plain(worktreeDestination({ path: "C:/second", worktreeId: "worktree:second", state })), { path: "C:/second", view: "health" });
  state.view.worktrees[1].status = "ready";
  assert.equal(worktreeDestination({ path: "C:/second", worktreeId: "worktree:second", state }).view, "changes");
  assert.throws(() => worktreeDestination({ path: "C:/wrong", worktreeId: "worktree:second", state }));
  assert.throws(() => worktreeDestination({ path: "C:/second", worktreeId: "worktree:first", state }));
});

test("selection form emits literal exact paths and segment-bounded subtrees", () => {
  const selected = selectionFromRules([{ id: 1, kind: "path", path: "README.md" }, { id: 2, kind: "subtree", path: "src" }], "portable_windows");
  assert.deepEqual(plain(cloneSelectedRequest("space:a", hash("a"), selected)), { action: "cloneProjectionSelected", space: "space:a", step: hash("a"), selection: { kind: "selected", paths: ["README.md"], subtrees: ["src"] } });
  assert.equal(selection.projectionSelectionIncludes(selected, "src/main.ts"), true);
  assert.equal(selection.projectionSelectionIncludes(selected, "src-other/main.ts"), false);
  assert.equal(selection.projectionSelectionIncludes(selected, "README.md/child"), false);
});

test("malformed, glob, traversal and colliding path payloads are refused", () => {
  for (const path of ["", "../secret", "/absolute", "src\\file", ".layrs/config", "src/**", "src/[a]", "aux.txt"]) {
    assert.throws(() => selectionFromRules([{ id: 1, kind: "path", path }], "portable_windows"));
  }
  assert.throws(() => selectionFromRules([], "portable_windows"));
  assert.throws(() => selection.selectedProjectionPaths(["src/a", "SRC/b"], [], "portable_windows"));
  for (const invalid of [null, [], { kind: "future" }, { kind: "all", paths: [] }, { kind: "selected", paths: "a", subtrees: [] }]) assert.throws(() => selection.validateProjectionSelection(invalid, "portable_windows"));
  assert.throws(() => cloneSelectedRequest("space:a", "@layer:main", { kind: "all" }));
});

test("excluded, permission reserved and unsupported entries stay distinct and fenced", () => {
  const result = files.projectionStoreFiles(status());
  assert.equal(namespace.nativeProjectionReady(result), true);
  assert.deepEqual(plain(result.entries.map(entry => entry.contentState)), ["materialized", "excluded", "reserved", "not_materialized"]);
  assert.equal(result.entries[1].access, "read_write");
  assert.equal(result.entries[2].access, "denied");
  assert.equal(result.entries[2].selection, "excluded");
  for (const path of ["excluded.txt", "secret.txt", "unsupported"]) {
    assert.equal(namespace.checkNativeNamespaceMutation(result, { kind: "create", destination: path }).code, "reserved_path");
    assert.equal(namespace.checkNativeNamespaceMutation(result, { kind: "move", source: path, destination: "new.txt" }).allowed, false);
  }
  assert.match(namespace.nativeProjectFileLabel(result.entries[1]), /Exclu/);
  assert.match(namespace.nativeProjectFileLabel(result.entries[2]), /interdit.*hors sélection/);
  assert.match(namespace.nativeProjectFileLabel(result.entries[3]), /indisponible/);
  assert.equal(result.entries.some(entry => entry.kind === "absent" || entry.kind === "tombstone"), false);
});

test("exclusion cannot hide contradictory absence, materialization or an unknown engine state", () => {
  for (const state of ["materialized", "absent"]) {
    const source = status(); source.projection.snapshot.entries[1].state = state;
    if (state === "absent") source.projection.snapshot.entries[1].target = null;
    assert.throws(() => files.projectionStoreFiles(source));
  }
  const source = status(); source.projection.snapshot.entries[1].state = "future";
  assert.equal(files.projectionStoreFiles(source).state, "incomplete");
});

test("expansion sends additions at the reviewed generation without duplicating persisted rules", () => {
  const source = status(); source.selection.subtrees = ["docs"];
  const request = expandSelectionRequest(source, 4, { kind: "selected", paths: ["excluded.txt"], subtrees: ["src"] });
  assert.deepEqual(plain(request), { action: "expandProjectionSelection", expectedGeneration: 4, selection: { kind: "selected", paths: ["excluded.txt"], subtrees: ["src"] } });
  assert.deepEqual(source.selection.subtrees, ["docs"]);
  for (const generation of [3, 5, -1, NaN]) assert.throws(() => expandSelectionRequest(source, generation, { kind: "all" }));
  source.pendingPublication = { sequence: 1 };
  assert.throws(() => expandSelectionRequest(source, 4, { kind: "all" }));
});
