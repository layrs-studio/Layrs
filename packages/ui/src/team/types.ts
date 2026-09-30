/*
 * Shapes of the administration contracts used by the team, access, settings,
 * inbox and instance pages. Components receive transports as props so Web
 * (HTTP) and Desktop (Tauri) can both drive them; nothing here authorizes.
 */

export type TeamCommand = { action: string; [key: string]: unknown };
/** `nativeAdminRoute` commands (Web) or the Desktop `administration` bridge. */
export type TeamTransport = <T>(command: TeamCommand) => Promise<T>;
/**
 * readPolicyRules, previewPolicyRules, previewReplacePolicyRules,
 * previewPolicy, applyPolicy and collaboration/projection reads.
 */
export type PolicyTransport = <T>(command: TeamCommand) => Promise<T>;

export type Person = { principalId: string; displayName: string; kind?: string };

export type Account = { principalId: string; displayName: string; kind: string; status: string };

export type MemberRole = "member" | "auditor" | "release_manager" | "automation" | "revoked";
export type TeamRole = "member" | "auditor" | "release_manager";

export type Team = {
  teamId: string;
  version: number;
  name: string;
  role: TeamRole;
  memberPrincipalIds: string[];
  state: string;
  eventObjectId: string;
  updatedAt: string;
  changedBy: string;
  reason?: string | null;
  policyProductObjectId?: string | null;
};

export type TeamHistoryPage = {
  events: { eventObjectId: string; record: Team }[];
  anchorVersion: number;
  nextAfter: number;
  hasMore: boolean;
};

export type PolicyManagement = { mode: "managed" | "custom"; currentPolicySetId: string; managedPolicySetId: string };

export type Invitation = {
  invitationId: string;
  spaceId: string;
  inviterPrincipalId: string;
  recipientPrincipalId: string;
  role: string;
  state: string;
  createdAt?: string;
  expiresAt: string;
  decidedAt?: string | null;
  expired: boolean;
  stale: boolean;
};
export type InvitationPage = { invitations: Invitation[]; nextAfter: number | null };

export type Notice = {
  sequence: number;
  kind: string;
  subjectId: string;
  actorPrincipalId?: string;
  createdAt: string;
  reviewContext?: { reviewId: string; version: number } | null;
};
export type NoticePage = { notifications: Notice[]; nextAfter: number; hasMore: boolean; lastReadSequence: number };

export type Webhook = { webhookId: string; url: string; eventKinds: string[]; secretGeneration: number; enabled: boolean; createdAt?: string };
export type Delivery = {
  deliveryId: string;
  sequence?: number;
  state: string;
  attempts: number;
  lastStatus: number | null;
  lastError: string | null;
  nextAttemptAt?: string;
};

export type Protection = {
  minimumApprovals: number;
  allowSelfApproval: boolean;
  rejectConflicts: boolean;
  recipeObjectId: string | null;
  trustedRunnerPrincipalIds: string[];
  maximumProofAgeSeconds: number;
  configuredBy?: string;
  configuredAt?: string;
};

export type Token = { tokenId: string; label: string; kind: string; expiresAt: string; revoked: boolean };

export type PolicyRulesRead = {
  principalId: string;
  spaceId: string;
  policySetId: string;
  authorizationEpoch: number;
  policies: { policyId: string; policyVersionId: string; policyProductObjectId: string; rules: unknown }[];
};

/** Common fields of the Web and Desktop policy previews. */
export type PolicyPreview = {
  spaceId: string;
  expectedPolicySetId: string;
  expectedAuthorizationEpoch: number;
  proposedPolicySetId: string;
  inlineRules?: { rules: unknown; replacement?: { policyId: string; previousProductObjectId: string } } | null;
};
export type PolicyReceipt = { status: string; rejections?: { code: string; message?: string }[] };

export type LayerRef = { layerId: string; name: string; headStepIds: string[] };

export const memberRoles: Record<MemberRole, { label: string; description: string }> = {
  member: { label: "Membre", description: "Consulter, publier et participer aux revues." },
  auditor: { label: "Lecture seule", description: "Consulter le projet, son historique et les revues." },
  release_manager: { label: "Gestion des versions", description: "Lire, publier et retirer des versions." },
  automation: { label: "Automatisation", description: "Lire le projet et déposer des preuves CI. Réservé aux comptes de service." },
  revoked: { label: "Accès retiré", description: "Interdit toute action sur ce projet, même accordée par une équipe." },
};

export const teamRoles: Record<TeamRole, string> = {
  member: "Membre",
  auditor: "Lecture seule",
  release_manager: "Gestion des versions",
};

/** Notification and webhook event kinds known by the server. */
export const eventLabels: Record<string, string> = {
  "review.proposed": "Nouvelle version de revue",
  "review.action": "Décision ou commentaire de revue",
  "review.integrated": "Revue intégrée",
  "review.comment_corrected": "Commentaire corrigé",
  "proof.completed": "Résultat de validation automatique",
  "proof.cancelled": "Validation automatique annulée",
  "protection.changed": "Protection d’un Layer modifiée",
  "membership.changed": "Accès d’une personne modifié",
  "team.changed": "Équipe modifiée",
  "invitation.created": "Invitation envoyée",
};

export const webhookEvents = [
  "review.proposed",
  "review.action",
  "review.integrated",
  "review.comment_corrected",
  "proof.completed",
  "proof.cancelled",
  "protection.changed",
  "membership.changed",
  "team.changed",
] as const;

export const invitationStates: Record<string, string> = {
  pending: "En attente",
  accepted: "Acceptée",
  rejected: "Refusée",
  revoked: "Révoquée",
};

export const accountKinds: Record<string, string> = {
  bootstrap_admin: "Administrateur de l’instance",
  account: "Compte personnel",
  service: "Compte de service",
};

/** Readable name for a principal, falling back to a shortened reference. */
export function personName(people: Person[] | Map<string, Person>, id: string) {
  const found = people instanceof Map ? people.get(id) : people.find((person) => person.principalId === id);
  return found?.displayName || `Compte ${id.replace(/^principal:/, "").slice(0, 8)}…`;
}
