import { expect, test } from "@playwright/test";

test("renders the sign-in page and refuses a non-HTTPS origin", async ({ page }) => {
  await page.goto("/");

  await expect(page.getByRole("heading", { name: "Retrouvez votre équipe" })).toBeVisible();
  const signIn = page.getByRole("button", { name: "Se connecter", exact: true });
  await expect(signIn).toBeDisabled();
  await page.getByLabel("Identifiant", { exact: true }).fill("alice");
  await page.getByLabel("Mot de passe", { exact: true }).fill("not-sent");
  await signIn.click();

  await expect(page.getByRole("alert")).toContainText("Ouvrez Studio sur l’adresse HTTPS");
  await expect(page.getByLabel("Référence du projet")).toHaveCount(0);
});

test("applies the stored theme before the first paint", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("layrs.appearance.v1", JSON.stringify({ theme: "light" })));
  await page.goto("/");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
});
