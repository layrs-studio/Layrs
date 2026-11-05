import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, readdir, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import {
  AUDIT_TABLES, auditExpectation, canRetryCapture, logicalBytes, payload, relativeFile,
  validateAudit, validateDiff, validateHistory,
} from "./measure-worktrees.mjs";

const runner = fileURLToPath(new URL("./measure-worktrees.mjs", import.meta.url));
const helper = fileURLToPath(new URL("./measure-worktree-process.ps1", import.meta.url));
const repository = fileURLToPath(new URL("../", import.meta.url));
const clone = value => structuredClone(value);
// All process fixtures are fresh and retained. No Layrs binary or existing Store is used.
const fresh = name => mkdtemp(join(tmpdir(), `layrs-worktrees-test-${name}-`));

const captureFailure = error => ({ error, success: false, exitCode: 1, timedOut: false,
  outputLimitExceeded: false, invalidJson: false, helperError: null, cancelled: false });
const retryPolicy = { retry: true, attempt: 0, casRetries: 2, stopped: false };
const captureMessages = [
  ["CAS mismatch", "store backend failed: compare-and-swap mismatch"],
  ["stale capture", "Worktree or repository changed during capture; read a new base and recapture"],
];
for (const [name, message] of captureMessages) {
  for (const structured of [false, true]) test(`capture retry policy accepts ${name}, ${structured ? "JSON error" : "string error"}`, () => {
    const record = captureFailure(structured ? { message } : message), before = clone(record);
    assert.equal(canRetryCapture(record, retryPolicy), true);
    assert.deepEqual(record, before);
  });
}
for (const [name, error] of [
  ["LockBusy code", { code: "LockBusy" }],
  ["Store lock busy message", { message: "store backend failed: native Store lock failed: store lock is busy: C:\\fixture\\.layrs\\metadata\\layrs.lock" }],
  ["database busy", "database is locked"], ["unknown failure", { message: "permission denied" }],
  ["absent error", undefined], ["null error", null], ["empty error", {}],
]) test(`capture retry policy rejects ${name}`, () => assert.equal(canRetryCapture(captureFailure(error), retryPolicy), false));

for (const [name, fields, policy] of [
  ["retry-disabled phase", {}, { retry: false }], ["stopped run", {}, { stopped: true }],
  ["no exit code", { exitCode: null }, {}], ["timeout", { timedOut: true }, {}],
  ["output limit", { outputLimitExceeded: true }, {}], ["invalid JSON", { invalidJson: true }, {}],
  ["helper failure", { helperError: "launcher failed" }, {}], ["cancellation", { cancelled: true }, {}],
]) test(`capture retry policy rejects ${name} even with a known CAS/stale error`, () => {
  for (const [, message] of captureMessages) assert.equal(canRetryCapture(
    { ...captureFailure({ message }), ...fields }, { ...retryPolicy, ...policy }), false);
});

for (const casRetries of [0, 1, 2, 3, 4, 5]) test(`capture retry policy exhausts exactly ${casRetries} additional attempts`, () => {
  for (const [, message] of captureMessages) {
    const record = captureFailure({ message });
    // attempt 0 is the initial call. The attempt at the configured retry count must stop.
    const decisions = Array.from({ length: 8 }, (_, attempt) => canRetryCapture(record, { ...retryPolicy, attempt, casRetries }));
    assert.deepEqual(decisions, [...Array(casRetries).fill(true), ...Array(8 - casRetries).fill(false)]);
  }
});

test("import and help have no fixture/process-launch side effects", async () => {
  const cwd = await fresh("import");
  const imported = spawnSync(process.execPath, ["--input-type=module", "-e",
    `await import(${JSON.stringify(new URL("./measure-worktrees.mjs", import.meta.url).href)});`],
  { cwd, encoding: "utf8", windowsHide: true, timeout: 10000 });
  assert.equal(imported.status, 0, imported.stderr);
  assert.equal(imported.stdout, "");
  const help = spawnSync(process.execPath, [runner, "--help"], { cwd, encoding: "utf8", windowsHide: true, timeout: 10000 });
  assert.equal(help.status, 0, help.stderr);
  assert.match(help.stdout, /--audit-binary/);
  assert.match(help.stdout, /--audit-sha256 HEX\s+Required common auditor hash/);
  assert.deepEqual(await readdir(cwd), []);
});

