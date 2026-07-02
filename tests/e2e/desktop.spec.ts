import { expect, test } from "./fixtures/desktopTauriMock";

test("captures working changes into a visible local Step without losing the latest diff", async ({
  desktopTauriMock,
  page,
  testSpace
}) => {
  desktopTauriMock.seedFolder(testSpace);
  desktopTauriMock.queueSelectedFolder(testSpace.rootPath);

  await page.goto("/#desktop-draft");
  await page.getByRole("button", { name: "Choose folder" }).first().click();
  await page.getByLabel("Space name").first().fill(testSpace.name);
  await page.getByRole("button", { name: "Initialize existing folder" }).click();

  await expect(page.getByRole("heading", { name: testSpace.name })).toBeVisible();
  await expect(page.getByRole("tab", { name: /Changes/ })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("button", { name: /README\.md/ })).toBeVisible();
  await expect(page.getByRole("button", { name: /src\/new-surface\.ts/ })).toBeVisible();

  await page.keyboard.press("Control+S");

  await expect(page.getByText("Step created")).toBeVisible();
  await expect(page.getByText("No changes to show.")).toBeVisible();
  await expect(page.getByRole("button", { name: /README\.md/ })).toHaveCount(0);

  await page.getByRole("tab", { name: /Steps/ }).click();

  await page.getByRole("button", { name: /Step step-/ }).click();
  await expect(page.getByRole("button", { name: /README\.md/ })).toBeVisible();
  await expect(page.getByText("README.md has local edits that must be captured.").first()).toBeVisible();
  expect(desktopTauriMock.commandLog().map((entry) => entry.command)).toContain("save_local_step");
});

test("creates a Layer from current files and switches back to Main visibly", async ({
  desktopTauriMock,
  page,
  testSpace
}) => {
  desktopTauriMock.seedFolder(testSpace);
  desktopTauriMock.queueSelectedFolder(testSpace.rootPath);

  await page.goto("/#desktop-draft");
  await page.getByRole("button", { name: "Choose folder" }).first().click();
  await page.getByLabel("Space name").first().fill(testSpace.name);
  await page.getByRole("button", { name: "Initialize existing folder" }).click();
  await page.getByRole("tab", { name: /Layers/ }).click();

  await page.getByLabel("New Layer").fill("Review Layer");
  await page.getByRole("button", { name: "Create from current" }).click();

  await expect(page.getByText("Layer created from current files")).toBeVisible();
  await expect(page.getByText("Layer: Review Layer")).toBeVisible();

  await page.getByRole("button", { name: /Main\s+Base Layer/ }).first().click();

  await expect(page.getByText("Layer switched")).toBeVisible();
  await expect(page.getByText("Layer: Main")).toBeVisible();
  expect(desktopTauriMock.commandLog().map((entry) => entry.command)).toEqual(
    expect.arrayContaining(["create_layer_from_current", "switch_layer"])
  );
});
