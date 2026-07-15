import { AppShell, StatusPill, useNotifications } from "@layrs/ui";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { DesktopConfirmations } from "./DesktopConfirmations";
import { CommandErrors, SettingsView } from "./DesktopSettingsView";
import { DesktopSidebar } from "./DesktopSidebar";
import { AvailableSpacesView, DraftLocalSpaceView, LocalSpacesView, SpaceSettingsView, SpaceWeavesView } from "./DesktopViews";
import {
  buildChanges,
  buildLayerFiles,
  buildTimeline,
  defaultCreateDraft,
  diffWindowKey,
  errorMessage,
  isConsumedDeviceCodeError,
  isEditableShortcutTarget,
  nameFromFolder,
  pageFromHash,
  pageTitle,
  shortcutFromKeyboardEvent,
  shortcutMatches,
  validateShortcuts
} from "./desktopModel";
import {
  defaultShortcuts,
  FOCUS_SCAN_THROTTLE_MS,
  statusLabels,
  type CommandKey,
  type CreateDraft,
  type DesktopPage,
  type LoadState,
  type LocalSpaceTab,
  type PulseTarget,
  type TimelineItem
} from "./desktopTypes";
import { DesktopOperationController, isDesktopOperationCancelled } from "./desktopOperationQueue";
import { isLayerRestricted, purgeRestrictedDiffWindows, purgeRestrictedWeaves, purgeRestrictedWorkingTrees, sanitizeWorkingTreeForAccess } from "./desktopPrivateState";
import { resetDesktopPrivateRequests } from "./desktopPrivateApi";
import { useDesktopAuthPurge } from "./useDesktopAuthPurge";
import { useDesktopDiffWindow } from "./useDesktopDiffWindow";
import { useDesktopLocalActions } from "./useDesktopLocalActions";
import { useDesktopSyncActions } from "./useDesktopSyncActions";
import {
  AvailableSpaceView,
  BootstrapData,
  DesktopSettings,
  DesktopShortcutSettings,
  DesktopStatus,
  DeviceLoginPollResponse,
  DeviceLoginStartResponse,
  getDesktopStatus,
  LensDiffEntry,
  listAvailableSpaces,
  listLocalSpaces,
  loadDesktopSettings,
  LocalSpaceSummary,
  pollDeviceLogin,
  refreshBootstrap,
  saveDesktopSettings,
  scanWorkingTree,
  selectFolder,
  startDeviceLogin,
  weaveStatus,
  WeaveSessionSummary,
  WorkingTreeScan
} from "./tauri";
export function DesktopApp() {
  const { notify } = useNotifications();
  const [loadState, setLoadState] = useState<LoadState>("loading");
  const [page, setPage] = useState<DesktopPage>(() => pageFromHash(window.location.hash));
  const [status, setStatus] = useState<DesktopStatus | null>(null);
  const [bootstrap, setBootstrap] = useState<BootstrapData | null>(null);
  const [availableSpaces, setAvailableSpaces] = useState<AvailableSpaceView[]>([]);
  const [localSpaces, setLocalSpaces] = useState<LocalSpaceSummary[]>([]);
  const [workingTrees, setWorkingTrees] = useState<Record<string, WorkingTreeScan>>({});
  const [weaveSessions, setWeaveSessions] = useState<Record<string, WeaveSessionSummary | null>>({});
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
  const [disconnectLayerTargetId, setDisconnectLayerTargetId] = useState<string | null>(null);
  const [clearStepsTargetId, setClearStepsTargetId] = useState<string | null>(null);
  const [confirmWeaveParent, setConfirmWeaveParent] = useState(false);
  const [diffWindowOverrides, setDiffWindowOverrides] = useState<Record<string, LensDiffEntry>>({});
  const [createDrafts, setCreateDrafts] = useState<Record<string, CreateDraft>>({});
  const [draftSpaceName, setDraftSpaceName] = useState("");
  const [draftSpaceFolder, setDraftSpaceFolder] = useState("");
  const [initSpaceName, setInitSpaceName] = useState("");
  const [initSpaceFolder, setInitSpaceFolder] = useState("");
  const [sendWorkspaceId, setSendWorkspaceId] = useState("");
  const [layerSearchQuery, setLayerSearchQuery] = useState("");
  const [weaveSourceLayerId, setWeaveSourceLayerId] = useState("");
  const [weaveTargetLayerId, setWeaveTargetLayerId] = useState("");
  const [autoReceive, setAutoReceive] = useState(false);
  const [autoPublish, setAutoPublish] = useState(false);
  const [autoLocalSteps, setAutoLocalSteps] = useState(true);
  const [syncIntervalMinutes, setSyncIntervalMinutes] = useState(15);
  const [shortcuts, setShortcuts] = useState<DesktopShortcutSettings>(defaultShortcuts);
  const [pulseTargets, setPulseTargets] = useState<Set<PulseTarget>>(new Set());
  const [commandErrors, setCommandErrors] = useState<Partial<Record<CommandKey, string>>>({});
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const operations = useMemo(() => new DesktopOperationController(setBusyAction), []);
  const localSpacesRef = useRef<LocalSpaceSummary[]>([]);
  const pollInFlightRef = useRef(false);
  const lastFocusScanRef = useRef<{ localSpaceId: string; at: number }>({ localSpaceId: "", at: 0 });
  const pulseTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const purgeEphemeralPrivateState = useCallback(() => {
    localSpacesRef.current = []; setCreateDrafts({}); setSendWorkspaceId("");
    setWeaveSourceLayerId(""); setWeaveTargetLayerId(""); setConfirmWeaveParent(false);
    setForgetTargetId(null); setDeleteLayerTargetId(null); setDisconnectLayerTargetId(null); setClearStepsTargetId(null);
  }, []);
  useDesktopAuthPurge({
    onPurgePrivateState: purgeEphemeralPrivateState,
    operations,
    setAvailableSpaces,
    setBootstrap,
    setCommandErrors,
    setDiffWindowOverrides,
    setError,
    setLoadState,
    setLocalSpaces,
    setPage,
    setScanRevisions,
    setSelectedDiffPath,
    setSelectedLocalSpaceId,
    setSelectedStepId,
    setStatus,
    setWeaveSessions,
    setWorkingTrees
  });
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
  const applyLocalSpaces = useCallback((nextSpaces: LocalSpaceSummary[]) => {
    localSpacesRef.current = nextSpaces;
    setLocalSpaces(nextSpaces);
    setWorkingTrees((current) => purgeRestrictedWorkingTrees(nextSpaces, current));
    setWeaveSessions((current) => purgeRestrictedWeaves(nextSpaces, current));
    setDiffWindowOverrides((current) => purgeRestrictedDiffWindows(nextSpaces, current));
    if (nextSpaces.some((space) => space.layers.some((layer) => isLayerRestricted(layer.access)))) {
      setSelectedStepId(null);
      setSelectedDiffPath(null);
    }
  }, []);
  const replaceLocalSpace = useCallback((nextSpace: LocalSpaceSummary) => {
    const current = localSpacesRef.current;
    const next = current.some((space) => space.localSpaceId === nextSpace.localSpaceId)
      ? current.map((space) => (space.localSpaceId === nextSpace.localSpaceId ? nextSpace : space))
      : [...current, nextSpace];
    applyLocalSpaces(next);
  }, [applyLocalSpaces]);
  const performScan = useCallback(
    async (localSpaceId: string) => {
      const space = localSpacesRef.current.find((entry) => entry.localSpaceId === localSpaceId);
      const activeLayer = space?.layers.find((layer) => layer.layerId === space.activeLayerId);
      if (activeLayer && isLayerRestricted(activeLayer.access)) {
        applyLocalSpaces(localSpacesRef.current);
        throw new Error("This Layer is restricted. Local previews and diffs were purged.");
      }
      try {
        const [scan, activeWeave] = await Promise.all([
          scanWorkingTree(localSpaceId),
          weaveStatus(localSpaceId).catch(() => null)
        ]);
        const sanitized = space ? sanitizeWorkingTreeForAccess(scan, space) : scan;
        if (sanitized) {
          setWorkingTrees((current) => ({ ...current, [localSpaceId]: sanitized }));
        }
        setWeaveSessions((current) => ({ ...current, [localSpaceId]: activeWeave }));
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
      }
    },
    [applyLocalSpaces, clearCommandError, recordCommandError]
  );
  const scanLocalSpace = useCallback(
    (localSpaceId: string, background = false) =>
      operations.run({
        busyAction: background ? undefined : `scan:${localSpaceId}`,
        coalesce: true,
        key: `scan:${localSpaceId}`,
        task: () => performScan(localSpaceId)
      }),
    [operations, performScan]
  );
  const runAutoScan = useCallback(
    (localSpaceId: string) => {
      void scanLocalSpace(localSpaceId, true)
        .catch(() => {
          // The scan command already records the visible error state.
        });
    },
    [scanLocalSpace]
  );
  const performLoadSpaces = useCallback(
    async (showMessage = false) => {
      const [availableResult, localResult] = await Promise.allSettled([listAvailableSpaces(), listLocalSpaces()]);
      if (availableResult.status === "fulfilled") {
        setAvailableSpaces(availableResult.value);
        clearCommandError("available");
      } else {
        recordCommandError("available", availableResult.reason);
      }
      if (localResult.status === "fulfilled") {
        applyLocalSpaces(localResult.value);
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
    [applyLocalSpaces, clearCommandError, notify, recordCommandError]
  );
  const loadSpaces = useCallback(
    (showMessage = false) => operations.run({
      coalesce: true,
      key: "state-refresh",
      task: () => performLoadSpaces(showMessage)
    }),
    [operations, performLoadSpaces]
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
      } else {
        try {
          const localResult = await listLocalSpaces();
          applyLocalSpaces(localResult);
          clearCommandError("local");
          setSelectedLocalSpaceId((current) => current ?? localResult[0]?.localSpaceId ?? null);
        } catch (nextLocalError) {
          recordCommandError("local", nextLocalError);
        }
      }
      setLoadState("ready");
    } catch (nextError) {
      setLoadState("error");
      setError(errorMessage(nextError));
    }
  }, [applyLocalSpaces, clearCommandError, loadSpaces, notify, recordCommandError]);
  useEffect(() => {
    void hydrateStatus();
  }, [hydrateStatus]);
  useEffect(() => {
    localSpacesRef.current = localSpaces;
  }, [localSpaces]);
  useEffect(() => {
    const applyHash = () => {
      const hash = window.location.hash;
      setPage(pageFromHash(hash));
      const localMatch = hash.match(/desktop-local:([^#]+)/);
      if (localMatch?.[1]) {
        setSelectedLocalSpaceId(decodeURIComponent(localMatch[1]));
      }
    };
    const onHashChange = () => applyHash();
    applyHash();
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
  const selectedLayer =
    selectedLocalSpace?.layers.find((layer) => layer.layerId === selectedLocalSpace.activeLayerId) ?? selectedLocalSpace?.layers[0] ?? null;
  const selectedLayerRestricted = isLayerRestricted(selectedLayer?.access);
  const selectedWorkingTree = selectedLocalSpace && !selectedLayerRestricted ? workingTrees[selectedLocalSpace.localSpaceId] : undefined;
  const files = buildLayerFiles(selectedWorkingTree, selectedLayer);
  const activeDiffStepId = localSpaceTab === "steps" ? selectedStepId : null;
  const activeDiffContextKey = activeDiffStepId ? `step:${activeDiffStepId}` : "workingTree";
  const workingTreeChanges = buildChanges(selectedWorkingTree, null);
  const changes = activeDiffStepId ? buildChanges(selectedWorkingTree, activeDiffStepId) : workingTreeChanges;
  const baseSelectedDiff =
    changes.find((change) => change.path === selectedDiffPath)?.diff ?? changes[0]?.diff ?? null;
  const selectedDiffKey = diffWindowKey(
    selectedLocalSpace?.workspaceId,
    selectedLocalSpace?.spaceId,
    selectedLayer?.layerId,
    selectedLocalSpace?.localSpaceId,
    activeDiffStepId,
    baseSelectedDiff?.path
  );
  const selectedDiff =
    selectedDiffKey && diffWindowOverrides[selectedDiffKey]
      ? diffWindowOverrides[selectedDiffKey]
      : baseSelectedDiff;
  const timeline = buildTimeline(selectedLocalSpace, selectedWorkingTree, selectedStepId);
  const selectedWeaveSession = selectedLocalSpace ? weaveSessions[selectedLocalSpace.localSpaceId] ?? null : null;
  const selectedScanRevision = selectedLocalSpace ? scanRevisions[selectedLocalSpace.localSpaceId] ?? 0 : 0;
  const workspaceName = selectedLocalSpace?.name ?? availableSpaces[0]?.name ?? effectiveBootstrap?.workspaces[0]?.name ?? "Layrs Desktop";
  const connectedLabel = isConnected ? effectiveBootstrap?.account?.email ?? "Connected device" : "Not connected";
  const forgetTarget = forgetTargetId ? localSpaces.find((space) => space.localSpaceId === forgetTargetId) ?? null : null;
  const deleteLayerTarget = deleteLayerTargetId && selectedLocalSpace
    ? selectedLocalSpace.layers.find((layer) => layer.layerId === deleteLayerTargetId) ?? null
    : null;
  const disconnectLayerTarget = disconnectLayerTargetId && selectedLocalSpace
    ? selectedLocalSpace.layers.find((layer) => layer.layerId === disconnectLayerTargetId) ?? null
    : null;
  const clearStepsTarget = clearStepsTargetId && selectedLocalSpace
    ? selectedLocalSpace.layers.find((layer) => layer.layerId === clearStepsTargetId) ?? null
    : null;
  const activeParentLayer = selectedLayer?.parentLayerId && selectedLocalSpace
    ? selectedLocalSpace.layers.find((layer) => layer.layerId === selectedLayer.parentLayerId) ?? null
    : null;
  useEffect(() => {
    if (!selectedLocalSpace) {
      return;
    }
    setWeaveSourceLayerId((current) => current || selectedLocalSpace.activeLayerId || selectedLocalSpace.layers[0]?.layerId || "");
    setWeaveTargetLayerId((current) => {
      if (current) {
        return current;
      }
      const active = selectedLocalSpace.layers.find((layer) => layer.layerId === selectedLocalSpace.activeLayerId);
      return active?.parentLayerId || selectedLocalSpace.layers.find((layer) => layer.layerId !== active?.layerId)?.layerId || "";
    });
  }, [selectedLocalSpace?.localSpaceId, selectedLocalSpace?.activeLayerId]);
  useEffect(() => {
    if (!selectedStepId || selectedWorkingTree?.steps?.some((step) => step.stepId === selectedStepId)) {
      return;
    }
    setSelectedStepId(null);
    setSelectedDiffPath(null);
  }, [selectedStepId, selectedWorkingTree]);
  useEffect(() => {
    if (
      !selectedLocalSpace || selectedLayerRestricted ||
      workingTrees[selectedLocalSpace.localSpaceId]
    ) {
      return;
    }
    runAutoScan(selectedLocalSpace.localSpaceId);
  }, [runAutoScan, selectedLayerRestricted, selectedLocalSpace, workingTrees]);
  useEffect(() => {
    const localSpaceId = selectedLocalSpace?.localSpaceId;
    if (!localSpaceId || selectedLayerRestricted) {
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
  }, [runAutoScan, selectedLayerRestricted, selectedLocalSpace?.localSpaceId]);
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
    try {
      await operations.run({
        busyAction: "refresh",
        key: "connected-refresh",
        task: async () => {
          const response = await refreshBootstrap(defaultLocalRoot);
          applyPollResponse(response);
          await performLoadSpaces(true);
        }
      });
    } catch (nextError) {
      if (!isDesktopOperationCancelled(nextError)) {
        setError(errorMessage(nextError));
      }
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
      resetDesktopPrivateRequests();
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
  const {
    confirmClearLayerSteps,
    confirmDeleteLayer,
    confirmDisconnectLayer,
    confirmForgetLocalSpace,
    handleClearLayerSteps,
    handleCreateDraftLocalSpace,
    handleCreateLayerFromCurrent,
    handleCreateLocalSpace,
    handleDeleteLayer,
    handleDisconnectLayer,
    handleForgetLocalSpace,
    handleInitLocalSpace,
    handleOpenLocalSpace,
    handleSendDraftToStudio,
    handleSwitchLayer,
    selectLocalSpace
  } = useDesktopLocalActions({
    applyLocalSpaces,
    choosePage,
    clearCommandError,
    createDrafts,
    defaultLocalRoot,
    draftSpaceFolder,
    draftSpaceName,
    initSpaceFolder,
    initSpaceName,
    layerSearchQuery,
    loadSpaces,
    localSpaces,
    notify,
    operations,
    performScan,
    recordCommandError,
    refreshConnectedBootstrap,
    replaceLocalSpace,
    scanLocalSpace,
    selectedLocalSpace,
    sendWorkspaceId,
    setBusyAction,
    setClearStepsTargetId,
    setDeleteLayerTargetId,
    setDisconnectLayerTargetId,
    setDraftSpaceName,
    setError,
    setForgetTargetId,
    setInitSpaceFolder,
    setInitSpaceName,
    setLayerSearchQuery,
    setLocalSpaceTab,
    setSelectedDiffPath,
    setSelectedLocalSpaceId,
    setSelectedStepId,
    triggerPulse,
    workingTrees
  });
  const {
    confirmWeaveActiveLayerToParent,
    handleAbortWeave,
    handleContinueWeave,
    handlePublish,
    handleResolveWeaveConflict,
    handleSaveStep,
    handleSmartSaveShortcut,
    handleSync,
    handleWeave
  } = useDesktopSyncActions({
    choosePage,
    clearCommandError,
    handleSendDraftToStudio,
    notify,
    operations,
    performLoadSpaces,
    performScan,
    recordCommandError,
    replaceLocalSpace,
    selectedLocalSpace,
    selectedWorkingTree,
    sendWorkspaceId,
    setConfirmWeaveParent,
    setError,
    setWeaveSessions,
    shortcuts,
    triggerPulse,
    weaveSourceLayerId,
    weaveTargetLayerId,
    workingTreeChanges
  });
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (
        !shortcuts.enabled ||
        loadState !== "ready" ||
        !selectedLocalSpace ||
        forgetTarget ||
        deleteLayerTarget ||
        disconnectLayerTarget ||
        clearStepsTarget ||
        confirmWeaveParent ||
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
  }, [clearStepsTarget, confirmWeaveParent, deleteLayerTarget, disconnectLayerTarget, forgetTarget, loadState, selectedLocalSpace, shortcuts, workingTreeChanges.length, selectedWorkingTree?.pendingPublishCount]);
  const handleLoadDiffWindow = useDesktopDiffWindow({
    clearCommandError,
    localSpaceTab,
    localSpacesRef,
    recordCommandError,
    selectedLayerId: selectedLayer?.layerId,
    selectedLocalSpace,
    selectedStepId,
    setDiffWindowOverrides,
    setError
  });
  function selectTimelineItem(item: TimelineItem) {
    setSelectedStepId(item.kind === "step" ? item.id : null);
    setSelectedDiffPath(null);
  }
  return (
    <AppShell
      productName="Layrs Desktop"
      workspaceName={workspaceName}
      sidebar={
        <DesktopSidebar
          availableCount={availableSpaces.length}
          localSpaces={localSpaces}
          page={page}
          selectedLocalSpace={selectedLocalSpace}
          shortcuts={shortcuts}
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
            layerSearchQuery={layerSearchQuery}
            pulseTargets={pulseTargets}
            activeTab={localSpaceTab}
            busyAction={busyAction}
            commandErrors={commandErrors}
            onScan={(localSpaceId) => void scanLocalSpace(localSpaceId)}
            onSelectDiff={setSelectedDiffPath}
            onLoadDiffWindow={(path, start, limit) => void handleLoadDiffWindow(path, start, limit)}
            onSelectTimeline={selectTimelineItem}
            onSelectLayer={(layerId) => void handleSwitchLayer(layerId)}
            onLayerSearchChange={setLayerSearchQuery}
            onTabChange={setLocalSpaceTab}
            onCreateLayer={() => void handleCreateLayerFromCurrent()}
            onWeaveToParent={() => setConfirmWeaveParent(true)}
            onSync={() => void handleSync()}
            onClearLayerSteps={(layerId) => handleClearLayerSteps(layerId)}
            onDeleteLayer={(layerId) => void handleDeleteLayer(layerId)}
            onDisconnectLayer={(layerId) => handleDisconnectLayer(layerId)}
            onOpenSpaceSettings={() => choosePage("spaceSettings")}
            onOpenSpaceWeaves={() => choosePage("weaves")}
          />
        ) : null}
        {page === "spaceSettings" ? (
          <SpaceSettingsView
            busyAction={busyAction}
            changes={changes}
            commandErrors={commandErrors}
            onBack={() => choosePage("local")}
            onForgetSpace={(localSpaceId) => void handleForgetLocalSpace(localSpaceId)}
            onOpenSpace={(localSpaceId) => void handleOpenLocalSpace(localSpaceId)}
            onSendDraft={() => void handleSendDraftToStudio()}
            onSendWorkspaceChange={setSendWorkspaceId}
            selectedLayer={selectedLayer}
            selectedSpace={selectedLocalSpace}
            sendWorkspaceId={sendWorkspaceId}
            workspaces={effectiveBootstrap?.workspaces ?? []}
          />
        ) : null}
        {page === "weaves" ? (
          <SpaceWeavesView
            busyAction={busyAction}
            commandErrors={commandErrors}
            onBack={() => choosePage("local")}
            selectedSpace={selectedLocalSpace}
            weaveSession={selectedWeaveSession}
            weaveSourceLayerId={weaveSourceLayerId}
            weaveTargetLayerId={weaveTargetLayerId}
            onWeaveSourceLayerChange={setWeaveSourceLayerId}
            onWeaveTargetLayerChange={setWeaveTargetLayerId}
            onPreviewWeave={() => void handleWeave(true)}
            onApplyWeave={() => void handleWeave(false)}
            onResolveWeaveConflict={(path, resolution, manualText) => void handleResolveWeaveConflict(path, resolution, manualText)}
            onContinueWeave={() => void handleContinueWeave()}
            onAbortWeave={() => void handleAbortWeave()}
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
            autoLocalSteps={autoLocalSteps}
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
            onAutoLocalStepsChange={setAutoLocalSteps}
            onShortcutsChange={setShortcuts}
          />
        ) : null}
      </section>
      <DesktopConfirmations
        activeParentLayer={activeParentLayer}
        busyAction={busyAction}
        clearStepsTarget={clearStepsTarget}
        commandErrors={commandErrors}
        confirmWeaveParent={confirmWeaveParent}
        deleteLayerTarget={deleteLayerTarget}
        disconnectLayerTarget={disconnectLayerTarget}
        forgetTarget={forgetTarget}
        selectedLayer={selectedLayer}
        onCancelClearSteps={() => setClearStepsTargetId(null)}
        onCancelDeleteLayer={() => setDeleteLayerTargetId(null)}
        onCancelDisconnectLayer={() => setDisconnectLayerTargetId(null)}
        onCancelForget={() => setForgetTargetId(null)}
        onCancelWeaveParent={() => setConfirmWeaveParent(false)}
        onConfirmClearSteps={(layerId) => void confirmClearLayerSteps(layerId)}
        onConfirmDeleteLayer={(layerId) => void confirmDeleteLayer(layerId)}
        onConfirmDisconnectLayer={(layerId) => void confirmDisconnectLayer(layerId)}
        onConfirmForget={(localSpaceId) => void confirmForgetLocalSpace(localSpaceId)}
        onConfirmWeaveParent={() => void confirmWeaveActiveLayerToParent()}
      />
    </AppShell>
  );
}
