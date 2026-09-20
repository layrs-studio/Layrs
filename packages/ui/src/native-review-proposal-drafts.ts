import { createDraftStore, type LocalDraft, type DraftStorage } from "./native-draft-storage";
import { draftIdentityScope, type ReviewDraftIdentity } from "./native-review-drafts";
export type ProposalTarget = { reviewId: string | null; expectedVersion: number };
export type ProposalDraftScope = ReviewDraftIdentity & ProposalTarget & { spaceId: string };
export type ProposalDraftInput = { title: string; layerId: string; stepId: string; participants: string };
export type ProposalDraft = LocalDraft<ProposalDraftInput>;
export const PROPOSAL_DRAFT_STORAGE_KEY = "layrs.review-proposal-drafts.v1";
export function proposalDraftScope(value: ProposalDraftScope): ProposalDraftScope {
  if ((value.reviewId === null && value.expectedVersion !== 0) || (value.reviewId !== null && (typeof value.reviewId !== "string" || value.reviewId.length > 256 || !/^review:[A-Za-z0-9:._-]+$/.test(value.reviewId) || !Number.isSafeInteger(value.expectedVersion) || value.expectedVersion < 1 || value.expectedVersion > 10000))) throw new Error("Cible de proposition invalide.");
  const identity = draftIdentityScope(value);
  return { origin: identity.origin, principalId: identity.principalId, spaceId: identity.spaceId, reviewId: value.reviewId, expectedVersion: value.expectedVersion };
}
function input(value: ProposalDraftInput): ProposalDraftInput {
  for (const [field, maximum] of [["title", 1024], ["layerId", 256], ["stepId", 256], ["participants", 32768]] as const) {
    if (typeof value[field] !== "string" || value[field].length > maximum || value[field].includes("\0")) throw new Error("Brouillon de proposition invalide ou trop volumineux.");
  }
  return { title: value.title, layerId: value.layerId, stepId: value.stepId, participants: value.participants };
}
const store = createDraftStore(PROPOSAL_DRAFT_STORAGE_KEY, proposalDraftScope, input);
export const readProposalDraft = store.read;
export const saveProposalDraft = store.save;
export const removeProposalDraft = store.remove;
/** Metadata only, restricted to this authenticated account/Space/review. */
export function proposalDraftTargets(storage: DraftStorage, scope: ReviewDraftIdentity & { spaceId: string }, reviewId: string, now = Date.now()): ProposalTarget[] {
  const identity = draftIdentityScope(scope);
  return store.scopes(storage, now).filter(item => item.origin === identity.origin && item.principalId === identity.principalId && item.spaceId === identity.spaceId && item.reviewId === reviewId)
    .map(item => ({ reviewId: item.reviewId, expectedVersion: item.expectedVersion })).sort((a, b) => b.expectedVersion - a.expectedVersion);
}
/** Called only by an explicit submit. Saving partial IDs remains possible. */
export function proposalRequest(target: ProposalTarget, value: ProposalDraftInput, idempotencyKey: string) {
  const clean = input(value);
  const participants = clean.participants.split(",").map(item => item.trim()).filter(Boolean);
  if (!clean.title.trim() || new TextEncoder().encode(clean.title).byteLength > 1024
    || !/^layer:[A-Za-z0-9:._-]+$/.test(clean.layerId) || !/^blake3:[a-f0-9]{64}$/.test(clean.stepId)
    || participants.length > 100 || participants.some(id => id.length > 256 || !/^principal:[A-Za-z0-9:._-]+$/.test(id))
    || !/^[A-Za-z0-9_.:-]{16,128}$/.test(idempotencyKey)
    || !Number.isSafeInteger(target.expectedVersion) || (target.reviewId === null ? target.expectedVersion !== 0 : target.expectedVersion < 1 || target.expectedVersion > 10000 || !/^review:[A-Za-z0-9:._-]+$/.test(target.reviewId))) throw new Error("Vérifiez le titre, les identifiants et les participants de la proposition.");
  return { layerId: clean.layerId, proposedStepId: clean.stepId, title: clean.title, participants, expectedVersion: target.expectedVersion, idempotencyKey };
}
/** Once attempted, a request keeps its actual key even if storage later fails. */
export function prepareProposalAttempt(context: string, target: ProposalTarget, input: ProposalDraftInput, persistedKey: string | null, previous: { signature: string; key: string } | null, newKey = () => crypto.randomUUID()) {
  const signature = JSON.stringify([context, target, input]);
  const key = previous?.signature === signature ? previous.key : persistedKey ?? newKey();
  return { signature, key, request: proposalRequest(target, input, key) };
}
