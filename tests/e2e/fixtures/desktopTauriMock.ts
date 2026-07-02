import { test as base, expect, type Page } from "@playwright/test";
import { makeTestSpace, type TestSpace, type TestSpaceFile } from "./testSpace";

type JsonRecord = Record<string, unknown>;

interface DesktopMockState {
  bootstrap: JsonRecord;
  commandLog: Array<{ command: string; args?: JsonRecord }>;
  folderSeeds: Record<string, TestSpace>;
  localSpaces: JsonRecord[];
  scans: Record<string, JsonRecord>;
  selectedFolders: string[];
  settings: JsonRecord;
}

export interface DesktopTauriMock {
  commandLog(): Array<{ command: string; args?: JsonRecord }>;
  queueSelectedFolder(folder: string): void;
  seedFolder(space: TestSpace): void;
}

export const test = base.extend<{
  desktopTauriMock: DesktopTauriMock;
  testSpace: TestSpace;
}>({
  testSpace: async ({}, use) => {
    await use(makeTestSpace());
  },
  desktopTauriMock: async ({ page }, use) => {
    const state = createInitialState();
    await installDesktopInvoke(page, state);

    await use({
      commandLog: () => [...state.commandLog],
      queueSelectedFolder: (folder) => {
        state.selectedFolders.push(folder);
      },
      seedFolder: (space) => {
        state.folderSeeds[space.rootPath] = space;
      }
    });
  }
});

export { expect };

async function installDesktopInvoke(page: Page, state: DesktopMockState) {
  await page.exposeBinding("__layrsDesktopInvoke", async (_source, command: string, args?: JsonRecord) => {
    state.commandLog.push({ command, args });
    return clone(handleCommand(state, command, args ?? {}));
  });

  await page.addInitScript(() => {
    const desktopWindow = window as unknown as {
      __TAURI__?: {
        core?: {
          invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
        };
      };
      __TAURI_INTERNALS__?: {
        invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
      };
      __layrsDesktopInvoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
    };
    const invoke = (command: string, args?: Record<string, unknown>) =>
      desktopWindow.__layrsDesktopInvoke(command, args);

    desktopWindow.__TAURI__ = {
      ...(desktopWindow.__TAURI__ ?? {}),
      core: { invoke }
    };
    desktopWindow.__TAURI_INTERNALS__ = { invoke };
  });
}

function createInitialState(): DesktopMockState {
  const bootstrap = {
    account: {
      id: "account-playwright",
      email: "playwright@layrs.local",
      displayName: "Playwright"
    },
    workspaces: [
      {
        id: "workspace-playwright",
        name: "Playwright Workspace",
        slug: "playwright"
      }
    ],
    spaces: [],
    layers: []
  };

  return {
    bootstrap,
    commandLog: [],
    folderSeeds: {},
    localSpaces: [],
    scans: {},
    selectedFolders: [],
    settings: {
      serverEndpoint: "http://127.0.0.1:3000",
      autoReceive: false,
      autoPublish: false,
      autoLocalSteps: true,
      syncIntervalSeconds: 900,
      defaultLocalSpacesFolder: "D:\\Layrs\\tmp",
      shortcuts: {
        enabled: true,
        saveStep: "Ctrl+S",
        publish: "Ctrl+P",
        smartSavePublishesPendingStep: true
      }
    }
  };
}

function handleCommand(state: DesktopMockState, command: string, args: JsonRecord) {
  switch (command) {
    case "desktop_status":
      return {
        serverEndpoint: state.settings.serverEndpoint,
        deviceId: "desktop-playwright",
        secretStore: {
          available: true,
          provider: "playwright-memory",
          message: "Mock secret store available."
        },
        connected: true,
        cachedBootstrap: state.bootstrap
      };
    case "load_desktop_settings":
      return state.settings;
    case "save_desktop_settings":
      state.settings = args.settings as JsonRecord;
      return state.settings;
    case "refresh_bootstrap":
      return {
        status: "connected",
        message: "Mock bootstrap refreshed.",
        bootstrap: state.bootstrap
      };
    case "list_available_spaces":
      return [];
    case "list_local_spaces":
      return state.localSpaces;
    case "select_folder":
      return state.selectedFolders.shift() ?? args.initialDirectory ?? "D:\\Layrs\\tmp\\selected-space";
    case "init_local_space":
      return initLocalSpace(state, String(args.name ?? ""), String(args.targetFolder ?? ""));
    case "open_local_space":
      return openLocalSpace(state, String(args.localSpaceIdOrPath ?? ""));
    case "scan_working_tree":
      return scanWorkingTree(state, String(args.localSpace ?? ""));
    case "save_local_step":
      return saveLocalStep(state, String(args.localSpace ?? ""));
    case "create_layer_from_current":
      return createLayerFromCurrent(state, String(args.localSpace ?? ""), String(args.name ?? ""));
    case "switch_layer":
      return switchLayer(state, String(args.localSpace ?? ""), String(args.targetLayerId ?? ""));
    case "load_diff_window":
      return loadDiffWindow(state, String(args.localSpace ?? ""), String(args.path ?? ""), String(args.source ?? "workingTree"));
    default:
      throw new Error(`Unhandled desktop mock command: ${command}`);
  }
}

