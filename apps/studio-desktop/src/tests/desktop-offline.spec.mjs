import { test, expect } from "@playwright/test";
import { calls, first, second, hash, installDesktop, route } from "./desktop-selection-fixture.mjs";

const localCalls = async page => (await calls(page)).filter(c => c.command === "native_projection" && c.args.request.action === "offline").map(c => c.args.request.request);
const networkCalls = async page => (await calls(page)).filter(c => c.command === "native_remote").map(c => c.args.request.action);
const inspectedBinding = page => page.evaluate(() => window.__lastInspection.binding);
async function open(page, view = "files", scenario = {}) {
  await installDesktop(page, { projection: true, offline: scenario });
  await page.goto(route(first, view));
  await expect(page.getByRole("heading", { name: view === "settings" ? "Stockage partagé et quota" : view === "worktrees" ? "Worktrees" : "Disponibilité hors ligne", exact: true })).toBeVisible();
}
async function paths(page, value = "chosen.txt") { await page.getByLabel("Chemins à vérifier", { exact: true }).fill(value); }

test("opening surfaces only reads; local availability preserves independent states", async ({ page }) => {
  await open(page);
  expect(await localCalls(page)).toEqual([]);
  expect((await networkCalls(page)).some(a => /Offline|observeProjection/.test(a.action))).toBe(false);
  await paths(page, "secret.txt\nexcluded.txt\nchosen.txt");
  await page.getByRole("button", { name: "Vérifier la disponibilité", exact: true }).click();
  await expect(page.getByText("Exclu par choix", { exact: true })).toBeVisible();
  await expect(page.getByText("Accès protégé", { exact: true })).toBeVisible();
  await expect(page.getByText("Retenu par une protection", { exact: true })).toBeVisible();
  await expect(page.getByText("Absence enregistrée", { exact: true })).toHaveCount(0);
  const actions = await localCalls(page);
  expect(actions.map(a => a.action)).toEqual(["currentVersion", "availability"]);
  expect(actions[1].request).toEqual({ version: { kind: "generation", worktreeId: "worktree:partial", generation: 4, digest: "a".repeat(64) }, other: null, paths: ["chosen.txt", "excluded.txt", "secret.txt"], operation: "read" });
});

test("remote observation keeps its own selection and exact digest for explicit preparation", async ({ page }) => {
  await open(page);
  await paths(page);
  await page.getByText("Observer une version distante exacte", { exact: true }).click();
  await page.getByLabel("Step distant exact", { exact: true }).fill(hash("d"));
  await page.getByLabel("Chemins de l’observation distante", { exact: true }).fill("remote.txt\nchosen.txt");
  await page.getByRole("button", { name: "Observer les métadonnées", exact: true }).click();
  await expect(page.getByLabel("Version à vérifier", { exact: true })).toHaveValue(/observationDigest/);
  await page.getByRole("button", { name: "Télécharger et préparer", exact: true }).click();
  await expect(page.getByText(/Préparation prête/)).toBeVisible();
  const actions = await networkCalls(page);
  expect(actions.find(a => a.action === "observeProjectionVersion")).toEqual({ action: "observeProjectionVersion", expectedGeneration: 4, step: hash("d"), paths: ["chosen.txt", "remote.txt"] });
  expect(actions.find(a => a.action === "prepareProjectionOffline")).toEqual({ action: "prepareProjectionOffline", expectedGeneration: 4, request: { version: { kind: "remote", stepId: hash("d"), rootTreeId: hash("a"), policySetId: hash("b"), authorizationEpoch: 2, observationDigest: "c".repeat(64) }, other: null, paths: ["chosen.txt"], operation: "read" } });
  expect(actions.some(a => a.action === "expandProjectionSelection")).toBe(false);
});

test("stale current version is refused before any preparation transport", async ({ page }) => {
  await open(page);
  await paths(page);
  await page.evaluate(() => window.__advanceProjection());
  await page.getByRole("button", { name: "Télécharger et préparer", exact: true }).click();
  await expect(page.getByText(/version courante a changé/).first()).toBeVisible();
  expect((await networkCalls(page)).some(a => a.action === "prepareProjectionOffline")).toBe(false);
});

test("remote preparation submits the reviewed generation and exposes a native stale refusal", async ({ page }) => {
  await open(page);
  await paths(page);
  await page.getByText("Observer une version distante exacte", { exact: true }).click();
  await page.getByLabel("Step distant exact", { exact: true }).fill(hash("d"));
  await page.getByLabel("Chemins de l’observation distante", { exact: true }).fill("chosen.txt");
  await page.getByRole("button", { name: "Observer les métadonnées", exact: true }).click();
  await expect(page.getByLabel("Version à vérifier", { exact: true })).toHaveValue(/observationDigest/);
  await page.evaluate(() => window.__advanceProjection());
  await page.getByRole("button", { name: "Télécharger et préparer", exact: true }).click();
  await expect(page.getByText(/offline_generation_changed/).first()).toBeVisible();
  expect((await networkCalls(page)).filter(a => a.action === "prepareProjectionOffline").map(a => a.expectedGeneration)).toEqual([4]);
  await expect(page.getByText(/Préparation prête/)).toHaveCount(0);
});

