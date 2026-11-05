export type GitTarget = { path: string; storeId: string; spaceId: string; worktreeId: string; operationId: string; viewId: string };
export type GitExportMode = { kind: "history" } | { kind: "selected_history"; references: string[] } | { kind: "snapshot"; stepId: string; reference: string };
export type GitImportReport = { importId: string; objectFormat: string; commits: number; sourceBytes: number; warnings: string[]; references: { gitRef: string; gitId: string; stepId: string }[] };
export type GitImportReview = { reviewId: string; review: { target: GitTarget; checkpoint: string; digest: string; report: GitImportReport } };
export type GitExportReview = { reviewId: string; review: { target: GitTarget; destination: string; mode: GitExportMode; contentPreview: "unavailable" } };
export type GitExportResult = { destination: string; export: { commits: Record<string, string>; references: Record<string, string>; warnings: string[] } };

/** Only list formatting lives here; the bridge validates Git reference syntax. */
export function gitReferences(value: string): string[] {
  return value.split(/\r?\n/).map(value => value.trim()).filter(Boolean);
}
export function gitExportMode(kind: GitExportMode["kind"], references: string, step: string, reference: string): GitExportMode {
  if (kind === "history") return { kind };
  if (kind === "selected_history") {
    const selected = gitReferences(references);
    if (!selected.length) throw new Error("Indiquez au moins une référence Git complète.");
    return { kind, references: selected };
  }
  if (kind !== "snapshot") throw new Error("Mode d’export Git non pris en charge.");
  if (!/^blake3:[a-f0-9]{64}$/.test(step.trim())) throw new Error("Indiquez une Step exacte pour l’export sans historique.");
  if (!reference.trim()) throw new Error("Indiquez la référence Git de destination.");
  return { kind, stepId: step.trim(), reference: reference.trim() };
}
function validExportMode(value: unknown): value is GitExportMode {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const mode = value as Record<string, unknown>;
  const keys = mode.kind === "history" ? ["kind"] : mode.kind === "selected_history" ? ["kind", "references"] : mode.kind === "snapshot" ? ["kind", "stepId", "reference"] : null;
  if (!keys || Object.keys(mode).length !== keys.length || !keys.every(key => Object.hasOwn(mode, key))) return false;
  if (mode.kind === "selected_history") return Array.isArray(mode.references) && mode.references.length > 0 && Array.from(mode.references).every(ref => typeof ref === "string" && ref.trim().length > 0);
  if (mode.kind === "snapshot") return typeof mode.stepId === "string" && /^blake3:[a-f0-9]{64}$/.test(mode.stepId) && typeof mode.reference === "string" && mode.reference.trim().length > 0;
  return true;
}
function sameExportMode(left: unknown, right: unknown): boolean {
  if (!validExportMode(left) || !validExportMode(right) || left.kind !== right.kind) return false;
  if (left.kind === "snapshot") return right.kind === "snapshot" && left.stepId === right.stepId && left.reference === right.reference;
  if (left.kind === "selected_history") return right.kind === "selected_history" && left.references.length === right.references.length && left.references.every((ref, index) => ref === right.references[index]);
  return true;
}
export function gitImportRequest(review: GitImportReview, checkpoint: string, digest: string, worktreeId: string, operationId: string) {
  if (review.review.checkpoint !== checkpoint || review.review.digest !== digest || review.review.target.worktreeId !== worktreeId || review.review.target.operationId !== operationId) {
    throw new Error("Le checkpoint ou la cible a changé. Vérifiez de nouveau avant d’importer.");
  }
  return { action: "applyImport", reviewId: review.reviewId, checkpoint, digest };
}
export function gitExportRequest(review: GitExportReview, destination: string, mode: GitExportMode, worktreeId: string, operationId: string, resume: boolean) {
  if (review.review.destination !== destination || !sameExportMode(review.review.mode, mode) || review.review.target.worktreeId !== worktreeId || review.review.target.operationId !== operationId) {
    throw new Error("La destination, les références ou la cible a changé. Vérifiez de nouveau avant d’exporter.");
  }
  return { action: resume ? "resumeExport" : "export", reviewId: review.reviewId, destination, mode };
}
