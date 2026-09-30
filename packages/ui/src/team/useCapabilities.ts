import { useCallback, useEffect, useRef, useState } from "react";
import type { SpaceCapabilities, SpaceCapabilityFlags } from "../api/catalog-types";

/**
 * Per-project capability hint (`GET /v3/spaces/:space/capabilities`). It only
 * decides which actions to *offer*; the server rechecks every mutation, and a
 * refusal triggers a refetch. `null` flags mean "unknown": callers fall back
 * to asking the guarded read and interpreting its refusal.
 */
export function useCapabilities(scope: string, load?: () => Promise<SpaceCapabilities>) {
  const [value, setValue] = useState<SpaceCapabilities | null>(null);
  const [loading, setLoading] = useState(Boolean(load));
  const generation = useRef(0);
  const loader = useRef(load);
  loader.current = load;

  const reload = useCallback(async () => {
    const current = ++generation.current;
    if (!loader.current) {
      setLoading(false);
      return;
    }
    setLoading(true);
    try {
      const next = await loader.current();
      if (current === generation.current) setValue(next.schemaVersion === "layrs.space-capabilities.v1" ? next : null);
    } catch {
      if (current === generation.current) setValue(null);
    } finally {
      if (current === generation.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    setValue(null);
    void reload();
    return () => {
      generation.current++;
    };
  }, [scope, reload]);

  const flag = useCallback((name: keyof SpaceCapabilityFlags): boolean | null => (value ? value.capabilities[name] === true : null), [value]);
  return { value, flags: value?.capabilities ?? null, flag, loading, reload };
}

export type Capabilities = ReturnType<typeof useCapabilities>;
