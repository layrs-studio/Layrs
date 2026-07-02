import { expect, test } from "@playwright/test";

test("shows Files and Steps in explicit mock mode without a redundant View action", async ({ page }) => {
  await page.goto("/spaces/space-studio/layers/layer-studio-base");

  await expect(page.getByText("Explicit dev/mock mode")).toBeVisible();
  await expect(page.getByRole("tab", { name: /Files/ })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("table", { name: "Layer files" })).toBeVisible();
  await expect(
    page.getByRole("row", { name: /Studio surface map.*docs\/studio\/surface-map\.md/ })
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "View" })).toHaveCount(0);

  await page.getByRole("tab", { name: /Steps/ }).click();

  await expect(page.getByText("Check terminology alignment")).toBeVisible();
  await expect(page.getByRole("button", { name: /docs\/studio\/surface-map\.md/ })).toBeVisible();
});
