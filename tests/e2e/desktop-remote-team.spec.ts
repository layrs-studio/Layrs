import { expect, test } from "@playwright/test";
import { resolve } from "node:path";
import { hash, installWorkStub, projectPath, remoteActions } from "./fixtures/desktopRendererStub";

const policyModule = `/@fs/${resolve("packages/ui/src/team/PolicyChangeDialog.tsx").replaceAll("\\", "/")}`;
const policyHarnessModule = `/@fs/${resolve("tests/e2e/fixtures/desktopPolicyReplayHarness.tsx").replaceAll("\\", "/")}`;

declare global {
  interface Window {
    __layrsPolicyReplay?: { remount: () => void; state: () => { committed: boolean; applied: string[]; previews: number }; dispose: () => void; targetPolicy: string };
    __layrsAccessReplay?: { remount: () => void; state: () => { committed: boolean; acknowledged: boolean; actions: string[]; applied: string[] }; dispose: () => void };
  }
}

test("an instance administrator can open administration after project access is revoked", async ({ page }) => {
  await installWorkStub(page, {
    origin: "https://layrs.example.test",
    signedIn: true,
    bootstrapDenied: true,
    instanceAdmin: true,
  });
  await page.goto(projectPath("team"));
  await expect(page.getByText("Le projet distant n’a pas pu être lu")).toBeVisible();
  await page.getByRole("button", { name: "Administration de l’instance" }).click();
  await expect(page.getByRole("heading", { name: "Administration de l’instance" })).toBeVisible();
  await expect(page.getByText("Alice Démo")).toBeVisible();
  const actions = await remoteActions(page);
  expect(actions.some((action) => action.action === "administration" && (action.command as { action: string }).action === "listAccounts")).toBe(true);
});

test("a non-administrator cannot read instance accounts through the admin route", async ({ page }) => {
  await installWorkStub(page, {
    origin: "https://layrs.example.test",
    signedIn: true,
    bootstrapDenied: true,
    instanceAdmin: false,
  });
  await page.goto(projectPath("admin"));
  await expect(page.getByText("Réservé aux administrateurs de l’instance")).toBeVisible();
  const actions = await remoteActions(page);
  expect(actions.some((action) => action.action === "administration" && ["listAccounts", "createAccount", "listLenses", "installLens", "storageBudget", "metrics"].includes((action.command as { action: string }).action))).toBe(false);
});

test("an external credential rotation cannot dispatch an administrator mutation from the old view", async ({ page }) => {
  await installWorkStub(page, { origin: "https://layrs.example.test", signedIn: true, bootstrapDenied: true, instanceAdmin: true });
  await page.goto(projectPath("admin"));
  await expect(page.getByText("Alice Démo")).toBeVisible();
  await page.evaluate(() => window.__layrsRotatePrincipal?.("principal:bob"));
  await page.getByRole("button", { name: "Créer un compte" }).click();
  const form = page.getByRole("dialog", { name: "Créer un compte" });
  await form.getByRole("textbox", { name: "Nom affiché" }).fill("Compte interdit");
  await form.getByRole("textbox", { name: "Identifiant de connexion" }).fill("blocked@example.test");
  await form.getByRole("textbox", { name: "Mot de passe initial" }).fill("a-secure-test-password");
  await form.getByRole("button", { name: "Créer le compte" }).click();
  await expect(form.getByText(/compte connecté à ce serveur a changé/)).toBeVisible();
  const state = await page.evaluate(() => ({ calls: window.__layrsInvocations, dispatched: window.__layrsDispatched }));
  const attempted = state.calls?.find((call) => call.command === "native_remote" && (call.args?.request as { action?: { action?: string; command?: { action?: string } } })?.action?.command?.action === "createAccount");
  expect((attempted?.args?.request as { expectedPrincipalId?: string })?.expectedPrincipalId).toBe("principal:alice");
  expect(state.dispatched).not.toContain("administration:createAccount");
});