function initLocalSpace(state: DesktopMockState, name: string, rootPath: string) {
  const existing = state.localSpaces.find((space) => space.rootPath === rootPath);
  if (existing) {
    return { localSpace: existing, created: false };
  }

  const localSpace = makeLocalSpace(name, rootPath);
  state.localSpaces.push(localSpace);
  state.scans[String(localSpace.localSpaceId)] = makeScan(localSpace, state.folderSeeds[rootPath]?.files ?? []);
  return { localSpace, created: true };
}

function openLocalSpace(state: DesktopMockState, localSpaceIdOrPath: string) {
  const existing = state.localSpaces.find(
    (space) => space.localSpaceId === localSpaceIdOrPath || space.rootPath === localSpaceIdOrPath
  );
  if (existing) {
    return existing;
  }

  const seed = state.folderSeeds[localSpaceIdOrPath] ?? makeTestSpace({ rootPath: localSpaceIdOrPath });
  return initLocalSpace(state, seed.name, seed.rootPath).localSpace;
}

function scanWorkingTree(state: DesktopMockState, localSpaceId: string) {
  const scan = state.scans[localSpaceId];
  if (!scan) {
    throw new Error(`Unknown Local Space: ${localSpaceId}`);
  }
  return scan;
}

function saveLocalStep(state: DesktopMockState, localSpaceId: string) {
  const localSpace = findLocalSpace(state, localSpaceId);
  const scan = scanWorkingTree(state, localSpaceId);
  const diffs = (scan.diffs as JsonRecord[]) ?? [];

  if (diffs.length === 0) {
    return {
      localSpace,
      status: "clean",
      message: "No local changes to capture.",
      changedFiles: 0,
      diffStats: { files: 0, additions: 0, deletions: 0 },
      pendingPublishCount: scan.pendingPublishCount ?? 0
    };
  }

  const stepId = `step-${Date.now()}`;
  const diffStats = diffStatsFromDiffs(diffs);
  const step = {
    stepId,
    layerId: localSpace.activeLayerId,
    capturedAt: Math.floor(Date.now() / 1000),
    changedFiles: diffs.length,
    diffStats,
    diffs
  };

  scan.steps = [...((scan.steps as JsonRecord[]) ?? []), step];
  scan.layerActivities = [
    {
      layerId: localSpace.activeLayerId,
      latestStepAt: step.capturedAt,
      stepCount: ((scan.steps as JsonRecord[]) ?? []).filter((item) => item.layerId === localSpace.activeLayerId).length
    }
  ];
  scan.changed = false;
  scan.added = [];
  scan.modified = [];
  scan.deleted = [];
  scan.diffs = [];
  scan.pendingPublishCount = Number(scan.pendingPublishCount ?? 0) + 1;

  return {
    localSpace,
    status: "saved",
    message: "Mock Step captured from working tree.",
    stepId,
    changedFiles: diffs.length,
    diffStats,
    pendingPublishCount: scan.pendingPublishCount
  };
}

function createLayerFromCurrent(state: DesktopMockState, localSpaceId: string, name: string) {
  const localSpace = findLocalSpace(state, localSpaceId);
  const layerId = `layer-${slug(name)}`;
  const layer = {
    layerId,
    displayName: name,
    parentLayerId: localSpace.activeLayerId,
    access: "open",
    canOpen: true,
    path: `${localSpace.rootPath}\\.layrs\\layers\\${layerId}`,
    syncStatus: "local-only"
  };

  localSpace.layers = [...(localSpace.layers as JsonRecord[]), layer];
  const previousLayerId = localSpace.activeLayerId;
  localSpace.activeLayerId = layerId;
  const scan = scanWorkingTree(state, localSpaceId);
  scan.activeLayerId = layerId;

  return {
    localSpace,
    previousLayerId,
    activeLayerId: layerId,
    changedFiles: 0
  };
}

function switchLayer(state: DesktopMockState, localSpaceId: string, targetLayerId: string) {
  const localSpace = findLocalSpace(state, localSpaceId);
  const layers = localSpace.layers as JsonRecord[];
  if (!layers.some((layer) => layer.layerId === targetLayerId)) {
    throw new Error(`Unknown Layer: ${targetLayerId}`);
  }

  const previousLayerId = String(localSpace.activeLayerId);
  localSpace.activeLayerId = targetLayerId;
  const scan = scanWorkingTree(state, localSpaceId);
  scan.activeLayerId = targetLayerId;

  return {
    localSpace,
    previousLayerId,
    activeLayerId: targetLayerId,
    changedFiles: 0
  };
}

