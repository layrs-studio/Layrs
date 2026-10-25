import { test, expect } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { webSignIn, webOpenProjectByName, webSignOut } from "./fixtures/nativeWebAccess";
import { fixtureHttp } from "./fixtures/nativeSharedClients";

const canonical = (value: any): string => value === null || typeof value !== "object" ? JSON.stringify(value) : Array.isArray(value) ? `[${value.map(canonical).join(",")}]` : `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`;

test("installs a server lens, creates a project, assigns it and runs its downloaded viewer", async ({ page }) => {
  test.setTimeout(180_000);
  page.setDefaultTimeout(15_000);
  const fixture = JSON.parse(await readFile(process.env.LAYRS_NATIVE_UI_FIXTURE!, "utf8"));
  const packet = JSON.parse(await readFile("deploy/lenses/example-markdown.layrslens.json", "utf8"));
  // The disposable published fixture contains a UTF-8 .txt document.
  packet.manifest.analyzer.fileExtensions.push("txt");
  delete packet.manifest.digest;
  packet.manifest.digest = `sha256:${createHash("sha256").update(canonical(packet.manifest)).digest("hex")}`;
  await page.goto("/#/admin/lenses");
  await webSignIn(page, fixture.accounts.alice);
  const installed = page.getByRole("region", { name: "Lenses installées", exact: true });
  await expect(installed.getByRole("table")).toContainText("layrs.text");
  await installed.getByLabel("Installer une lens").setInputFiles({ name: "markdown.layrslens.json", mimeType: "application/json", buffer: Buffer.from(JSON.stringify(packet)) });
  await installed.getByRole("checkbox").check();
  await installed.getByRole("button", { name: "Vérifier et installer", exact: true }).click();
  await expect(installed.getByText(/Lens installée\./)).toBeVisible();
  await expect(installed.getByRole("row").filter({ hasText: "example.markdown" })).toContainText("Disponible");

  await page.getByRole("link", { name: "Tous les projets", exact: true }).click();
  await page.getByRole("button", { name: "Nouveau projet", exact: true }).click();
  await expect(page.getByLabel("Nom de l’organisation", { exact: true })).toHaveCount(0);
  await page.getByLabel("Nom du projet", { exact: true }).fill("Projet lenses UI");
  await page.getByRole("combobox", { name: "Markdown", exact: true }).selectOption(packet.manifest.digest);
  await page.getByRole("button", { name: "Créer le projet", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Fichiers", exact: true })).toBeVisible();
  await expect(page.getByRole("navigation", { name: "Fil d’Ariane" })).toContainText("Projet lenses UI");
  const catalog = await fixtureHttp(fixture, "GET", "/v3/spaces?limit=100");
  const created = catalog.data.spaces.find((item: any) => item.name === "Projet lenses UI");
  expect(created).toBeTruthy();
  const config = await fixtureHttp(fixture, "GET", `/v3/spaces/${created.spaceId}/lenses`);
  expect(config.data.pins.some((pin: any) => pin.digest === packet.manifest.digest)).toBe(true);

  await webOpenProjectByName(page, fixture);
  await page.getByRole("link", { name: "Réglages", exact: true }).click();
  await page.getByRole("navigation", { name: "Sections des réglages" }).getByRole("link", { name: "Lenses", exact: true }).click();
  const settings = page.getByRole("region", { name: "Lenses du projet", exact: true });
  await settings.getByRole("checkbox", { name: /Markdown/ }).check();
  await settings.getByLabel(/Associations prioritaires/).fill("txt=example.markdown");
  await settings.getByRole("button", { name: "Enregistrer", exact: true }).click();
  await expect(settings.getByText("Lenses du projet enregistrées.", { exact: true })).toBeVisible();
  await page.getByRole("link", { name: "Fichiers", exact: true }).click();
  const files = page.getByRole("region", { name: "Fichiers du projet", exact: true });
  await files.getByRole("button", { name: /^docs/ }).click();
  await files.getByRole("list", { name: "Contenu du dossier" }).getByRole("button", { name: /readme\.txt/ }).click();
  const viewer = files.getByRole("region", { name: "Lens Markdown", exact: true });
  await expect(viewer.getByText("Markdown · 1.0.0 · Chargée", { exact: true })).toBeVisible();
  await expect(viewer.getByRole("heading", { name: "docs/readme.txt", exact: true })).toBeVisible();
  await webSignOut(page);
  expect(await page.evaluate(async () => (await indexedDB.databases()).map(database => database.name))).not.toContain("layrs.lens-cache.v1");
});
