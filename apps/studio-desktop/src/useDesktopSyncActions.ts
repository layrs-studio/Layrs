import type { NotificationContextValue } from "@layrs/ui";
import type { Dispatch, SetStateAction } from "react";
import { errorMessage } from "./desktopModel";
import { isLayerRestricted } from "./desktopPrivateState";
import type { DesktopOperationController } from "./desktopOperationQueue";
import type { CommandKey, DesktopPage, LocalChange, PulseTarget } from "./desktopTypes";
import {
  abortWeave,
  continueWeave,
  publishLocalSpace,
  resolveWeaveConflict,
  saveLocalStep,
  syncLocalSpace,
  weaveActiveLayerToParent,
  weaveLayers,
  type DesktopShortcutSettings,
  type LocalSpaceSummary,
  type WeaveSessionSummary,
  type WorkingTreeScan
} from "./tauri";

type Setter<T> = Dispatch<SetStateAction<T>>;

interface DesktopSyncActionsOptions {
  choosePage: (page: DesktopPage) => void;
  clearCommandError: (key: CommandKey) => void;
  handleSendDraftToStudio: () => Promise<void>;
  notify: NotificationContextValue["notify"];
  operations: DesktopOperationController;
  performLoadSpaces: (showMessage?: boolean) => Promise<void>;
  performScan: (localSpaceId: string) => Promise<WorkingTreeScan | undefined>;
  recordCommandError: (key: CommandKey, error: unknown) => void;
  replaceLocalSpace: (space: LocalSpaceSummary) => void;
  selectedLocalSpace: LocalSpaceSummary | null;
  selectedWorkingTree?: WorkingTreeScan;
  sendWorkspaceId: string;
  setConfirmWeaveParent: Setter<boolean>;
  setError: Setter<string | null>;
  setWeaveSessions: Setter<Record<string, WeaveSessionSummary | null>>;
  shortcuts: DesktopShortcutSettings;
  triggerPulse: (targets: PulseTarget[]) => void;
  weaveSourceLayerId: string;
  weaveTargetLayerId: string;
  workingTreeChanges: LocalChange[];
}

