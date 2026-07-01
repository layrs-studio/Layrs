import { normalizeDiffColumnWindow } from "./lenses/diff";
import type {
  DiffColumnWindow,
  DiffLineWindow,
  DiffModel,
  DiffModelMetadata,
  PreviewKind,
  PreviewModel
} from "./lenses/contracts";

export type LayrsId = string;

export type GateStatus = "passing" | "blocked" | "needs-proof" | "pending";
export type LayerKind = "base" | "proposal" | "experiment" | "release";
export type LayerStatus = "stable" | "active" | "review" | "archived";
export type ArtifactType = "file" | "note" | "image" | "report" | "proof" | "step-output";
export type ProofStatus = "accepted" | "missing" | "stale" | "reviewing";
export type PolicyEffect = "allow" | "require-proof" | "block" | "notify";
export type AccountRole = "owner" | "admin" | "member";
export type AuthSessionState = "anonymous" | "authenticated" | "onboarding";
export type AccessMode = "none" | "read" | "write" | "admin";
export type AccessSubjectKind = "account" | "team";
export type DeviceFlowStatus = "pending" | "approved" | "expired" | "denied";
export type DeviceStatus = "trusted" | "pending" | "revoked";
export type WorkspaceMemberRole = "owner" | "admin" | "member" | "viewer";
export type TeamMemberRole = "maintainer" | "member" | "viewer";
export type InvitationStatus = "pending" | "accepted" | "declined" | "expired";
export type LayerAccessRuleMode = "inherit_layer" | "restricted" | "reserved_redacted";
export type LayerAccessVisibility = "full" | "stub";

export interface Account {
  id: LayrsId;
  email: string;
  name: string;
  role: AccountRole;
  avatarInitials: string;
  createdAt: string;
}

export interface Session {
  id: LayrsId;
  accountId: LayrsId;
  activeWorkspaceId?: LayrsId;
  expiresAt: string;
  createdAt: string;
}

export interface AuthSessionResponse {
  state: AuthSessionState;
  account?: Account;
  session?: Session;
  workspaces: Workspace[];
  activeWorkspaceId?: LayrsId;
}

export interface Workspace {
  id: LayrsId;
  name: string;
  slug: string;
  description: string;
  health: GateStatus;
  updatedAt: string;
}

export interface Team {
  id: LayrsId;
  workspaceId: LayrsId;
  name: string;
  purpose: string;
  members: number;
  gateResponsibility: string;
}

export interface WorkspaceMember {
  id: LayrsId;
  workspaceId: LayrsId;
  accountId: LayrsId;
  email: string;
  name: string;
  role: WorkspaceMemberRole;
  teamIds: LayrsId[];
  createdAt: string;
  updatedAt: string;
}

export interface TeamMember {
  id: LayrsId;
  workspaceId: LayrsId;
  teamId: LayrsId;
  accountId: LayrsId;
  email: string;
  name: string;
  role: TeamMemberRole;
  createdAt: string;
}

export interface Invitation {
  id: LayrsId;
  workspaceId: LayrsId;
  email: string;
  role: WorkspaceMemberRole;
  teamIds: LayrsId[];
  status: InvitationStatus;
  createdAt: string;
  expiresAt?: string;
}

export interface Space {
  id: LayrsId;
  workspaceId: LayrsId;
  teamId: LayrsId;
  name: string;
  description: string;
  status: GateStatus;
  currentLayerId: LayrsId;
  updatedAt: string;
}

export interface Layer {
  id: LayrsId;
  spaceId: LayrsId;
  parentId?: LayrsId;
  name: string;
  kind: LayerKind;
  status: LayerStatus;
  summary: string;
  artifactIds: LayrsId[];
  stepIds: LayrsId[];
  gateIds: LayrsId[];
  rootTreeId?: LayrsId;
  policyEpoch?: number;
  serverCursor?: string;
  head?: LayerHeadMetadata;
}

export interface Artifact {
  id: LayrsId;
  spaceId: LayrsId;
  layerId?: LayrsId;
  name: string;
  type: ArtifactType;
  summary: string;
  location: string;
  updatedAt: string;
  sizeLabel: string;
  proofIds: LayrsId[];
  access?: ArtifactAccessDecision;
  mediaType?: string;
  contentHash?: string;
  byteLen?: number;
  lensId?: string;
  rootTreeId?: LayrsId;
  currentTreeId?: LayrsId;
  fileObjectId?: LayrsId;
  fileObject?: FileObjectMetadata;
  chunks?: ChunkMetadata[];
  preview?: Partial<PreviewModel>;
}

export interface LayerHeadMetadata {
  layerStateId?: LayrsId;
  rootTreeId?: LayrsId;
  policyEpoch?: number;
  serverCursor?: string;
  updatedAt?: string;
  updatedByAccountId?: LayrsId;
}

export interface ChunkMetadata {
  id: LayrsId;
  chunkId: LayrsId;
  sha256?: string;
  sizeBytes?: number;
  byteOffset?: number;
  chunkIndex?: number;
  mediaType?: string;
  compression?: string;
  state?: string;
  objectKey?: string;
  value?: string;
  encoding?: string;
}

export interface FileObjectMetadata {
  id: LayrsId;
  fileObjectId: LayrsId;
  sha256?: string;
  sizeBytes?: number;
  mediaType?: string;
  chunkCount?: number;
  chunks: ChunkMetadata[];
}

export interface ArtifactResolvedContent {
  encoding?: string;
  mediaType: string;
  sha256?: string;
  value?: string;
  bytes?: Uint8Array;
  base64?: string;
  dataUrl?: string;
  fileObject?: FileObjectMetadata;
  chunks: ChunkMetadata[];
  storage?: string;
}

export interface ArtifactContentPayload {
  artifactId?: LayrsId;
  workspaceId?: LayrsId;
  spaceId?: LayrsId;
  layerId?: LayrsId;
  path?: string;
  type?: ArtifactType;
  content: ArtifactResolvedContent;
  source?: Record<string, unknown>;
  fields: Record<string, unknown>;
}

export interface ArtifactWindowMetadata {
  start: number;
  limit: number;
  count: number;
  totalLines?: number;
  hasMore: boolean;
  hasMoreBefore?: boolean;
  hasMoreAfter?: boolean;
  columnStart?: number;
  columnLimit?: number;
  hasLongLines?: boolean;
}

export interface ArtifactPreviewWindowPayload {
  artifactId?: LayrsId;
  workspaceId?: LayrsId;
  spaceId?: LayrsId;
  layerId?: LayrsId;
  baseLayerId?: LayrsId;
  path?: string;
  type?: ArtifactType;
  preview?: PreviewModel;
  diff?: DiffModel;
  window: ArtifactWindowMetadata;
  source?: Record<string, unknown>;
  fields: Record<string, unknown>;
}

export interface LayerStepDiffStats {
  files: number;
  additions: number;
  deletions: number;
  removals: number;
}

export interface LayerStep {
  id: LayrsId;
  stepId: LayrsId;
  layerId: LayrsId;
  baseLayerId?: LayrsId;
  baseTreeId?: LayrsId;
  rootTreeId?: LayrsId;
  capturedAt?: number;
  startedAt?: string;
  completedAt?: string;
  changedFiles: number;
  diffStats: LayerStepDiffStats;
  files: StepChangedFile[];
  diffs: StepDiffWindow[];
  fields: Record<string, unknown>;
}

export interface StepChangedFile {
  path: string;
  name: string;
  action: "added" | "modified" | "deleted" | "missing" | string;
  lensId?: string;
  mediaType?: string;
  baseLayerId?: LayrsId;
  baseFileObjectId?: LayrsId;
  targetFileObjectId?: LayrsId;
  sizeBytes?: number;
  access?: ArtifactAccessDecision;
}

export interface StepDiffWindow {
  path: string;
  state?: string;
  lensId?: string;
  title: string;
  diff: DiffModel;
  message?: string;
  source?: string;
  layerId?: LayrsId;
  stepId?: LayrsId;
  lineWindow?: DiffLineWindow;
  columnWindow?: DiffColumnWindow;
  totalLineCount?: number;
  totalDiffLineCount?: number;
  renderedLineCount?: number;
  windowStart?: number;
  windowEnd?: number;
  windowLimit?: number;
  hasMoreBefore: boolean;
  hasMoreAfter: boolean;
  hasMoreColumns: boolean;
  fields: Record<string, unknown>;
}

export interface ArtifactAccessDecision {
  mode: AccessMode;
  canOpen: boolean;
  isRedacted: boolean;
  reason?: string;
}

export interface Step {
  id: LayrsId;
  spaceId: LayrsId;
  layerId: LayrsId;
  name: string;
  status: GateStatus;
  actor: "human" | "automation";
  startedAt: string;
  completedAt?: string;
  artifactIds: LayrsId[];
  proofIds: LayrsId[];
}

export interface WeaveDiffStats {
  files: number;
  additions: number;
  removals: number;
}

export interface WeaveEvent {
  id: LayrsId;
  weaveId: LayrsId;
  kind: "intent" | "change" | "decision" | "comment" | "proof" | "artifact";
  title: string;
  actor: string;
  at: string;
  summary: string;
  diffStats?: WeaveDiffStats;
  artifactIds: LayrsId[];
  proofIds: LayrsId[];
}

export interface Weave {
  id: LayrsId;
  spaceId: LayrsId;
  layerId: LayrsId;
  title: string;
  status: GateStatus;
  events: WeaveEvent[];
}

export interface Proof {
  id: LayrsId;
  targetId: LayrsId;
  targetType: "layer" | "artifact" | "step" | "gate" | "policy";
  kind: "test" | "review" | "security" | "decision" | "snapshot";
  status: ProofStatus;
  title: string;
  evidence: string;
  createdAt: string;
  gateId?: LayrsId;
  artifactId?: LayrsId;
}

export interface Gate {
  id: LayrsId;
  spaceId: LayrsId;
  layerId: LayrsId;
  name: string;
  status: GateStatus;
  ownerTeamId: LayrsId;
  requiredProofKinds: Proof["kind"][];
  summary: string;
}

export interface PolicyRule {
  id: LayrsId;
  subject: "workspace" | "team" | "space" | "layer" | "step";
  action: string;
  effect: PolicyEffect;
  gateId?: LayrsId;
}

export interface Policy {
  id: LayrsId;
  scope: "workspace" | "team" | "space";
  targetId: LayrsId;
  name: string;
  appliesTo: string;
  effect: PolicyEffect;
  rules: PolicyRule[];
}

export interface TimelineItem {
  id: LayrsId;
  at: string;
  title: string;
  summary: string;
  status: GateStatus;
  relatedIds: LayrsId[];
}

export interface LayerAccessPrincipalSet {
  accounts: LayrsId[];
  teams: LayrsId[];
}

export interface LayerAccessRulePermissions {
  read: LayerAccessPrincipalSet;
  write: LayerAccessPrincipalSet;
  admin: LayerAccessPrincipalSet;
}

export interface LayerAccessSignature {
  keyId: string;
  value: string;
}

export interface LayerAccessRule {
  id: LayrsId;
  path: string;
  artifactId?: LayrsId;
  mode: LayerAccessRuleMode;
  visibility: LayerAccessVisibility;
  permissions: LayerAccessRulePermissions;
}

export interface LayerAccessPolicy {
  schema: string;
  workspaceId: LayrsId;
  spaceId: LayrsId;
  layerId: LayrsId;
  policyEpoch: number;
  generatedAt: string;
  rules: LayerAccessRule[];
  signature: LayerAccessSignature;
}

export interface LayerAccessRegistryRule {
  id: LayrsId;
  subjectKind: AccessSubjectKind;
  subjectId: LayrsId;
  subjectName: string;
  mode: AccessMode;
}

export type LegacyLayerAccessRule = LayerAccessRegistryRule;

export interface LayerAccessRegistry {
  id: LayrsId;
  workspaceId: LayrsId;
  layerId: LayrsId;
  rules: LayerAccessRegistryRule[];
  updatedAt: string;
}

export interface DeviceFlow {
  id: LayrsId;
  userCode: string;
  verificationUri: string;
  status: DeviceFlowStatus;
  expiresAt: string;
}

export interface Device {
  id: LayrsId;
  accountId: LayrsId;
  name: string;
  kind: "browser" | "desktop" | "cli";
  status: DeviceStatus;
  lastSeenAt: string;
}

export interface AuditEvent {
  id: LayrsId;
  workspaceId: LayrsId;
  actorAccountId: LayrsId;
  action: string;
  target: string;
  summary: string;
  at: string;
}

export interface StudioFixture {
  account: Account;
  session: Session;
  workspace: Workspace;
  workspaces: Workspace[];
  teams: Team[];
  spaces: Space[];
  layers: Layer[];
  artifacts: Artifact[];
  steps: Step[];
  weaves: Weave[];
  proofs: Proof[];
  gates: Gate[];
  policies: Policy[];
  timeline: TimelineItem[];
  layerAccessPolicies: LayerAccessPolicy[];
  accessRegistries: LayerAccessRegistry[];
  workspaceMembers: WorkspaceMember[];
  teamMembers: TeamMember[];
  invitations: Invitation[];
  devices: Device[];
  auditEvents: AuditEvent[];
}

export type StudioSnapshot = StudioFixture;

const account: Account = {
  id: "account-alexa",
  email: "alex@layrs.local",
  name: "Alex Martin",
  role: "owner",
  avatarInitials: "AM",
  createdAt: "2026-06-29T13:10:00Z"
};

const workspace: Workspace = {
  id: "workspace-layrs-labs",
  name: "Layrs Labs",
  slug: "layrs-labs",
  description: "Local-first workspace for product, code and proof-driven delivery.",
  health: "needs-proof",
  updatedAt: "2026-06-29T16:10:00Z"
};

const session: Session = {
  id: "session-studio-web",
  accountId: account.id,
  activeWorkspaceId: workspace.id,
  expiresAt: "2026-06-30T16:10:00Z",
  createdAt: "2026-06-29T16:10:00Z"
};

