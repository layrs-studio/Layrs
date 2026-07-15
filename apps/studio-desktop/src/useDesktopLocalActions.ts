import type { NotificationContextValue } from "@layrs/ui";
import type { Dispatch, SetStateAction } from "react";
import { defaultCreateDraft, errorMessage } from "./desktopModel";
import { isLayerRestricted } from "./desktopPrivateState";
import type { DesktopOperationController } from "./desktopOperationQueue";
import type { CommandKey, CreateDraft, DesktopPage, LocalSpaceTab, PulseTarget } from "./desktopTypes";
import {
  clearLayerSteps,
  createDraftLocalSpace,
  createLayerFromCurrent,
  createLocalSpace,
  deleteLayer,
  disconnectLayerFromParent,
  forgetLocalSpace,
  initLocalSpace,
  openLocalSpace,
  sendDraftLocalSpace,
  switchLayer,
  type AvailableSpaceView,
  type LocalSpaceSummary,
  type WorkingTreeScan
} from "./tauri";

type Setter<T> = Dispatch<SetStateAction<T>>;

interface DesktopLocalActionsOptions {
  applyLocalSpaces: (spaces: LocalSpaceSummary[]) => void;
  choosePage: (page: DesktopPage) => void;
  clearCommandError: (key: CommandKey) => void;
  createDrafts: Record<string, CreateDraft>;
  defaultLocalRoot: string;
  draftSpaceFolder: string;
  draftSpaceName: string;
  initSpaceFolder: string;
  initSpaceName: string;
  layerSearchQuery: string;
  loadSpaces: (showMessage?: boolean) => Promise<void>;
  localSpaces: LocalSpaceSummary[];
  notify: NotificationContextValue["notify"];
  operations: DesktopOperationController;
  performScan: (localSpaceId: string) => Promise<WorkingTreeScan | undefined>;
  recordCommandError: (key: CommandKey, error: unknown) => void;
  refreshConnectedBootstrap: () => Promise<void>;
  replaceLocalSpace: (space: LocalSpaceSummary) => void;
  scanLocalSpace: (localSpaceId: string, background?: boolean) => Promise<WorkingTreeScan | undefined>;
  selectedLocalSpace: LocalSpaceSummary | null;
  sendWorkspaceId: string;
  setBusyAction: Setter<string | null>;
  setClearStepsTargetId: Setter<string | null>;
  setDeleteLayerTargetId: Setter<string | null>;
  setDisconnectLayerTargetId: Setter<string | null>;
  setDraftSpaceName: Setter<string>;
  setError: Setter<string | null>;
  setForgetTargetId: Setter<string | null>;
  setInitSpaceFolder: Setter<string>;
  setInitSpaceName: Setter<string>;
  setLayerSearchQuery: Setter<string>;
  setLocalSpaceTab: Setter<LocalSpaceTab>;
  setSelectedDiffPath: Setter<string | null>;
  setSelectedLocalSpaceId: Setter<string | null>;
  setSelectedStepId: Setter<string | null>;
  triggerPulse: (targets: PulseTarget[]) => void;
  workingTrees: Record<string, WorkingTreeScan>;
}

