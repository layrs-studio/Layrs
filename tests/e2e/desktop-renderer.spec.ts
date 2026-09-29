import { expect, test } from "@playwright/test";

type Invocation = { command: string; args?: Record<string, unknown> };

declare global {
  interface Window {
    __layrsInvocations?: Invocation[];
  }
}

test("renders the native workbench and reports the missing Tauri runtime", async ({ page }) => {
  await page.goto("/");

  await expect(page.getByRole("heading", { name: "Votre projet, son histoire" })).toBeVisible();
  const open = page.getByRole("region", { name: "Ouvrir un projet" });
  await expect(open.getByRole("button", { name: "Ouvrir", exact: true })).toBeDisabled();
  await open.getByLabel("Dossier").fill("C:/projects/demo");
  await open.getByRole("button", { name: "Ouvrir", exact: true }).click();

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
      }
    };
  });
  await page.goto("/");

  const open = page.getByRole("region", { name: "Ouvrir un projet" });
  await open.getByRole("button", { name: "Parcourir…" }).click();
  await expect(open.getByLabel("Dossier")).toHaveValue("C:/projects/demo");
  await open.getByRole("button", { name: "Ouvrir", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("renderer stub: native_workspace is not available");

  const invocations = await page.evaluate(() => window.__layrsInvocations ?? []);
  expect(invocations).toEqual([
    { command: "select_folder", args: { initialDirectory: undefined } },
    { command: "native_checkout_kind", args: { root: "C:/projects/demo" } },
    { command: "native_workspace", args: { root: "C:/projects/demo", request: { action: "status" } } }
  ]);
});
