import { expect, test, type Page } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { fixtureHttp, type SharedFixture } from "./fixtures/nativeSharedClients";
import { webSignIn, webSignOut } from "./fixtures/nativeWebAccess";

async function fixture(): Promise<SharedFixture> {
  return JSON.parse(await readFile(process.env.LAYRS_NATIVE_UI_FIXTURE!, "utf8"));
}

async function startDevice(server: SharedFixture, label: string) {
  const response = await fixtureHttp(server, "POST", "/v3/auth/devices/start", { label });
  expect(response.status).toBe(200);
  return response.data as { deviceCode: string; userCode: string };
}

async function authorizedDevice(server: SharedFixture, deviceCode: string, principalId: string) {
  let issued: string | undefined;
  await expect.poll(async () => {
    const response = await fixtureHttp(server, "POST", "/v3/auth/devices/poll", { deviceCode });
    if (response.status === 200) issued = response.data.access_token;
    return response.status;
  }, { intervals: [5_500], timeout: 25_000 }).toBe(200);
  expect(typeof issued === "string" && /^[a-fA-F0-9]{64}$/.test(issued)).toBe(true);
  const identity = await fixtureHttp(server, "GET", "/v3/auth/whoami", undefined, issued);
  expect(identity.status).toBe(200);
  expect(identity.data.principal_id).toBe(principalId);
  expect((await fixtureHttp(server, "POST", "/v3/auth/logout", {}, issued)).status).toBe(204);
}

async function inspect(page: Page, code: string, label: string) {
  await page.getByLabel("Code de l'appareil").fill(code);
  await page.getByRole("button", { name: "Vérifier la demande" }).click();
  await expect(page.getByRole("heading", { name: "Confirmer l'accès" })).toBeVisible();
  await expect(page.locator("#device")).toHaveText(`Appareil : ${label}`);
}

test("device page supports manual login, an invalid code, explicit approval and a real device token", async ({ page }, testInfo) => {
  test.setTimeout(90_000);
  const server = await fixture();
  const script = await page.goto("/auth/device.js");
  expect(script?.status()).toBe(200);
  expect(script?.headers()["content-type"]).toContain("application/javascript");
  for (const [asset, mime] of [["style.css", "text/css"], ["appearance.js", "application/javascript"], ["icon.svg", "image/svg+xml"], ["inter.woff2", "font/woff2"], ["audiowide.woff2", "font/woff2"], ["jetbrains-mono.woff2", "font/woff2"]]) {
    const response = await page.evaluate(async name => {
      const result = await fetch(`/auth/device/${name}`);
      return { status: result.status, type: result.headers.get("content-type") };
    }, asset);
    expect(response.status, asset).toBe(200);
    expect(response.type, asset).toContain(mime);
  }

  await page.goto("/auth/device");
  await expect(page.getByRole("heading", { name: "Autoriser un appareil" })).toBeVisible();
  await page.getByLabel("Compte utilisé pour l'autorisation").selectOption("manual");
  await page.evaluate(() => document.fonts.ready);
  await page.screenshot({ path: testInfo.outputPath("device-dark.png"), animations: "disabled" });
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("device-mobile.png"), fullPage: true, animations: "disabled" });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.evaluate(() => {
    localStorage.setItem("layrs.appearance.v1", JSON.stringify({ theme: "light" }));
    window.dispatchEvent(new StorageEvent("storage", { key: "layrs.appearance.v1" }));
  });
  await page.screenshot({ path: testInfo.outputPath("device-light.png"), animations: "disabled" });
  await page.getByLabel("Compte", { exact: true }).fill(server.accounts.alice.login);
  await page.getByLabel("Mot de passe").fill(server.accounts.alice.password);
  await page.getByLabel("Code de l'appareil").fill("AAAAA-AAAAA");
  await page.getByRole("button", { name: "Vérifier la demande" }).click();
  await expect(page.getByRole("status")).toContainText("Demande refusée ou expirée");
  await expect(page.getByRole("heading", { name: "Confirmer l'accès" })).toBeHidden();

  const device = await startDevice(server, "Ordinateur manuel");
  await page.getByLabel("Mot de passe").fill(server.accounts.alice.password);
  await inspect(page, device.userCode, "Ordinateur manuel");
  await page.screenshot({ path: testInfo.outputPath("device-confirmation.png"), animations: "disabled" });
  expect((await fixtureHttp(server, "POST", "/v3/auth/devices/poll", { deviceCode: device.deviceCode })).status).toBe(202);
  await page.getByRole("button", { name: "Autoriser cet appareil" }).click();
  await expect(page.getByRole("status")).toContainText("Appareil autorisé");
  await authorizedDevice(server, device.deviceCode, server.accounts.alice.principalId);
});