export function useDesktopLocalActions(options: DesktopLocalActionsOptions) {
  const {
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
  } = options;

  async function scanResultIfOpen(space: LocalSpaceSummary) {
    const activeLayer = space.layers.find((layer) => layer.layerId === space.activeLayerId);
    if (!isLayerRestricted(activeLayer?.access)) {
      await performScan(space.localSpaceId);
    }
  }

  async function selectLocalSpace(localSpaceId: string) {
    setSelectedLocalSpaceId(localSpaceId);
    choosePage("local");
    window.location.hash = `desktop-local:${encodeURIComponent(localSpaceId)}`;
    if (!workingTrees[localSpaceId]) {
      await scanLocalSpace(localSpaceId).catch(() => undefined);
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
      await scanResultIfOpen(result.localSpace);
      await loadSpaces();
      clearCommandError("create");
      notify({ tone: "success", title: result.created ? "Local Space created" : "Existing Local Space opened", dedupeKey: "desktop-local-space-created" });
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
    if (!name || !targetFolder) {
      setError(!name ? "Name the empty local Space before creating it." : "Choose a folder before creating an empty local Space.");
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
      await scanResultIfOpen(result.localSpace);
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
    if (!name || !targetFolder) {
      setError(!name ? "Name the Local Space before initializing this folder." : "Choose an existing folder before initializing a Local Space.");
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
      await scanResultIfOpen(result.localSpace);
      await loadSpaces();
      clearCommandError("init-local");
      triggerPulse(["changes", "steps"]);
      notify({ tone: "success", title: result.created ? "Folder initialized" : "Local Space opened", dedupeKey: "desktop-local-init" });
    } catch (nextError) {
      recordCommandError("init-local", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
    }
  }

  async function handleSendDraftToStudio() {
    if (!selectedLocalSpace || !sendWorkspaceId.trim()) {
      setError("Choose a Workspace before sending this Draft Local Space.");
      return;
    }
    setBusyAction("send-draft");
    setError(null);
    try {
      const result = await sendDraftLocalSpace(selectedLocalSpace.localSpaceId, sendWorkspaceId.trim());
      replaceLocalSpace(result.localSpace);
      setSelectedLocalSpaceId(result.localSpace.localSpaceId);
      await scanResultIfOpen(result.localSpace);
      await loadSpaces();
      clearCommandError("send-draft");
      notify({ tone: "success", title: "Draft sent to Studio", message: `${result.publishedLayers} Layer(s) published.`, dedupeKey: "desktop-draft-sent" });
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
      await scanResultIfOpen(nextSpace);
      clearCommandError("open");
      notify({ tone: "success", title: "Local Space opened", dedupeKey: "desktop-local-opened" });
    } catch (nextError) {
      recordCommandError("open", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
    }
  }

  function handleForgetLocalSpace(localSpaceId: string) {
    if (localSpaces.some((space) => space.localSpaceId === localSpaceId)) {
      clearCommandError("forget-local");
      setForgetTargetId(localSpaceId);
    }
  }

  async function confirmForgetLocalSpace(localSpaceId: string) {
    setBusyAction("forget-local");
    setError(null);
    try {
      const result = await forgetLocalSpace(localSpaceId);
      const nextSpaces = localSpaces.filter((space) => space.localSpaceId !== localSpaceId);
      applyLocalSpaces(nextSpaces);
      setSelectedDiffPath(null);
      setSelectedStepId(null);
      setSelectedLocalSpaceId((current) => current === localSpaceId ? nextSpaces[0]?.localSpaceId ?? null : current);
      clearCommandError("forget-local");
      setForgetTargetId(null);
      notify({ tone: "success", title: "Local Space forgotten", message: result.archivedLayrsPath ? `${result.message} Archived metadata: ${result.archivedLayrsPath}` : result.message, dedupeKey: "desktop-forgot-local" });
      await refreshConnectedBootstrap();
    } catch (nextError) {
      recordCommandError("forget-local", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
    }
  }

  async function handleSwitchLayer(layerId: string) {
    if (!selectedLocalSpace || layerId === selectedLocalSpace.activeLayerId) {
      return;
    }
    setError(null);
    try {
      await operations.run({
        busyAction: `switch:${layerId}`,
        key: `switch:${selectedLocalSpace.localSpaceId}:${layerId}`,
        task: async () => {
          const result = await switchLayer(selectedLocalSpace.localSpaceId, layerId);
          replaceLocalSpace(result.localSpace);
          setSelectedLocalSpaceId(result.localSpace.localSpaceId);
          await scanResultIfOpen(result.localSpace);
          clearCommandError("switch");
          triggerPulse(["layer", "steps"]);
          notify({ tone: "success", title: "Layer switched", message: `${result.changedFiles} changed file(s) preserved in local Steps.`, dedupeKey: "desktop-layer-switched" });
        }
      });
    } catch (nextError) {
      recordCommandError("switch", nextError);
      setError(errorMessage(nextError));
    }
  }

  async function handleCreateLayerFromCurrent() {
    if (!selectedLocalSpace || !layerSearchQuery.trim()) {
      setError("Name the new Layer before creating it.");
      return;
    }
    setBusyAction("create-layer");
    setError(null);
    try {
      const result = await createLayerFromCurrent(selectedLocalSpace.localSpaceId, layerSearchQuery.trim());
      replaceLocalSpace(result.localSpace);
      setSelectedLocalSpaceId(result.localSpace.localSpaceId);
      setLayerSearchQuery("");
      await scanResultIfOpen(result.localSpace);
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

  function handleDeleteLayer(layerId: string) {
    if (selectedLocalSpace) {
      clearCommandError("delete-layer");
      setDeleteLayerTargetId(layerId);
    }
  }

  async function confirmDeleteLayer(layerId: string) {
    if (!selectedLocalSpace) return;
    setBusyAction(`delete-layer:${layerId}`);
    setError(null);
    try {
      const result = await deleteLayer(selectedLocalSpace.localSpaceId, layerId);
      replaceLocalSpace(result.localSpace);
      setSelectedLocalSpaceId(result.localSpace.localSpaceId);
      setSelectedStepId(null);
      setSelectedDiffPath(null);
      await scanResultIfOpen(result.localSpace);
      await loadSpaces();
      clearCommandError("delete-layer");
      setDeleteLayerTargetId(null);
      triggerPulse(["layer"]);
      notify({ tone: "success", title: "Layer deleted", message: result.message, dedupeKey: "desktop-layer-deleted" });
    } catch (nextError) {
      recordCommandError("delete-layer", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
    }
  }

  function handleDisconnectLayer(layerId: string) {
    clearCommandError("disconnect-layer");
    setDisconnectLayerTargetId(layerId);
  }

  async function confirmDisconnectLayer(layerId: string) {
    if (!selectedLocalSpace) return;
    setBusyAction("disconnect-layer");
    setError(null);
    try {
      const result = await disconnectLayerFromParent(selectedLocalSpace.localSpaceId, layerId);
      replaceLocalSpace(result.localSpace);
      await scanResultIfOpen(result.localSpace);
      clearCommandError("disconnect-layer");
      setDisconnectLayerTargetId(null);
      triggerPulse(["layer"]);
      notify({ tone: "success", title: "Layer disconnected", message: result.message, dedupeKey: "desktop-layer-disconnected" });
    } catch (nextError) {
      recordCommandError("disconnect-layer", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
    }
  }

  function handleClearLayerSteps(layerId: string) {
    clearCommandError("clear-steps");
    setClearStepsTargetId(layerId);
  }

  async function confirmClearLayerSteps(layerId: string) {
    if (!selectedLocalSpace) return;
    setBusyAction("clear-steps");
    setError(null);
    try {
      const result = await clearLayerSteps(selectedLocalSpace.localSpaceId, layerId);
      replaceLocalSpace(result.localSpace);
      setSelectedStepId(null);
      setSelectedDiffPath(null);
      await scanResultIfOpen(result.localSpace);
      clearCommandError("clear-steps");
      setClearStepsTargetId(null);
      triggerPulse(["steps", "layer"]);
      notify({ tone: "success", title: "Layer Steps cleared", message: result.message, dedupeKey: "desktop-layer-steps-cleared" });
    } catch (nextError) {
      recordCommandError("clear-steps", nextError);
      setError(errorMessage(nextError));
    } finally {
      setBusyAction(null);
    }
  }

  return {
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
  };
}
