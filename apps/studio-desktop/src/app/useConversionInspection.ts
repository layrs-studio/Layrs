import { useCallback, useEffect, useRef, useState } from "react";
import { nativeProjection } from "../tauri";
import { conversionInspection, type ConversionInspection } from "./offline";
import { useProjection } from "./projection";
import { useWorkspace } from "./workspace";

/** Source identity inspection shared by conversion and Diagnostic; never migrates. */
export function useProjectionInspection(action: "inspectConversionSource" | "inspectRecovery") {
  const { root } = useWorkspace(), { serial } = useProjection();
  const [source, setSource] = useState<ConversionInspection | null>(null);
  const [error, setError] = useState(""), [loading, setLoading] = useState(false);
  const revision = useRef(0);
  const invalidate = useCallback(() => { revision.current++; setSource(null); setLoading(false); }, []);
  const replace = useCallback((next: ConversionInspection) => { revision.current++; setSource(next); setError(""); setLoading(false); }, []);
  const refresh = useCallback(async () => {
    const current = ++revision.current;
    setSource(null); setError(""); setLoading(true);
    try {
      const result = await serial(() => revision.current === current ? nativeProjection(root, { action: "offline", request: { action } }) : Promise.resolve(null));
      if (revision.current !== current) return;
      const next = conversionInspection(result);
      if (revision.current === current) setSource(next);
    } catch (cause) {
      if (revision.current === current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (revision.current === current) setLoading(false);
    }
  }, [root, serial, action]);
  useEffect(() => { void refresh(); return () => { revision.current++; }; }, [refresh]);
  return { source, error, loading, refresh, invalidate, replace };
}
export function useConversionInspection() { return useProjectionInspection("inspectConversionSource"); }
