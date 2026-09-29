import { expect, test } from "@playwright/test";

test("renders native Studio sign-in and refuses a non-HTTPS origin", async ({ page }) => {
  await page.goto("/");

  await expect(page.getByRole("heading", { name: "Projets et revues" })).toBeVisible();
  const signIn = page.getByRole("button", { name: "Se connecter", exact: true });
  await expect(signIn).toBeDisabled();
  await page.getByLabel("Compte", { exact: true }).fill("alice");
  await page.getByLabel("Mot de passe", { exact: true }).fill("not-sent");
  await signIn.click();

  await expect(page.getByRole("alert")).toContainText("Ouvrez Studio sur l’adresse HTTPS");
  await expect(page.getByLabel("Identifiant du projet")).toHaveCount(0);
});
