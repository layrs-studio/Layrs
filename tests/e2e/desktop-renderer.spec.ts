import { expect, test } from "@playwright/test";
import { hash, installWorkStub, projectPath, remoteActions, root as stubRoot, workspaceActions } from "./fixtures/desktopRendererStub";

type Invocation = { command: string; args?: Record<string, unknown> };

declare global {
  interface Window {
    __layrsInvocations?: Invocation[];
  }
}

const root = "C:/projects/demo";
const projectRoute = `/#/projects/${encodeURIComponent(root)}/changes`;

for (const [kind, button, action, destination] of [
  ["sameProject", "Lier et ouvrir ce projet", "linkExisting", "changes"],
  ["otherProject", "Synchroniser ce projet local", "linkExisting", "sync"],
  ["unpublishedProject", "Préparer la publication de ce projet", "prepareExistingPublication", "sync"],
] as const) {
  test(`offers ${kind} for an occupied clone destination without cloning`, async ({ page }) => {
    await installWorkStub(page, { origin: "https://layrs.example.test", signedIn: true, destination: kind });
    await page.goto("/#/projects/new?mode=clone");
    await page.getByRole("button", { name: /Site vitrine/ }).click();
    await page.getByLabel("Dossier", { exact: true }).fill(root);
    await expect(page.getByRole("button", { name: "Récupérer le projet", exact: true })).toBeDisabled();
    await page.getByRole("button", { name: button, exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`/projects/${encodeURIComponent(root)}/${destination}`));
    const calls = await remoteActions(page);
    expect(calls.some((item) => item.action === action && item.space === (kind === "sameProject" ? "space:site" : "space:demo"))).toBe(true);
    expect(calls.some((item) => item.action === "cloneCancellable")).toBe(false);
    expect(calls.some((item) => item.action === "publishCancellable" || item.action === "provision")).toBe(false);
    if (kind === "unpublishedProject") await expect(page.getByRole("heading", { name: "Publier ce projet local" })).toBeVisible();
  });
}