test("policy failures distinguish refusal, pre-dispatch rejection and earlier uncertain apply", async ({ page }) => {
  await page.goto(projectPath("team"));
  const cases = await page.evaluate(async (moduleUrl) => {
    const { normalizeTeamError } = await import("/src/pages/remoteTeam.ts");
    const { policyApplyFailure } = await import(/* @vite-ignore */ moduleUrl);
    const refused = normalizeTeamError("remote_forbidden: remote HTTP status 403: invalid_capability");
    const changed = normalizeTeamError("remote_principal_changed: Recheck the Desktop session before continuing");
    const lost = normalizeTeamError("remote_transport_failed: remote transport failed");
    return {
      refusedStatus: refused.status,
      refused: policyApplyFailure(refused, false),
      preDispatch: policyApplyFailure(changed, false),
      lost: policyApplyFailure(lost, false),
      afterLost: policyApplyFailure(changed, true),
    };
  }, policyModule);
  expect(cases.refusedStatus).toBe(403);
  expect(cases.refused.uncertain).toBe(false);
  expect(cases.preDispatch.uncertain).toBe(false);
  expect(cases.preDispatch.message).toContain("demande n’a pas été envoyée");
  expect(cases.lost.uncertain).toBe(true);
  expect(cases.afterLost.uncertain).toBe(true);
  expect(cases.afterLost.message).toContain("tentative précédente reste sans réponse");
  const preDispatch = await page.evaluate(async () => {
    const { normalizeTeamError } = await import("/src/pages/remoteTeam.ts");
    return [
      normalizeTeamError("remote_identity_check_failed: Sign in to this server first with remote login").dispatched,
      normalizeTeamError("remote_lock_unavailable: native remote lock is unavailable before dispatch").dispatched,
      normalizeTeamError("native_remote_journal_io: write failed").dispatched,
    ];
  });
  expect(preDispatch).toEqual([false, false, undefined]);
});

test("a lost policy response stays uncertain for the exact native intent after reopening", async ({ page }) => {
  await page.goto(projectPath("team"));
  const intent = {
    origin: "https://layrs.example.test", principalId: "principal:alice", spaceId: "space:demo", worktreeId: "worktree:demo",
    expectedPolicySetId: hash("a"), expectedAuthorizationEpoch: 3, proposedPolicySetId: hash("b"),
    request: { clientId: "client:demo", idempotencyKey: "policy-intent-1", requestDigest: hash("c") },
  };
  await page.evaluate(async ({ preview, moduleUrl }) => {
    const { rememberPolicyUncertain } = await import(/* @vite-ignore */ moduleUrl);
    rememberPolicyUncertain(preview);
  }, { preview: intent, moduleUrl: policyModule });
  await page.reload();
  const reopened = await page.evaluate(async ({ preview, moduleUrl }) => {
    const { policyWasUncertain, policyApplyFailure, clearPolicyUncertain } = await import(/* @vite-ignore */ moduleUrl);
    const same = policyWasUncertain(preview);
    const other = policyWasUncertain({ ...preview, request: { ...preview.request, idempotencyKey: "policy-intent-2" } });
    const refused = policyApplyFailure({ status: 403, code: "invalid_capability" }, same);
    clearPolicyUncertain(preview);
    return { same, other, refused, cleared: policyWasUncertain(preview) };
  }, { preview: intent, moduleUrl: policyModule });
  expect(reopened.same).toBe(true);
  expect(reopened.other).toBe(false);
  expect(reopened.refused.uncertain).toBe(true);
  expect(reopened.refused.message).toContain("tentative précédente reste sans réponse");
  expect(reopened.cleared).toBe(false);
});

test("a committed policy with a lost answer reopens the exact pending preview against newer rules", async ({ page }) => {
  await page.goto(projectPath("team"));
  await page.evaluate(async (moduleUrl) => {
    const { mountPolicyReplayHarness } = await import(/* @vite-ignore */ moduleUrl);
    const host = document.createElement("div");
    host.id = "policy-replay-harness";
    document.body.append(host);
    window.__layrsPolicyReplay = mountPolicyReplayHarness(host);
  }, policyHarnessModule);
  const host = page.locator("#policy-replay-harness");
  const card = host.locator(".settings-card").filter({ hasText: "Appliquer une politique enregistrée" });
  const target = await page.evaluate(() => window.__layrsPolicyReplay?.targetPolicy);
  await card.getByRole("textbox", { name: "Référence de la politique" }).fill(target!);
  await card.getByRole("button", { name: "Prévisualiser" }).click();
  let dialog = host.getByRole("dialog", { name: "Appliquer une politique enregistrée" });
  await dialog.getByRole("button", { name: "Appliquer cette politique" }).click();
  await expect(dialog.getByText(/La réponse du serveur n’est pas arrivée/)).toBeVisible();
  await page.evaluate(() => window.__layrsPolicyReplay?.remount());
  await card.getByRole("textbox", { name: "Référence de la politique" }).fill(target!);
  await card.getByRole("button", { name: "Prévisualiser" }).click();
  dialog = host.getByRole("dialog", { name: "Appliquer une politique enregistrée" });
  await expect(dialog.getByText(/Une demande identique a déjà été envoyée/)).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Réessayer ce même aperçu" })).toBeVisible();
  await dialog.getByRole("button", { name: "Réessayer ce même aperçu" }).click();
  await expect(host.getByText("Politique appliquée")).toBeVisible();
  const state = await page.evaluate(() => window.__layrsPolicyReplay?.state());
  expect(state?.committed).toBe(true);
  expect(state?.previews).toBe(2);
  expect(state?.applied).toHaveLength(2);
  expect(state?.applied[1]).toBe(state?.applied[0]);
  await page.evaluate(() => window.__layrsPolicyReplay?.dispose());
});

