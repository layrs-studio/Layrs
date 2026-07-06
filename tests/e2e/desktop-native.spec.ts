import { expect, test } from "@playwright/test";
import { access, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { launchNativeTauriDesktop, makeNativeSpaceFolder } from "./fixtures/nativeTauriDesktop";

test("visible Desktop UI initializes a Local Space, captures a Step and switches Layers", async ({}, testInfo) => {
  const root = await makeNativeSpaceFolder(testInfo, [
    {
      path: "README.md",
      body: "# Native Desktop Visual E2E\n\nThis file is initialized through the visible UI.\n"
    },
    {
      path: "src/game.ts",
      body: "export const nativeDesktopVisualE2E = true;\n"
    }
  ]);

  const desktop = await launchNativeTauriDesktop(testInfo, {
    selectedFolder: root,
    visual: true
  });

  try {
    await expect(desktop.page.getByRole("link", { name: /Local setup/ })).toBeVisible({
      timeout: 60_000
    });
    await desktop.pause();

    await desktop.page.getByRole("link", { name: /Local setup/ }).click();
    await desktop.pause();

    await desktop.page.getByRole("button", { name: "Choose folder" }).first().click();
    await desktop.page.getByLabel("Space name").first().fill("Native Desktop Visual E2E");
    await desktop.pause();
    await desktop.page.getByRole("button", { name: "Initialize existing folder" }).click();

    await expect(desktop.page.getByRole("heading", { name: "Native Desktop Visual E2E" })).toBeVisible({
      timeout: 30_000
    });
    await expect(desktop.page.getByRole("button", { name: /Main\s+Base Layer/ })).toBeVisible();
    await desktop.pause();

    await writeFile(resolve(root, "src", "visible-change.ts"), "export const visibleChange = true;\n", "utf8");
    await desktop.page.getByRole("button", { name: "Scan" }).click();

    await expect(desktop.page.getByRole("button", { name: /src\/visible-change\.ts/ })).toBeVisible({
      timeout: 30_000
    });
    await desktop.pause();

    await desktop.page.keyboard.press("Control+S");
    await expect(desktop.page.getByText("Step created")).toBeVisible();
    await desktop.pause();

    await desktop.page.keyboard.press("Control+S");
    await expect(desktop.page.getByText("Choose a Workspace in Space Settings before publishing this Draft Local Space.")).toBeVisible();
    await desktop.page.getByRole("button", { name: "Back to Space" }).click();
    await desktop.pause();

    await desktop.page.getByRole("tab", { name: /Steps/ }).click();
    await expect(desktop.page.getByRole("button", { name: /Step/ }).first()).toBeVisible();
    await desktop.pause();

    await desktop.page.getByLabel("Search or create Layer").fill("Review Layer");
    await desktop.page.getByRole("button", { name: 'Create "Review Layer" from current' }).click();
    await expect(desktop.page.getByText("Layer created from current files")).toBeVisible();
    await expect(desktop.page.getByRole("button", { name: /Review Layer\s+Parent: Main/ })).toBeVisible();
    await desktop.pause();

    await desktop.page.getByRole("button", { name: /Main\s+Base Layer/ }).first().click();
    await expect(desktop.page.getByText("Layer switched")).toBeVisible();
    await expect(desktop.page.getByRole("button", { name: /Main\s+Base Layer/ })).toBeDisabled();
    await desktop.pause();
  } finally {
    await desktop.dispose();
  }
});

test("visible Desktop UI creates an empty Local Space and scans a new file", async ({}, testInfo) => {
  const root = await makeNativeSpaceFolder(testInfo, []);
  const desktop = await launchNativeTauriDesktop(testInfo, {
    selectedFolder: root,
    visual: true
  });

  try {
    await expect(desktop.page.getByRole("link", { name: /Local setup/ })).toBeVisible({
      timeout: 60_000
    });
    await desktop.page.getByRole("link", { name: /Local setup/ }).click();
    await desktop.pause();

    await desktop.page.getByRole("button", { name: "Choose folder" }).nth(1).click();
    await desktop.page.getByLabel("Space name").nth(1).fill("Empty Native Space");
    await desktop.page.getByRole("button", { name: "Create empty local Space" }).click();

    await expect(desktop.page.getByRole("heading", { name: "Empty Native Space" })).toBeVisible({
      timeout: 30_000
    });
    await expect(desktop.page.getByRole("button", { name: /Main\s+Base Layer/ })).toBeVisible();
    await expect(desktop.page.getByRole("tab", { name: /Changes\s+0/ })).toBeVisible();
    await expect(desktop.page.getByRole("tab", { name: /Steps\s+0/ })).toBeVisible();
    await desktop.pause();

    await writeFile(resolve(root, "spawned-from-test.txt"), "created after init\n", "utf8");
    await desktop.page.getByRole("button", { name: "Scan", exact: true }).click();
    await expect(desktop.page.getByRole("button", { name: /spawned-from-test\.txt/ })).toBeVisible({
      timeout: 30_000
    });
    await expect(desktop.page.getByRole("tab", { name: /Changes\s+1/ })).toBeVisible();
    await desktop.pause();
  } finally {
    await desktop.dispose();
  }
});

test("visible Desktop UI blocks active Layer delete and forgets local metadata without deleting files", async ({}, testInfo) => {
  const root = await makeNativeSpaceFolder(testInfo, [
    {
      path: "keep.txt",
      body: "this project file must stay\n"
    }
  ]);
  const desktop = await launchNativeTauriDesktop(testInfo, {
    selectedFolder: root,
    visual: true
  });

  try {
    await expect(desktop.page.getByRole("link", { name: /Local setup/ })).toBeVisible({
      timeout: 60_000
    });
    await desktop.page.getByRole("link", { name: /Local setup/ }).click();
    await desktop.page.getByRole("button", { name: "Choose folder" }).first().click();
    await desktop.page.getByLabel("Space name").first().fill("Forget Safety Space");
    await desktop.page.getByRole("button", { name: "Initialize existing folder" }).click();
    await expect(desktop.page.getByRole("heading", { name: "Forget Safety Space" })).toBeVisible({
      timeout: 30_000
    });

    await desktop.page.getByRole("tab", { name: /Layer settings/ }).click();
    await expect(desktop.page.getByRole("button", { name: "Delete layer" })).toBeDisabled();
    await desktop.pause();

    await desktop.page.getByRole("button", { name: "Space settings" }).click();
    await desktop.page.getByRole("button", { name: "Forget local" }).click();
    const forgetDialog = desktop.page.getByRole("dialog", { name: /Forget Forget Safety Space/ });
    await expect(forgetDialog).toBeVisible();
    await forgetDialog.getByRole("button", { name: "Forget local" }).click();
    await expect(desktop.page.getByText("Select a Local Space before opening Space settings.")).toBeVisible({
      timeout: 30_000
    });
    await access(resolve(root, "keep.txt"));
    await expect(access(resolve(root, ".layrs"))).rejects.toThrow();
    await desktop.pause();
  } finally {
    await desktop.dispose();
  }
});
