import { useEffect, useState } from "react";
import { nativeWorkspace } from "../tauri";
import type { FileChange } from "../app/workspace";

type Observation = { observedAt: string; captureReason: string; worktreeId?: string | null };
type History = { steps: { stepId: string; parentStepIds: string[] }[]; observations: Record<string, Observation[]> };
type Capture = { parents: string[]; observation?: Observation; changes?: FileChange[]; error?: string };

/** Reads immutable Steps; a response from another Layer or folder is never displayed. */
export function useCapturedChanges(root: string, worktreeId: string | undefined, layerId: string | null | undefined, step: string | null | undefined) {
  const identity = JSON.stringify([root, worktreeId, layerId, step]);
  const [base, setBase] = useState<{ identity: string; parent: string } | null>(null);
  const parent = base?.identity === identity ? base.parent : "";
  const key = JSON.stringify([identity, parent]);
  const [result, setResult] = useState<{ key: string; value: Capture } | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    if (!step) return;
    let active = true;
    async function read() {
      try {
        const history = await nativeWorkspace<History>(root, { action: "stepHistory", selector: step, cursor: null, limit: 1, path: null });
        if (!active) return;
        const head = history.steps.find((item) => item.stepId === step);
        if (!head) throw new Error("La capture de ce dossier n’est plus disponible.");
        const observation = history.observations[step!]?.find((item) => item.worktreeId === worktreeId);
        const value: Capture = { parents: head.parentStepIds, observation };
        if (head.parentStepIds.length > 1 && !head.parentStepIds.includes(parent)) {
          setResult({ key, value });
          return;
        }
        const diff = await nativeWorkspace<{ stepId: string; changes: FileChange[] }>(root, { action: "stepDiff", step, parent: parent || null });
        if (!active) return;
        if (diff.stepId !== step) throw new Error("La comparaison ne correspond pas à la capture demandée.");
        setResult({ key, value: { ...value, changes: diff.changes } });
      } catch (cause) {
        if (active) setResult({ key, value: { parents: [], error: String(cause) } });
      }
    }
    void read();
    return () => { active = false; };
  }, [key, attempt]);
  const value = step && result?.key === key ? result.value : undefined;
  return { ...value, loading: Boolean(step && !value), parent, setParent: (next: string) => setBase({ identity, parent: next }), retry: () => setAttempt((current) => current + 1) };
}
