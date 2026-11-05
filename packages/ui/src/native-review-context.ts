/** A notification identifies an immutable proposal version, not "latest". */
export type NativeReviewContext = { reviewId: string; version: number };
export function validReviewContext(value: unknown): value is NativeReviewContext {
  if (!value || typeof value !== "object") return false;
  const context = value as NativeReviewContext;
  return typeof context.reviewId === "string" && context.reviewId.length <= 256
    && /^review:[A-Za-z0-9:._-]+$/.test(context.reviewId)
    && Number.isSafeInteger(context.version) && context.version >= 1 && context.version <= 10001;
}
/** Missing historical data is a hard failure; it must never fall back to latest. */
export function exactReviewVersion<T extends NativeReviewContext>(versions: T[], requested: NativeReviewContext): T {
  if (!validReviewContext(requested)) throw new Error("Contexte de revue invalide.");
  const matching = versions.filter(version => version.reviewId === requested.reviewId && version.version === requested.version);
  if (matching.length !== 1) throw new Error("La version exacte de cette notification est indisponible.");
  return matching[0]!;
}
