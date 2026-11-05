// Team, access, settings, inbox and instance administration (lot L5).
// Transport-agnostic: Web passes HTTP transports, Desktop can pass Tauri ones.
export { AccessPanel } from "./AccessPanel";
export { AccountsPanel, ServiceAccountsPanel } from "./AccountsPanels";
export { AdvancedPanel } from "./AdvancedPanel";
export { AutomationsPanel } from "./AutomationsPanel";
export { ConfirmDialog } from "./ConfirmDialog";
export type { Confirmation } from "./ConfirmDialog";
export { InvitationInbox, ProjectActivity } from "./InboxPanels";
export { InvitationsPanel } from "./InvitationsPanel";
export { ServerStatusPanel, StoragePanel } from "./InstancePanels";
export { MembersPanel } from "./MembersPanel";
export { PersonPicker } from "./PersonPicker";
export { PolicyChangeDialog } from "./PolicyChangeDialog";
export type { PreparedPolicyChange } from "./PolicyChangeDialog";
export { ProtectionsPanel } from "./ProtectionsPanel";
export { SecretReveal } from "./SecretReveal";
export type { RevealedSecret } from "./SecretReveal";
export { TeamsPanel } from "./TeamsPanel";
export { TokensPanel } from "./TokensPanel";
export { WebhooksPanel } from "./WebhooksPanel";
export { describeError as describeTeamError } from "./errors";
export { useCapabilities } from "./useCapabilities";
export type { Capabilities } from "./useCapabilities";
export { useDirectory } from "./useDirectory";
export type { Directory } from "./useDirectory";
export { applyPathChange, parseRules, pathAccess, roleSummary } from "./access-model";
export type { AccessLevel, PathAccess, PathChange, ReadPolicy, RoleSummary } from "./access-model";
export { normalizeTime } from "./time";
// Unsaved-changes guard of the shared router, used by the settings pages.
export { useNavigationGuard } from "../shell/router";
export { accountKinds, eventLabels, memberRoles, personName, teamRoles } from "./types";
export type {
  Account,
  Invitation,
  LayerRef,
  MemberRole,
  Notice,
  Person,
  PolicyManagement,
  PolicyPreview,
  PolicyRulesRead,
  PolicyTransport,
  Protection,
  Team,
  TeamCommand,
  TeamRole,
  TeamTransport,
  Token,
  Webhook,
} from "./types";
