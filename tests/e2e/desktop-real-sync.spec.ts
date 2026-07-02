import { expect, test } from "@playwright/test";
import { launchNativeTauriDesktop, makeNativeSpaceFolder } from "./fixtures/nativeTauriDesktop";

type WorkspaceResponse = {
  id: string;
  name: string;
};

const serverUrl = (process.env.LAYRS_E2E_SERVER_URL ?? "http://127.0.0.1:8787").replace(/\/$/, "");

test("visible Desktop publishes a Draft Space that Studio Web can review", async ({ page }, testInfo) => {
  const suffix = uniqueSuffix();
  const email = `desktop-real-${suffix}@layrs.local`;
  const password = `Correct horse ${suffix}!`;
  const workspaceName = `Real E2E Workspace ${suffix}`;
  const workspaceSlug = `real-e2e-${suffix}`;
  const spaceName = `Real Desktop Space ${suffix}`;
  const root = await makeNativeSpaceFolder(testInfo, [
    {
      path: "README.md",
      body: `# ${spaceName}\n\nPublished from the native Layrs Desktop app.\n`
    },
    {
      path: "src/game.ts",
      body: "export const source = 'native-desktop-real-sync';\n"
    }
  ]);

  const api = new CookieApi(serverUrl);
  await api.healthz();
  await api.postJson("/v1/auth/signup", {
    name: "Real Desktop E2E",
    email,
    password
  });
  await api.postJson<WorkspaceResponse>("/v1/workspaces", {
    name: workspaceName,
    slug: workspaceSlug
  });

  const desktop = await launchNativeTauriDesktop(testInfo, {
    selectedFolder: root,
    visual: true
  });

  try {
    await expect(desktop.page.getByRole("link", { name: /Local setup/ })).toBeVisible({
      timeout: 60_000
    });
    await desktop.pause();

    await desktop.page.getByRole("link", { name: /Settings/ }).click();
    await desktop.page.getByLabel("Endpoint").fill(serverUrl);
    await desktop.page.getByRole("button", { name: "Save settings" }).click();
    await expect(desktop.page.getByText("Settings saved")).toBeVisible();
    await desktop.pause();

    await desktop.page.getByRole("button", { name: /Connect device|Reconnect device/ }).click();
    const userCodeLocator = desktop.page.getByLabel("Device user code");
    await expect(userCodeLocator).toBeVisible();
    const userCode = (await userCodeLocator.textContent())?.trim();
    expect(userCode).toBeTruthy();
    await api.postForm("/v1/desktop/device/approve", { user_code: userCode! });
    await desktop.page.getByRole("button", { name: /Check now/ }).click();
    await expect(desktop.page.getByText(email).first()).toBeVisible({ timeout: 30_000 });
    await expect(desktop.page.getByText("Secure session")).toBeVisible();
    await desktop.pause();

    await desktop.page.getByRole("link", { name: /Local setup/ }).click();
    await desktop.page.getByRole("button", { name: "Choose folder" }).first().click();
    await desktop.page.getByLabel("Space name").first().fill(spaceName);
    await desktop.page.getByRole("button", { name: "Initialize existing folder" }).click();
    await expect(desktop.page.getByRole("heading", { name: spaceName })).toBeVisible({
      timeout: 30_000
    });
    await expect(desktop.page.getByText("Layer: Main")).toBeVisible();
    await desktop.pause();

    const workspaceSelect = desktop.page.getByLabel("Workspace target for Draft Local Space");
    await expect(workspaceSelect).toBeVisible();
    await workspaceSelect.selectOption({ label: workspaceName });
    await desktop.page.getByRole("button", { name: "Publish" }).click();
    await expect(desktop.page.getByText("Draft sent to Studio")).toBeVisible({ timeout: 60_000 });
    await expect(desktop.page.getByText("Linked")).toBeVisible({ timeout: 30_000 });
    await desktop.pause();

    await page.goto("/");
    await page.getByLabel("Email").fill(email);
    await page.getByLabel("Password").fill(password);
    await page.locator("form").getByRole("button", { name: "Login" }).click();

    await expect(page.getByRole("button", { name: new RegExp(spaceName) })).toBeVisible({
      timeout: 30_000
    });
    await page.getByRole("button", { name: new RegExp(spaceName) }).click();
    await expect(page.getByRole("tab", { name: /Files/ })).toHaveAttribute("aria-selected", "true");
    await expect(page.getByRole("row", { name: /README\.md/ })).toBeVisible();

    await page.getByRole("tab", { name: /Steps/ }).click();
    await expect(page.getByRole("button", { name: /README\.md/ })).toBeVisible({
      timeout: 30_000
    });
    await page.waitForTimeout(1_000);
  } finally {
    await desktop.dispose();
  }
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

  async getJson<T = unknown>(path: string): Promise<T> {
    return this.request<T>("GET", path);
  }

  async postJson<T = unknown>(path: string, body: unknown): Promise<T> {
    return this.request<T>("POST", path, {
      "content-type": "application/json"
    }, JSON.stringify(body));
  }

  async postForm<T = unknown>(path: string, body: Record<string, string>): Promise<T> {
    return this.request<T>("POST", path, {
      "content-type": "application/x-www-form-urlencoded"
    }, new URLSearchParams(body).toString());
  }

  private async request<T>(
    method: string,
    path: string,
    headers: Record<string, string> = {},
    body?: string
  ): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, {
      method,
      headers: {
        ...headers,
        ...(this.cookie ? { cookie: this.cookie } : {})
      },
      body
    });
    this.captureCookie(response);
    if (!response.ok) {
      const text = await response.text();
      throw new Error(`${method} ${path} failed with HTTP ${response.status}: ${text}`);
    }
    const text = await response.text();
    const contentType = response.headers.get("content-type") ?? "";
    if (!text || !contentType.includes("json")) {
      return text as T;
    }
    return JSON.parse(text) as T;
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

function uniqueSuffix() {
  return Math.random().toString(16).slice(2, 10);
}
