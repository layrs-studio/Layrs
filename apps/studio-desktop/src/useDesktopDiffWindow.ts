import { useEffect, useMemo, type Dispatch, type MutableRefObject, type SetStateAction } from "react";
import { diffWindowKey, errorMessage } from "./desktopModel";
import { isLayerRestricted } from "./desktopPrivateState";
import type { CommandKey } from "./desktopTypes";
import { loadDiffWindow, type LensDiffEntry, type LocalSpaceSummary } from "./tauri";

export class LatestDesktopRequest {
  private controller: AbortController | null = null;
  private generation = 0;
  private key = "";

  async run<T>(key: string, task: (signal: AbortSignal) => Promise<T>): Promise<T | undefined> {
    this.controller?.abort();
    const controller = new AbortController();
    const generation = ++this.generation;
    this.controller = controller;
    this.key = key;
    try {
      const result = await task(controller.signal);
      return this.isCurrent(key, generation, controller) ? result : undefined;
    } catch (error) {
      if (!this.isCurrent(key, generation, controller)) {
        return undefined;
      }
      throw error;
    }
  }

  cancel() {
    this.generation += 1;
    this.controller?.abort();
    this.controller = null;
    this.key = "";
  }

  private isCurrent(key: string, generation: number, controller: AbortController) {
    return this.key === key && this.generation === generation && this.controller === controller && !controller.signal.aborted;
  }
}

export function useDesktopDiffWindow({
  clearCommandError,
  localSpaceTab,
  localSpacesRef,
  recordCommandError,
  selectedLayerId,
  selectedLocalSpace,
  selectedStepId,
  setDiffWindowOverrides,
  setError
}: {
  clearCommandError: (key: CommandKey) => void;
  localSpaceTab: string;
  localSpacesRef: MutableRefObject<LocalSpaceSummary[]>;
  recordCommandError: (key: CommandKey, error: unknown) => void;
  selectedLayerId?: string;
  selectedLocalSpace: LocalSpaceSummary | null;
  selectedStepId: string | null;
  setDiffWindowOverrides: Dispatch<SetStateAction<Record<string, LensDiffEntry>>>;
  setError: Dispatch<SetStateAction<string | null>>;
}) {
  const requests = useMemo(() => new LatestDesktopRequest(), []);
  useEffect(() => () => requests.cancel(), [requests]);

  return async (path: string, start: number, limit: number) => {
    if (!selectedLocalSpace || !selectedLayerId) {
      return;
    }
    const sourceStepId = localSpaceTab === "steps" ? selectedStepId : null;
    const source = sourceStepId ? `localStep:${sourceStepId}` : "workingTree";
    const key = diffWindowKey(
      selectedLocalSpace.workspaceId,
      selectedLocalSpace.spaceId,
      selectedLayerId,
      selectedLocalSpace.localSpaceId,
      sourceStepId,
      path
    );
    setError(null);
    try {
      const nextDiff = await requests.run(key, () =>
        loadDiffWindow(selectedLocalSpace.localSpaceId, path, source, start, limit)
      );
      const currentSpace = localSpacesRef.current.find((space) => space.localSpaceId === selectedLocalSpace.localSpaceId);
      const currentLayer = currentSpace?.layers.find((layer) => layer.layerId === selectedLayerId);
      if (nextDiff && currentSpace?.workspaceId === selectedLocalSpace.workspaceId && !isLayerRestricted(currentLayer?.access)) {
        setDiffWindowOverrides((current) => ({ ...current, [key]: nextDiff }));
        clearCommandError("diff-window");
      }
    } catch (nextError) {
      recordCommandError("diff-window", nextError);
      setError(errorMessage(nextError));
    }
  };
}