test("payloads are deterministic, unique across files/versions, correctly sized and bounded", () => {
  for (const shape of ["code", "assets", "mixed"]) {
    const unique = new Set(); let bytes = 0;
    for (let index = 0; index < 32; index++) {
      const base = payload(shape, index);
      assert.deepEqual(base, payload(shape, index));
      bytes += base.length;
      for (const variant of ["base", "agent-0-round-0", "agent-1-round-0", "agent-0-round-1"]) {
        const value = payload(shape, index, variant);
        assert.equal(value.length, base.length);
        assert(!unique.has(value.toString("hex"))); unique.add(value.toString("hex"));
      }
    }
    assert.equal(bytes, logicalBytes(shape, 32));
  }
  assert.equal(logicalBytes("mixed", 3), 5120);
  assert.equal(relativeFile("mixed", 999), "mixed/d000/f000999.bin");
  assert.equal(relativeFile("mixed", 1000), "mixed/d001/f001000.mjs");
  assert.equal(99000 + 1 + Math.ceil(99000 / 1000) + 1, 99101);
});

const edit = { path: "mixed/d000/f000000.mjs", bytes: 512 };
const change = { path: edit.path, status: "modified", beforeBytes: 512, afterBytes: 512,
  beforeDigest: "old", afterDigest: "new", executableChanged: false };
test("diff accepts only exact clean/dirty changes and does not mutate input", () => {
  assert.deepEqual(validateDiff({ changes: [] }, []), []);
  const data = { changes: [clone(change)] }, before = clone(data);
  assert.equal(validateDiff(data, [edit]).length, 1); assert.deepEqual(data, before);
});
for (const [name, data] of [
  ["fixture-only response", { fixtureOnly: true }], ["extra response field", { changes: [], fixtureOnly: true }],
  ["missing changes", {}], ["nonarray changes", { changes: null }], ["extra clean path", { changes: [change] }],
]) test(`diff rejects ${name}`, () => assert.throws(() => validateDiff(data, [])));
for (const [name, changes] of [
  ["missing dirty path", []], ["extra dirty path", [change, { ...change, path: "other" }]],
  ["wrong path", [{ ...change, path: "other" }]], ["wrong before bytes", [{ ...change, beforeBytes: 0 }]],
  ["wrong after bytes", [{ ...change, afterBytes: 513 }]], ["wrong status", [{ ...change, status: "added" }]],
  ["same digest", [{ ...change, afterDigest: "old" }]], ["missing digest", [{ ...change, beforeDigest: null }]],
  ["marked change", [{ ...change, fixtureOnly: true }]], ["mode change", [{ ...change, executableChanged: true }]],
]) test(`diff rejects ${name}`, () => assert.throws(() => validateDiff({ changes }, [edit])));

const primaryKinds = ["capture_worktree", "capture_worktree", "initialize"];
const secondaryKinds = ["capture_worktree", "capture_worktree", "restore", "restore", "initialize"];
function history(kinds) {
  return { operations: kinds.map((kind, index) => ({ schemaVersion: "layrs.operation.v1",
    operationId: `operation:${index}`, spaceId: "space:test", resultingViewId: `blake3:${"a".repeat(64)}`,
    parentOperationIds: index + 1 < kinds.length ? [`operation:${index + 1}`] : [], kind })) };
}
const primary = history(primaryKinds);
const checkHistory = (data, kinds = primaryKinds) => validateHistory(data, kinds,
  ["operation:0", "operation:1", "operation:2"], `operation:${kinds.length - 1}`, "space:test");
test("three/five-Operation schemas remain explicitly unequal work (no shared CLI execution)", () => {
  for (const kinds of [primaryKinds, secondaryKinds]) {
    const data = history(kinds), before = clone(data), result = checkHistory(data, kinds);
    assert.equal(result.operationCount, kinds.length);
    assert.equal(result.equalWorkComparisonEligible, false);
    assert.deepEqual(data, before);
  }
});
for (const [name, mutate] of Object.entries({
  "fixture-only history": x => { delete x.operations; x.fixtureOnly = true; },
  "missing operations": x => { delete x.operations; }, "empty history": x => { x.operations = []; },
  "extra lifecycle": x => { x.operations = history(secondaryKinds).operations; },
  "repeated Operation": x => { x.operations[1] = x.operations[0]; },
  "missing fields": x => { x.operations = x.operations.map(({ operationId }) => ({ operationId })); },
  "wrong parent": x => { x.operations[0].parentOperationIds = ["operation:2"]; },
  "wrong kind": x => { x.operations[1].kind = "restore"; },
  "wrong Space": x => { x.operations[0].spaceId = "space:other"; },
  "wrong view ID": x => { x.operations[0].resultingViewId = "fixtureOnly"; },
})) test(`history rejects ${name}`, () => { const data = clone(primary); mutate(data); assert.throws(() => checkHistory(data)); });
test("history rejects wrong observed capture and initialization identities", () => {
  assert.throws(() => validateHistory(primary, primaryKinds, ["operation:other"], "operation:2", "space:test"));
  assert.throws(() => validateHistory(primary, primaryKinds, ["operation:0"], "operation:other", "space:test"));
});

