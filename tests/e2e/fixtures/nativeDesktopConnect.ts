import { expect, type Page } from "@playwright/test";
import { request as httpsRequest } from "node:https";

type Fixture = { origin: string; caPem: string; adminAccessToken: string };

/** Approves a device code as the fixture administrator, over the fixture TLS. */
export function approveDevice(fixture: Fixture, userCode: string) {
  return new Promise<void>((resolve, reject) => {
    const request = httpsRequest(
      new URL("/v3/auth/devices/approve", fixture.origin),
      { method: "POST", ca: fixture.caPem, headers: { Authorization: `Bearer ${fixture.adminAccessToken}`, "Content-Type": "application/json" } },
      (response) => {
        response.resume();
        response.on("end", () => (response.statusCode === 204 ? resolve() : reject(new Error(`Approval HTTP ${response.statusCode}`))));
      },
    );
    request.on("error", () => reject(new Error("TLS fixture approval failed")));
    request.end(JSON.stringify({ userCode }));
  });
}

/**
 * Connects the open project through the top-bar panel and the P02 flow: address,
 * enterprise CA in the advanced step, device code approved out of band, then the
 * automatic check. Returns to the project page.
 */
export async function connectDesktop(page: Page, fixture: Fixture) {
  await page.getByRole("button", { name: /^Synchronisation/ }).click();
  await page.getByRole("dialog", { name: "Synchronisation" }).getByRole("button", { name: /^(Connecter un serveur|Se connecter)…$/ }).click();
  await expect(page.getByRole("heading", { name: "Connexion au serveur", exact: true })).toBeVisible();
  await page.getByLabel("Adresse du serveur", { exact: true }).fill(fixture.origin);
  await page.getByText("Certificat d’entreprise (avancé)", { exact: true }).click();
  await page.getByLabel("Autorité de certification (PEM)", { exact: true }).fill(fixture.caPem);
  await page.getByRole("button", { name: "Se connecter", exact: true }).click();
  const code = page.getByLabel("Code de connexion", { exact: true });
  await expect(code).toBeVisible();
  await approveDevice(fixture, (await code.textContent()) ?? "");
  // The flow checks again at the interval requested by the server.
  await expect(page.getByRole("heading", { name: "Desktop est connecté", exact: true })).toBeVisible({ timeout: 30_000 });
  await page.getByRole("button", { name: "Revenir au projet", exact: true }).click();
}
