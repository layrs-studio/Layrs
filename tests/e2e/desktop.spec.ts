import { expect, test } from "./fixtures/desktopTauriMock";
import { DesktopOperationController } from "../../apps/studio-desktop/src/desktopOperationQueue";
import {
  purgeRestrictedDiffWindows,
  purgeRestrictedWeaves,
  purgeRestrictedWorkingTrees
} from "../../apps/studio-desktop/src/desktopPrivateState";
import { LatestDesktopRequest } from "../../apps/studio-desktop/src/useDesktopDiffWindow";

test("serializes sync and coalesces focus scans without releasing sync busy state", async () => {
  const busyChanges: Array<string | null> = [];
  const operations = new DesktopOperationController((busy) => busyChanges.push(busy));
  const syncGate = deferred<void>();
  const trace: string[] = [];
  const sync = operations.run({
    busyAction: "sync",
    key: "sync:local-a",
    task: async () => {
      trace.push("sync:start");
      await syncGate.promise;
      trace.push("sync:end");
    }
  });

  await Promise.resolve();
  const focusScanA = operations.run({
    coalesce: true,
    key: "scan:focus:local-a",
    task: async () => trace.push("scan")
  });
  const focusScanB = operations.run({
    coalesce: true,
    key: "scan:focus:local-a",
    task: async () => trace.push("scan")
  });

  expect(focusScanA).toBe(focusScanB);
  expect(trace).toEqual(["sync:start"]);
  expect(busyChanges).toEqual(["sync"]);
  syncGate.resolve(undefined);
  await Promise.all([sync, focusScanA, focusScanB]);
  expect(trace).toEqual(["sync:start", "sync:end", "scan"]);
  expect(busyChanges).toEqual(["sync", null]);
});

test("ignores a late Desktop diff response after its keyed selection changes", async () => {
  const requests = new LatestDesktopRequest();
  const lateA = deferred<string>();
  let signalA: AbortSignal | undefined;
  const requestA = requests.run("workspace::space::layer::artifact-a", (signal) => {
    signalA = signal;
    return lateA.promise;
  });

  const resultB = await requests.run("workspace::space::layer::artifact-b", async () => "diff-b");
  expect(resultB).toBe("diff-b");
  expect(signalA?.aborted).toBe(true);
  lateA.resolve("diff-a");
  await expect(requestA).resolves.toBeUndefined();
});

test("purges redacted Desktop steps, diffs and weave caches", () => {
  const spaces = [
    {
      localSpaceId: "local-private",
      workspaceId: "workspace-private",
      spaceId: "space-private",
      activeLayerId: "layer-open",
      layers: [
        { layerId: "layer-open", access: "open" },
        { layerId: "layer-redacted", access: "redacted" }
      ]
    }
  ] as Parameters<typeof purgeRestrictedWorkingTrees>[0];
  const trees = {
    "local-private": {
      activeLayerId: "layer-open",
      steps: [
        { stepId: "step-open", layerId: "layer-open" },
        { stepId: "step-private", layerId: "layer-redacted", diffs: [{ body: "private" }] }
      ],
      layerActivities: [
        { layerId: "layer-open" },
        { layerId: "layer-redacted", title: "private activity" }
      ]
    }
  } as Parameters<typeof purgeRestrictedWorkingTrees>[1];
  const openDiffKey = "workspace-private::space-private::layer-open::local-private::workingTree::open.txt";
  const redactedDiffKey = "workspace-private::space-private::layer-redacted::local-private::step-private::secret.txt";
  const diffs = {
    [openDiffKey]: { path: "open.txt" },
    [redactedDiffKey]: { path: "secret.txt", diff: { body: "private" } }
  } as Parameters<typeof purgeRestrictedDiffWindows>[1];
  const weaves = {
    "local-private": { sourceLayerId: "layer-redacted", targetLayerId: "layer-open" },
    "orphan-local": { sourceLayerId: "layer-open", targetLayerId: "layer-open" }
  } as Parameters<typeof purgeRestrictedWeaves>[1];

  expect(purgeRestrictedWorkingTrees(spaces, trees)["local-private"].steps).toEqual([
    expect.objectContaining({ stepId: "step-open" })
  ]);
  expect(purgeRestrictedDiffWindows(spaces, diffs)).toEqual({ [openDiffKey]: diffs[openDiffKey] });
  expect(purgeRestrictedWeaves(spaces, weaves)).toEqual({});

  const activeLayerRedacted = [
    { ...spaces[0], layers: spaces[0].layers.map((layer) => ({ ...layer, access: "redacted" })) }
  ] as Parameters<typeof purgeRestrictedWorkingTrees>[0];
  expect(purgeRestrictedWorkingTrees(activeLayerRedacted, trees)).toEqual({});
});

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

test("keeps Clear Steps confirmation open with a persistent command error", async ({
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
  await page.getByRole("tab", { name: /Layer settings/ }).click();

  desktopTauriMock.failNext("clear_layer_steps", "Clear Steps rejected by access policy.");
  await page.getByRole("button", { name: "Clear steps" }).click();
  const dialog = page.getByRole("dialog", { name: /Clear Steps from Main/ });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Clear Steps" }).click();

  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("alert")).toContainText("Clear Steps rejected by access policy.");
  expect(desktopTauriMock.commandLog().map((entry) => entry.command)).toContain("clear_layer_steps");
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