test("rejects arbitrary files and ignores a late inspection of another folder", async ({ page }) => {
  await installWorkStub(page, { origin: "https://layrs.example.test", signedIn: true, destination: "blocked", holdDestination: true });
  await page.goto("/#/projects/new?mode=clone");
  await page.getByRole("button", { name: /Site vitrine/ }).click();
  const folder = page.getByLabel("Dossier", { exact: true });
  await folder.fill(root);
  await expect(page.getByText("Ce dossier ne peut pas recevoir ce clonage", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Récupérer le projet", exact: true })).toBeDisabled();
  await folder.fill("C:/projects/old");
  await expect.poll(async () => page.evaluate(() => Boolean(window.__layrsReleaseDestination))).toBe(true);
  await folder.fill("C:/projects/empty");
  await expect(page.getByRole("button", { name: "Récupérer le projet", exact: true })).toBeEnabled();
  await page.evaluate(() => window.__layrsReleaseDestination?.());
  await expect(page.getByRole("button", { name: "Récupérer le projet", exact: true })).toBeEnabled();
  await expect(page.getByText("Ce dossier ne peut pas recevoir ce clonage", { exact: true })).toHaveCount(0);
});

test("keeps an automatically captured document visible and cannot capture it twice", async ({ page }) => {
  await page.addInitScript(({ initial, root }) => {
    const capture = `blake3:${"a".repeat(64)}`;
    const genesis = `blake3:${"b".repeat(64)}`;
    const changes = [{ ...initial.changes[0], path: "test.txt", status: "added", beforeBytes: null }];
    let captured = localStorage.getItem("test-auto-captured") === "1";
    window.__layrsInvocations = [];
    window.__TAURI_INTERNALS__ = { invoke: async <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
      window.__layrsInvocations!.push({ command, args });
      if (command === "native_checkout_kind") return "complete" as T;
      const action = args?.request as { action: string; selector?: string; step?: string };
      if (command !== "native_workspace") throw new Error("No server configured in this test");
      if (action.action === "stepHistory") return { steps: [{ stepId: action.selector, parentStepIds: action.selector === capture ? [genesis] : [] }], observations: { [capture]: [{ observedAt: "2026-10-06T00:00:00Z", captureReason: "automatic_idle", worktreeId: "worktree:demo" }] } } as T;
      if (action.action === "stepDiff") return { stepId: action.step, changes: action.step === capture ? changes : [] } as T;
      if (action.action === "captureAutomatic") { captured = true; localStorage.setItem("test-auto-captured", "1"); }
      if (action.action === "status" || action.action === "captureAutomatic") return {
        ...initial,
        changes: captured ? [] : changes,
        state: { ...initial.state, worktree: { ...initial.state.worktree, checkoutRoot: root }, layers: [{ ...initial.state.layers[0], settings: { automaticCapture: true, automaticCaptureIdleMs: 500 } }], view: { worktrees: [{ worktreeId: "worktree:demo", activeLayerId: "layer:main", baseStepId: captured ? capture : genesis, status: "ready" }] } },
      } as T;
      throw new Error(`Unexpected action ${action.action}`);
    } };
  }, { initial: status, root });
  await page.goto(projectRoute);
  await expect(page.getByRole("region", { name: "Fichiers modifiés" }).getByText("test.txt", { exact: true })).toBeVisible();
  await expect(page.getByRole("region", { name: "Fichiers de la dernière capture" }).getByText("test.txt", { exact: true })).toBeVisible({ timeout: 12_000 });
  await expect(page.getByRole("button", { name: "Créer une capture", exact: true })).toBeDisabled();
  await expect(page.getByRole("region", { name: "Comparaison" }).getByText("hello world")).toBeVisible();
  await page.reload();
  await expect(page.getByRole("region", { name: "Fichiers de la dernière capture" }).getByText("test.txt", { exact: true })).toBeVisible();
  expect(await workspaceActions(page)).toContainEqual(expect.objectContaining({ action: "stepDiff" }));
});

test("a received capture is not presented as a local automatic capture", async ({ page }) => {
  await page.addInitScript(({ initial, root }) => {
    const step = `blake3:${"c".repeat(64)}`;
    window.__TAURI_INTERNALS__ = { invoke: async <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
      if (command === "native_checkout_kind") return "complete" as T;
      if (command !== "native_workspace") throw new Error("No server configured");
      const action = args?.request as { action: string; step?: string };
      if (action.action === "stepHistory") return { steps: [{ stepId: step, parentStepIds: [] }], observations: {} } as T;
      if (action.action === "stepDiff") return { stepId: step, changes: initial.changes } as T;
      return { ...initial, changes: [], state: { ...initial.state, worktree: { ...initial.state.worktree, checkoutRoot: root }, view: { worktrees: [{ worktreeId: "worktree:demo", activeLayerId: "layer:main", baseStepId: step, status: "ready" }] } } } as T;
    } };
  }, { initial: status, root });
  await page.goto(projectRoute);
  await expect(page.getByRole("region", { name: "Fichiers de la capture de référence" }).getByText("readme.txt", { exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: /Dernière capture/ })).toHaveCount(0);
  await expect(page.getByText("La dernière capture a enregistré ces modifications.", { exact: false })).toHaveCount(0);
});

test("does not clone into a changed folder after server creation completes", async ({ page }) => {
  await installWorkStub(page, { origin: "https://layrs.example.test", signedIn: true });
  await page.addInitScript(() => {
    const original = window.__TAURI_INTERNALS__!.invoke;
    window.__TAURI_INTERNALS__!.invoke = async <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
      const request = args?.request as { action?: { action?: string } } | undefined;
      if (command === "native_remote" && request?.action?.action === "createProject") {
        await new Promise<void>((resolve) => { window.__layrsReleaseDestination = resolve; });
      }
      return original<T>(command, args);
    };
  });
  await page.goto("/#/projects/new?mode=server");
  await page.getByLabel("Nom du projet", { exact: true }).fill("Projet différé");
  await page.getByLabel("Dossier", { exact: true }).fill("C:/projects/first");
  await page.getByRole("button", { name: "Créer et récupérer" }).click();
  await expect.poll(() => page.evaluate(() => Boolean(window.__layrsReleaseDestination))).toBe(true);
  await page.getByLabel("Dossier", { exact: true }).fill("C:/projects/second");
  await page.evaluate(() => window.__layrsReleaseDestination?.());
  await expect(page.getByText(/le dossier ou la connexion a changé/)).toBeVisible();
  expect((await remoteActions(page)).some((item) => item.action === "cloneCancellable")).toBe(false);
});