test("a connected Web tab is offered without another password; inspection and cancellation preserve its session", async ({ page }) => {
  test.setTimeout(90_000);
  const server = await fixture();
  await page.goto("/#/projects");
  await webSignIn(page, server.accounts.alice);
  const devicePage = await page.context().newPage();
  try {
    await devicePage.goto("/auth/device");
    const account = devicePage.getByLabel("Compte utilisé pour l'autorisation");
    await expect(account).toHaveValue(/.+/);
    await expect(account).not.toHaveValue("manual");
    await expect(devicePage.locator("#credentials")).toBeHidden();
    await expect(account.locator("option")).toHaveCount(2);
    await expect(account.locator("option").first()).toContainText(server.accounts.alice.principalId);

    const cancelled = await startDevice(server, "Appareil annulé");
    await inspect(devicePage, cancelled.userCode, "Appareil annulé");
    expect((await fixtureHttp(server, "POST", "/v3/auth/devices/poll", { deviceCode: cancelled.deviceCode })).status).toBe(202);
    await devicePage.getByRole("button", { name: "Annuler" }).click();
    await expect(devicePage.getByRole("status")).toContainText("Autorisation annulée");
    await expect.poll(async () =>
      (await fixtureHttp(server, "POST", "/v3/auth/devices/poll", { deviceCode: cancelled.deviceCode })).status,
    { intervals: [5_500], timeout: 12_000 }).toBe(202);
    await expect(page.getByRole("button", { name: /^Compte de / })).toBeVisible();

    const approved = await startDevice(server, "Ordinateur depuis Web");
    await inspect(devicePage, approved.userCode, "Ordinateur depuis Web");
    await expect(devicePage.locator("#identity")).toContainText(server.accounts.alice.principalId);
    await devicePage.getByRole("button", { name: "Autoriser cet appareil" }).click();
    await expect(devicePage.getByRole("status")).toContainText("Appareil autorisé");
    await authorizedDevice(server, approved.deviceCode, server.accounts.alice.principalId);
    await expect(page.getByRole("button", { name: /^Compte de / })).toBeVisible();
  } finally {
    await devicePage.close();
    await webSignOut(page);
  }
});

test("two connected accounts require an explicit choice and authorize only the chosen account", async ({ page }) => {
  test.setTimeout(90_000);
  const server = await fixture();
  await page.goto("/#/projects");
  await webSignIn(page, server.accounts.alice);
  const bob = await page.context().newPage();
  const devicePage = await page.context().newPage();
  try {
    await bob.goto("/#/projects");
    await webSignIn(bob, server.accounts.bob);
    await devicePage.goto("/auth/device");
    const account = devicePage.getByLabel("Compte utilisé pour l'autorisation");
    await expect(account).toHaveValue("");
    await expect(account.locator("option")).toHaveCount(4);
    await expect(account.locator("option").first()).toContainText("Choisir le compte");
    await expect(account.locator("option").filter({ hasText: server.accounts.alice.principalId })).toHaveCount(1);
    await expect(account.locator("option").filter({ hasText: server.accounts.bob.principalId })).toHaveCount(1);
    const device = await startDevice(server, "Ordinateur Bob");
    await devicePage.getByLabel("Code de l'appareil").fill(device.userCode);
    await devicePage.getByRole("button", { name: "Vérifier la demande" }).click();
    await expect(devicePage.getByRole("heading", { name: "Confirmer l'accès" })).toBeHidden();
    const bobOption = account.locator("option").filter({ hasText: server.accounts.bob.principalId });
    await account.selectOption((await bobOption.getAttribute("value"))!);
    await inspect(devicePage, device.userCode, "Ordinateur Bob");
    await expect(devicePage.locator("#identity")).toContainText(server.accounts.bob.principalId);
    await devicePage.getByRole("button", { name: "Autoriser cet appareil" }).click();
    await expect(devicePage.getByRole("status")).toContainText("Appareil autorisé");
    await authorizedDevice(server, device.deviceCode, server.accounts.bob.principalId);
  } finally {
    await devicePage.close();
    await webSignOut(bob);
    await bob.close();
    await webSignOut(page);
  }
});

test("an inspected code cannot be approved through a signed-out tab; missing script keeps submit disabled", async ({ page }) => {
  test.setTimeout(90_000);
  const server = await fixture();
  await page.goto("/#/projects");
  await webSignIn(page, server.accounts.alice);
  const devicePage = await page.context().newPage();
  try {
    await devicePage.goto("/auth/device");
    const device = await startDevice(server, "Ancienne session");
    await inspect(devicePage, device.userCode, "Ancienne session");
    await webSignOut(page);
    await webSignIn(page, server.accounts.bob);
    await devicePage.getByRole("button", { name: "Autoriser cet appareil" }).click();
    await expect(devicePage.getByRole("status")).toContainText("La confirmation n'a pas été reçue", { timeout: 20_000 });
    expect((await fixtureHttp(server, "POST", "/v3/auth/devices/poll", { deviceCode: device.deviceCode })).status).toBe(202);
  } finally {
    await devicePage.close();
    await webSignOut(page);
  }

  await page.route("**/auth/device.js", route => route.abort());
  await page.goto("/auth/device");
  await expect(page.getByRole("heading", { name: "Autoriser un appareil" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Vérifier la demande" })).toBeDisabled();
});
