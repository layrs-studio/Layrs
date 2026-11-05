import { test, expect } from "@playwright/test";
import { calls, first, hash, installDesktop, route, second } from "./desktop-selection-fixture.mjs";

async function fillWorktree(page) {
  await page.getByLabel("Dossier du nouveau Worktree", { exact: true }).fill(second);
  await page.getByLabel("Layer du nouveau Worktree", { exact: true }).selectOption("layer:task");
  await page.getByLabel("Step exacte de départ", { exact: true }).fill(hash("b"));
}
async function addSelection(page, path = "excluded.txt") {
  await page.getByRole("button", { name: "Ajouter un chemin", exact: true }).click();
  await page.getByLabel("Chemin 1", { exact: true }).fill(path);
}

test("create and open changes only the displayed root and shows independent Step/Operation", async ({ page }) => {
  await installDesktop(page);
  await page.goto(route(first, "worktrees"));
  await expect(page.getByRole("heading", { name: "Dossiers enregistrés" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Créer et ouvrir le Worktree" })).toBeDisabled();
  await fillWorktree(page);
  await page.getByRole("button", { name: "Créer et ouvrir le Worktree" }).click();
  await expect(page).toHaveURL(new RegExp(`${encodeURIComponent(second)}/changes`));
  await page.getByRole("link", { name: "Worktrees", exact: true }).click();
  await expect(page.getByText("operation:second", { exact: false }).first()).toBeVisible();
  await expect(page.getByRole("region", { name: "Worktree courant", exact: true }).getByText(hash("b"), { exact: true })).toBeVisible();
  const invocations = await calls(page);
  expect(invocations.find(call => call.args?.request?.action === "createWorktree").args).toEqual({ root: first, request: { action: "createWorktree", path: second, layer: "layer:task", step: hash("b") } });
  expect(invocations.some(call => call.args?.request?.action === "switchLayer")).toBe(false);
  await expect(page.getByRole("button", { name: `Ouvrir ${first}` })).toBeVisible();
});

test("open an existing Worktree calls the native opener before navigating", async ({ page }) => {
  await installDesktop(page, { existing: true });
  await page.goto(route(first, "worktrees"));
  await page.getByRole("button", { name: `Ouvrir ${second}` }).click();
  await expect(page).toHaveURL(new RegExp(`${encodeURIComponent(second)}/changes`));
  expect((await calls(page)).filter(call => call.args?.request?.action === "openWorktree").map(call => call.args)).toEqual([{ root: first, request: { action: "openWorktree", path: second } }]);
});

test("interrupted registered creation requires journal recovery before resume", async ({ page }) => {
  await installDesktop(page, { interrupted: true });
  await page.goto(route(first, "worktrees"));
  await fillWorktree(page);
  await page.getByRole("button", { name: "Créer et ouvrir le Worktree" }).click();
  await page.getByRole("button", { name: "Ouvrir le dossier pour reprendre" }).click();
  await expect(page).toHaveURL(new RegExp(`${encodeURIComponent(second)}/health`));
  await expect(page.getByRole("button", { name: "Reprendre la création du Worktree" })).toBeDisabled();
  await page.getByRole("button", { name: "Reprendre après interruption" }).click();
  await page.getByRole("button", { name: "Reprendre la création du Worktree" }).click();
  await expect(page.getByText("Aucune opération interrompue", { exact: true })).toBeVisible();
  const actions = (await calls(page)).filter(call => ["recover", "resumeWorktreeCreation"].includes(call.args?.request?.action));
  expect(actions.map(call => [call.args.root, call.args.request.action])).toEqual([[second, "recover"], [second, "resumeWorktreeCreation"]]);
});

test("pre-registration creation can resume even when ordinary open fails", async ({ page }) => {
  await installDesktop(page, { unregistered: true });
  await page.goto(route(first, "worktrees"));
  await fillWorktree(page);
  await page.getByRole("button", { name: "Créer et ouvrir le Worktree" }).click();
  await page.getByRole("button", { name: "Ouvrir le dossier pour reprendre" }).click();
  await expect(page.getByText(/pending unregistered Worktree intent/)).toBeVisible();
  await page.getByRole("button", { name: "Reprendre la création dans ce dossier" }).click();
  await expect(page).toHaveURL(new RegExp(`${encodeURIComponent(second)}/changes`));
  expect((await calls(page)).find(call => call.args?.request?.action === "resumeWorktreeCreation").args.root).toBe(second);
});

test("partial clone sends exact path/subtree selection and opens its native result", async ({ page }) => {
  await installDesktop(page);
  await page.goto("/#/projects/new?mode=clone");
  await page.getByLabel("Dossier", { exact: true }).fill(second);
  await page.getByText("Référence du projet (avancé)", { exact: true }).click();
  await page.getByLabel("Identifiant du projet distant", { exact: true }).fill("space:test");
  await page.getByRole("radio", { name: "Choisir des chemins", exact: true }).click();
  await page.getByLabel("Capture publiée à récupérer", { exact: true }).fill(hash("a"));
  await addSelection(page, "README.md");
  await page.getByRole("button", { name: "Ajouter un chemin", exact: true }).click();
  await page.getByLabel("Type du chemin 2", { exact: true }).selectOption("subtree");
  await page.getByLabel("Chemin 2", { exact: true }).fill("src");
  await page.getByRole("button", { name: "Récupérer les fichiers autorisés", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`${encodeURIComponent(second)}/changes`));
  const action = (await calls(page)).find(call => call.args?.request?.action?.action === "cloneProjectionSelected").args.request.action;
  expect(action).toEqual({ action: "cloneProjectionSelected", space: "space:test", step: hash("a"), selection: { kind: "selected", paths: ["README.md"], subtrees: ["src"] } });
});

test("Diagnostic can resume pre-registration intent after reopening the app", async ({ page }) => {
  await installDesktop(page, { unregistered: true });
  await page.goto(route(second, "health"));
  await expect(page.getByText(/pending unregistered Worktree intent/)).toBeVisible();
  await page.getByRole("button", { name: "Reprendre la création du Worktree" }).click();
  await expect(page.getByText("Aucune opération interrompue", { exact: true })).toBeVisible();
  expect((await calls(page)).find(call => call.args?.request?.action === "resumeWorktreeCreation").args.root).toBe(second);
});

test("selection expansion preserves permission distinctions and submits the shown generation", async ({ page }) => {
  await installDesktop(page, { projection: true });
  await page.goto(route(first, "files"));
  const list = page.getByRole("region", { name: "Fichiers du projet", exact: true });
  await expect(list.locator("li").filter({ hasText: "excluded.txt" })).toContainText("Exclu de la sélection");
  await expect(list.locator("li").filter({ hasText: "secret.txt" })).toContainText("contenu interdit");
  await expect(list.locator("li").filter({ hasText: "unsupported" })).toContainText("indisponible");
  await addSelection(page);
  await page.getByRole("button", { name: "Ajouter et télécharger" }).click();
  await expect(list.locator("li").filter({ hasText: "excluded.txt" })).toContainText("Matérialisé");
  expect((await calls(page)).find(call => call.args?.request?.action?.action === "expandProjectionSelection").args.request.action).toEqual({ action: "expandProjectionSelection", expectedGeneration: 4, selection: { kind: "selected", paths: ["excluded.txt"], subtrees: [] } });
  await expect(list.locator("li").filter({ hasText: "secret.txt" })).toContainText("contenu interdit");
});

test("a stale reviewed generation blocks remote expansion until a fresh read", async ({ page }) => {
  await installDesktop(page, { projection: true });
  await page.goto(route(first, "files"));
  await addSelection(page);
  await page.evaluate(() => window.__advanceProjection());
  await page.getByRole("button", { name: "Ajouter et télécharger" }).click();
  await expect(page.getByText(/La sélection a changé/)).toBeVisible();
  expect((await calls(page)).some(call => call.args?.request?.action?.action === "expandProjectionSelection")).toBe(false);
  await page.getByRole("button", { name: "Actualiser les fichiers", exact: true }).click();
  await addSelection(page);
  await page.getByRole("button", { name: "Ajouter et télécharger" }).click();
  await expect.poll(async () => (await calls(page)).find(call => call.args?.request?.action?.action === "expandProjectionSelection")?.args.request.action.expectedGeneration).toBe(5);
});

test("collision errors remain visible and no successful expansion is announced", async ({ page }) => {
  await installDesktop(page, { projection: true, collision: true });
  await page.goto(route(first, "files"));
  await addSelection(page);
  await page.getByRole("button", { name: "Ajouter et télécharger" }).click();
  await expect(page.getByText(/collision with local file/)).toBeVisible();
  await expect(page.getByText("Sélection élargie et fichiers vérifiés.", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Actualiser les fichiers", exact: true }).click();
  await expect(page.getByRole("region", { name: "Fichiers du projet", exact: true }).locator("li").filter({ hasText: "excluded.txt" })).toContainText("Exclu de la sélection");
});
