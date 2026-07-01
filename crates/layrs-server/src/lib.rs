//! Server runtime and route contract for Layrs.
//!
//! The workspace currently has no HTTP framework dependency in `Cargo.lock`.
//! This crate keeps a clean runtime/auth boundary so Axum, Tokio and SQLx can be
//! wired later without changing the public route contract.

use layrs_api::{
    CommitChunkRequest, CreateLayerRequest, CreatePolicyRequest, CreateSpaceRequest,
    CreateTeamRequest, CreateWeaveRequest, CreateWorkspaceRequest, EvaluatePolicyRequest,
    LayerResponse, PolicyDecisionResponse, PolicyResponse, ReserveChunkRequest, SpaceResponse,
    SubmitProofRequest, SyncPublishResponse, SyncReceiveResponse, TeamResponse,
    TimelineEventResponse, TimelineRequest, Validate, WeaveResponse, WorkspaceResponse,
};
use layrs_sync::{PublishRequest, ReceiveRequest};
use std::fmt;

pub mod auth;
pub mod lenses;
pub mod runtime;
pub mod web;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum AuthRequirement {
    Public,
    Session,
    Device,
    Principal,
    WorkspaceRead,
    WorkspaceWrite,
    WorkspaceAdmin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum HandlerKind {
    Health,
    Routes,
    Signup,
    Login,
    Logout,
    Me,
    AuthSession,
    StudioSnapshot,
    DesktopDevicePage,
    DesktopDeviceStart,
    DesktopDevicePoll,
    DesktopDeviceApprove,
    DesktopBootstrap,
    ListLenses,
    ListDevices,
    ListAuditEvents,
    LocalSpaceBootstrap,
    GetLayerAccessPolicy,
    PutLayerAccessPolicy,
    CreateLayerAccessRule,
    UpdateLayerAccessRule,
    DeleteLayerAccessRule,
    ListWorkspaces,
    CreateWorkspace,
    ListTeams,
    CreateTeam,
    GetTeam,
    ListTeamMembers,
    AddTeamMember,
    RemoveTeamMember,
    ListWorkspaceMembers,
    ListWorkspaceInvitations,
    CreateInvitation,
    ListMyInvitations,
    AcceptInvitation,
    DeclineInvitation,
    CreateSpace,
    CreateSpaceFromLocal,
    CreateLayer,
    DeleteLayer,
    ReserveChunk,
    CommitChunk,
    PrepareSpaceChunks,
    PutSpaceChunk,
    GetSpaceChunk,
    PublishSync,
    ReceiveSync,
    PublishLocalSpaceSync,
    ReceiveLocalSpaceSync,
    CreateWeave,
    SubmitProof,
    CreatePolicy,
    EvaluatePolicy,
    Timeline,
    LayerTimeline,
    LayerSteps,
    LayerStep,
    LayerStepDiff,
    LayerArtifacts,
    LayerArtifactContent,
    LayerArtifactDiff,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RouteDescriptor {
    pub method: HttpMethod,
    pub path: &'static str,
    pub name: &'static str,
    pub handler: HandlerKind,
    pub auth: AuthRequirement,
}

pub const ROUTES: &[RouteDescriptor] = &[
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/healthz",
        name: "runtime.healthz",
        handler: HandlerKind::Health,
        auth: AuthRequirement::Public,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/routes",
        name: "runtime.routes",
        handler: HandlerKind::Routes,
        auth: AuthRequirement::Public,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/auth/signup",
        name: "auth.signup",
        handler: HandlerKind::Signup,
        auth: AuthRequirement::Public,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/auth/login",
        name: "auth.login",
        handler: HandlerKind::Login,
        auth: AuthRequirement::Public,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/auth/logout",
        name: "auth.logout",
        handler: HandlerKind::Logout,
        auth: AuthRequirement::Session,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/me",
        name: "auth.me",
        handler: HandlerKind::Me,
        auth: AuthRequirement::Session,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/auth/session",
        name: "auth.session",
        handler: HandlerKind::AuthSession,
        auth: AuthRequirement::Session,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/studio/snapshot",
        name: "studio.snapshot",
        handler: HandlerKind::StudioSnapshot,
        auth: AuthRequirement::Session,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/desktop/device",
        name: "desktop.device.verify",
        handler: HandlerKind::DesktopDevicePage,
        auth: AuthRequirement::Public,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/desktop/device/start",
        name: "desktop.device.start",
        handler: HandlerKind::DesktopDeviceStart,
        auth: AuthRequirement::Public,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/desktop/device/poll",
        name: "desktop.device.poll",
        handler: HandlerKind::DesktopDevicePoll,
        auth: AuthRequirement::Public,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/desktop/device/approve",
        name: "desktop.device.approve",
        handler: HandlerKind::DesktopDeviceApprove,
        auth: AuthRequirement::Public,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/desktop/bootstrap",
        name: "desktop.bootstrap",
        handler: HandlerKind::DesktopBootstrap,
        auth: AuthRequirement::Device,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/lenses",
        name: "lenses.list",
        handler: HandlerKind::ListLenses,
        auth: AuthRequirement::Public,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/devices",
        name: "devices.list",
        handler: HandlerKind::ListDevices,
        auth: AuthRequirement::Session,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/workspaces",
        name: "workspaces.list",
        handler: HandlerKind::ListWorkspaces,
        auth: AuthRequirement::Principal,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/workspaces/{workspace_id}/audit-events",
        name: "audit_events.list",
        handler: HandlerKind::ListAuditEvents,
        auth: AuthRequirement::WorkspaceRead,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/local-space-bootstrap",
        name: "local_space.bootstrap",
        handler: HandlerKind::LocalSpaceBootstrap,
        auth: AuthRequirement::Principal,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/access",
        name: "layer_access.get",
        handler: HandlerKind::GetLayerAccessPolicy,
        auth: AuthRequirement::WorkspaceRead,
    },
    RouteDescriptor {
        method: HttpMethod::Put,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/access",
        name: "layer_access.put",
        handler: HandlerKind::PutLayerAccessPolicy,
        auth: AuthRequirement::WorkspaceAdmin,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/access/rules",
        name: "layer_access_rules.create",
        handler: HandlerKind::CreateLayerAccessRule,
        auth: AuthRequirement::WorkspaceAdmin,
    },
    RouteDescriptor {
        method: HttpMethod::Patch,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/access/rules/{rule_id}",
        name: "layer_access_rules.update",
        handler: HandlerKind::UpdateLayerAccessRule,
        auth: AuthRequirement::WorkspaceAdmin,
    },
    RouteDescriptor {
        method: HttpMethod::Delete,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/access/rules/{rule_id}",
        name: "layer_access_rules.delete",
        handler: HandlerKind::DeleteLayerAccessRule,
        auth: AuthRequirement::WorkspaceAdmin,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/workspaces",
        name: "workspaces.create",
        handler: HandlerKind::CreateWorkspace,
        auth: AuthRequirement::Principal,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/workspaces/{workspace_id}/teams",
        name: "teams.list",
        handler: HandlerKind::ListTeams,
        auth: AuthRequirement::WorkspaceRead,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/workspaces/{workspace_id}/teams",
        name: "teams.create",
        handler: HandlerKind::CreateTeam,
        auth: AuthRequirement::WorkspaceAdmin,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/workspaces/{workspace_id}/teams/{team_id}",
        name: "teams.get",
        handler: HandlerKind::GetTeam,
        auth: AuthRequirement::WorkspaceRead,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/workspaces/{workspace_id}/teams/{team_id}/members",
        name: "team_members.list",
        handler: HandlerKind::ListTeamMembers,
        auth: AuthRequirement::WorkspaceRead,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/workspaces/{workspace_id}/teams/{team_id}/members",
        name: "team_members.add",
        handler: HandlerKind::AddTeamMember,
        auth: AuthRequirement::WorkspaceAdmin,
    },
    RouteDescriptor {
        method: HttpMethod::Delete,
        path: "/v1/workspaces/{workspace_id}/teams/{team_id}/members/{account_id}",
        name: "team_members.remove",
        handler: HandlerKind::RemoveTeamMember,
        auth: AuthRequirement::WorkspaceAdmin,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/workspaces/{workspace_id}/members",
        name: "workspace_members.list",
        handler: HandlerKind::ListWorkspaceMembers,
        auth: AuthRequirement::WorkspaceRead,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/workspaces/{workspace_id}/invitations",
        name: "workspace_invitations.list",
        handler: HandlerKind::ListWorkspaceInvitations,
        auth: AuthRequirement::WorkspaceAdmin,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/workspaces/{workspace_id}/invitations",
        name: "workspace_invitations.create",
        handler: HandlerKind::CreateInvitation,
        auth: AuthRequirement::WorkspaceAdmin,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/me/invitations",
        name: "me_invitations.list",
        handler: HandlerKind::ListMyInvitations,
        auth: AuthRequirement::Session,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/invitations/{invitation_id}/accept",
        name: "invitations.accept",
        handler: HandlerKind::AcceptInvitation,
        auth: AuthRequirement::Session,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/invitations/{invitation_id}/decline",
        name: "invitations.decline",
        handler: HandlerKind::DeclineInvitation,
        auth: AuthRequirement::Session,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/workspaces/{workspace_id}/spaces",
        name: "spaces.create",
        handler: HandlerKind::CreateSpace,
        auth: AuthRequirement::WorkspaceWrite,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/workspaces/{workspace_id}/spaces/from-local",
        name: "spaces.create_from_local",
        handler: HandlerKind::CreateSpaceFromLocal,
        auth: AuthRequirement::WorkspaceWrite,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers",
        name: "layers.create",
        handler: HandlerKind::CreateLayer,
        auth: AuthRequirement::WorkspaceWrite,
    },
    RouteDescriptor {
        method: HttpMethod::Delete,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}",
        name: "layers.delete",
        handler: HandlerKind::DeleteLayer,
        auth: AuthRequirement::WorkspaceWrite,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/workspaces/{workspace_id}/chunks/reserve",
        name: "chunks.reserve",
        handler: HandlerKind::ReserveChunk,
        auth: AuthRequirement::WorkspaceWrite,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/workspaces/{workspace_id}/chunks/commit",
        name: "chunks.commit",
        handler: HandlerKind::CommitChunk,
        auth: AuthRequirement::WorkspaceWrite,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/workspaces/{workspace_id}/sync/publish",
        name: "sync.publish",
        handler: HandlerKind::PublishSync,
        auth: AuthRequirement::WorkspaceWrite,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/workspaces/{workspace_id}/sync/receive",
        name: "sync.receive",
        handler: HandlerKind::ReceiveSync,
        auth: AuthRequirement::WorkspaceRead,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/sync/publish",
        name: "local_space_sync.publish",
        handler: HandlerKind::PublishLocalSpaceSync,
        auth: AuthRequirement::WorkspaceWrite,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/chunks/prepare",
        name: "space_chunks.prepare",
        handler: HandlerKind::PrepareSpaceChunks,
        auth: AuthRequirement::WorkspaceWrite,
    },
    RouteDescriptor {
        method: HttpMethod::Put,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/chunks/{chunk_id}",
        name: "space_chunks.put",
        handler: HandlerKind::PutSpaceChunk,
        auth: AuthRequirement::WorkspaceWrite,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/chunks/{chunk_id}",
        name: "space_chunks.get",
        handler: HandlerKind::GetSpaceChunk,
        auth: AuthRequirement::WorkspaceRead,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/sync/receive",
        name: "local_space_sync.receive",
        handler: HandlerKind::ReceiveLocalSpaceSync,
        auth: AuthRequirement::WorkspaceRead,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/workspaces/{workspace_id}/weaves",
        name: "weaves.create",
        handler: HandlerKind::CreateWeave,
        auth: AuthRequirement::WorkspaceWrite,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/workspaces/{workspace_id}/proofs",
        name: "proofs.submit",
        handler: HandlerKind::SubmitProof,
        auth: AuthRequirement::WorkspaceWrite,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/workspaces/{workspace_id}/policies",
        name: "policies.create",
        handler: HandlerKind::CreatePolicy,
        auth: AuthRequirement::WorkspaceAdmin,
    },
    RouteDescriptor {
        method: HttpMethod::Post,
        path: "/v1/workspaces/{workspace_id}/policies/evaluate",
        name: "policies.evaluate",
        handler: HandlerKind::EvaluatePolicy,
        auth: AuthRequirement::WorkspaceRead,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/workspaces/{workspace_id}/timeline",
        name: "timeline.list",
        handler: HandlerKind::Timeline,
        auth: AuthRequirement::WorkspaceRead,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/timeline",
        name: "layer_timeline.list",
        handler: HandlerKind::LayerTimeline,
        auth: AuthRequirement::WorkspaceRead,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/steps",
        name: "layer_steps.list",
        handler: HandlerKind::LayerSteps,
        auth: AuthRequirement::WorkspaceRead,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/steps/{step_id}",
        name: "layer_step.get",
        handler: HandlerKind::LayerStep,
        auth: AuthRequirement::WorkspaceRead,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/steps/{step_id}/diff",
        name: "layer_step_diff.get",
        handler: HandlerKind::LayerStepDiff,
        auth: AuthRequirement::WorkspaceRead,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/artifacts",
        name: "layer_artifacts.list",
        handler: HandlerKind::LayerArtifacts,
        auth: AuthRequirement::WorkspaceRead,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/artifacts/{artifact_id}/content",
        name: "layer_artifact_content.get",
        handler: HandlerKind::LayerArtifactContent,
        auth: AuthRequirement::WorkspaceRead,
    },
    RouteDescriptor {
        method: HttpMethod::Get,
        path: "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/artifacts/{artifact_id}/diff",
        name: "layer_artifact_diff.get",
        handler: HandlerKind::LayerArtifactDiff,
        auth: AuthRequirement::WorkspaceRead,
    },
];

