import { expect, test } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { seedNativeProof } from "./fixtures/nativeProofSeed";
import {fixtureHttp,sharedNativeClients} from "./fixtures/nativeSharedClients";
import {webOpenProjectByName,webReleaseGate,webRestrictedAccess,webRestrictedAccessWithDesktop,webSignIn,webSignOut} from "./fixtures/nativeWebAccess";

// Non-sensitive preferences the Web may keep; everything else (tokens, drafts
// left behind, project data) must be absent from browser storage.
const allowedStorageKeys=["layrs.appearance.v1","layrs.shell.v1"];

test("Web uses the real native identity, project, review, proof and account services", async ({ page },testInfo) => {
  // The restricted-account path needs a Desktop runtime: shared clients, or the explicit opt-in below.
  const restrictedWithDesktop=process.env.LAYRS_NATIVE_WEB_RESTRICTED==="1";
  test.setTimeout(restrictedWithDesktop?600_000:240_000);
  page.setDefaultTimeout(15_000);
  if (!process.env.LAYRS_NATIVE_UI_FIXTURE) throw new Error("Native TLS fixture is mandatory");
  const fixture = JSON.parse(await readFile(process.env.LAYRS_NATIVE_UI_FIXTURE, "utf8"));
  // The Linux Web job has no native Desktop runtime. The explicit combined
  // recipe additionally requires a built CLI and supported Tauri environment.
  const shared=process.env.LAYRS_NATIVE_SHARED_CLIENTS==="1"?await sharedNativeClients(testInfo,fixture):null;
  if(shared)console.log("[shared-native] Desktop and CLI: distinct Stores, exact references and bytes verified");
  try{
  const proof = await seedNativeProof(fixture);
  await page.goto("/");
  await webSignIn(page,fixture.accounts.alice);
  const project=await webOpenProjectByName(page,fixture);
  await expect(page.getByRole("navigation",{name:"Fil d’Ariane"})).toContainText(project.name);

  // P06 — files and access rights, opened by default for a server project.
  const files = page.getByRole("region", { name: "Fichiers du projet", exact: true });
  await files.getByRole("button", { name: /^docs/ }).click();
  const readme = files.getByRole("list", { name: "Contenu du dossier" }).getByRole("button", { name: /readme\.txt/ });
  await expect(readme).toContainText("Lecture et écriture");
  await readme.click();
  const selection = files.getByRole("complementary", { name: "Détail de la sélection" });
  await expect(selection.getByRole("heading", { name: "readme.txt" })).toBeVisible();
  await expect(selection.getByText("docs/readme.txt", { exact: true })).toBeVisible();

  // P07 — history of that path, reached from the file without copying a hash.
  await selection.getByRole("button", { name: "Historique du fichier", exact: true }).click();
  const history = page.getByRole("region", { name: "Historique publié", exact: true });
  await expect(history.getByLabel("Chemin exact", { exact: true })).toHaveValue("docs/readme.txt");
  const captures = history.getByRole("list", { name: "Captures publiées" });
  await expect(captures.locator(":scope > li")).toHaveCount(2);
  await expect(captures.locator(":scope > li").first()).toContainText("Version 0.1.0-ui");
  await expect(captures.locator(":scope > li").last()).toContainText("Première capture");
  await captures.locator(":scope > li").last().getByText("Détails techniques", { exact: true }).click();
  await expect(captures.locator(":scope > li").last().getByText(fixture.firstStepId, { exact: true })).toBeVisible();
  if(shared){
    for(const markerName of [shared.markerName,shared.cliMarkerName])
      await expect(captures.locator(":scope > li").first()).toContainText(`Repère ${markerName}`);
  }

  // P08 on the Web — read-only capture with its comparison to the parent.
  await captures.locator(":scope > li").first().getByRole("button").first().click();
  const capture = page.getByRole("region", { name: "Capture publiée", exact: true });
  await expect(capture.getByText("Consultation seule sur le Web", { exact: false })).toBeVisible();
  await expect(capture.getByRole("region", { name: "Comparaison du fichier sélectionné" })).toContainText("Modification proposée pour la revue.");
  await capture.getByRole("button", { name: "Retour à l’historique", exact: true }).click();
  await expect(page.getByRole("region", { name: "Historique publié", exact: true }).getByLabel("Chemin exact", { exact: true })).toHaveValue("docs/readme.txt");

  // P12/P13 — open the review by its title, then comment on the exact version.
  await page.getByRole("link", { name: "Revues", exact: true }).click();
  await page.getByRole("list", { name: "Liste des revues" }).getByRole("button", { name: /Vérifier la modification du guide/ }).click();
  await expect(page.getByRole("heading", { name: "Vérifier la modification du guide", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Approuver cette version", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Intégrer", exact: true })).toBeDisabled();
  const review = page.getByRole("region", { name: "Discussion", exact: true });
  const comment = `Commentaire Web ${Date.now()}`;
  await review.getByLabel("Commentaire", { exact: true }).fill(comment);
  await review.getByRole("button", { name: "Commenter", exact: true }).click();
  await expect(review.getByRole("list", { name: "Fil de la revue" }).getByText(comment, { exact: true })).toBeVisible();
  await expect(review.getByLabel("Commentaire", { exact: true })).toHaveValue("");

  // Lost answer: the server records the comment but the browser never gets the reply.
  // The text stays in the composer and the retry reuses its key: exactly one comment.
  const lost = `Retour perdu ${Date.now()}`;
  let recorded = 0;
  await page.route("**/reviews/*/actions", async (route) => {
    // Same account and body, sent from Node straight to the TLS fixture; the browser only sees a reset.
    const sent = await fixtureHttp(fixture, "POST", new URL(route.request().url()).pathname, JSON.parse(route.request().postData() ?? "{}"));
    recorded = sent.status;
    await route.abort("connectionreset");
  }, { times: 1 });
  await review.getByLabel("Commentaire", { exact: true }).fill(lost);
  await review.getByRole("button", { name: "Commenter", exact: true }).click();
  await expect(review.getByText("Envoi non confirmé", { exact: true })).toBeVisible();
  await expect(review.getByLabel("Commentaire", { exact: true })).toHaveValue(lost);
  expect(recorded).toBe(200);
  await review.getByRole("button", { name: "Commenter", exact: true }).click();
  await expect(review.getByRole("list", { name: "Fil de la revue" }).getByText(lost, { exact: true })).toHaveCount(1);
  await expect(review.getByLabel("Commentaire", { exact: true })).toHaveValue("");
  const reread = await fixtureHttp(fixture, "GET", `/v3/spaces/${fixture.spaceId}/reviews/${fixture.reviewId}`);
  expect(reread.data.actions.filter((action: any) => action.body === lost).length).toBe(1);
  if(shared){
    const cliReview=await shared.cli(["review",fixture.reviewId],true);
    expect(cliReview.actions.some((action:any)=>action.body===comment)).toBe(true);
    console.log("[shared-native] Web: exact references, history and comment reread by CLI verified");
  }

  // Files tab: the proposal against its parent capture.
  await page.getByRole("tab", { name: "Fichiers" }).click();
  await expect(page.getByRole("region", { name: "Fichiers proposés" }).getByRole("region", { name: "Comparaison du fichier sélectionné" })).toContainText("Modification proposée pour la revue.");

  // P13b — CI proof and log of the seeded run.
  await page.getByRole("tab", { name: "Validations" }).click();
  const validations = page.getByRole("region", { name: "Validations", exact: true });
  await validations.getByRole("radio", { name: "Tout le projet" }).click();
  await validations.getByRole("listitem").filter({ hasText: proof.proofObjectId })
    .getByRole("button", { name: "Consulter la preuve et le journal", exact: true }).click();
  const run = page.getByRole("region", { name: "Preuve et journal de validation", exact: true });
  await expect(run.getByText("Réussi", { exact: true })).toBeVisible();
  await expect(run.getByRole("region", { name: "Journal de l’exécution" }).getByText("UI proof fixture", { exact: true })).toBeVisible();
  console.log("[shared-native] Web: exact seeded proof rendered");
  await run.getByRole("button", { name: "Retour à la revue", exact: true }).click();
  await expect(page.getByRole("tab", { name: "Validations" })).toHaveAttribute("aria-selected", "true");

  // A new proposal version makes decisions on the previous one visibly obsolete,
  // and a link to the old version opens it read-only instead of the latest.
  // The seeded protection changed the project after version 1: decisions on it are refused.
  await page.getByRole("tab", { name: /Discussion/ }).click();
  await expect(page.getByText("Le projet a changé depuis cette proposition", { exact: true })).toBeVisible();
  const propose = async (version: number, capture: { label: string } | { reference: string }) => {
    await page.getByRole("button", { name: "Nouvelle proposition", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Nouvelle proposition" });
    await dialog.getByRole("button", { name: `Reprendre le titre et le Layer de la version ${version - 1}`, exact: true }).click();
    if ("label" in capture) await dialog.getByLabel("Capture proposée", { exact: true }).selectOption({ label: capture.label });
    else {
      await dialog.getByLabel("Capture proposée", { exact: true }).selectOption({ label: "Autre capture publiée (référence technique)…" });
      await dialog.getByLabel("Référence de la capture proposée", { exact: true }).fill(capture.reference);
    }
    await dialog.getByRole("button", { name: "Proposer cette nouvelle version", exact: true }).click();
    await expect(page.getByLabel("Version affichée", { exact: true })).toHaveValue(String(version));
  };
  await propose(2, { reference: fixture.firstStepId });
  await review.getByLabel("Commentaire", { exact: true }).fill("À revoir avant intégration.");
  await review.getByRole("button", { name: "Demander des modifications", exact: true }).click();
  await expect(review.getByText("Modifications demandées", { exact: true })).toBeVisible();
  await propose(3, { label: "Version 0.1.0-ui" });
  await expect(page.getByText("Nouvelle proposition : version 3", { exact: true })).toBeVisible();
  await expect(review.getByText("Décision sur la version 2 · ne compte plus", { exact: true })).toBeVisible();
  await page.getByLabel("Version affichée", { exact: true }).selectOption("2");
  await expect(page.getByText("Version 2 affichée en lecture seule", { exact: true })).toBeVisible();
  await expect(review.getByLabel("Commentaire", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Ouvrir la version actuelle", exact: true }).click();
  await expect(page.getByLabel("Version affichée", { exact: true })).toHaveValue("3");

  // A comment draft survives a reload (which asks to sign in again) for this account and exact version.
  const kept = `Brouillon conservé ${Date.now()}`;
  await review.getByLabel("Commentaire", { exact: true }).fill(kept);
  await expect(review.getByText("Brouillon enregistré sur cet appareil.", { exact: true })).toBeVisible();
  await page.reload();
  await page.getByLabel("Identifiant", { exact: true }).fill(fixture.accounts.alice.login);
  await page.getByLabel("Mot de passe", { exact: true }).fill(fixture.accounts.alice.password);
  await page.getByRole("button", { name: "Se connecter", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Vérifier la modification du guide", exact: true })).toBeVisible();
  await review.getByRole("button", { name: "Restaurer le brouillon", exact: true }).click();
  await expect(review.getByLabel("Commentaire", { exact: true })).toHaveValue(kept);
  await review.getByRole("button", { name: "Commenter", exact: true }).click();
  await expect(review.getByRole("list", { name: "Fil de la revue" }).getByText(kept, { exact: true })).toBeVisible();

  // P14 — release publication refused by the Gate, journal unchanged.
  await webReleaseGate(page,fixture);
  if(shared){
    await page.getByRole("tab",{name:/Repères/}).click();
    for(const markerName of [shared.markerName,shared.cliMarkerName])
      await expect(page.getByRole("list",{name:"Repères publiés"}).getByRole("heading",{name:markerName,exact:true})).toBeVisible();
  }

  // Team and accounts stay on the legacy administration surface for now.
  await page.getByRole("link", { name: "Équipe et accès", exact: true }).click();
  await page.getByText("Équipe, protections et intégrations", { exact: true }).click();
  await page.getByText("Comptes et automatisations", { exact: true }).click();
  await page.getByRole("button", { name: "Charger les comptes", exact: true }).click();
  await expect(page.getByText("Bob — relecteur test", { exact: true })).toBeVisible();

  // Session credentials live only in React memory; storage keeps preferences only.
  const storage = await page.evaluate(() => {
    const dump = (store: Storage) => Array.from({ length: store.length }, (_, index) => store.key(index)!).map((key) => [key, store.getItem(key) ?? ""]);
    return { local: dump(localStorage), session: dump(sessionStorage) };
  });
  for (const [key, value] of [...storage.local, ...storage.session]) {
    expect(allowedStorageKeys, `unexpected storage key ${key}`).toContain(key);
    for (const secret of [fixture.accounts.alice.password, fixture.adminAccessToken, fixture.spaceId, fixture.reviewId, fixture.accounts.alice.principalId, comment])
      expect(value.includes(secret), `storage ${key} holds session or project data`).toBe(false);
  }
  if(shared)await webRestrictedAccess(page,fixture,shared.remote);
  else if(restrictedWithDesktop)await webRestrictedAccessWithDesktop(page,fixture,testInfo);
  await webSignOut(page);
  await page.reload();
  await expect(page.getByRole("button", { name: "Se connecter", exact: true })).toBeVisible();
  }finally{await shared?.close();}
});