test("a lost file-policy addition recovers its native intent after the current rules become a replacement", async ({ page }) => {
  await page.goto(projectPath("team"));
  await page.evaluate(async (moduleUrl) => {
    const { mountAccessReplayHarness } = await import(/* @vite-ignore */ moduleUrl);
    const host = document.createElement("div");
    host.id = "access-replay-harness";
    document.body.append(host);
    window.__layrsAccessReplay = mountAccessReplayHarness(host);
  }, policyHarnessModule);
  const host = page.locator("#access-replay-harness");
  await host.getByRole("button", { name: "readme.txt" }).click();
  await host.getByRole("button", { name: "Prévisualiser le changement" }).click();
  let dialog = host.getByRole("dialog", { name: "Confirmer le changement d’accès" });
  await dialog.getByRole("button", { name: "Appliquer ce changement" }).click();
  await expect(dialog.getByText(/La réponse du serveur n’est pas arrivée/)).toBeVisible();
  await page.evaluate(() => window.__layrsAccessReplay?.remount());
  await host.getByRole("button", { name: "Reprendre la modification en attente" }).click();
  dialog = host.getByRole("dialog", { name: "Reprendre la modification en attente" });
  await expect(dialog.getByText(/aucune nouvelle règle n’est préparée/)).toBeVisible();
  await dialog.getByRole("button", { name: "Réessayer ce même aperçu" }).click();
  await expect(host.getByText("Politique appliquée")).toBeVisible();
  const state = await page.evaluate(() => window.__layrsAccessReplay?.state());
  expect(state?.committed).toBe(true);
  expect(state?.acknowledged).toBe(true);
  expect(state?.actions).toContain("previewPolicyRules");
  expect(state?.actions.filter((action) => action === "readPendingPolicy")).toHaveLength(2);
  expect(state?.actions).not.toContain("previewReplacePolicyRules");
  expect(state?.applied).toHaveLength(2);
  expect(state?.applied[1]).toBe(state?.applied[0]);
  await page.evaluate(() => window.__layrsAccessReplay?.dispose());
});

test("a rejected recovery releases the pending file policy and permits a fresh edit without remount", async ({ page }) => {
  await page.goto(projectPath("team"));
  await page.evaluate(async (moduleUrl) => {
    const { mountAccessReplayHarness } = await import(/* @vite-ignore */ moduleUrl);
    const host = document.createElement("div");
    host.id = "access-replay-harness";
    document.body.append(host);
    window.__layrsAccessReplay = mountAccessReplayHarness(host, "rejected");
  }, policyHarnessModule);
  const host = page.locator("#access-replay-harness");
  await host.getByRole("button", { name: "readme.txt" }).click();
  await host.getByRole("button", { name: "Prévisualiser le changement" }).click();
  let dialog = host.getByRole("dialog", { name: "Confirmer le changement d’accès" });
  await dialog.getByRole("button", { name: "Appliquer ce changement" }).click();
  await expect(dialog.getByText(/La réponse du serveur n’est pas arrivée/)).toBeVisible();
  await page.evaluate(() => window.__layrsAccessReplay?.remount());
  await host.getByRole("button", { name: "Reprendre la modification en attente" }).click();
  dialog = host.getByRole("dialog", { name: "Reprendre la modification en attente" });
  await dialog.getByRole("button", { name: "Réessayer ce même aperçu" }).click();
  await expect(dialog.getByText(/Le serveur a refusé la modification/)).toBeVisible();
  await dialog.getByRole("button", { name: "Annuler" }).click();
  await host.getByRole("button", { name: "readme.txt" }).click();
  await host.getByRole("button", { name: "Prévisualiser le changement" }).click();
  await expect(host.getByRole("dialog", { name: "Confirmer le changement d’accès" })).toBeVisible();
  const state = await page.evaluate(() => window.__layrsAccessReplay?.state());
  expect(state?.actions).toContain("previewReplacePolicyRules");
  await page.evaluate(() => window.__layrsAccessReplay?.dispose());
});
