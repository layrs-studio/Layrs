import { expect, test, type Page } from "@playwright/test";

type WorkspaceResponse = {
  id: string;
  name: string;
};

type SpaceResponse = {
  id: string;
  currentLayerId: string;
  name: string;
};

type LayerResponse = {
  id: string;
  name: string;
};

type StudioSnapshotResponse = {
  layers: Array<{ id: string; name: string; spaceId: string }>;
};

type ArtifactCollectionResponse = {
  items: Array<{ id?: string; location?: string; path?: string; logicalPath?: string }>;
};

type WeaveRequestResponse = {
  appliedSteps?: string[];
  conflicts?: Array<{
    conflictId?: string;
    resolvedFileObjectId?: string | null;
    status?: string;
  }>;
  status?: string;
};

type WeaveRequestListResponse = Array<{ weaveId?: string }> | {
  items?: Array<{ weaveId?: string }>;
};

type TestFile = {
  body: string;
  digest: string;
  path: string;
};

const serverUrl = (process.env.LAYRS_E2E_SERVER_URL ?? "http://127.0.0.1:18887").replace(/\/$/, "");

const TEXT_FIXTURES = {
  baseReadme: { body: "base-readme-v1\n", digest: "blake3:61028eaf891fd13aad54869fe98774705bbe8b5671f84624c6b6383a0ffb8798" },
  baseShared: { body: "base-shared-v1\n", digest: "blake3:dd04a9793803bd006fd27ab62002bb2fd7dba8ecad24947b6cdd48882d72aee1" },
  featureA: { body: "source-feature-a\n", digest: "blake3:6d79b81cbb7accbe11068172f56e3b12942ab9a82fcaf60efbafe41e580220ea" },
  featureB: { body: "source-feature-b\n", digest: "blake3:271ce88974b451c6440c69ef55eac96968329bcfad0e93677cd84c21e56cb159" },
  conflictBase: { body: "conflict-base\nshared\n", digest: "blake3:039d51484a232c8b50386d8e262d3204f779543a08f8b59af955571800588049" },
  conflictExisting: { body: "conflict-existing\nshared\n", digest: "blake3:a3b96d8ae512efd3263b69fc45b9cb06e1cf35be7627d43804c6f1f2c9669fc1" },
  conflictIncoming: { body: "conflict-incoming\nshared\n", digest: "blake3:01ac401a435705b09bc4272f01a53cff17a041d7237e3e59d185680469f21ec9" }
} as const;

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

