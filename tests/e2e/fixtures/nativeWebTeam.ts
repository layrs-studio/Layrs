import { expect, type Page } from "@playwright/test";
import { randomBytes } from "node:crypto";
import { fixtureHttp, type SharedFixture } from "./nativeSharedClients";
import { webSignIn, webSignOut } from "./nativeWebAccess";

/*
 * Team, access and administration steps of the real native Web recipe (lot
 * L5). They start and end signed in as Alice (instance administrator and
 * project owner) and leave the project under team-managed rules.
 */

type Fixture = SharedFixture & { webhookUrl: string };
type Credentials = { login: string; password: string };

/** Hash navigation keeps the in-memory session (a reload would sign out). */
export async function openHash(page: Page, path: string) {
  await page.evaluate((hash) => {
    window.location.hash = hash;
  }, path);
}

async function readable(fixture: Fixture, account: Credentials, path: string) {
  const login = await fixtureHttp(fixture, "POST", "/v3/auth/login", { login: account.login, password: account.password });
  expect(login.status).toBe(200);
  const token = login.data.access_token;
  try {
    const base = `/v3/spaces/${fixture.spaceId}`;
    const bootstrap = await fixtureHttp(fixture, "GET", `${base}/bootstrap`, undefined, token);
    expect(bootstrap.status).toBe(200);
    const listing = await fixtureHttp(fixture, "POST", `${base}/projection`, {
      stepId: fixture.stepId,
      expectedPolicySetId: bootstrap.data.currentPolicySetId,
      expectedAuthorizationEpoch: bootstrap.data.authorizationEpoch,
      cursor: null,
      limit: 100,
    }, token);
    expect(listing.status).toBe(200);
    return listing.data.entries.find((entry: { path: string }) => entry.path === path).contentReadable as boolean;
  } finally {
    await fixtureHttp(fixture, "POST", "/v3/auth/logout", {}, token);
  }
}

async function acknowledgeSecret(page: Page, title: string) {
  const dialog = page.getByRole("dialog", { name: title });
  await expect(dialog).toBeVisible();
  const field = dialog.getByLabel("Secret", { exact: true });
  await expect(field).toHaveAttribute("type", "password");
  const secret = await field.inputValue();
  expect(secret.length).toBeGreaterThanOrEqual(32);
  await dialog.getByRole("button", { name: "J’ai enregistré le secret" }).click();
  await expect(dialog).toBeHidden();
  // Shown once: the value is gone from the document once acknowledged.
  expect(await page.content()).not.toContain(secret);
}

/** Instance accounts list (replaces the former "Comptes et automatisations" panel). */
export async function webInstanceAccounts(page: Page) {
  await openHash(page, "/admin/accounts");
  const accounts = page.getByRole("list", { name: "Comptes de l’instance" });
  await expect(accounts.getByText("Bob — relecteur test", { exact: true })).toBeVisible();
}

