import { expect, test } from "@playwright/test";

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
