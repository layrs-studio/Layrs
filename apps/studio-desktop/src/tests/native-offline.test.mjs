import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import { createRequire } from "node:module";
import { loadUiTypeScript } from "../../../../scripts/lib/load-ui-typescript.mjs";
const ts = createRequire(new URL("../../package.json", import.meta.url))("typescript");
const ui = loadUiTypeScript("native-project-files-model.ts");
const store = loadUiTypeScript("native-projection-store-model.ts");
const module = { exports: {} };
vm.runInNewContext(ts.transpileModule(fs.readFileSync(new URL("../app/offline.ts", import.meta.url), "utf8"), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText, { module, exports: module.exports, TextEncoder, TextDecoder, Uint8Array, atob, btoa, require: name => { assert.equal(name, "@layrs/ui"); return ui; } });
const m = module.exports, plain = v => JSON.parse(JSON.stringify(v));
const raw = "a".repeat(64), hash = `blake3:${raw}`;
const current = { kind: "generation", worktreeId: "worktree:test", generation: 4, digest: raw };
const capture = { kind: "capture", worktreeId: "worktree:test", sequence: 3, digest: hash };
const remote = { kind: "remote", stepId: hash, rootTreeId: hash, policySetId: hash, authorizationEpoch: 1, observationDigest: raw };
const status = { identity: { worktreeId: "worktree:test", storeId: "store:test", origin: "https://example.test", spaceId: "space:test", principalId: "principal:test" }, projection: { generation: 4, digest: raw }, offline: { shared: true }, needsRepair: false };
const binding = (identity = status.identity, shared = true) => ({ identity, checkoutIdentity: "native-checkout:test", markerDigest: hash, databaseIdentity: "native-database:test", familyMarkerDigest: shared ? hash : null });
const request = m.availabilityRequest(current, null, ["a.txt"], "read");
const restoreModule = { exports: {} };
vm.runInNewContext(ts.transpileModule(fs.readFileSync(new URL("../app/offlineRestore.ts", import.meta.url), "utf8"), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText, { module: restoreModule, exports: restoreModule.exports, require: name => name === "@layrs/ui" ? store : name === "./offline" ? m : assert.fail(`Unexpected import ${name}`) });
const restore = restoreModule.exports;
const restoreStatus = { ...status, capabilities: { restore: true }, projection: { ...status.projection, snapshot: { pathSemantics: "portable_windows" } } };
const restoreTarget = { version: current, paths: ["a.txt"] };
const restorePreview = { operationId: "1234-56", destination: { identity: status.identity, checkoutIdentity: "native-checkout:test" }, source: { kind: "version", ...restoreTarget }, projectionGeneration: 4, projectionDigest: raw, checkoutDigest: hash, afterCheckoutDigest: hash, before: { "a.txt": { fileId: hash, bytes: 5, executable: false } }, after: { "a.txt": null }, digest: hash };

test("local Restore request binds current generation and exact version/paths without preparation semantics", () => {
  assert.deepEqual(plain(restore.localRestoreRequest(restoreTarget, restoreStatus)), { action: "previewRestore", expectedGeneration: 4, ...restoreTarget });
  for (const s of [{ ...restoreStatus, needsRepair: true }, { ...restoreStatus, pendingPublication: {} }, { ...restoreStatus, capabilities: { restore: false } }, { ...restoreStatus, offline: { shared: false } }]) assert.throws(() => restore.localRestoreRequest(restoreTarget, s));
  for (const t of [{ ...restoreTarget, paths: ["../a"] }, { ...restoreTarget, paths: [] }, { ...restoreTarget, version: { ...current, worktreeId: "worktree:other" } }]) assert.throws(() => restore.localRestoreRequest(t, restoreStatus));
});
test("local Restore preview preserves the exact typed payload and rejects swapped operands, generations and metadata", () => {
  assert.equal(restore.localRestorePreview({ preview: restorePreview }, restoreTarget, restoreStatus), restorePreview);
  const invalid = [
    { ...restorePreview, source: { kind: "capture", sequence: 3, paths: ["a.txt"] } },
    { ...restorePreview, source: { ...restorePreview.source, version: capture } },
    { ...restorePreview, source: { ...restorePreview.source, paths: ["other.txt"] } },
    { ...restorePreview, projectionGeneration: 5 }, { ...restorePreview, projectionDigest: hash },
    { ...restorePreview, checkoutDigest: raw }, { ...restorePreview, digest: raw },
    { ...restorePreview, after: { "other.txt": null } }, { ...restorePreview, after: { "a.txt": { fileId: hash, executable: false, bytes: -1 } } },
    { ...restorePreview, operationId: "unbound" }, { ...restorePreview, ignored: true },
  ];
  for (const preview of invalid) assert.throws(() => restore.localRestorePreview({ preview }, restoreTarget, restoreStatus));
});
test("local Restore receipt requires the reviewed operation and matching native capture identity", () => {
  const capture = { schemaVersion: "layrs.projection-capture.v1", sequence: 4, projectionGeneration: 4, projectionDigest: raw, identity: status.identity, changes: [], digest: hash };
  const receipt = { operationId: restorePreview.operationId, capture };
  assert.equal(restore.localRestoreReceipt({ restoration: receipt }, restorePreview, restoreStatus), receipt);
  for (const changed of [{ ...receipt, operationId: "9999" }, { ...receipt, capture: { ...capture, identity: { ...status.identity, worktreeId: "worktree:other" } } }, { ...receipt, capture: { ...capture, projectionGeneration: 5 } }, { ...receipt, capture: { ...capture, digest: raw } }]) assert.throws(() => restore.localRestoreReceipt({ restoration: changed }, restorePreview, restoreStatus));
});

test("Remote Restore requires a current destination binding even with identical source, generation and checkout bytes", () => {
  const target = { ...restoreTarget, version: remote }, preview = { ...restorePreview, source: { kind: "version", ...target } };
  assert.equal(restore.localRestorePreview({ preview }, target, restoreStatus), preview);
  for (const key of Object.keys(status.identity)) {
    const foreign = { ...preview, destination: { ...preview.destination, identity: { ...status.identity, [key]: "foreign" } } };
    assert.throws(() => restore.localRestorePreview({ preview: foreign }, target, restoreStatus));
  }
  assert.throws(() => restore.localRestorePreview({ preview }, target, { ...restoreStatus, identity: { ...status.identity, worktreeId: "worktree:second" } }));
  const legacy = { ...preview }; delete legacy.destination;
  for (const invalid of [legacy, { ...preview, destination: null }, ...["", " ", "bad\0identity", "a".repeat(4097)].map(checkoutIdentity => ({ ...preview, destination: { ...preview.destination, checkoutIdentity } }))]) assert.throws(() => restore.localRestorePreview({ preview: invalid }, target, restoreStatus));
});

test("digest kinds are exact and never inferred or prefix-converted", () => {
  for (const v of [current, capture, remote]) assert.deepEqual(plain(m.version(v)), v);
  for (const v of [{ ...current, digest: hash }, { ...capture, digest: raw }, { ...remote, observationDigest: hash }, { ...current, generation: Number.MAX_SAFE_INTEGER + 1 }]) assert.throws(() => m.version(v));
});
test("currentVersion refuses stale generation, digest and foreign Worktree", () => {
  assert.deepEqual(plain(m.currentVersion({ version: current, shared: true }, status)), current);
  for (const version of [{ ...current, generation: 5 }, { ...current, digest: "b".repeat(64) }, { ...current, worktreeId: "worktree:foreign" }]) assert.throws(() => m.currentVersion({ version, shared: true }, status));
});
test("history references come from actual metadata and bind the opened Worktree", () => {
  assert.deepEqual(plain(m.historyVersions(status, [{ sequence: 3, digest: hash, projectionGeneration: 4, projectionDigest: raw, identity: { worktreeId: "worktree:source" } }])), [capture, current]);
  assert.throws(() => m.historyVersions(status, [{ sequence: 3, digest: hash }]));
});
test("exact paths reject globs aliases traversal and limits; sort by native UTF8 order", () => {
  assert.deepEqual(plain(m.exactPaths("b.txt\na.txt", "portable_windows")), ["a.txt", "b.txt"]);
  for (const s of ["", "a.txt\na.txt", "a.txt\nA.txt", "../a", "src/**", "a[0]", ".layrs/file", Array.from({ length: 1025 }, (_, i) => `a${i}`).join("\n")]) assert.throws(() => m.exactPaths(s, "portable_windows"));
  assert.deepEqual(plain(m.exactPaths("\u{10000}\n\ue000", "case_sensitive")), ["\ue000", "\u{10000}"]);
});
test("comparisons require exactly two operands, preparation cannot fake capture or restore", () => {
  assert.throws(() => m.availabilityRequest(current, null, ["a"], "compareContent"));
  assert.throws(() => m.availabilityRequest(current, capture, ["a"], "read"));
  for (const op of ["capture", "restore"]) assert.throws(() => m.availabilityRequest(current, null, ["a"], op));
  assert.deepEqual(plain(m.availabilityRequest(current, capture, ["a"], "compareMetadata")).other, capture);
});
test("network preconditions preserve displayed generation and reject repair/pending/nonshared", () => {
  assert.deepEqual(plain(m.networkRequest(status, 4)), { expectedGeneration: 4 });
  for (const s of [{ ...status, projection: { generation: 5 } }, { ...status, needsRepair: true }, { ...status, pendingPublication: {} }, { ...status, offline: { shared: false } }]) assert.throws(() => m.networkRequest(s, 4));
});
test("availability keeps exclusion, permission, residence and retention independent", () => {
  const row = { path: "a.txt", operand: 0, absent: false, selected: false, observedWritable: false, materialized: false, retained: true, fileId: hash, manifestId: hash, blockers: ["protectedPath"] };
  const response = { request, paths: [row], effectivePaths: ["a.txt"], blockers: [], available: false, prepared: false };
  assert.deepEqual(plain(m.availability(response, request)).paths[0], row);
  assert.throws(() => m.availability({ ...response, paths: [] }, request));
  assert.throws(() => m.availability({ ...response, available: true }, request));
  assert.throws(() => m.availability({ ...response, request: { ...request, version: capture } }, request));
  assert.throws(() => m.availability({ ...response, paths: [{ ...row, blockers: ["future"] }] }, request));
});
test("prepared replies bind request and recovery identity", () => {
  const p = { id: "preparation:1", status: "ready", request };
  const r = { status: "projectionOfflinePrepared", preparation: p };
  assert.deepEqual(plain(m.preparedReply(r, request, p.id)), plain(p));
  assert.throws(() => m.preparedReply(r, request, "preparation:other"));
  assert.throws(() => m.preparedReply(r, { ...request, version: capture }));
  assert.throws(() => m.preparations({ preparations: [p, p] }));
  assert.equal(m.preparations({ preparations: [{ ...p, status: "released" }] })[0].status, "released");
  assert.throws(() => m.preparedReply({ ...r, preparation: { ...p, status: "released" } }, request));
});
test("remote observations bind exact Step and raw digest", () => {
  assert.deepEqual(plain(m.observedReply({ status: "projectionVersionObserved", version: remote }, hash)), remote);
  assert.throws(() => m.observedReply({ status: "projectionVersionObserved", version: remote }, `blake3:${"b".repeat(64)}`));
});

test("preparation history accepts the actual 1024 active plus 64 released boundary with canonical entries", () => {
  const active = Array.from({ length: 1024 }, (_, i) => ({ id: `active:${i}`, status: i % 2 ? "acquiring" : "ready", request }));
  const released = Array.from({ length: 64 }, (_, i) => ({ id: `released:${i}`, status: "released", request }));
  const all = [...active, ...released];
  assert.deepEqual(plain(m.preparations({ preparations: all })), plain(all));
  for (const preparations of [
    [...all, { ...released[0], id: "released:extra" }],
    [...active, { ...active[0], id: "active:extra" }, ...released.slice(1)],
    [...active.slice(1), ...released, { ...released[0], id: "released:extra" }],
    [...active, ...released.slice(1), { ...released[0], id: active[0].id }],
    [...all.slice(0, -1), { ...released[0], request: { ...request, paths: ["b.txt", "a.txt"] } }],
  ]) assert.throws(() => m.preparations({ preparations }));
});
test("bounded read verifies bytes/version/path and refuses oversized or malformed base64", () => {
  const reply = { version: current, path: "a.txt", encoding: "base64", bytes: 3, content: "YWJj" };
  assert.equal(m.preview(reply, current, "a.txt").text, "abc");
  for (const r of [{ ...reply, bytes: 2 }, { ...reply, content: "YWJj\n" }, { ...reply, version: capture }, { ...reply, path: "b.txt" }, { ...reply, bytes: 1048577 }]) assert.throws(() => m.preview(r, current, "a.txt"));
  assert.equal(m.preview({ ...reply, bytes: 2, content: "/wA=" }, current, "a.txt").text, null);
  assert.equal(m.preview({ ...reply, bytes: 1, content: "AA==" }, current, "a.txt").text, null);
  const maximum = "x".repeat(1048576);
  assert.equal(m.preview({ ...reply, bytes: maximum.length, content: btoa(maximum) }, current, "a.txt").text.length, maximum.length);
});
test("quota uses exact integer octets without rounding", () => {
  assert.equal(m.quotaBytes("9007199254740991"), Number.MAX_SAFE_INTEGER);
  for (const v of ["9007199254740992", "1.2", "1e9", "-1", "", " 2", "03"]) assert.throws(() => m.quotaBytes(v));
});
test("conversion receipt binds source/destination identities and source retention", () => {
  const dest = { ...status.identity, worktreeId: "worktree:copy", storeId: "store:family" };
  const r = { path: "C:/copy", identity: dest, receipt: { sourceCheckout: "C:/source", sourceIdentity: status.identity, destinationIdentity: dest, sourceRetained: true, importedCaptureCount: 3, sourceGeneration: 4, checkoutDigest: raw, sourceDatabaseDigest: raw, sourceProjectionDigest: raw, publicationHistoryDigest: raw } };
  assert.equal(m.conversion(r, "C:/source", "C:/copy", { identity: status.identity }).path, "C:/copy");
  assert.throws(() => m.conversion({ ...r, receipt: { ...r.receipt, sourceRetained: false } }, "C:/source", "C:/copy", status));
  assert.throws(() => m.conversion(r, "C:/source", "C:/other", status));
  const missing = { ...dest }; delete missing.storeId;
  assert.throws(() => m.conversion({ ...r, identity: missing, receipt: { ...r.receipt, destinationIdentity: missing } }, "C:/source", "C:/copy", status));
  for (const args of [["C:/source", "C:/source/copy"], ["C:/source", "C:/copy", "C:/copy/family"], ["C:/source", "C:/../copy"]]) assert.throws(() => m.separateDirectories(...args));
});
test("conversion inspection validates identity and permission without requiring ordinary status", () => {
  const allowed = { identity: status.identity, binding: binding(status.identity, false), shared: false, recovery: [], conversion: { allowed: true, reason: null } };
  assert.deepEqual(plain(m.conversionInspection(allowed)), allowed);
  const shared = { ...allowed, binding: binding(), shared: true, conversion: { allowed: false, reason: "already shared" } };
  assert.deepEqual(plain(m.conversionInspection(shared)), shared);
  const refused = { ...allowed, conversion: { allowed: false, reason: "source unavailable for conversion" } };
  assert.equal(m.conversionInspection(refused).conversion.allowed, false);
  for (const bad of [null, status, { ...allowed, identity: null }, { ...allowed, identity: { ...status.identity, worktreeId: "" } }, { ...allowed, shared: true }, { ...allowed, conversion: { allowed: "true", reason: null } }, { ...allowed, conversion: { allowed: false, reason: null } }, { ...allowed, conversion: { allowed: true, reason: "contradiction" } }]) assert.throws(() => m.conversionInspection(bad));
});

test("typed recovery distinguishes remaining application needs and refuses malformed or contradictory inspection", () => {
  const recovery = ["familyRollback", "worktreeRollback", "initializing", "conversion", "filesystemOperation", "publication"];
  const pending = { identity: status.identity, binding: binding(), shared: true, recovery, conversion: { allowed: false, reason: "recovery_required" } };
  assert.deepEqual(plain(m.conversionInspection(pending)), pending);
  for (const value of [undefined, null, "familyRollback", ["unknown"], ["familyRollback", "familyRollback"], [...recovery, "publication"]]) assert.throws(() => m.conversionInspection({ ...pending, recovery: value }));
  assert.throws(() => m.conversionInspection({ ...pending, shared: false, conversion: { allowed: true, reason: null } }));
});

test("database recovery requires typed rollback evidence and preserves application needs in the bound reply", () => {
  const before = { identity: status.identity, binding: binding(), shared: true, recovery: ["familyRollback", "worktreeRollback"], conversion: { allowed: false, reason: "database_recovery_required" } };
  assert.deepEqual(plain(m.databaseRecoveryRequest(before)), { action: "recoverDatabases", expected: before.binding });
  const after = { ...before, recovery: ["filesystemOperation", "publication", "conversion", "initializing"] };
  assert.deepEqual(plain(m.recoveredInspection(after, before)), after);
  for (const recovery of [[], ["filesystemOperation"], ["publication"], ["initializing"], ["conversion"]]) assert.throws(() => m.databaseRecoveryRequest({ ...before, recovery }));
  for (const identity of Object.keys(status.identity).map(key => ({ ...status.identity, [key]: "foreign" }))) assert.throws(() => m.recoveredInspection({ ...after, identity }, before));
  assert.throws(() => m.recoveredInspection({ ...after, shared: false }, before));
});

test("initialization recovery requires only the exact typed initializing reason", () => {
  const pending = { identity: status.identity, binding: binding(), shared: true, recovery: ["initializing"], conversion: { allowed: false, reason: "initializing" } };
  assert.deepEqual(plain(m.initializationRecoveryRequest(pending)), { action: "resumeInitialization", expected: pending.binding });
  for (const recovery of [[], ["conversion"], ...["familyRollback", "worktreeRollback", "conversion", "filesystemOperation", "publication"].map(r => ["initializing", r])]) assert.throws(() => m.initializationRecoveryRequest({ ...pending, recovery }));
});

for (const reason of ["worktreeRollback", "initializing"]) test(`recovery reply preserves the full native binding with equal logical identity (${reason})`, () => {
  const before = { identity: status.identity, binding: binding(), shared: true, recovery: [reason], conversion: { allowed: false, reason } };
  const sent = reason === "initializing" ? m.initializationRecoveryRequest(before) : m.databaseRecoveryRequest(before);
  assert.equal(sent.expected, before.binding);
  const after = { ...before, recovery: [], binding: Object.fromEntries(Object.entries(before.binding).reverse()) };
  after.binding.identity = Object.fromEntries(Object.entries(before.identity).reverse());
  assert.equal(m.recoveredInspection(after, before), after);
  for (const key of ["checkoutIdentity", "databaseIdentity", "markerDigest", "familyMarkerDigest"]) {
    const changed = { ...after, binding: { ...after.binding, [key]: key.endsWith("Digest") ? `blake3:${"b".repeat(64)}` : "native:replacement" } };
    assert.equal(m.conversionInspection(changed), changed);
    assert.deepEqual(changed.identity, before.identity);
    assert.throws(() => m.recoveredInspection(changed, before), /La reprise ne correspond pas au dossier inspecté/);
  }
  for (const key of Object.keys(before.identity)) {
    const identity = { ...before.identity, [key]: "foreign" };
    const changed = { ...after, identity, binding: { ...after.binding, identity } };
    assert.equal(m.conversionInspection(changed), changed);
    assert.throws(() => m.recoveredInspection(changed, before), /La reprise ne correspond pas au dossier inspecté/);
  }
});

test("filesystem and reservation recovery require valid inspection, appropriate needs and shared state", () => {
  const shared = { identity: status.identity, binding: binding(), shared: true, recovery: [], conversion: { allowed: false, reason: "already_shared" } };
  assert.equal(m.recoveryActionBlocker(shared, "reservation"), undefined);
  assert.ok(m.recoveryActionBlocker(shared, "filesystem"));
  const filesystem = { ...shared, recovery: ["filesystemOperation"] };
  assert.equal(m.recoveryActionBlocker(filesystem, "filesystem"), undefined);
  assert.ok(m.recoveryActionBlocker(filesystem, "reservation"));
  for (const source of [null, { ...shared, identity: null }, { ...shared, recovery: ["unknown"] }]) for (const action of ["filesystem", "reservation"]) assert.ok(m.recoveryActionBlocker(source, action));
  for (const reason of ["familyRollback", "worktreeRollback", "initializing", "conversion"]) assert.ok(m.recoveryActionBlocker({ ...filesystem, recovery: ["filesystemOperation", reason] }, "filesystem"));
  assert.ok(m.recoveryActionBlocker({ ...shared, shared: false, conversion: { allowed: true, reason: null } }, "reservation"));
});

test("every recovery request retains the entire native binding and refuses missing or malformed bindings", () => {
  const base = { identity: status.identity, binding: binding(), shared: true, recovery: [], conversion: { allowed: false, reason: "already_shared" } };
  const cases = [
    [[], value => m.reservationRecoveryRequest(value)],
    [["familyRollback"], value => m.databaseRecoveryRequest(value)],
    [["initializing"], value => m.initializationRecoveryRequest(value)],
    ...["resumeInstallation", "resumeNamespace", "resumeRefresh", "resumeRestore", "resumeRebase"].map(action => [["filesystemOperation"], value => m.filesystemRecoveryRequest(value, action)]),
  ];
  for (const [recovery, send] of cases) {
    const inspected = { ...base, recovery };
    assert.equal(send(inspected).expected, inspected.binding);
    for (const changed of [undefined, null, { ...base.binding, identity: { ...status.identity, storeId: "store:foreign" } }, { ...base.binding, markerDigest: raw }, { ...base.binding, familyMarkerDigest: null }, { ...base.binding, checkoutIdentity: "" }, { ...base.binding, databaseIdentity: "é".repeat(257) }, { ...base.binding, extra: "unvalidated" }]) assert.throws(() => send({ ...inspected, binding: changed }));
  }
  assert.throws(() => m.conversionInspection({ ...base, shared: false, conversion: { allowed: true, reason: null } }));
  assert.equal(m.conversionInspection({ ...base, binding: binding(status.identity, false), shared: false, conversion: { allowed: true, reason: null } }).binding.familyMarkerDigest, null);
  assert.throws(() => m.filesystemRecoveryRequest({ ...base, recovery: ["filesystemOperation"] }, "capture"));
});
test("created family binds returned path and authority before conversion uses it", () => {
  const r = { path: "C:/family", identity: status.identity, usage: { contentBytes: 0, reservedBytes: 0, limitBytes: 1024, protectionRoots: 0, windowsPowerLossQualified: false } };
  assert.equal(m.createdFamily(r, "C:/family", status), "C:/family");
  assert.throws(() => m.createdFamily(r, "C:/other", status));
  assert.throws(() => m.createdFamily({ ...r, identity: { ...r.identity, principalId: "principal:other" } }, "C:/family", status));
});
test("malformed additive offline status fails closed", () => {
  assert.throws(() => store.projectionStoreFiles({ ...status, offline: { shared: "true" } }), /hors ligne/);
  assert.throws(() => store.projectionStoreFiles({ ...status, latestCaptureImported: "false" }), /hors ligne/);
});

test("generation release sends only an explicit earlier integer and validates the exact reply", () => {
  assert.deepEqual(plain(m.releaseGenerationRequest("2", status)), { action: "releaseGenerationRetention", generation: 2 });
  for (const input of ["", "-1", "1.5", "1e0", "9007199254740992", "4", "5"]) assert.throws(() => m.releaseGenerationRequest(input, status));
  for (const changed of [{ ...status, offline: { shared: false } }, { ...status, needsRepair: true }, { ...status, pendingPublication: {} }]) assert.throws(() => m.releaseGenerationRequest("2", changed));
  assert.equal(m.releasedGeneration({ generation: 2, releasedClaims: 0 }, 2), 0);
  for (const reply of [{ generation: 3, releasedClaims: 2 }, { generation: 2, releasedClaims: -1 }, { generation: 2, releasedClaims: "2" }, { generation: 2, releasedClaims: 9007199254740992 }]) assert.throws(() => m.releasedGeneration(reply, 2));
});
test("reservation cancellation requires the explicit native acknowledgement", () => {
  assert.equal(m.cancelledReservation({ reservationCancelled: true }), true);
  for (const value of [null, {}, { reservationCancelled: false }, { reservationCancelled: "true" }]) assert.throws(() => m.cancelledReservation(value));
});
test("conversion abandonment requires separate explicit paths and retained-files acknowledgement without source state", () => {
  assert.deepEqual(plain(m.abandonConversionRequest("C:/family", "C:/copy")), { action: "abandonConversion", familyRoot: "C:/family", destination: "C:/copy" });
  for (const pair of [["", "C:/copy"], ["relative", "C:/copy"], ["C:/family", "C:/family/copy"], ["C:/family", "C:/family"]]) assert.throws(() => m.abandonConversionRequest(...pair));
  assert.equal(m.abandonedConversion({ destination: "C:/copy", abandoned: true, filesRetained: true }, "C:/copy"), "C:/copy");
  for (const reply of [{ destination: "C:/other", abandoned: true, filesRetained: true }, { destination: "C:/copy", abandoned: false, filesRetained: true }, { destination: "C:/copy", abandoned: true, filesRetained: false }, { destination: "C:/copy", abandoned: true }]) assert.throws(() => m.abandonedConversion(reply, "C:/copy"));
});