pub fn find_route(method: HttpMethod, path: &str) -> Option<&'static RouteDescriptor> {
    ROUTES
        .iter()
        .find(|route| route.method == method && route.path == path)
}

pub type HandlerResult<T> = Result<T, HandlerError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandlerError {
    pub code: HandlerErrorCode,
    pub message: String,
}

impl HandlerError {
    pub fn not_implemented(handler: &'static str) -> Self {
        Self {
            code: HandlerErrorCode::NotImplemented,
            message: format!("{handler} is registered but not implemented yet"),
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self {
            code: HandlerErrorCode::Validation,
            message: message.into(),
        }
    }
}

impl fmt::Display for HandlerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for HandlerError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum HandlerErrorCode {
    Validation,
    PermissionDenied,
    NotImplemented,
    StoreUnavailable,
}

impl From<layrs_api::ValidationError> for HandlerError {
    fn from(value: layrs_api::ValidationError) -> Self {
        Self::validation(value.to_string())
    }
}

impl From<layrs_sync::SyncValidationError> for HandlerError {
    fn from(value: layrs_sync::SyncValidationError) -> Self {
        Self::validation(value.to_string())
    }
}

pub mod handlers {
    use super::*;

    pub fn create_workspace(request: CreateWorkspaceRequest) -> HandlerResult<WorkspaceResponse> {
        request.validate()?;
        Err(HandlerError::not_implemented("create_workspace"))
    }

