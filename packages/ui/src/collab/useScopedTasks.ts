import { useCallback, useEffect, useRef, useState } from "react";

/**
 * Generation guard shared by the collaboration pages. Every task is bound to
 * the scope (account, server, project) it started in: when the scope changes
 * or the view unmounts, late answers are dropped instead of being rendered.
 * One task per key runs at a time, which prevents double submissions without
 * freezing unrelated reads.
 */
export function useScopedTasks(scope: string) {
  const generation = useRef(0);
  const running = useRef(new Set<string>());
  const [busy, setBusy] = useState<ReadonlySet<string>>(() => new Set());

  useEffect(() => {
    generation.current++;
    running.current = new Set();
    setBusy(new Set());
    return () => {
      generation.current++;
    };
  }, [scope]);

  const run = useCallback(async (key: string, work: (alive: () => boolean) => Promise<void>, onError?: (error: unknown) => void) => {
    if (running.current.has(key)) return false;
    const current = generation.current;
    const alive = () => current === generation.current;
    const set = running.current;
    set.add(key);
    setBusy(new Set(set));
    try {
      await work(alive);
      return alive();
    } catch (error) {
      if (alive()) onError?.(error);
      return false;
    } finally {
      set.delete(key);
      if (alive()) setBusy(new Set(set));
    }
  }, []);

  const isBusy = useCallback((key: string) => busy.has(key), [busy]);
  return { run, isBusy, anyBusy: busy.size > 0 };
}