const teams: Team[] = [
  {
    id: "team-studio",
    workspaceId: workspace.id,
    name: "Studio",
    purpose: "Owns the operator experience across web and desktop.",
    members: 5,
    gateResponsibility: "UX and release readiness"
  },
  {
    id: "team-core",
    workspaceId: workspace.id,
    name: "Core",
    purpose: "Owns store, graph, policies and gates.",
    members: 4,
    gateResponsibility: "Durability and invariants"
  },
  {
    id: "team-automation",
    workspaceId: workspace.id,
    name: "Automation",
    purpose: "Owns Steps, Weaves and generated Artifacts.",
    members: 3,
    gateResponsibility: "Proof completeness"
  }
];

const workspaceMembers: WorkspaceMember[] = [
  {
    id: "membership-alexa",
    workspaceId: workspace.id,
    accountId: account.id,
    email: account.email,
    name: account.name,
    role: "owner",
    teamIds: ["team-studio", "team-core"],
    createdAt: "2026-06-29T13:12:00Z",
    updatedAt: "2026-06-29T16:10:00Z"
  },
  {
    id: "membership-core-reviewer",
    workspaceId: workspace.id,
    accountId: "account-core-reviewer",
    email: "core.reviewer@layrs.local",
    name: "Core Reviewer",
    role: "admin",
    teamIds: ["team-core"],
    createdAt: "2026-06-29T13:24:00Z",
    updatedAt: "2026-06-29T15:55:00Z"
  },
  {
    id: "membership-automation",
    workspaceId: workspace.id,
    accountId: "account-automation",
    email: "automation@layrs.local",
    name: "Automation Step",
    role: "member",
    teamIds: ["team-automation"],
    createdAt: "2026-06-29T13:31:00Z",
    updatedAt: "2026-06-29T16:05:00Z"
  }
];

const teamMembers: TeamMember[] = workspaceMembers.flatMap((member) =>
  member.teamIds.map((teamId) => ({
    id: `team-membership-${teamId}-${member.accountId}`,
    workspaceId: member.workspaceId,
    teamId,
    accountId: member.accountId,
    email: member.email,
    name: member.name,
    role: member.role === "owner" || member.role === "admin" ? "maintainer" : "member",
    createdAt: member.createdAt
  }))
);

const invitations: Invitation[] = [
  {
    id: "invitation-design-review",
    workspaceId: workspace.id,
    email: "design.review@layrs.local",
    role: "member",
    teamIds: ["team-studio"],
    status: "pending",
    createdAt: "2026-06-29T16:02:00Z",
    expiresAt: "2026-07-06T16:02:00Z"
  }
];

const spaces: Space[] = [
  {
    id: "space-studio",
    workspaceId: workspace.id,
    teamId: "team-studio",
    name: "Studio Experience",
    description: "Web and desktop shell for navigating Spaces, Layers, Weaves and Proofs.",
    status: "needs-proof",
    currentLayerId: "layer-studio-review",
    updatedAt: "2026-06-29T16:08:00Z"
  },
  {
    id: "space-policy",
    workspaceId: workspace.id,
    teamId: "team-core",
    name: "Policy Runtime",
    description: "Declarative controls that resolve Gates without renaming product concepts.",
    status: "passing",
    currentLayerId: "layer-policy-base",
    updatedAt: "2026-06-29T15:52:00Z"
  },
  {
    id: "space-weave",
    workspaceId: workspace.id,
    teamId: "team-automation",
    name: "Weave Engine",
    description: "Narrative thread model that keeps decisions, Artifacts and Proofs connected.",
    status: "pending",
    currentLayerId: "layer-weave-experiment",
    updatedAt: "2026-06-29T15:37:00Z"
  }
];

const layers: Layer[] = [
  {
    id: "layer-studio-base",
    spaceId: "space-studio",
    name: "Base shell",
    kind: "base",
    status: "stable",
    summary: "Stable application frame, navigation model and terminology.",
    artifactIds: ["artifact-shell-map"],
    stepIds: ["step-shell-audit"],
    gateIds: ["gate-studio-proof"]
  },
  {
    id: "layer-studio-review",
    spaceId: "space-studio",
    parentId: "layer-studio-base",
    name: "Review workspace",
    kind: "proposal",
    status: "review",
    summary: "Adds Weave review, Artifact browser, Timeline and policy surfaces.",
    artifactIds: ["artifact-weave-sample", "artifact-policy-draft"],
    stepIds: ["step-visual-pass"],
    gateIds: ["gate-studio-proof", "gate-policy-review"]
  },
  {
    id: "layer-policy-base",
    spaceId: "space-policy",
    name: "Policy matrix",
    kind: "base",
    status: "active",
    summary: "Initial rules for ownership, Step permissions and promotion Gates.",
    artifactIds: ["artifact-policy-draft"],
    stepIds: ["step-policy-check"],
    gateIds: ["gate-policy-review"]
  },
  {
    id: "layer-weave-experiment",
    spaceId: "space-weave",
    name: "Large diff reader",
    kind: "experiment",
    status: "active",
    summary: "Virtualized Weave stream that stays responsive with many events.",
    artifactIds: ["artifact-weave-sample"],
    stepIds: ["step-weave-load"],
    gateIds: ["gate-weave-performance"]
  }
];

const artifacts: Artifact[] = [
  {
    id: "artifact-shell-map",
    spaceId: "space-studio",
    layerId: "layer-studio-base",
    name: "Studio surface map",
    type: "note",
    summary: "Dashboard, Spaces list, Layer tree, Weave review, Timeline and Proof panels.",
    location: "docs/studio/surface-map.md",
    updatedAt: "2026-06-29T15:58:00Z",
    sizeLabel: "4.2 KB",
    proofIds: ["proof-ux-review"],
    access: {
      mode: "write",
      canOpen: true,
      isRedacted: false
    }
  },
  {
    id: "artifact-weave-sample",
    spaceId: "space-weave",
    layerId: "layer-weave-experiment",
    name: "Large Weave sample",
    type: "step-output",
    summary: "Synthetic event stream for testing virtualized review rendering.",
    location: "fixtures/weaves/large-review.json",
    updatedAt: "2026-06-29T16:00:00Z",
    sizeLabel: "128 KB",
    proofIds: ["proof-load-model"],
    access: {
      mode: "none",
      canOpen: false,
      isRedacted: true,
      reason: "Restricted by Layer access policy"
    }
  },
  {
    id: "artifact-policy-draft",
    spaceId: "space-policy",
    layerId: "layer-policy-base",
    name: "Promotion policy draft",
    type: "report",
    summary: "Matrix of workspace, team and space rules for promotion and automated Steps.",
    location: "policies/promotion-policy.layrs.json",
    updatedAt: "2026-06-29T16:04:00Z",
    sizeLabel: "9.8 KB",
    proofIds: ["proof-policy-review"],
    access: {
      mode: "admin",
      canOpen: true,
      isRedacted: false
    }
  }
];

const steps: Step[] = [
  {
    id: "step-shell-audit",
    spaceId: "space-studio",
    layerId: "layer-studio-base",
    name: "Check terminology alignment",
    status: "passing",
    actor: "human",
    startedAt: "2026-06-29T15:44:00Z",
    completedAt: "2026-06-29T15:52:00Z",
    artifactIds: ["artifact-shell-map"],
    proofIds: ["proof-ux-review"]
  },
  {
    id: "step-visual-pass",
    spaceId: "space-studio",
    layerId: "layer-studio-review",
    name: "Review app-first layout",
    status: "needs-proof",
    actor: "human",
    startedAt: "2026-06-29T16:03:00Z",
    artifactIds: ["artifact-shell-map"],
    proofIds: []
  },
  {
    id: "step-policy-check",
    spaceId: "space-policy",
    layerId: "layer-policy-base",
    name: "Evaluate promotion policy",
    status: "passing",
    actor: "automation",
    startedAt: "2026-06-29T15:46:00Z",
    completedAt: "2026-06-29T15:49:00Z",
    artifactIds: ["artifact-policy-draft"],
    proofIds: ["proof-policy-review"]
  },
  {
    id: "step-weave-load",
    spaceId: "space-weave",
    layerId: "layer-weave-experiment",
    name: "Generate large Weave review sample",
    status: "pending",
    actor: "automation",
    startedAt: "2026-06-29T16:05:00Z",
    artifactIds: ["artifact-weave-sample"],
    proofIds: ["proof-load-model"]
  }
];

const gates: Gate[] = [
  {
    id: "gate-studio-proof",
    spaceId: "space-studio",
    layerId: "layer-studio-review",
    name: "Studio proof required",
    status: "needs-proof",
    ownerTeamId: "team-studio",
    requiredProofKinds: ["review", "snapshot"],
    summary: "Promotion needs visible proof that web and desktop shells expose the same concepts."
  },
  {
    id: "gate-policy-review",
    spaceId: "space-policy",
    layerId: "layer-policy-base",
    name: "Policy owner review",
    status: "passing",
    ownerTeamId: "team-core",
    requiredProofKinds: ["review"],
    summary: "Policy changes have a Core Team review and clear scope."
  },
  {
    id: "gate-weave-performance",
    spaceId: "space-weave",
    layerId: "layer-weave-experiment",
    name: "Large Weave performance",
    status: "pending",
    ownerTeamId: "team-automation",
    requiredProofKinds: ["test"],
    summary: "Large review streams must render with virtualization before promotion."
  }
];

const proofs: Proof[] = [
  {
    id: "proof-ux-review",
    targetId: "artifact-shell-map",
    targetType: "artifact",
    kind: "review",
    status: "accepted",
    title: "Terminology review",
    evidence: "Workspace, Team and Space are used as primary labels across the shell.",
    createdAt: "2026-06-29T15:53:00Z",
    gateId: "gate-studio-proof",
    artifactId: "artifact-shell-map"
  },
  {
    id: "proof-policy-review",
    targetId: "gate-policy-review",
    targetType: "gate",
    kind: "review",
    status: "accepted",
    title: "Policy owner approval",
    evidence: "Core Team owns promotion policy edits for this Space.",
    createdAt: "2026-06-29T15:55:00Z",
    gateId: "gate-policy-review",
    artifactId: "artifact-policy-draft"
  },
  {
    id: "proof-load-model",
    targetId: "artifact-weave-sample",
    targetType: "artifact",
    kind: "test",
    status: "reviewing",
    title: "Virtualized Weave model",
    evidence: "Large event streams are windowed by row height and scroll offset.",
    createdAt: "2026-06-29T16:06:00Z",
    gateId: "gate-weave-performance",
    artifactId: "artifact-weave-sample"
  },
  {
    id: "proof-desktop-snapshot",
    targetId: "layer-studio-review",
    targetType: "layer",
    kind: "snapshot",
    status: "missing",
    title: "Desktop shell snapshot",
    evidence: "Pending local Tauri dependency install.",
    createdAt: "2026-06-29T16:09:00Z",
    gateId: "gate-studio-proof"
  }
];

const policies: Policy[] = [
  {
    id: "policy-workspace-promotion",
    scope: "workspace",
    targetId: workspace.id,
    name: "Workspace promotion",
    appliesTo: "Layer promotion",
    effect: "require-proof",
    rules: [
      {
        id: "rule-promotion-review",
        subject: "layer",
        action: "promote",
        effect: "require-proof",
        gateId: "gate-studio-proof"
      },
      {
        id: "rule-policy-owner",
        subject: "space",
        action: "edit-policy",
        effect: "require-proof",
        gateId: "gate-policy-review"
      }
    ]
  },
  {
    id: "policy-automation-scope",
    scope: "team",
    targetId: "team-automation",
    name: "Automation scope",
    appliesTo: "Automated Steps",
    effect: "allow",
    rules: [
      {
        id: "rule-step-artifacts",
        subject: "step",
        action: "create-artifact",
        effect: "allow"
      },
      {
        id: "rule-step-approval",
        subject: "step",
        action: "approve-gate",
        effect: "block"
      }
    ]
  },
  {
    id: "policy-space-performance",
    scope: "space",
    targetId: "space-weave",
    name: "Weave performance",
    appliesTo: "Weave review",
    effect: "require-proof",
    rules: [
      {
        id: "rule-weave-load",
        subject: "space",
        action: "render-large-weave",
        effect: "require-proof",
        gateId: "gate-weave-performance"
      }
    ]
  }
];

function createWeaveEvents(count: number): WeaveEvent[] {
  const kinds: WeaveEvent["kind"][] = ["intent", "change", "decision", "comment", "proof", "artifact"];

  return Array.from({ length: count }, (_, index) => {
    const kind = kinds[index % kinds.length];
    const eventNumber = index + 1;

    return {
      id: `weave-event-${eventNumber.toString().padStart(3, "0")}`,
      weaveId: "weave-studio-review",
      kind,
      title: `${kind} ${eventNumber}`,
      actor: index % 3 === 0 ? "Studio Team" : index % 3 === 1 ? "Automation Step" : "Core Reviewer",
      at: new Date(Date.UTC(2026, 5, 29, 14, Math.floor(index / 2), (index % 2) * 30)).toISOString(),
      summary:
        index % 5 === 0
          ? "Large diff checkpoint with related Artifact and Proof references kept in the review stream."
          : "Incremental context update for the selected Layer and its promotion path.",
      diffStats:
        kind === "change"
          ? {
              files: 2 + (index % 9),
              additions: 12 + index * 3,
              removals: 4 + index
            }
          : undefined,
      artifactIds: index % 4 === 0 ? ["artifact-weave-sample"] : [],
      proofIds: index % 6 === 0 ? ["proof-load-model"] : []
    };
  });
}

const weaves: Weave[] = [
  {
    id: "weave-studio-review",
    spaceId: "space-studio",
    layerId: "layer-studio-review",
    title: "Studio shell review",
    status: "needs-proof",
    events: createWeaveEvents(180)
  }
];

