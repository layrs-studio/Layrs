import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { LensDiffHost } from "@layrs/lenses";
import { ConfirmModal, DangerZone, AppShell, Sidebar, StatusPill, Tabs, useNotifications } from "@layrs/ui";
import {
  AvailableSpaceView,
  BootstrapData,
  DesktopSettings,
  DesktopStatus,
  DeviceLoginPollResponse,
  DeviceLoginStartResponse,
  DesktopShortcutSettings,
  LayerAccessKind,
  LensDiffEntry,
  LocalDiffStats,
  LocalLayerSummary,
  LocalSpaceSummary,
  LocalStepSummary,
  WorkingTreeScan,
  createLayerFromCurrent,
  createDraftLocalSpace,
  createLocalSpace,
  deleteLayer,
  forgetLocalSpace,
  getDesktopStatus,
  initLocalSpace,
  listAvailableSpaces,
  listLocalSpaces,
  loadDiffWindow,
  loadDesktopSettings,
  openLocalSpace,
  pollDeviceLogin,
  publishLocalSpace,
  receiveLocalSpace,
  refreshBootstrap,
  saveDesktopSettings,
  saveLocalStep,
  scanWorkingTree,
  sendDraftLocalSpace,
  selectFolder,
  startDeviceLogin,
  switchLayer
} from "./tauri";

type LoadState = "loading" | "ready" | "error";
type DesktopPage = "available" | "local" | "draft" | "settings";
type LocalSpaceTab = "changes" | "files" | "steps" | "layers" | "sync" | "settings";
type FileState = "clean" | "modified" | "added" | "deleted" | "redacted";
type ChangeState = "modified" | "added" | "deleted";
const FOCUS_SCAN_THROTTLE_MS = 1500;
type CommandKey =
  | "available"
  | "local"
  | "create"
  | "create-draft"
  | "init-local"
  | "forget-local"
  | "open"
  | "switch"
  | "create-layer"
  | "delete-layer"
  | "scan"
  | "diff-window"
  | "receive"
  | "save-step"
  | "publish"
  | "send-draft"
  | "settings";

interface LayerFile {
  path: string;
  kind: "Code" | "Document" | "Image" | "Data";
  state: FileState;
  lensId?: string;
  sizeLabel: string;
  redacted?: boolean;
}

interface LocalChange {
  path: string;
  state: ChangeState;
  summary: string;
  lensId: string;
  diff: LensDiffEntry;
}

interface TimelineItem {
  id: string;
  kind: "active" | "scan" | "step";
  title: string;
  actor: string;
  at: string;
  summary: string;
  isActive: boolean;
  diffStats?: LocalDiffStats;
}

interface CreateDraft {
  targetFolder: string;
  layerId: string;
}

const statusLabels: Record<string, string> = {
  pending: "Waiting for approval",
  authorization_pending: "Waiting for approval",
  slow_down: "Server asked to slow down",
  connected: "Connected",
  denied: "Denied",
  expired: "Expired"
};

const defaultShortcuts: DesktopShortcutSettings = {
  enabled: true,
  saveStep: "Ctrl+S",
  publish: "Ctrl+P",
  smartSavePublishesPendingStep: true
};

type PulseTarget = "changes" | "layer" | "steps" | "sync";

