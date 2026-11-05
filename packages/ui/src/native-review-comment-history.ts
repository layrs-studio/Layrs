export type ReviewCommentAction = {
  actionId: string; version: number; action: string; body: string; actorPrincipalId: string; createdAt: string;
  replyTo?: string | null; anchor?: { path: string; startLine: number; endLine: number } | null;
};
export type CommentCorrection = {
  evidenceObjectId: string;
  record: {
    schemaVersion: string; correctionId: string; reviewId: string; version: number; actionId: string; revision: number;
    body: string; reason: string; actorPrincipalId: string; correctedAt: string;
    originalActionObjectId: string; previousCorrectionObjectId: string | null;
  };
};
/** Rendering integrity only; canonical Evidence admission remains server-side. */
export function commentHistory(reviewId: string, action: ReviewCommentAction, all: CommentCorrection[]) {
  const corrections = all.filter(item => item.record.actionId === action.actionId).sort((a, b) => a.record.revision - b.record.revision);
  if (corrections.length > 100) throw new Error("Historique de corrections trop volumineux.");
  let previous: string | null = null;
  let original: string | null = null;
  for (const [index, item] of corrections.entries()) {
    const record = item.record;
    if (action.action !== "comment" || record.schemaVersion !== "layrs.review-comment-correction.v1"
      || record.reviewId !== reviewId || record.version !== action.version || record.revision !== index + 1
      || record.actorPrincipalId !== action.actorPrincipalId || record.previousCorrectionObjectId !== previous
      || typeof record.body !== "string" || typeof record.reason !== "string" || typeof record.correctedAt !== "string"
      || typeof item.evidenceObjectId !== "string" || !item.evidenceObjectId.startsWith("blake3:")
      || typeof record.originalActionObjectId !== "string" || !record.originalActionObjectId.startsWith("blake3:")
      || (original !== null && original !== record.originalActionObjectId)) throw new Error("Historique de corrections incohérent.");
    original = record.originalActionObjectId; previous = item.evidenceObjectId;
  }
  return { body: corrections.at(-1)?.record.body ?? action.body, revision: corrections.length, corrections };
}