const timeline: TimelineItem[] = [
  {
    id: "timeline-terminology",
    at: "2026-06-29T15:44:00Z",
    title: "Terminology aligned",
    summary: "Studio shell labels were mapped to Workspace, Team, Space, Layer and Artifact.",
    status: "passing",
    relatedIds: ["artifact-shell-map", "proof-ux-review"]
  },
  {
    id: "timeline-policy",
    at: "2026-06-29T15:55:00Z",
    title: "Policy reviewed",
    summary: "Promotion rules require Proof before a Layer can pass its Gate.",
    status: "passing",
    relatedIds: ["policy-workspace-promotion", "gate-policy-review"]
  },
  {
    id: "timeline-weave",
    at: "2026-06-29T16:06:00Z",
    title: "Weave load model queued",
    summary: "Large review data is available for virtualization validation.",
    status: "pending",
    relatedIds: ["weave-studio-review", "gate-weave-performance"]
  },
  {
    id: "timeline-desktop",
    at: "2026-06-29T16:09:00Z",
    title: "Desktop snapshot pending",
    summary: "Tauri shell is scaffolded; dependency install and runtime proof remain open.",
    status: "needs-proof",
    relatedIds: ["proof-desktop-snapshot", "gate-studio-proof"]
  }
];

const accessRegistries: LayerAccessRegistry[] = [
  {
    id: "access-layer-studio-base",
    workspaceId: workspace.id,
    layerId: "layer-studio-base",
    updatedAt: "2026-06-29T16:04:00Z",
    rules: [
      {
        id: "access-layer-studio-base-account",
        subjectKind: "account",
        subjectId: account.id,
        subjectName: account.name,
        mode: "admin"
      },
      {
        id: "access-layer-studio-base-team",
        subjectKind: "team",
        subjectId: "team-studio",
        subjectName: "Studio",
        mode: "write"
      }
    ]
  },
  {
    id: "access-layer-studio-review",
    workspaceId: workspace.id,
    layerId: "layer-studio-review",
    updatedAt: "2026-06-29T16:05:00Z",
    rules: [
      {
        id: "access-layer-studio-review-account",
        subjectKind: "account",
        subjectId: account.id,
        subjectName: account.name,
        mode: "admin"
      },
      {
        id: "access-layer-studio-review-core",
        subjectKind: "team",
        subjectId: "team-core",
        subjectName: "Core",
        mode: "read"
      },
      {
        id: "access-layer-studio-review-automation",
        subjectKind: "team",
        subjectId: "team-automation",
        subjectName: "Automation",
        mode: "none"
      }
    ]
  },
  {
    id: "access-layer-policy-base",
    workspaceId: workspace.id,
    layerId: "layer-policy-base",
    updatedAt: "2026-06-29T16:05:00Z",
    rules: [
      {
        id: "access-layer-policy-base-core",
        subjectKind: "team",
        subjectId: "team-core",
        subjectName: "Core",
        mode: "admin"
      },
      {
        id: "access-layer-policy-base-studio",
        subjectKind: "team",
        subjectId: "team-studio",
        subjectName: "Studio",
        mode: "read"
      }
    ]
  }
];

const layerAccessPolicies: LayerAccessPolicy[] = accessRegistries.map(legacyRegistryToLayerAccessPolicy);

const devices: Device[] = [
  {
    id: "device-web-chrome",
    accountId: account.id,
    name: "Chrome on Windows",
    kind: "browser",
    status: "trusted",
    lastSeenAt: "2026-06-29T16:14:00Z"
  },
  {
    id: "device-desktop-preview",
    accountId: account.id,
    name: "Layrs Desktop Preview",
    kind: "desktop",
    status: "pending",
    lastSeenAt: "2026-06-29T15:44:00Z"
  }
];

const auditEvents: AuditEvent[] = [
  {
    id: "audit-login",
    workspaceId: workspace.id,
    actorAccountId: account.id,
    action: "session.login",
    target: "studio-web",
    summary: "Signed in to Studio Web.",
    at: "2026-06-29T16:10:00Z"
  },
  {
    id: "audit-access",
    workspaceId: workspace.id,
    actorAccountId: account.id,
    action: "layer.access.reviewed",
    target: "layer-studio-review",
    summary: "Reviewed access registry for the Studio review layer.",
    at: "2026-06-29T16:12:00Z"
  },
  {
    id: "audit-redaction",
    workspaceId: workspace.id,
    actorAccountId: account.id,
    action: "artifact.redacted",
    target: "artifact-weave-sample",
    summary: "Artifact contents hidden by Layer access policy.",
    at: "2026-06-29T16:13:00Z"
  }
];

const fixture: StudioFixture = {
  account,
  session,
  workspace,
  workspaces: [workspace],
  teams,
  spaces,
  layers,
  artifacts,
  steps,
  weaves,
  proofs,
  gates,
  policies,
  timeline,
  layerAccessPolicies,
  accessRegistries,
  workspaceMembers,
  teamMembers,
  invitations,
  devices,
  auditEvents
};

export function getStudioFixture(): StudioFixture {
  return fixture;
}

export const studioFixture = fixture;

export interface LayrsClientOptions {
  baseUrl?: string;
  fetchImpl?: typeof fetch;
}

export interface LoginRequest {
  email: string;
  password: string;
}

export interface SignupRequest {
  name: string;
  email: string;
  password: string;
}

export interface CreateWorkspaceInput {
  name: string;
  slug: string;
  description?: string;
}

export interface CreateTeamInput {
  workspaceId: LayrsId;
  name: string;
  purpose?: string;
  memberAccountIds?: LayrsId[];
}

export interface AddTeamMemberInput {
  workspaceId: LayrsId;
  teamId: LayrsId;
  email?: string;
  accountId?: LayrsId;
  role: TeamMemberRole;
}

export interface CreateInvitationInput {
  workspaceId: LayrsId;
  email: string;
  role: WorkspaceMemberRole;
  teamIds?: LayrsId[];
}

export interface CreateLayerAccessRuleInput {
  workspaceId: LayrsId;
  spaceId: LayrsId;
  layerId: LayrsId;
  rule: LayerAccessRule;
}

export interface UpdateLayerAccessRuleInput extends CreateLayerAccessRuleInput {}

export interface DeleteLayerAccessRuleInput {
  workspaceId: LayrsId;
  spaceId: LayrsId;
  layerId: LayrsId;
  ruleId: LayrsId;
}

export interface CreateSpaceInput {
  workspaceId: LayrsId;
  name: string;
  key: string;
  teamId?: LayrsId;
  description?: string;
}

export interface CreateLayerInput {
  workspaceId: LayrsId;
  spaceId: LayrsId;
  name: string;
  parentLayerId?: LayrsId;
  baseLayerIds?: LayrsId[];
  description?: string;
}

export interface DeleteSpaceResult {
  id: LayrsId;
  workspaceId: LayrsId;
  deleted: boolean;
  deletedLayers: number;
  deletedArtifacts: number;
}

export interface DeleteLayerResult {
  id: LayrsId;
  spaceId: LayrsId;
  deleted: boolean;
}

export interface StartDeviceFlowInput {
  label: string;
  kind: Device["kind"];
}

export interface LayrsClientLike {
  getSession(): Promise<AuthSessionResponse>;
  login(request: LoginRequest): Promise<AuthSessionResponse>;
  signup(request: SignupRequest): Promise<AuthSessionResponse>;
  logout(): Promise<void>;
  listWorkspaces(): Promise<Workspace[]>;
  createWorkspace(request: CreateWorkspaceInput): Promise<Workspace>;
  getStudioSnapshot(workspaceId?: LayrsId): Promise<StudioSnapshot>;
  listTeams(workspaceId: LayrsId): Promise<Team[]>;
  getTeam(workspaceId: LayrsId, teamId: LayrsId): Promise<Team>;
  createTeam(request: CreateTeamInput): Promise<Team>;
  listTeamMembers(workspaceId: LayrsId, teamId: LayrsId): Promise<TeamMember[]>;
  addTeamMember(request: AddTeamMemberInput): Promise<TeamMember>;
  removeTeamMember(workspaceId: LayrsId, teamId: LayrsId, accountId: LayrsId): Promise<void>;
  listWorkspaceMembers(workspaceId: LayrsId): Promise<WorkspaceMember[]>;
  listInvitations(workspaceId: LayrsId): Promise<Invitation[]>;
  createInvitation(request: CreateInvitationInput): Promise<Invitation>;
  listMyInvitations(): Promise<Invitation[]>;
  acceptInvitation(invitationId: LayrsId): Promise<Invitation>;
  declineInvitation(invitationId: LayrsId): Promise<Invitation>;
  createSpace(request: CreateSpaceInput): Promise<Space>;
  deleteSpace(workspaceId: LayrsId, spaceId: LayrsId): Promise<DeleteSpaceResult>;
  createLayer(request: CreateLayerInput): Promise<Layer>;
  deleteLayer(workspaceId: LayrsId, spaceId: LayrsId, layerId: LayrsId): Promise<DeleteLayerResult>;
  getLayerAccessPolicy(workspaceId: LayrsId, spaceId: LayrsId, layerId: LayrsId): Promise<LayerAccessPolicy>;
  replaceLayerAccessPolicy(policy: LayerAccessPolicy): Promise<LayerAccessPolicy>;
  createLayerAccessRule(input: CreateLayerAccessRuleInput): Promise<LayerAccessPolicy>;
  updateLayerAccessRule(input: UpdateLayerAccessRuleInput): Promise<LayerAccessPolicy>;
  deleteLayerAccessRule(input: DeleteLayerAccessRuleInput): Promise<LayerAccessPolicy>;
  updateLayerAccessRegistry(registry: LayerAccessRegistry): Promise<LayerAccessRegistry>;
  listDevices(): Promise<Device[]>;
  startDeviceFlow(request: StartDeviceFlowInput): Promise<DeviceFlow>;
  listAuditEvents(workspaceId: LayrsId): Promise<AuditEvent[]>;
}

export class LayrsApiError extends Error {
  readonly status: number;
  readonly code?: string;

  constructor(message: string, status: number, code?: string) {
    super(message);
    this.name = "LayrsApiError";
    this.status = status;
    this.code = code;
  }
}

export class LayrsClient implements LayrsClientLike {
  private readonly baseUrl: string;
  private readonly fetchImpl: typeof fetch;

  constructor(options: LayrsClientOptions = {}) {
    this.baseUrl = (options.baseUrl ?? "").replace(/\/$/, "");
    this.fetchImpl = (options.fetchImpl ?? globalThis.fetch).bind(globalThis) as typeof fetch;
  }

  getSession(): Promise<AuthSessionResponse> {
    return this.request<AuthSessionResponse>("/v1/auth/session");
  }

  login(request: LoginRequest): Promise<AuthSessionResponse> {
    return this.request<AuthSessionResponse>("/v1/auth/login", {
      method: "POST",
      body: request
    });
  }

  signup(request: SignupRequest): Promise<AuthSessionResponse> {
    return this.request<AuthSessionResponse>("/v1/auth/signup", {
      method: "POST",
      body: request
    });
  }

  async logout(): Promise<void> {
    await this.request<unknown>("/v1/auth/logout", { method: "POST" });
  }

  async listWorkspaces(): Promise<Workspace[]> {
    return unwrapItems(await this.request<Workspace[] | { items: Workspace[] }>("/v1/workspaces"));
  }

  createWorkspace(request: CreateWorkspaceInput): Promise<Workspace> {
    return this.request<Workspace>("/v1/workspaces", {
      method: "POST",
      body: request
    });
  }

  async getStudioSnapshot(workspaceId?: LayrsId): Promise<StudioSnapshot> {
    const query = workspaceId ? `?workspace_id=${encodeURIComponent(workspaceId)}` : "";
    return normalizeStudioSnapshot(await this.request<StudioSnapshotWire>(`/v1/studio/snapshot${query}`));
  }

  async listTeams(workspaceId: LayrsId): Promise<Team[]> {
    return unwrapItems(
      await this.request<Team[] | { items: Team[] }>(`/v1/workspaces/${encodeURIComponent(workspaceId)}/teams`)
    );
  }

  getTeam(workspaceId: LayrsId, teamId: LayrsId): Promise<Team> {
    return this.request<Team>(
      `/v1/workspaces/${encodeURIComponent(workspaceId)}/teams/${encodeURIComponent(teamId)}`
    );
  }

  createTeam(request: CreateTeamInput): Promise<Team> {
    return this.request<Team>(`/v1/workspaces/${encodeURIComponent(request.workspaceId)}/teams`, {
      method: "POST",
      body: {
        name: request.name,
        purpose: request.purpose,
        memberAccountIds: request.memberAccountIds
      }
    });
  }

  async listTeamMembers(workspaceId: LayrsId, teamId: LayrsId): Promise<TeamMember[]> {
    return unwrapItems(
      await this.request<TeamMember[] | { items: TeamMember[] }>(
        `/v1/workspaces/${encodeURIComponent(workspaceId)}/teams/${encodeURIComponent(teamId)}/members`
      )
    );
  }

  addTeamMember(request: AddTeamMemberInput): Promise<TeamMember> {
    return this.request<TeamMember>(
      `/v1/workspaces/${encodeURIComponent(request.workspaceId)}/teams/${encodeURIComponent(request.teamId)}/members`,
      {
        method: "POST",
        body: {
          email: request.email,
          accountId: request.accountId,
          role: request.role
        }
      }
    );
  }

  async removeTeamMember(workspaceId: LayrsId, teamId: LayrsId, accountId: LayrsId): Promise<void> {
    await this.request<unknown>(
      `/v1/workspaces/${encodeURIComponent(workspaceId)}/teams/${encodeURIComponent(teamId)}/members/${encodeURIComponent(accountId)}`,
      { method: "DELETE" }
    );
  }

  async listWorkspaceMembers(workspaceId: LayrsId): Promise<WorkspaceMember[]> {
    return unwrapItems(
      await this.request<WorkspaceMember[] | { items: WorkspaceMember[] }>(
        `/v1/workspaces/${encodeURIComponent(workspaceId)}/members`
      )
    );
  }

  async listInvitations(workspaceId: LayrsId): Promise<Invitation[]> {
    return unwrapItems(
      await this.request<Invitation[] | { items: Invitation[] }>(
        `/v1/workspaces/${encodeURIComponent(workspaceId)}/invitations`
      )
    );
  }