/** Minimal native status: one Layer and one modified text file. */
const status = {
  state: {
    space: { name: "Démo", spaceId: "space:demo" },
    worktree: { worktreeId: "worktree:demo", checkoutRoot: root },
    operation: { operationId: "op:1", summary: "Initialisation", kind: "initialize", createdAt: "2026-09-01T10:00:00Z" },
    layers: [{ layerId: "layer:main", name: "main", settings: { automaticCapture: false, automaticCaptureIdleMs: 600000 } }],
    view: { worktrees: [{ worktreeId: "worktree:demo", activeLayerId: "layer:main", status: "clean" }] },
  },
  needsRepair: false,
  changes: [
    {
      path: "docs/readme.txt",
      status: "modified",
      binary: false,
      textOmitted: false,
      beforeBytes: 6,
      afterBytes: 12,
      textDiff: [
        { op: "delete", text: "hello\n" },
        { op: "insert", text: "hello world\n" },
      ],
    },
  ],
  operations: [{ operationId: "op:1", summary: "Initialisation", kind: "initialize", createdAt: "2026-09-01T10:00:00Z" }],
  markers: [],
  releases: [],
};

test("reports the missing Tauri runtime without leaving the project list", async ({ page }) => {
  await page.goto("/");

  await expect(page.getByRole("heading", { name: "Vos projets" })).toBeVisible();
  await page.getByRole("button", { name: "Ouvrir un dossier" }).first().click();
  await expect(page.getByRole("alert")).toContainText("only inside the Tauri desktop runtime");
});

test("drives only the native Tauri commands", async ({ page }) => {
  await page.addInitScript(() => {
    const invocations: Invocation[] = [];
    window.__layrsInvocations = invocations;
    window.__TAURI_INTERNALS__ = {
      invoke: async <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
        invocations.push({ command, args });
        if (command === "select_folder") return "C:/projects/demo" as T;
        if (command === "native_checkout_kind") return "complete" as T;
        throw new Error(`renderer stub: ${command} is not available`);
      },
    };
  });
  await page.goto("/");

  await page.getByRole("button", { name: "Ouvrir un dossier" }).first().click();
  await expect(page.getByRole("alert")).toContainText("renderer stub: native_workspace is not available");

  const invocations = await page.evaluate(() => window.__layrsInvocations ?? []);
  expect(invocations).toEqual([
    { command: "select_folder", args: { initialDirectory: undefined } },
    { command: "native_checkout_kind", args: { root } },
    { command: "native_workspace", args: { root, request: { action: "status" } } },
  ]);
});

test("shows local changes side by side and captures them", async ({ page }) => {
  await page.addInitScript((snapshot) => {
    const invocations: Invocation[] = [];
    window.__layrsInvocations = invocations;
    let current = snapshot;
    window.__TAURI_INTERNALS__ = {
      invoke: async <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
        invocations.push({ command, args });
        if (command === "native_checkout_kind") return "complete" as T;
        const request = (args?.request ?? {}) as { action?: string };
        if (command === "native_workspace" && request.action === "status") return current as T;
        if (command === "native_workspace" && request.action === "capture") {
          current = { ...current, changes: [] };
          return current as T;
        }
        throw new Error(`renderer stub: ${command} ${request.action ?? ""}`);
      },
    };
  }, status);
  await page.goto(projectRoute);

  await expect(page.getByRole("heading", { name: "Changements" })).toBeVisible();
  await expect(page.getByText("1 fichier modifié dans main")).toBeVisible();
  const comparison = page.getByRole("region", { name: "Comparaison" });
  await expect(comparison.getByRole("heading", { name: "docs/readme.txt" })).toBeVisible();
  await expect(comparison.getByText("hello world")).toBeVisible();

  await page.getByRole("button", { name: "Créer une capture" }).click();
  await expect(page.getByText("Capture locale créée")).toBeVisible();
  await expect(page.getByText("Votre travail local est capturé dans main")).toBeVisible();
  const actions = await page.evaluate(() =>
    (window.__layrsInvocations ?? [])
      .map((call) => (call.args?.request as { action?: string } | undefined)?.action)
      .filter((action) => action && action !== "status"),
  );
  expect(actions).toEqual(["capture"]);
});

