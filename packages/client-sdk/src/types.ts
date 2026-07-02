import type { DiffColumnWindow, DiffLineWindow, DiffModel, PreviewModel } from "./lenses/contracts";

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

