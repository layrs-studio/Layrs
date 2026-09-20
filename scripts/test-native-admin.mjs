// Pure administration transport contract; no browser or additional test runtime.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(path.join(root, "packages/ui/package.json"));
const ts = require("typescript");
const source = fs.readFileSync(path.join(root, "packages/ui/src/native-admin-route.ts"), "utf8");
const compiled = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX } }).outputText;
const module = { exports: {} };
vm.runInNewContext(compiled, { require, module, exports: module.exports });
const route = module.exports.nativeAdminRoute;
const plain = value => JSON.parse(JSON.stringify(value));
const metricsSource = fs.readFileSync(path.join(root, "packages/ui/src/native-metrics-model.ts"), "utf8");
const metricsModule = { exports: {} };
vm.runInNewContext(ts.transpileModule(metricsSource, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText, { module: metricsModule, exports: metricsModule.exports });
const metrics = metricsModule.exports.nativeMetrics;
const sampleMetrics = () => ({
  schemaVersion: "layrs.telemetry.v1", scope: "current_process", observedUnixMs: 1789900000000, uptimeSeconds: 10,
  http: { inFlight: 1, completed: 4, interruptedBeforeHeaders: 0, headersLatencyMicrosecondsTotal: 8000,
    statusClasses: [1, 2, 3, 4, 5].map(n => ({ class: `${n}xx`, count: n === 2 ? 4 : 0 })),
    headersLatencyBuckets: [5, 25, 100, 500, 2000, 10000].map(n => ({ lessOrEqualMs: n, count: 4 })) },
  readiness: { checks: 3, databaseFailures: 0, casFailures: 1 },
});

test("teams remain Space-scoped and preserve mutation/version bodies", () => {
  const request = {idempotencyKey:"retry-stable",expectedVersion:3,name:"Design",role:"auditor",memberPrincipalIds:["principal:bob"]};
  assert.equal(route({action:"eligibleTeamMembers"},"space:team").path,"/v3/spaces/space%3Ateam/teams/eligible-members");
  assert.deepEqual(plain(route({action:"policyManagement"},"space:team")), {method:"GET",path:"/v3/spaces/space%3Ateam/policy-management"});
  assert.deepEqual(plain(route({action:"updateTeam",team:"team:123",request},"space:team")),{method:"PUT",path:"/v3/spaces/space%3Ateam/teams/team%3A123",body:request});
  assert.equal(route({action:"teamHistory",team:"team:123",after:3,limit:20,anchor:7},"space:team").path,"/v3/spaces/space%3Ateam/teams/team%3A123/history?after=3&limit=20&anchor=7");
  for (const action of ["policyManagement","listTeams","createTeam","updateTeam","archiveTeam","teamHistory"]) assert.throws(()=>route({action,team:"team:123",request},""));
  for (const team of ["../auth","team:id?version=9","team:id/archive"]) assert.throws(()=>route({action:"archiveTeam",team,request},"space:team"));
  for (const key of ["after","limit","anchor"]) for (const value of [-1,Infinity,"0&other=1",Number.MAX_SAFE_INTEGER+1]) {
    assert.throws(()=>route({action:"teamHistory",team:"team:123",after:0,limit:20,anchor:3,[key]:value},"space:team"));
  }
});

test("metrics presentation projects only bounded numeric observations", () => {
  const sample = sampleMetrics();
  assert.deepEqual(plain(metrics(sample)), sample);
  assert.equal(JSON.stringify(metrics({ ...sample, accessToken: "never-display" })).includes("never-display"), false);
  for (const value of [NaN, Infinity, -1, "secret", Number.MAX_SAFE_INTEGER + 1]) {
    assert.throws(() => metrics({ ...sample, uptimeSeconds: value }));
  }
  assert.throws(() => metrics({ ...sample, schemaVersion: "unknown" }));
  assert.throws(() => metrics({ ...sample, http: { ...sample.http, statusClasses: [] } }));
  assert.throws(() => metrics({ ...sample, http: { ...sample.http, headersLatencyBuckets: [{ lessOrEqualMs: "secret", count: 1 }] } }));
});

test("global identity routes keep secrets in the explicit body and need no Space", () => {
  assert.deepEqual(plain(route({ action: "metrics" }, "")), { method: "GET", path: "/v3/admin/metrics" });
  const request = { login: "test@example.test", password: "test-only-password", displayName: "Test" };
  const result = plain(route({ action: "createAccount", request }, ""));
  assert.deepEqual(result, { method: "POST", path: "/v3/auth/accounts", body: request });
  assert.ok(!result.path.includes(request.password));
  assert.deepEqual(plain(route({ action: "listTokens" }, "")), { method: "GET", path: "/v3/auth/tokens" });
});
test("protection uses PUT and webhook secret rotation is explicit POST", () => {
  assert.equal(route({ action: "configureProtection", layer: "layer:main", request: { minimumApprovals: 2 } }, "space:team").method, "PUT");
  const rotation = plain(route({ action: "rotateWebhook", webhook: "webhook:example" }, "space:team"));
  assert.equal(rotation.method, "POST");
  assert.equal(rotation.path, "/v3/spaces/space%3Ateam/webhooks/webhook%3Aexample/rotate");
});
test("a command cannot escape the authenticated origin or inject a route", () => {
  for (const webhook of ["../auth/logout", "webhook:id?secret=x", "https://attacker.test", "webhook:id/rotate", "webhook:id#fragment"]) {
    assert.throws(() => route({ action: "rotateWebhook", webhook }, "space:team"));
  }
  assert.throws(() => route({ action: "arbitrary", path: "https://attacker.test" }, "space:team"));
  assert.throws(() => route({ action: "listWebhooks" }, "../other"));
});
test("notification cursors cannot inject query strings or exceed safe integer range", () => {
  for (const after of [-1, Infinity, NaN, Number.MAX_SAFE_INTEGER + 1, "0&other=1"]) assert.throws(() => route({ action: "notifications", after }, "space:team"));
  assert.equal(route({ action: "notifications", after: 42 }, "space:team").path, "/v3/spaces/space%3Ateam/notifications?after=42");
});

test("invitation inbox and recipient decisions require no selected Space", () => {
  assert.deepEqual(plain(route({ action: "invitationInbox", after: 0 }, "")), { method: "GET", path: "/v3/invitations?after=0" });
  for (const decision of ["accept", "reject", "revoke"]) {
    assert.deepEqual(plain(route({ action: "decideInvitation", invitation: "invitation:123", decision }, "")), {
      method: "POST", path: "/v3/invitations/invitation%3A123/actions", body: { action: decision },
    });
  }
  assert.throws(() => route({ action: "decideInvitation", invitation: "../accounts", decision: "accept" }, ""));
  assert.throws(() => route({ action: "decideInvitation", invitation: "invitation:123", decision: "grantAdmin" }, ""));
});

test("invitation administration stays Space-scoped and validates pagination", () => {
  const request = { recipientPrincipalId: "principal:bob", role: "auditor", expiresInSeconds: 86400, idempotencyKey: "test-key" };
  assert.deepEqual(plain(route({ action: "inviteMember", request }, "space:team")), { method: "POST", path: "/v3/spaces/space%3Ateam/invitations", body: request });
  assert.equal(route({ action: "listInvitations", after: 100 }, "space:team").path, "/v3/spaces/space%3Ateam/invitations?after=100");
  assert.throws(() => route({ action: "inviteMember", request }, ""));
  for (const action of ["invitationInbox", "listInvitations"]) {
    for (const after of [-1, Infinity, "0&spaceId=other"]) assert.throws(() => route({ action, after }, "space:team"));
  }
});