test("connects Desktop with the device flow, then publishes and cancels from the top-bar panel", async ({ page }) => {
  await installWorkStub(page, { publish: "hold" });
  await page.goto(projectPath("changes"));

  const trigger = page.getByRole("button", { name: /^Synchronisation/ });
  await trigger.click();
  const panel = page.getByRole("dialog", { name: "Synchronisation" });
  await expect(panel.getByText("Aucun serveur", { exact: true })).toBeVisible();
  await expect(panel.getByRole("button", { name: "Publier" })).toHaveAttribute("aria-disabled", "true");
  await panel.getByRole("button", { name: "Connecter un serveur…" }).click();

  await expect(page.getByRole("heading", { name: "Connexion au serveur" })).toBeVisible();
  await page.getByLabel("Adresse du serveur").fill("https://layrs.example.test");
  await page.getByText("Certificat d’entreprise (avancé)").click();
  await page.getByLabel("Autorité de certification (PEM)").fill("-----BEGIN CERTIFICATE-----\nfictif\n-----END CERTIFICATE-----");
  await page.getByRole("button", { name: "Se connecter", exact: true }).click();
  await expect(page.getByLabel("Code de connexion")).toHaveText("WDJB-MJHT");
  await page.getByRole("button", { name: "Vérifier maintenant" }).click();
  await expect(page.getByText(/En attente de votre autorisation/)).toBeVisible();
  await page.getByRole("button", { name: "Vérifier maintenant" }).click();
  await expect(page.getByRole("heading", { name: "Desktop est connecté" })).toBeVisible();
  await expect(page.getByText("Alice Démo")).toBeVisible();
  await page.getByRole("button", { name: "Revenir au projet" }).click();

  await expect(page.getByRole("heading", { name: "Changements" })).toBeVisible();
  await trigger.click();
  await expect(panel.getByText("Connecté", { exact: true })).toBeVisible();
  await panel.getByRole("button", { name: "Publier" }).click();
  const transfer = panel.getByRole("region", { name: "Transfert en cours" });
  await expect(transfer.getByText("Publication des captures")).toBeVisible();
  // Closing the panel never stops the transfer; the top bar keeps showing it.
  await page.keyboard.press("Escape");
  await expect(trigger).toContainText("Publication des captures");
  await trigger.click();
  await transfer.getByRole("button", { name: "Annuler le transfert" }).click();
  await expect(transfer.getByText(/^Arrêt demandé\./)).toBeVisible();
  await page.evaluate(() => window.__layrsRelease?.("cancelled"));
  await expect(panel.getByRole("status").filter({ hasText: "Publication annulée avant envoi de la transaction." })).toBeVisible();
  await expect(panel.getByText("Annulé", { exact: true })).toBeVisible();

  const actions = await remoteActions(page);
  const publish = actions.find((action) => action.action === "publishCancellable");
  const cancel = actions.find((action) => action.action === "cancelTransfer");
  expect(publish).toMatchObject({ integrate: false });
  expect(cancel?.transferId).toBe(publish?.transferId);
  // The initial session probe may precede adding a private authority; all following calls use it.
  const requests = await page.evaluate(() =>
    (window.__layrsInvocations ?? []).filter((call) => call.command === "native_remote").map((call) => call.args?.request as { caPem: string | null }),
  );
  const firstPinned = requests.findIndex((request) => request.caPem?.includes("BEGIN CERTIFICATE"));
  expect(firstPinned).toBeGreaterThanOrEqual(0);
  expect(requests.slice(firstPinned).every((request) => request.caPem?.includes("BEGIN CERTIFICATE"))).toBe(true);
  const memory = await page.evaluate(() => localStorage.getItem("layrs.desktop.servers.v1") ?? "");
  expect(memory).toContain("https://layrs.example.test");
  expect(memory).not.toMatch(/token/i);
});

