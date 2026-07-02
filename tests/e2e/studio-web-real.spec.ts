import { expect, test } from "@playwright/test";

type WorkspaceResponse = {
  id: string;
  name: string;
};

const serverUrl = (process.env.LAYRS_E2E_SERVER_URL ?? "http://127.0.0.1:18887").replace(/\/$/, "");

test("real Studio Web opens a Space with Files and Steps tabs", async ({ page }) => {
  const suffix = Math.random().toString(16).slice(2, 10);
  const email = `studio-web-${suffix}@layrs.local`;
  const password = `Correct horse ${suffix}!`;
  const workspaceName = `Web E2E Workspace ${suffix}`;
  const spaceName = `Web E2E Space ${suffix}`;
  const api = new CookieApi(serverUrl);

  await api.healthz();
  await api.postJson("/v1/auth/signup", {
    name: "Studio Web E2E",
    email,
    password
  });
  const workspace = await api.postJson<WorkspaceResponse>("/v1/workspaces", {
    name: workspaceName,
    slug: `web-e2e-${suffix}`
  });
  await api.postJson(`/v1/workspaces/${workspace.id}/spaces`, {
    name: spaceName,
    description: "Created by the real Studio Web E2E setup."
  });

  await page.goto("/");
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password").fill(password);
  await page.locator("form").getByRole("button", { name: "Login" }).click();

  await expect(page.getByRole("button", { name: new RegExp(spaceName) })).toBeVisible({
    timeout: 30_000
  });
  await page.getByRole("button", { name: new RegExp(spaceName) }).click();

  await expect(page.getByRole("tab", { name: /Files/ })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("button", { name: "View" })).toHaveCount(0);
  await expect(page.getByRole("tab", { name: /Steps/ })).toBeVisible();

  await page.getByRole("tab", { name: /Steps/ }).click();
  await expect(page.getByRole("tab", { name: /Steps/ })).toHaveAttribute("aria-selected", "true");
});

class CookieApi {
  private cookie = "";

  constructor(private readonly baseUrl: string) {}

  async healthz() {
    const response = await fetch(`${this.baseUrl}/healthz`);
    if (!response.ok) {
      throw new Error(`Layrs server is not available at ${this.baseUrl}: HTTP ${response.status}`);
    }
  }

  async postJson<T = unknown>(path: string, body: unknown): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        ...(this.cookie ? { cookie: this.cookie } : {})
      },
      body: JSON.stringify(body)
    });
    this.captureCookie(response);
    if (!response.ok) {
      throw new Error(`POST ${path} failed with HTTP ${response.status}: ${await response.text()}`);
    }
    return (await response.json()) as T;
  }

  private captureCookie(response: Response) {
    const setCookie = response.headers.get("set-cookie");
    if (!setCookie) {
      return;
    }
    this.cookie = setCookie
      .split(",")
      .map((value) => value.split(";")[0]?.trim())
      .filter(Boolean)
      .join("; ");
  }
}
