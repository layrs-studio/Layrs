import { useEffect, useRef, useState } from "react";
import { nativeProjection } from "../tauri";
import { useProjection } from "./projection";
import { useWorkspace } from "./workspace";

/** Shares the projection's native queue; errors remain visible on this surface. */
export function useOfflineTask() {
  const projection = useProjection(), workspace = useWorkspace();
  const [error, setError] = useState("");
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  async function execute<T>(label: string, work: () => Promise<T>): Promise<T | undefined> {
    let result: T | undefined;
    setError("");
    await projection.perform(label, async revision => {
      try {
        const next = await work();
        if (mounted.current && projection.isCurrent(revision)) result = next;
      } catch (cause) {
        if (mounted.current) setError(cause instanceof Error ? cause.message : String(cause));
        // The provider invalidates its status on errors; this prevents stale writes.
        throw cause;
      }
    });
    return result;
  }
  const local = <T,>(request: Record<string, unknown>) => nativeProjection<T>(workspace.root, { action: "offline", request });
  return { execute, local, error, busy: Boolean(workspace.busy), blocked: workspace.busy ? `Opération en cours : ${workspace.busy}` : !projection.data ? "Actualisez l’accès limité avant de poursuivre." : undefined };
}
