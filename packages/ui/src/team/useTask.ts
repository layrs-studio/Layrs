import { useCallback, useEffect, useRef, useState } from "react";
import { describeError } from "./errors";

/**
 * Per-view request guard. Each `scope` (account + project + origin) is a
 * generation: answers from an older generation are dropped, so a late reply
 * can never display another account's or project's data. One mutation runs
 * at a time per view; reads of other pages stay possible.
 */
export function useTask(scope: string) {
  const generation = useRef(0);
  const running = useRef<string | null>(null);
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    generation.current++;
    running.current = null;
    setPending(null);
    setError("");
    return () => {
      generation.current++;
    };
  }, [scope]);

  /**
   * Runs `work` unless another task is in flight. `live()` tells whether the
   * view still shows the same scope; check it before applying a result.
   * Returns true when the work completed in the same scope.
   */
  const run = useCallback(async (key: string, work: (live: () => boolean) => Promise<void>, context?: string) => {
    if (running.current) return false;
    const current = generation.current;
    const live = () => current === generation.current;
    running.current = key;
    setPending(key);
    setError("");
    try {
      await work(live);
      return live();
    } catch (cause) {
      if (live()) setError(describeError(cause, context));
      return false;
    } finally {
      if (live()) {
        running.current = null;
        setPending(null);
      }
    }
  }, []);

  /** Starts a fresh generation without changing scope, e.g. before a reload. */
  const invalidate = useCallback(() => {
    generation.current++;
    running.current = null;
    setPending(null);
  }, []);

  return { run, pending, busy: pending !== null, error, setError, invalidate };
}

export type Task = ReturnType<typeof useTask>;