test("keeps an unconfirmed publication distinct from a failure", async ({ page }) => {
  await installWorkStub(page, { origin: "https://layrs.example.test", signedIn: true, publish: "unconfirmed" });
  await page.goto(projectPath("sync"));
  await expect(page.getByRole("main").getByText("Connecté", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Publier", exact: true }).click();
  await expect(page.getByText("La publication n’a pas été confirmée.").first()).toBeVisible();
  await expect(page.getByText(/Relancez Publier : la même publication est rejouée sans doublon/)).toBeVisible();
  await expect(page.getByText("À confirmer", { exact: true })).toBeVisible();
});

test("resolves a conflict in the merge workspace through a preview bound to the engine state", async ({ page }) => {
  await installWorkStub(page, { conflicted: true });
  await page.goto(projectPath("changes"));
  await page.getByRole("button", { name: "Résoudre les conflits" }).click();

  await expect(page.getByRole("heading", { name: "Fusion", exact: true })).toBeVisible();
  const list = page.getByRole("navigation", { name: "Conflits" });
  await expect(list.getByRole("link")).toHaveCount(2);
  await expect(list.getByText("Chemins liés par un renommage")).toBeVisible();
  await expect(page.getByText("2 conflits restants")).toBeVisible();
  const comparison = page.getByRole("region", { name: "Comparaison des variantes" });
  await expect(comparison.getByText('const value = "topic";')).toBeVisible();

  const variant = page.getByRole("listitem", { name: "Variante 2" });
  await variant.getByRole("button", { name: "Prévisualiser cette variante" }).click();
  const preview = page.getByRole("region", { name: "Aperçu de fusion" });
  await expect(preview.getByText("1 conflit restera après cette opération.")).toBeVisible();
  await preview.getByRole("button", { name: "Appliquer cet aperçu" }).click();
  await expect(page.getByRole("heading", { name: "Préparer une fusion" })).toBeVisible();

  const applied = (await workspaceActions(page)).find((request) => request.action === "applyResolution");
  expect(applied).toMatchObject({ layer: "layer:main", choices: { "conflict:code": "term:topic" }, expectedOperation: "op:1", expectedStep: hash("9") });
});

test("refuses a merge preview that became stale and keeps the conflicts", async ({ page }) => {
  await installWorkStub(page, { conflicted: true, stale: true });
  await page.goto(projectPath("merge"));
  await page.getByRole("listitem", { name: "Variante 1" }).getByRole("button", { name: "Prévisualiser cette variante" }).click();
  await page.getByRole("region", { name: "Aperçu de fusion" }).getByRole("button", { name: "Appliquer cet aperçu" }).click();
  await expect(page.getByRole("alert").filter({ hasText: "Le projet a changé depuis l’aperçu." })).toBeVisible();
  await expect(page.getByRole("navigation", { name: "Conflits" }).getByRole("link")).toHaveCount(2);
});

test("opens a limited-access folder in the same shell with its real capabilities", async ({ page }) => {
  await installWorkStub(page, { kind: "projection" });
  await page.goto(projectPath("changes"));

  await expect(page.getByRole("heading", { name: "Changements" })).toBeVisible();
  const nav = page.getByRole("complementary", { name: "Navigation principale" });
  await expect(nav.getByRole("link", { name: "Fichiers" })).toBeVisible();
  await expect(nav.getByRole("link", { name: "Layers" })).toHaveCount(0);
  await expect(nav.getByRole("link", { name: "Import / export Git" })).toHaveCount(0);
  await expect(page.getByText("1 chemin réservé")).toBeVisible();
  await expect(page.getByRole("link", { name: "Layer distant à choisir" })).toBeVisible();

  await page.getByRole("button", { name: "Capturer les modifications autorisées" }).click();
  await expect(page.getByRole("region", { name: "Dernière capture locale" })).toContainText("docs/allowed.txt");
  expect((await workspaceActions(page)).find((request) => request.action === "capture")).toEqual({ action: "capture", expectedGeneration: 4 });

  // A full-project page is never offered, even through its address.
  await page.goto(projectPath("layers"));
  await expect(page.getByText("Page indisponible pour un accès limité")).toBeVisible();
  expect((await workspaceActions(page)).some((request) => String(request.action).toLowerCase().includes("layer"))).toBe(false);
  for (const view of ["git", "reviews", "published", "access", "admin", "remote-settings"]) {
    await page.goto(projectPath(view));
    await expect(page.getByText("Page indisponible pour un accès limité")).toBeVisible();
    expect(await page.evaluate(() => (window.__layrsInvocations ?? []).some((call) => call.command === "native_git"))).toBe(false);
  }
});

test("opens Git from the keyboard palette and keeps the compact layout usable", async ({ page }, testInfo) => {
  await installWorkStub(page, {});
  await page.setViewportSize({ width: 960, height: 800 });
  await page.goto(projectPath("changes"));
  const opener = page.getByRole("button", { name: "Rechercher…", exact: true });
  await opener.focus();
  await page.keyboard.press("Control+k");
  const palette = page.getByRole("dialog", { name: "Palette de commandes" });
  await expect(palette.getByRole("combobox", { name: "Rechercher" })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(palette).not.toBeVisible();
  await expect(opener).toBeFocused();
  await page.keyboard.press("Control+k");
  await palette.getByRole("combobox", { name: "Rechercher" }).fill("export Git");
  await page.keyboard.press("Enter");
  await expect(page.getByRole("heading", { name: "Git", exact: true })).toBeVisible();
  await expect(palette).not.toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("git-compact-dark.png"), animations: "disabled" });
  await page.getByRole("radio", { name: "Clair", exact: true }).click();
  await page.screenshot({ path: testInfo.outputPath("git-compact-light.png"), animations: "disabled" });
  expect(await page.evaluate(() => (window.__layrsInvocations ?? []).some((call) => call.command === "native_git"))).toBe(false);
});

test("retrieves a team project chosen by name from the server catalog", async ({ page }) => {
  await installWorkStub(page, { origin: "https://layrs.example.test", signedIn: true });
  await page.goto("/#/projects/new?mode=clone");
  await page.getByRole("button", { name: /Site vitrine/ }).click();
  await page.getByLabel("Dossier", { exact: true }).fill(stubRoot);
  await page.getByRole("button", { name: "Récupérer le projet" }).click();
  await expect(page.getByRole("heading", { name: "Changements" })).toBeVisible();
  const clone = (await remoteActions(page)).find((action) => action.action === "cloneCancellable");
  expect(clone).toMatchObject({ space: "space:site", name: "Site vitrine" });
});

test("loads team projects after connecting from New Project without choosing a folder", async ({ page }) => {
  await installWorkStub(page, { origin: "https://layrs.example.test" });
  await page.goto("/#/projects/new?mode=clone");
  await expect(page.getByRole("heading", { name: "Connecter Desktop à un serveur Layrs" })).toBeVisible();
  await page.getByRole("button", { name: "Se connecter", exact: true }).click();
  await expect(page.getByLabel("Code de connexion")).toBeVisible();
  await page.getByRole("button", { name: "Vérifier maintenant" }).click();
  await page.getByRole("button", { name: "Vérifier maintenant" }).click();
  await expect(page.getByRole("button", { name: /Site vitrine/ })).toBeVisible();
  await expect(page.getByText("Chargement de l’espace…")).toHaveCount(0);
  const calls = await remoteActions(page);
  expect(calls.some((action) => action.action === "listContexts")).toBe(true);
  expect(calls.some((action) => action.action === "listSpaces" && action.contextId === "workspace:a")).toBe(true);
});

test("shows an empty state when the connected account has no accessible space", async ({ page }) => {
  await installWorkStub(page, { origin: "https://layrs.example.test", signedIn: true, emptyContexts: true });
  await page.goto("/#/projects/new?mode=clone");
  await expect(page.getByText("Aucun espace accessible sur ce serveur")).toBeVisible();
  await expect(page.getByText("Chargement de l’espace…")).toHaveCount(0);
  expect((await remoteActions(page)).some((action) => action.action === "listSpaces")).toBe(false);
});

test("Desktop lists only the selected company, personal or organisation projects", async ({ page }) => {
  await installWorkStub(page, { origin: "https://layrs.example.test", signedIn: true, multiContexts: true });
  await page.goto("/#/projects");
  const selector = page.getByRole("button", { name: "Organisation : Atelier" });
  await expect(selector).toBeVisible();
  const sidebar = page.getByRole("complementary", { name: "Navigation principale" });
  await expect(sidebar.getByText("Organisation", { exact: true })).toBeVisible();
  expect(await sidebar.locator(".layrs-nav-section .layrs-nav > li").first().textContent()).toContain("Tous les projets");
  await expect(page.getByRole("button", { name: /Site vitrine/ })).toBeVisible();
  await sidebar.getByRole("button", { name: "Projets personnels" }).click();
  await expect(page.getByRole("button", { name: /Site vitrine/ })).toHaveCount(0);
  await expect(page.getByRole("button", { name: /Notes privées/ })).toBeVisible();
  await page.getByRole("button", { name: "Organisation : choisir" }).click();
  const menu = page.getByRole("menu", { name: "Organisations" });
  await expect(menu.getByRole("menuitemradio")).toHaveCount(2);
  await menu.getByRole("menuitemradio", { name: "Design" }).click();
  await expect(page.getByRole("button", { name: /Notes privées/ })).toHaveCount(0);
  await expect(page.getByRole("button", { name: /Maquettes/ })).toBeVisible();
  const calls = await remoteActions(page);
  expect(calls.filter((action) => action.action === "listSpaces").map((action) => action.contextId)).toEqual(["workspace:a", "workspace:personal", "workspace:org"]);
});

test("Desktop creates a project in the selected personal context before cloning it", async ({ page }) => {
  await installWorkStub(page, { origin: "https://layrs.example.test", signedIn: true, multiContexts: true });
  await page.goto("/#/projects");
  await page.getByRole("button", { name: "Projets personnels" }).click();
  await page.goto("/#/projects/new?mode=server");
  await page.getByLabel("Nom du projet", { exact: true }).fill("Carnet personnel");
  await page.getByLabel("Dossier", { exact: true }).fill(stubRoot);
  await page.getByRole("button", { name: "Créer et récupérer" }).click();
  await expect(page.getByRole("heading", { name: "Changements" })).toBeVisible();
  const calls = await remoteActions(page);
  expect(calls.find((action) => action.action === "createProject")?.request).toMatchObject({ contextId: "workspace:personal", name: "Carnet personnel", spaceSlug: "carnet-personnel" });
  expect(calls.find((action) => action.action === "cloneCancellable")?.space).toBe("space:new");
});

test("organisation creation is the final menu action and works from the keyboard", async ({ page }) => {
  await installWorkStub(page, { origin: "https://layrs.example.test", signedIn: true, multiContexts: true, canCreateOrganisation: true });
  await page.goto("/#/projects");
  const selector = page.getByRole("button", { name: "Organisation : Atelier" });
  await selector.focus();
  await page.keyboard.press("ArrowDown");
  const menu = page.getByRole("menu", { name: "Organisations" });
  await expect(menu).toBeVisible();
  await page.keyboard.press("End");
  await expect(menu.getByRole("menuitem")).toBeFocused();
  await expect(menu.getByRole("menuitem")).toHaveText("Créer une nouvelle organisation");
  await page.keyboard.press("Enter");
  await expect(page.getByRole("heading", { name: "Nouvelle organisation" })).toBeVisible();
});

test("the organisation menu remains clickable inside the mobile navigation drawer", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 700 });
  await installWorkStub(page, { origin: "https://layrs.example.test", signedIn: true, multiContexts: true });
  await page.goto("/#/projects");
  await page.getByRole("button", { name: "Ouvrir la navigation" }).click();
  await page.getByRole("button", { name: "Organisation : Atelier" }).click();
  await page.getByRole("menuitemradio", { name: "Design" }).click();
  await expect(page.getByRole("button", { name: /Maquettes/ })).toBeVisible();
});