  createInvitation(request: CreateInvitationInput): Promise<Invitation> {
    return this.request<Invitation>(`/v1/workspaces/${encodeURIComponent(request.workspaceId)}/invitations`, {
      method: "POST",
      body: {
        email: request.email,
        role: request.role,
        teamIds: request.teamIds ?? []
      }
    });
  }

  async listMyInvitations(): Promise<Invitation[]> {
    return unwrapItems(await this.request<Invitation[] | { items: Invitation[] }>("/v1/me/invitations"));
  }

  acceptInvitation(invitationId: LayrsId): Promise<Invitation> {
    return this.request<Invitation>(`/v1/invitations/${encodeURIComponent(invitationId)}/accept`, { method: "POST" });
  }

  declineInvitation(invitationId: LayrsId): Promise<Invitation> {
    return this.request<Invitation>(`/v1/invitations/${encodeURIComponent(invitationId)}/decline`, { method: "POST" });
  }

  createSpace(request: CreateSpaceInput): Promise<Space> {
    return this.request<Space>(`/v1/workspaces/${encodeURIComponent(request.workspaceId)}/spaces`, {
      method: "POST",
      body: request
    });
  }

  deleteSpace(workspaceId: LayrsId, spaceId: LayrsId): Promise<DeleteSpaceResult> {
    return this.request<DeleteSpaceResult>(
      `/v1/workspaces/${encodeURIComponent(workspaceId)}/spaces/${encodeURIComponent(spaceId)}`,
      { method: "DELETE" }
    );
  }

  createLayer(request: CreateLayerInput): Promise<Layer> {
    return this.request<Layer>(
      `/v1/workspaces/${encodeURIComponent(request.workspaceId)}/spaces/${encodeURIComponent(request.spaceId)}/layers`,
      {
        method: "POST",
        body: request
      }
    );
  }

  deleteLayer(workspaceId: LayrsId, spaceId: LayrsId, layerId: LayrsId): Promise<DeleteLayerResult> {
    return this.request<DeleteLayerResult>(
      `/v1/workspaces/${encodeURIComponent(workspaceId)}/spaces/${encodeURIComponent(spaceId)}/layers/${encodeURIComponent(layerId)}`,
      { method: "DELETE" }
    );
  }

  async getLayerAccessPolicy(
    workspaceId: LayrsId,
    spaceId: LayrsId,
    layerId: LayrsId
  ): Promise<LayerAccessPolicy> {
    const policy = await this.request<LayerAccessPolicyWire>(
      layerAccessPolicyPath(workspaceId, spaceId, layerId)
    );
    return layerAccessPolicyFromWire(policy);
  }

  async replaceLayerAccessPolicy(policy: LayerAccessPolicy): Promise<LayerAccessPolicy> {
    const saved = await this.request<LayerAccessPolicyWire>(
      layerAccessPolicyPath(policy.workspaceId, policy.spaceId, policy.layerId),
      {
        method: "PUT",
        body: layerAccessPolicyToWire(policy)
      }
    );
    return layerAccessPolicyFromWire(saved);
  }

  async createLayerAccessRule(input: CreateLayerAccessRuleInput): Promise<LayerAccessPolicy> {
    const policy = await this.getLayerAccessPolicy(input.workspaceId, input.spaceId, input.layerId);
    return this.replaceLayerAccessPolicy({
      ...policy,
      rules: [...policy.rules.filter((rule) => rule.id !== input.rule.id), input.rule]
    });
  }

  async updateLayerAccessRule(input: UpdateLayerAccessRuleInput): Promise<LayerAccessPolicy> {
    const policy = await this.getLayerAccessPolicy(input.workspaceId, input.spaceId, input.layerId);
    return this.replaceLayerAccessPolicy({
      ...policy,
      rules: policy.rules.map((rule) => (rule.id === input.rule.id ? input.rule : rule))
    });
  }

  async deleteLayerAccessRule(input: DeleteLayerAccessRuleInput): Promise<LayerAccessPolicy> {
    const policy = await this.getLayerAccessPolicy(input.workspaceId, input.spaceId, input.layerId);
    return this.replaceLayerAccessPolicy({
      ...policy,
      rules: policy.rules.filter((rule) => rule.id !== input.ruleId)
    });
  }

  async updateLayerAccessRegistry(registry: LayerAccessRegistry): Promise<LayerAccessRegistry> {
    // Legacy compat only: use replaceLayerAccessPolicy for server writes.
    return { ...registry, updatedAt: new Date().toISOString() };
  }

  async listDevices(): Promise<Device[]> {
    return unwrapItems(await this.request<Device[] | { items: Device[] }>("/v1/devices"));
  }

  startDeviceFlow(request: StartDeviceFlowInput): Promise<DeviceFlow> {
    return this.request<DeviceFlow>("/v1/desktop/device/start", {
      method: "POST",
      body: request
    });
  }

  async listAuditEvents(workspaceId: LayrsId): Promise<AuditEvent[]> {
    return unwrapItems(
      await this.request<AuditEvent[] | { items: AuditEvent[] }>(
        `/v1/workspaces/${encodeURIComponent(workspaceId)}/audit-events`
      )
    );
  }

  private async request<T>(path: string, options: { method?: string; body?: unknown } = {}): Promise<T> {
    const fetchImpl = this.fetchImpl;
    const response = await fetchImpl(`${this.baseUrl}${path}`, {
      method: options.method ?? "GET",
      credentials: "include",
      headers: options.body === undefined ? undefined : { "Content-Type": "application/json" },
      body: options.body === undefined ? undefined : JSON.stringify(options.body)
    });

    if (response.status === 204) {
      return undefined as T;
    }

    const payload = await readJson(response);

    if (!response.ok) {
      const message = getErrorMessage(payload, response.statusText || "Layrs API request failed");
      const code = getErrorCode(payload);
      throw new LayrsApiError(message, response.status, code);
    }

    return payload as T;
  }
}

export function createMockLayrsClient(): LayrsClientLike {
  let snapshot: StudioSnapshot = getStudioFixture();

  return {
    async getSession() {
      return {
        state: "authenticated",
        account: snapshot.account,
        session: snapshot.session,
        workspaces: snapshot.workspaces,
        activeWorkspaceId: snapshot.session.activeWorkspaceId
      };
    },
    async login() {
      return this.getSession();
    },
    async signup() {
      return this.getSession();
    },
    async logout() {
      return undefined;
    },
    async listWorkspaces() {
      return snapshot.workspaces;
    },
    async createWorkspace(request) {
      const workspace: Workspace = {
        id: `workspace-${request.slug}`,
        name: request.name,
        slug: request.slug,
        description: request.description ?? "New Layrs workspace.",
        health: "pending",
        updatedAt: new Date().toISOString()
      };
      snapshot = {
        ...snapshot,
        workspace,
        workspaces: [...snapshot.workspaces, workspace],
        session: { ...snapshot.session, activeWorkspaceId: workspace.id }
      };
      return workspace;
    },
    async getStudioSnapshot(workspaceId) {
      const workspace = snapshot.workspaces.find((item) => item.id === workspaceId) ?? snapshot.workspace;
      return {
        ...snapshot,
        workspace,
        session: { ...snapshot.session, activeWorkspaceId: workspace.id }
      };
    },
    async listTeams(workspaceId) {
      return snapshot.teams.filter((team) => team.workspaceId === workspaceId);
    },
    async getTeam(workspaceId, teamId) {
      const team = snapshot.teams.find((item) => item.workspaceId === workspaceId && item.id === teamId);
      if (!team) {
        throw new LayrsApiError("Team not found", 404, "not_found");
      }
      return team;
    },
    async createTeam(request) {
      const team: Team = {
        id: `team-${slugify(request.name)}`,
        workspaceId: request.workspaceId,
        name: request.name,
        purpose: request.purpose ?? "New team created from Studio Web.",
        members: request.memberAccountIds?.length ?? 0,
        gateResponsibility: "Workspace operations"
      };
      snapshot = { ...snapshot, teams: [...snapshot.teams, team] };
      return team;
    },
    async listTeamMembers(workspaceId, teamId) {
      return snapshot.teamMembers.filter((member) => member.workspaceId === workspaceId && member.teamId === teamId);
    },
    async addTeamMember(request) {
      const accountId = request.accountId ?? `account-${slugify(request.email ?? "invited-member")}`;
      const member: TeamMember = {
        id: `team-membership-${request.teamId}-${accountId}`,
        workspaceId: request.workspaceId,
        teamId: request.teamId,
        accountId,
        email: request.email ?? `${accountId}@layrs.local`,
        name: request.email?.split("@")[0] ?? accountId,
        role: request.role,
        createdAt: new Date().toISOString()
      };
      snapshot = {
        ...snapshot,
        teams: snapshot.teams.map((team) =>
          team.workspaceId === request.workspaceId && team.id === request.teamId
            ? { ...team, members: team.members + 1 }
            : team
        ),
        teamMembers: [...snapshot.teamMembers.filter((item) => item.id !== member.id), member]
      };
      return member;
    },
    async removeTeamMember(workspaceId, teamId, accountId) {
      const nextMembers = snapshot.teamMembers.filter(
        (member) => !(member.workspaceId === workspaceId && member.teamId === teamId && member.accountId === accountId)
      );
      snapshot = {
        ...snapshot,
        teams: snapshot.teams.map((team) =>
          team.workspaceId === workspaceId && team.id === teamId
            ? { ...team, members: Math.max(0, team.members - (nextMembers.length === snapshot.teamMembers.length ? 0 : 1)) }
            : team
        ),
        teamMembers: nextMembers
      };
    },
    async listWorkspaceMembers(workspaceId) {
      return snapshot.workspaceMembers.filter((member) => member.workspaceId === workspaceId);
    },
    async listInvitations(workspaceId) {
      return snapshot.invitations.filter((invitation) => invitation.workspaceId === workspaceId);
    },
    async createInvitation(request) {
      const invitation: Invitation = {
        id: `invitation-${slugify(request.email)}`,
        workspaceId: request.workspaceId,
        email: request.email,
        role: request.role,
        teamIds: request.teamIds ?? [],
        status: "pending",
        createdAt: new Date().toISOString(),
        expiresAt: new Date(Date.now() + 7 * 24 * 60 * 60 * 1000).toISOString()
      };
      snapshot = {
        ...snapshot,
        invitations: [...snapshot.invitations.filter((item) => item.id !== invitation.id), invitation]
      };
      return invitation;
    },
    async listMyInvitations() {
      return snapshot.invitations.filter((invitation) => invitation.status === "pending");
    },
    async acceptInvitation(invitationId) {
      const invitation = snapshot.invitations.find((item) => item.id === invitationId);
      if (!invitation) {
        throw new LayrsApiError("Invitation not found", 404, "not_found");
      }
      const accepted = { ...invitation, status: "accepted" as const };
      snapshot = {
        ...snapshot,
        invitations: snapshot.invitations.map((item) => (item.id === invitationId ? accepted : item))
      };
      return accepted;
    },
    async declineInvitation(invitationId) {
      const invitation = snapshot.invitations.find((item) => item.id === invitationId);
      if (!invitation) {
        throw new LayrsApiError("Invitation not found", 404, "not_found");
      }
      const declined = { ...invitation, status: "declined" as const };
      snapshot = {
        ...snapshot,
        invitations: snapshot.invitations.map((item) => (item.id === invitationId ? declined : item))
      };
      return declined;
    },
    async createSpace(request) {
      const space: Space = {
        id: `space-${request.key}`,
        workspaceId: request.workspaceId,
        teamId: request.teamId ?? snapshot.teams[0]?.id ?? "team-unassigned",
        name: request.name,
        description: request.description ?? "New Space created from Studio Web.",
        status: "pending",
        currentLayerId: "",
        updatedAt: new Date().toISOString()
      };
      snapshot = { ...snapshot, spaces: [...snapshot.spaces, space] };
      return space;
    },
    async deleteSpace(workspaceId, spaceId) {
      const deletedLayers = snapshot.layers.filter((layer) => layer.spaceId === spaceId).length;
      const deletedArtifacts = snapshot.artifacts.filter((artifact) => artifact.spaceId === spaceId).length;
      snapshot = {
        ...snapshot,
        spaces: snapshot.spaces.filter((space) => !(space.workspaceId === workspaceId && space.id === spaceId)),
        layers: snapshot.layers.filter((layer) => layer.spaceId !== spaceId),
        artifacts: snapshot.artifacts.filter((artifact) => artifact.spaceId !== spaceId),
        layerAccessPolicies: snapshot.layerAccessPolicies.filter((policy) => policy.spaceId !== spaceId),
        accessRegistries: snapshot.accessRegistries.filter((registry) =>
          snapshot.layers.some((layer) => layer.id === registry.layerId && layer.spaceId !== spaceId)
        ),
        timeline: snapshot.timeline.filter((event) => !event.relatedIds.includes(spaceId))
      };
      return { id: spaceId, workspaceId, deleted: true, deletedLayers, deletedArtifacts };
    },
    async createLayer(request) {
      const layer: Layer = {
        id: `layer-${slugify(request.name)}`,
        spaceId: request.spaceId,
        parentId: request.parentLayerId,
        name: request.name,
        kind: "proposal",
        status: "review",
        summary: request.description ?? "New Layer created from Studio Web.",
        artifactIds: [],
        stepIds: [],
        gateIds: []
      };
      snapshot = { ...snapshot, layers: [...snapshot.layers, layer] };
      return layer;
    },
    async deleteLayer(_workspaceId, spaceId, layerId) {
      snapshot = {
        ...snapshot,
        layers: snapshot.layers.filter((layer) => !(layer.spaceId === spaceId && layer.id === layerId)),
        artifacts: snapshot.artifacts.filter((artifact) => artifact.layerId !== layerId),
        layerAccessPolicies: snapshot.layerAccessPolicies.filter((policy) => policy.layerId !== layerId),
        accessRegistries: snapshot.accessRegistries.filter((registry) => registry.layerId !== layerId),
        timeline: snapshot.timeline.filter((event) => !event.relatedIds.includes(layerId))
      };
      return { id: layerId, spaceId, deleted: true };
    },
    async getLayerAccessPolicy(workspaceId, spaceId, layerId) {
      const policy = snapshot.layerAccessPolicies.find(
        (item) => item.workspaceId === workspaceId && item.spaceId === spaceId && item.layerId === layerId
      );
      if (!policy) {
        throw new LayrsApiError("Layer access policy not found", 404, "not_found");
      }
      return policy;
    },
    async replaceLayerAccessPolicy(policy) {
      const saved = {
        ...policy,
        policyEpoch: policy.policyEpoch + 1,
        generatedAt: new Date().toISOString()
      };
      const policies = [...snapshot.layerAccessPolicies.filter((item) => item.layerId !== policy.layerId), saved];
      snapshot = {
        ...snapshot,
        layerAccessPolicies: policies,
        accessRegistries: policies.map(layerAccessPolicyToLegacyRegistry)
      };
      return saved;
    },
    async createLayerAccessRule(input) {
      const policy = await this.getLayerAccessPolicy(input.workspaceId, input.spaceId, input.layerId);
      return this.replaceLayerAccessPolicy({
        ...policy,
        rules: [...policy.rules.filter((rule) => rule.id !== input.rule.id), input.rule]
      });
    },
    async updateLayerAccessRule(input) {
      const policy = await this.getLayerAccessPolicy(input.workspaceId, input.spaceId, input.layerId);
      return this.replaceLayerAccessPolicy({
        ...policy,
        rules: policy.rules.map((rule) => (rule.id === input.rule.id ? input.rule : rule))
      });
    },
    async deleteLayerAccessRule(input) {
      const policy = await this.getLayerAccessPolicy(input.workspaceId, input.spaceId, input.layerId);
      return this.replaceLayerAccessPolicy({
        ...policy,
        rules: policy.rules.filter((rule) => rule.id !== input.ruleId)
      });
    },
    async updateLayerAccessRegistry(registry) {
      const policy = legacyRegistryToLayerAccessPolicy(registry);
      snapshot = {
        ...snapshot,
        accessRegistries: snapshot.accessRegistries.map((item) =>
          item.id === registry.id ? { ...registry, updatedAt: new Date().toISOString() } : item
        ),
        layerAccessPolicies: [
          ...snapshot.layerAccessPolicies.filter((item) => item.layerId !== registry.layerId),
          { ...policy, generatedAt: new Date().toISOString() }
        ]
      };
      return registry;
    },
    async listDevices() {
      return snapshot.devices;
    },
    async startDeviceFlow(request) {
      return {
        id: `device-flow-${slugify(request.label)}`,
        userCode: "LAYRS-2026",
        verificationUri: "https://studio.layrs.local/device",
        status: "pending",
        expiresAt: new Date(Date.now() + 15 * 60 * 1000).toISOString()
      };
    },
    async listAuditEvents() {
      return snapshot.auditEvents;
    }
  };
}

