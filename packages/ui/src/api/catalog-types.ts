// Wire types for the UI navigation contracts (lot L0).
// Contract: docs/contracts/ui-catalog-and-capabilities.md
// Server: crates/layrs-store-server/src/v3/collaboration/space_{catalog,capabilities}.rs
// Capabilities are presentation hints only: the server rechecks every action.

/** `limited` = content-restricted: the project opens, paths are filtered. */
export type SpaceAccess = "full" | "limited";

/** Stored Workspace membership role; per-project rights come from policy. */
export type WorkspaceRole = "owner" | "administrator" | "member" | "auditor";

export interface SpaceCatalogWorkspace {
  workspaceId: string;
  slug: string;
  name: string;
}

export interface SpaceCatalogEntry {
  spaceId: string;
  slug: string;
  name: string;
  workspace: SpaceCatalogWorkspace;
  workspaceRole: WorkspaceRole;
  access: SpaceAccess;
  /** RFC 3339 UTC time of the latest accepted repository event; null when none or limited. */
  lastActivityAt: string | null;
}

/** `GET /v3/spaces?limit=&cursor=` */
export interface SpaceCatalogPage {
  schemaVersion: "layrs.space-catalog.v1";
  /** May hold fewer than `limit` entries, even none, while `hasMore` is true. */
  spaces: SpaceCatalogEntry[];
  /** Opaque; valid only for the principal that received it. */
  nextCursor: string | null;
  hasMore: boolean;
}

export interface SpaceCatalogQuery {
  /** 1..=100, default 50. */
  limit?: number;
  cursor?: string;
}

export interface SpaceCapabilityFlags {
  open: boolean;
  browseFiles: boolean;
  applyChanges: boolean;
  readReviews: boolean;
  proposeReviews: boolean;
  decideReviews: boolean;
  integrateReviews: boolean;
  readScopedReviews: boolean;
  proposeScopedReviews: boolean;
  decideScopedReviews: boolean;
  integrateScopedReviews: boolean;
  manageMembers: boolean;
  manageTeams: boolean;
  manageInvitations: boolean;
  managePolicy: boolean;
  manageWebhooks: boolean;
  configureProtection: boolean;
  readReleases: boolean;
  publishReleases: boolean;
  revokeReleases: boolean;
  readRuns: boolean;
  runProofs: boolean;
}

/** `GET /v3/spaces/:space/capabilities` */
export interface SpaceCapabilities {
  schemaVersion: "layrs.space-capabilities.v1";
  spaceId: string;
  /** Flags describe this epoch; refetch after any 409 `authorization_epoch_changed`. */
  authorizationEpoch: number;
  access: SpaceAccess;
  capabilities: SpaceCapabilityFlags;
}

/** `GET /v3/auth/whoami` (snake_case, as served since V3). */
export interface WhoAmI {
  principal_id: string;
  actor_id: string;
  display_name: string;
  kind: "account" | "service" | "bootstrap_admin";
  /** UI hint: every instance-administration endpoint rechecks it. */
  instance_administrator: boolean;
}

/** Error body shared by these routes. */
export interface ApiErrorBody {
  error: { code: string };
}

/** Desktop `native_remote` actions (Tauri); `root` is ignored by both. */
export type CatalogRemoteAction =
  | { action: "listSpaces"; limit?: number; cursor?: string }
  | { action: "spaceCapabilities"; space: string };
