// Real CLI -> OS credential store -> HTTPS fixture. Run sequentially with the
// Desktop team harness: both deliberately use the same origin-scoped vault.
// Fixture credentials and device codes never appear in output or request files.
import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { readFile, writeFile, mkdtemp, rm, stat } from "node:fs/promises";
import { request as httpsRequest } from "node:https";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repository = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const MAX_OUTPUT = 4 * 1024 * 1024;
const MAX_REQUEST = 256 * 1024;
const started = Date.now();
const deadline = started + 180_000;
let commands = 0;
let temporary;
let authenticated = false;
let logout;

function ensure(condition, message) {
  if (!condition) throw new Error(message);
}

function noCredentialFields(value) {
  if (!value || typeof value !== "object") return;
  for (const [key, child] of Object.entries(value)) {
    ensure(!/^(access_?token|refresh_?token|device_?code|password)$/i.test(key), "CLI exposed a credential field");
    noCredentialFields(child);
  }
}

async function main() {
  const fixturePath = process.env.LAYRS_NATIVE_UI_FIXTURE;
  ensure(fixturePath, "Set LAYRS_NATIVE_UI_FIXTURE to the private disposable TLS fixture");
  ensure((await stat(fixturePath)).size <= 1024 * 1024, "Fixture exceeds 1 MiB");
  let fixture;
  try { fixture = JSON.parse(await readFile(fixturePath, "utf8")); }
  catch { throw new Error("Private fixture could not be decoded"); }
  for (const key of ["origin", "caPem", "workspaceRoot", "spaceId", "reviewId", "releaseId", "firstStepId", "stepId", "adminAccessToken"]) {
    ensure(typeof fixture[key] === "string" && fixture[key].length > 0, "Fixture is missing a required field");
  }
  const origin = new URL(fixture.origin);
  ensure(origin.protocol === "https:" && !origin.username && !origin.password && origin.pathname === "/", "Fixture origin must be HTTPS without credentials or path");
  const binary = path.resolve(process.env.LAYRS_NATIVE_CLI ?? path.join(repository, "target/debug", process.platform === "win32" ? "layrs.exe" : "layrs"));
  ensure((await stat(binary)).isFile(), "Build the native CLI before running this test");
  temporary = await mkdtemp(path.join(tmpdir(), "layrs-cli-remote-"));
  const ca = path.join(temporary, "ca.pem");
  await writeFile(ca, fixture.caPem, { mode: 0o600, flag: "wx" });

  const environment = { ...process.env };
  for (const name of Object.keys(environment)) {
    if (/^LAYRS_(?:TEST_DATABASE_URL|DATABASE_URL|BOOTSTRAP|ADMIN_ACCESS_TOKEN)/.test(name)) delete environment[name];
  }
  function run(arguments_, options = {}) {
    commands++;
    const timeout = Math.min(options.timeout ?? 20_000, deadline - Date.now());
    ensure(timeout > 0, "CLI remote test exceeded its overall deadline");
    return new Promise((resolve, reject) => {
      const child = spawn(binary, ["--json", "--space", fixture.workspaceRoot, "remote", ...arguments_, "--endpoint", origin.origin, "--ca", ca], {
        cwd: repository, env: environment, shell: false, windowsHide: true, stdio: ["ignore", "pipe", "pipe"],
      });
      let stdout = "";
      let stderr = "";
      let bytes = 0;
      let finished = false;
      function fail(message) {
        if (finished) return;
        finished = true;
        clearTimeout(timer);
        child.kill();
        reject(new Error(message));
      }
      const timer = setTimeout(() => fail("CLI command timed out"), timeout);
      child.stdout.setEncoding("utf8");
      child.stderr.setEncoding("utf8");
      function capture(chunk, errorStream) {
        if (finished) return;
        bytes += Buffer.byteLength(chunk);
        if (bytes > MAX_OUTPUT) return fail("CLI output exceeded its bound");
        if (errorStream) {
          stderr += chunk;
          try { options.stderr?.(stderr); } catch { fail("Device approval parsing failed"); }
        } else stdout += chunk;
      }
      child.stdout.on("data", chunk => capture(chunk, false));
      child.stderr.on("data", chunk => capture(chunk, true));
      child.on("error", () => fail("Could not launch native CLI"));
      child.on("close", code => {
        if (finished) return;
        finished = true;
        clearTimeout(timer);
        try {
          ensure(!stdout.includes(fixture.adminAccessToken) && !stderr.includes(fixture.adminAccessToken), "Fixture credential leaked into CLI output");
          const response = JSON.parse(stdout);
          noCredentialFields(response);
          if (options.expectFailure) {
            ensure(code !== 0 && response.ok === false, "Expected CLI request rejection");
          } else {
            const message = String(response.error?.message || "");
            const category = /^(remote_[a-z_]+|native_[a-z_]+|invalid_[a-z_]+):/.exec(message)?.[1]
              || ["OS credential store unavailable","No device login in progress for this server"].find(value=>message===value)
              || /^Windows Credential Manager (?:refused|could not read|could not delete) the Layrs token \(error \d+\)\.$/.exec(message)?.[0];
            const status = /remote HTTP status (\d{3})/.exec(message)?.[1];
            const errorCode = category || status ? ` (${category || "HTTP"}${status ? ` ${status}` : ""})` : "";
            ensure(code === 0 && response.ok === true, `Native CLI command ${arguments_[0]} failed${errorCode} (output withheld)`);
          }
          resolve(response.data);
        } catch (error) {
          reject(new Error(error instanceof SyntaxError ? "Native CLI did not return JSON" : error.message));
        }
      });
    });
  }

  function approve(userCode) {
    return new Promise((resolve, reject) => {
      const request = httpsRequest(new URL("/v3/auth/devices/approve", origin), {
        method: "POST", ca: fixture.caPem, rejectUnauthorized: true, timeout: 10_000,
        headers: { Authorization: `Bearer ${fixture.adminAccessToken}`, "Content-Type": "application/json" },
      }, response => {
        response.resume();
        response.on("end", () => response.statusCode === 204 ? resolve(true) : reject(new Error("Device approval rejected")));
      });
      request.on("timeout", () => request.destroy());
      request.on("error", () => reject(new Error("Device approval TLS request failed")));
      request.end(JSON.stringify({ userCode }));
    });
  }
  let approval;
  const login = await run(["login"], {
    timeout: 45_000,
    stderr(text) {
      const match = /and approve code ([A-Z0-9-]+)(?:\r?\n|$)/.exec(text);
      if (match && !approval) approval = approve(match[1]).then(() => true, () => false);
    },
  });
  authenticated = login.status === "authorized";
  logout = () => run(["logout"]);
  ensure(authenticated && approval && await approval, "Device login was not authorized");
  const who = await run(["whoami"]);
  ensure(who.principal_id === login.principalId, "Device identity differs from whoami");

  const listed = await run(["reviews"]);
  ensure(Array.isArray(listed.reviews) && typeof listed.hasMore === "boolean", "Review list must expose pagination");
  if (listed.nextCursor) {
    const next = await run(["reviews", listed.nextCursor]);
    const ids = new Set(listed.reviews.map(review => review.proposal.reviewId));
    ensure(next.reviews.every(review => !ids.has(review.proposal.reviewId)), "Review pages contain duplicates");
  }
  const detail = await run(["review", fixture.reviewId]);
  const proposal = detail.versions?.at(-1);
  ensure(proposal?.reviewId === fixture.reviewId && Number.isInteger(proposal.version), "Exact review version is unavailable");
  const nonce = randomUUID();
  const comment = {
    idempotencyKey: `cli-comment-${nonce}`, version: proposal.version, action: "comment",
    body: "Commentaire de recette CLI distante", anchor: { path: "docs/readme.txt", startLine: 1, endLine: 1 }, replyTo: null,
  };
  async function requestFile(name, document) {
    const contents = JSON.stringify(document);
    ensure(Buffer.byteLength(contents) <= MAX_REQUEST, "Request fixture exceeds CLI JSON limit");
    const filename = path.join(temporary, name);
    await writeFile(filename, contents, { mode: 0o600, flag: "wx" });
    return filename;
  }
  const commentFile = await requestFile("comment.json", comment);
  const action = await run(["review-action", fixture.reviewId, commentFile]);
  const replay = await run(["review-action", fixture.reviewId, commentFile]);
  ensure(action.actionId && JSON.stringify(action) === JSON.stringify(replay), "Comment retry did not preserve its immutable action");
  const replyFile = await requestFile("reply.json", { ...comment, idempotencyKey: `cli-reply-${nonce}`, body: "Réponse conservée sur la version exacte", anchor: null, replyTo: action.actionId });
  const reply = await run(["review-action", fixture.reviewId, replyFile]);
  const updated = await run(["review", fixture.reviewId]);
  ensure(updated.actions.some(item => item.actionId === reply.actionId && item.replyTo === action.actionId && item.version === proposal.version), "Comment reply lost its version or parent");
  const correctionFile = await requestFile("correction.json", {idempotencyKey:`cli-correction-${nonce}`,version:proposal.version,expectedRevision:0,body:"Texte corrige dans la recette CLI",reason:"Precision auditee"});
  const corrected = await run(["review-correct",fixture.reviewId,action.actionId,correctionFile]);
  const correctionReplay = await run(["review-correct",fixture.reviewId,action.actionId,correctionFile]);
  ensure(JSON.stringify(corrected) === JSON.stringify(correctionReplay), "Correction retry differs");
  const audited = await run(["review",fixture.reviewId]);
  ensure(audited.actions.some(item => item.actionId === action.actionId && item.body === comment.body), "Correction overwrote the original comment");
  ensure(audited.commentCorrections.some(item => item.record.body === "Texte corrige dans la recette CLI" && item.record.version === proposal.version), "Comment correction audit is absent");
  const oversized = path.join(temporary, "oversized.json");
  await writeFile(oversized, " ".repeat(MAX_REQUEST + 1), { mode: 0o600, flag: "wx" });
  await run(["review-action", fixture.reviewId, oversized], { expectFailure: true });

  const comparison = await run(["compare", fixture.firstStepId, fixture.stepId]);
  const downloadedFile = path.join(temporary, "downloaded-readme.txt");
  const fileResult = await run(["file", fixture.stepId, "docs/readme.txt", "--output", downloadedFile]);
  const expectedFile = await readFile(path.join(fixture.workspaceRoot, "docs/readme.txt"));
  ensure((await readFile(downloadedFile)).equals(expectedFile) && fileResult.byteLength === expectedFile.length, "Point file read differs from the admitted content");
  await run(["file", fixture.stepId, "docs/readme.txt", "--output", downloadedFile], {expectFailure:true});
  ensure((await readFile(downloadedFile)).equals(expectedFile), "Point file read overwrote an existing destination");
  const missingOutput = path.join(temporary, "missing-output.txt");
  await run(["file", fixture.stepId, "../private", "--output", missingOutput], {expectFailure:true});
  await run(["file", fixture.stepId, "absent-file.txt", "--output", missingOutput], {expectFailure:true});
  ensure(await stat(missingOutput).then(()=>false,error=>error.code === "ENOENT"), "Failed point file read created an output");
  const history = await run(["history", fixture.stepId]);
  const pathHistory = await run(["history", fixture.stepId,"--path","docs/readme.txt"]);
  ensure(pathHistory.path === "docs/readme.txt" && pathHistory.steps.some(step=>step.stepId===fixture.firstStepId),"Path history lost the original creation");
  const missingHistory = await run(["history", fixture.stepId,"--path","absent-file.txt"]);
  ensure(missingHistory.steps.length===0 && missingHistory.next===null,"Absent path unexpectedly has a history");
  ensure(history.anchor === fixture.stepId && history.steps.some(step => step.stepId === fixture.firstStepId), "Remote history lost an ancestor");
  const admissions=Object.values(history.observations).flat().filter(value=>value.provenance === "server_admission");
  ensure(admissions.length>0 && admissions.every(value=>value.provenance==="server_admission" && value.principalId===who.principal_id && value.observation.actorId===who.actor_id),"Published provenance is not the authenticated first admission");
  const actorHistory=await run(["history",fixture.stepId,"--actor",who.actor_id,"--since","2000-01-01T00:00:00Z","--until","2100-01-01T00:00:00Z"]);
  ensure(actorHistory.steps.length===history.steps.length,"Actor and publication period lost admitted Steps");
  const otherActorHistory=await run(["history",fixture.stepId,"--actor","actor:unknown-cli-history"]);
  ensure(otherActorHistory.steps.length===0,"Unknown actor matched a server admission");
  await run(["history",fixture.stepId,"--since","2100-01-01T00:00:00Z","--until","2000-01-01T00:00:00Z"],{expectFailure:true});
  ensure(comparison.before === fixture.firstStepId && comparison.after === fixture.stepId && comparison.changes.some(change => change.path === "docs/readme.txt"), "Comparison did not return the two exact fixture Steps");
  const releases = await run(["releases"]);
  ensure(releases.releases.some(entry => entry.record.releaseId === fixture.releaseId), "Published release manifest is absent");
  const events = await run(["release-events", fixture.releaseId]);
  ensure(events.releaseId === fixture.releaseId && Array.isArray(events.events), "Release event history is invalid");
  const runs = await run(["runs"]);
  ensure(Array.isArray(runs.runs), "Run list is invalid");
  ensure(typeof runs.hasMore === "boolean", "Run continuation metadata is absent");
  for (const item of runs.runs) ensure(Number.isFinite(Date.parse(item.expiresAt)), "Run expiry is not an RFC3339 timestamp");
  const inbox = await run(["invitations-inbox"]);
  ensure(Array.isArray(inbox.invitations), "Invitation inbox is invalid");
  const invitationFile = await requestFile("invitation.json", {idempotencyKey:`cli-invitation-${nonce}`,recipientPrincipalId:fixture.accounts.bob.principalId,role:"auditor",expiresInSeconds:3600});
  const invitation = await run(["invite-member",invitationFile]);
  ensure(invitation.state === "pending", "Invitation granted access before acceptance");
  const invited = await run(["invitations"]);
  ensure(invited.invitations.some(item => item.invitationId === invitation.invitationId), "Created invitation is absent");
  const revoked = await run(["invitation-decision",invitation.invitationId,"revoke"]);
  ensure(revoked.state === "revoked", "Invitation was not revoked");
  const eligible = await run(["team-members"]);
  ensure(eligible.members.some(member => member.principalId === fixture.accounts.bob.principalId), "Eligible Workspace member is absent");
  const teamFile = await requestFile("team.json", {idempotencyKey:`cli-team-${nonce}`,expectedVersion:0,name:`CLI ${nonce}`,role:"auditor",memberPrincipalIds:[fixture.accounts.bob.principalId]});
  const createdTeam = await run(["team-create",teamFile]);
  const repeatedTeam = await run(["team-create",teamFile]);
  ensure(JSON.stringify(createdTeam) === JSON.stringify(repeatedTeam) && createdTeam.team.version === 1,"Team creation retry differs");
  const teamId = createdTeam.team.teamId;
  const teamUpdateFile = await requestFile("team-update.json",{idempotencyKey:`cli-team-update-${nonce}`,expectedVersion:1,name:`CLI updated ${nonce}`,role:"member",memberPrincipalIds:[fixture.accounts.bob.principalId]});
  const updatedTeam = await run(["team-update",teamId,teamUpdateFile]);
  ensure(updatedTeam.team.version === 2 && updatedTeam.team.role === "member","Team update failed");
  const staleTeamFile = await requestFile("team-stale.json",{idempotencyKey:`cli-team-stale-${nonce}`,expectedVersion:1,name:"Stale",role:"auditor",memberPrincipalIds:[]});
  await run(["team-update",teamId,staleTeamFile],{expectFailure:true});
  const archiveTeamFile = await requestFile("team-archive.json",{idempotencyKey:`cli-team-archive-${nonce}`,expectedVersion:2,reason:"Recette CLI terminée"});
  const archivedTeam = await run(["team-archive",teamId,archiveTeamFile]);
  ensure(archivedTeam.team.state === "archived" && archivedTeam.team.version === 3,"Team archive failed");
  const teamHistory = await run(["team-history",teamId]);
  ensure(teamHistory.events.length === 3 && teamHistory.events[0].record.name === createdTeam.team.name,"Team immutable history lost its original version");
  const teams = await run(["teams"]);
  const policyManagement = await run(["policy-management"]);
  ensure(policyManagement.mode === "managed" && policyManagement.currentPolicySetId === policyManagement.managedPolicySetId, "Managed policy authority mismatch");
  ensure(teams.policyManagement.currentPolicySetId === policyManagement.currentPolicySetId, "Teams and policy authority disagree");
  const policyPreviewFile = path.join(temporary, "policy-preview.json");
  const policyPreview = await run(["policy-preview", policyManagement.currentPolicySetId, "--output", policyPreviewFile]);
  ensure(policyPreview.preview.expectedPolicySetId === policyManagement.currentPolicySetId && policyPreview.preview.proposedPolicySetId === policyManagement.currentPolicySetId, "Policy preview did not preserve explicit target");
  const policyApplied = await run(["policy-apply", policyPreviewFile]);
  ensure(policyApplied.receipt.status === "applied" && policyApplied.receipt.acceptedStepIds.length === 0, "Policy-only no-op created a Step or was refused");
  const policyReplay = await run(["policy-apply", policyPreviewFile]);
  ensure(JSON.stringify(policyReplay.receipt) === JSON.stringify(policyApplied.receipt), "Policy retry did not return exact receipt");
  ensure(fixture.initialPolicySetId !== policyManagement.currentPolicySetId, "Fixture member grant did not produce a new managed root");
  const customPolicyFile = path.join(temporary, "policy-custom.json");
  await run(["policy-preview", fixture.initialPolicySetId, "--output", customPolicyFile]);
  const changedPolicy = await run(["policy-apply", customPolicyFile]);
  ensure(changedPolicy.receipt.status === "applied", "Stored policy change failed");
  const customPolicy = await run(["policy-management"]);
  ensure(customPolicy.mode === "custom" && customPolicy.managedPolicySetId === policyManagement.currentPolicySetId, "Custom mode lost managed root provenance");
  const blockedTeamFile = await requestFile("team-custom-blocked.json", {idempotencyKey:`cli-custom-blocked-${nonce}`,expectedVersion:0,name:"Refused custom mode",role:"auditor",memberPrincipalIds:[]});
  await run(["team-create",blockedTeamFile],{expectFailure:true});
  const restoredPolicyFile = path.join(temporary, "policy-restore.json");
  await run(["policy-preview", customPolicy.managedPolicySetId, "--output", restoredPolicyFile]);
  await run(["policy-apply", restoredPolicyFile]);
  const contentRules = {schemaVersion:"layrs.space-policy-rules.v2",rules:[],contentPolicies:[{principalIds:[fixture.accounts.bob.principalId],default:"deny",rules:[{effect:"allow",selector:{kind:"subtree",path:"docs"}}]}]};
  const rulesFile = await requestFile("content-rules.json", contentRules);
  const rulesPreviewFile = path.join(temporary,"rules-preview.json");
  const rulesPreview = await run(["policy-preview-rules",rulesFile,"--output",rulesPreviewFile]);
  ensure(rulesPreview.preview.inlineRules.rules.contentPolicies[0].principalIds[0] === fixture.accounts.bob.principalId && rulesPreview.preview.request.products.length === 1,"Rules preview changed principal or created extra Products");
  const beforeRules = await run(["policy-management"]);
  ensure(beforeRules.mode === "managed" && beforeRules.currentPolicySetId === policyManagement.currentPolicySetId,"Rules preview mutated server policy");
  const rulesApplied = await run(["policy-apply",rulesPreviewFile]);
  const rulesReplay = await run(["policy-apply",rulesPreviewFile]);
  ensure(rulesApplied.receipt.status === "applied" && JSON.stringify(rulesApplied.receipt) === JSON.stringify(rulesReplay.receipt),"Inline rules retry differs");
  const rulesActive = await run(["policy-management"]);
  ensure(rulesActive.mode === "custom" && rulesActive.currentPolicySetId === rulesPreview.preview.proposedPolicySetId,"Inline rules did not activate exact preview");
  const rulesRestoreFile = path.join(temporary,"rules-restore.json");
  await run(["policy-preview",rulesActive.managedPolicySetId,"--output",rulesRestoreFile]);
  await run(["policy-apply",rulesRestoreFile]);
  const unknownRules = await requestFile("rules-unknown.json",{...contentRules,unexpected:true});
  await run(["policy-preview-rules",unknownRules],{expectFailure:true});
  const badPathRules = await requestFile("rules-badpath.json",{...contentRules,contentPolicies:[{...contentRules.contentPolicies[0],rules:[{effect:"allow",selector:{kind:"subtree",path:"../private"}}]}]});
  await run(["policy-preview-rules",badPathRules],{expectFailure:true});
  await run(["policy-preview-rules",oversized],{expectFailure:true});
  ensure((await run(["policy-management"])).mode === "managed", "Restoring exact managed policy failed");
  const observationFile = await requestFile("observation.json", {step:fixture.stepId,observedAt:"2020-02-02T03:04:05Z",reason:"manual_snapshot",idempotencyKey:`cli-observation-${nonce}`});
  const observation = await run(["observe",observationFile]);
  const observationReplay = await run(["observe",observationFile]);
  ensure(observation.receipt.acceptedObservationIds.length === 1 && observation.receipt.acceptedStepIds.length === 0, "Observation assertion was not admitted independently");
  ensure(JSON.stringify(observation.receipt) === JSON.stringify(observationReplay.receipt), "Observation retry differs");
  const observedHistory = await run(["history",fixture.stepId]);
  const assertions = observedHistory.observations[fixture.stepId].filter(item => item.provenance === "client_assertion");
  const assertion = assertions.find(item => item.observation.observationId === observation.receipt.acceptedObservationIds[0]);
  ensure(assertion && assertion.observation.observedAt === "2020-02-02T03:04:05Z" && assertion.receivedAt !== assertion.observation.observedAt, "History confused declared time with server receipt");
  const forgedObservation = await requestFile("observation-forged.json", {step:fixture.stepId,observedAt:"2020-01-02T03:04:05Z",reason:"manual_snapshot",idempotencyKey:`cli-forged-${nonce}`,actorId:"actor:forged"});
  await run(["observe",forgedObservation],{expectFailure:true});
  ensure(teams.teams.some(team => team.teamId === teamId && team.state === "archived"),"Archived team is absent from audit list");
  const budget = await run(["storage-budget", fixture.spaceId]);
  ensure(budget.schemaVersion === "layrs.storage-budget.v1" && budget.databaseBytes > 0, "Storage budget route is missing or invalid");
  const metrics = await run(["metrics"]);
  ensure(metrics.http.completed > 0 && metrics.http.statusClasses.length === 5, "Admin metrics are absent or unbounded");
  const chooseRequest = await requestFile("choose-request.json",{layer:fixture.layerId,chosenHeads:[],reason:"Retirer les têtes du projet jetable puis les rétablir"});
  const candidates = await run(["publish-candidates"]);
  ensure(candidates.receipt.status === "applied" && !candidates.receipt.viewResult,"Candidate admission moved the remote View");
  const invalidChoose = await requestFile("invalid-choose.json",{layer:fixture.layerId,chosenHeads:[],reason:"Invalid closed request",unexpected:true});
  await run(["choose-preview",invalidChoose],{expectFailure:true});
  const chooseFile = path.join(temporary,"choose-preview.json");
  const chosenPreview = await run(["choose-preview",chooseRequest,"--output",chooseFile]);
  ensure(chosenPreview.preview.layerId === fixture.layerId && chosenPreview.preview.chosenHeads.length === 0 && chosenPreview.preview.discardedHeads.includes(fixture.stepId),"Choose preview changed the selected Layer or discarded heads");
  ensure(JSON.stringify(JSON.parse(await readFile(chooseFile,"utf8")))===JSON.stringify(chosenPreview.preview),"Saved preview differs from displayed intent");
  const chosen = await run(["choose-apply",chooseFile]);
  const chosenReplay = await run(["choose-apply",chooseFile]);
  ensure(JSON.stringify(chosen) === JSON.stringify(chosenReplay),"Choose retry changed the decision receipt");
  const restoreChooseRequest = await requestFile("choose-restore-request.json",{layer:fixture.layerId,chosenHeads:[fixture.stepId],reason:"Rétablir les têtes du projet jetable"});
  const restoreChoose = await run(["choose-preview",restoreChooseRequest]);
  ensure(restoreChoose.preview.previousHeads.length === 0,"Choose did not clear published heads");
  await run(["choose-apply",await requestFile("choose-restore.json",restoreChoose)]);
  ensure((await logout()).status === "signedOut", "CLI logout did not complete");
  authenticated = false;
  await run(["whoami"], { expectFailure: true });
  return { status: "passed", commands, commentReply: true, idempotentComment: true, requestSizeGuard: true, tlsVerified: true, storageBudget: true, elapsedMs: Date.now() - started };
}

try {
  const result = await main();
  console.log(JSON.stringify(result));
} catch (error) {
  // Raw child output, HTTP bodies, fixture data and stack traces are withheld.
  console.error(`Native CLI remote test failed: ${error instanceof Error ? error.message : "unknown failure"}`);
  process.exitCode = 1;
} finally {
  if (authenticated && logout) {
    try { await logout(); } catch { console.error("Test session cleanup failed; retry remote logout for this fixture origin."); process.exitCode = 1; }
  }
  if (temporary) {
    // Delete only the unique directory created by this invocation.
    const target = path.resolve(temporary);
    const parent = path.resolve(tmpdir());
    if (path.dirname(target) === parent && path.basename(target).startsWith("layrs-cli-remote-")) {
      try { await rm(target, { recursive: true, force: true }); }
      catch { console.error("Temporary CLI test files could not be removed."); process.exitCode = 1; }
    } else {
      console.error("Temporary cleanup target failed its path boundary check.");
      process.exitCode = 1;
    }
  }
}