export async function webTeamAndAccess(page: Page, fixture: Fixture) {
  const project = encodeURIComponent(fixture.spaceId);
  const alice = fixture.accounts.alice;
  const bob = fixture.accounts.bob;
  const carole = { login: `carole-${randomBytes(4).toString("hex")}@example.test`, password: `ui-test-${randomBytes(12).toString("hex")}` };

  console.log("[team] instance administration: accounts, service account, storage, server state");
  await webInstanceAccounts(page);
  await page.getByRole("button", { name: "Créer un compte", exact: true }).click();
  const create = page.getByRole("dialog", { name: "Créer un compte" });
  await create.getByLabel("Nom affiché", { exact: true }).fill("Carole — invitée test");
  await create.getByLabel("Identifiant de connexion", { exact: true }).fill(carole.login);
  await create.getByLabel("Mot de passe initial", { exact: true }).fill(carole.password);
  await create.getByRole("button", { name: "Créer le compte", exact: true }).click();
  await expect(create).toBeHidden();
  await expect(page.getByRole("list", { name: "Comptes de l’instance" }).getByText("Carole — invitée test", { exact: true })).toBeVisible();

  await openHash(page, "/admin/services");
  await page.getByRole("button", { name: "Créer un compte de service", exact: true }).click();
  const service = page.getByRole("dialog", { name: "Créer un compte de service" });
  await service.getByLabel("Nom du compte de service", { exact: true }).fill("CI recette");
  await service.getByLabel("Nom du jeton", { exact: true }).fill("runner");
  await service.getByRole("button", { name: "Créer et afficher le jeton" }).click();
  await acknowledgeSecret(page, "Jeton du compte de service");
  await expect(page.getByRole("list", { name: "Comptes de service" }).getByText("CI recette", { exact: true })).toBeVisible();

  await openHash(page, "/admin/storage");
  await expect(page.getByText("État général", { exact: true })).toBeVisible();
  await openHash(page, "/admin/status");
  await expect(page.getByRole("table", { name: "Réponses HTTP par classe" })).toBeVisible();

  console.log("[team] members, teams and invitation by name");
  await openHash(page, `/projects/${project}/team`);
  const members = page.getByRole("list", { name: "Membres du projet" });
  await expect(members.getByRole("listitem").filter({ hasText: "Bob — relecteur test" })).toContainText("Membre");
  await page.getByRole("tab", { name: /Équipes/ }).click();
  await page.getByRole("button", { name: "Nouvelle équipe", exact: true }).click();
  const editor = page.getByRole("dialog", { name: "Nouvelle équipe" });
  await editor.getByLabel("Nom de l’équipe", { exact: true }).fill("Relecture");
  await editor.getByRole("checkbox", { name: "Bob — relecteur test", exact: true }).check();
  await editor.getByRole("button", { name: "Enregistrer l’équipe", exact: true }).click();
  await expect(editor).toBeHidden();
  await page.getByRole("button", { name: "Historique de l’équipe Relecture" }).click();
  const history = page.getByRole("dialog", { name: "Historique · Relecture" });
  await expect(history.getByText(/Version 1 · Relecture/)).toBeVisible();
  await history.getByRole("button", { name: "Fermer" }).click();

  // The invitation is created last: any later rights change would make it stale.
  await page.getByRole("tab", { name: /Invitations/ }).click();
  await page.getByRole("button", { name: "Inviter une personne", exact: true }).click();
  const invite = page.getByRole("dialog", { name: "Inviter une personne" });
  await invite.getByRole("button", { name: /Carole — invitée test/ }).click();
  await invite.getByRole("button", { name: "Créer l’invitation", exact: true }).click();
  await expect(invite).toBeHidden();
  await expect(page.getByRole("list", { name: "Invitations du projet" }).getByRole("listitem").filter({ hasText: "Carole — invitée test" })).toContainText("En attente");

  console.log("[team] invited account: inbox acceptance, then its own file rights");
  await webSignOut(page);
  await webSignIn(page, carole);
  await openHash(page, "/inbox");
  const pending = page.getByRole("list", { name: "Invitations en attente" });
  await pending.getByRole("button", { name: "Accepter", exact: true }).click();
  await expect(page.getByRole("list", { name: "Invitations traitées" }).getByText("Acceptée", { exact: true })).toBeVisible();
  await openHash(page, `/projects/${project}/access`);
  await page.getByRole("button", { name: "Déplier docs" }).click();
  const own = page.getByRole("list", { name: "Chemins du projet et droits de votre compte" });
  await expect(own.getByRole("listitem").filter({ hasText: "readme.txt" }).first()).toContainText("Écriture");
  await expect(page.getByText("Demandez à un administrateur du projet", { exact: false })).toHaveCount(0);
  await webSignOut(page);
  await webSignIn(page, alice);

  console.log("[team] protections with unsaved-changes guard, webhook with one-time secret");
  await openHash(page, `/projects/${project}/settings/protections`);
  const approvals = page.getByLabel("Approbations humaines requises", { exact: true });
  await expect(approvals).toBeVisible();
  const next = (await approvals.inputValue()) === "0" ? "1" : "0";
  await approvals.fill(next);
  page.once("dialog", (dialog) => void dialog.dismiss());
  await page.getByRole("navigation", { name: "Sections des réglages" }).getByRole("link", { name: /Webhooks/ }).click();
  await expect(approvals).toHaveValue(next);
  const ci = page.getByText("Recette actuelle conservée par référence", { exact: true });
  const hadCI = await ci.isVisible();
  if (hadCI) {
    // The server keeps the recipe by reference: saving requires it again, or an explicit removal.
    await expect(page.getByText("Collez la recette exacte actuelle", { exact: false })).toBeAttached();
    await page.getByRole("checkbox", { name: "Exiger une preuve de validation pour la capture exacte" }).uncheck();
  }
  await page.getByRole("button", { name: "Enregistrer la protection", exact: true }).click();
  const confirmProtection = page.getByRole("dialog", { name: "Enregistrer la protection" });
  if (hadCI) await expect(confirmProtection).toContainText("La validation automatique actuelle est retirée.");
  await confirmProtection.getByRole("button", { name: "Enregistrer", exact: true }).click();
  await expect(confirmProtection).toBeHidden();
  await expect(page.getByText(/^Configurée le /)).toBeVisible();

  await page.getByRole("navigation", { name: "Sections des réglages" }).getByRole("link", { name: /Webhooks/ }).click();
  await page.getByRole("button", { name: "Nouveau webhook", exact: true }).click();
  const hook = page.getByRole("dialog", { name: "Nouveau webhook" });
  await hook.getByLabel("Adresse HTTPS du récepteur", { exact: true }).fill(fixture.webhookUrl);
  await hook.getByRole("button", { name: "Créer le webhook", exact: true }).click();
  await acknowledgeSecret(page, "Secret de signature du webhook");
  const hooks = page.getByRole("list", { name: "Webhooks du projet" });
  await expect(hooks.getByText(fixture.webhookUrl, { exact: true })).toBeVisible();
  await expect(hooks.getByText("Actif", { exact: true })).toBeVisible();

  console.log("[team] file exception through preview and confirmation, then return to team-managed rules");
  expect(await readable(fixture, bob, "docs/readme.txt")).toBe(true);
  await openHash(page, `/projects/${project}/access`);
  await page.getByRole("list", { name: "Personne dont afficher les accès" }).getByRole("button", { name: /Bob — relecteur test/ }).click();
  await page.getByRole("button", { name: "Déplier docs" }).click();
  const tree = page.getByRole("list", { name: "Chemins du projet et droits de Bob — relecteur test" });
  await tree.getByRole("button", { name: "readme.txt", exact: true }).click();
  await expect(page.getByText("Lecture et écriture").first()).toBeVisible();
  await page.getByRole("radio", { name: /Interdit/ }).check();
  await page.getByRole("button", { name: "Prévisualiser le changement", exact: true }).click();
  const change = page.getByRole("dialog", { name: "Confirmer le changement d’accès" });
  await expect(change).toContainText("Avant : Lecture et écriture. Après : Interdit.");
  await expect(change).toContainText("Le projet passera en règles personnalisées");
  await change.getByRole("button", { name: "Appliquer ce changement", exact: true }).click();
  await expect(change).toBeHidden();
  await expect(tree.getByRole("listitem").filter({ hasText: "readme.txt" }).first()).toContainText("Refus prioritaire");
  expect(await readable(fixture, bob, "docs/readme.txt")).toBe(false);

  await openHash(page, `/projects/${project}/settings/advanced`);
  await page.getByRole("button", { name: "Préparer le retour", exact: true }).click();
  const restore = page.getByRole("dialog", { name: "Revenir à la gestion par équipes" });
  await restore.getByRole("button", { name: "Rétablir la gestion par équipes", exact: true }).click();
  await expect(restore).toBeHidden();
  await expect(page.getByText("Équipes et rôles", { exact: true })).toBeVisible();
  expect(await readable(fixture, bob, "docs/readme.txt")).toBe(true);
  console.log("[team] restriction enforced by the server, then removed; project back to team-managed rules");
}