    pub fn create_team(request: CreateTeamRequest) -> HandlerResult<TeamResponse> {
        request.validate()?;
        Err(HandlerError::not_implemented("create_team"))
    }

    pub fn create_space(request: CreateSpaceRequest) -> HandlerResult<SpaceResponse> {
        request.validate()?;
        Err(HandlerError::not_implemented("create_space"))
    }

    pub fn create_layer(request: CreateLayerRequest) -> HandlerResult<LayerResponse> {
        request.validate()?;
        Err(HandlerError::not_implemented("create_layer"))
    }

    pub fn reserve_chunk(request: ReserveChunkRequest) -> HandlerResult<layrs_api::ChunkResponse> {
        request.validate()?;
        Err(HandlerError::not_implemented("reserve_chunk"))
    }

    pub fn commit_chunk(request: CommitChunkRequest) -> HandlerResult<layrs_api::ChunkResponse> {
        request.validate()?;
        Err(HandlerError::not_implemented("commit_chunk"))
    }

    pub fn publish_sync(request: PublishRequest) -> HandlerResult<SyncPublishResponse> {
        request.validate()?;
        Err(HandlerError::not_implemented("publish_sync"))
    }

    pub fn receive_sync(request: ReceiveRequest) -> HandlerResult<SyncReceiveResponse> {
        request.validate()?;
        Err(HandlerError::not_implemented("receive_sync"))
    }