function loadDiffWindow(state: DesktopMockState, localSpaceId: string, path: string, source: string) {
  const scan = scanWorkingTree(state, localSpaceId);
  const stepId = source.startsWith("localStep:") ? source.replace("localStep:", "") : undefined;
  const step = stepId ? ((scan.steps as JsonRecord[]) ?? []).find((item) => item.stepId === stepId) : undefined;
  const diffs = step ? (step.diffs as JsonRecord[]) : (scan.diffs as JsonRecord[]);
  const diff = diffs.find((item) => item.path === path);
  if (!diff) {
    throw new Error(`No diff for ${path}`);
  }
  return diff;
}

function makeLocalSpace(name: string, rootPath: string): JsonRecord {
  const spaceSlug = slug(name || "local-space");
  const mainLayerId = `layer-${spaceSlug}-main`;

  return {
    localSpaceId: `local-${spaceSlug}`,
    spaceId: `space-${spaceSlug}`,
    workspaceId: "workspace-playwright",
    state: "draft",
    name,
    rootPath,
    activeLayerId: mainLayerId,
    layers: [
      {
        layerId: mainLayerId,
        displayName: "Main",
        access: "open",
        canOpen: true,
        path: rootPath,
        syncStatus: "local-only"
      }
    ]
  };
}

function makeScan(localSpace: JsonRecord, files: TestSpaceFile[]): JsonRecord {
  const diffs = files
    .filter((file) => file.state)
    .map((file) => makeDiffEntry(file, String(localSpace.activeLayerId)));

  return {
    rootPath: localSpace.rootPath,
    activeLayerId: localSpace.activeLayerId,
    changed: diffs.length > 0,
    added: files.filter((file) => file.state === "added").map((file) => file.path),
    modified: files.filter((file) => file.state === "modified").map((file) => file.path),
    deleted: files.filter((file) => file.state === "deleted").map((file) => file.path),
    diffs,
    steps: [],
    layerActivities: [],
    pendingPublishCount: 0,
    files: files
      .filter((file) => file.state !== "deleted")
      .map((file) => ({
        path: file.path,
        object: `object-${slug(file.path)}`,
        hash: `hash-${slug(file.path)}`,
        size: file.size ?? 256
      }))
  };
}

function makeDiffEntry(file: TestSpaceFile, layerId: string): JsonRecord {
  const state = file.state ?? "modified";
  const lensId = file.path.endsWith(".ts") || file.path.endsWith(".tsx") ? "layrs.code" : "layrs.text";

  return {
    path: file.path,
    state,
    lensId,
    title: file.summary ?? `${file.path} changed`,
    diff: {
      kind: "textLines",
      summary: file.summary ?? `${file.path} changed`,
      hunks: [
        {
          oldStart: 1,
          oldLines: state === "added" ? 0 : 1,
          newStart: 1,
          newLines: state === "deleted" ? 0 : 1,
          lines: diffLinesForFile(file)
        }
      ],
      fields: {
        path: file.path,
        state,
        lensId,
        layerId
      },
      metadata: {
        path: file.path,
        state,
        lensId,
        layerId,
        renderedLineCount: 2,
        totalLineCount: 2
      }
    }
  };
}

function diffLinesForFile(file: TestSpaceFile) {
  if (file.state === "added") {
    return [{ op: "insert", newLine: 1, text: file.body ?? `Added ${file.path}` }];
  }
  if (file.state === "deleted") {
    return [{ op: "delete", oldLine: 1, text: `Deleted ${file.path}` }];
  }
  return [
    { op: "delete", oldLine: 1, text: `Previous ${file.path}` },
    { op: "insert", newLine: 1, text: file.body ?? `Updated ${file.path}` }
  ];
}

function diffStatsFromDiffs(diffs: JsonRecord[]) {
  return diffs.reduce(
    (stats, entry) => {
      const diff = entry.diff as JsonRecord;
      const hunks = (diff.hunks as JsonRecord[]) ?? [];
      stats.files += 1;
      for (const hunk of hunks) {
        for (const line of ((hunk.lines as JsonRecord[]) ?? [])) {
          if (line.op === "insert") {
            stats.additions += 1;
          } else if (line.op === "delete") {
            stats.deletions += 1;
          }
        }
      }
      return stats;
    },
    { files: 0, additions: 0, deletions: 0 }
  );
}

function findLocalSpace(state: DesktopMockState, localSpaceId: string): JsonRecord {
  const localSpace = state.localSpaces.find((space) => space.localSpaceId === localSpaceId);
  if (!localSpace) {
    throw new Error(`Unknown Local Space: ${localSpaceId}`);
  }
  return localSpace;
}

function slug(value: string) {
  return value
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/(^-|-$)/g, "");
}

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}
