/** Device-local comment drafts only. No transport, session, token or password. */
import { createDraftStore, DRAFT_MAX_BYTES as REVIEW_DRAFT_MAX_BYTES, type LocalDraft } from "./native-draft-storage";
export { DRAFT_MAX_BYTES as REVIEW_DRAFT_MAX_BYTES, DRAFT_TOTAL_BYTES as REVIEW_DRAFT_TOTAL_BYTES, DRAFT_MAX_ENTRIES as REVIEW_DRAFT_MAX_ENTRIES, DRAFT_TTL_MS as REVIEW_DRAFT_TTL_MS } from "./native-draft-storage";
export type { DraftStorage } from "./native-draft-storage";
export type ReviewDraftIdentity = { origin: string; principalId: string };
export type ReviewDraftScope = ReviewDraftIdentity & { spaceId: string; reviewId: string; version: number };
export type ReviewDraftInput = { body: string; path: string; line: number; replyTo: string | null };
export type ReviewCommentDraft = LocalDraft<ReviewDraftInput>;
export const REVIEW_DRAFT_STORAGE_KEY = "layrs.review-comment-drafts.v1";
function fail(message: string): never { throw new Error(message); }
function text(value: unknown, maximum: number): value is string { return typeof value === "string" && value.length <= maximum; }
function id(value: unknown, prefix: string): value is string { return text(value, 256) && value.startsWith(prefix) && /^[A-Za-z0-9:._-]+$/.test(value); }

export function draftIdentityScope(scope: ReviewDraftIdentity & { spaceId: string }) {
  let origin: URL;
  try { origin = new URL(scope.origin); } catch { return fail("Contexte de brouillon invalide."); }
  if (origin.protocol !== "https:" || origin.username || origin.password || origin.pathname !== "/" || origin.search || origin.hash
    || !id(scope.principalId, "principal:") || !id(scope.spaceId, "space:")) fail("Contexte de brouillon invalide.");
  return { origin: origin.origin, principalId: scope.principalId, spaceId: scope.spaceId };
}
export function reviewDraftScope(scope: ReviewDraftScope): ReviewDraftScope {
  const identity = draftIdentityScope(scope);
  if (!id(scope.reviewId, "review:") || !Number.isSafeInteger(scope.version) || scope.version < 1 || scope.version > 10001) fail("Contexte de brouillon invalide.");
  return { ...identity, reviewId: scope.reviewId, version: scope.version };
}
function input(value: ReviewDraftInput): ReviewDraftInput {
  if (!text(value.body, REVIEW_DRAFT_MAX_BYTES) || !text(value.path, 4096) || !Number.isSafeInteger(value.line) || value.line < 1 || value.line > 2000000
    || (value.replyTo !== null && !id(value.replyTo, "review-action:"))) fail("Le brouillon dépasse les limites de texte, de chemin ou de ligne.");
  return { body: value.body, path: value.path, line: value.line, replyTo: value.replyTo };
}
const store = createDraftStore(REVIEW_DRAFT_STORAGE_KEY, reviewDraftScope, input);
export const readReviewDraft = store.read;
export const saveReviewDraft = store.save;
export const removeReviewDraft = store.remove;