export function createLayrsClient(options: LayrsClientOptions = {}): LayrsClient {
  return new LayrsClient(options);
}

export * from "./lenses";

interface LayerAccessPrincipalSetWire {
  accounts?: LayrsId[];
  teams?: LayrsId[];
}

interface LayerAccessRulePermissionsWire {
  read?: LayerAccessPrincipalSetWire;
  write?: LayerAccessPrincipalSetWire;
  admin?: LayerAccessPrincipalSetWire;
}

interface LayerAccessSignatureWire {
  key_id?: string;
  keyId?: string;
  value?: string;
}

interface LayerAccessRuleWire {
  id?: LayrsId;
  path?: string;
  artifact_id?: LayrsId;
  artifactId?: LayrsId;
  mode?: LayerAccessRuleMode;
  visibility?: LayerAccessVisibility;
  permissions?: LayerAccessRulePermissionsWire;
}

interface LayerAccessPolicyWire {
  schema?: string;
  workspace_id?: LayrsId;
  workspaceId?: LayrsId;
  space_id?: LayrsId;
  spaceId?: LayrsId;
  layer_id?: LayrsId;
  layerId?: LayrsId;
  policy_epoch?: number;
  policyEpoch?: number;
  generated_at?: string;
  generatedAt?: string;
  rules?: LayerAccessRuleWire[];
  signature?: LayerAccessSignatureWire;
}

type StudioSnapshotWire = Partial<StudioFixture> & {
  layers?: Array<Layer | LayerWire>;
  artifacts?: Array<Artifact | ArtifactWire>;
  layer_access_policies?: LayerAccessPolicyWire[];
  layerAccessPolicies?: Array<LayerAccessPolicy | LayerAccessPolicyWire>;
  workspace_members?: WorkspaceMember[];
  workspaceMembers?: WorkspaceMember[];
  team_members?: TeamMember[];
  teamMembers?: TeamMember[];
  invitations?: Invitation[];
};

type LayerWire = Partial<Layer> & {
  space_id?: LayrsId;
  parent_id?: LayrsId;
  artifact_ids?: LayrsId[];
  step_ids?: LayrsId[];
  gate_ids?: LayrsId[];
  root_tree_id?: LayrsId;
  policy_epoch?: number;
  server_cursor?: string;
  layer_head?: LayerHeadWire;
  layerHead?: LayerHeadWire;
  head?: LayerHeadWire;
};

type LayerHeadWire = Partial<LayerHeadMetadata> & {
  layer_state_id?: LayrsId;
  layerStateId?: LayrsId;
  root_tree_id?: LayrsId;
  rootTreeId?: LayrsId;
  policy_epoch?: number;
  policyEpoch?: number;
  server_cursor?: string;
  serverCursor?: string;
  updated_at?: string;
  updatedAt?: string;
  updated_by_account_id?: LayrsId;
  updatedByAccountId?: LayrsId;
};

type ArtifactWire = Partial<Artifact> & {
  artifact_id?: LayrsId;
  space_id?: LayrsId;
  layer_id?: LayrsId;
  artifact_kind?: ArtifactType | string;
  logical_path?: string;
  path?: string;
  updated_at?: string;
  size_label?: string;
  size_bytes?: number;
  sizeBytes?: number;
  proof_ids?: LayrsId[];
  media_type?: string;
  content_hash?: string;
  sha256?: string;
  byte_len?: number;
  byteLen?: number;
  lens_id?: string;
  root_tree_id?: LayrsId;
  current_tree_id?: LayrsId;
  current_file_object_id?: LayrsId;
  file_object_id?: LayrsId;
  fileObjectId?: LayrsId;
  file_object?: FileObjectWire;
  fileObject?: FileObjectWire;
  chunks?: ChunkWire[];
};

type FileObjectWire = Partial<FileObjectMetadata> & {
  file_object_id?: LayrsId;
  fileObjectId?: LayrsId;
  size_bytes?: number;
  sizeBytes?: number;
  media_type?: string;
  mediaType?: string;
  chunk_count?: number;
  chunkCount?: number;
  chunks?: ChunkWire[];
};

type ChunkWire = Partial<ChunkMetadata> & {
  chunk_id?: LayrsId;
  chunkId?: LayrsId;
  size_bytes?: number;
  sizeBytes?: number;
  byte_offset?: number;
  byteOffset?: number;
  chunk_index?: number;
  chunkIndex?: number;
  media_type?: string;
  mediaType?: string;
  object_key?: string;
  objectKey?: string;
  content?: unknown;
  data?: unknown;
};

function layerAccessPolicyPath(workspaceId: LayrsId, spaceId: LayrsId, layerId: LayrsId): string {
  return `/v1/workspaces/${encodeURIComponent(workspaceId)}/spaces/${encodeURIComponent(spaceId)}/layers/${encodeURIComponent(layerId)}/access`;
}

function normalizeStudioSnapshot(snapshot: StudioSnapshotWire): StudioSnapshot {
  const accessRegistries = snapshot.accessRegistries ?? [];
  const wirePolicies = snapshot.layerAccessPolicies ?? snapshot.layer_access_policies;
  const layerAccessPolicies =
    wirePolicies && wirePolicies.length > 0
      ? wirePolicies.map((policy) => layerAccessPolicyFromWire(policy))
      : accessRegistries.map(legacyRegistryToLayerAccessPolicy);

  return {
    ...(snapshot as StudioFixture),
    layers: (snapshot.layers ?? []).map(layerFromWire),
    artifacts: (snapshot.artifacts ?? []).map(artifactFromWire),
    layerAccessPolicies,
    accessRegistries: accessRegistries.length > 0 ? accessRegistries : layerAccessPolicies.map(layerAccessPolicyToLegacyRegistry),
    workspaceMembers: snapshot.workspaceMembers ?? snapshot.workspace_members ?? [],
    teamMembers: snapshot.teamMembers ?? snapshot.team_members ?? [],
    invitations: snapshot.invitations ?? []
  };
}

export function normalizeArtifactCollection(payload: unknown): Artifact[] {
  const record = recordValue(payload);
  const items =
    (Array.isArray(payload) ? payload : undefined) ??
    arrayValue(record?.items) ??
    arrayValue(record?.artifacts) ??
    [];

  return items.map((item) => artifactFromWire(item as ArtifactWire));
}

export function normalizeArtifactContentPayload(payload: unknown): ArtifactContentPayload | undefined {
  const record = recordValue(payload);
  if (!record) {
    return undefined;
  }

  const contentRecord = recordValue(record.content) ?? record;
  const fileObject = fileObjectFromWire(
    firstRecord(contentRecord.fileObject, contentRecord.file_object, record.fileObject, record.file_object) as
      | FileObjectWire
      | undefined
  );
  const mediaType =
    stringFrom(contentRecord.mediaType) ??
    stringFrom(contentRecord.media_type) ??
    stringFrom(record.mediaType) ??
    stringFrom(record.media_type) ??
    fileObject?.mediaType ??
    "application/octet-stream";
  const encoding = normalizeContentEncoding(
    stringFrom(contentRecord.encoding) ??
      stringFrom(record.encoding) ??
      stringFrom(contentRecord.contentEncoding) ??
      stringFrom(contentRecord.content_encoding)
  );
  const chunks = normalizeChunks([
    ...arrayValue(contentRecord.chunks),
    ...arrayValue(record.chunks),
    ...(fileObject?.chunks ?? [])
  ]);
  const sourceValue = contentSourceValue(
    contentRecord.value ?? contentRecord.content ?? contentRecord.body ?? contentRecord.data,
    chunks
  );
  const decoded = decodeContentValue(sourceValue, encoding, mediaType);
  const sha256 =
    stringFrom(contentRecord.sha256) ??
    stringFrom(contentRecord.contentHash) ??
    stringFrom(contentRecord.content_hash) ??
    stringFrom(record.sha256) ??
    stringFrom(record.contentHash) ??
    stringFrom(record.content_hash) ??
    fileObject?.sha256;
  const source = recordValue(record.source);
  const fields = compactRecord({
    encoding,
    contentHash: sha256,
    sha256,
    byteLen: decoded.bytes?.byteLength,
    base64: decoded.base64,
    data: decoded.base64,
    dataUrl: decoded.dataUrl,
    storage: stringFrom(contentRecord.storage) ?? stringFrom(record.storage),
    fileObjectId: fileObject?.fileObjectId,
    fileObjectSha256: fileObject?.sha256,
    fileObjectSizeBytes: fileObject?.sizeBytes,
    chunkCount: fileObject?.chunkCount ?? chunks.length,
    chunks: chunks.map((chunk) => ({
      chunkId: chunk.chunkId,
      sha256: chunk.sha256,
      sizeBytes: chunk.sizeBytes,
      byteOffset: chunk.byteOffset
    })),
    source
  });

  return {
    artifactId: stringFrom(record.artifactId) ?? stringFrom(record.artifact_id),
    workspaceId: stringFrom(record.workspaceId) ?? stringFrom(record.workspace_id),
    spaceId: stringFrom(record.spaceId) ?? stringFrom(record.space_id),
    layerId: stringFrom(record.layerId) ?? stringFrom(record.layer_id),
    path: stringFrom(record.path) ?? stringFrom(record.logicalPath) ?? stringFrom(record.logical_path),
    type: artifactTypeFromWire(record.type ?? record.artifact_kind),
    content: {
      encoding,
      mediaType,
      sha256,
      value: decoded.value,
      bytes: decoded.bytes,
      base64: decoded.base64,
      dataUrl: decoded.dataUrl,
      fileObject,
      chunks,
      storage: stringFrom(contentRecord.storage) ?? stringFrom(record.storage)
    },
    source,
    fields
  };
}

export function normalizeArtifactPreviewWindowPayload(payload: unknown): ArtifactPreviewWindowPayload | undefined {
  const record = recordValue(payload);
  if (!record) {
    return undefined;
  }

  const windowRecord = recordValue(record.window);
  const start = numberFrom(windowRecord?.start);
  const limit = numberFrom(windowRecord?.limit);
  const count = numberFrom(windowRecord?.count);
  if (start === undefined || limit === undefined || count === undefined) {
    return undefined;
  }

  const preview = previewModelFromWire(record.preview);
  const diff = diffModelFromWire(record.diff);
  const source = recordValue(record.source);
  const columnWindowRecord = recordValue(record.columnWindow ?? record.column_window) ?? recordValue(diff?.fields.columnWindow);
  const columnStart = numberFrom(columnWindowRecord?.columnStart ?? columnWindowRecord?.column_start);
  const columnLimit = numberFrom(columnWindowRecord?.columnLimit ?? columnWindowRecord?.column_limit);
  const hasLongLines = Boolean(columnWindowRecord?.hasLongLines ?? columnWindowRecord?.has_long_lines ?? diff?.fields.hasLongLines);
  const fields = compactRecord({
    ...(recordValue(record.fields) ?? {}),
    window: {
      start,
      limit,
      count,
      totalLines: numberFrom(windowRecord?.totalLines ?? windowRecord?.total_lines),
      hasMore: Boolean(windowRecord?.hasMore ?? windowRecord?.has_more),
      hasMoreBefore: Boolean(windowRecord?.hasMoreBefore ?? windowRecord?.has_more_before),
      hasMoreAfter: Boolean(windowRecord?.hasMoreAfter ?? windowRecord?.has_more_after),
      columnStart,
      columnLimit,
      hasLongLines
    },
    source
  });

  return {
    artifactId: stringFrom(record.artifactId) ?? stringFrom(record.artifact_id),
    workspaceId: stringFrom(record.workspaceId) ?? stringFrom(record.workspace_id),
    spaceId: stringFrom(record.spaceId) ?? stringFrom(record.space_id),
    layerId: stringFrom(record.layerId) ?? stringFrom(record.layer_id),
    baseLayerId: stringFrom(record.baseLayerId) ?? stringFrom(record.base_layer_id),
    path: stringFrom(record.path) ?? stringFrom(record.logicalPath) ?? stringFrom(record.logical_path),
    type: artifactTypeFromWire(record.type ?? record.artifact_kind),
    preview,
    diff,
    window: {
      start,
      limit,
      count,
      totalLines: numberFrom(windowRecord?.totalLines ?? windowRecord?.total_lines),
      hasMore: Boolean(windowRecord?.hasMore ?? windowRecord?.has_more),
      hasMoreBefore: Boolean(windowRecord?.hasMoreBefore ?? windowRecord?.has_more_before),
      hasMoreAfter: Boolean(windowRecord?.hasMoreAfter ?? windowRecord?.has_more_after),
      columnStart,
      columnLimit,
      hasLongLines
    },
    source,
    fields
  };
}