test("real Studio Web applies a non-conflicting Weave Request into the target Layer", async ({ page }) => {
  const suffix = uniqueSuffix();
  const api = new CookieApi(serverUrl);
  const setup = await createWorkspaceSpace(api, suffix, "Web Weave Nonconflict");

  const baseTreeId = await publishFiles(api, setup.workspace.id, setup.space.id, setup.mainLayerId, [
    { path: "README.md", ...TEXT_FIXTURES.baseReadme },
    { path: "shared.txt", ...TEXT_FIXTURES.baseShared }
  ]);
  const sourceLayer = await api.postJson<LayerResponse>(
    `/v1/workspaces/${setup.workspace.id}/spaces/${setup.space.id}/layers`,
    {
      name: "Source Layer",
      parentId: setup.mainLayerId
    }
  );
  await publishFiles(api, setup.workspace.id, setup.space.id, sourceLayer.id, [
    { path: "README.md", ...TEXT_FIXTURES.baseReadme },
    { path: "shared.txt", ...TEXT_FIXTURES.baseShared },
    { path: "feature-a.txt", ...TEXT_FIXTURES.featureA },
    { path: "feature-b.txt", ...TEXT_FIXTURES.featureB }
  ], { baseTreeId, changedPaths: ["feature-a.txt", "feature-b.txt"] });

  await openSpace(page, setup.email, setup.password, setup.space.name);
  await page.getByRole("button", { name: "Weaves" }).click();
  await byTestId(page, "weave-source-layer").selectOption({ label: "Source Layer" });
  await byTestId(page, "weave-target-layer").selectOption({ label: "Main" });
  await byTestId(page, "weave-create-request").click();

  const weaveRequest = byTestId(page, "weave-request");
  await expect(weaveRequest).toBeVisible({ timeout: 30_000 });
  await expect(byTestId(page, "weave-apply")).toBeEnabled({ timeout: 30_000 });
  await byTestId(page, "weave-apply").click();

  await expect(weaveRequest.getByText("applied")).toBeVisible({ timeout: 30_000 });
  await expect.poll(async () => {
    const weaveId = await latestWeaveRequestId(api, setup.workspace.id, setup.space.id);
    if (!weaveId) {
      return [];
    }
    const detail = await api.getJson<WeaveRequestResponse>(
      `/v1/workspaces/${setup.workspace.id}/spaces/${setup.space.id}/weave-requests/${weaveId}`
    );
    return detail.appliedSteps ?? [];
  }, { timeout: 30_000 }).toHaveLength(1);
  await expect.poll(async () => {
    const artifacts = await api.getJson<ArtifactCollectionResponse>(
      `/v1/workspaces/${setup.workspace.id}/spaces/${setup.space.id}/layers/${setup.mainLayerId}/artifacts`
    );
    return artifacts.items.map((item) => item.location ?? item.path ?? item.logicalPath).sort();
  }, { timeout: 30_000 }).toContain("feature-a.txt");
  await page.getByRole("tab", { name: /Files/ }).click();
  await expect(page.getByRole("row", { name: /feature-a\.txt/ })).toBeVisible({ timeout: 30_000 });
  await expect(page.getByRole("row", { name: /feature-b\.txt/ })).toBeVisible();

  await page.getByRole("tab", { name: /Steps/ }).click();
  await expect(page.getByRole("button", { name: /feature-a\.txt/ })).toBeVisible({ timeout: 30_000 });
  await expect(page.getByRole("button", { name: /feature-b\.txt/ })).toBeVisible();
});

