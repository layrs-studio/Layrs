import { useEffect, type Dispatch, type SetStateAction } from "react";
import type { CommandKey, DesktopPage, LoadState } from "./desktopTypes";
import type {
  BootstrapData,
  DesktopStatus,
  AvailableSpaceView,
  LensDiffEntry,
  LocalSpaceSummary,
  WeaveSessionSummary,
  WorkingTreeScan
} from "./tauri";
import { DESKTOP_AUTH_EXPIRED_EVENT, DESKTOP_AUTH_EXPIRED_MESSAGE } from "./desktopPrivateApi";
import type { DesktopOperationController } from "./desktopOperationQueue";

type Setter<T> = Dispatch<SetStateAction<T>>;

export function useDesktopAuthPurge({
  onPurgePrivateState,
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
}: {
  onPurgePrivateState: () => void;
  operations: DesktopOperationController;
  setAvailableSpaces: Setter<AvailableSpaceView[]>;
  setBootstrap: Setter<BootstrapData | null>;
  setCommandErrors: Setter<Partial<Record<CommandKey, string>>>;
  setDiffWindowOverrides: Setter<Record<string, LensDiffEntry>>;
  setError: Setter<string | null>;
  setLoadState: Setter<LoadState>;
  setLocalSpaces: Setter<LocalSpaceSummary[]>;
  setPage: Setter<DesktopPage>;
  setScanRevisions: Setter<Record<string, number>>;
  setSelectedDiffPath: Setter<string | null>;
  setSelectedLocalSpaceId: Setter<string | null>;
  setSelectedStepId: Setter<string | null>;
  setStatus: Setter<DesktopStatus | null>;
  setWeaveSessions: Setter<Record<string, WeaveSessionSummary | null>>;
  setWorkingTrees: Setter<Record<string, WorkingTreeScan>>;
}) {
  useEffect(() => {
    function purge(event: Event) {
      const message = (event as CustomEvent<{ message?: string }>).detail?.message ?? DESKTOP_AUTH_EXPIRED_MESSAGE;
      operations.invalidate();
      onPurgePrivateState();
      setBootstrap(null);
      setStatus((current) => current ? { ...current, connected: false, cachedBootstrap: undefined } : current);
      setAvailableSpaces([]);
      setLocalSpaces([]);
      setWorkingTrees({});
      setWeaveSessions({});
      setScanRevisions({});
      setDiffWindowOverrides({});
      setSelectedLocalSpaceId(null);
      setSelectedStepId(null);
      setSelectedDiffPath(null);
      setCommandErrors({});
      setError(message);
      setLoadState("ready");
      setPage("settings");
      window.location.hash = "desktop-settings";
    }

    window.addEventListener(DESKTOP_AUTH_EXPIRED_EVENT, purge);
    return () => window.removeEventListener(DESKTOP_AUTH_EXPIRED_EVENT, purge);
  }, [
    onPurgePrivateState,
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
  ]);
}