export function normalizeLayerStep(payload: unknown): LayerStep | undefined {
  const record = recordValue(payload);
  if (!record) {
    return undefined;
  }

  const stepId = stringFrom(record.stepId) ?? stringFrom(record.step_id) ?? stringFrom(record.id);
  const layerId = stringFrom(record.layerId) ?? stringFrom(record.layer_id);
  if (!stepId || !layerId) {
    return undefined;
  }

  const diffs = normalizeStepDiffWindows(record.diffs);
  const files = normalizeStepChangedFiles(record.files);
  const diffStats = layerStepDiffStatsFromWire(record.diffStats ?? record.diff_stats, diffs);
  const fields = compactRecord({
    ...(recordValue(record.fields) ?? {}),
    source: recordValue(record.source)
  });

  return {
    id: stringFrom(record.id) ?? stepId,
    stepId,
    layerId,
    baseLayerId: stringFrom(record.baseLayerId) ?? stringFrom(record.base_layer_id),
    baseTreeId: stringFrom(record.baseTreeId) ?? stringFrom(record.base_tree_id),
    rootTreeId: stringFrom(record.rootTreeId) ?? stringFrom(record.root_tree_id),
    capturedAt: numberLikeFrom(record.capturedAt ?? record.captured_at),
    startedAt: stringFrom(record.startedAt) ?? stringFrom(record.started_at),
    completedAt: stringFrom(record.completedAt) ?? stringFrom(record.completed_at),
    changedFiles: numberLikeFrom(record.changedFiles ?? record.changed_files) ?? files.length ?? diffs.length,
    diffStats,
    files,
    diffs,
    fields
  };
}

export function normalizeLayerSteps(payload: unknown): LayerStep[] {
  const items = arrayValue(recordValue(payload)?.items) ?? arrayValue(payload);
  return items
    .map(normalizeLayerStep)
    .filter((step): step is LayerStep => Boolean(step));
}

export function normalizeStepChangedFiles(payload: unknown): StepChangedFile[] {
  return arrayValue(payload)
    .map(normalizeStepChangedFile)
    .filter((file): file is StepChangedFile => Boolean(file));
}

function normalizeStepChangedFile(payload: unknown): StepChangedFile | undefined {
  const record = recordValue(payload);
  if (!record) {
    return undefined;
  }
  const path = stringFrom(record.path);
  if (!path) {
    return undefined;
  }
  return {
    path,
    name: stringFrom(record.name) ?? path.split("/").pop() ?? path,
    action: stringFrom(record.action) ?? stringFrom(record.state) ?? "modified",
    lensId: stringFrom(record.lensId) ?? stringFrom(record.lens_id),
    mediaType: stringFrom(record.mediaType) ?? stringFrom(record.media_type),
    baseLayerId: stringFrom(record.baseLayerId) ?? stringFrom(record.base_layer_id),
    baseFileObjectId: stringFrom(record.baseFileObjectId) ?? stringFrom(record.base_file_object_id),
    targetFileObjectId: stringFrom(record.targetFileObjectId) ?? stringFrom(record.target_file_object_id),
    sizeBytes: numberLikeFrom(record.sizeBytes ?? record.size_bytes),
    access: recordValue(record.access) as ArtifactAccessDecision | undefined
  };
}

export function normalizeStepDiffWindow(payload: unknown): StepDiffWindow | undefined {
  const record = recordValue(payload);
  if (!record) {
    return undefined;
  }

  const diff = diffModelFromWire(record.diff) ?? diffModelFromWire(record);
  if (!diff) {
    return undefined;
  }

  const fields = diff.fields ?? {};
  const metadata = diff.metadata;
  const path = stringFrom(record.path) ?? stringFrom(fields.path) ?? metadata?.path ?? "";
  const title = (stringFrom(record.title) ?? path) || "Lens diff";
  const lineWindow = metadata?.lineWindow ?? diffLineWindowFromWire(fields.lineWindow);
  const columnWindow =
    metadata?.columnWindow ??
    normalizeDiffColumnWindow(fields.columnWindow) ??
    normalizeDiffColumnWindow(fields);
  const totalDiffLineCount =
    metadata?.totalDiffLineCount ??
    numberLikeFrom(fields.totalDiffLineCount) ??
    numberLikeFrom(fields.totalDiffLines);
  const totalLineCount =
    metadata?.totalLineCount ??
    numberLikeFrom(fields.totalLineCount) ??
    totalDiffLineCount;
  const renderedLineCount =
    metadata?.renderedLineCount ??
    numberLikeFrom(fields.renderedLineCount) ??
    diff.hunks.reduce((count, hunk) => count + hunk.lines.length, 0);
  const windowStart =
    numberLikeFrom(fields.windowStart) ??
    numberLikeFrom(fields.start) ??
    (lineWindow ? Math.max(0, lineWindow.startLine - 1) : undefined);
  const windowEnd =
    numberLikeFrom(fields.windowEnd) ??
    numberLikeFrom(fields.end) ??
    (lineWindow ? lineWindow.endLine : undefined);
  const windowLimit =
    numberLikeFrom(fields.windowLimit) ??
    numberLikeFrom(fields.limit) ??
    (windowStart !== undefined && windowEnd !== undefined ? Math.max(0, windowEnd - windowStart) : undefined);
  const hasMoreBefore =
    metadata?.hasMoreBefore ??
    booleanFrom(fields.hasMoreBefore) ??
    (windowStart !== undefined ? windowStart > 0 : false);
  const hasMoreAfter =
    metadata?.hasMoreAfter ??
    booleanFrom(fields.hasMoreAfter) ??
    booleanFrom(fields.hasMore) ??
    (windowEnd !== undefined && totalDiffLineCount !== undefined ? windowEnd < totalDiffLineCount : false);
  const hasMoreColumns =
    metadata?.hasMoreColumns ??
    booleanFrom(fields.hasMoreColumns) ??
    booleanFrom(fields.lineTextTruncated) ??
    Boolean(columnWindow?.hasMoreColumns);

  return {
    path,
    state: stringFrom(record.state) ?? stringFrom(fields.state) ?? metadata?.state,
    lensId: stringFrom(record.lensId) ?? stringFrom(record.lens_id) ?? stringFrom(fields.lensId) ?? metadata?.lensId,
    title,
    diff,
    message: stringFrom(record.message),
    source: stringFrom(record.source) ?? stringFrom(fields.source) ?? metadata?.source,
    layerId: stringFrom(record.layerId) ?? stringFrom(record.layer_id) ?? stringFrom(fields.layerId) ?? metadata?.layerId,
    stepId: stringFrom(record.stepId) ?? stringFrom(record.step_id) ?? stringFrom(fields.stepId) ?? metadata?.stepId,
    lineWindow,
    columnWindow,
    totalLineCount,
    totalDiffLineCount,
    renderedLineCount,
    windowStart,
    windowEnd,
    windowLimit,
    hasMoreBefore,
    hasMoreAfter,
    hasMoreColumns,
    fields: {
      ...(recordValue(record.fields) ?? {}),
      ...fields
    }
  };
}

export function normalizeStepDiffWindows(payload: unknown): StepDiffWindow[] {
  return arrayValue(payload)
    .map(normalizeStepDiffWindow)
    .filter((diff): diff is StepDiffWindow => Boolean(diff));
}

function previewModelFromWire(value: unknown): PreviewModel | undefined {
  const record = recordValue(value);
  if (!record) {
    return undefined;
  }
  const kind = stringFrom(record.kind);
  const title = stringFrom(record.title);
  const mediaType = stringFrom(record.mediaType) ?? stringFrom(record.media_type);
  if (!kind || !title || !mediaType) {
    return undefined;
  }

  return {
    kind: kind as PreviewKind,
    title,
    body: stringFrom(record.body),
    mediaType,
    language: stringFrom(record.language),
    fields: recordValue(record.fields) ?? {}
  };
}

function diffModelFromWire(value: unknown): DiffModel | undefined {
  const record = recordValue(value);
  if (!record) {
    return undefined;
  }
  const kind = stringFrom(record.kind);
  const summary = stringFrom(record.summary);
  if (!kind || !summary) {
    return undefined;
  }
  const fields = recordValue(record.fields) ?? {};
  const hunks = arrayValue(record.hunks).map(diffHunkFromWire).filter((hunk): hunk is DiffModel["hunks"][number] => Boolean(hunk));
  const metadata = diffModelMetadataFromWire(record.metadata, fields, hunks);

  return {
    kind: kind as DiffModel["kind"],
    summary,
    hunks,
    ...(metadata ? { metadata } : {}),
    fields
  };
}

function diffHunkFromWire(value: unknown): DiffModel["hunks"][number] | undefined {
  const record = recordValue(value);
  if (!record) {
    return undefined;
  }
  const oldStart = numberFrom(record.oldStart ?? record.old_start);
  const oldLines = numberFrom(record.oldLines ?? record.old_lines);
  const newStart = numberFrom(record.newStart ?? record.new_start);
  const newLines = numberFrom(record.newLines ?? record.new_lines);
  if (oldStart === undefined || oldLines === undefined || newStart === undefined || newLines === undefined) {
    return undefined;
  }

  return {
    oldStart,
    oldLines,
    newStart,
    newLines,
    lines: arrayValue(record.lines).map(diffLineFromWire).filter((line): line is DiffModel["hunks"][number]["lines"][number] => Boolean(line))
  };
}

function diffLineFromWire(value: unknown): DiffModel["hunks"][number]["lines"][number] | undefined {
  const record = recordValue(value);
  if (!record) {
    return undefined;
  }
  const op = stringFrom(record.op);
  const text =
    typeof record.text === "string"
      ? record.text
      : typeof record.textSegment === "string"
        ? record.textSegment
        : typeof record.text_segment === "string"
          ? record.text_segment
          : undefined;
  if ((op !== "equal" && op !== "insert" && op !== "delete") || text === undefined) {
    return undefined;
  }

  return {
    op,
    oldLine: numberFrom(record.oldLine ?? record.old_line),
    newLine: numberFrom(record.newLine ?? record.new_line),
    text,
    ...diffLineSegmentFromWire(record, text)
  };
}

function diffModelMetadataFromWire(
  value: unknown,
  fields: Record<string, unknown>,
  hunks: DiffModel["hunks"]
): DiffModelMetadata | undefined {
  const record = recordValue(value) ?? {};
  const actualLineCount = hunks.reduce((count, hunk) => count + hunk.lines.length, 0);
  const windowStart = numberLikeFrom(record.windowStart ?? record.window_start ?? fields.windowStart ?? fields.start);
  const windowEnd = numberLikeFrom(record.windowEnd ?? record.window_end ?? fields.windowEnd ?? fields.end);
  const lineWindow =
    diffLineWindowFromWire(record.lineWindow ?? record.line_window ?? fields.lineWindow) ??
    diffLineWindowFromLegacyFields(windowStart, windowEnd);
  const columnWindow =
    normalizeDiffColumnWindow(record.columnWindow ?? record.column_window ?? fields.columnWindow) ??
    normalizeDiffColumnWindow(record) ??
    normalizeDiffColumnWindow(fields);
  const totalDiffLineCount =
    numberLikeFrom(record.totalDiffLineCount ?? record.total_diff_line_count) ??
    numberLikeFrom(fields.totalDiffLineCount) ??
    numberLikeFrom(fields.totalDiffLines);
  const totalLineCount =
    numberLikeFrom(record.totalLineCount ?? record.total_line_count) ??
    numberLikeFrom(fields.totalLineCount) ??
    totalDiffLineCount;
  const renderedLineCount =
    numberLikeFrom(record.renderedLineCount ?? record.rendered_line_count) ??
    numberLikeFrom(fields.renderedLineCount) ??
    actualLineCount;
  const hasMoreBefore =
    booleanFrom(record.hasMoreBefore ?? record.has_more_before) ??
    booleanFrom(fields.hasMoreBefore) ??
    (windowStart !== undefined ? windowStart > 0 : undefined);
  const hasMoreAfter =
    booleanFrom(record.hasMoreAfter ?? record.has_more_after) ??
    booleanFrom(fields.hasMoreAfter) ??
    booleanFrom(fields.hasMore) ??
    (windowEnd !== undefined && totalDiffLineCount !== undefined ? windowEnd < totalDiffLineCount : undefined);
  const hasMoreColumns =
    booleanFrom(record.hasMoreColumns ?? record.has_more_columns) ??
    booleanFrom(fields.hasMoreColumns) ??
    booleanFrom(fields.lineTextTruncated) ??
    columnWindow?.hasMoreColumns;
  const virtualization = diffVirtualizationFromWire(record.virtualization ?? fields.virtualization);
  const metadata = compactRecord({
    totalLineCount,
    totalDiffLineCount,
    renderedLineCount,
    lineWindow,
    columnWindow,
    hasMoreBefore,
    hasMoreAfter,
    hasMoreColumns,
    truncated:
      booleanFrom(record.truncated) ??
      booleanFrom(fields.truncated) ??
      booleanFrom(fields.oldTruncated) ??
      booleanFrom(fields.newTruncated),
    lineTextTruncated: booleanFrom(record.lineTextTruncated ?? record.line_text_truncated) ?? booleanFrom(fields.lineTextTruncated),
    virtualization,
    source: stringFrom(record.source) ?? stringFrom(fields.source),
    layerId: stringFrom(record.layerId) ?? stringFrom(record.layer_id) ?? stringFrom(fields.layerId),
    stepId: stringFrom(record.stepId) ?? stringFrom(record.step_id) ?? stringFrom(fields.stepId),
    path: stringFrom(record.path) ?? stringFrom(fields.path),
    state: stringFrom(record.state) ?? stringFrom(fields.state),
    lensId: stringFrom(record.lensId) ?? stringFrom(record.lens_id) ?? stringFrom(fields.lensId)
  }) as DiffModelMetadata;

  return Object.keys(metadata).length > 0 ? metadata : undefined;
}

