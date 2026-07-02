import { expect, test } from "@playwright/test";
import { writeFile } from "node:fs/promises";
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
    await expect(desktop.page.getByText("Layer: Main")).toBeVisible();
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

    await desktop.page.getByRole("tab", { name: /Steps/ }).click();
    await expect(desktop.page.getByRole("button", { name: /Step/ }).first()).toBeVisible();
    await desktop.pause();

    await desktop.page.getByRole("tab", { name: /Layers/ }).click();
    await desktop.page.getByLabel("New Layer").fill("Review Layer");
    await desktop.page.getByRole("button", { name: "Create from current" }).click();
    await expect(desktop.page.getByText("Layer created from current files")).toBeVisible();
    await expect(desktop.page.getByText("Layer: Review Layer")).toBeVisible();
    await desktop.pause();

    await desktop.page.getByRole("button", { name: /Main\s+Base Layer/ }).first().click();
    await expect(desktop.page.getByText("Layer switched")).toBeVisible();
    await expect(desktop.page.getByText("Layer: Main")).toBeVisible();
    await desktop.pause();
  } finally {
    await desktop.dispose();
  }
});