export function useDesktopSyncActions(options: DesktopSyncActionsOptions) {
  const {
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
  } = options;

  async function scanResultIfOpen(space: LocalSpaceSummary) {
    const activeLayer = space.layers.find((layer) => layer.layerId === space.activeLayerId);
    if (!isLayerRestricted(activeLayer?.access)) {
      await performScan(space.localSpaceId);
    }
  }

  async function confirmWeaveActiveLayerToParent() {
    if (!selectedLocalSpace) return;
    setError(null);
    try {
      await operations.run({
        busyAction: "weave-parent",
        key: `weave-parent:${selectedLocalSpace.localSpaceId}`,
        task: async () => {
          const result = await weaveActiveLayerToParent(selectedLocalSpace.localSpaceId, false);
          replaceLocalSpace(result.localSpace);
          setWeaveSessions((current) => ({ ...current, [result.localSpace.localSpaceId]: result.session }));
          await scanResultIfOpen(result.localSpace);
          clearCommandError("weave-parent");
          setConfirmWeaveParent(false);
          triggerPulse(["steps", "layer", "changes"]);
          if (result.session.status === "conflicted") choosePage("weaves");
          notify({ tone: result.session.status === "conflicted" ? "warning" : "success", title: result.session.status === "conflicted" ? "Weave needs resolution" : "Woven to parent", message: result.message, dedupeKey: "desktop-weave-parent" });
        }
      });
    } catch (nextError) {
      recordCommandError("weave-parent", nextError);
      setError(errorMessage(nextError));
    }
  }

  async function handleSaveStep() {
    if (!selectedLocalSpace) {
      notify({ tone: "warning", title: "No Local Space selected", dedupeKey: "desktop-save-no-space" });
      return;
    }
    setError(null);
    try {
      await operations.run({
        busyAction: "save-step",
        key: `save-step:${selectedLocalSpace.localSpaceId}`,
        task: async () => {
          const result = await saveLocalStep(selectedLocalSpace.localSpaceId);
          replaceLocalSpace(result.localSpace);
          await scanResultIfOpen(result.localSpace);
          clearCommandError("save-step");
          if (result.status === "clean") {
            notify({ tone: "info", title: "Nothing to save", message: result.message, dedupeKey: "desktop-save-clean" });
          } else {
            triggerPulse(["changes", "steps", "sync", "layer"]);
            notify({ tone: "success", title: "Step created", message: `${result.changedFiles} file(s), +${result.diffStats.additions}, -${result.diffStats.deletions}`, dedupeKey: "desktop-step-saved" });
          }
        }
      });
    } catch (nextError) {
      recordCommandError("save-step", nextError);
      setError(errorMessage(nextError));
    }
  }

  async function handlePublish() {
    if (!selectedLocalSpace) return;
    if (selectedLocalSpace.state === "draft") {
      if (!sendWorkspaceId.trim()) {
        choosePage("spaceSettings");
        notify({ tone: "warning", title: "Choose a Workspace", message: "Choose the target Workspace in Space Settings before creating this Draft in Studio.", dedupeKey: "desktop-draft-publish-workspace" });
        setError("Choose a Workspace in Space Settings before publishing this Draft Local Space.");
        return;
      }
      await handleSendDraftToStudio();
      return;
    }
    setError(null);
    try {
      await operations.run({
        busyAction: "publish",
        key: `publish:${selectedLocalSpace.localSpaceId}`,
        task: async () => {
          const result = await publishLocalSpace(selectedLocalSpace.localSpaceId);
          replaceLocalSpace(result.localSpace);
          await scanResultIfOpen(result.localSpace);
          await performLoadSpaces();
          clearCommandError("publish");
          triggerPulse(["sync", "steps"]);
          notify({ tone: result.status === "clean" ? "info" : "success", title: "Publish complete", message: result.message, dedupeKey: "desktop-publish" });
        }
      });
    } catch (nextError) {
      recordCommandError("publish", nextError);
      setError(errorMessage(nextError));
    }
  }

  async function handleSmartSaveShortcut() {
    if (!selectedLocalSpace) {
      notify({ tone: "warning", title: "No Local Space selected", dedupeKey: "desktop-save-no-space" });
      return;
    }
    if (workingTreeChanges.length > 0) {
      await handleSaveStep();
    } else if (shortcuts.smartSavePublishesPendingStep && (selectedWorkingTree?.pendingPublishCount ?? 0) > 0) {
      await handlePublish();
    } else {
      notify({ tone: "info", title: "Nothing to save", message: "No local changes or pending publish step.", dedupeKey: "desktop-save-nothing" });
    }
  }

  async function handleSync() {
    if (!selectedLocalSpace) return;
    if (selectedLocalSpace.state === "draft") {
      if (!sendWorkspaceId.trim()) {
        choosePage("spaceSettings");
        notify({ tone: "warning", title: "Choose a Workspace", message: "Choose the target Workspace in Space Settings before creating this Draft in Studio.", dedupeKey: "desktop-draft-sync-workspace" });
        setError("Choose a Workspace in Space Settings before syncing this Draft Local Space.");
        return;
      }
      await handleSendDraftToStudio();
      return;
    }
    setError(null);
    try {
      await operations.run({
        busyAction: "sync",
        key: `sync:${selectedLocalSpace.localSpaceId}`,
        task: async () => {
          const result = await syncLocalSpace(selectedLocalSpace.localSpaceId);
          replaceLocalSpace(result.localSpace);
          await scanResultIfOpen(result.localSpace);
          await performLoadSpaces();
          clearCommandError("sync");
          triggerPulse(result.status === "conflicted" ? ["sync", "changes", "layer"] : ["sync", "steps", "changes"]);
          notify({ tone: result.status === "conflicted" ? "warning" : "success", title: result.status === "conflicted" ? "Sync needs resolution" : "Sync complete", message: result.message, dedupeKey: "desktop-sync" });
        }
      });
    } catch (nextError) {
      recordCommandError("sync", nextError);
      setError(errorMessage(nextError));
    }
  }

  async function handleWeave(preview: boolean) {
    if (!selectedLocalSpace || !weaveSourceLayerId || !weaveTargetLayerId) {
      notify({ tone: "warning", title: "Choose two Layers", dedupeKey: "desktop-weave-missing-layers" });
      return;
    }
    setError(null);
    const busyAction = preview ? "weave-preview" : "weave-apply";
    try {
      await operations.run({
        busyAction,
        key: `${busyAction}:${selectedLocalSpace.localSpaceId}`,
        task: async () => {
          const result = await weaveLayers(selectedLocalSpace.localSpaceId, weaveSourceLayerId, weaveTargetLayerId, preview);
          replaceLocalSpace(result.localSpace);
          setWeaveSessions((current) => ({ ...current, [result.localSpace.localSpaceId]: result.session }));
          await scanResultIfOpen(result.localSpace);
          clearCommandError("weave");
          triggerPulse(["steps", "layer", "changes"]);
          notify({ tone: result.session.status === "conflicted" ? "warning" : "success", title: preview ? "Weave preview ready" : "Weave updated", message: result.message, dedupeKey: preview ? "desktop-weave-preview" : "desktop-weave-apply" });
        }
      });
    } catch (nextError) {
      recordCommandError("weave", nextError);
      setError(errorMessage(nextError));
    }
  }

  async function handleResolveWeaveConflict(path: string, resolution: string, manualText?: string) {
    if (!selectedLocalSpace) return;
    setError(null);
    try {
      await operations.run({
        busyAction: `weave-resolve:${path}`,
        key: `weave-resolve:${selectedLocalSpace.localSpaceId}:${path}`,
        task: async () => {
          const result = await resolveWeaveConflict(selectedLocalSpace.localSpaceId, path, resolution, undefined, manualText);
          replaceLocalSpace(result.localSpace);
          setWeaveSessions((current) => ({ ...current, [result.localSpace.localSpaceId]: result.session }));
          await scanResultIfOpen(result.localSpace);
          clearCommandError("weave");
          notify({ tone: "success", title: "Conflict resolved", message: result.message, dedupeKey: `desktop-weave-resolved-${path}` });
        }
      });
    } catch (nextError) {
      recordCommandError("weave", nextError);
      setError(errorMessage(nextError));
    }
  }

  async function handleContinueWeave() {
    if (!selectedLocalSpace) return;
    setError(null);
    try {
      await operations.run({
        busyAction: "weave-continue",
        key: `weave-continue:${selectedLocalSpace.localSpaceId}`,
        task: async () => {
          const result = await continueWeave(selectedLocalSpace.localSpaceId);
          replaceLocalSpace(result.localSpace);
          setWeaveSessions((current) => ({ ...current, [result.localSpace.localSpaceId]: null }));
          await scanResultIfOpen(result.localSpace);
          clearCommandError("weave");
          triggerPulse(["steps", "layer", "sync"]);
          notify({ tone: "success", title: "Weave complete", message: result.message, dedupeKey: "desktop-weave-continue" });
        }
      });
    } catch (nextError) {
      recordCommandError("weave", nextError);
      setError(errorMessage(nextError));
    }
  }

  async function handleAbortWeave() {
    if (!selectedLocalSpace) return;
    setError(null);
    try {
      await operations.run({
        busyAction: "weave-abort",
        key: `weave-abort:${selectedLocalSpace.localSpaceId}`,
        task: async () => {
          const result = await abortWeave(selectedLocalSpace.localSpaceId);
          replaceLocalSpace(result.localSpace);
          setWeaveSessions((current) => ({ ...current, [result.localSpace.localSpaceId]: null }));
          await scanResultIfOpen(result.localSpace);
          clearCommandError("weave");
          triggerPulse(["layer", "changes"]);
          notify({ tone: "info", title: "Weave aborted", message: result.message, dedupeKey: "desktop-weave-abort" });
        }
      });
    } catch (nextError) {
      recordCommandError("weave", nextError);
      setError(errorMessage(nextError));
    }
  }

  return {
    confirmWeaveActiveLayerToParent,
    handleAbortWeave,
    handleContinueWeave,
    handlePublish,
    handleResolveWeaveConflict,
    handleSaveStep,
    handleSmartSaveShortcut,
    handleSync,
    handleWeave
  };
}
