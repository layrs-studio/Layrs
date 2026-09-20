import test from "node:test";
import assert from "node:assert/strict";
import { loadUiTypeScript } from "./lib/load-ui-typescript.mjs";
const { nativeRunPresentation: present } = loadUiTypeScript("native-run-state.ts");
const now = Date.parse("2026-09-20T12:00:00Z");
const pending = { outcome: null, proofObjectId: null, expiresAt: "2026-09-20T13:00:00Z" };

test("missing declaration waits for a result without inventing an execution or queue", () => {
  assert.equal(present(pending, now).state, "awaiting_result");
  assert.equal(present({ ...pending, status: "running" }, now).state, "running");
  assert.match(present(pending, now).detail, /aucune file d’attente/);
});
test("unknown and contradictory states never become running, success or failure", () => {
  for (const value of [
    { ...pending, status: "new-future-state" },
    { ...pending, status: "completed" }, { ...pending, outcome: "failed" },
    { ...pending, outcome: "passed" }, { ...pending, proofObjectId: "proof" },
    { status: "running", outcome: "passed", proofObjectId: "proof" },
    { status: "queued", outcome: "passed", proofObjectId: "proof" },
    { status: "expired", outcome: "failed", proofObjectId: "proof" },
    { status: "cancelled", outcome: "passed", proofObjectId: "proof" },
  ]) {
    assert.equal(present(value, now).state, "unknown");
    assert.equal(present(value, now).canCancel, false);
  }
});
test("explicit registered preparation waits for the runner begin signal", () => {
  const queued=present({...pending,status:"queued"},now);
  assert.equal(queued.state,"queued");
  assert.equal(queued.canCancel,true);
  assert.match(queued.label,/attente de démarrage/);
  assert.equal(present({...pending,status:"queued",expiresAt:"2026-09-20T11:00:00Z"},now).state,"expired");
});
test("terminal verdict requires its Proof and remains terminal past the deadline", () => {
  for (const outcome of ["passed", "failed"]) {
    const value = present({ status: "completed", outcome, proofObjectId: "proof", expiresAt: "2000-01-01T00:00:00Z" }, now);
    assert.equal(value.state, "completed");
    assert.equal(value.label, outcome === "passed" ? "Réussi" : "Échoué");
    assert.equal(value.canCancel, false);
  }
});
test("cancellation never fabricates failure or claims the remote process stopped", () => {
  const value = present({ ...pending, status: "cancelled" }, now);
  assert.equal(value.state, "cancelled");
  assert.equal(value.canCancel, false);
  assert.match(value.detail, /ne confirme pas l’arrêt/);
});
test("server expiry differs from local clock estimate and neither produces a verdict", () => {
  const server = present({ ...pending, status: "expired" }, now);
  assert.equal(server.state, "expired");
  assert.match(server.detail, /serveur indique/);
  const local = present({ ...pending, status: "running", expiresAt: "2026-09-20T11:00:00Z" }, now);
  assert.equal(local.state, "expired");
  assert.match(local.detail, /horloge locale/);
  assert.equal(present({ ...pending, status: "running", expiresAt: "invalid" }, now).state, "running");
});
