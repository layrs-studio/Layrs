import type { Snapshot, WorktreeViewState } from "./workspace";

export type WorktreeRecord = {
  worktreeId: string;
  checkoutRoot: string;
  activeOperationId: string;
};
export type WorktreeList = { currentWorktreeId: string; worktrees: WorktreeRecord[] };
export type WorktreeReply = { path: string; worktreeId: string; state: Snapshot["state"] };

export function currentWorktree(state: Snapshot["state"]): WorktreeViewState | undefined {
  // Other Worktrees in this View are historical observations, not their current state.
  return state.view.worktrees.find(item => item.worktreeId === state.worktree.worktreeId);
}

export function createWorktreeRequest(path: string, layer: string, step: string) {
  if (!path.trim()) throw new Error("Choisissez un dossier vide pour le nouveau Worktree.");
  if (!layer.trim()) throw new Error("Choisissez explicitement un Layer existant.");
  if (!/^blake3:[a-f0-9]{64}$/.test(step.trim())) throw new Error("Indiquez l’identifiant exact de la Step de départ (blake3:…).");
  return { action: "createWorktree", path: path.trim(), layer: layer.trim(), step: step.trim() };
}

/** Use the engine's validated destination, never a speculative form value. */
export function worktreeDestination(reply: WorktreeReply): { path: string; view: "changes" | "health" } {
  if (!reply?.path || !reply.worktreeId || reply.state?.worktree?.worktreeId !== reply.worktreeId || reply.state.worktree.checkoutRoot !== reply.path) {
    throw new Error("La réponse du Worktree ne correspond pas au dossier validé.");
  }
  const own = currentWorktree(reply.state);
  if (!own || !["ready", "conflicted", "stale", "needs_repair"].includes(own.status)) throw new Error("État du Worktree inconnu. Actualisez avant de l’ouvrir.");
  return { path: reply.path, view: own.status === "stale" || own.status === "needs_repair" ? "health" : "changes" };
}