export function DesktopApp() {
  const { notify } = useNotifications();
  const [loadState, setLoadState] = useState<LoadState>("loading");
  const [page, setPage] = useState<DesktopPage>(() => pageFromHash(window.location.hash));
  const [status, setStatus] = useState<DesktopStatus | null>(null);
  const [bootstrap, setBootstrap] = useState<BootstrapData | null>(null);
  const [availableSpaces, setAvailableSpaces] = useState<AvailableSpaceView[]>([]);
  const [localSpaces, setLocalSpaces] = useState<LocalSpaceSummary[]>([]);
  const [workingTrees, setWorkingTrees] = useState<Record<string, WorkingTreeScan>>({});
  const [scanRevisions, setScanRevisions] = useState<Record<string, number>>({});
  const [endpointDraft, setEndpointDraft] = useState("");
  const [defaultLocalRoot, setDefaultLocalRoot] = useState("");
  const [login, setLogin] = useState<DeviceLoginStartResponse | null>(null);
  const [pollStatus, setPollStatus] = useState<string | null>(null);
  const [pollInFlight, setPollInFlight] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selectedLocalSpaceId, setSelectedLocalSpaceId] = useState<string | null>(null);
  const [localSpaceTab, setLocalSpaceTab] = useState<LocalSpaceTab>("changes");
  const [selectedDiffPath, setSelectedDiffPath] = useState<string | null>(null);
  const [selectedStepId, setSelectedStepId] = useState<string | null>(null);
  const [forgetTargetId, setForgetTargetId] = useState<string | null>(null);
  const [deleteLayerTargetId, setDeleteLayerTargetId] = useState<string | null>(null);
  const [diffWindowOverrides, setDiffWindowOverrides] = useState<Record<string, LensDiffEntry>>({});
  const [createDrafts, setCreateDrafts] = useState<Record<string, CreateDraft>>({});
  const [draftSpaceName, setDraftSpaceName] = useState("");
  const [draftSpaceFolder, setDraftSpaceFolder] = useState("");
  const [initSpaceName, setInitSpaceName] = useState("");
  const [initSpaceFolder, setInitSpaceFolder] = useState("");
  const [sendWorkspaceId, setSendWorkspaceId] = useState("");
  const [newLayerName, setNewLayerName] = useState("");
  const [layerSearchQuery, setLayerSearchQuery] = useState("");
  const [autoReceive, setAutoReceive] = useState(false);
  const [autoPublish, setAutoPublish] = useState(false);
  const [autoLocalSteps, setAutoLocalSteps] = useState(true);
  const [syncIntervalMinutes, setSyncIntervalMinutes] = useState(15);
  const [shortcuts, setShortcuts] = useState<DesktopShortcutSettings>(defaultShortcuts);
  const [pulseTargets, setPulseTargets] = useState<Set<PulseTarget>>(new Set());
  const [commandErrors, setCommandErrors] = useState<Partial<Record<CommandKey, string>>>({});
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const pollInFlightRef = useRef(false);
  const autoScanInFlightRef = useRef<Set<string>>(new Set());
  const lastFocusScanRef = useRef<{ localSpaceId: string; at: number }>({ localSpaceId: "", at: 0 });
  const pulseTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const effectiveBootstrap = bootstrap ?? status?.cachedBootstrap ?? null;
  const isConnected = Boolean(status?.connected || effectiveBootstrap?.account);

  const recordCommandError = useCallback(
    (key: CommandKey, nextError: unknown) => {
      const message = errorMessage(nextError);
      setCommandErrors((current) => ({ ...current, [key]: message }));
      notify({
        tone: "danger",
        title: "Action failed",
        message,
        dedupeKey: `desktop-error-${key}`
      });
    },
    [notify]
  );

  const clearCommandError = useCallback((key: CommandKey) => {
    setCommandErrors((current) => {
      const next = { ...current };
      delete next[key];
      return next;
    });
  }, []);

  const replaceLocalSpace = useCallback((nextSpace: LocalSpaceSummary) => {
    setLocalSpaces((current) => {
      const index = current.findIndex((space) => space.localSpaceId === nextSpace.localSpaceId);
      if (index === -1) {
        return [...current, nextSpace];
      }
      return current.map((space) => (space.localSpaceId === nextSpace.localSpaceId ? nextSpace : space));
    });
  }, []);

  const scanLocalSpace = useCallback(
    async (localSpaceId: string) => {
      setBusyAction(`scan:${localSpaceId}`);
      try {
        const scan = await scanWorkingTree(localSpaceId);
        setWorkingTrees((current) => ({ ...current, [localSpaceId]: scan }));
        setScanRevisions((current) => ({ ...current, [localSpaceId]: (current[localSpaceId] ?? 0) + 1 }));
        setDiffWindowOverrides((current) =>
          Object.fromEntries(
            Object.entries(current).filter(([key]) => !key.startsWith(`${localSpaceId}::`))
          )
        );
        clearCommandError("scan");
        return scan;
      } catch (nextError) {
        recordCommandError("scan", nextError);
        throw nextError;
      } finally {
        setBusyAction(null);
      }
    },
    [clearCommandError, notify, recordCommandError]
  );

  const runAutoScan = useCallback(
    (localSpaceId: string) => {
      if (autoScanInFlightRef.current.has(localSpaceId)) {
        return;
      }

      autoScanInFlightRef.current.add(localSpaceId);
      void scanLocalSpace(localSpaceId)
        .catch(() => {
          // The scan command already records the visible error state.
        })
        .finally(() => {
          autoScanInFlightRef.current.delete(localSpaceId);
        });
    },
    [scanLocalSpace]
  );

  const loadSpaces = useCallback(
    async (showMessage = false) => {
      const [availableResult, localResult] = await Promise.allSettled([listAvailableSpaces(), listLocalSpaces()]);

      if (availableResult.status === "fulfilled") {
        setAvailableSpaces(availableResult.value);
        clearCommandError("available");
      } else {
        recordCommandError("available", availableResult.reason);
      }

      if (localResult.status === "fulfilled") {
        setLocalSpaces(localResult.value);
        clearCommandError("local");
        setSelectedLocalSpaceId((current) => current ?? localResult.value[0]?.localSpaceId ?? null);
      } else {
        recordCommandError("local", localResult.reason);
      }

      if (showMessage && availableResult.status === "fulfilled" && localResult.status === "fulfilled") {
        notify({ tone: "success", title: "Distant Spaces refreshed", dedupeKey: "desktop-distant-refreshed" });
      }

      if (availableResult.status === "fulfilled") {
        try {
          const nextStatus = await getDesktopStatus();
          setStatus(nextStatus);
          setEndpointDraft(nextStatus.serverEndpoint);
          if (nextStatus.cachedBootstrap) {
            setBootstrap(nextStatus.cachedBootstrap);
          }
        } catch {
          // Distant already surfaced the live/cache result; status refresh is best-effort here.
        }
      }
    },
    [clearCommandError, notify, recordCommandError]
  );

  const hydrateStatus = useCallback(async () => {
    setLoadState("loading");
    setError(null);
    try {
      const [statusResult, settingsResult] = await Promise.allSettled([getDesktopStatus(), loadDesktopSettings()]);

      if (statusResult.status === "rejected") {
        throw statusResult.reason;
      }

      const nextStatus = statusResult.value;
      setStatus(nextStatus);
      setBootstrap(nextStatus.cachedBootstrap ?? null);
      setEndpointDraft(nextStatus.serverEndpoint);
      const loadedSettings = settingsResult.status === "fulfilled" ? settingsResult.value : undefined;

      if (settingsResult.status === "fulfilled") {
        applySettings(settingsResult.value);
        clearCommandError("settings");
      } else {
        recordCommandError("settings", settingsResult.reason);
      }

      if (nextStatus.connected || nextStatus.cachedBootstrap) {
        try {
          const response = await refreshBootstrap(loadedSettings?.defaultLocalSpacesFolder ?? "");
          applyPollResponse(response);
        } catch {
          notify({ tone: "info", title: "Using cached account", message: "Distant refresh failed, but cached account data is available.", dedupeKey: "desktop-bootstrap-cache" });
          // A cached account is still useful offline; Distant refresh below will expose detailed errors.
        }
        await loadSpaces();
      }

      setLoadState("ready");
    } catch (nextError) {
      setLoadState("error");
      setError(errorMessage(nextError));
    }
  }, [clearCommandError, loadSpaces, notify, recordCommandError]);

  useEffect(() => {
    void hydrateStatus();
  }, [hydrateStatus]);

  useEffect(() => {
    const onHashChange = () => setPage(pageFromHash(window.location.hash));
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  useEffect(() => {
    if (!login || pollStatus === "connected" || pollStatus === "denied" || pollStatus === "expired") {
      return undefined;
    }

    const intervalMs = Math.max(login.interval, 2) * 1000;
    const timer = window.setInterval(() => {
      if (!pollInFlightRef.current) {
        void pollOnce(login.deviceCode);
      }
    }, intervalMs);

    return () => window.clearInterval(timer);
  }, [login, pollStatus, defaultLocalRoot]);

  useEffect(() => {
    setSendWorkspaceId((current) => current || effectiveBootstrap?.workspaces[0]?.id || "");
  }, [effectiveBootstrap]);

  const selectedLocalSpace = localSpaces.find((space) => space.localSpaceId === selectedLocalSpaceId) ?? localSpaces[0] ?? null;
  const selectedWorkingTree = selectedLocalSpace ? workingTrees[selectedLocalSpace.localSpaceId] : undefined;
  const selectedLayer =
    selectedLocalSpace?.layers.find((layer) => layer.layerId === selectedLocalSpace.activeLayerId) ?? selectedLocalSpace?.layers[0] ?? null;
  const files = buildLayerFiles(selectedWorkingTree, selectedLayer);
  const activeDiffStepId = localSpaceTab === "steps" ? selectedStepId : null;
  const activeDiffContextKey = activeDiffStepId ? `step:${activeDiffStepId}` : "workingTree";
  const workingTreeChanges = buildChanges(selectedWorkingTree, null);
  const changes = activeDiffStepId ? buildChanges(selectedWorkingTree, activeDiffStepId) : workingTreeChanges;
  const baseSelectedDiff =
    changes.find((change) => change.path === selectedDiffPath)?.diff ?? changes[0]?.diff ?? null;
  const selectedDiffKey = diffWindowKey(
    selectedLocalSpace?.localSpaceId,
    activeDiffStepId,
    baseSelectedDiff?.path
  );
  const selectedDiff =
    selectedDiffKey && diffWindowOverrides[selectedDiffKey]
      ? diffWindowOverrides[selectedDiffKey]
      : baseSelectedDiff;
  const timeline = buildTimeline(selectedLocalSpace, selectedWorkingTree, selectedStepId);
  const selectedScanRevision = selectedLocalSpace ? scanRevisions[selectedLocalSpace.localSpaceId] ?? 0 : 0;
  const workspaceName = selectedLocalSpace?.name ?? availableSpaces[0]?.name ?? effectiveBootstrap?.workspaces[0]?.name ?? "Layrs Desktop";
  const connectedLabel = isConnected ? effectiveBootstrap?.account?.email ?? "Connected device" : "Not connected";
  const forgetTarget = forgetTargetId ? localSpaces.find((space) => space.localSpaceId === forgetTargetId) ?? null : null;
  const deleteLayerTarget = deleteLayerTargetId && selectedLocalSpace
    ? selectedLocalSpace.layers.find((layer) => layer.layerId === deleteLayerTargetId) ?? null
    : null;

  useEffect(() => {
    if (!selectedStepId || selectedWorkingTree?.steps?.some((step) => step.stepId === selectedStepId)) {
      return;
    }
    setSelectedStepId(null);
    setSelectedDiffPath(null);
  }, [selectedStepId, selectedWorkingTree]);

  useEffect(() => {
    if (
      !selectedLocalSpace ||
      workingTrees[selectedLocalSpace.localSpaceId]
    ) {
      return;
    }

    runAutoScan(selectedLocalSpace.localSpaceId);
  }, [runAutoScan, selectedLocalSpace, workingTrees]);

  useEffect(() => {
    const localSpaceId = selectedLocalSpace?.localSpaceId;
    if (!localSpaceId) {
      return undefined;
    }

    function scanOnFocus() {
      if (document.visibilityState === "hidden") {
        return;
      }

      const now = Date.now();
      const last = lastFocusScanRef.current;
      if (last.localSpaceId === localSpaceId && now - last.at < FOCUS_SCAN_THROTTLE_MS) {
        return;
      }

      lastFocusScanRef.current = { localSpaceId, at: now };
      runAutoScan(localSpaceId);
    }

    function scanOnVisibilityChange() {
      if (document.visibilityState === "visible") {
        scanOnFocus();
      }
    }

    window.addEventListener("focus", scanOnFocus);
    document.addEventListener("visibilitychange", scanOnVisibilityChange);
    return () => {
      window.removeEventListener("focus", scanOnFocus);
      document.removeEventListener("visibilitychange", scanOnVisibilityChange);
    };
  }, [runAutoScan, selectedLocalSpace?.localSpaceId]);

  useEffect(
    () => () => {
      if (pulseTimerRef.current) {
        window.clearTimeout(pulseTimerRef.current);
      }
    },
    []
  );

  function triggerPulse(targets: PulseTarget[]) {
    if (pulseTimerRef.current) {
      window.clearTimeout(pulseTimerRef.current);
    }
    setPulseTargets(new Set(targets));
    pulseTimerRef.current = window.setTimeout(() => setPulseTargets(new Set()), 1100);
  }

  function applySettings(settings: DesktopSettings) {
    setEndpointDraft(settings.serverEndpoint);
    setDefaultLocalRoot(settings.defaultLocalSpacesFolder);
    setAutoReceive(settings.autoReceive);
    setAutoPublish(settings.autoPublish);
    setAutoLocalSteps(settings.autoLocalSteps);
    setSyncIntervalMinutes(Math.max(1, Math.round(settings.syncIntervalSeconds / 60)));
    setShortcuts(settings.shortcuts ?? defaultShortcuts);
    setDraftSpaceFolder((current) => current || settings.defaultLocalSpacesFolder);
  }

  function currentSettings(): DesktopSettings {
    return {
      serverEndpoint: endpointDraft,
      autoReceive,
      autoPublish,
      autoLocalSteps,
      syncIntervalSeconds: Number.isFinite(syncIntervalMinutes) ? Math.max(60, syncIntervalMinutes * 60) : 900,
      defaultLocalSpacesFolder: defaultLocalRoot,
      shortcuts
    };
  }

  async function saveSettings() {
    setError(null);
    const shortcutError = validateShortcuts(shortcuts);
    if (shortcutError) {
      setError(shortcutError);
      notify({ tone: "danger", title: "Shortcuts not saved", message: shortcutError, dedupeKey: "desktop-shortcut-validation" });
      return;
    }
    setBusyAction("settings");
    try {
      const nextSettings = await saveDesktopSettings(currentSettings());
      applySettings(nextSettings);
      setStatus((current) => (current ? { ...current, serverEndpoint: nextSettings.serverEndpoint } : current));
      clearCommandError("settings");
      notify({ tone: "success", title: "Settings saved", dedupeKey: "desktop-settings-saved" });
    } catch (nextError) {
      recordCommandError("settings", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
    }
  }

  async function beginLogin() {
    if (pollInFlightRef.current) {
      return;
    }

    setError(null);
    setPollStatus(null);
    setLogin(null);
    try {
      const nextLogin = await startDeviceLogin();
      setLogin(nextLogin);
      setPollStatus("pending");
      notify({
        tone: "info",
        title: "Device login started",
        message: nextLogin.message ?? "Enter the user code in Layrs Studio, then leave this window open.",
        dedupeKey: "desktop-device-login"
      });
    } catch (nextError) {
      setError(errorMessage(nextError));
    }
  }

  async function pollOnce(deviceCode: string) {
    if (pollInFlightRef.current) {
      return;
    }

    pollInFlightRef.current = true;
    setPollInFlight(true);
    setError(null);
    try {
      const response = await pollDeviceLogin(deviceCode, defaultLocalRoot);
      applyPollResponse(response);
      await loadSpaces();
    } catch (nextError) {
      if (isConsumedDeviceCodeError(nextError)) {
        await recoverConsumedDeviceCode();
      } else {
        setError(errorMessage(nextError));
      }
    } finally {
      pollInFlightRef.current = false;
      setPollInFlight(false);
    }
  }

  async function refreshConnectedBootstrap() {
    setError(null);
    setBusyAction("refresh");
    try {
      const response = await refreshBootstrap(defaultLocalRoot);
      applyPollResponse(response);
      await loadSpaces(true);
    } catch (nextError) {
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
    }
  }

  async function chooseFolder(initialPath: string | undefined, onChoose: (folder: string) => void) {
    setError(null);
    try {
      const folder = await selectFolder(initialPath || defaultLocalRoot || undefined);
      if (folder) {
        onChoose(folder);
      }
    } catch (nextError) {
      setError(errorMessage(nextError));
    }
  }

  function chooseAvailableTargetFolder(space: AvailableSpaceView) {
    const draft = createDrafts[space.spaceId] ?? defaultCreateDraft(space, defaultLocalRoot);
    void chooseFolder(draft.targetFolder, (folder) => {
      setCreateDrafts((current) => ({
        ...current,
        [space.spaceId]: {
          ...(current[space.spaceId] ?? draft),
          targetFolder: folder
        }
      }));
    });
  }

  function chooseDraftFolder() {
    void chooseFolder(draftSpaceFolder, setDraftSpaceFolder);
  }

  function chooseInitFolder() {
    void chooseFolder(initSpaceFolder, (folder) => {
      setInitSpaceFolder(folder);
      setInitSpaceName((current) => current || nameFromFolder(folder));
    });
  }

  function chooseDefaultLocalSpacesFolder() {
    void chooseFolder(defaultLocalRoot, setDefaultLocalRoot);
  }

  function applyPollResponse(response: DeviceLoginPollResponse) {
    if (response.status === "connected") {
      setLogin(null);
      setPollStatus(null);
    } else {
      setPollStatus(response.status);
    }

    notify({
      tone: response.status === "authorized" || response.status === "connected" ? "success" : "info",
      title: statusLabels[response.status] ?? response.status,
      message: response.message,
      dedupeKey: "desktop-device-poll"
    });

    if (response.bootstrap) {
      setBootstrap(response.bootstrap);
      setStatus((current) => (current ? { ...current, connected: true, cachedBootstrap: response.bootstrap } : current));
    } else if (response.account) {
      const accountBootstrap: BootstrapData = {
        account: response.account,
        workspaces: [],
        spaces: [],
        layers: []
      };
      setBootstrap(accountBootstrap);
      setStatus((current) => (current ? { ...current, connected: true, cachedBootstrap: accountBootstrap } : current));
    }
  }

  async function recoverConsumedDeviceCode() {
    try {
      const nextStatus = await getDesktopStatus();
      setStatus(nextStatus);
      setEndpointDraft(nextStatus.serverEndpoint);
      setBootstrap(nextStatus.cachedBootstrap ?? null);
      setLoadState("ready");

      if (nextStatus.connected || nextStatus.cachedBootstrap) {
        setLogin(null);
        setPollStatus(null);
        notify({ tone: "success", title: "Desktop connection refreshed", dedupeKey: "desktop-device-consumed" });
        await loadSpaces();
        return;
      }
    } catch {
      // Fall through to an authenticated bootstrap refresh before showing a new-login hint.
    }

    try {
      const response = await refreshBootstrap(defaultLocalRoot);
      applyPollResponse(response);
      if (response.status === "connected" || response.bootstrap) {
        setLogin(null);
        setPollStatus(null);
        notify({ tone: "success", title: "Desktop connection refreshed", dedupeKey: "desktop-device-consumed" });
        await loadSpaces();
        return;
      }
    } catch {
      // The consumed code could not be recovered from the local session.
    }

    setLogin(null);
    setPollStatus(null);
    setError("This device code was already used. Start a new login from Settings.");
    notify({
      tone: "warning",
      title: "Device code already used",
      message: "Start a new login from Settings.",
      dedupeKey: "desktop-device-code-used"
    });
  }

  function choosePage(nextPage: DesktopPage) {
    setPage(nextPage);
    window.location.hash = `desktop-${nextPage}`;
  }

  async function selectLocalSpace(localSpaceId: string) {
    setSelectedLocalSpaceId(localSpaceId);
    choosePage("local");
    if (!workingTrees[localSpaceId]) {
      try {
        await scanLocalSpace(localSpaceId);
      } catch {
        // Error state is already recorded for the scan action.
      }
    }
  }

  async function handleCreateLocalSpace(space: AvailableSpaceView) {
    const draft = createDrafts[space.spaceId] ?? defaultCreateDraft(space, defaultLocalRoot);
    if (!draft.targetFolder.trim()) {
      setError("Choose a target folder before creating a Local Space.");
      return;
    }

    setBusyAction(`create:${space.spaceId}`);
    setError(null);
    try {
      const result = await createLocalSpace(space.spaceId, draft.targetFolder.trim(), draft.layerId || undefined);
      replaceLocalSpace(result.localSpace);
      setSelectedLocalSpaceId(result.localSpace.localSpaceId);
      choosePage("local");
      await scanLocalSpace(result.localSpace.localSpaceId);
      await loadSpaces();
      clearCommandError("create");
      notify({
        tone: "success",
        title: result.created ? "Local Space created" : "Existing Local Space opened",
        dedupeKey: "desktop-local-space-created"
      });
    } catch (nextError) {
      recordCommandError("create", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
    }
  }

  async function handleCreateDraftLocalSpace() {
    const name = draftSpaceName.trim();
    const targetFolder = draftSpaceFolder.trim();
    if (!name) {
      setError("Name the empty local Space before creating it.");
      return;
    }
    if (!targetFolder) {
      setError("Choose a folder before creating an empty local Space.");
      return;
    }

    setBusyAction("create-draft");
    setError(null);
    try {
      const result = await createDraftLocalSpace(name, targetFolder);
      replaceLocalSpace(result.localSpace);
      setSelectedLocalSpaceId(result.localSpace.localSpaceId);
      setDraftSpaceName("");
      choosePage("local");
      await scanLocalSpace(result.localSpace.localSpaceId);
      clearCommandError("create-draft");
      notify({ tone: "success", title: "Draft Local Space created", dedupeKey: "desktop-draft-created" });
    } catch (nextError) {
      recordCommandError("create-draft", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
    }
  }

  async function handleInitLocalSpace() {
    const name = initSpaceName.trim();
    const targetFolder = initSpaceFolder.trim();
    if (!name) {
      setError("Name the Local Space before initializing this folder.");
      return;
    }
    if (!targetFolder) {
      setError("Choose an existing folder before initializing a Local Space.");
      return;
    }

    setBusyAction("init-local");
    setError(null);
    try {
      const result = await initLocalSpace(name, targetFolder);
      replaceLocalSpace(result.localSpace);
      setSelectedLocalSpaceId(result.localSpace.localSpaceId);
      setInitSpaceName("");
      setInitSpaceFolder("");
      setSelectedStepId(null);
      setSelectedDiffPath(null);
      setLocalSpaceTab("changes");
      choosePage("local");
      await scanLocalSpace(result.localSpace.localSpaceId);
      await loadSpaces();
      clearCommandError("init-local");
      triggerPulse(["changes", "steps"]);
      notify({
        tone: "success",
        title: result.created ? "Folder initialized" : "Local Space opened",
        dedupeKey: "desktop-local-init"
      });
    } catch (nextError) {
      recordCommandError("init-local", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
    }
  }

  async function handleSendDraftToStudio() {
    if (!selectedLocalSpace) {
      return;
    }
    if (!sendWorkspaceId.trim()) {
      setError("Choose a Workspace before sending this Draft Local Space.");
      return;
    }

    setBusyAction("send-draft");
    setError(null);
    try {
      const result = await sendDraftLocalSpace(selectedLocalSpace.localSpaceId, sendWorkspaceId.trim());
      replaceLocalSpace(result.localSpace);
      setSelectedLocalSpaceId(result.localSpace.localSpaceId);
      await scanLocalSpace(result.localSpace.localSpaceId);
      await loadSpaces();
      clearCommandError("send-draft");
      notify({
        tone: "success",
        title: "Draft sent to Studio",
        message: `${result.publishedLayers} Layer(s) published.`,
        dedupeKey: "desktop-draft-sent"
      });
    } catch (nextError) {
      recordCommandError("send-draft", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
    }
  }

  async function handleOpenLocalSpace(localSpaceIdOrPath: string) {
    setBusyAction(`open:${localSpaceIdOrPath}`);
    setError(null);
    try {
      const nextSpace = await openLocalSpace(localSpaceIdOrPath);
      replaceLocalSpace(nextSpace);
      setSelectedLocalSpaceId(nextSpace.localSpaceId);
      choosePage("local");
      await scanLocalSpace(nextSpace.localSpaceId);
      clearCommandError("open");
      notify({ tone: "success", title: "Local Space opened", dedupeKey: "desktop-local-opened" });
    } catch (nextError) {
      recordCommandError("open", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
    }
  }

  async function handleForgetLocalSpace(localSpaceId: string) {
    const space = localSpaces.find((entry) => entry.localSpaceId === localSpaceId);
    if (!space) {
      return;
    }
    setForgetTargetId(localSpaceId);
  }

  async function confirmForgetLocalSpace(localSpaceId: string) {
    setBusyAction("forget-local");
    setError(null);
    try {
      const result = await forgetLocalSpace(localSpaceId);
      setLocalSpaces((current) => current.filter((entry) => entry.localSpaceId !== localSpaceId));
      setWorkingTrees((current) => {
        const next = { ...current };
        delete next[localSpaceId];
        return next;
      });
      setScanRevisions((current) => {
        const next = { ...current };
        delete next[localSpaceId];
        return next;
      });
      setDiffWindowOverrides((current) => {
        const next: Record<string, LensDiffEntry> = {};
        for (const [key, value] of Object.entries(current)) {
          if (!key.startsWith(`${localSpaceId}:`)) {
            next[key] = value;
          }
        }
        return next;
      });
      setSelectedDiffPath(null);
      setSelectedStepId(null);
      setSelectedLocalSpaceId((current) => {
        if (current !== localSpaceId) {
          return current;
        }
        const nextSpace = localSpaces.find((entry) => entry.localSpaceId !== localSpaceId);
        return nextSpace?.localSpaceId ?? null;
      });
      clearCommandError("forget-local");
      notify({
        tone: "success",
        title: "Local Space forgotten",
        message: result.archivedLayrsPath ? `${result.message} Archived metadata: ${result.archivedLayrsPath}` : result.message,
        dedupeKey: "desktop-forgot-local"
      });
      await refreshConnectedBootstrap();
    } catch (nextError) {
      recordCommandError("forget-local", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
      setForgetTargetId(null);
    }
  }

  async function handleSwitchLayer(layerId: string) {
    if (!selectedLocalSpace || layerId === selectedLocalSpace.activeLayerId) {
      return;
    }

    setBusyAction(`switch:${layerId}`);
    setError(null);
    try {
      const result = await switchLayer(selectedLocalSpace.localSpaceId, layerId);
      replaceLocalSpace(result.localSpace);
      setSelectedLocalSpaceId(result.localSpace.localSpaceId);
      await scanLocalSpace(result.localSpace.localSpaceId);
      clearCommandError("switch");
      triggerPulse(["layer", "steps"]);
      notify({
        tone: "success",
        title: "Layer switched",
        message: `${result.changedFiles} changed file(s) preserved in local Steps.`,
        dedupeKey: "desktop-layer-switched"
      });
    } catch (nextError) {
      recordCommandError("switch", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
    }
  }

  async function handleCreateLayerFromCurrent() {
    if (!selectedLocalSpace) {
      return;
    }
    const layerName = newLayerName.trim();
    if (!layerName) {
      setError("Name the new Layer before creating it.");
      return;
    }

    setBusyAction("create-layer");
    setError(null);
    try {
      const result = await createLayerFromCurrent(selectedLocalSpace.localSpaceId, layerName);
      replaceLocalSpace(result.localSpace);
      setSelectedLocalSpaceId(result.localSpace.localSpaceId);
      setNewLayerName("");
      await scanLocalSpace(result.localSpace.localSpaceId);
      clearCommandError("create-layer");
      triggerPulse(["layer", "steps"]);
      notify({ tone: "success", title: "Layer created from current files", dedupeKey: "desktop-layer-created" });
    } catch (nextError) {
      recordCommandError("create-layer", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
    }
  }

  async function handleDeleteLayer(layerId: string) {
    if (!selectedLocalSpace) {
      return;
    }
    setDeleteLayerTargetId(layerId);
  }

  async function confirmDeleteLayer(layerId: string) {
    if (!selectedLocalSpace) {
      return;
    }

    setBusyAction(`delete-layer:${layerId}`);
    setError(null);
    try {
      const result = await deleteLayer(selectedLocalSpace.localSpaceId, layerId);
      replaceLocalSpace(result.localSpace);
      setSelectedLocalSpaceId(result.localSpace.localSpaceId);
      setSelectedStepId(null);
      setSelectedDiffPath(null);
      await scanLocalSpace(result.localSpace.localSpaceId);
      await loadSpaces();
      clearCommandError("delete-layer");
      triggerPulse(["layer"]);
      notify({ tone: "success", title: "Layer deleted", message: result.message, dedupeKey: "desktop-layer-deleted" });
    } catch (nextError) {
      recordCommandError("delete-layer", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
      setDeleteLayerTargetId(null);
    }
  }

  async function handleReceive() {
    if (!selectedLocalSpace) {
      return;
    }

    setBusyAction("receive");
    setError(null);
    try {
      const result = await receiveLocalSpace(selectedLocalSpace.localSpaceId);
      replaceLocalSpace(result.localSpace);
      await scanLocalSpace(result.localSpace.localSpaceId);
      await loadSpaces();
      clearCommandError("receive");
      triggerPulse(["sync", "changes"]);
      notify({ tone: "success", title: "Receive complete", message: result.message, dedupeKey: "desktop-receive" });
    } catch (nextError) {
      recordCommandError("receive", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
    }
  }

  async function handleSaveStep() {
    if (!selectedLocalSpace) {
      notify({ tone: "warning", title: "No Local Space selected", dedupeKey: "desktop-save-no-space" });
      return;
    }

    setBusyAction("save-step");
    setError(null);
    try {
      const result = await saveLocalStep(selectedLocalSpace.localSpaceId);
      replaceLocalSpace(result.localSpace);
      await scanLocalSpace(result.localSpace.localSpaceId);
      clearCommandError("save-step");
      if (result.status === "clean") {
        notify({ tone: "info", title: "Nothing to save", message: result.message, dedupeKey: "desktop-save-clean" });
      } else {
        triggerPulse(["changes", "steps", "sync", "layer"]);
        notify({
          tone: "success",
          title: "Step created",
          message: `${result.changedFiles} file(s), +${result.diffStats.additions}, -${result.diffStats.deletions}`,
          dedupeKey: "desktop-step-saved"
        });
      }
    } catch (nextError) {
      recordCommandError("save-step", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
    }
  }

  async function handleSmartSaveShortcut() {
    if (!selectedLocalSpace) {
      notify({ tone: "warning", title: "No Local Space selected", dedupeKey: "desktop-save-no-space" });
      return;
    }

    if (workingTreeChanges.length > 0) {
      await handleSaveStep();
      return;
    }

    if (shortcuts.smartSavePublishesPendingStep && (selectedWorkingTree?.pendingPublishCount ?? 0) > 0) {
      await handlePublish();
      return;
    }

    notify({ tone: "info", title: "Nothing to save", message: "No local changes or pending publish step.", dedupeKey: "desktop-save-nothing" });
  }

  async function handlePublish() {
    if (!selectedLocalSpace) {
      return;
    }
    if (selectedLocalSpace.state === "draft") {
      if (!sendWorkspaceId.trim()) {
        notify({
          tone: "warning",
          title: "Choose a Workspace",
          message: "Draft publish creates the Space in Studio first.",
          dedupeKey: "desktop-draft-publish-workspace"
        });
        setError("Choose a Workspace before publishing this Draft Local Space.");
        return;
      }
      await handleSendDraftToStudio();
      return;
    }

    setBusyAction("publish");
    setError(null);
    try {
      const result = await publishLocalSpace(selectedLocalSpace.localSpaceId);
      replaceLocalSpace(result.localSpace);
      await scanLocalSpace(result.localSpace.localSpaceId);
      await loadSpaces();
      clearCommandError("publish");
      triggerPulse(["sync", "steps"]);
      notify({ tone: result.status === "clean" ? "info" : "success", title: "Publish complete", message: result.message, dedupeKey: "desktop-publish" });
    } catch (nextError) {
      recordCommandError("publish", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
    }
  }

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (
        !shortcuts.enabled ||
        loadState !== "ready" ||
        !selectedLocalSpace ||
        forgetTarget ||
        deleteLayerTarget ||
        isEditableShortcutTarget(event.target)
      ) {
        return;
      }

      const pressed = shortcutFromKeyboardEvent(event);
      if (!pressed) {
        return;
      }

      if (shortcutMatches(pressed, shortcuts.saveStep)) {
        event.preventDefault();
        void handleSmartSaveShortcut();
      } else if (shortcutMatches(pressed, shortcuts.publish)) {
        event.preventDefault();
        void handlePublish();
      }
    }

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [deleteLayerTarget, forgetTarget, loadState, selectedLocalSpace, shortcuts, workingTreeChanges.length, selectedWorkingTree?.pendingPublishCount]);

  async function handleLoadDiffWindow(path: string, start: number, limit: number) {
    if (!selectedLocalSpace) {
      return;
    }

    setBusyAction(`diff-window:${path}`);
    setError(null);
    try {
      const sourceStepId = localSpaceTab === "steps" ? selectedStepId : null;
      const source = sourceStepId ? `localStep:${sourceStepId}` : "workingTree";
      const nextDiff = await loadDiffWindow(selectedLocalSpace.localSpaceId, path, source, start, limit);
      setDiffWindowOverrides((current) => ({
        ...current,
        [diffWindowKey(selectedLocalSpace.localSpaceId, sourceStepId, path)]: nextDiff
      }));
      clearCommandError("diff-window");
    } catch (nextError) {
      recordCommandError("diff-window", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
    }
  }

  function selectTimelineItem(item: TimelineItem) {
    setSelectedStepId(item.kind === "step" ? item.id : null);
    setSelectedDiffPath(null);
  }

  return (
    <AppShell
      productName="Layrs Desktop"
      workspaceName={workspaceName}
      sidebar={
        <Sidebar
          items={[
            {
              id: "desktop-available",
              label: "Distant",
              eyebrow: "Server",
              isActive: page === "available",
              meta: `${availableSpaces.length}`
            },
            {
              id: "desktop-local",
              label: "Local",
              eyebrow: "This machine",
              isActive: page === "local",
              meta: `${localSpaces.length}`
            },
            {
              id: "desktop-draft",
              label: "Local setup",
              eyebrow: "Offline",
              isActive: page === "draft"
            },
            {
              id: "desktop-settings",
              label: "Settings",
              eyebrow: "Device",
              isActive: page === "settings"
            }
          ]}
          footer={<ShortcutFooter hasLocalSpace={Boolean(selectedLocalSpace)} shortcuts={shortcuts} />}
        />
      }
      toolbar={
        <>
          <div className="desktop-title">
            <span>{connectedLabel}</span>
            <strong>{pageTitle(page, selectedLocalSpace)}</strong>
          </div>
          <div className="desktop-toolbar-actions">
            <button
              type="button"
              className="desktop-ghost-button"
              onClick={refreshConnectedBootstrap}
              disabled={!isConnected || busyAction === "refresh"}
            >
              Refresh distant
            </button>
            <StatusPill status={isConnected ? "passing" : "needs-proof"} label={isConnected ? "Secure session" : "Connect device"} />
          </div>
        </>
      }
    >
      <section className="desktop-page">
        {loadState === "loading" ? <p className="desktop-alert">Loading desktop state...</p> : null}
        {error ? <p className="desktop-alert desktop-alert--error">{error}</p> : null}
        <CommandErrors errors={commandErrors} />

        {page === "available" ? (
          <AvailableSpacesView
            spaces={availableSpaces}
            createDrafts={createDrafts}
            defaultLocalRoot={defaultLocalRoot}
            connected={isConnected}
            busyAction={busyAction}
            onRefresh={() => void refreshConnectedBootstrap()}
            onOpenLocal={(localSpaceId) => void handleOpenLocalSpace(localSpaceId)}
            onCreate={(space) => void handleCreateLocalSpace(space)}
            onDraftChange={(spaceId, draft) => setCreateDrafts((current) => ({ ...current, [spaceId]: draft }))}
            onChooseTargetFolder={chooseAvailableTargetFolder}
          />
        ) : null}

        {page === "local" ? (
          <LocalSpacesView
            localSpaces={localSpaces}
            selectedSpace={selectedLocalSpace}
            selectedLayer={selectedLayer}
            files={files}
            changes={changes}
            selectedDiff={selectedDiff}
            selectedDiffPath={selectedDiff?.path ?? selectedDiffPath}
            selectedDiffContextKey={activeDiffContextKey}
            selectedScanRevision={selectedScanRevision}
            timeline={timeline}
            workingTree={selectedWorkingTree}
            workingTreeChangeCount={workingTreeChanges.length}
            workspaces={effectiveBootstrap?.workspaces ?? []}
            sendWorkspaceId={sendWorkspaceId}
            newLayerName={newLayerName}
            layerSearchQuery={layerSearchQuery}
            pulseTargets={pulseTargets}
            activeTab={localSpaceTab}
            busyAction={busyAction}
            commandErrors={commandErrors}
            onSelectSpace={(localSpaceId) => void selectLocalSpace(localSpaceId)}
            onOpenSpace={(localSpaceId) => void handleOpenLocalSpace(localSpaceId)}
            onForgetSpace={(localSpaceId) => void handleForgetLocalSpace(localSpaceId)}
            onScan={(localSpaceId) => void scanLocalSpace(localSpaceId)}
            onSelectDiff={setSelectedDiffPath}
            onLoadDiffWindow={(path, start, limit) => void handleLoadDiffWindow(path, start, limit)}
            onSelectTimeline={selectTimelineItem}
            onSendWorkspaceChange={setSendWorkspaceId}
            onSendDraft={() => void handleSendDraftToStudio()}
            onSelectLayer={(layerId) => void handleSwitchLayer(layerId)}
            onNewLayerNameChange={setNewLayerName}
            onLayerSearchChange={setLayerSearchQuery}
            onTabChange={setLocalSpaceTab}
            onCreateLayer={() => void handleCreateLayerFromCurrent()}
            onDeleteLayer={(layerId) => void handleDeleteLayer(layerId)}
            onReceive={() => void handleReceive()}
            onPublish={() => void handlePublish()}
          />
        ) : null}

        {page === "draft" ? (
          <DraftLocalSpaceView
            busyAction={busyAction}
            draftSpaceFolder={draftSpaceFolder}
            draftSpaceName={draftSpaceName}
            initSpaceFolder={initSpaceFolder}
            initSpaceName={initSpaceName}
            onChooseDraftFolder={chooseDraftFolder}
            onChooseInitFolder={chooseInitFolder}
            onCreateDraftSpace={() => void handleCreateDraftLocalSpace()}
            onInitLocalSpace={() => void handleInitLocalSpace()}
            onDraftSpaceNameChange={setDraftSpaceName}
            onInitSpaceNameChange={setInitSpaceName}
          />
        ) : null}

        {page === "settings" ? (
          <SettingsView
            status={status}
            bootstrap={effectiveBootstrap}
            endpointDraft={endpointDraft}
            defaultLocalRoot={defaultLocalRoot}
            login={login}
            pollStatus={pollStatus}
            pollInFlight={pollInFlight}
            autoReceive={autoReceive}
            autoPublish={autoPublish}
            autoLocalSteps={autoLocalSteps}
            syncIntervalMinutes={syncIntervalMinutes}
            shortcuts={shortcuts}
            loadState={loadState}
            saving={busyAction === "settings"}
            onEndpointChange={setEndpointDraft}
            onChooseDefaultRoot={chooseDefaultLocalSpacesFolder}
            onSaveSettings={saveSettings}
            onBeginLogin={beginLogin}
            onPollNow={() => {
              if (login) {
                void pollOnce(login.deviceCode);
              }
            }}
            onAutoReceiveChange={setAutoReceive}
            onAutoPublishChange={setAutoPublish}
            onAutoLocalStepsChange={setAutoLocalSteps}
            onSyncIntervalChange={setSyncIntervalMinutes}
            onShortcutsChange={setShortcuts}
          />
        ) : null}
      </section>
      <ConfirmModal
        confirmLabel="Forget local"
        danger
        description={
          <p>
            Layrs will keep the project files, archive local .layrs metadata, and disconnect this folder from Studio so
            it can be pulled again.
          </p>
        }
        disabled={!forgetTarget}
        onCancel={() => setForgetTargetId(null)}
        onConfirm={() => forgetTarget && void confirmForgetLocalSpace(forgetTarget.localSpaceId)}
        open={Boolean(forgetTarget)}
        title={`Forget ${forgetTarget?.name ?? "Local Space"}`}
      />
      <ConfirmModal
        confirmLabel="Delete Layer"
        danger
        description={<p>Deleting a Layer removes its local Layer state. Keep this action away from receive and publish.</p>}
        disabled={!deleteLayerTarget}
        onCancel={() => setDeleteLayerTargetId(null)}
        onConfirm={() => deleteLayerTarget && void confirmDeleteLayer(deleteLayerTarget.layerId)}
        open={Boolean(deleteLayerTarget)}
        title={`Delete ${deleteLayerTarget?.displayName ?? "Layer"}`}
      />
    </AppShell>
  );
}

interface AvailableSpacesViewProps {
  spaces: AvailableSpaceView[];
  createDrafts: Record<string, CreateDraft>;
  defaultLocalRoot: string;
  connected: boolean;
  busyAction: string | null;
  onRefresh: () => void;
  onOpenLocal: (localSpaceId: string) => void;
  onCreate: (space: AvailableSpaceView) => void;
  onDraftChange: (spaceId: string, draft: CreateDraft) => void;
  onChooseTargetFolder: (space: AvailableSpaceView) => void;
}

function AvailableSpacesView({
  spaces,
  createDrafts,
  defaultLocalRoot,
  connected,
  busyAction,
  onRefresh,
  onOpenLocal,
  onCreate,
  onDraftChange,
  onChooseTargetFolder
}: AvailableSpacesViewProps) {
  const freshness = spaces[0]?.freshness;
  const freshnessMessage = spaces[0]?.message;
  return (
    <div className="desktop-view" id="desktop-available">
      <section className="desktop-panel desktop-panel--wide">
        <div className="desktop-heading-line">
          <div className="layrs-section-heading">
            <span>Distant</span>
            <h1>Server Spaces</h1>
          </div>
          <button type="button" className="desktop-secondary-button" onClick={onRefresh} disabled={!connected}>
            Refresh
          </button>
        </div>
        <div className="desktop-table desktop-table--spaces" role="table" aria-label="Available spaces">
          <div className="desktop-table__head" role="row">
            <span role="columnheader">Space</span>
            <span role="columnheader">Workspace</span>
            <span role="columnheader">Layers</span>
            <span role="columnheader">Local</span>
            <span role="columnheader">Target folder</span>
            <span role="columnheader">Action</span>
          </div>
          {spaces.map((space) => {
            const draft = createDrafts[space.spaceId] ?? defaultCreateDraft(space, defaultLocalRoot);
            const localSpace = space.localSpaces[0];
            return (
              <div className="desktop-table__row" role="row" key={space.spaceId}>
                <strong role="cell">{space.name}</strong>
                <span role="cell">{space.workspaceId}</span>
                <span role="cell">{space.layers.length}</span>
                <span role="cell">{localSpace ? <PathText value={localSpace.rootPath} /> : "Not on this machine"}</span>
                <span className="desktop-create-fields" role="cell">
                  {localSpace ? (
                    <em>Already local</em>
                  ) : (
                    <>
                      <FolderChoice
                        value={draft.targetFolder}
                        placeholder="Choose target folder"
                        onChoose={() => onChooseTargetFolder(space)}
                      />
                      <select
                        aria-label={`Initial Layer for ${space.name}`}
                        value={draft.layerId}
                        onChange={(event) => onDraftChange(space.spaceId, { ...draft, layerId: event.currentTarget.value })}
                      >
                        <option value="">Current Layer</option>
                        {space.layers.map((layer) => (
                          <option value={layer.layerId} key={layer.layerId}>
                            {layer.displayName}
                          </option>
                        ))}
                      </select>
                    </>
                  )}
                </span>
                <span className="desktop-button-row" role="cell">
                  {localSpace ? (
                    <button type="button" className="desktop-primary-button" onClick={() => onOpenLocal(localSpace.localSpaceId)}>
                      Open
                    </button>
                  ) : (
                    <button
                      type="button"
                      className="desktop-primary-button"
                      onClick={() => onCreate(space)}
                      disabled={!connected || busyAction === `create:${space.spaceId}`}
                    >
                      Pull to this machine
                    </button>
                  )}
                </span>
              </div>
            );
          })}
        </div>
        {spaces.length === 0 ? (
          <p className="desktop-empty">
            {connected
              ? "No distant Spaces are visible to the connected account. Create one in Studio Web, then Refresh."
              : "Connect a device in Settings to load distant Spaces."}
          </p>
        ) : null}
        {freshness ? <p className="desktop-footnote">Distant list: {freshness}{freshnessMessage ? ` - ${freshnessMessage}` : ""}</p> : null}
      </section>
    </div>
  );
}

interface DraftLocalSpaceViewProps {
  busyAction: string | null;
  draftSpaceFolder: string;
  draftSpaceName: string;
  initSpaceFolder: string;
  initSpaceName: string;
  onChooseDraftFolder: () => void;
  onChooseInitFolder: () => void;
  onCreateDraftSpace: () => void;
  onInitLocalSpace: () => void;
  onDraftSpaceNameChange: (value: string) => void;
  onInitSpaceNameChange: (value: string) => void;
}

function DraftLocalSpaceView({
  busyAction,
  draftSpaceFolder,
  draftSpaceName,
  initSpaceFolder,
  initSpaceName,
  onChooseDraftFolder,
  onChooseInitFolder,
  onCreateDraftSpace,
  onInitLocalSpace,
  onDraftSpaceNameChange,
  onInitSpaceNameChange
}: DraftLocalSpaceViewProps) {
  return (
    <div className="desktop-view" id="desktop-draft">
      <section className="desktop-panel desktop-panel--wide desktop-draft-page">
        <div className="desktop-heading-line">
          <div className="layrs-section-heading">
            <span>Offline Local Spaces</span>
            <h1>Local setup</h1>
          </div>
        </div>
        <div className="desktop-draft-grid">
          <div className="desktop-setting-card desktop-draft-form desktop-local-init-card">
            <span>Initialize existing folder</span>
            <strong>Use files that are already on this machine</strong>
            <label className="desktop-field">
              <span>Space name</span>
              <input value={initSpaceName} onChange={(event) => onInitSpaceNameChange(event.currentTarget.value)} placeholder="My existing project" />
            </label>
            <FolderField
              label="Existing folder"
              value={initSpaceFolder}
              placeholder="Choose the folder to initialize"
              onChoose={onChooseInitFolder}
            />
            <button
              type="button"
              className="desktop-primary-button"
              onClick={onInitLocalSpace}
              disabled={!initSpaceName.trim() || !initSpaceFolder.trim() || busyAction === "init-local"}
            >
              Initialize existing folder
            </button>
            <em>The folder becomes a Local Space, then opens on Changes so you can review and save Steps.</em>
          </div>
          <div className="desktop-setting-card desktop-draft-form desktop-local-init-card">
            <span>Create empty local Space</span>
            <strong>Start offline with an empty Main Layer</strong>
            <label className="desktop-field">
              <span>Space name</span>
              <input value={draftSpaceName} onChange={(event) => onDraftSpaceNameChange(event.currentTarget.value)} placeholder="New game space" />
            </label>
            <FolderField
              label="New folder"
              value={draftSpaceFolder}
              placeholder="Choose where to create this Local Space"
              onChoose={onChooseDraftFolder}
            />
            <button
              type="button"
              className="desktop-primary-button"
              onClick={onCreateDraftSpace}
              disabled={!draftSpaceName.trim() || !draftSpaceFolder.trim() || busyAction === "create-draft"}
            >
              Create empty local Space
            </button>
            <em>Stays local until you choose a Workspace and send it to Studio from the Local header.</em>
          </div>
          <div className="desktop-setting-card">
            <span>Publish path</span>
            <strong>Draft publishing stays in Local</strong>
            <em>Create empty local Space still opens as a draft. Select it under Local, choose a Workspace, then Send to Studio before regular Publish is enabled.</em>
          </div>
        </div>
      </section>
    </div>
  );
}

interface LocalSpacesViewProps {
  localSpaces: LocalSpaceSummary[];
  selectedSpace: LocalSpaceSummary | null;
  selectedLayer: LocalLayerSummary | null;
  files: LayerFile[];
  changes: LocalChange[];
  selectedDiff: LensDiffEntry | null;
  selectedDiffPath: string | null;
  selectedDiffContextKey: string;
  selectedScanRevision: number;
  timeline: TimelineItem[];
  workingTree?: WorkingTreeScan;
  workingTreeChangeCount: number;
  workspaces: BootstrapData["workspaces"];
  sendWorkspaceId: string;
  newLayerName: string;
  layerSearchQuery: string;
  pulseTargets: Set<PulseTarget>;
  activeTab: LocalSpaceTab;
  busyAction: string | null;
  commandErrors: Partial<Record<CommandKey, string>>;
  onSelectSpace: (localSpaceId: string) => void;
  onOpenSpace: (localSpaceId: string) => void;
  onForgetSpace: (localSpaceId: string) => void;
  onScan: (localSpaceId: string) => void;
  onSelectDiff: (path: string) => void;
  onLoadDiffWindow: (path: string, start: number, limit: number) => void;
  onSelectTimeline: (item: TimelineItem) => void;
  onSendWorkspaceChange: (value: string) => void;
  onSendDraft: () => void;
  onSelectLayer: (layerId: string) => void;
  onNewLayerNameChange: (value: string) => void;
  onLayerSearchChange: (value: string) => void;
  onTabChange: (tab: LocalSpaceTab) => void;
  onCreateLayer: () => void;
  onDeleteLayer: (layerId: string) => void;
  onReceive: () => void;
  onPublish: () => void;
}

function LocalSpacesView({
  localSpaces,
  selectedSpace,
  selectedLayer,
  files,
  changes,
  selectedDiff,
  selectedDiffPath,
  selectedDiffContextKey,
  selectedScanRevision,
  timeline,
  workingTree,
  workingTreeChangeCount,
  workspaces,
  sendWorkspaceId,
  newLayerName,
  layerSearchQuery,
  pulseTargets,
  activeTab,
  busyAction,
  commandErrors,
  onSelectSpace,
  onOpenSpace,
  onForgetSpace,
  onScan,
  onSelectDiff,
  onLoadDiffWindow,
  onSelectTimeline,
  onSendWorkspaceChange,
  onSendDraft,
  onSelectLayer,
  onNewLayerNameChange,
  onLayerSearchChange,
  onTabChange,
  onCreateLayer,
  onDeleteLayer,
  onReceive,
  onPublish
}: LocalSpacesViewProps) {
  const activeAccess = selectedLayer?.access ?? "open";
  const pulseClassName = [
    pulseTargets.has("changes") ? "is-pulsing-changes" : "",
    pulseTargets.has("layer") ? "is-pulsing-layer" : "",
    pulseTargets.has("steps") ? "is-pulsing-steps" : "",
    pulseTargets.has("sync") ? "is-pulsing-sync" : ""
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <div className={`desktop-view desktop-view--local ${pulseClassName}`} id="desktop-local">
      <section className="desktop-panel desktop-local-list">
        <div className="layrs-section-heading">
          <span>Local Spaces</span>
          <h2>This machine</h2>
        </div>
        <div className="desktop-stack">
          {localSpaces.map((space) => (
            <button
              type="button"
              className={selectedSpace?.localSpaceId === space.localSpaceId ? "desktop-space-card is-active" : "desktop-space-card"}
              key={space.localSpaceId}
              onClick={() => onSelectSpace(space.localSpaceId)}
            >
              <strong>
                {space.name}
                {space.state === "draft" ? <span className="desktop-inline-badge">Draft</span> : null}
              </strong>
              <PathText value={space.rootPath} />
              <em>{activeLayerCaption(space)}</em>
            </button>
          ))}
        </div>
        {localSpaces.length === 0 ? <p className="desktop-empty">No Local Spaces detected. Pull one from Distant or create one offline.</p> : null}
        {selectedSpace ? (
          <LayerRailPanel
            busyAction={busyAction}
            query={layerSearchQuery}
            selectedLayer={selectedLayer}
            selectedSpace={selectedSpace}
            workingTree={workingTree}
            onQueryChange={onLayerSearchChange}
            onSelectLayer={onSelectLayer}
          />
        ) : null}
      </section>

      <section className="desktop-panel desktop-space-detail">
        {selectedSpace ? (
          <>
            <div className="desktop-heading-line">
              <div className="layrs-section-heading">
                <span title={displayPath(selectedSpace.rootPath)}>{compactPath(selectedSpace.rootPath, 72)}</span>
                <h1>
                  {selectedSpace.name}
                  {selectedSpace.state === "draft" ? <span className="desktop-inline-badge">Local draft</span> : null}
                </h1>
              </div>
              <div className="desktop-actions">
                {selectedSpace.state === "draft" ? (
                  <>
                    <select
                      aria-label="Workspace target for Draft Local Space"
                      value={sendWorkspaceId}
                      onChange={(event) => onSendWorkspaceChange(event.currentTarget.value)}
                    >
                      <option value="">Choose Workspace</option>
                      {workspaces.map((workspace) => (
                        <option value={workspace.id} key={workspace.id}>
                          {workspace.name}
                        </option>
                      ))}
                    </select>
                    <button
                      type="button"
                      className="desktop-primary-button"
                      onClick={onSendDraft}
                      disabled={!sendWorkspaceId || busyAction === "send-draft" || Boolean(commandErrors["send-draft"])}
                    >
                      Send to Studio
                    </button>
                  </>
                ) : null}
                <button
                  type="button"
                  className="desktop-secondary-button"
                  onClick={onReceive}
                  disabled={selectedSpace.state === "draft" || busyAction === "receive" || Boolean(commandErrors.receive)}
                >
                  Receive
                </button>
                <button
                  type="button"
                  className="desktop-primary-button"
                  onClick={onPublish}
                  disabled={
                    (selectedSpace.state === "draft" && !sendWorkspaceId) ||
                    busyAction === "publish" ||
                    busyAction === "send-draft" ||
                    Boolean(commandErrors.publish) ||
                    Boolean(commandErrors["send-draft"])
                  }
                >
                  Publish
                </button>
                <button type="button" className="desktop-secondary-button" onClick={() => onOpenSpace(selectedSpace.localSpaceId)} disabled={Boolean(commandErrors.open)}>
                  Open folder
                </button>
                <button
                  type="button"
                  className="desktop-secondary-button"
                  onClick={() => onScan(selectedSpace.localSpaceId)}
                  disabled={busyAction === `scan:${selectedSpace.localSpaceId}` || Boolean(commandErrors.scan)}
                >
                  Scan
                </button>
                <details className="desktop-more-menu">
                  <summary>More</summary>
                  <button
                    type="button"
                    className="desktop-danger-button"
                    onClick={() => onForgetSpace(selectedSpace.localSpaceId)}
                    disabled={busyAction === "forget-local"}
                  >
                    Forget local
                  </button>
                </details>
              </div>
            </div>

            <Tabs
              activeId={activeTab}
              ariaLabel="Local Space sections"
              onChange={(nextTab) => onTabChange(nextTab as LocalSpaceTab)}
              tabs={[
                { id: "changes", label: "Changes", count: workingTreeChangeCount },
                { id: "files", label: "Files", count: files.length },
                { id: "steps", label: "Steps", count: timeline.filter((item) => item.kind === "step").length },
                { id: "layers", label: "Layers", count: selectedSpace.layers.length },
                { id: "sync", label: "Sync" },
                { id: "settings", label: "Settings" }
              ]}
            />

            {activeTab === "changes" ? (
              <div className="desktop-review-grid">
                <ChangesPanel
                  changes={changes}
                  selectedPath={selectedDiff?.path ?? selectedDiffPath}
                  onSelectDiff={onSelectDiff}
                />
              <LensDiffPanel
                isLoading={selectedDiff ? busyAction === `diff-window:${selectedDiff.path}` : false}
                onLoadWindow={onLoadDiffWindow}
                renderContextKey={selectedDiffContextKey}
                renderRevision={selectedScanRevision}
                selectedDiff={selectedDiff}
              />
              </div>
            ) : null}

            {activeTab === "files" ? <FilesPanel files={files} selectedLayerAccess={activeAccess} /> : null}

            {activeTab === "steps" ? (
              <div className="desktop-steps-grid">
                <TimelinePanel timeline={timeline} onSelectTimeline={onSelectTimeline} />
                <ChangesPanel changes={changes} selectedPath={selectedDiff?.path ?? selectedDiffPath} onSelectDiff={onSelectDiff} />
                <LensDiffPanel
                  isLoading={selectedDiff ? busyAction === `diff-window:${selectedDiff.path}` : false}
                  onLoadWindow={onLoadDiffWindow}
                  renderContextKey={selectedDiffContextKey}
                  renderRevision={selectedScanRevision}
                  selectedDiff={selectedDiff}
                />
              </div>
            ) : null}

            {activeTab === "layers" ? (
              <LayerManagementPanel
                selectedSpace={selectedSpace}
                selectedLayer={selectedLayer}
                workingTree={workingTree}
                query={layerSearchQuery}
                newLayerName={newLayerName}
                busyAction={busyAction}
                commandErrors={commandErrors}
                onQueryChange={onLayerSearchChange}
                onSelectLayer={onSelectLayer}
                onNewLayerNameChange={onNewLayerNameChange}
                onCreateLayer={onCreateLayer}
                onDeleteLayer={onDeleteLayer}
              />
            ) : null}

            {activeTab === "sync" ? (
              <SyncPanel selectedSpace={selectedSpace} selectedLayer={selectedLayer} changes={changes} commandErrors={commandErrors} />
            ) : null}

            {activeTab === "settings" ? (
              <LocalSpaceSettingsPanel selectedSpace={selectedSpace} selectedLayer={selectedLayer} onForgetSpace={onForgetSpace} busyAction={busyAction} />
            ) : null}
          </>
        ) : (
          <p className="desktop-empty">Select a Local Space to inspect Layers, files, local changes and timeline.</p>
        )}
      </section>
    </div>
  );
}

interface LayerManagementPanelProps {
  selectedSpace: LocalSpaceSummary;
  selectedLayer: LocalLayerSummary | null;
  workingTree?: WorkingTreeScan;
  query: string;
  newLayerName: string;
  busyAction: string | null;
  commandErrors: Partial<Record<CommandKey, string>>;
  onQueryChange: (value: string) => void;
  onSelectLayer: (layerId: string) => void;
  onNewLayerNameChange: (value: string) => void;
  onCreateLayer: () => void;
  onDeleteLayer: (layerId: string) => void;
}

interface LayerRailPanelProps {
  selectedSpace: LocalSpaceSummary;
  selectedLayer: LocalLayerSummary | null;
  workingTree?: WorkingTreeScan;
  query: string;
  busyAction: string | null;
  onQueryChange: (value: string) => void;
  onSelectLayer: (layerId: string) => void;
}

function LayerRailPanel({
  selectedSpace,
  selectedLayer,
  workingTree,
  query,
  busyAction,
  onQueryChange,
  onSelectLayer
}: LayerRailPanelProps) {
  const layers = layersByLatestStep(selectedSpace, workingTree, query);

  return (
    <div className="desktop-layer-card desktop-layer-card--rail">
      <div className="layrs-section-heading">
        <span>Layers</span>
        <h3>{selectedLayer?.displayName ?? "Switch Layer"}</h3>
      </div>
      <label className="desktop-field">
        <span>Search Layers</span>
        <input value={query} onChange={(event) => onQueryChange(event.currentTarget.value)} placeholder="Search by name, access, sync" />
      </label>
      <div className="desktop-layer-list desktop-layer-list--rail">
        {layers.map(({ layer, latestStepAt, stepCount }) => {
          const isActive = layer.layerId === selectedSpace.activeLayerId;
          return (
            <article className={isActive ? "desktop-layer-row is-active" : "desktop-layer-row"} key={layer.layerId}>
              <button
                type="button"
                className="desktop-layer-row__main"
                onClick={() => onSelectLayer(layer.layerId)}
                disabled={isActive || !layer.canOpen || busyAction === `switch:${layer.layerId}`}
              >
                <strong>{layer.displayName}</strong>
                <span>{layer.parentLayerId ? `Parent: ${layerDisplayName(selectedSpace, layer.parentLayerId)}` : "Base Layer"}</span>
                <span className="desktop-layer-row__latest">
                  {stepCount > 0 ? `Latest step ${formatUnixTime(latestStepAt)}` : "No local steps yet"}
                </span>
              </button>
              <div className="desktop-layer-row__rules">
                <StatusPill status={layer.access === "blocked" ? "blocked" : layer.access === "redacted" ? "needs-proof" : "passing"} label={layer.access} />
                <StatusPill status={layer.syncStatus === "local-only" ? "needs-proof" : "passing"} label={syncStatusLabel(layer.syncStatus)} />
              </div>
            </article>
          );
        })}
        {layers.length === 0 ? <p className="desktop-empty">No Layers match this search.</p> : null}
      </div>
    </div>
  );
}

function SyncPanel({
  changes,
  commandErrors,
  selectedLayer,
  selectedSpace
}: {
  changes: LocalChange[];
  commandErrors: Partial<Record<CommandKey, string>>;
  selectedLayer: LocalLayerSummary | null;
  selectedSpace: LocalSpaceSummary;
}) {
  return (
    <section className="desktop-subpanel">
      <div className="desktop-subheading">
        <strong>Sync status</strong>
        <span>{selectedSpace.state}</span>
      </div>
      <div className="desktop-settings-grid desktop-settings-grid--cards">
        <div className="desktop-setting-card">
          <span>Active Layer</span>
          <strong>{selectedLayer?.displayName ?? "No active Layer"}</strong>
          <em>{selectedLayer ? syncStatusLabel(selectedLayer.syncStatus) : "No sync state"}</em>
        </div>
        <div className="desktop-setting-card">
          <span>Pending changes</span>
          <strong>{changes.length}</strong>
          <em>Review in Changes before publishing.</em>
        </div>
        <div className="desktop-setting-card">
          <span>Receive</span>
          <strong>{commandErrors.receive ? "Blocked" : "Ready"}</strong>
          <em>{commandErrors.receive ?? "Manual receive keeps server data explicit."}</em>
        </div>
        <div className="desktop-setting-card">
          <span>Publish</span>
          <strong>{commandErrors.publish ? "Blocked" : "Ready"}</strong>
          <em>{commandErrors.publish ?? "Manual publish sends the active Layer state."}</em>
        </div>
      </div>
    </section>
  );
}

function LocalSpaceSettingsPanel({
  busyAction,
  onForgetSpace,
  selectedLayer,
  selectedSpace
}: {
  busyAction: string | null;
  onForgetSpace: (localSpaceId: string) => void;
  selectedLayer: LocalLayerSummary | null;
  selectedSpace: LocalSpaceSummary;
}) {
  return (
    <section className="desktop-subpanel">
      <div className="desktop-subheading">
        <strong>Local Space settings</strong>
        <span>{selectedSpace.state}</span>
      </div>
      <div className="desktop-settings-grid desktop-settings-grid--cards">
        <div className="desktop-setting-card">
          <span>Folder</span>
          <strong title={displayPath(selectedSpace.rootPath)}>{compactPath(selectedSpace.rootPath, 88)}</strong>
          <em>Project files stay on disk when a Local Space is forgotten.</em>
        </div>
        <div className="desktop-setting-card">
          <span>Layer</span>
          <strong>{selectedLayer?.displayName ?? "No active Layer"}</strong>
          <em>{selectedLayer?.access ?? "No access state"}</em>
        </div>
      </div>
      <div className="desktop-danger-stack">
        <DangerZone
          title="Forget this Local Space"
          description="Disconnects this folder from Layrs Desktop and archives .layrs metadata. Your project files remain in place."
        >
          <button
            type="button"
            className="desktop-danger-button"
            onClick={() => onForgetSpace(selectedSpace.localSpaceId)}
            disabled={busyAction === "forget-local"}
          >
            Forget local
          </button>
        </DangerZone>
      </div>
    </section>
  );
}

function LayerManagementPanel({
  selectedSpace,
  selectedLayer,
  workingTree,
  query,
  newLayerName,
  busyAction,
  commandErrors,
  onQueryChange,
  onSelectLayer,
  onNewLayerNameChange,
  onCreateLayer,
  onDeleteLayer
}: LayerManagementPanelProps) {
  const layers = layersByLatestStep(selectedSpace, workingTree, query);
  const activeAccess = selectedLayer?.access ?? "open";
  const activeSyncStatus = selectedLayer?.syncStatus ?? (selectedSpace.state === "draft" ? "local" : "linked");
  const parentLabel = selectedLayer?.parentLayerId ? layerDisplayName(selectedSpace, selectedLayer.parentLayerId) : "None";

  return (
    <section className="desktop-layer-card">
      <div className="layrs-section-heading">
        <span>Layers</span>
        <h3>{selectedLayer?.displayName ?? "No active Layer"}</h3>
      </div>
      <label className="desktop-field">
        <span>Search Layers</span>
        <input value={query} onChange={(event) => onQueryChange(event.currentTarget.value)} placeholder="Search by name, access, sync" />
      </label>
      <div className="desktop-layer-list">
        {layers.map(({ layer, latestStepAt, stepCount }) => {
          const isActive = layer.layerId === selectedSpace.activeLayerId;
          const canDelete = !isActive && selectedSpace.layers.length > 1;
          return (
            <article className={isActive ? "desktop-layer-row is-active" : "desktop-layer-row"} key={layer.layerId}>
              <button
                type="button"
                className="desktop-layer-row__main"
                onClick={() => onSelectLayer(layer.layerId)}
                disabled={isActive || !layer.canOpen || busyAction === `switch:${layer.layerId}`}
              >
                <strong>{layer.displayName}</strong>
                <span>{layer.parentLayerId ? `Parent: ${layerDisplayName(selectedSpace, layer.parentLayerId)}` : "Base Layer"}</span>
                <span className="desktop-layer-row__latest">
                  {stepCount > 0 ? `Latest step ${formatUnixTime(latestStepAt)}` : "No local steps yet"}
                </span>
              </button>
              <div className="desktop-layer-row__rules">
                <StatusPill status={layer.access === "blocked" ? "blocked" : layer.access === "redacted" ? "needs-proof" : "passing"} label={layer.access} />
                <StatusPill status={layer.syncStatus === "local-only" ? "needs-proof" : "passing"} label={syncStatusLabel(layer.syncStatus)} />
              </div>
              <button
                type="button"
                className="desktop-layer-delete"
                onClick={() => onDeleteLayer(layer.layerId)}
                disabled={!canDelete || busyAction === `delete-layer:${layer.layerId}`}
              >
                Delete
              </button>
            </article>
          );
        })}
        {layers.length === 0 ? <p className="desktop-empty">No Layers match this search.</p> : null}
      </div>
      <div className="desktop-layer-rules">
        <div>
          <span>Access</span>
          <strong>{activeAccess}</strong>
        </div>
        <div>
          <span>Sync</span>
          <strong>{syncStatusLabel(activeSyncStatus)}</strong>
        </div>
        <div>
          <span>Parent</span>
          <strong>{parentLabel}</strong>
        </div>
      </div>
      <div className="desktop-layer-create">
        <label className="desktop-field">
          <span>New Layer</span>
          <input value={newLayerName} onChange={(event) => onNewLayerNameChange(event.currentTarget.value)} placeholder="Layer name" />
        </label>
        <button
          type="button"
          className="desktop-secondary-button"
          onClick={onCreateLayer}
          disabled={!newLayerName.trim() || busyAction === "create-layer" || Boolean(commandErrors["create-layer"])}
        >
          Create from current
        </button>
      </div>
    </section>
  );
}

function FilesPanel({ files, selectedLayerAccess }: { files: LayerFile[]; selectedLayerAccess: LayerAccessKind }) {
  return (
    <section className="desktop-subpanel">
      <div className="desktop-subheading">
        <strong>Files in active Layer</strong>
        <span>{files.length}</span>
      </div>
      <div className="desktop-file-list">
        {files.map((file) => (
          <article className={file.redacted ? "desktop-file-row is-redacted" : "desktop-file-row"} key={file.path}>
            <div>
              <strong>{file.path}</strong>
              <span>
                {file.kind} - {file.sizeLabel}
              </span>
            </div>
            <em>{file.redacted ? "Restricted by Layer access policy" : file.lensId ?? file.state}</em>
            <button type="button" disabled={file.redacted || selectedLayerAccess === "blocked"} title={file.redacted ? "Restricted by Layer access policy" : undefined}>
              View
            </button>
          </article>
        ))}
        {files.length === 0 ? <p className="desktop-empty">Scan the Local Space to load files.</p> : null}
      </div>
    </section>
  );
}

function ChangesPanel({
  changes,
  selectedPath,
  onSelectDiff
}: {
  changes: LocalChange[];
  selectedPath: string | null;
  onSelectDiff: (path: string) => void;
}) {
  return (
    <section className="desktop-subpanel">
      <div className="desktop-subheading">
        <strong>Changes</strong>
        <span>{changes.length}</span>
      </div>
      <div className="desktop-change-list">
        {changes.map((change) => (
          <button
            type="button"
            className={
              selectedPath === change.path
                ? `desktop-change desktop-change--${change.state} is-active`
                : `desktop-change desktop-change--${change.state}`
            }
            key={`${change.state}-${change.path}`}
            onClick={() => onSelectDiff(change.path)}
          >
            <strong>{change.path}</strong>
            <span>{change.summary}</span>
            <em>{change.lensId}</em>
          </button>
        ))}
        {changes.length === 0 ? <p className="desktop-empty">No changes to show.</p> : null}
      </div>
    </section>
  );
}

function LensDiffPanel({
  isLoading,
  onLoadWindow,
  renderContextKey,
  renderRevision,
  selectedDiff
}: {
  isLoading: boolean;
  onLoadWindow: (path: string, start: number, limit: number) => void;
  renderContextKey: string;
  renderRevision: number;
  selectedDiff: LensDiffEntry | null;
}) {
  const windowState = selectedDiff ? diffWindowState(selectedDiff) : null;
  const lensSurfaceKey = selectedDiff
    ? `${renderContextKey}:${renderRevision}:${selectedDiff.path}:${selectedDiff.diff.kind}:${selectedDiff.diff.summary}:${selectedDiff.diff.hunks.length}:${windowState?.label ?? "full"}`
    : `empty:${renderContextKey}:${renderRevision}`;
  return (
    <section className="desktop-subpanel desktop-diff-panel">
      <div className="desktop-subheading">
        <strong>Lens diff</strong>
        <span>{selectedDiff?.lensId ?? "Lens"}</span>
      </div>
      <div className="desktop-diff-panel__meta">
        <strong>{selectedDiff?.path ?? "Select a local change"}</strong>
        <span>{selectedDiff ? selectedDiff.title : "No Lens diff selected"}</span>
      </div>
      <LensDiffHost
        key={lensSurfaceKey}
        className="desktop-shared-lens-diff"
        diff={selectedDiff?.diff ?? null}
        emptyMessage="Select a local change to inspect its Lens diff."
        title={selectedDiff ? selectedDiff.path : "Lens diff"}
      />
      {selectedDiff && windowState?.isWindowed ? (
        <div className="desktop-diff-window-controls">
          <span>{windowState.label}</span>
          <div>
            <button
              type="button"
              className="desktop-secondary-button"
              disabled={isLoading || !windowState.canLoadPrevious}
              onClick={() => onLoadWindow(selectedDiff.path, windowState.previousStart, windowState.limit)}
            >
              Load previous
            </button>
            <button
              type="button"
              className="desktop-secondary-button"
              disabled={isLoading || !windowState.canLoadNext}
              onClick={() => onLoadWindow(selectedDiff.path, windowState.nextStart, windowState.limit)}
            >
              Load next
            </button>
          </div>
        </div>
      ) : null}
      {selectedDiff?.message ? <p className="desktop-footnote">{selectedDiff.message}</p> : null}
    </section>
  );
}

function TimelinePanel({
  timeline,
  onSelectTimeline
}: {
  timeline: TimelineItem[];
  onSelectTimeline: (item: TimelineItem) => void;
}) {
  return (
    <section className="desktop-subpanel">
      <div className="desktop-subheading">
        <strong>Timeline</strong>
        <span>{timeline.length}</span>
      </div>
      <div className="desktop-timeline">
        {timeline.map((item) => (
          <button
            type="button"
            className={item.isActive ? "desktop-timeline-row is-active" : "desktop-timeline-row"}
            key={item.id}
            onClick={() => onSelectTimeline(item)}
          >
            <time>{item.at}</time>
            <div>
              <strong>{item.title}</strong>
              <span>{item.actor}</span>
              <p>{item.summary}</p>
              {item.diffStats ? (
                <em>
                  {item.diffStats.files} files, +{item.diffStats.additions}, -{item.diffStats.deletions}
                </em>
              ) : null}
            </div>
          </button>
        ))}
      </div>
    </section>
  );
}

interface SettingsViewProps {
  status: DesktopStatus | null;
  bootstrap: BootstrapData | null;
  endpointDraft: string;
  defaultLocalRoot: string;
  login: DeviceLoginStartResponse | null;
  pollStatus: string | null;
  pollInFlight: boolean;
  autoReceive: boolean;
  autoPublish: boolean;
  autoLocalSteps: boolean;
  syncIntervalMinutes: number;
  shortcuts: DesktopShortcutSettings;
  loadState: LoadState;
  saving: boolean;
  onEndpointChange: (value: string) => void;
  onChooseDefaultRoot: () => void;
  onSaveSettings: () => void;
  onBeginLogin: () => void;
  onPollNow: () => void;
  onAutoReceiveChange: (value: boolean) => void;
  onAutoPublishChange: (value: boolean) => void;
  onAutoLocalStepsChange: (value: boolean) => void;
  onSyncIntervalChange: (value: number) => void;
  onShortcutsChange: (value: DesktopShortcutSettings) => void;
}

function SettingsView({
  status,
  bootstrap,
  endpointDraft,
  defaultLocalRoot,
  login,
  pollStatus,
  pollInFlight,
  autoReceive,
  autoPublish,
  autoLocalSteps,
  syncIntervalMinutes,
  shortcuts,
  loadState,
  saving,
  onEndpointChange,
  onChooseDefaultRoot,
  onSaveSettings,
  onBeginLogin,
  onPollNow,
  onAutoReceiveChange,
  onAutoPublishChange,
  onAutoLocalStepsChange,
  onSyncIntervalChange,
  onShortcutsChange
}: SettingsViewProps) {
  return (
    <div className="desktop-view" id="desktop-settings">
      <section className="desktop-panel desktop-panel--wide">
        <div className="desktop-heading-line">
          <div className="layrs-section-heading">
            <span>Settings</span>
            <h1>Desktop sync and device</h1>
          </div>
          <StatusPill status={status?.secretStore.available ? "passing" : "blocked"} label={status?.secretStore.available ? "Secret store ready" : "Secret store required"} />
        </div>

        <div className="desktop-settings-grid desktop-settings-grid--cards">
          <section className="desktop-setting-card">
            <span>Account</span>
            <strong>{bootstrap?.account?.displayName ?? (status?.connected ? "Connected device" : "Not connected")}</strong>
            <em>{bootstrap?.account?.email ?? (status?.connected ? "Refresh Distant to load account details" : "Device login required")}</em>
            <button type="button" className="desktop-primary-button" onClick={onBeginLogin} disabled={!status?.secretStore.available || pollInFlight}>
              {status?.connected ? "Reconnect device" : "Connect device"}
            </button>
            {login ? (
              <div className="desktop-code">
                <div>
                  <span>User code</span>
                  <strong>{login.userCode}</strong>
                </div>
                <a href={login.verificationUriComplete ?? login.verificationUri} target="_blank" rel="noreferrer">
                  Open verification page
                </a>
                <button type="button" onClick={onPollNow} disabled={pollInFlight}>
                  {pollInFlight ? "Checking..." : "Check now"}
                </button>
                <p>{statusLabels[pollStatus ?? "pending"] ?? pollStatus}</p>
              </div>
            ) : null}
          </section>

          <section className="desktop-setting-card">
            <span>Server</span>
            <label className="desktop-field">
              <span>Endpoint</span>
              <input value={endpointDraft} onChange={(event) => onEndpointChange(event.currentTarget.value)} />
            </label>
            <button type="button" className="desktop-primary-button desktop-save-button" onClick={onSaveSettings} disabled={loadState !== "ready" || saving}>
              Save settings
            </button>
          </section>

          <section className="desktop-setting-card">
            <span>Sync</span>
            <ToggleRow label="Auto receive" checked={autoReceive} onChange={onAutoReceiveChange} />
            <ToggleRow label="Auto publish" checked={autoPublish} onChange={onAutoPublishChange} />
            <ToggleRow label="Auto local steps" checked={autoLocalSteps} onChange={onAutoLocalStepsChange} />
            <label className="desktop-field">
              <span>Sync interval</span>
              <input
                type="number"
                min={1}
                max={1440}
                value={syncIntervalMinutes}
                onChange={(event) => onSyncIntervalChange(Number(event.currentTarget.value))}
              />
            </label>
          </section>

          <section className="desktop-setting-card">
            <span>Shortcuts</span>
            <ToggleRow
              label="Enable keyboard shortcuts"
              checked={shortcuts.enabled}
              onChange={(enabled) => onShortcutsChange({ ...shortcuts, enabled })}
            />
            <ShortcutCaptureField
              label="Save Step"
              value={shortcuts.saveStep}
              onChange={(saveStep) => onShortcutsChange({ ...shortcuts, saveStep })}
            />
            <ShortcutCaptureField
              label="Publish"
              value={shortcuts.publish}
              onChange={(publish) => onShortcutsChange({ ...shortcuts, publish })}
            />
            <ToggleRow
              label="Use Save Step again to publish pending step"
              checked={shortcuts.smartSavePublishesPendingStep}
              onChange={(smartSavePublishesPendingStep) => onShortcutsChange({ ...shortcuts, smartSavePublishesPendingStep })}
            />
            <button type="button" className="desktop-secondary-button" onClick={() => onShortcutsChange(defaultShortcuts)}>
              Reset defaults
            </button>
          </section>

          <section className="desktop-setting-card">
            <span>Storage</span>
            <FolderField
              label="Default Local Spaces folder"
              value={defaultLocalRoot}
              placeholder="Choose the default folder for new Local Spaces"
              onChoose={onChooseDefaultRoot}
              wide
            />
            <div className="desktop-device-grid">
              <div>
                <span>Device id</span>
                <strong>{status?.deviceId ?? "Not initialized"}</strong>
                <em>{status?.secretStore.provider ?? "Unknown provider"}</em>
              </div>
              <p>{status?.secretStore.message ?? "Desktop status is not loaded yet."}</p>
            </div>
          </section>
        </div>
      </section>
    </div>
  );
}

function ToggleRow({ label, checked, onChange }: { label: string; checked: boolean; onChange: (value: boolean) => void }) {
  return (
    <label className="desktop-toggle">
      <span>{label}</span>
      <input type="checkbox" checked={checked} onChange={(event) => onChange(event.currentTarget.checked)} />
    </label>
  );
}

function ShortcutCaptureField({
  label,
  onChange,
  value
}: {
  label: string;
  onChange: (value: string) => void;
  value: string;
}) {
  return (
    <label className="desktop-field">
      <span>{label}</span>
      <input
        value={value}
        onChange={(event) => onChange(normalizeShortcut(event.currentTarget.value))}
        onKeyDown={(event) => {
          if (event.key === "Tab") {
            return;
          }
          event.preventDefault();
          const shortcut = shortcutFromKeyboardEvent(event);
          if (shortcut) {
            onChange(shortcut);
          }
        }}
        placeholder="Press shortcut"
      />
    </label>
  );
}

function ShortcutFooter({ hasLocalSpace, shortcuts }: { hasLocalSpace: boolean; shortcuts: DesktopShortcutSettings }) {
  if (!shortcuts.enabled) {
    return <span className="desktop-shortcut-footer is-muted">Keyboard shortcuts disabled</span>;
  }

  return (
    <div className={hasLocalSpace ? "desktop-shortcut-footer" : "desktop-shortcut-footer is-muted"}>
      <span>
        <kbd>{shortcuts.saveStep}</kbd> Step
      </span>
      {shortcuts.smartSavePublishesPendingStep ? (
        <span>
          <kbd>{shortcuts.saveStep}</kbd> again Publish
        </span>
      ) : null}
      <span>
        <kbd>{shortcuts.publish}</kbd> Publish
      </span>
    </div>
  );
}

function CommandErrors({ errors }: { errors: Partial<Record<CommandKey, string>> }) {
  const entries = Object.entries(errors).filter(([, value]) => value);
  if (entries.length === 0) {
    return null;
  }

  return (
    <div className="desktop-command-errors">
      {entries.map(([key, value]) => (
        <p className="desktop-alert desktop-alert--error" key={key}>
          {key}: {value}
        </p>
      ))}
    </div>
  );
}

function FolderField({
  label,
  value,
  placeholder,
  wide = false,
  onChoose
}: {
  label: string;
  value: string;
  placeholder: string;
  wide?: boolean;
  onChoose: () => void;
}) {
  return (
    <div className={wide ? "desktop-field desktop-field--wide" : "desktop-field"}>
      <span>{label}</span>
      <FolderChoice value={value} placeholder={placeholder} onChoose={onChoose} />
    </div>
  );
}

function FolderChoice({ value, placeholder, onChoose }: { value: string; placeholder: string; onChoose: () => void }) {
  const normalized = value ? displayPath(value) : "";
  return (
    <div className="desktop-folder-choice">
      <span className={normalized ? "desktop-folder-choice__path" : "desktop-folder-choice__path is-empty"} title={normalized || placeholder}>
        {normalized ? compactPath(normalized, 58) : placeholder}
      </span>
      <button type="button" className="desktop-secondary-button" onClick={onChoose}>
        Choose folder
      </button>
    </div>
  );
}

function PathText({ value }: { value: string }) {
  const normalized = displayPath(value);
  return (
    <span className="desktop-path-text" title={normalized}>
      {compactPath(normalized, 48)}
    </span>
  );
}

function buildLayerFiles(scan: WorkingTreeScan | undefined, layer: LocalLayerSummary | null): LayerFile[] {
  const access = layer?.access ?? "open";
  if (access === "blocked" || access === "redacted") {
    return [
      {
        path: layer?.path ?? "layer contents",
        kind: "Document",
        state: "redacted",
        sizeLabel: "restricted",
        redacted: true
      }
    ];
  }

  if (!scan) {
    return [];
  }

  const changed = new Map<string, FileState>();
  for (const path of scan.added) {
    changed.set(path, "added");
  }
  for (const path of scan.modified) {
    changed.set(path, "modified");
  }
  for (const path of scan.deleted) {
    changed.set(path, "deleted");
  }

  const currentFiles = scan.files.map((file) => ({
    path: file.path,
    kind: fileKind(file.path),
    state: changed.get(file.path) ?? "clean",
    lensId: lensForPath(file.path),
    sizeLabel: formatBytes(file.size)
  }));

  const deletedFiles = scan.deleted
    .filter((path) => !scan.files.some((file) => file.path === path))
    .map((path) => ({
      path,
      kind: fileKind(path),
      state: "deleted" as FileState,
      lensId: lensForPath(path),
      sizeLabel: "deleted"
    }));

  return [...currentFiles, ...deletedFiles];
}

function buildChanges(scan: WorkingTreeScan | undefined, selectedStepId: string | null): LocalChange[] {
  if (!scan) {
    return [];
  }

  const selectedStep = selectedStepId ? (scan.steps ?? []).find((step) => step.stepId === selectedStepId) : undefined;
  const diffs = selectedStep?.diffs ?? scan.diffs;

  return diffs.map((diff) => ({
    path: diff.path,
    state: normalizeChangeState(diff.state),
    summary: diff.diff.summary || diff.title,
    lensId: diff.lensId,
    diff
  }));
}

interface LayerWithStepActivity {
  layer: LocalLayerSummary;
  latestStepAt: number;
  stepCount: number;
}

function layersByLatestStep(
  space: LocalSpaceSummary,
  scan: WorkingTreeScan | undefined,
  query: string
): LayerWithStepActivity[] {
  const normalizedQuery = query.trim().toLowerCase();
  const activity = new Map<string, { latestStepAt: number; stepCount: number }>();

  for (const layerActivity of scan?.layerActivities ?? []) {
    activity.set(layerActivity.layerId, {
      latestStepAt: layerActivity.latestStepAt,
      stepCount: layerActivity.stepCount
    });
  }

  if (!scan?.layerActivities?.length) {
    for (const step of scan?.steps ?? []) {
      const current = activity.get(step.layerId) ?? { latestStepAt: 0, stepCount: 0 };
      activity.set(step.layerId, {
        latestStepAt: Math.max(current.latestStepAt, step.capturedAt),
        stepCount: current.stepCount + 1
      });
    }
  }

  return space.layers
    .filter((layer) => {
      if (!normalizedQuery) {
        return true;
      }
      return (
        layer.displayName.toLowerCase().includes(normalizedQuery) ||
        layer.layerId.toLowerCase().includes(normalizedQuery) ||
        layer.syncStatus.toLowerCase().includes(normalizedQuery) ||
        layer.access.toLowerCase().includes(normalizedQuery)
      );
    })
    .map((layer) => ({
      layer,
      latestStepAt: activity.get(layer.layerId)?.latestStepAt ?? 0,
      stepCount: activity.get(layer.layerId)?.stepCount ?? 0
    }))
    .sort((left, right) => {
      if (left.latestStepAt !== right.latestStepAt) {
        return right.latestStepAt - left.latestStepAt;
      }
      if (left.layer.layerId === space.activeLayerId && right.layer.layerId !== space.activeLayerId) {
        return -1;
      }
      if (right.layer.layerId === space.activeLayerId && left.layer.layerId !== space.activeLayerId) {
        return 1;
      }
      return left.layer.displayName.localeCompare(right.layer.displayName, undefined, { sensitivity: "base" });
    });
}

function buildTimeline(
  space: LocalSpaceSummary | null,
  scan: WorkingTreeScan | undefined,
  selectedStepId: string | null
): TimelineItem[] {
  if (!space) {
    return [];
  }

  const changedCount = scan ? scan.added.length + scan.modified.length + scan.deleted.length : 0;
  const scanStats = scan
    ? diffStatsFromDiffs(scan.diffs)
    : undefined;
  const base: TimelineItem[] = [
    {
      id: "working-tree",
      kind: "scan",
      title: "Working tree",
      actor: displayPath(space.rootPath),
      at: scan ? "Latest" : "Pending",
      summary: scan ? `${changedCount} local changes before publication.` : "Run Scan to load files and local changes.",
      isActive: selectedStepId === null,
      diffStats: scanStats
    }
  ];

  const steps = (scan?.steps ?? [])
    .slice()
    .reverse()
    .map((step) => timelineItemFromStep(step, selectedStepId, layerDisplayName(space, step.layerId)));

  return [...base, ...steps];
}

function timelineItemFromStep(step: LocalStepSummary, selectedStepId: string | null, layerName: string): TimelineItem {
  return {
    id: step.stepId,
    kind: "step",
    title: `Step ${shortId(step.stepId)}`,
    actor: layerName,
    at: formatUnixTime(step.capturedAt),
    summary: `${step.changedFiles} changed files captured in this Layer step.`,
    isActive: selectedStepId === step.stepId,
    diffStats: step.diffStats
  };
}

function normalizeChangeState(state: string): ChangeState {
  return state === "added" || state === "deleted" ? state : "modified";
}

function diffStatsFromDiffs(diffs: LensDiffEntry[]): LocalDiffStats {
  return diffs.reduce<LocalDiffStats>(
    (stats, diff) => {
      stats.files += 1;
      for (const hunk of diff.diff.hunks) {
        for (const line of hunk.lines) {
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

function diffWindowKey(localSpaceId: string | undefined, selectedStepId: string | null, path: string | undefined): string {
  return localSpaceId && path ? `${localSpaceId}::${selectedStepId ?? "workingTree"}::${path}` : "";
}

interface DesktopDiffWindowState {
  canLoadNext: boolean;
  canLoadPrevious: boolean;
  isWindowed: boolean;
  label: string;
  limit: number;
  nextStart: number;
  previousStart: number;
}

function diffWindowState(entry: LensDiffEntry): DesktopDiffWindowState {
  const fields = entry.diff.fields ?? {};
  const metadata = entry.diff.metadata;
  const metadataWindow = metadata?.lineWindow;
  const windowStart =
    numberField(fields, "windowStart") ??
    numberField(fields, "start") ??
    (metadataWindow?.startLine ? metadataWindow.startLine - 1 : 0);
  const windowLimit =
    numberField(fields, "windowLimit") ??
    numberField(fields, "limit") ??
    metadata?.renderedLineCount ??
    entry.diff.hunks.reduce((count, hunk) => count + hunk.lines.length, 0);
  const windowEnd =
    numberField(fields, "windowEnd") ??
    numberField(fields, "end") ??
    (metadataWindow?.endLine ? metadataWindow.endLine : windowStart + windowLimit);
  const totalLines =
    numberField(fields, "totalDiffLines") ??
    numberField(fields, "totalLineCount") ??
    metadata?.totalLineCount ??
    windowEnd;
  const hasMore =
    booleanField(fields, "hasMore") ??
    metadata?.hasMoreAfter ??
    windowEnd < totalLines;
  const hasMoreBefore =
    booleanField(fields, "hasMoreBefore") ??
    metadata?.hasMoreBefore ??
    windowStart > 0;
  const isWindowed =
    Boolean(booleanField(fields, "largeDiff")) ||
    hasMore ||
    hasMoreBefore ||
    totalLines > windowLimit;
  const safeLimit = Math.max(1, windowLimit || 400);
  const previousStart = Math.max(0, windowStart - safeLimit);
  const nextStart = Math.min(Math.max(0, totalLines - 1), windowStart + safeLimit);
  const displayStart = totalLines === 0 ? 0 : windowStart + 1;
  const displayEnd = Math.min(windowEnd, totalLines);

  return {
    canLoadNext: hasMore && nextStart !== windowStart,
    canLoadPrevious: hasMoreBefore && previousStart !== windowStart,
    isWindowed,
    label: `Diff lines ${displayStart}-${displayEnd} of ${totalLines}`,
    limit: safeLimit,
    nextStart,
    previousStart
  };
}

function numberField(fields: Record<string, unknown>, key: string): number | undefined {
  const value = fields[key];
  if (typeof value === "number" && Number.isFinite(value)) {
    return value;
  }
  if (typeof value === "string") {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : undefined;
  }
  return undefined;
}

function booleanField(fields: Record<string, unknown>, key: string): boolean | undefined {
  const value = fields[key];
  if (typeof value === "boolean") {
    return value;
  }
  if (typeof value === "string") {
    if (value === "true") {
      return true;
    }
    if (value === "false") {
      return false;
    }
  }
  return undefined;
}

function formatUnixTime(value: number): string {
  if (!Number.isFinite(value) || value <= 0) {
    return "Unknown";
  }
  return new Date(value * 1000).toLocaleString(undefined, {
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit"
  });
}

function shortId(value: string): string {
  return value.length > 14 ? value.slice(0, 14) : value;
}

function activeLayerLabel(space: LocalSpaceSummary): string {
  const activeLayer = space.layers.find((layer) => layer.layerId === space.activeLayerId);
  return activeLayer?.displayName ?? space.activeLayerId ?? "No active Layer";
}

function layerDisplayName(space: LocalSpaceSummary, layerId: string): string {
  return space.layers.find((layer) => layer.layerId === layerId)?.displayName ?? shortId(layerId);
}

function syncStatusLabel(status: string): string {
  if (status === "local-only") {
    return "Local only";
  }
  if (status === "local") {
    return "Local draft";
  }
  if (status === "linked") {
    return "Linked";
  }
  return status;
}

function activeLayerCaption(space: LocalSpaceSummary): string {
  return space.activeLayerId ? `Layer: ${activeLayerLabel(space)}` : activeLayerLabel(space);
}

function displayPath(value: string): string {
  return value.replace(/^\\\\\?\\UNC\\/i, "\\\\").replace(/^\\\\\?\\/, "");
}

function compactPath(value: string, maxLength: number): string {
  const normalized = displayPath(value);
  if (normalized.length <= maxLength) {
    return normalized;
  }

  const separator = normalized.includes("/") && !normalized.includes("\\") ? "/" : "\\";
  const parts = normalized.split(/[\\/]+/).filter(Boolean);
  if (parts.length <= 2) {
    return normalized.length <= maxLength ? normalized : `...${normalized.slice(-(maxLength - 3))}`;
  }

  const prefix = /^[A-Za-z]:/.test(normalized) ? normalized.slice(0, 2) : normalized.startsWith("\\\\") ? "\\\\" : "";
  const tailCount = maxLength < 42 ? 1 : 2;
  const tail = parts.slice(-tailCount).join(separator);
  const compacted = prefix ? `${prefix}${separator}...${separator}${tail}` : `...${separator}${tail}`;

  if (compacted.length <= maxLength) {
    return compacted;
  }
  return `...${normalized.slice(-(maxLength - 3))}`;
}

function defaultCreateDraft(space: AvailableSpaceView, defaultLocalRoot: string): CreateDraft {
  const root = trimPath(defaultLocalRoot || ".");
  return {
    targetFolder: `${root}\\${slug(space.name)}`,
    layerId: space.currentLayerId ?? ""
  };
}

function fileKind(path: string): LayerFile["kind"] {
  const lower = path.toLowerCase();
  if (/\.(ts|tsx|js|jsx|rs|css|json|toml|yaml|yml)$/.test(lower)) {
    return "Code";
  }
  if (/\.(png|jpg|jpeg|gif|webp|svg)$/.test(lower)) {
    return "Image";
  }
  if (/\.(csv|parquet|db|sqlite)$/.test(lower)) {
    return "Data";
  }
  return "Document";
}

function lensForPath(path: string) {
  const kind = fileKind(path);
  if (kind === "Image") {
    return "layrs.image";
  }
  if (kind === "Code") {
    return "layrs.code";
  }
  return kind === "Document" ? "layrs.text" : "layrs.raw";
}

function formatBytes(size: number) {
  if (size < 1024) {
    return `${size} B`;
  }
  if (size < 1024 * 1024) {
    return `${Math.round(size / 1024)} KB`;
  }
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}

function pageFromHash(hash: string): DesktopPage {
  if (hash.includes("desktop-local")) {
    return "local";
  }
  if (hash.includes("desktop-draft")) {
    return "draft";
  }
  if (hash.includes("desktop-settings")) {
    return "settings";
  }
  return "available";
}

function pageTitle(page: DesktopPage, selectedSpace: LocalSpaceSummary | null) {
  if (page === "local") {
    return selectedSpace?.name ?? "Local Spaces";
  }
  if (page === "settings") {
    return "Settings";
  }
  if (page === "draft") {
    return "Local setup";
  }
  return "Distant Spaces";
}

function isEditableShortcutTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  const tagName = target.tagName.toLowerCase();
  return tagName === "input" || tagName === "textarea" || tagName === "select" || target.isContentEditable;
}

function shortcutFromKeyboardEvent(event: KeyboardEvent | React.KeyboardEvent): string {
  const key = event.key;
  if (!key || ["Control", "Shift", "Alt", "Meta"].includes(key)) {
    return "";
  }
  const parts: string[] = [];
  if (event.ctrlKey || event.metaKey) {
    parts.push("Ctrl");
  }
  if (event.shiftKey) {
    parts.push("Shift");
  }
  if (event.altKey) {
    parts.push("Alt");
  }
  parts.push(normalizeShortcutKey(key));
  return parts.join("+");
}

function normalizeShortcutKey(key: string): string {
  if (key.length === 1) {
    return key.toUpperCase();
  }
  if (key === " ") {
    return "Space";
  }
  return key
    .replace(/^Arrow/, "")
    .replace("Escape", "Esc")
    .replace("Delete", "Del");
}

function normalizeShortcut(value: string): string {
  return value
    .split("+")
    .map((part) => part.trim())
    .filter(Boolean)
    .map((part) => {
      const lower = part.toLowerCase();
      if (lower === "control" || lower === "cmd" || lower === "meta" || lower === "ctrl") {
        return "Ctrl";
      }
      if (lower === "shift") {
        return "Shift";
      }
      if (lower === "alt" || lower === "option") {
        return "Alt";
      }
      return normalizeShortcutKey(part);
    })
    .join("+");
}

function shortcutMatches(pressed: string, configured: string): boolean {
  return normalizeShortcut(pressed) === normalizeShortcut(configured);
}

function validateShortcuts(shortcuts: DesktopShortcutSettings): string | null {
  if (!shortcuts.enabled) {
    return null;
  }
  const saveStep = normalizeShortcut(shortcuts.saveStep);
  const publish = normalizeShortcut(shortcuts.publish);
  if (!saveStep || !publish) {
    return "Shortcut fields cannot be empty while shortcuts are enabled.";
  }
  if (saveStep === publish) {
    return "Save Step and Publish shortcuts must be different.";
  }
  return null;
}

function trimPath(value: string) {
  return value.replace(/[\\/]+$/, "");
}

function nameFromFolder(value: string) {
  const normalized = trimPath(displayPath(value));
  const parts = normalized.split(/[\\/]+/).filter(Boolean);
  return parts[parts.length - 1] ?? "";
}

function slug(value: string) {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/(^-|-$)/g, "");
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function isConsumedDeviceCodeError(error: unknown): boolean {
  const message = errorMessage(error).toLowerCase();
  return message.includes("already consumed") || message.includes("device code was already consumed");
}
