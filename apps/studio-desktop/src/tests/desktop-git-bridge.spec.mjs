import { test, expect } from "@playwright/test";
import { calls, first, hash, installDesktop, route } from "./desktop-selection-fixture.mjs";

const pin = `sha256:${"a".repeat(64)}`;
const actions = async page => (await calls(page)).filter(call => call.command === "native_git_bridge").map(call => call.args.request);
async function existingCheckpoint(page) {
  await page.getByLabel("Fichier checkpoint Git", { exact: true }).fill("C:/review.scan");
  await page.getByLabel("Empreinte SHA-256 du checkpoint", { exact: true }).fill(pin);
  await page.getByRole("button", { name: "Vérifier le checkpoint et la cible", exact: true }).click();
  await expect(page.getByRole("region", { name: "Import Git vérifié", exact: true })).toBeVisible();
}
async function exportReview(page) {
  await page.getByLabel("Dossier Git de destination", { exact: true }).fill("C:/export.git");
  await page.getByRole("button", { name: "Vérifier la cible de l’export", exact: true }).click();
  await expect(page.getByRole("region", { name: "Cible Git vérifiée", exact: true })).toBeVisible();
}

async function nativeSnapshotReply(page, stepOverride = null) {
  // serde_json maps return kind/reference/stepId, unlike the editor's insertion order.
  await page.evaluate(stepOverride => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    window.__TAURI_INTERNALS__.invoke = async (command, args) => {
      const result = await invoke(command, args);
      if (command !== "native_git_bridge" || args.request.action !== "reviewExport" || result.review.mode.kind !== "snapshot") return result;
      const { kind, reference, stepId } = result.review.mode;
      const response = { ...result, review: { ...result.review, mode: { kind, reference, stepId: stepOverride ?? stepId } } };
      window.__gitNativeOrderedReview = structuredClone(response);
      return response;
    };
  }, stepOverride);
}

test("prepare, review exact digest and target, then explicitly import; loading writes nothing", async ({ page }) => {
  await installDesktop(page);
  await page.goto(route(first, "git"));
  await expect(page.getByRole("heading", { name: "Pont Git", exact: true })).toBeVisible();
  expect(await actions(page)).toEqual([]);
  await page.getByLabel("Dépôt Git source", { exact: true }).fill("C:/source.git");
  await page.getByLabel("Fichier checkpoint Git", { exact: true }).fill("C:/review.scan");
  await page.getByLabel("Références Git à préparer (optionnel)", { exact: true }).fill("refs/heads/main");
  await page.getByRole("button", { name: "Créer le checkpoint Git", exact: true }).click();
  await expect(page.getByLabel("Empreinte SHA-256 du checkpoint", { exact: true })).toHaveValue(pin);
  await page.getByRole("button", { name: "Vérifier le checkpoint et la cible", exact: true }).click();
  const review = page.getByRole("region", { name: "Import Git vérifié", exact: true });
  await expect(review).toContainText(first);
  await expect(review).toContainText("operation:first");
  await expect(review).toContainText(pin);
  expect((await actions(page)).map(action => action.action)).toEqual(["prepareImport", "reviewImport"]);
  await page.getByRole("button", { name: "Importer ce checkpoint dans ce Worktree", exact: true }).click();
  await expect(page.getByText(/Import confirmé\./)).toBeVisible();
  expect(await actions(page)).toEqual([
    { action: "prepareImport", source: "C:/source.git", checkpoint: "C:/review.scan", references: ["refs/heads/main"] },
    { action: "reviewImport", checkpoint: "C:/review.scan", digest: pin },
    { action: "applyImport", reviewId: "review:import", checkpoint: "C:/review.scan", digest: pin },
  ]);
});

test("editing a reviewed pin removes import confirmation and requires a new native review", async ({ page }) => {
  await installDesktop(page);
  await page.goto(route(first, "git"));
  await existingCheckpoint(page);
  await page.getByLabel("Empreinte SHA-256 du checkpoint", { exact: true }).fill(`sha256:${"b".repeat(64)}`);
  await expect(page.getByRole("button", { name: "Importer ce checkpoint dans ce Worktree", exact: true })).toHaveCount(0);
  await page.getByLabel("Fichier checkpoint Git", { exact: true }).fill("C:/other.scan");
  await expect(page.getByRole("button", { name: "Vérifier le checkpoint et la cible", exact: true })).toBeDisabled();
  expect((await actions(page)).map(action => action.action)).toEqual(["reviewImport"]);
});

test("native stale-target refusal is shown and never becomes an import success", async ({ page }) => {
  await installDesktop(page, { gitErrorAction: "applyImport", gitError: "git_bridge_error: stale Worktree Operation/View" });
  await page.goto(route(first, "git"));
  await existingCheckpoint(page);
  await page.getByRole("button", { name: "Importer ce checkpoint dans ce Worktree", exact: true }).click();
  await expect(page.getByText("Error: git_bridge_error: stale Worktree Operation/View", { exact: true })).toBeVisible();
  await expect(page.getByText(/Import confirmé\./)).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Importer ce checkpoint dans ce Worktree", exact: true })).toHaveCount(0);
});

