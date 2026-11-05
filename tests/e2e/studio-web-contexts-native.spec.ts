import { test, expect } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { webSignIn, webSignOut } from "./fixtures/nativeWebAccess";
import { fixtureHttp } from "./fixtures/nativeSharedClients";

test("context selection isolates company, personal and organisation projects on real HTTPS", async ({ page }, testInfo) => {
  test.setTimeout(180_000);
  const fixture = JSON.parse(await readFile(process.env.LAYRS_NATIVE_UI_FIXTURE!, "utf8"));
  await page.goto("/#/projects");
  await webSignIn(page, fixture.accounts.alice);
  const contexts = await fixtureHttp(fixture, "GET", "/v3/contexts");
  expect(contexts.status).toBe(200);
  const company = contexts.data.contexts.find((item: any) => item.kind === "company");
  const personal = contexts.data.contexts.find((item: any) => item.kind === "personal");
  const selector = page.getByRole("button", { name: /^Organisation :/ });
  const chooseOrganisation = async (name: string) => {
    await selector.click();
    await page.getByRole("menuitemradio", { name, exact: true }).click();
  };
  await page.getByRole("button", { name: "Projets personnels", exact: true }).click();
  await page.getByRole("button", { name: "Nouveau projet", exact: true }).click();
  await expect(page.getByLabel("Nom de l’organisation", { exact: true })).toHaveCount(0);
  await page.getByLabel("Nom du projet", { exact: true }).fill("Notes personnelles UI");
  await page.getByRole("button", { name: "Créer le projet", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Fichiers", exact: true })).toBeVisible();
  const personalCatalog = await fixtureHttp(fixture, "GET", `/v3/spaces?contextId=${personal.id}`);
  const notes = personalCatalog.data.spaces.find((item: any) => item.name === "Notes personnelles UI");
  expect(notes).toBeTruthy();
  const share = await fixtureHttp(fixture, "POST", `/v3/spaces/${notes.spaceId}/members`, { principalId: fixture.accounts.bob.principalId, role: "member" });
  expect(share.status).toBeGreaterThanOrEqual(400);

  await chooseOrganisation(company.name);
  await expect(page.getByRole("heading", { name: "Vos projets", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: /Notes personnelles UI/ })).toHaveCount(0);
  await selector.click();
  await page.getByRole("menuitem", { name: "Créer une nouvelle organisation", exact: true }).click();
  await page.getByLabel("Nom de l’organisation", { exact: true }).fill("Équipe produit UI");
  const creation = page.waitForResponse(response => new URL(response.url()).pathname === "/v3/organisations" && response.request().method() === "POST");
  await page.getByRole("button", { name: "Créer l’organisation", exact: true }).click();
  const creationResponse = await creation;
  // The app navigates once the organisation exists, and Chromium may then drop
  // the response body: read it only to explain a failure.
  const creationStatus = creationResponse.status();
  const creationDetail = creationStatus < 300 ? "" : await creationResponse.text().catch(() => "response body unavailable");
  expect(creationStatus, creationDetail).toBeLessThan(300);
  await expect(selector).toContainText("Équipe produit UI");
  await page.getByRole("button", { name: "Nouveau projet", exact: true }).click();
  await page.getByLabel("Nom du projet", { exact: true }).fill("Projet organisation UI");
  await page.getByRole("button", { name: "Créer le projet", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Fichiers", exact: true })).toBeVisible();
  const updated = await fixtureHttp(fixture, "GET", "/v3/contexts");
  const org = updated.data.contexts.find((item: any) => item.name === "Équipe produit UI");
  const orgCatalog = await fixtureHttp(fixture, "GET", `/v3/spaces?contextId=${org.id}`);
  expect(orgCatalog.data.spaces.map((item: any) => item.name)).toEqual(["Projet organisation UI"]);
  const orgProject = orgCatalog.data.spaces[0];
  await page.goto(`/#/projects/${encodeURIComponent(orgProject.spaceId)}/team/teams`);
  const teams = page.getByRole("region", { name: "Équipes de l’organisation", exact: true });
  await teams.getByRole("button", { name: "Créer une équipe de l’organisation", exact: true }).click();
  const editor = teams.getByRole("form", { name: "Modifier une équipe de l’organisation", exact: true });
  await editor.getByLabel("Nom de l’équipe", { exact: true }).fill("Design UI");
  await editor.getByRole("checkbox").first().check();
  await editor.getByRole("button", { name: "Enregistrer", exact: true }).click();
  await expect(teams.getByRole("heading", { name: "Design UI", exact: true })).toBeVisible();
  await teams.getByRole("combobox", { name: "Rôle dans ce projet", exact: true }).selectOption("member");
  await expect(teams.getByRole("status")).toContainText("Droits du projet mis à jour");
  const assigned = await fixtureHttp(fixture, "GET", `/v3/spaces/${orgProject.spaceId}/organisation-teams`);
  expect(assigned.data.teams.find((item: any) => item.name === "Design UI").assignment.role).toBe("member");
  await chooseOrganisation(company.name);
  await page.goto(`/#/projects/${encodeURIComponent(fixture.spaceId)}/team/teams`);
  await expect(page.getByRole("region", { name: "Équipes de l’organisation", exact: true })).not.toContainText("Design UI");
  const companyCatalog = await fixtureHttp(fixture, "GET", `/v3/spaces?contextId=${company.id}`);
  expect(companyCatalog.data.spaces.some((item: any) => ["Notes personnelles UI", "Projet organisation UI"].includes(item.name))).toBe(false);

  // A reversible mode change requires an explicit primary and danger confirmation.
  const duplicate = await fixtureHttp(fixture, "POST", "/v3/organisations", { name: org.name, slug: "equipe-produit-homonyme" });
  expect(duplicate.status).toBeLessThan(300);
  await page.evaluate(() => window.dispatchEvent(new Event("layrs-organisations-changed")));
  const originalSettings = await fixtureHttp(fixture, "GET", "/v3/admin/organisations/settings");
  expect(originalSettings.status).toBe(200);
  await selector.click();
  await expect(page.getByRole("menu", { name: "Organisations", exact: true })).toBeVisible();
  await expect(page.getByRole("menuitemradio", { name: `${org.name} · ${org.slug}`, exact: true })).toBeVisible();
  await expect(page.getByRole("menuitemradio", { name: `${org.name} · equipe-produit-homonyme`, exact: true })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("organisation-selector.png"), animations: "disabled" });
  await page.keyboard.press("Escape");
  await page.goto("/#/admin/organisations");
  await page.getByRole("button", { name: "Passer en mono-organisation", exact: true }).click();
  const warning = page.getByRole("dialog", { name: "Attention : changement dangereux", exact: true });
  const confirm = warning.getByRole("button", { name: "Confirmer le passage en mono-organisation", exact: true });
  await expect(confirm).toBeDisabled();
  await warning.getByRole("combobox", { name: "Organisation à garder comme principale", exact: true }).selectOption(org.id);
  await expect(warning.getByRole("list", { name: "Organisations rendues inaccessibles" })).toContainText(company.name);
  await page.screenshot({ path: testInfo.outputPath("organisation-mode-warning.png"), animations: "disabled" });
  await warning.getByRole("button", { name: "Annuler", exact: true }).click();
  expect((await fixtureHttp(fixture, "GET", "/v3/admin/organisations/settings")).data.multiOrganisationEnabled).toBe(true);
  await page.getByRole("button", { name: "Passer en mono-organisation", exact: true }).click();
  await warning.getByRole("combobox", { name: "Organisation à garder comme principale", exact: true }).selectOption(org.id);
  const confirmation = warning.getByLabel("Recopiez l’identifiant de l’organisation principale pour confirmer", { exact: true });
  await confirmation.fill(org.name);
  await expect(confirm).toBeDisabled();
  await confirmation.fill(org.slug);
  await confirm.click();
  await expect(warning).toBeHidden();
  await expect(page.getByText("Mode mono-organisation", { exact: true })).toBeVisible();
  await expect(selector).toHaveCount(0);
  const single = await fixtureHttp(fixture, "GET", "/v3/contexts");
  expect(single.data.contexts.filter((item: any) => item.kind !== "personal").map((item: any) => item.id)).toEqual([org.id]);
  expect(single.data.personalProjectsEnabled).toBe(true);
  const hidden = await fixtureHttp(fixture, "GET", `/v3/spaces/${fixture.spaceId}/bootstrap`);
  expect(hidden.status).toBeGreaterThanOrEqual(400);
  expect((await fixtureHttp(fixture, "GET", `/v3/spaces/${orgProject.spaceId}/bootstrap`)).status).toBe(200);
  expect((await fixtureHttp(fixture, "GET", `/v3/spaces/${notes.spaceId}/bootstrap`)).status).toBe(200);
  await page.getByRole("button", { name: "Activer le multi-organisation", exact: true }).click();
  await page.getByRole("dialog", { name: "Activer le multi-organisation", exact: true }).getByRole("button", { name: "Activer les organisations", exact: true }).click();
  await expect(page.getByText("Mode multi-organisation", { exact: true })).toBeVisible();
  await expect(selector).toBeVisible();
  expect((await fixtureHttp(fixture, "GET", `/v3/spaces/${fixture.spaceId}/bootstrap`)).status).toBe(200);
  // Restore the shared fixture's original default for the other independent recipes.
  const restored = (await fixtureHttp(fixture, "GET", "/v3/admin/organisations/settings")).data;
  const reset = await fixtureHttp(fixture, "PUT", "/v3/admin/organisations/settings", {
    expectedRevision: restored.revision, expectedStateToken: restored.stateToken,
    multiOrganisationEnabled: true, primaryOrganisationId: originalSettings.data.primaryOrganisationId, confirmDanger: false,
  });
  expect(reset.status).toBe(200);
  await webSignOut(page);

  await page.goto("/#/projects");
  await webSignIn(page, fixture.accounts.bob);
  await expect(page.getByRole("heading", { name: "Vos projets", exact: true })).toBeVisible();
  await selector.click();
  await expect(page.getByRole("menuitemradio", { name: "Équipe produit UI", exact: true })).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(page.getByText("Notes personnelles UI", { exact: true })).toHaveCount(0);
  await webSignOut(page);
});
