//! Native SCM preparation/admission boundary shared by future CLI/Desktop flows.
//!
//! Disk capture verifies checkout identity and bounded reads before preparing
//! a mutation. Materialization requires a separate recoverable write protocol.
//! No legacy store migration or direct SQL is performed here. Prepared changes
//! carry exact expectations and remain retryable after a lost commit response.

mod admin;
mod commands;
mod credentials;
mod filesystem;
pub use admin::{
    AdministrationCommand, NativeArchiveTeamRequest, NativeTeamRequest, NativeTeamRole,
};
mod collaboration;
pub use collaboration::{
    CancelRunRequest, CollaborationCommand, CorrectCommentRequest, PublishReleaseRequest,
    ReviewAction, ReviewAnchor, ReviewDecision, ReviewProposal, RevokeReleaseRequest,
};
pub use credentials::{execute_remote, NativeRemoteAction, NativeRemoteRequest};
mod diagnostics;
mod diff;
pub use diagnostics::diagnose_checkout;
mod history;
mod workflow_merge;
pub use history::{
    OperationHistoryCursor, OperationHistoryFilter, OperationHistoryPage, StepHistoryCursor,
    StepHistoryFilter, StepHistoryPage,
};
mod local_identity;
pub use local_identity::LocalIdentity;
mod merge;
mod rewrite;
pub use rewrite::{NativeRewriteKind, NativeRewritePlan, NativeRewriteSummary};
mod references;
mod restore;
mod restore_preview;
mod restore_projection;
pub use restore_preview::{NativeRestorePreview, RestoreCollision, RestoreExpectation};
mod prepare;
mod projection;
pub mod remote;
mod remote_choose;
mod remote_products;
pub use remote_choose::NativeChoosePreview;
mod transfer_control;
pub use transfer_control::{NativeTransferCancelStatus, NativeTransferControl};
mod selection;
mod snapshot;
mod workspace;
pub use remote::{
    NativeDevicePoll, NativeDeviceStart, NativeInlinePolicyRules, NativePolicyPreview,
    NativePolicyReplacement, NativePolicyRulesEntry, NativePolicyRulesSnapshot,
    NativeProjectionDirectoryPreview, NativeProjectionNamespaceMutation,
    NativeProjectionNamespacePreview, NativeRemote, NativeRemoteError, NativeRemoteSession,
    ReceiveReport, RemoteProvisionedSpace, ScopedProjectionPublicationOutcome, ScopedReviewCommand,
};

pub use commands::{execute_workspace, WorkspaceAction};
mod partial;
pub use diff::{NativeDiffLine, NativeFileChange};
pub use filesystem::capture_checkout;
pub use layrs_graph::scm::{ReferenceSelector, StepSelector};
pub use layrs_weave::graph_merge::ConflictReplacement;
pub use merge::{
    BlockResolutionChoice, NativeAcceptedMerge, NativeAcceptedResolution, NativeMergePreview,
    NativeResolutionPreview,
};
pub use partial::{checkout_kind, execute_projection, ProjectionAction};
pub use prepare::{
    prepare_initialize, CaptureBase, CapturePlan, MutationStamp, NativeClient, PreparedMutation,
};
pub use selection::ResolvedStep;
pub use snapshot::{PreparedSnapshot, SnapshotBuilder, SnapshotLimits};
pub use workspace::{NativeWorkspace, WorkspaceState};

use std::fmt;

#[derive(Debug)]
pub enum NativeClientError {
    Filesystem(std::io::Error),
    Core(layrs_core::scm::ScmError),
    Store(layrs_store::scm::StoreError),
    Format(layrs_store_local::scm_v1::NativeStoreError),
    InvalidSnapshot(String),
    Missing(&'static str),
    NeedsIntegration,
    StaleCapture,
    NeedsRepair,
    InvalidState(&'static str),
    Selector(layrs_graph::scm::SelectorError),
}

pub type Result<T> = std::result::Result<T, NativeClientError>;

impl NativeClientError {
    pub fn code(&self) -> &str {
        match self {
            Self::Filesystem(_) => "checkout_unavailable_or_unstable",
            Self::Core(_) => "invalid_canonical_input",
            Self::Store(error) => error.code(),
            Self::Format(_) => "native_store_unavailable",
            Self::InvalidSnapshot(_) => "invalid_snapshot",
            Self::Missing(_) => "missing_native_state",
            Self::NeedsIntegration => "operation_integration_required",
            Self::StaleCapture => "stale_capture_base",
            Self::NeedsRepair => "worktree_needs_repair",
            Self::InvalidState(_) => "invalid_worktree_state",
            Self::Selector(_) => "invalid_selector",
        }
    }
}

impl fmt::Display for NativeClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Filesystem(e) => e.fmt(f),
            Self::Core(e) => e.fmt(f),
            Self::Store(e) => e.fmt(f),
            Self::Format(e) => e.fmt(f),
            Self::InvalidSnapshot(reason) => write!(f, "invalid snapshot: {reason}"),
            Self::Missing(kind) => write!(f, "missing native {kind}"),
            Self::NeedsIntegration => write!(
                f,
                "integrate concurrent Operations or Layer heads before capture"
            ),
            Self::InvalidState(reason) => write!(f, "invalid Worktree state: {reason}"),
            Self::StaleCapture => write!(
                f,
                "Worktree or repository changed during capture; read a new base and recapture"
            ),
            Self::NeedsRepair => write!(
                f,
                "Worktree has an interrupted write; run layrs recover before continuing"
            ),
            Self::Selector(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for NativeClientError {}
impl From<std::io::Error> for NativeClientError {
    fn from(e: std::io::Error) -> Self {
        Self::Filesystem(e)
    }
}
impl From<layrs_graph::scm::SelectorError> for NativeClientError {
    fn from(e: layrs_graph::scm::SelectorError) -> Self {
        Self::Selector(e)
    }
}
impl From<layrs_core::scm::ScmError> for NativeClientError {
    fn from(e: layrs_core::scm::ScmError) -> Self {
        Self::Core(e)
    }
}
impl From<layrs_store::scm::StoreError> for NativeClientError {
    fn from(e: layrs_store::scm::StoreError) -> Self {
        Self::Store(e)
    }
}
impl From<layrs_store_local::scm_v1::NativeStoreError> for NativeClientError {
    fn from(e: layrs_store_local::scm_v1::NativeStoreError) -> Self {
        Self::Format(e)
    }
}
