import type { DiffModel } from "@layrs/client-sdk";

export interface SecretStoreStatus {
  available: boolean;
  provider: string;
  message: string;
}

export interface Account {
  id: string;
  email: string;
  displayName: string;
}

export interface WorkspaceSummary {
  id: string;
  name: string;
  slug?: string;
}

export interface SpaceSummary {
  id: string;
  workspaceId: string;
  name: string;
  currentLayerId?: string;
}

export interface LayerSummary {
  id: string;
  workspaceId: string;
  spaceId: string;
  name: string;
  kind?: string;
  parentLayerId?: string;
  access?: string;
}

export interface BootstrapData {
  account?: Account;
  workspaces: WorkspaceSummary[];
  spaces: SpaceSummary[];
  layers: LayerSummary[];
}

export type LayerAccessKind = "open" | "redacted" | "blocked";

export interface LayerAccessView {
  layerId: string;
  workspaceId: string;
  spaceId: string;
  displayName: string;
  access: LayerAccessKind;
  canOpen: boolean;
  localPath?: string;
  reason?: string;
}

export interface AccessRegistryResult {
  root: string;
  pointerPath: string;
  layers: LayerAccessView[];
}

export interface DesktopStatus {
  serverEndpoint: string;
  deviceId: string;
  secretStore: SecretStoreStatus;
  connected: boolean;
  cachedBootstrap?: BootstrapData;
}

export interface DesktopSettings {
  serverEndpoint: string;
  autoReceive: boolean;
  autoPublish: boolean;
  autoLocalSteps: boolean;
  syncIntervalSeconds: number;
  defaultLocalSpacesFolder: string;
  shortcuts: DesktopShortcutSettings;
}

export interface DesktopShortcutSettings {
  enabled: boolean;
  saveStep: string;
  publish: string;
  smartSavePublishesPendingStep: boolean;
}

export interface DeviceLoginStartResponse {
  deviceCode: string;
  userCode: string;
  verificationUri: string;
  verificationUriComplete?: string;
  expiresIn: number;
  interval: number;
  message?: string;
}

export interface DeviceLoginPollResponse {
  status: string;
  message?: string;
  account?: Account;
  bootstrap?: BootstrapData;
  accessRegistry?: AccessRegistryResult;
}

export interface LocalLayerSummary {
  layerId: string;
  displayName: string;
  parentLayerId?: string;
  access: LayerAccessKind;
  canOpen: boolean;
  path: string;
  syncStatus: "linked" | "local" | "local-only" | string;
}

export interface LocalSpaceSummary {
  localSpaceId: string;
  spaceId: string;
  workspaceId: string;
  serverSpaceId?: string;
  state: "linked" | "draft" | string;
  name: string;
  rootPath: string;
  activeLayerId?: string;
  layers: LocalLayerSummary[];
}

export interface AvailableSpaceView {
  spaceId: string;
  workspaceId: string;
  name: string;
  currentLayerId?: string;
  layers: LayerAccessView[];
  localSpaces: LocalSpaceSummary[];
  freshness?: "fresh" | "stale" | "offline" | string;
  message?: string;
}

export interface CreateLocalSpaceResult {
  localSpace: LocalSpaceSummary;
  created: boolean;
}

export interface LayerIdMapping {
  localLayerId: string;
  serverLayerId: string;
  name: string;
}

export interface SendDraftLocalSpaceResult {
  localSpace: LocalSpaceSummary;
  workspaceId: string;
  serverSpaceId: string;
  layerMappings: LayerIdMapping[];
  publishedLayers: number;
}

export interface ForgetLocalSpaceResult {
  localSpaceId: string;
  rootPath: string;
  archivedLayrsPath?: string | null;
  message: string;
}

export interface LayerSwitchResult {
  localSpace: LocalSpaceSummary;
  previousLayerId: string;
  activeLayerId: string;
  savedStepId?: string;
  changedFiles: number;
}

export interface DeleteLayerResult {
  localSpace: LocalSpaceSummary;
  deletedLayerId: string;
  message: string;
}

export interface FileSnapshotEntry {
  path: string;
  object: string;
  hash: string;
  size: number;
}

export interface LensDiffEntry {
  path: string;
  state: "added" | "modified" | "deleted" | string;
  lensId: string;
  title: string;
  diff: DiffModel;
  message?: string;
}

export interface LocalDiffStats {
  files: number;
  additions: number;
  deletions: number;
}

export interface LocalStepSummary {
  stepId: string;
  layerId: string;
  capturedAt: number;
  changedFiles: number;
  diffStats: LocalDiffStats;
  diffs: LensDiffEntry[];
}

export interface LayerStepActivity {
  layerId: string;
  latestStepAt: number;
  stepCount: number;
}

export interface WorkingTreeScan {
  rootPath: string;
  activeLayerId: string;
  changed: boolean;
  added: string[];
  modified: string[];
  deleted: string[];
  diffs: LensDiffEntry[];
  steps: LocalStepSummary[];
  layerActivities: LayerStepActivity[];
  pendingPublishCount: number;
  files: FileSnapshotEntry[];
}