function diffLineSegmentFromWire(record: Record<string, unknown>, fallbackText: string): Partial<DiffModel["hunks"][number]["lines"][number]> {
  const textSegment = stringValue(record.textSegment ?? record.text_segment);
  const columnWindow = normalizeDiffColumnWindow(record);
  const textLength =
    numberLikeFrom(record.textLength ?? record.text_length) ??
    columnWindow?.textLength ??
    (textSegment !== undefined ? Array.from(fallbackText).length : undefined);
  const columnStart =
    numberLikeFrom(record.columnStart ?? record.column_start) ??
    columnWindow?.columnStart;
  const columnEnd =
    numberLikeFrom(record.columnEnd ?? record.column_end) ??
    columnWindow?.columnEnd;
  const hasMoreColumns =
    booleanFrom(record.hasMoreColumns ?? record.has_more_columns) ??
    columnWindow?.hasMoreColumns;

  return compactRecord({
    textSegment,
    textLength,
    columnStart,
    columnEnd,
    hasMoreColumns
  }) as Partial<DiffModel["hunks"][number]["lines"][number]>;
}

function diffLineWindowFromWire(value: unknown): DiffLineWindow | undefined {
  const record = recordValue(value);
  if (!record) {
    return undefined;
  }

  const startLine = numberLikeFrom(record.startLine ?? record.start_line ?? record.start);
  const limit = numberLikeFrom(record.limit);
  const endLine =
    numberLikeFrom(record.endLine ?? record.end_line ?? record.end) ??
    (startLine !== undefined && limit !== undefined ? startLine + limit - 1 : undefined);

  return startLine !== undefined && endLine !== undefined
    ? { startLine, endLine: Math.max(startLine, endLine) }
    : undefined;
}

function diffLineWindowFromLegacyFields(windowStart: number | undefined, windowEnd: number | undefined): DiffLineWindow | undefined {
  if (windowStart === undefined || windowEnd === undefined) {
    return undefined;
  }

  const startLine = Math.max(1, windowStart + 1);
  return {
    startLine,
    endLine: Math.max(startLine, windowEnd)
  };
}

function diffVirtualizationFromWire(value: unknown): DiffModelMetadata["virtualization"] | undefined {
  const record = recordValue(value);
  if (!record) {
    return undefined;
  }

  const virtualization = compactRecord({
    strategy: stringFrom(record.strategy),
    maxRenderedLineCount: numberLikeFrom(record.maxRenderedLineCount ?? record.max_rendered_line_count),
    maxRenderedColumnCount: numberLikeFrom(record.maxRenderedColumnCount ?? record.max_rendered_column_count),
    rowHeightPx: numberLikeFrom(record.rowHeightPx ?? record.row_height_px),
    overscanLineCount: numberLikeFrom(record.overscanLineCount ?? record.overscan_line_count)
  }) as DiffModelMetadata["virtualization"];

  return virtualization && Object.keys(virtualization).length > 0 ? virtualization : undefined;
}

function layerStepDiffStatsFromWire(value: unknown, diffs: StepDiffWindow[]): LayerStepDiffStats {
  const record = recordValue(value);
  const additions =
    numberLikeFrom(record?.additions) ??
    diffs.reduce((count, diff) => count + (numberLikeFrom(diff.diff.fields.additions) ?? 0), 0);
  const deletions =
    numberLikeFrom(record?.deletions) ??
    numberLikeFrom(record?.removals) ??
    diffs.reduce((count, diff) => count + (numberLikeFrom(diff.diff.fields.deletions) ?? 0), 0);
  const files = numberLikeFrom(record?.files) ?? diffs.length;

  return {
    files,
    additions,
    deletions,
    removals: numberLikeFrom(record?.removals) ?? deletions
  };
}

export function createPreviewModelFromArtifactContent(input: {
  payload: ArtifactContentPayload;
  artifact?: Artifact;
  kind?: PreviewKind;
  title?: string;
  fields?: Record<string, unknown>;
}): PreviewModel | undefined {
  const { payload, artifact } = input;
  const value = payload.content.dataUrl ?? payload.content.value ?? payload.content.base64;
  const mediaType = payload.content.mediaType;
  const kind = input.kind ?? inferPreviewKindFromContent(mediaType, payload.path ?? artifact?.location);
  const fields = {
    ...(artifact?.fileObject ? { fileObjectId: artifact.fileObject.fileObjectId } : {}),
    ...(artifact?.rootTreeId ? { rootTreeId: artifact.rootTreeId } : {}),
    ...payload.fields,
    ...(payload.content.base64 ? { data: payload.content.base64, base64: payload.content.base64 } : {}),
    ...(payload.content.dataUrl ? { dataUrl: payload.content.dataUrl, src: payload.content.dataUrl } : {}),
    ...(payload.content.bytes ? { byteLen: payload.content.bytes.byteLength } : {}),
    ...(input.fields ?? {})
  };

  if (!value && !payload.content.bytes && payload.content.chunks.length === 0 && !payload.content.fileObject) {
    return undefined;
  }

  return {
    kind,
    title: input.title ?? artifact?.name ?? payload.path ?? "Artifact preview",
    body: value ?? "",
    mediaType,
    fields
  };
}

function layerFromWire(layer: Layer | LayerWire): Layer {
  const wire = layer as LayerWire;
  const head = layerHeadFromWire(wire.head ?? wire.layerHead ?? wire.layer_head);
  const rootTreeId = wire.rootTreeId ?? wire.root_tree_id ?? head?.rootTreeId;

  return {
    id: wire.id ?? "",
    spaceId: wire.spaceId ?? wire.space_id ?? "",
    parentId: wire.parentId ?? wire.parent_id,
    name: wire.name ?? "Untitled Layer",
    kind: wire.kind ?? "proposal",
    status: wire.status ?? "active",
    summary: wire.summary ?? "",
    artifactIds: wire.artifactIds ?? wire.artifact_ids ?? [],
    stepIds: wire.stepIds ?? wire.step_ids ?? [],
    gateIds: wire.gateIds ?? wire.gate_ids ?? [],
    rootTreeId,
    policyEpoch: wire.policyEpoch ?? wire.policy_epoch ?? head?.policyEpoch,
    serverCursor: wire.serverCursor ?? wire.server_cursor ?? head?.serverCursor,
    head
  };
}

function layerHeadFromWire(head: LayerHeadWire | undefined): LayerHeadMetadata | undefined {
  if (!head) {
    return undefined;
  }

  return compactRecord({
    layerStateId: head.layerStateId ?? head.layer_state_id,
    rootTreeId: head.rootTreeId ?? head.root_tree_id,
    policyEpoch: head.policyEpoch ?? head.policy_epoch,
    serverCursor: head.serverCursor ?? head.server_cursor,
    updatedAt: head.updatedAt ?? head.updated_at,
    updatedByAccountId: head.updatedByAccountId ?? head.updated_by_account_id
  }) as LayerHeadMetadata;
}

function artifactFromWire(artifact: Artifact | ArtifactWire): Artifact {
  const wire = artifact as ArtifactWire;
  const fileObject = fileObjectFromWire(wire.fileObject ?? wire.file_object);
  const chunks = normalizeChunks(wire.chunks);
  const fileObjectId =
    wire.fileObjectId ??
    wire.file_object_id ??
    wire.current_file_object_id ??
    fileObject?.fileObjectId;
  const byteLen = numberFrom(wire.byteLen) ?? numberFrom(wire.byte_len) ?? numberFrom(wire.sizeBytes) ?? numberFrom(wire.size_bytes);
  const contentHash = wire.contentHash ?? wire.content_hash ?? wire.sha256 ?? fileObject?.sha256;

  return {
    id: wire.id ?? wire.artifact_id ?? "",
    spaceId: wire.spaceId ?? wire.space_id ?? "",
    layerId: wire.layerId ?? wire.layer_id,
    name: wire.name ?? wire.path ?? wire.logical_path ?? "Untitled artifact",
    type: artifactTypeFromWire(wire.type ?? wire.artifact_kind) ?? "file",
    summary: wire.summary ?? "",
    location: wire.location ?? wire.logical_path ?? wire.path ?? "",
    updatedAt: wire.updatedAt ?? wire.updated_at ?? new Date(0).toISOString(),
    sizeLabel: wire.sizeLabel ?? wire.size_label ?? (byteLen === undefined ? "" : formatByteLabel(byteLen)),
    proofIds: wire.proofIds ?? wire.proof_ids ?? [],
    access: wire.access,
    mediaType: wire.mediaType ?? wire.media_type ?? fileObject?.mediaType,
    contentHash,
    byteLen,
    lensId: wire.lensId ?? wire.lens_id,
    rootTreeId: wire.rootTreeId ?? wire.root_tree_id,
    currentTreeId: wire.currentTreeId ?? wire.current_tree_id,
    fileObjectId,
    fileObject,
    chunks: chunks.length > 0 ? chunks : fileObject?.chunks,
    preview: wire.preview
  };
}

function fileObjectFromWire(fileObject: FileObjectWire | undefined): FileObjectMetadata | undefined {
  if (!fileObject) {
    return undefined;
  }

  const fileObjectId = fileObject.fileObjectId ?? fileObject.file_object_id ?? fileObject.id;
  if (!fileObjectId) {
    return undefined;
  }

  const chunks = normalizeChunks(fileObject.chunks);
  return {
    id: fileObjectId,
    fileObjectId,
    sha256: fileObject.sha256,
    sizeBytes: numberFrom(fileObject.sizeBytes) ?? numberFrom(fileObject.size_bytes),
    mediaType: fileObject.mediaType ?? fileObject.media_type,
    chunkCount: numberFrom(fileObject.chunkCount) ?? numberFrom(fileObject.chunk_count) ?? chunks.length,
    chunks
  };
}

function chunkFromWire(chunk: ChunkWire | undefined): ChunkMetadata | undefined {
  if (!chunk) {
    return undefined;
  }

  const chunkId = chunk.chunkId ?? chunk.chunk_id ?? chunk.id;
  if (!chunkId) {
    return undefined;
  }

  return {
    id: chunkId,
    chunkId,
    sha256: chunk.sha256,
    sizeBytes: numberFrom(chunk.sizeBytes) ?? numberFrom(chunk.size_bytes),
    byteOffset: numberFrom(chunk.byteOffset) ?? numberFrom(chunk.byte_offset),
    chunkIndex: numberFrom(chunk.chunkIndex) ?? numberFrom(chunk.chunk_index),
    mediaType: chunk.mediaType ?? chunk.media_type,
    compression: chunk.compression,
    state: chunk.state,
    objectKey: chunk.objectKey ?? chunk.object_key,
    value: stringFrom(chunk.value) ?? stringFrom(chunk.content) ?? stringFrom(chunk.data),
    encoding: chunk.encoding
  };
}

function normalizeChunks(chunks: unknown): ChunkMetadata[] {
  return arrayValue(chunks)
    .map((chunk) => chunkFromWire(chunk as ChunkWire))
    .filter((chunk): chunk is ChunkMetadata => Boolean(chunk))
    .sort((left, right) => (left.chunkIndex ?? 0) - (right.chunkIndex ?? 0));
}

interface DecodedContentValue {
  value?: string;
  bytes?: Uint8Array;
  base64?: string;
  dataUrl?: string;
}

function contentSourceValue(value: unknown, chunks: ChunkMetadata[]): string | undefined {
  const direct = contentScalarValue(value);
  if (direct !== undefined) {
    return direct;
  }

  const chunkValues = chunks.map((chunk) => chunk.value).filter((chunkValue): chunkValue is string => Boolean(chunkValue));
  return chunkValues.length > 0 ? chunkValues.join("") : undefined;
}

function decodeContentValue(value: string | undefined, encoding: string | undefined, mediaType: string): DecodedContentValue {
  if (value === undefined) {
    return {};
  }

  if (encoding === "base64") {
    const base64 = normalizeBase64(value);
    const bytes = decodeBase64(base64);
    return decodedBytesValue(bytes, mediaType, base64);
  }

  if (encoding === "hex") {
    const bytes = decodeHex(value);
    return decodedBytesValue(bytes, mediaType, encodeBase64(bytes));
  }

  if (isBinaryMediaType(mediaType) && looksLikeDataUrl(value)) {
    return {
      value,
      dataUrl: value,
      base64: value.slice(value.indexOf(",") + 1)
    };
  }

  return { value };
}

function decodedBytesValue(bytes: Uint8Array, mediaType: string, base64: string): DecodedContentValue {
  if (isTextMediaType(mediaType)) {
    return {
      value: decodeUtf8(bytes),
      bytes,
      base64
    };
  }

  if (mediaType.startsWith("image/")) {
    return {
      value: `data:${mediaType};base64,${base64}`,
      bytes,
      base64,
      dataUrl: `data:${mediaType};base64,${base64}`
    };
  }

  return {
    value: base64,
    bytes,
    base64
  };
}