test("interrupted preparation is listed, explicitly resumed and independently released", async ({ page }) => {
  await open(page, "files", { interruptedPreparation: true });
  await paths(page);
  await page.getByRole("button", { name: "Télécharger et préparer", exact: true }).click();
  await expect(page.getByText(/offline_download_interrupted/).first()).toBeVisible();
  // Recover the status through the normal file-list refresh after provider invalidation.
  await page.getByRole("button", { name: /Actualiser/ }).first().click();
  await page.getByRole("button", { name: "Actualiser les préparations", exact: true }).click();
  await expect(page.getByText(/Acquisition interrompue ou en cours/)).toBeVisible();
  await page.getByRole("button", { name: "Reprendre cette préparation", exact: true }).click();
  await expect(page.getByText("Reprise terminée ; état des préparations actualisé.", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Libérer cette préparation", exact: true }).click();
  await expect(page.getByText(/Libérée ou annulée/)).toBeVisible();
  await expect(page.getByRole("button", { name: "Reprendre cette préparation", exact: true })).toHaveCount(0);
  expect((await networkCalls(page)).find(a => a.action === "resumeProjectionOffline")).toEqual({ action: "resumeProjectionOffline", expectedGeneration: 4, preparationId: "preparation:test" });
  expect((await localCalls(page)).find(a => a.action === "releasePreparation")).toEqual({ action: "releasePreparation", id: "preparation:test" });
});

test("explicitly cancelling an acquiring preparation uses the wired release action", async ({ page }) => {
  await open(page, "files", { interruptedPreparation: true });
  await paths(page);
  await page.getByRole("button", { name: "Télécharger et préparer", exact: true }).click();
  await expect(page.getByText(/offline_download_interrupted/).first()).toBeVisible();
  await page.getByRole("button", { name: /Actualiser/ }).first().click();
  await page.getByRole("button", { name: "Actualiser les préparations", exact: true }).click();
  await page.getByRole("button", { name: "Annuler cette préparation", exact: true }).click();
  await expect(page.getByText("Acquisition annulée et réservation libérée.", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Reprendre cette préparation", exact: true })).toHaveCount(0);
  expect((await localCalls(page)).filter(a => a.action === "releasePreparation")).toEqual([{ action: "releasePreparation", id: "preparation:test" }]);
});

test("historical comparison carries exact capture and recorded generation metadata", async ({ page }) => {
  await open(page);
  await paths(page);
  await page.getByRole("button", { name: "Charger les versions de l’historique", exact: true }).click();
  await page.getByLabel("Version à vérifier", { exact: true }).selectOption({ label: `Capture 3 · ${"e".repeat(12)}` });
  await page.getByLabel("Opération hors ligne", { exact: true }).selectOption("compareContent");
  await page.getByLabel("Seconde version", { exact: true }).selectOption({ label: `Génération 2 · ${"b".repeat(12)}` });
  await page.getByRole("button", { name: "Comparer ces versions", exact: true }).click();
  await expect(page.getByText("Comparaison des identifiants de contenu et des modes exécutables.", { exact: true })).toBeVisible();
  const request = (await localCalls(page)).find(a => a.action === "compare").request;
  expect(request.version).toEqual({ kind: "capture", worktreeId: "worktree:partial", sequence: 3, digest: hash("e") });
  expect(request.other).toEqual({ kind: "generation", worktreeId: "worktree:partial", generation: 2, digest: "b".repeat(64) });
});

for (const binary of [false, true]) test(`bounded preview makes an explicit UTF8 decision (binary=${binary})`, async ({ page }) => {
  await open(page, "files", { binary });
  await paths(page);
  await page.getByRole("button", { name: "Lire l’aperçu (1 Mio maximum)", exact: true }).click();
  if (binary) { await expect(page.getByText("Contenu binaire ou non UTF-8 : aucun aperçu textuel.", { exact: true })).toBeVisible(); await expect(page.getByLabel("Aperçu UTF-8")).toHaveCount(0); }
  else await expect(page.getByLabel("Aperçu UTF-8", { exact: true })).toHaveText("salut");
  expect((await localCalls(page)).find(a => a.action === "readFile").maxBytes).toBe(1048576);
});

test("quota and eviction are explicit; unsafe numeric input never reaches native", async ({ page }) => {
  await open(page, "settings");
  expect(await localCalls(page)).toEqual([]);
  await page.getByRole("button", { name: "Actualiser l’usage partagé", exact: true }).click();
  await page.getByLabel("Quota partagé (octets)", { exact: true }).fill("9007199254740992");
  await expect(page.getByRole("button", { name: "Appliquer ce quota", exact: true })).toBeDisabled();
  await page.getByLabel("Quota partagé (octets)", { exact: true }).fill("2048");
  await page.getByRole("button", { name: "Appliquer ce quota", exact: true }).click();
  await expect(page.getByText("Quota partagé enregistré.", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Évincer les contenus non retenus", exact: true }).click();
  await expect(page.getByText("1 objet(s) non retenu(s) évincé(s).", { exact: true })).toBeVisible();
  expect((await localCalls(page)).map(a => a.action)).toEqual(["familyUsage", "setFamilyQuota", "evictUnretained"]);
  expect((await localCalls(page))[1].bytes).toBe(2048);
});

test("conversion preserves source and requires a separate Open; imported history requires capture", async ({ page }) => {
  await open(page, "worktrees", { shared: false });
  expect(await localCalls(page)).toEqual([{ action: "inspectConversionSource" }]);
  await page.getByLabel("Dossier du stockage partagé", { exact: true }).fill("C:/family");
  await page.getByText("Créer une nouvelle famille", { exact: true }).click();
  await page.getByLabel("Quota initial (octets)", { exact: true }).fill("4096");
  await page.getByRole("button", { name: "Créer ce stockage partagé", exact: true }).click();
  await expect(page.getByText(/Stockage partagé créé/)).toBeVisible();
  await page.getByLabel("Destination de la copie", { exact: true }).fill(second);
  await page.getByRole("button", { name: "Créer la copie en conservant la source", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Copie vérifiée", exact: true })).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`${encodeURIComponent(first)}/worktrees`));
  expect((await localCalls(page)).map(a => a.action)).toEqual(["inspectConversionSource", "createFamily", "convertIntoFamily"]);
  expect((await calls(page)).some(c => c.command === "native_projection" && c.args.request.action === "status")).toBe(false);
  await page.getByRole("button", { name: "Ouvrir la copie", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`${encodeURIComponent(second)}/changes`));
  await expect(page.getByText("Historique importé conservé", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: /Capturer/, exact: false }).first().click();
  await expect(page.getByText("Historique importé conservé", { exact: true })).toHaveCount(0);
  const actions = (await calls(page)).filter(c => c.command === "native_projection" && c.args.root === second).map(c => c.args.request.action);
  expect(actions.slice(-2)).toEqual(["capture", "status"]);
});

test("already shared copy does not offer the unqualified new Worktree backend", async ({ page }) => {
  await open(page, "worktrees");
  await expect(page.getByText(/création d’un Worktree supplémentaire/)).toBeVisible();
  await expect(page.getByRole("button", { name: "Créer la copie en conservant la source", exact: true })).toHaveCount(0);
  expect(await localCalls(page)).toEqual([{ action: "inspectConversionSource" }]);
  expect((await calls(page)).some(c => c.command === "native_projection" && c.args.request.action === "status")).toBe(false);
});

test("unsupported conversion source never invokes ordinary status, including explicit reinspection", async ({ page }) => {
  const refusal = "source-preserving conversion supports private schema6 only; no automatic source migration";
  await open(page, "worktrees", { shared: false, inspectionError: refusal, forbidOrdinaryStatus: true });
  await expect(page.getByText(refusal, { exact: true })).toBeVisible();
  await page.getByLabel("Dossier du stockage partagé", { exact: true }).fill("C:/family");
  await page.getByLabel("Destination de la copie", { exact: true }).fill(second);
  await page.getByText("Créer une nouvelle famille", { exact: true }).click();
  await page.getByLabel("Quota initial (octets)", { exact: true }).fill("4096");
  for (const name of ["Créer ce stockage partagé", "Créer la copie en conservant la source", "Reprendre cette conversion"]) await expect(page.getByRole("button", { name, exact: true })).toBeDisabled();
  await page.getByRole("button", { name: "Relire la source avant de reprendre", exact: true }).click();
  await expect(page.getByText(refusal, { exact: true })).toBeVisible();
  await expect(page.getByLabel("Destination de la copie", { exact: true })).toHaveValue(second);
  const requests = (await calls(page)).filter(c => c.command === "native_projection").map(c => c.args.request);
  expect(requests).toEqual(Array.from({ length: 2 }, () => ({ action: "offline", request: { action: "inspectConversionSource" } })));
});

test("malformed conversion inspection fails closed without opening ordinary state", async ({ page }) => {
  await open(page, "worktrees", { shared: false, invalidInspection: true, forbidOrdinaryStatus: true });
  await expect(page.getByText("Inspection locale invalide. Le dossier reste fermé.", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Créer la copie en conservant la source", exact: true })).toBeDisabled();
  expect((await calls(page)).filter(c => c.command === "native_projection").map(c => c.args.request.action)).toEqual(["offline"]);
});

test("following a conversion refusal into Diagnostic stays read-only across reinspection, reload and sidebar navigation", async ({ page }) => {
  const refusal = "source-preserving conversion supports private schema6 only; no automatic source migration";
  await open(page, "worktrees", { shared: false, inspectionError: refusal, forbidOrdinaryStatus: true });
  await expect(page.getByText(refusal, { exact: true })).toBeVisible();
  await page.getByRole("link", { name: "Consulter le diagnostic et les reprises", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Diagnostic et reprise", exact: true })).toBeVisible();
  await expect(page.getByText(refusal, { exact: true })).toBeVisible();
  const inspections = (...actions) => actions.map(action => ({ action: "offline", request: { action } }));
  const projectionCalls = async () => (await calls(page)).filter(c => c.command === "native_projection").map(c => c.args.request);
  expect(await projectionCalls()).toEqual(inspections("inspectConversionSource", "inspectRecovery"));
  await page.getByRole("button", { name: "Relire sans modifier le dossier", exact: true }).click();
  await expect(page.getByText(refusal, { exact: true })).toBeVisible();
  expect(await projectionCalls()).toEqual(inspections("inspectConversionSource", "inspectRecovery", "inspectRecovery"));
  await page.reload();
  await expect(page.getByText(refusal, { exact: true })).toBeVisible();
  expect(await projectionCalls()).toEqual(inspections("inspectRecovery"));
  await page.getByRole("link", { name: "Worktrees", exact: true }).click();
  await expect(page.getByText(refusal, { exact: true })).toBeVisible();
  await page.getByRole("link", { name: "Diagnostic", exact: true }).click();
  await expect(page.getByText(refusal, { exact: true })).toBeVisible();
  expect(await projectionCalls()).toEqual(inspections("inspectRecovery", "inspectConversionSource", "inspectRecovery"));
  expect((await calls(page)).some(c => c.command === "native_workspace")).toBe(false);
});

test("route changes isolate conversion inspection from ordinary status reads", async ({ page }) => {
  await open(page, "worktrees", { shared: false });
  await expect(page.getByText("Propre à ce dossier", { exact: true })).toBeVisible();
  expect((await calls(page)).some(c => c.command === "native_projection" && c.args.request.action === "status")).toBe(false);
  await page.getByRole("link", { name: "Diagnostic", exact: true }).click();
  await expect(page.getByText("Aucun besoin de reprise signalé par l’inspection", { exact: true })).toBeVisible();
  expect((await calls(page)).some(c => c.command === "native_projection" && c.args.request.action === "status")).toBe(false);
  await page.getByRole("link", { name: "Fichiers", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Disponibilité hors ligne", exact: true })).toBeVisible();
  await expect.poll(async () => (await calls(page)).filter(c => c.command === "native_projection" && c.args.request.action === "status").length).toBeGreaterThan(0);
  const statusReads = (await calls(page)).filter(c => c.command === "native_projection" && c.args.request.action === "status").length;
  expect(statusReads).toBeGreaterThan(0);
  await page.getByRole("link", { name: "Worktrees", exact: true }).click();
  await expect(page.getByText("Propre à ce dossier", { exact: true })).toBeVisible();
  expect((await calls(page)).filter(c => c.command === "native_projection" && c.args.request.action === "status")).toHaveLength(statusReads);
  expect((await localCalls(page)).filter(a => a.action === "inspectConversionSource")).toHaveLength(2);
  expect((await localCalls(page)).filter(a => a.action === "inspectRecovery")).toHaveLength(1);
});

test("fresh Health restart routes native projection kind to explicit journal recovery without ordinary open", async ({ page }) => {
  await installDesktop(page, { projection: true, offline: { pendingJournal: "resumeRefresh" } });
  await page.goto(route(first, "health"));
  await expect(page.getByText("Des reprises restent nécessaires", { exact: true })).toBeVisible();
  await page.reload();
  await expect(page.getByText("Des reprises restent nécessaires", { exact: true })).toBeVisible();
  const recover = page.getByRole("button", { name: "Reprendre la réception locale", exact: true });
  await expect(recover).toBeEnabled();
  const expected = await inspectedBinding(page);
  expect((await calls(page)).some(c => c.command === "native_workspace")).toBe(false);
  expect((await calls(page)).some(c => c.command === "native_projection" && c.args.request.action.startsWith("resume"))).toBe(false);
  await recover.click();
  await expect(page.getByText("Reprise exécutée. Relisez l’inspection du dossier pour vérifier son état.", { exact: true })).toBeVisible();
  expect((await calls(page)).filter(c => c.command === "native_projection").map(c => c.args.request)).toEqual([{ action: "offline", request: { action: "inspectRecovery" } }, { action: "resumeRefresh", expected }]);
  await page.getByRole("button", { name: "Relire sans modifier le dossier", exact: true }).click();
  await expect(page.getByText("Aucun besoin de reprise signalé par l’inspection", { exact: true })).toBeVisible();
  expect((await calls(page)).filter(c => c.command === "native_projection").map(c => c.args.request.action)).toEqual(["offline", "resumeRefresh", "offline"]);
  expect((await calls(page)).filter(c => c.command === "native_checkout_kind")).toHaveLength(1);
});

test("interrupted conversion preserves its explicit destination for recovery", async ({ page }) => {
  await open(page, "worktrees", { shared: false, interrupted: true });
  await page.getByLabel("Dossier du stockage partagé", { exact: true }).fill("C:/family");
  await page.getByLabel("Destination de la copie", { exact: true }).fill(second);
  await page.getByRole("button", { name: "Créer la copie en conservant la source", exact: true }).click();
  await expect(page.getByText(/conversion_interrupted/).first()).toBeVisible();
  await expect(page.getByLabel("Destination de la copie", { exact: true })).toHaveValue(second);
  await page.getByRole("button", { name: "Relire la source avant de reprendre", exact: true }).click();
  await page.getByRole("button", { name: "Reprendre cette conversion", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Copie vérifiée", exact: true })).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`${encodeURIComponent(first)}/worktrees`));
  expect((await localCalls(page)).at(-1)).toEqual({ action: "resumeConversion", destination: second });
});

for (const refused of [false, true]) test(`capture reservation cancellation is explicit and reports native refusal (${refused})`, async ({ page }) => {
  await installDesktop(page, { projection: true, offline: { recoveryRefusal: refused ? "cancelSharedReservation" : null } });
  await page.goto(route(first, "health"));
  const button = page.getByRole("button", { name: "Annuler la réservation de capture", exact: true });
  await expect(button).toBeEnabled();
  await expect(page.getByText("Aucun besoin de reprise signalé par l’inspection", { exact: true })).toBeVisible();
  expect(await localCalls(page)).toEqual([{ action: "inspectRecovery" }]);
  const expected = await inspectedBinding(page);
  await button.click();
  if (refused) {
    await expect(page.getByText(/operation may own checkout staging/).first()).toBeVisible();
    await expect(page.getByText(/Réservation de capture annulée ou déjà absente/)).toHaveCount(0);
  } else await expect(page.getByText(/Réservation de capture annulée ou déjà absente/)).toBeVisible();
  expect(await localCalls(page)).toEqual([{ action: "inspectRecovery" }, { action: "cancelSharedReservation", expected }]);
});

for (const refused of [false, true]) test(`generation protection release validates input, preserves refusals, never evicts (${refused})`, async ({ page }) => {
  await open(page, "settings", { recoveryRefusal: refused ? "releaseGenerationRetention" : null });
  await page.getByText("Libérer la protection d’une ancienne génération", { exact: true }).click();
  const input = page.getByLabel("Génération exacte à libérer", { exact: true });
  const button = page.getByRole("button", { name: "Libérer la protection de cette génération", exact: true });
  for (const value of ["4", "1.5", "9007199254740992"]) { await input.fill(value); await expect(button).toBeDisabled(); }
  expect(await localCalls(page)).toEqual([]);
  await input.fill("2");
  await button.click();
  if (refused) {
    await expect(page.getByText(/generation is required by retained capture history/).first()).toBeVisible();
    await expect(input).toHaveValue("2");
    await expect(button).toBeDisabled();
    await page.getByRole("button", { name: "Relire l’état du dossier", exact: true }).click();
    await expect(input).toHaveValue("2");
    await expect(button).toBeEnabled();
    await expect(page.getByText(/2 protection\(s\) libérée/)).toHaveCount(0);
  } else await expect(page.getByText(/Génération 2 : 2 protection\(s\) libérée/)).toBeVisible();
  expect(await localCalls(page)).toEqual([{ action: "releaseGenerationRetention", generation: 2 }]);
});

for (const unavailable of ["error", "absent"]) test(`conversion abandonment works with unavailable source (${unavailable}) and keeps files/path explicit`, async ({ page }) => {
  await installDesktop(page, { projection: true, offline: { sourceUnavailable: unavailable } });
  await page.goto(route(first, unavailable === "absent" ? "changes" : "health"));
  if (unavailable === "absent") await page.getByRole("button", { name: "Diagnostic et reprise", exact: true }).click();
  await page.getByLabel("Stockage de la conversion interrompue", { exact: true }).fill("C:/family");
  await page.getByLabel("Destination de la conversion interrompue", { exact: true }).fill(second);
  expect(await localCalls(page)).toEqual([]);
  await page.getByRole("button", { name: "Abandonner cette conversion en conservant les fichiers", exact: true }).click();
  await expect(page.getByText(/Conversion abandonnée/)).toBeVisible();
  await expect(page.getByText(/Fichiers conservés sur disque/)).toBeVisible();
  await expect(page.getByLabel("Destination de la conversion interrompue", { exact: true })).toHaveValue(second);
  await expect(page).toHaveURL(new RegExp(`${encodeURIComponent(first)}/health`));
  expect((await calls(page)).filter(c => c.command === "native_projection").map(c => c.args.request)).toEqual([{ action: "offline", request: { action: "abandonConversion", familyRoot: "C:/family", destination: second } }]);
});

test("a ready conversion cannot be abandoned; entered source/family/destination are preserved", async ({ page }) => {
  await open(page, "worktrees", { shared: false });
  await page.getByLabel("Dossier du stockage partagé", { exact: true }).fill("C:/family");
  await page.getByLabel("Destination de la copie", { exact: true }).fill(second);
  await page.getByRole("button", { name: "Créer la copie en conservant la source", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Copie vérifiée", exact: true })).toBeVisible();
  await page.getByText("Abandonner une conversion non terminée", { exact: true }).click();
  await page.getByRole("button", { name: "Abandonner cette conversion en conservant les fichiers", exact: true }).click();
  await expect(page.getByText(/accepted destination cannot be abandoned/).first()).toBeVisible();
  await expect(page.getByLabel("Dossier du stockage partagé", { exact: true })).toHaveValue("C:/family");
  await expect(page.getByLabel("Destination de la copie", { exact: true })).toHaveValue(second);
  await expect(page.getByText(/Conversion abandonnée/)).toHaveCount(0);
  await expect(page).toHaveURL(new RegExp(`${encodeURIComponent(first)}/worktrees`));
});

test("an abandonment reply for another destination cannot become a success", async ({ page }) => {
  await installDesktop(page, { projection: true, offline: { sourceUnavailable: "error", wrongRecoveryReply: true } });
  await page.goto(route(first, "health"));
  await page.getByLabel("Stockage de la conversion interrompue", { exact: true }).fill("C:/family");
  await page.getByLabel("Destination de la conversion interrompue", { exact: true }).fill(second);
  await page.getByRole("button", { name: "Abandonner cette conversion en conservant les fichiers", exact: true }).click();
  await expect(page.getByText(/Abandon non confirmé/).first()).toBeVisible();
  await expect(page.getByText(/Conversion abandonnée/)).toHaveCount(0);
  await expect(page.getByLabel("Destination de la conversion interrompue", { exact: true })).toHaveValue(second);
});

test("local Restore reviews exact metadata then applies the unchanged preview without HTTP or preparation receipts", async ({ page }) => {
  await open(page, "files", { restoreAbsence: true });
  await paths(page);
  expect(await localCalls(page)).toEqual([]);
  await page.getByRole("button", { name: "Préparer l’aperçu local", exact: true }).click();
  await expect(page.getByLabel("Aperçu de restauration locale", { exact: true })).toBeVisible();
  await expect(page.getByText("Absence enregistrée", { exact: true })).toBeVisible();
  const actions = await localCalls(page);
  expect(actions).toEqual([{ action: "currentVersion" }, { action: "previewRestore", expectedGeneration: 4, version: { kind: "generation", worktreeId: "worktree:partial", generation: 4, digest: "a".repeat(64) }, paths: ["chosen.txt"] }]);
  const reviewed = await page.evaluate(() => window.__restorePreview);
  await page.getByRole("button", { name: "Appliquer cette restauration locale", exact: true }).click();
  await expect(page.getByText(/Restauration locale confirmée · capture 3/)).toBeVisible();
  expect((await localCalls(page)).at(-1)).toEqual({ action: "applyRestore", preview: reviewed });
  expect((await networkCalls(page)).filter(a => a.action !== "whoami")).toEqual([]);
  expect((await calls(page)).filter(c => c.command === "native_projection").at(-1).args.request).toEqual({ action: "status" });
});

test("historical local Restore keeps exact capture identity and editing selection cancels confirmation", async ({ page }) => {
  await open(page);
  await paths(page);
  await page.getByRole("button", { name: "Charger les versions de l’historique", exact: true }).click();
  const version = { kind: "capture", worktreeId: "worktree:partial", sequence: 3, digest: hash("e") };
  await page.getByLabel("Version à vérifier", { exact: true }).selectOption(JSON.stringify(version));
  await page.getByRole("button", { name: "Préparer l’aperçu local", exact: true }).click();
  await expect(page.getByRole("button", { name: "Appliquer cette restauration locale", exact: true })).toBeVisible();
  expect((await localCalls(page)).at(-1)).toEqual({ action: "previewRestore", expectedGeneration: 4, version, paths: ["chosen.txt"] });
  await page.getByLabel("Chemins à vérifier", { exact: true }).fill("other.txt");
  await expect(page.getByRole("button", { name: "Appliquer cette restauration locale", exact: true })).toHaveCount(0);
  expect((await localCalls(page)).some(a => a.action === "applyRestore")).toBe(false);
  await page.getByLabel("Opération hors ligne", { exact: true }).selectOption("compareMetadata");
  await expect(page.getByRole("button", { name: "Préparer l’aperçu local", exact: true })).toBeDisabled();
});

for (const invalid of [false, true]) test(`local Restore preview refusal or mismatched source never enables apply (${invalid})`, async ({ page }) => {
  await open(page, "files", invalid ? { invalidRestorePreview: true } : { restoreRefusal: "previewRestore" });
  await paths(page);
  await page.getByRole("button", { name: "Préparer l’aperçu local", exact: true }).click();
  await expect(page.getByText(invalid ? /restauration incohérent/ : /currently writable regular-file leaves/).first()).toBeVisible();
  await expect(page.getByRole("button", { name: "Appliquer cette restauration locale", exact: true })).toHaveCount(0);
  expect((await localCalls(page)).some(a => ["applyRestore", "releasePreparation"].includes(a.action))).toBe(false);
  expect((await networkCalls(page)).filter(a => a.action !== "whoami")).toEqual([]);
});

for (const wrongReceipt of [false, true]) test(`local Restore stale apply or foreign receipt cannot report success (${wrongReceipt})`, async ({ page }) => {
  await open(page, "files", wrongReceipt ? { wrongRestoreReceipt: true } : { restoreRefusal: "applyRestore" });
  await paths(page);
  await page.getByRole("button", { name: "Préparer l’aperçu local", exact: true }).click();
  await page.getByRole("button", { name: "Appliquer cette restauration locale", exact: true }).click();
  await expect(page.getByText(wrongReceipt ? /restauration incohérent/ : /restore preview is stale/).first()).toBeVisible();
  await expect(page.getByText(/Restauration locale confirmée/)).toHaveCount(0);
  expect((await localCalls(page)).filter(a => a.action === "applyRestore")).toHaveLength(1);
  expect((await networkCalls(page)).filter(a => a.action !== "whoami")).toEqual([]);
});

test("typed hot-database recovery is explicit and leaves application recovery needs visible", async ({ page }) => {
  const remaining = ["filesystemOperation", "publication", "conversion", "initializing"];
  await installDesktop(page, { projection: true, offline: { recovery: ["familyRollback", "worktreeRollback", ...remaining], remainingRecovery: remaining, forbidOrdinaryStatus: true } });
  await page.goto(route(first, "health"));
  const recover = page.getByRole("button", { name: "Reprendre les bases de données locales", exact: true });
  await expect(recover).toBeEnabled();
  expect(await localCalls(page)).toEqual([{ action: "inspectRecovery" }]);
  const expected = await inspectedBinding(page);
  await recover.click();
  await expect(page.getByText(/Reprise des bases exécutée\./)).toBeVisible();
  await expect(page.getByText("Des reprises restent nécessaires", { exact: true })).toBeVisible();
  for (const label of ["Opération sur les fichiers à reprendre", "Publication à confirmer", "Conversion inachevée", "Initialisation du dossier inachevée"]) await expect(page.getByLabel("Besoins de reprise restants").getByText(label, { exact: true })).toBeVisible();
  await expect(recover).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Reprendre l’initialisation du dossier", exact: true })).toHaveCount(0);
  await expect(page.getByText("Aucun besoin de reprise signalé par l’inspection", { exact: true })).toHaveCount(0);
  expect((await calls(page)).filter(c => c.command === "native_projection").map(c => c.args.request)).toEqual([{ action: "offline", request: { action: "inspectRecovery" } }, { action: "offline", request: { action: "recoverDatabases", expected } }]);
});

test("initialization-only recovery is explicit and accepts the typed remaining-state reply", async ({ page }) => {
  await installDesktop(page, { projection: true, offline: { recovery: ["initializing"], forbidOrdinaryStatus: true } });
  await page.goto(route(first, "health"));
  const button = page.getByRole("button", { name: "Reprendre l’initialisation du dossier", exact: true });
  await expect(button).toBeEnabled();
  await expect(page.getByRole("button", { name: "Reprendre les bases de données locales", exact: true })).toHaveCount(0);
  expect(await localCalls(page)).toEqual([{ action: "inspectRecovery" }]);
  const expected = await inspectedBinding(page);
  await button.click();
  await expect(page.getByText(/Reprise de l’initialisation exécutée\./)).toBeVisible();
  await expect(page.getByText("Aucun besoin de reprise signalé par l’inspection", { exact: true })).toBeVisible();
  await expect(button).toHaveCount(0);
  expect((await calls(page)).filter(c => c.command === "native_projection").map(c => c.args.request)).toEqual([{ action: "offline", request: { action: "inspectRecovery" } }, { action: "offline", request: { action: "resumeInitialization", expected } }]);
});

for (const wrongIdentity of [false, true]) test(`database recovery refusal or foreign identity cannot become a recovery success (${wrongIdentity})`, async ({ page }) => {
  await installDesktop(page, { projection: true, offline: { recovery: ["worktreeRollback"], recoveryRefusal: wrongIdentity ? null : "recoverDatabases", wrongDatabaseIdentity: wrongIdentity, forbidOrdinaryStatus: true } });
  await page.goto(route(first, "health"));
  await page.getByRole("button", { name: "Reprendre les bases de données locales", exact: true }).click();
  await expect(page.getByText(wrongIdentity ? /reprise ne correspond pas au dossier/ : /database rollback refused/).first()).toBeVisible();
  await expect(page.getByText(/Reprise des bases exécutée\./)).toHaveCount(0);
  await expect(page.getByText("Aucun besoin de reprise signalé par l’inspection", { exact: true })).toHaveCount(0);
  expect((await calls(page)).filter(c => c.command === "native_projection").map(c => c.args.request.action)).toEqual(["offline", "offline"]);
});

for (const [action, label, reason] of [
  ["recoverDatabases", "Reprendre les bases de données locales", "worktreeRollback"],
  ["resumeInitialization", "Reprendre l’initialisation du dossier", "initializing"],
]) for (const field of ["checkoutIdentity", "databaseIdentity", "markerDigest", "familyMarkerDigest"]) test(`recovery reply with equal logical IDs but changed native binding cannot report success (${action}, ${field})`, async ({ page }) => {
  await installDesktop(page, { projection: true, offline: { recovery: [reason], wrongRecoveryBinding: field, forbidOrdinaryStatus: true } });
  await page.goto(route(first, "health"));
  const button = page.getByRole("button", { name: label, exact: true });
  await expect(button).toBeEnabled();
  const expected = await inspectedBinding(page);
  await button.click();
  await expect(page.getByText(/La reprise ne correspond pas au dossier inspecté/).first()).toBeVisible();
  await expect(page.getByText(/Reprise (des bases|de l’initialisation) exécutée\./)).toHaveCount(0);
  await expect(page.getByText("Aucun besoin de reprise signalé par l’inspection", { exact: true })).toHaveCount(0);
  await expect(button).toHaveCount(0);
  expect(await localCalls(page)).toEqual([{ action: "inspectRecovery" }, { action, expected }]);
});

test("pending private conversion source is refused by typed eligibility without ordinary status", async ({ page }) => {
  await open(page, "worktrees", { shared: false, recovery: ["filesystemOperation"], forbidOrdinaryStatus: true });
  await expect(page.getByRole("status").getByText("recovery_required: filesystemOperation", { exact: true })).toBeVisible();
  await page.getByLabel("Dossier du stockage partagé", { exact: true }).fill("C:/family");
  await page.getByLabel("Destination de la copie", { exact: true }).fill(second);
  await expect(page.getByRole("button", { name: "Créer la copie en conservant la source", exact: true })).toBeDisabled();
  expect((await calls(page)).filter(c => c.command === "native_projection").map(c => c.args.request)).toEqual([{ action: "offline", request: { action: "inspectConversionSource" } }]);
});

for (const destination of ["worktreeId", "storeId", "missing"]) test(`Remote Restore cannot apply a foreign or absent destination binding (${destination})`, async ({ page }) => {
  await open(page, "files", destination === "missing" ? { missingRestoreDestination: true } : { foreignRestoreDestination: destination });
  await paths(page);
  await page.getByText("Observer une version distante exacte", { exact: true }).click();
  await page.getByLabel("Step distant exact", { exact: true }).fill(hash("d"));
  await page.getByLabel("Chemins de l’observation distante", { exact: true }).fill("chosen.txt");
  await page.getByRole("button", { name: "Observer les métadonnées", exact: true }).click();
  await expect(page.getByLabel("Version à vérifier", { exact: true })).toHaveValue(/observationDigest/);
  const networkBefore = await networkCalls(page);
  await page.getByRole("button", { name: "Préparer l’aperçu local", exact: true }).click();
  await expect(page.getByText(/restauration incohérent/).first()).toBeVisible();
  await expect(page.getByRole("button", { name: "Appliquer cette restauration locale", exact: true })).toHaveCount(0);
  expect((await localCalls(page)).filter(a => a.action === "previewRestore")[0].version.kind).toBe("remote");
  expect((await localCalls(page)).some(a => a.action === "applyRestore")).toBe(false);
  expect(await networkCalls(page)).toEqual(networkBefore);
});

for (const invalid of ["legacy", "failed", "malformed", "binding"]) test(`refused inspection blocks actual clicks on all current-dossier recovery buttons (${invalid})`, async ({ page }) => {
  const inspectionError = invalid === "legacy" ? "schema5 source cannot open without migration" : "inspection I/O failure";
  await installDesktop(page, { projection: true, offline: { ...(invalid === "malformed" ? { invalidInspection: true } : invalid === "binding" ? { invalidBinding: true } : { inspectionError }), forbidOrdinaryStatus: true } });
  await page.goto(route(first, "health"));
  await expect(page.getByText(["malformed", "binding"].includes(invalid) ? "Inspection locale invalide. Le dossier reste fermé." : inspectionError, { exact: true })).toBeVisible();
  for (const name of ["Reprendre la récupération locale", "Reprendre une création ou un déplacement", "Reprendre la réception locale", "Reprendre la restauration locale", "Reprendre la fusion locale", "Annuler la réservation de capture"]) {
    const button = page.getByRole("button", { name, exact: true });
    await expect(button).toBeDisabled();
    await button.click({ force: true });
  }
  expect((await calls(page)).filter(c => c.command === "native_projection").map(c => c.args.request)).toEqual([{ action: "offline", request: { action: "inspectRecovery" } }]);
});

test("valid private inspection cannot cancel a shared reservation or resume an unreported filesystem operation", async ({ page }) => {
  await installDesktop(page, { projection: true, offline: { shared: false, forbidOrdinaryStatus: true } });
  await page.goto(route(first, "health"));
  await expect(page.getByText("Aucun besoin de reprise signalé par l’inspection", { exact: true })).toBeVisible();
  for (const name of ["Annuler la réservation de capture", "Reprendre la restauration locale"]) await expect(page.getByRole("button", { name, exact: true })).toBeDisabled();
  expect(await localCalls(page)).toEqual([{ action: "inspectRecovery" }]);
});

test("mixed preparation history renders all 1024 active and 64 released entries without a false validation failure", async ({ page }) => {
  await open(page, "files", { mixedPreparations: true });
  await page.getByRole("button", { name: "Actualiser les préparations", exact: true }).click();
  await expect(page.getByText("Page 1 sur 44", { exact: true })).toBeVisible();
  for (let i = 0; i < 43; i++) await page.getByRole("button", { name: "Préparations suivantes", exact: true }).click();
  await expect(page.getByText("Page 44 sur 44", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Préparations suivantes", exact: true })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Reprendre cette préparation", exact: true })).toHaveCount(0);
  await expect(page.getByText(/Libérée ou annulée/)).toHaveCount(13);
  expect(await localCalls(page)).toEqual([{ action: "preparations" }]);
});

for (const [action, label, recovery, topLevel] of [
  ["recoverDatabases", "Reprendre les bases de données locales", ["familyRollback"], false],
  ["resumeInitialization", "Reprendre l’initialisation du dossier", ["initializing"], false],
  ["cancelSharedReservation", "Annuler la réservation de capture", [], false],
  ["resumeInstallation", "Reprendre la récupération locale", [], true],
  ["resumeNamespace", "Reprendre une création ou un déplacement", [], true],
  ["resumeRefresh", "Reprendre la réception locale", [], true],
  ["resumeRestore", "Reprendre la restauration locale", [], true],
  ["resumeRebase", "Reprendre la fusion locale", [], true],
]) test(`recovery carries reviewed A binding and refuses valid replacement B before any mutation (${action})`, async ({ page }) => {
  await installDesktop(page, { projection: true, offline: { recovery, ...(topLevel ? { pendingJournal: action } : {}), forbidOrdinaryStatus: true } });
  await page.goto(route(first, "health"));
  const button = page.getByRole("button", { name: label, exact: true });
  await expect(button).toBeEnabled();
  const expected = await inspectedBinding(page);
  const replacement = await page.evaluate(() => { window.__replaceProjectionRoot(); return window.__currentProjectionInspection(); });
  expect(replacement.identity).toEqual(expected.identity);
  expect(replacement.binding.identity).toEqual(replacement.identity);
  for (const field of ["checkoutIdentity", "databaseIdentity", "markerDigest", "familyMarkerDigest"]) expect(replacement.binding[field]).not.toBe(expected[field]);
  await button.click();
  await expect(page.getByText(/recovery binding changed before mutation/).first()).toBeVisible();
  expect(await page.evaluate(() => window.__recoveryMutationCount)).toBe(0);
  const sent = (await calls(page)).filter(c => c.command === "native_projection").map(c => c.args.request);
  expect(sent).toEqual([{ action: "offline", request: { action: "inspectRecovery" } }, topLevel ? { action, expected } : { action: "offline", request: { action, expected } }]);
  if (["recoverDatabases", "resumeInitialization"].includes(action)) await expect(button).toHaveCount(0);
  else await expect(button).toBeDisabled();
  await page.getByRole("button", { name: "Relire sans modifier le dossier", exact: true }).click();
  await expect(button).toBeEnabled();
  expect(await inspectedBinding(page)).toEqual(replacement.binding);
  expect(await page.evaluate(() => window.__recoveryMutationCount)).toBe(0);
});
