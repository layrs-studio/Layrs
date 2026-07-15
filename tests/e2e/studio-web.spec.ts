import { expect, test } from "@playwright/test";
import { sanitizeArtifact } from "../../apps/studio-web/src/privateData";
import {
  PrivateRequestCancelledError,
  StudioPrivateApiController
} from "../../apps/studio-web/src/studioPrivateApi";

test("ignores a late private response after selection A changes to B", async () => {
  const api = new StudioPrivateApiController();
  const lateA = deferred<string>();
  let signalA: AbortSignal | undefined;
  const requestA = api.request("artifact-preview", "workspace::space::layer::artifact-a", ({ signal }) => {
    signalA = signal;
    return lateA.promise;
  });

  await Promise.resolve();
  const resultB = await api.request(
    "artifact-preview",
    "workspace::space::layer::artifact-b",
    async () => "preview-b"
  );

  expect(resultB).toBe("preview-b");
  expect(signalA?.aborted).toBe(true);
  lateA.resolve("preview-a");
  await expect(requestA).rejects.toBeInstanceOf(PrivateRequestCancelledError);
});

test("purges private state and aborts other requests when a request returns 401", async () => {
  let privateState = {
    account: "account-private",
    workspaces: ["workspace-private"],
    spaces: ["space-private"],
    artifacts: ["artifact-private"],
    steps: ["step-private"],
    diffs: ["diff-private"],
    access: ["policy-private"]
  };
  let expiredMessage = "";
  const api = new StudioPrivateApiController((message) => {
    expiredMessage = message;
    privateState = { account: "", workspaces: [], spaces: [], artifacts: [], steps: [], diffs: [], access: [] };
  });
  const pending = deferred<string>();
  let pendingSignal: AbortSignal | undefined;
  const privateRequest = api.request("steps", "workspace::space::layer::step", ({ signal }) => {
    pendingSignal = signal;
    return pending.promise;
  });

  await expect(
    api.request("access", "workspace::space::layer::policy", async () => {
      throw Object.assign(new Error("Session expired"), { status: 401 });
    })
  ).rejects.toBeInstanceOf(PrivateRequestCancelledError);

  expect(expiredMessage).toContain("session expired");
  expect(privateState).toEqual({ account: "", workspaces: [], spaces: [], artifacts: [], steps: [], diffs: [], access: [] });
  expect(pendingSignal?.aborted).toBe(true);
  pending.resolve("stale private steps");
  await expect(privateRequest).rejects.toBeInstanceOf(PrivateRequestCancelledError);
});

test("redaction strips cached preview payloads while preserving only the file stub", () => {
  const artifact = {
    id: "artifact-redacted",
    spaceId: "space-private",
    layerId: "layer-private",
    name: "secrets.env",
    type: "file",
    summary: "DATABASE_PASSWORD=private",
    location: "config/secrets.env",
    updatedAt: "2026-07-15T10:00:00.000Z",
    sizeLabel: "1 KB",
    proofIds: ["proof-private"],
    contentHash: "private-hash",
    lensId: "layrs.text",
    preview: { kind: "code", body: "DATABASE_PASSWORD=private" },
    access: { mode: "redacted", canOpen: false, isRedacted: true, reason: "Policy redaction" }
  } as Parameters<typeof sanitizeArtifact>[0];

  const sanitized = sanitizeArtifact(artifact);

  expect(sanitized).toMatchObject({
    id: artifact.id,
    location: artifact.location,
    summary: "Restricted by Layer access policy",
    sizeLabel: "restricted",
    proofIds: []
  });
  expect(sanitized).not.toHaveProperty("preview");
  expect(sanitized).not.toHaveProperty("contentHash");
  expect(sanitized).not.toHaveProperty("lensId");
});

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

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
}
