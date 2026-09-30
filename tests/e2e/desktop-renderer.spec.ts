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
  // The authority certificate goes with each call; only public preferences are remembered.
  const requests = await page.evaluate(() =>
    (window.__layrsInvocations ?? []).filter((call) => call.command === "native_remote").map((call) => call.args?.request as { caPem: string | null }),
  );
  expect(requests.every((request) => request.caPem?.includes("BEGIN CERTIFICATE"))).toBe(true);
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
  await expect(page.getByText("1 chemin réservé")).toBeVisible();
  await expect(page.getByRole("link", { name: "Layer distant à choisir" })).toBeVisible();

  await page.getByRole("button", { name: "Capturer les modifications autorisées" }).click();
  await expect(page.getByRole("region", { name: "Dernière capture locale" })).toContainText("docs/allowed.txt");
  expect((await workspaceActions(page)).find((request) => request.action === "capture")).toEqual({ action: "capture", expectedGeneration: 4 });

  // A full-project page is never offered, even through its address.
  await page.goto(projectPath("layers"));
  await expect(page.getByText("Page indisponible pour un accès limité")).toBeVisible();
  expect((await workspaceActions(page)).some((request) => String(request.action).toLowerCase().includes("layer"))).toBe(false);
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