function scenario(shared, shape = "mixed") {
  const value = { logicalFiles: 32, logicalBytesPerCheckout: logicalBytes(shape, 32),
    roots: [0, 1].map(agent => resolve(`fake-only/${shared ? "shared" : "independent"}/agent-${agent}`)),
    initialStates: [0, 1].map(agent => ({ storeId: shared ? "store:shared" : `store:${agent}`,
      worktreeId: `worktree:${agent}`, spaceId: shared ? "space:shared" : `space:${agent}`,
      stepId: shared ? "step:base" : `step:base${agent}` })), histories: [], privateByteChecks: [] };
  for (const agent of [0, 1]) {
    const ids = [`operation:${agent}r1`, `operation:${agent}r0`,
      ...(shared && agent === 1 ? ["operation:restore", "operation:register"] : []),
      shared ? "operation:init" : `operation:init${agent}`];
    const item = { agent, operationIds: ids, parentOperationIds: ids.map((_, index) => index + 1 < ids.length ? [ids[index + 1]] : []) };
    value.histories.push(item, clone(item)); // Repeat timeline reads must not double-count history.
    for (const invocation of [0, 1]) for (const index of shape === "mixed" ? [0, 1] : [0]) {
      value.privateByteChecks.push({ agent, invocation, path: relativeFile(shape, index), capturedStepId: `step:${agent}r${invocation}` });
    }
  }
  return value;
}
function audit(expected) {
  const tableRows = Object.fromEntries([...AUDIT_TABLES, "spaces"].map(table => [table, 0]));
  Object.assign(tableRows, { objects: 100, file_manifests: expected.fileManifests, file_manifest_chunks: expected.fileManifests,
    operations: expected.operationIds.length, steps: expected.stepIds.length, worktrees: expected.worktreeIds.length,
    spaces: expected.spaceIds.length, operation_parents: expected.operationParents.length, step_parents: expected.stepIds.length - 1 });
  const objectBytes = 1024 + expected.fileBytes;
  return { schemaVersion: "layrs.worktree-audit.v4", readOnly: true, formatVersion: 2,
    storeRoot: expected.storeRoot, storeId: expected.storeId, clean: true, complete: true, truncatedCount: 0,
    budgets: { maxFindings: 15625, workRows: 1000000, historyRowsPerTable: 256, canonicalMetadataBytes: 268435456,
      chunkBytes: 4294967296, fileBytes: 4294967296, traversalWorkUnits: 4000000 },
    preflight: { complete: true, observedCountsWithinBudgets: true, tableRows,
      workRows: AUDIT_TABLES.reduce((n, table) => n + tableRows[table], 0),
      ...Object.fromEntries(["stepIds", "operationIds", "worktreeIds", "spaceIds", "operationParents"].map(field => [field, clone(expected[field])])),
      canonicalObjectBytes: 1024, chunkBytes: expected.fileBytes, fileBytes: expected.fileBytes, uniqueChunks: expected.fileManifests },
    checkoutBindings: expected.checkoutRoots.map(checkoutRoot => ({ checkoutRoot, storeRoot: expected.storeRoot, validatedBeforeAndAfter: true })),
    physicalStore: { complete: true, reconciled: true, entryLimit: 2000000, depthLimit: 16, visitedEntries: 110,
      indexedExpected: { files: 100, bytes: objectBytes }, objects: { files: 100, bytes: objectBytes },
      metadata: { files: 1, bytes: 11, samplePaths: [] }, other: { files: 1, bytes: 2, samplePaths: ["FORMAT"] },
      repairBackups: { files: 0, bytes: 0, dirs: 0, samplePaths: [] }, total: { files: 103, bytes: objectBytes + 13 },
      staging: { files: 1, bytes: 0, leaseFiles: 1, leaseBytes: 0, pendingEntries: 0, samplePaths: [] } },
    checkedObjects: 100, checkedRows: 200, retentionRoots: 1, reachableObjects: 100,
    unreachableObjects: 0, reconstructibleProjections: 0, findings: [] };
}
const sharedScenario = scenario(true);
const shared = auditExpectation(sharedScenario, [0, 1], join(sharedScenario.roots[0], ".layrs"));
test("audit fixture expectations deduplicate history and count every unique mixed edit", () => {
  assert.equal(shared.operationIds.length, 7); assert.equal(shared.operationParents.length, 6);
  assert.equal(shared.stepIds.length, 5); assert.equal(shared.fileManifests, 40); assert.equal(shared.fileBytes, 92160);
  for (const sharing of [false, true]) for (const shape of ["code", "assets", "mixed"]) {
    const fixture = scenario(sharing, shape);
    for (const agents of sharing ? [[0, 1]] : [[0], [1]]) {
      const expected = auditExpectation(fixture, agents, join(fixture.roots[agents[0]], ".layrs"));
      const data = audit(expected), before = clone(data);
      validateAudit(data, expected); assert.deepEqual(data, before);
      if (!sharing) assert.equal(expected.operationIds.length, 3);
    }
  }
});
test("runner, example and current fsck have identical work-table sets", async () => {
  const rust = await readFile(join(repository, "crates/layrs-store-local/examples/measure_worktree_audit.rs"), "utf8");
  const fsck = await readFile(join(repository, "crates/layrs-store-local/src/scm_v1/fsck.rs"), "utf8");
  const rustTables = [...rust.match(/const WORK_TABLES:[\s\S]*?= &\[([\s\S]*?)\];/)[1].matchAll(/"([a-z_]+)"/g)].map(x => x[1]);
  const fsckTables = [...fsck.match(/const WORK_ROWS: &str = "([\s\S]*?)";/)[1].matchAll(/FROM ([a-z_]+)/g)].map(x => x[1]);
  assert.deepEqual([...AUDIT_TABLES].sort(), rustTables.sort());
  assert.deepEqual([...AUDIT_TABLES].sort(), fsckTables.sort());
});
test("audit rejects missing data", () => assert.throws(() => validateAudit(undefined, shared)));
test("repair-backup payloads contribute exactly once to Store totals", () => {
  const data = audit(shared);
  data.physicalStore.repairBackups = { files: 2, bytes: 42, dirs: 2, samplePaths: [
    "metadata/repair-backups", "metadata/repair-backups/displaced", "metadata/repair-backups/displaced/a", "metadata/repair-backups/displaced/b",
  ] };
  data.physicalStore.total.files += 2; data.physicalStore.total.bytes += 42;
  data.physicalStore.visitedEntries += 4;
  validateAudit(data, shared);
  assert.equal(data.physicalStore.metadata.bytes, 11);
  const doubled = clone(data); doubled.physicalStore.metadata.bytes += 42;
  assert.throws(() => validateAudit(doubled, shared), /total bytes/);
  const omitted = clone(data); omitted.physicalStore.total.bytes -= 42;
  assert.throws(() => validateAudit(omitted, shared), /total bytes/);
});
for (const [name, mutate] of Object.entries({
  "fixture-only marker": x => { x.fixtureOnly = true; }, "wrong schema": x => { x.schemaVersion = "fake"; },
  "mutable audit": x => { x.readOnly = false; }, "wrong format": x => { x.formatVersion = 1; },
  "wrong Store": x => { x.storeId = "store:other"; }, "wrong root": x => { x.storeRoot = resolve("wrong"); },
  "unclean result": x => { x.clean = false; }, "incomplete result": x => { x.complete = false; },
  "truncated result": x => { x.truncatedCount = 1; }, "truncated output": x => { x.outputTruncated = true; },
  "wrong fixed budget": x => { x.budgets.maxFindings = 1024; }, "incomplete preflight": x => { x.preflight.complete = false; },
  "overflow preflight": x => { x.preflight.observedCountsWithinBudgets = false; },
  "ambiguous old preflight flag": x => { delete x.preflight.observedCountsWithinBudgets; x.preflight.withinBounds = true; },
  "missing table": x => { delete x.preflight.tableRows.layers; }, "extra table": x => { x.preflight.tableRows.fake = 0; },
  "negative rows": x => { x.preflight.tableRows.layers = -1; }, "mismatched rows": x => { x.preflight.workRows++; },
  "actual row overflow": x => { x.preflight.tableRows.objects += 1000000; x.preflight.workRows += 1000000; },
  "missing Step": x => { x.preflight.stepIds.pop(); }, "unexpected Operation": x => { x.preflight.operationIds[0] = "operation:extra"; },
  "duplicate Worktree": x => { x.preflight.worktreeIds[0] = x.preflight.worktreeIds[1]; },
  "wrong Space": x => { x.preflight.spaceIds = ["space:other"]; },
  "wrong parent": x => { x.preflight.operationParents[0][1] = "operation:other"; },
  "history overflow": x => { x.preflight.workRows += 257 - x.preflight.tableRows.step_parents; x.preflight.tableRows.step_parents = 257; },
  "missing manifest": x => { x.preflight.tableRows.file_manifests--; x.preflight.workRows--; },
  "missing manifest chunk": x => { x.preflight.tableRows.file_manifest_chunks--; x.preflight.workRows--; },
  "retained bytes mismatch": x => { x.preflight.fileBytes++; }, "extra chunk bytes": x => { x.preflight.chunkBytes++; },
  "missing chunk bytes": x => { delete x.preflight.chunkBytes; }, "duplicate chunk": x => { x.preflight.uniqueChunks--; },
  "metadata byte overflow": x => { x.preflight.canonicalObjectBytes = 268435457; },
  "missing checked rows": x => { delete x.checkedRows; }, "zero objects": x => { x.checkedObjects = 0; },
  "missing findings": x => { delete x.findings; }, "error finding": x => { x.findings = [{ severity: "error" }]; },
  "unknown severity": x => { x.findings = [{ severity: "unknown" }]; }, "unreachable object": x => { x.unreachableObjects = 1; },
  "missing checkout bindings": x => { delete x.checkoutBindings; },
  "missing secondary binding": x => { x.checkoutBindings.pop(); },
  "unvalidated binding": x => { x.checkoutBindings[0].validatedBeforeAndAfter = false; },
  "wrong checkout": x => { x.checkoutBindings[0].checkoutRoot = resolve("elsewhere"); },
  "wrong bound Store": x => { x.checkoutBindings[0].storeRoot = resolve("elsewhere"); },
  "missing physical accounting": x => { delete x.physicalStore; },
  "incomplete physical enumeration": x => { x.physicalStore.complete = false; },
  "unreconciled physical counts": x => { x.physicalStore.reconciled = false; },
  "retained physical error": x => { x.physicalStore.error = "reparse entry retained"; },
  "physical entry overflow": x => { x.physicalStore.visitedEntries = 2000001; },
  "changed physical budget": x => { x.physicalStore.depthLimit = 100; },
  "extra unindexed object file": x => { x.physicalStore.objects.files++; x.physicalStore.total.files++; },
  "physical CAS bytes mismatch": x => { x.physicalStore.objects.bytes++; x.physicalStore.total.bytes++; },
  "forged indexed expectation": x => { x.physicalStore.indexedExpected.files++; },
  "missing metadata accounting": x => { delete x.physicalStore.metadata; },
  "missing repair-backup accounting": x => { delete x.physicalStore.repairBackups; },
  "negative repair-backup directories": x => { x.physicalStore.repairBackups.dirs = -1; },
  "missing repair-backup samples": x => { delete x.physicalStore.repairBackups.samplePaths; },
  "incomplete repair-backup samples": x => { x.physicalStore.repairBackups.dirs = 1; },
  "excess repair-backup samples": x => { x.physicalStore.repairBackups.samplePaths = Array(9).fill("metadata/repair-backups"); },
  "missing non-database metadata samples": x => { delete x.physicalStore.metadata.samplePaths; },
  "excess non-database metadata samples": x => { x.physicalStore.metadata.samplePaths = Array(9).fill("metadata/control"); },
  "excess other samples": x => { x.physicalStore.other.samplePaths = Array(9).fill("FORMAT"); },
  "invalid other sample": x => { x.physicalStore.other.samplePaths = [null]; },
  "negative other bytes": x => { x.physicalStore.other.bytes = -1; },
  "unreconciled total files": x => { x.physicalStore.total.files++; },
  "unreconciled total bytes": x => { x.physicalStore.total.bytes++; },
  "empty staging residue file": x => { x.physicalStore.staging.files++; x.physicalStore.total.files++; },
  "staging residue directory": x => { x.physicalStore.staging.pendingEntries++; },
  "staging residue evidence": x => { x.physicalStore.staging.samplePaths.push("staging/unfinished"); },
  "nonempty staging lease": x => { x.physicalStore.staging.leaseBytes = 1; x.physicalStore.staging.bytes = 1; x.physicalStore.total.bytes++; },
  "missing staging lease": x => { x.physicalStore.staging.leaseFiles = 0; },
})) test(`audit rejects ${name}`, () => { const data = audit(shared); mutate(data); assert.throws(() => validateAudit(data, shared)); });