test("missing native review support is a visible error, with no fabricated preview", async ({ page }) => {
  await installDesktop(page, { gitErrorAction: "reviewImport", gitError: "Command native_git_bridge not found" });
  await page.goto(route(first, "git"));
  await page.getByLabel("Fichier checkpoint Git", { exact: true }).fill("C:/review.scan");
  await page.getByLabel("Empreinte SHA-256 du checkpoint", { exact: true }).fill(pin);
  await page.getByRole("button", { name: "Vérifier le checkpoint et la cible", exact: true }).click();
  await expect(page.getByText("Error: Command native_git_bridge not found", { exact: true })).toBeVisible();
  await expect(page.getByRole("region", { name: "Import Git vérifié", exact: true })).toHaveCount(0);
});

test("export review binds a snapshot and destination; edits require another review", async ({ page }) => {
  const pageErrors = [];
  page.on("pageerror", error => pageErrors.push(error.message));
  await installDesktop(page);
  await page.goto(route(first, "git"));
  await nativeSnapshotReply(page);
  await exportReview(page);
  await expect(page.getByText("Aperçu du contenu indisponible dans le moteur", { exact: true })).toBeVisible();
  expect((await actions(page)).map(action => action.action)).toEqual(["reviewExport"]);
  await page.getByLabel("Contenu de l’export", { exact: true }).selectOption("snapshot");
  await expect(page.getByRole("button", { name: "Créer cet export Git", exact: true })).toHaveCount(0);
  await page.getByLabel("Step exacte à exporter", { exact: true }).fill("@layer:main");
  await expect(page.getByRole("button", { name: "Vérifier la cible de l’export", exact: true })).toBeDisabled();
  await page.getByLabel("Step exacte à exporter", { exact: true }).fill(hash("b"));
  await page.getByRole("button", { name: "Vérifier la cible de l’export", exact: true }).click();
  await expect(page.getByRole("button", { name: "Créer cet export Git", exact: true })).toBeEnabled();
  expect(await page.evaluate(() => Object.keys(window.__gitNativeOrderedReview.review.mode))).toEqual(["kind", "reference", "stepId"]);
  await page.getByRole("button", { name: "Créer cet export Git", exact: true }).click();
  await expect(page.getByText("Export Git confirmé par le moteur", { exact: true })).toBeVisible();
  expect((await actions(page)).at(-1)).toEqual({ action: "export", reviewId: "review:export", destination: "C:/export.git", mode: { kind: "snapshot", stepId: hash("b"), reference: "refs/heads/main" } });
  expect(pageErrors).toEqual([]);
});

for (const resume of [false, true]) test(`snapshot review drift is displayed before native mutation, without an unhandled rejection (resume=${resume})`, async ({ page }) => {
  const pageErrors = [];
  page.on("pageerror", error => pageErrors.push(error.message));
  await installDesktop(page);
  await page.goto(route(first, "git"));
  await nativeSnapshotReply(page, hash("c"));
  await page.getByLabel("Contenu de l’export", { exact: true }).selectOption("snapshot");
  await page.getByLabel("Step exacte à exporter", { exact: true }).fill(hash("b"));
  await exportReview(page);
  await page.getByRole("button", { name: resume ? "Reprendre cet export interrompu" : "Créer cet export Git", exact: true }).click();
  await expect(page.getByText("Le moteur Git n’a pas confirmé l’opération", { exact: true })).toBeVisible();
  await expect(page.getByRole("region", { name: "Exporter vers un dépôt Git", exact: true }).getByText(/La destination, les références ou la cible a changé/)).toBeVisible();
  await expect(page.getByRole("region", { name: "Cible Git vérifiée", exact: true })).toHaveCount(0);
  await expect(page.getByText("Export Git confirmé par le moteur", { exact: true })).toHaveCount(0);
  expect((await actions(page)).map(action => action.action)).toEqual(["reviewExport"]);
  expect(pageErrors).toEqual([]);
});

test("resume is a separate explicit action and invalid recovery never reports success", async ({ page }) => {
  await installDesktop(page, { gitErrorAction: "resumeExport", gitError: "git_bridge_error: export checkpoint does not match exact native snapshot/mode" });
  await page.goto(route(first, "git"));
  await exportReview(page);
  expect((await actions(page)).map(action => action.action)).toEqual(["reviewExport"]);
  await page.getByRole("button", { name: "Reprendre cet export interrompu", exact: true }).click();
  await expect(page.getByText("Error: git_bridge_error: export checkpoint does not match exact native snapshot/mode", { exact: true })).toBeVisible();
  expect((await actions(page)).map(action => action.action)).toEqual(["reviewExport", "resumeExport"]);
  await expect(page.getByText("Export Git confirmé par le moteur", { exact: true })).toHaveCount(0);
});

test("the native canonical reference order remains confirmable after review", async ({ page }) => {
  await installDesktop(page);
  await page.goto(route(first, "git"));
  await page.getByLabel("Dossier Git de destination", { exact: true }).fill("C:/export.git");
  await page.getByLabel("Contenu de l’export", { exact: true }).selectOption("selected_history");
  await page.getByLabel("Références Git à exporter", { exact: true }).fill("refs/tags/v1\nrefs/heads/main");
  await page.getByRole("button", { name: "Vérifier la cible de l’export", exact: true }).click();
  await expect(page.getByLabel("Références Git à exporter", { exact: true })).toHaveValue("refs/heads/main\nrefs/tags/v1");
  await page.getByRole("button", { name: "Créer cet export Git", exact: true }).click();
  await expect(page.getByText("Export Git confirmé par le moteur", { exact: true })).toBeVisible();
  expect((await actions(page)).at(-1).mode).toEqual({ kind: "selected_history", references: ["refs/heads/main", "refs/tags/v1"] });
});