export interface SyncOperationResult {
  localSpace: LocalSpaceSummary;
  status: string;
  message: string;
  syncStatePath: string;
}

export interface SaveLocalStepResult {
  localSpace: LocalSpaceSummary;
  status: "saved" | "clean" | string;
  message: string;
  stepId?: string;
  changedFiles: number;
  diffStats: LocalDiffStats;
  pendingPublishCount: number;
}

type TauriCore = {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
};

declare global {
  interface Window {
    __TAURI__?: {
      core?: TauriCore;
    };
    __TAURI_INTERNALS__?: {
      invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
    };
  }
}

function tauriInvoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke;

  if (!invoke) {
    return Promise.reject(
      new Error("Layrs Desktop commands are available only inside the Tauri desktop runtime.")
    );
  }

  return invoke<T>(command, args);
}

export function getDesktopStatus() {
  return tauriInvoke<DesktopStatus>("desktop_status");
}

export function configureServerEndpoint(serverEndpoint: string) {
  return tauriInvoke<DesktopStatus>("configure_server_endpoint", { serverEndpoint });
}

export function startDeviceLogin() {
  return tauriInvoke<DeviceLoginStartResponse>("start_device_login");
}

export function pollDeviceLogin(deviceCode: string, workspaceRoot?: string) {
  return tauriInvoke<DeviceLoginPollResponse>("poll_device_login", {
    deviceCode,
    workspaceRoot: workspaceRoot || undefined
  });
}

export function refreshBootstrap(workspaceRoot?: string) {
  return tauriInvoke<DeviceLoginPollResponse>("refresh_bootstrap", {
    workspaceRoot: workspaceRoot || undefined
  });
}

export function listAvailableSpaces() {
  return tauriInvoke<AvailableSpaceView[]>("list_available_spaces");
}

export function listLocalSpaces() {
  return tauriInvoke<LocalSpaceSummary[]>("list_local_spaces");
}

export function createLocalSpace(
  spaceId: string,
  targetFolder: string,
  initialLayerId?: string
) {
  return tauriInvoke<CreateLocalSpaceResult>("create_local_space", {
    spaceId,
    targetFolder,
    initialLayerId: initialLayerId || undefined
  });
}

export function createDraftLocalSpace(name: string, targetFolder: string) {
  return tauriInvoke<CreateLocalSpaceResult>("create_draft_local_space", {
    name,
    targetFolder
  });
}

export function initLocalSpace(name: string, targetFolder: string) {
  return tauriInvoke<CreateLocalSpaceResult>("init_local_space", {
    name,
    targetFolder
  });
}

export function sendDraftLocalSpace(localSpace: string, workspaceId: string) {
  return tauriInvoke<SendDraftLocalSpaceResult>("send_draft_local_space", {
    localSpace,
    workspaceId
  });
}

export function openLocalSpace(localSpaceIdOrPath: string) {
  return tauriInvoke<LocalSpaceSummary>("open_local_space", { localSpaceIdOrPath });
}

export function forgetLocalSpace(localSpace: string) {
  return tauriInvoke<ForgetLocalSpaceResult>("forget_local_space", { localSpace });
}

export function switchLayer(localSpace: string, targetLayerId: string) {
  return tauriInvoke<LayerSwitchResult>("switch_layer", { localSpace, targetLayerId });
}

export function createLayerFromCurrent(localSpace: string, name: string) {
  return tauriInvoke<LayerSwitchResult>("create_layer_from_current", { localSpace, name });
}

export function deleteLayer(localSpace: string, layerId: string) {
  return tauriInvoke<DeleteLayerResult>("delete_layer", { localSpace, layerId });
}

export function scanWorkingTree(localSpace: string) {
  return tauriInvoke<WorkingTreeScan>("scan_working_tree", { localSpace });
}

export function loadDiffWindow(
  localSpace: string,
  path: string,
  source: string | undefined,
  start: number,
  limit: number
) {
  return tauriInvoke<LensDiffEntry>("load_diff_window", {
    localSpace,
    path,
    source: source || undefined,
    start,
    limit
  });
}

export function receiveLocalSpace(localSpace: string) {
  return tauriInvoke<SyncOperationResult>("receive_local_space", { localSpace });
}

export function publishLocalSpace(localSpace: string) {
  return tauriInvoke<SyncOperationResult>("publish_local_space", { localSpace });
}

export function saveLocalStep(localSpace: string) {
  return tauriInvoke<SaveLocalStepResult>("save_local_step", { localSpace });
}

export function loadDesktopSettings() {
  return tauriInvoke<DesktopSettings>("load_desktop_settings");
}

export function saveDesktopSettings(settings: DesktopSettings) {
  return tauriInvoke<DesktopSettings>("save_desktop_settings", { settings });
}

export function selectFolder(initialDirectory?: string) {
  return tauriInvoke<string | null>("select_folder", {
    initialDirectory: initialDirectory || undefined
  });
}