test("missing auditor/hash errors persist failed reports before any fixture CLI launch", async t => {
  const parent = await fresh("failures"); t.diagnostic(`Retained reports: ${parent}`);
  const cases = [
    ["missing option", [], /audit-binary/],
    ["missing required hash", ["--audit-binary", process.execPath, "--audit-profile", "fake-only"], /--audit-sha256.*required/],
    ["missing file", ["--audit-binary", join(parent, "missing.exe"), "--audit-profile", "fake-only", "--audit-sha256", "0".repeat(64)], /ENOENT/],
    ["malformed hash", ["--audit-binary", process.execPath, "--audit-profile", "fake-only", "--audit-sha256", "wrong"], /64 hex/],
    ["mismatched hash", ["--audit-binary", process.execPath, "--audit-profile", "fake-only", "--audit-sha256", "0".repeat(64)], /pinned cohort hash/],
  ];
  for (const [name, extra, error] of cases) await t.test(name, async () => {
    const result = spawnSync(process.execPath, [runner, "--binary", process.execPath, "--profile", "fake-only",
      "--mode", "independent-copies", "--counts", "10000", "--output-parent", parent, ...extra],
    { encoding: "utf8", windowsHide: true, shell: false, timeout: 20000 });
    assert.equal(result.status, 1, result.stderr);
    const path = result.stdout.match(/Report: (.+)\r?\n/)[1].trim();
    const report = JSON.parse(await readFile(path, "utf8"));
    assert.equal(report.status, "failed"); assert.equal(report.success, false);
    assert.deepEqual(report.cases, []); assert.match(report.error, error);
  });
});