function contentScalarValue(value: unknown): string | undefined {
  if (typeof value === "string") {
    return value;
  }

  if (value === undefined || value === null) {
    return undefined;
  }

  if (typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }

  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return undefined;
  }
}

function normalizeContentEncoding(encoding: string | undefined): string | undefined {
  const normalized = encoding?.trim().toLowerCase();
  if (normalized === "base64" || normalized === "hex" || normalized === "json" || normalized === "text" || normalized === "utf8") {
    return normalized;
  }
  return normalized;
}

function isTextMediaType(mediaType: string): boolean {
  return (
    mediaType.startsWith("text/") ||
    mediaType.includes("json") ||
    mediaType.includes("xml") ||
    mediaType.includes("javascript") ||
    mediaType.includes("typescript") ||
    mediaType.includes("toml") ||
    mediaType.includes("yaml")
  );
}

function isBinaryMediaType(mediaType: string): boolean {
  return mediaType.startsWith("image/") || mediaType === "application/octet-stream" || mediaType.startsWith("audio/") || mediaType.startsWith("video/");
}

function looksLikeDataUrl(value: string): boolean {
  return /^data:[^,]+;base64,/i.test(value);
}

function normalizeBase64(value: string): string {
  const commaIndex = value.indexOf(",");
  const raw = looksLikeDataUrl(value) && commaIndex >= 0 ? value.slice(commaIndex + 1) : value;
  return raw.replace(/\s+/g, "");
}

function decodeUtf8(bytes: Uint8Array): string {
  return new TextDecoder("utf-8", { fatal: false }).decode(bytes);
}

function decodeHex(value: string): Uint8Array {
  const clean = value.trim().replace(/^0x/i, "").replace(/\s+/g, "");
  if (clean.length % 2 !== 0 || /[^0-9a-f]/i.test(clean)) {
    return new Uint8Array();
  }

  const bytes = new Uint8Array(clean.length / 2);
  for (let index = 0; index < clean.length; index += 2) {
    bytes[index / 2] = Number.parseInt(clean.slice(index, index + 2), 16);
  }
  return bytes;
}

const BASE64_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

function decodeBase64(value: string): Uint8Array {
  const clean = normalizeBase64(value).replace(/=+$/, "");
  const bytes: number[] = [];
  let buffer = 0;
  let bits = 0;

  for (const char of clean) {
    const sixBits = BASE64_ALPHABET.indexOf(char);
    if (sixBits < 0) {
      continue;
    }

    buffer = (buffer << 6) | sixBits;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      bytes.push((buffer >> bits) & 0xff);
    }
  }

  return new Uint8Array(bytes);
}

function encodeBase64(bytes: Uint8Array): string {
  let output = "";
  let index = 0;

  for (; index + 2 < bytes.length; index += 3) {
    output +=
      BASE64_ALPHABET[bytes[index] >> 2] +
      BASE64_ALPHABET[((bytes[index] & 0x03) << 4) | (bytes[index + 1] >> 4)] +
      BASE64_ALPHABET[((bytes[index + 1] & 0x0f) << 2) | (bytes[index + 2] >> 6)] +
      BASE64_ALPHABET[bytes[index + 2] & 0x3f];
  }

  if (index < bytes.length) {
    output += BASE64_ALPHABET[bytes[index] >> 2];
    if (index + 1 < bytes.length) {
      output += BASE64_ALPHABET[((bytes[index] & 0x03) << 4) | (bytes[index + 1] >> 4)];
      output += BASE64_ALPHABET[(bytes[index + 1] & 0x0f) << 2];
      output += "=";
    } else {
      output += BASE64_ALPHABET[(bytes[index] & 0x03) << 4];
      output += "==";
    }
  }

  return output;
}

function inferPreviewKindFromContent(mediaType: string, path: string | undefined): PreviewKind {
  if (mediaType.startsWith("image/")) {
    return "image";
  }

  const extension = extensionFromPath(path);
  if (
    mediaType.includes("javascript") ||
    mediaType.includes("json") ||
    mediaType.includes("typescript") ||
    ["css", "html", "js", "json", "jsx", "rs", "ts", "tsx"].includes(extension ?? "")
  ) {
    return "code";
  }

  if (mediaType.startsWith("text/") || ["md", "mdx", "txt"].includes(extension ?? "")) {
    return "text";
  }

  return "raw";
}

function artifactTypeFromWire(value: unknown): ArtifactType | undefined {
  return value === "file" ||
    value === "note" ||
    value === "image" ||
    value === "report" ||
    value === "proof" ||
    value === "step-output"
    ? value
    : undefined;
}

function compactRecord(values: Record<string, unknown>): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(values).filter(([, value]) => value !== undefined && value !== null)
  );
}

function firstRecord(...values: unknown[]): Record<string, unknown> | undefined {
  for (const value of values) {
    const record = recordValue(value);
    if (record) {
      return record;
    }
  }
  return undefined;
}

function recordValue(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;
}

function arrayValue(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

function stringFrom(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function stringValue(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function numberFrom(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function numberLikeFrom(value: unknown): number | undefined {
  if (typeof value === "number" && Number.isFinite(value)) {
    return Math.floor(value);
  }

  if (typeof value === "string" && value.trim().length > 0) {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? Math.floor(parsed) : undefined;
  }

  return undefined;
}

function booleanFrom(value: unknown): boolean | undefined {
  if (typeof value === "boolean") {
    return value;
  }

  if (value === "true") {
    return true;
  }

  if (value === "false") {
    return false;
  }

  return undefined;
}

function extensionFromPath(path: string | undefined): string | undefined {
  const name = path?.split(/[\\/]/).pop();
  const dotIndex = name?.lastIndexOf(".") ?? -1;
  if (!name || dotIndex < 0 || dotIndex === name.length - 1) {
    return undefined;
  }
  return name.slice(dotIndex + 1).toLowerCase();
}

function formatByteLabel(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} B`;
  }

  const units = ["KB", "MB", "GB"];
  let size = bytes / 1024;
  let unitIndex = 0;

  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024;
    unitIndex += 1;
  }

  return `${size.toFixed(size >= 10 ? 0 : 1)} ${units[unitIndex]}`;
}

function layerAccessPolicyFromWire(policy: LayerAccessPolicy | LayerAccessPolicyWire): LayerAccessPolicy {
  const wire = policy as LayerAccessPolicyWire;
  return {
    schema: wire.schema ?? "layrs.layer_access.v1",
    workspaceId: wire.workspaceId ?? wire.workspace_id ?? "",
    spaceId: wire.spaceId ?? wire.space_id ?? "",
    layerId: wire.layerId ?? wire.layer_id ?? "",
    policyEpoch: wire.policyEpoch ?? wire.policy_epoch ?? 0,
    generatedAt: wire.generatedAt ?? wire.generated_at ?? new Date(0).toISOString(),
    rules: (wire.rules ?? []).map(layerAccessRuleFromWire),
    signature: layerAccessSignatureFromWire(wire.signature)
  };
}

function layerAccessPolicyToWire(policy: LayerAccessPolicy): LayerAccessPolicyWire {
  return {
    schema: policy.schema,
    workspace_id: policy.workspaceId,
    space_id: policy.spaceId,
    layer_id: policy.layerId,
    policy_epoch: policy.policyEpoch,
    generated_at: policy.generatedAt,
    rules: policy.rules.map((rule) => ({
      id: rule.id,
      path: rule.path,
      artifact_id: rule.artifactId,
      mode: rule.mode,
      visibility: rule.visibility,
      permissions: {
        read: principalSetToWire(rule.permissions.read),
        write: principalSetToWire(rule.permissions.write),
        admin: principalSetToWire(rule.permissions.admin)
      }
    })),
    signature: {
      key_id: policy.signature.keyId,
      value: policy.signature.value
    }
  };
}

function layerAccessRuleFromWire(rule: LayerAccessRule | LayerAccessRuleWire): LayerAccessRule {
  const wire = rule as LayerAccessRuleWire;
  return {
    id: wire.id ?? `rule-${cryptoSafeRandomId()}`,
    path: wire.path ?? ".",
    artifactId: wire.artifactId ?? wire.artifact_id,
    mode: wire.mode ?? "restricted",
    visibility: wire.visibility ?? "stub",
    permissions: {
      read: principalSetFromWire(wire.permissions?.read),
      write: principalSetFromWire(wire.permissions?.write),
      admin: principalSetFromWire(wire.permissions?.admin)
    }
  };
}

function layerAccessSignatureFromWire(signature: LayerAccessSignature | LayerAccessSignatureWire | undefined): LayerAccessSignature {
  const wire = signature as LayerAccessSignatureWire | undefined;
  return {
    keyId: wire?.keyId ?? wire?.key_id ?? "server_key_local",
    value: signature?.value ?? "unsigned-dev"
  };
}

function principalSetFromWire(set: LayerAccessPrincipalSetWire | undefined): LayerAccessPrincipalSet {
  return {
    accounts: set?.accounts ?? [],
    teams: set?.teams ?? []
  };
}

function principalSetToWire(set: LayerAccessPrincipalSet): LayerAccessPrincipalSetWire {
  return {
    accounts: set.accounts,
    teams: set.teams
  };
}

function legacyRegistryToLayerAccessPolicy(registry: LayerAccessRegistry): LayerAccessPolicy {
  const layer = layers.find((item) => item.id === registry.layerId);
  return {
    schema: "layrs.layer_access.v1",
    workspaceId: registry.workspaceId,
    spaceId: layer?.spaceId ?? "",
    layerId: registry.layerId,
    policyEpoch: 1,
    generatedAt: registry.updatedAt,
    rules: registry.rules.map((rule) => legacyRegistryRuleToLayerAccessRule(registry.layerId, rule)),
    signature: {
      keyId: "legacy_registry_adapter",
      value: registry.updatedAt
    }
  };
}

function legacyRegistryRuleToLayerAccessRule(layerId: LayrsId, rule: LayerAccessRegistryRule): LayerAccessRule {
  return {
    id: rule.id,
    path: `.layrs/layers/${layerId}/access.json`,
    mode: rule.mode === "none" ? "reserved_redacted" : "restricted",
    visibility: rule.mode === "none" ? "stub" : "full",
    permissions: {
      read: legacyPrincipalSet(rule, "read"),
      write: legacyPrincipalSet(rule, "write"),
      admin: legacyPrincipalSet(rule, "admin")
    }
  };
}

function legacyPrincipalSet(rule: LayerAccessRegistryRule, mode: AccessMode): LayerAccessPrincipalSet {
  if (rule.mode !== mode) {
    return { accounts: [], teams: [] };
  }
  return rule.subjectKind === "account"
    ? { accounts: [rule.subjectId], teams: [] }
    : { accounts: [], teams: [rule.subjectId] };
}

function layerAccessPolicyToLegacyRegistry(policy: LayerAccessPolicy): LayerAccessRegistry {
  const rules: LayerAccessRegistryRule[] = [];
  addLegacyRules(rules, policy, "read", policy.rules.flatMap((rule) => rule.permissions.read.accounts), "account");
  addLegacyRules(rules, policy, "read", policy.rules.flatMap((rule) => rule.permissions.read.teams), "team");
  addLegacyRules(rules, policy, "write", policy.rules.flatMap((rule) => rule.permissions.write.accounts), "account");
  addLegacyRules(rules, policy, "write", policy.rules.flatMap((rule) => rule.permissions.write.teams), "team");
  addLegacyRules(rules, policy, "admin", policy.rules.flatMap((rule) => rule.permissions.admin.accounts), "account");
  addLegacyRules(rules, policy, "admin", policy.rules.flatMap((rule) => rule.permissions.admin.teams), "team");

  return {
    id: `access-${policy.layerId}`,
    workspaceId: policy.workspaceId,
    layerId: policy.layerId,
    rules,
    updatedAt: policy.generatedAt
  };
}

function addLegacyRules(
  rules: LayerAccessRegistryRule[],
  policy: LayerAccessPolicy,
  mode: AccessMode,
  subjectIds: LayrsId[],
  subjectKind: AccessSubjectKind
): void {
  for (const subjectId of unique(subjectIds)) {
    rules.push({
      id: `access-${policy.layerId}-${mode}-${subjectKind}-${subjectId}`,
      subjectKind,
      subjectId,
      subjectName: subjectDisplayName(subjectKind, subjectId),
      mode
    });
  }
}

function subjectDisplayName(subjectKind: AccessSubjectKind, subjectId: LayrsId): string {
  if (subjectKind === "team") {
    return teams.find((team) => team.id === subjectId)?.name ?? subjectId;
  }
  return workspaceMembers.find((member) => member.accountId === subjectId)?.name ?? subjectId;
}

function unique<T>(items: T[]): T[] {
  return [...new Set(items)];
}

function cryptoSafeRandomId(): string {
  const value = globalThis.crypto?.randomUUID?.();
  return value ?? Math.random().toString(36).slice(2);
}

async function readJson(response: Response): Promise<unknown> {
  const text = await response.text();
  if (!text) {
    return undefined;
  }

  try {
    return JSON.parse(text) as unknown;
  } catch {
    return { message: text };
  }
}

function getErrorMessage(payload: unknown, fallback: string): string {
  if (typeof payload === "object" && payload) {
    if ("message" in payload && typeof payload.message === "string") {
      return payload.message;
    }
    if ("error" in payload && typeof payload.error === "string") {
      return payload.error;
    }
    if ("error" in payload && typeof payload.error === "object" && payload.error) {
      const error = payload.error as { message?: unknown };
      if (typeof error.message === "string") {
        return error.message;
      }
    }
  }

  return fallback;
}

function getErrorCode(payload: unknown): string | undefined {
  if (typeof payload !== "object" || !payload) {
    return undefined;
  }

  if ("code" in payload && typeof payload.code === "string") {
    return payload.code;
  }

  if ("error" in payload && typeof payload.error === "object" && payload.error) {
    const error = payload.error as { code?: unknown };
    if (typeof error.code === "string") {
      return error.code;
    }
  }

  return undefined;
}

function unwrapItems<T>(value: T[] | { items: T[] }): T[] {
  return Array.isArray(value) ? value : value.items;
}

function slugify(value: string): string {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}