test("personal projects remain reachable without the multi-organisation selector", async ({ page }) => {
  await installWorkStub(page, { origin: "https://layrs.example.test", signedIn: true, personalProjects: true });
  await page.goto("/#/projects");
  await expect(page.getByRole("button", { name: /^Organisation :/ })).toHaveCount(0);
  await page.getByRole("button", { name: "Projets personnels" }).click();
  await expect(page.getByRole("button", { name: /Notes privées/ })).toBeVisible();
  await expect(page.getByRole("link", { name: "Tous les projets" })).not.toHaveAttribute("aria-current", "page");
  await page.getByRole("link", { name: "Tous les projets" }).click();
  await expect(page.getByRole("button", { name: /Site vitrine/ })).toBeVisible();
  await expect(page.getByRole("link", { name: "Tous les projets" })).toHaveAttribute("aria-current", "page");
  const calls = await remoteActions(page);
  expect(calls.filter((action) => action.action === "listSpaces").map((action) => action.contextId)).toEqual(["workspace:a", "workspace:personal", "workspace:a"]);
});

test("the organisation selector disappears when administration switches to single mode", async ({ page }) => {
  await installWorkStub(page, { origin: "https://layrs.example.test", signedIn: true, multiContexts: true });
  await page.goto("/#/projects");
  await page.getByRole("button", { name: "Organisation : Atelier" }).click();
  await page.getByRole("menuitemradio", { name: "Design" }).click();
  await expect(page.getByRole("button", { name: /Maquettes/ })).toBeVisible();
  await page.evaluate(() => {
    window.__layrsSetMultiOrganisation?.(false);
    window.dispatchEvent(new Event("layrs-organisations-changed"));
  });
  await expect(page.getByRole("button", { name: /^Organisation :/ })).toHaveCount(0);
  await expect(page.getByRole("button", { name: /Site vitrine/ })).toBeVisible();
  await expect(page.getByRole("button", { name: "Projets personnels" })).toBeVisible();
});