test("Windows owned-process helper: quoting, real counters and bounded failure paths", {
  skip: process.platform !== "win32" ? "Windows handle instrumentation is not available on this OS" : false,
  timeout: 90000,
}, async t => {
  const base = await fresh("process"); t.diagnostic(`Retained fake-process evidence: ${base}`);
  const fake = join(base, "fake cli unicode-\u00e9.mjs");
  await writeFile(fake, `import fs from 'node:fs';
const [mode,...args]=process.argv.slice(2);
if(mode==='work') { const b=Buffer.alloc(8*1024*1024,37); fs.writeFileSync('owned.bin',b,{flag:'wx'}); fs.readFileSync('owned.bin'); const stop=Date.now()+200; while(Date.now()<stop) Math.sqrt(Math.random()); console.log(JSON.stringify({argv:args})); }
if(mode==='flood') { const block=Buffer.alloc(8192,120); for(let i=0;i<100;i++){fs.writeSync(1,block);fs.writeSync(2,block);} }
if(mode==='sleep') { fs.writeFileSync(args[0],String(process.pid),{flag:'wx'}); setTimeout(()=>{},15000); }
if(mode==='exit') process.exit(Number(args[0]));
`, { flag: "wx" });
  const launcher = join(process.env.SystemRoot || "C:\\Windows", "System32/WindowsPowerShell/v1.0/powershell.exe");
  let sequence = 0;
  async function probe(mode, args = [], { limit = 16777216, timeout = 15000, cancel = false,
    acknowledge = true, binary = process.execPath } = {}) {
    const id = ++sequence;
    const request = { binary, cwd: base, argv: [fake, mode, ...args], stdoutPath: join(base, `${id}.out`),
      stderrPath: join(base, `${id}.err`), stdoutLimit: limit, stderrLimit: Math.min(limit, 1048576), deadlineUnixMs: Date.now() + timeout };
    const child = spawn(launcher, ["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", helper],
      { windowsHide: true, shell: false, stdio: ["pipe", "pipe", "pipe"] });
    let out = "", err = "", ready = false, granted = false, protocolError = null;
    const stop = () => { child.stdin.end(); if (!granted) child.kill(); };
    child.stdin.on("error", () => {});
    child.stdout.on("data", bytes => {
      out += bytes.toString("utf8");
      if (out.length > 65536) { protocolError = "helper stdout cap"; stop(); return; }
      if (!ready && out.includes("\n")) {
        try {
          ready = JSON.parse(out.split("\n")[0]).ready === true;
          if (ready && acknowledge) { granted = true; child.stdin.write("start\n"); }
          else child.stdin.end();
        } catch (error) { protocolError = error.message; stop(); }
      }
    });
    child.stderr.on("data", bytes => { err += bytes.toString("utf8"); if (err.length > 65536) { protocolError = "helper stderr cap"; stop(); } });
    const guard = setTimeout(stop, timeout + 5000);
    let poll;
    if (cancel) poll = setInterval(async () => {
      try { await stat(args[0]); child.stdin.end(); clearInterval(poll); } catch (error) { if (error.code !== "ENOENT") stop(); }
    }, 20);
    const completion = new Promise((done, fail) => { child.on("error", fail); child.on("close", done); });
    child.stdin.write(JSON.stringify(request) + "\n");
    let exit;
    try { exit = await completion; } finally { clearTimeout(guard); clearInterval(poll); }
    assert.equal(protocolError, null);
    const data = JSON.parse(out.trim().split(/\r?\n/).at(-1));
    await writeFile(join(base, `${id}.summary.json`), JSON.stringify({ exit, data, launcherStderr: err }, null, 2), { flag: "wx" });
    return { exit, data, request };
  }
  await t.test("CRT argv round trip and measured CPU/memory/I/O", async () => {
    const argv = ["", "space value", 'quote"inside', "slash\\", "two\\\\", 'before\\"quote', "line\nbreak", "tab\there", "\u00e9\u6f22\u5b57", "$(not-a-command); & |"];
    const { exit, data, request } = await probe("work", argv);
    assert.equal(exit, 0); assert.equal(data.exitCode, 0); assert.equal(data.helperError, undefined);
    assert.deepEqual(JSON.parse(await readFile(request.stdoutPath, "utf8")).argv, argv);
    for (const [counter, fields] of [[data.cpu, ["totalMs"]], [data.memory, ["peakWorkingSetBytes", "peakPrivateCommitBytes"]], [data.io, ["readBytes", "writeBytes"]]]) {
      for (const field of fields) if (counter[field] === null) assert.equal(typeof counter.unavailableReason, "string");
      else assert(counter[field] > 0, `${field} must reflect the fake child's bounded real work`);
    }
    if (data.nativeElapsedWallMs !== null) assert(data.nativeElapsedWallMs > 0);
  });
  await t.test("stdout/stderr caps", async () => {
    const { data, request } = await probe("flood", [], { limit: 4096 });
    assert.equal(data.outputLimitExceeded, true);
    assert((await stat(request.stdoutPath)).size <= 4096); assert((await stat(request.stderrPath)).size <= 4096);
  });
  await t.test("deadline terminates the owned child", async () => {
    const { data } = await probe("sleep", [join(base, "timeout.pid")], { timeout: 6000 });
    assert.equal(data.timedOut, true); assert.equal(data.launched, true); assert.notEqual(data.exitCode, 0);
  });
  await t.test("parent EOF cancels the owned child", async () => {
    const { data } = await probe("sleep", [join(base, "cancel.pid")], { cancel: true });
    assert.equal(data.cancelled, true); assert.equal(data.launched, true); assert.notEqual(data.exitCode, 0);
  });
  await t.test("no child without a start grant", async () => {
    const { data } = await probe("sleep", [join(base, "must-not-launch.pid")], { acknowledge: false });
    assert.equal(data.launched, false); assert.equal(data.cancelled, true);
    await assert.rejects(stat(join(base, "must-not-launch.pid")), { code: "ENOENT" });
  });
  await t.test("nonzero native exit remains observable", async () => {
    const { data } = await probe("exit", ["7"]); assert.equal(data.exitCode, 7); assert.equal(data.timedOut, false);
  });
  await t.test("missing executable fails the helper", async () => {
    const { exit, data } = await probe("exit", [], { binary: join(base, "missing.exe") });
    assert.equal(exit, 1); assert.equal(typeof data.helperError, "string");
  });
});