test("real Studio Web resolves a text Weave conflict through the Lens UI and applies incoming content", async ({ page }) => {
  const suffix = uniqueSuffix();
  const api = new CookieApi(serverUrl);
  const setup = await createWorkspaceSpace(api, suffix, "Web Weave Conflict");

  const baseTreeId = await publishFiles(api, setup.workspace.id, setup.space.id, setup.mainLayerId, [
    { path: "note.txt", ...TEXT_FIXTURES.conflictBase }
  ]);
  const sourceLayer = await api.postJson<LayerResponse>(
    `/v1/workspaces/${setup.workspace.id}/spaces/${setup.space.id}/layers`,
    {
      name: "Incoming Layer",
      parentId: setup.mainLayerId
    }
  );
  await publishFiles(api, setup.workspace.id, setup.space.id, setup.mainLayerId, [
    { path: "note.txt", ...TEXT_FIXTURES.conflictExisting }
  ], { baseTreeId });
  await publishFiles(api, setup.workspace.id, setup.space.id, sourceLayer.id, [
    { path: "note.txt", ...TEXT_FIXTURES.conflictIncoming }
  ], { baseTreeId });

  await openSpace(page, setup.email, setup.password, setup.space.name);
  await page.getByRole("button", { name: "Weaves" }).click();
  await byTestId(page, "weave-source-layer").selectOption({ label: "Incoming Layer" });
  await byTestId(page, "weave-target-layer").selectOption({ label: "Main" });
  await byTestId(page, "weave-create-request").click();

  const weaveRequest = byTestId(page, "weave-request");
  await expect(byTestId(page, "lens-reconcile-surface")).toBeVisible({ timeout: 30_000 });
  await page.locator('[data-testid^="lens-resolve-block-"][data-testid$="-incoming"]').first().click();
  await expect(byTestId(page, "weave-apply")).toBeEnabled({ timeout: 30_000 });
  await expect.poll(async () => {
    const weaveId = await latestWeaveRequestId(api, setup.workspace.id, setup.space.id);
    if (!weaveId) {
      return "";
    }
    const detail = await api.getJson<WeaveRequestResponse>(
      `/v1/workspaces/${setup.workspace.id}/spaces/${setup.space.id}/weave-requests/${weaveId}`
    );
    const conflict = detail.conflicts?.[0];
    return `${detail.status}:${conflict?.status}:${conflict?.resolvedFileObjectId ?? ""}`;
  }, { timeout: 30_000 }).toContain("resolved:resolved:");
  await byTestId(page, "weave-apply").click();

  await expect(weaveRequest.getByText("applied")).toBeVisible({ timeout: 30_000 });
  await expect.poll(async () => {
    const weaveId = await latestWeaveRequestId(api, setup.workspace.id, setup.space.id);
    if (!weaveId) {
      return [];
    }
    const detail = await api.getJson<WeaveRequestResponse>(
      `/v1/workspaces/${setup.workspace.id}/spaces/${setup.space.id}/weave-requests/${weaveId}`
    );
    return detail.appliedSteps ?? [];
  }, { timeout: 30_000 }).toHaveLength(1);
  await expect.poll(async () => {
    const artifacts = await api.getJson<ArtifactCollectionResponse>(
      `/v1/workspaces/${setup.workspace.id}/spaces/${setup.space.id}/layers/${setup.mainLayerId}/artifacts`
    );
    const note = artifacts.items.find((item) => (item.location ?? item.path ?? item.logicalPath) === "note.txt");
    if (!note?.id) {
      return "";
    }
    const content = await api.getJson<Record<string, unknown>>(
      `/v1/workspaces/${setup.workspace.id}/spaces/${setup.space.id}/layers/${setup.mainLayerId}/artifacts/${encodeURIComponent(note.id)}/content`
    );
    return artifactContentText(content);
  }, { timeout: 30_000 }).toContain("conflict-incoming");
  await page.getByRole("tab", { name: /Files/ }).click();
  await page.getByRole("row", { name: /note\.txt/ }).click();
  await expect(page.getByText("conflict-incoming")).toBeVisible({ timeout: 30_000 });
  await expect(page.getByText("conflict-existing")).toHaveCount(0);
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
    return this.request<T>("POST", path, {
      "content-type": "application/json"
    }, JSON.stringify(body));
  }

  async getJson<T = unknown>(path: string): Promise<T> {
    return this.request<T>("GET", path);
  }

  async putRaw<T = unknown>(path: string, body: string): Promise<T> {
    return this.request<T>("PUT", path, {
      "content-type": "text/plain; charset=utf-8"
    }, body);
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
      throw new Error(`${method} ${path} failed with HTTP ${response.status}: ${await response.text()}`);
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

async function createWorkspaceSpace(api: CookieApi, suffix: string, label: string) {
  const email = `studio-web-${label.toLowerCase().replace(/\s+/g, "-")}-${suffix}@layrs.local`;
  const password = `Correct horse ${suffix}!`;
  const workspaceName = `${label} Workspace ${suffix}`;
  const spaceName = `${label} Space ${suffix}`;

  await api.healthz();
  await api.postJson("/v1/auth/signup", {
    name: "Studio Web Weave E2E",
    email,
    password
  });
  const workspace = await api.postJson<WorkspaceResponse>("/v1/workspaces", {
    name: workspaceName,
    slug: `${label.toLowerCase().replace(/\s+/g, "-")}-${suffix}`
  });
  const space = await api.postJson<SpaceResponse>(`/v1/workspaces/${workspace.id}/spaces`, {
    name: spaceName,
    description: "Created by the real Studio Web Weave E2E setup."
  });
  const snapshot = await api.getJson<StudioSnapshotResponse>(`/v1/studio/snapshot?workspace_id=${workspace.id}`);
  const mainLayerId =
    snapshot.layers.find((layer) => layer.spaceId === space.id && layer.name === "Main")?.id ?? space.currentLayerId;

  return { api, email, mainLayerId, password, space, workspace };
}

async function publishFiles(
  api: CookieApi,
  workspaceId: string,
  spaceId: string,
  layerId: string,
  files: TestFile[],
  options: { baseTreeId?: string; changedPaths?: string[] } = {}
): Promise<string> {
  const treeId = validBlake3Id();
  const changedPaths = options.changedPaths ?? files.map((file) => file.path);

  for (const file of files) {
    await api.putRaw(`/v1/workspaces/${workspaceId}/spaces/${spaceId}/chunks/${file.digest}`, file.body);
  }

  const body: Record<string, unknown> = {
    protocol: "layrs.sync.v2",
    layerId,
    policyEpoch: 1,
    idempotencyKey: `idem_${uniqueSuffix()}_${uniqueSuffix()}`,
    sourceClientId: "studio-web-weave-e2e",
    rootTreeId: treeId,
    changedPaths,
    step: {
      stepId: `step_${uniqueSuffix()}${uniqueSuffix()}`,
      layerId,
      rootTreeId: treeId,
      changedPaths,
      originLayerId: layerId,
      stepKind: "native"
    },
    storeObjects: {
      chunks: files.map((file) => ({
        chunkId: file.digest,
        digest: file.digest,
        size: Buffer.byteLength(file.body)
      })),
      fileObjects: files.map((file) => ({
        fileObjectId: file.digest,
        path: file.path,
        digest: file.digest,
        mediaType: "text/plain",
        size: Buffer.byteLength(file.body),
        chunks: [
          {
            chunkId: file.digest,
            size: Buffer.byteLength(file.body)
          }
        ]
      })),
      treeObjects: [
        {
          treeId,
          entries: files.map((file) => ({
            path: file.path,
            fileObjectId: file.digest
          }))
        }
      ],
      tombstones: [],
      deletedPaths: []
    }
  };

  if (options.baseTreeId) {
    body.baseTreeId = options.baseTreeId;
    (body.step as Record<string, unknown>).baseTreeId = options.baseTreeId;
  }

  await api.postJson(`/v1/workspaces/${workspaceId}/spaces/${spaceId}/sync/publish`, body);
  return treeId;
}

async function openSpace(page: Page, email: string, password: string, spaceName: string) {
  await page.goto("/");
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password").fill(password);
  await page.locator("form").getByRole("button", { name: "Login" }).click();

  await expect(page.getByRole("button", { name: new RegExp(spaceName) })).toBeVisible({
    timeout: 30_000
  });
  await page.getByRole("button", { name: new RegExp(spaceName) }).click();
}

function byTestId(page: Page, testId: string) {
  return page.locator(`[data-testid="${testId}"]`);
}

async function latestWeaveRequestId(api: CookieApi, workspaceId: string, spaceId: string) {
  const requests = await api.getJson<WeaveRequestListResponse>(
    `/v1/workspaces/${workspaceId}/spaces/${spaceId}/weave-requests`
  );
  const items = Array.isArray(requests) ? requests : requests.items ?? [];
  return items[0]?.weaveId;
}

function artifactContentText(payload: Record<string, unknown>) {
  const content = payload.content;
  if (typeof content === "string") {
    return content;
  }
  if (!content || typeof content !== "object" || Array.isArray(content)) {
    return "";
  }
  const record = content as Record<string, unknown>;
  const value = typeof record.value === "string" ? record.value : "";
  if (!value) {
    return "";
  }
  if (record.encoding === "base64") {
    return Buffer.from(value, "base64").toString("utf8");
  }
  return value;
}

function uniqueSuffix() {
  return Math.random().toString(16).slice(2, 10);
}

function validBlake3Id() {
  const hex = `${uniqueSuffix()}${uniqueSuffix()}${uniqueSuffix()}${uniqueSuffix()}${uniqueSuffix()}${uniqueSuffix()}${uniqueSuffix()}${uniqueSuffix()}`
    .padEnd(64, "0")
    .slice(0, 64);
  return `blake3:${hex}`;
}