test("Desktop follows the server of an opened local project", async ({ page }) => {
  const folderOrigin = "https://folder-b.example.test";
  await installWorkStub(page, { origin: folderOrigin, lastOrigin: "https://folder-a.example.test", linkedRecent: true, signedIn: true, multiContexts: true });
  await page.goto("/#/projects");
  await expect(page.getByRole("button", { name: /Site vitrine/ })).toBeVisible();
  await page.getByRole("button", { name: /C:\/projects\/demo/ }).click();
  await expect(page.getByRole("heading", { name: "Changements" })).toBeVisible();
  await expect.poll(async () => page.evaluate(() => (window.__layrsInvocations ?? [])
    .filter((call) => call.command === "native_remote" && (call.args?.request as { action?: { action?: string } })?.action?.action === "listContexts")
    .at(-1)?.args?.request as { origin?: string } | undefined)).toMatchObject({ origin: folderOrigin });
});

test("a late check from a closed folder cannot restore its server", async ({ page }) => {
  const first = "https://folder-a.example.test";
  const second = "https://folder-b.example.test";
  await installWorkStub(page, { origin: first, secondOrigin: second, signedIn: true, multiContexts: true, delayedFolderOrigin: first });
  await page.goto(projectPath("changes"));
  await expect.poll(async () => page.evaluate((root) => (window.__layrsInvocations ?? []).some((call) =>
    call.command === "native_remote" && call.args?.root === root &&
    (call.args?.request as { action?: { action?: string } })?.action?.action === "whoami"), stubRoot)).toBe(true);
  await page.goto(`/#/projects/${encodeURIComponent("C:/projects/second")}/changes`);
  await expect.poll(async () => page.evaluate(() => (window.__layrsInvocations ?? [])
    .filter((call) => call.command === "native_remote" && (call.args?.request as { action?: { action?: string } })?.action?.action === "listContexts")
    .at(-1)?.args?.request as { origin?: string } | undefined)).toMatchObject({ origin: second });
  await page.evaluate(() => window.__layrsReleaseWhoami?.());
  const lastOrigin = await page.evaluate(() => JSON.parse(localStorage.getItem("layrs.desktop.servers.v1") ?? "{}").last);
  expect(lastOrigin).toBe(second);
  const latest = await page.evaluate(() => (window.__layrsInvocations ?? [])
    .filter((call) => call.command === "native_remote" && (call.args?.request as { action?: { action?: string } })?.action?.action === "listContexts")
    .at(-1)?.args?.request as { origin?: string } | undefined);
  expect(latest?.origin).toBe(second);
});

test("local projects remain visible when the context catalogue fails", async ({ page }) => {
  await installWorkStub(page, { origin: "https://layrs.example.test", signedIn: true, linkedRecent: true, contextsUnavailable: true });
  await page.goto("/#/projects");
  await expect(page.getByRole("heading", { name: /Projets locaux/ })).toBeVisible();
  await expect(page.getByRole("button", { name: /C:\/projects\/demo/ })).toHaveCount(1);
  await expect(page.locator('a[href="#/projects/C%3A%2Fprojects%2Fdemo"]')).toBeVisible();
});
