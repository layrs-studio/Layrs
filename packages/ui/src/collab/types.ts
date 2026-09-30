/*
 * Data shapes of the V3 collaboration contracts as the pages read them. The
 * transport stays abstract: Web maps commands to HTTPS routes, Desktop to its
 * native remote bridge. Nothing here knows about tokens or storage.
 */
import type { SpaceCapabilityFlags } from "../api/catalog-types";
import type { CommentCorrection, ReviewCommentAction } from "../native-review-comment-history";

export type CollabCommand = { action: string; [key: string]: unknown };
export type CollabTransport = <T>(command: CollabCommand) => Promise<T>;

/**
 * Capability hints for one project (L0 contract). Absent means unknown: the
 * action stays offered and the server decides. `false` hides or disables it
 * with a reason; the server still rechecks every request.
 */
export type CollabCapabilities = Partial<SpaceCapabilityFlags>;

/** Named references of a project, read from its bootstrap. */
export type ProjectReferences = {
  layers: { layerId: string; name: string; heads: string[] }[];
  markers: { markerId: string; name: string; title: string; notes: string; targetStepId: string; createdAt: string }[];
  releases: { releaseId: string; version: string; title: string; notes: string; targetStepId: string; createdAt: string }[];
  /** The server declared a partial (restricted) content access for this account. */
  restricted: boolean;
};

/** Who is looking, and the names the client is allowed to know. */
export type PeopleDirectory = {
  currentPrincipalId?: string;
  names?: Record<string, string>;
};

export type ReviewProposal = {
  spaceId: string;
  reviewId: string;
  version: number;
  title: string;
  proposedStepId: string;
  layerId: string;
  authorPrincipalId: string;
  participants: string[];
  createdAt?: string;
};

export type DecisionState = {
  state: string;
  approvals: number;
  requiredApprovals: number;
  blockingDecisions: number;
  stale: boolean;
};

export type ReviewSummary = { lifecycle: string; proposal: ReviewProposal; decisionState?: DecisionState };

export type ReviewIntegration = {
  evidenceObjectId: string;
  record: { version: number; transactionId: string; integratedAt: string; integratedBy: string };
};

export type ReviewDetailData = {
  lifecycle: string;
  decisionState?: DecisionState;
  integration?: ReviewIntegration | null;
  versions: ReviewProposal[];
  actions: ReviewCommentAction[];
  commentCorrections?: CommentCorrection[];
};

export type RunSummary = {
  runId: string;
  stepId: string;
  outcome: string | null;
  proofObjectId: string | null;
  runnerPrincipalId?: string;
  recipeObjectId?: string;
  createdAt?: string;
  expiresAt?: string;
  completedAt?: string | null;
  status?: "queued" | "running" | "expired" | "completed" | "cancelled";
  cancellation?: {
    evidenceObjectId: string;
    record: { cancelledBy: string; cancelledAt: string; reason: string };
  } | null;
};

export type RunDetailData = RunSummary & {
  runnerPrincipalId: string;
  recipeObjectId: string;
  evidence: { schemaVersion: string; log?: string }[];
};

export type HistoryStep = { stepId: string; parentStepIds: string[]; rootTreeId: string };
export type HistoryObservation = {
  observation: { observationId: string; actorId: string | null; observedAt: string; captureReason: string };
  provenance: "server_admission" | "client_assertion";
  principalId: string;
  receivedAt: string;
};
export type HistoryFilterRequest = { actorId: string | null; since: string | null; until: string | null };
export type HistoryPageData = {
  anchor: string;
  steps: HistoryStep[];
  next: string | null;
  path?: string | null;
  filter: HistoryFilterRequest;
  observations: Record<string, HistoryObservation[]>;
};

export type CompareChange = {
  path: string;
  status: string;
  binary: boolean;
  textOmitted: boolean;
  beforeBytes: number | null;
  afterBytes: number | null;
  textDiff: { op: string; text: string }[] | null;
};

export type ReleaseEntry = {
  record: { releaseId: string; version: string; targetStepId: string; title?: string; notes?: string; createdAt?: string };
};
export type PublicationEvent = {
  eventObjectId: string;
  event: { kind: "published" | "revoked"; occurredAt: string; actorId: string; reason?: string; publicationId?: string };
};
export type PublicationHistory = {
  releaseId: string;
  lastEventObjectId: string | null;
  activePublicationId: string | null;
  events: PublicationEvent[];
};