    pub fn create_weave(request: CreateWeaveRequest) -> HandlerResult<WeaveResponse> {
        request.validate()?;
        Err(HandlerError::not_implemented("create_weave"))
    }

    pub fn submit_proof(request: SubmitProofRequest) -> HandlerResult<layrs_api::ProofResponse> {
        request.validate()?;
        Err(HandlerError::not_implemented("submit_proof"))
    }

    pub fn create_policy(request: CreatePolicyRequest) -> HandlerResult<PolicyResponse> {
        request.validate()?;
        Err(HandlerError::not_implemented("create_policy"))
    }

    pub fn evaluate_policy(
        request: EvaluatePolicyRequest,
    ) -> HandlerResult<PolicyDecisionResponse> {
        request.validate()?;
        Err(HandlerError::not_implemented("evaluate_policy"))
    }

    pub fn timeline(request: TimelineRequest) -> HandlerResult<Vec<TimelineEventResponse>> {
        request.validate()?;
        Err(HandlerError::not_implemented("timeline"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layrs_sync::{IdempotencyKey, SyncManifest};
    use std::collections::BTreeSet;

    #[test]
    fn route_registry_has_unique_method_path_pairs() {
        let mut seen = BTreeSet::new();

        for route in ROUTES {
            assert!(
                !route.name.is_empty(),
                "route name should be explicit for {:?}",
                route.handler
            );
            assert!(
                seen.insert(format!("{:?}:{}", route.method, route.path)),
                "duplicate route {:?} {}",
                route.method,
                route.path
            );
        }
    }

    #[test]
    fn sync_publish_and_receive_routes_are_registered() {
        assert_eq!(
            find_route(
                HttpMethod::Post,
                "/v1/workspaces/{workspace_id}/sync/publish"
            )
            .map(|route| route.handler),
            Some(HandlerKind::PublishSync)
        );
        assert_eq!(
            find_route(
                HttpMethod::Post,
                "/v1/workspaces/{workspace_id}/sync/receive"
            )
            .map(|route| route.handler),
            Some(HandlerKind::ReceiveSync)
        );
    }

    #[test]
    fn local_space_routes_are_registered() {
        let expected = [
            (HttpMethod::Get, "/v1/lenses", HandlerKind::ListLenses),
            (
                HttpMethod::Get,
                "/v1/workspaces/{workspace_id}/spaces/{space_id}/local-space-bootstrap",
                HandlerKind::LocalSpaceBootstrap,
            ),
            (
                HttpMethod::Post,
                "/v1/workspaces/{workspace_id}/spaces/{space_id}/sync/publish",
                HandlerKind::PublishLocalSpaceSync,
            ),
            (
                HttpMethod::Post,
                "/v1/workspaces/{workspace_id}/spaces/{space_id}/chunks/prepare",
                HandlerKind::PrepareSpaceChunks,
            ),
            (
                HttpMethod::Put,
                "/v1/workspaces/{workspace_id}/spaces/{space_id}/chunks/{chunk_id}",
                HandlerKind::PutSpaceChunk,
            ),
            (
                HttpMethod::Get,
                "/v1/workspaces/{workspace_id}/spaces/{space_id}/chunks/{chunk_id}",
                HandlerKind::GetSpaceChunk,
            ),
            (
                HttpMethod::Post,
                "/v1/workspaces/{workspace_id}/spaces/{space_id}/sync/receive",
                HandlerKind::ReceiveLocalSpaceSync,
            ),
            (
                HttpMethod::Get,
                "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/timeline",
                HandlerKind::LayerTimeline,
            ),
            (
                HttpMethod::Get,
                "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/steps",
                HandlerKind::LayerSteps,
            ),
            (
                HttpMethod::Get,
                "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/steps/{step_id}",
                HandlerKind::LayerStep,
            ),
            (
                HttpMethod::Get,
                "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/steps/{step_id}/diff",
                HandlerKind::LayerStepDiff,
            ),
            (
                HttpMethod::Get,
                "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/artifacts",
                HandlerKind::LayerArtifacts,
            ),
            (
                HttpMethod::Get,
                "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/artifacts/{artifact_id}/content",
                HandlerKind::LayerArtifactContent,
            ),
            (
                HttpMethod::Get,
                "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/artifacts/{artifact_id}/diff",
                HandlerKind::LayerArtifactDiff,
            ),
        ];

        for (method, path, handler) in expected {
            assert_eq!(
                find_route(method, path).map(|route| route.handler),
                Some(handler),
                "{method:?} {path} should be registered"
            );
        }
    }

    #[test]
    fn studio_v1_server_data_routes_are_registered() {
        let expected = [
            (
                HttpMethod::Get,
                "/v1/workspaces/{workspace_id}/teams",
                HandlerKind::ListTeams,
            ),
            (
                HttpMethod::Get,
                "/v1/workspaces/{workspace_id}/teams/{team_id}/members",
                HandlerKind::ListTeamMembers,
            ),
            (
                HttpMethod::Post,
                "/v1/workspaces/{workspace_id}/invitations",
                HandlerKind::CreateInvitation,
            ),
            (
                HttpMethod::Patch,
                "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/access/rules/{rule_id}",
                HandlerKind::UpdateLayerAccessRule,
            ),
            (
                HttpMethod::Delete,
                "/v1/workspaces/{workspace_id}/spaces/{space_id}/layers/{layer_id}/access/rules/{rule_id}",
                HandlerKind::DeleteLayerAccessRule,
            ),
        ];

        for (method, path, handler) in expected {
            assert_eq!(
                find_route(method, path).map(|route| route.handler),
                Some(handler),
                "{method:?} {path} should be registered"
            );
        }
    }

    #[test]
    fn legacy_access_registry_route_is_not_registered() {
        assert!(
            find_route(
                HttpMethod::Put,
                "/v1/workspaces/{workspace_id}/layers/{layer_id}/access-registry"
            )
            .is_none()
        );
    }

    #[test]
    fn publish_handler_rejects_invalid_request_before_stub() {
        let request = PublishRequest {
            idempotency_key: IdempotencyKey::unchecked("too-short"),
            manifest: SyncManifest {
                manifest_id: String::new(),
                workspace_id: String::new(),
                space_id: None,
                source_client_id: String::new(),
                base_cursor: None,
                capability_epoch: 0,
                generated_at: String::new(),
                chunks: vec![],
                operations: vec![],
            },
            expected_server_cursor: None,
            dry_run: false,
        };

        let error = handlers::publish_sync(request).expect_err("validation should fail");
        assert_eq!(error.code, HandlerErrorCode::Validation);
    }
}
