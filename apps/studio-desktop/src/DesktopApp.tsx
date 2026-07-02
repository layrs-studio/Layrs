import { AppShell, ConfirmModal, Sidebar, StatusPill, useNotifications } from "@layrs/ui";
import { useCallback, useEffect, useRef, useState } from "react";
import { CommandErrors, SettingsView, ShortcutFooter } from "./DesktopSettingsView";
import { AvailableSpacesView, DraftLocalSpaceView, LocalSpacesView } from "./DesktopViews";
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
import {
  AvailableSpaceView,
  BootstrapData,
  createDraftLocalSpace,
  createLayerFromCurrent,
  createLocalSpace,
  deleteLayer,
  DesktopSettings,
  DesktopShortcutSettings,
  DesktopStatus,
  DeviceLoginPollResponse,
  DeviceLoginStartResponse,
  forgetLocalSpace,
  getDesktopStatus,
  initLocalSpace,
  LensDiffEntry,
  listAvailableSpaces,
  listLocalSpaces,
  loadDesktopSettings,
  loadDiffWindow,
  LocalSpaceSummary,
  openLocalSpace,
  pollDeviceLogin,
  publishLocalSpace,
  receiveLocalSpace,
  refreshBootstrap,
  saveDesktopSettings,
  saveLocalStep,
  scanWorkingTree,
  selectFolder,
  sendDraftLocalSpace,
  startDeviceLogin,
  switchLayer,
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
      } else {
        try {
          const localResult = await listLocalSpaces();
          setLocalSpaces(localResult);
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


