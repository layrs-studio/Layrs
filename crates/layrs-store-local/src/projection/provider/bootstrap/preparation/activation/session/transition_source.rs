//! Authenticated local metadata for the network phase of a Provider transition.
//! No SQL connection or operation lock escapes the read. This view does not
//! freeze mutable checkout bytes or grant remote authority or a Store writer.
use super::*;
use crate::projection::{
    ProjectionIdentity, VersionedProjection,
    capture::CapturedPatch,
    provider::{
        cancellation, records::capture::ProviderCaptureState,
        validation::PrivateCaptureMutationValidation,
    },
};
use layrs_core::{
    projection::{ProjectionSelection, ProjectionSnapshot},
    scm::{FileManifest, RepoPath},
};

/// Read-only source DATA authenticated against this live session. The borrow
/// retains its owner, native root and lifecycle lease through remote metadata
/// acquisition, but holds no SQL scope or catalog/operation gate.
///
/// A conversion driver must additionally prove its complete native image and
/// import prefix, obtain remote authority, then revalidate all three under its
/// actual publication scopes. A successful `revalidate` is only a local read.
pub struct ProviderTransitionSource<'session> {
    session: &'session ProviderActiveSession,
    state: Box<PrivateCaptureMutationValidation>,
}

impl ProviderActiveSession {
    /// Observe the complete local predecessor before starting network work.
    /// Refuses a disconnected session, pending Capture or source transition.
    /// Complete private/family readers and observed closes run under the usual
    /// scoped locks; every operation lock is released before this returns.
    pub fn transition_source(
        &self,
        cancel: &Cancellation,
    ) -> std::result::Result<ProviderTransitionSource<'_>, ProviderActiveOpenFailure> {
        Ok(ProviderTransitionSource {
            session: self,
            state: self.read_transition_source(cancel)?,
        })
    }

    fn read_transition_source(
        &self,
        cancel: &Cancellation,
    ) -> std::result::Result<Box<PrivateCaptureMutationValidation>, ProviderActiveOpenFailure> {
        let result = (|| -> std::result::Result<_, preparation_report::Failure> {
            cancellation::check(Some(cancel)).map_err(preparation_report::Failure::from)?;
            if !self.is_connected() || self.source.lifecycle.get().is_none() {
                return Err(preparation_report::Failure::from(invalid(
                    "transition source requires a connected retained session",
                )));
            }
            let held = &self.source.held;
            let _locks = held
                .family
                .locks(
                    &held.artifacts.allocation.format.identity.worktree_id,
                    false,
                )
                .map_err(preparation_report::Failure::from)?;
            let full = capture_state::read_live(held, Some(&self.source.root), Some(cancel))?;
            if full.active.mount != self.source.record
                || full.active.activation != self.source.activation
            {
                return Err(preparation_report::Failure::private(invalid(
                    "transition source active owner changed",
                )));
            }
            if full.capture.transactions.iter().any(|transaction| {
                !matches!(
                    transaction.state,
                    ProviderCaptureState::Accepted { .. } | ProviderCaptureState::Cancelled { .. }
                )
            }) {
                return Err(preparation_report::Failure::private(invalid(
                    "transition source has a pending capture",
                )));
            }
            let state = PrivateCaptureMutationValidation::from_publication(full)
                .map_err(preparation_report::Failure::private)?;
            cancellation::check(Some(cancel)).map_err(preparation_report::Failure::from)?;
            Ok(Box::new(state))
        })();
        result.map_err(|failure| {
            ProviderActiveOpenFailure::from_failure("observe local transition source", failure)
        })
    }
}

impl ProviderTransitionSource<'_> {
    /// Internal pending-transition reauthentication. Public getters remain
    /// DATA. This retains the actual session/owner and deliberately bypasses
    /// only the no-pending VIEW opener, never original-prefix equality.
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn require_transition(
        &self,
        held: &outcome::PublicationPins,
        root: &layrs_vfs::ProviderRootGuard,
        full: &crate::projection::provider::validation::PrivateActivePublicationValidation,
        staging: &crate::projection::provider::records::transition::ProviderTransitionIntentRecord,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        cancel.check()?;
        if !self.session.is_connected()
            || self.session.source.lifecycle.get().is_none()
            || !std::ptr::eq(held, &self.session.source.held)
            || root.registration().root_id() != self.session.source.root.registration().root_id()
            || root.control_directory().identity()
                != self.session.source.root.control_directory().identity()
        {
            return Err(invalid(
                "retained transition source session/root owner differs",
            ));
        }
        self.state.require_transition_predecessor(full, staging)
    }

    pub fn identity(&self) -> &ProjectionIdentity {
        &self
            .session
            .source
            .held
            .artifacts
            .allocation
            .format
            .identity
    }
    pub fn binding(&self) -> &ProviderBootstrapBinding {
        &self.session.source.held.binding
    }
    /// The activated source generation, before applying its accepted Capture.
    pub fn versioned_snapshot(&self) -> &VersionedProjection {
        &self.state.active().activated
    }
    pub fn initial_snapshot(&self) -> &ProjectionSnapshot {
        self.state.active().source.initial()
    }
    pub fn snapshot_parent_digest(&self) -> &str {
        &self.state.active().activation.source_digest
    }
    pub fn selection(&self) -> &ProjectionSelection {
        self.state.active().source.selection()
    }
    pub fn selection_generation(&self) -> u64 {
        self.state.active().source.commitment().source_generation
    }
    pub fn selection_snapshot_digest(&self) -> &str {
        &self.state.active().source.commitment().snapshot_digest
    }
    pub fn selection_digest(&self) -> &str {
        &self.state.active().source.commitment().selection_digest
    }
    pub fn catalog_id(&self) -> &str {
        &self.state.active().source.commitment().catalog_id
    }
    /// Original canonical descriptor metadata; not evidence of resident bytes
    /// or current remote permission. No clone of the complete catalog is made.
    pub fn descriptors(
        &self,
    ) -> impl ExactSizeIterator<Item = (&RepoPath, &FileManifest, bool)> + '_ {
        self.state
            .active()
            .source
            .descriptors()
            .iter()
            .map(|row| (&row.path, &row.manifest, row.executable))
    }
    pub fn captures(&self) -> &[CapturedPatch] {
        &self.state.capture().patches
    }
    pub fn capture_count(&self) -> u64 {
        self.state.capture().patches.len() as u64
    }
    pub fn capture_head(&self) -> Option<&str> {
        self.state
            .capture()
            .patches
            .last()
            .map(|patch| patch.digest.as_str())
    }
    /// Reacquire the complete local interpretation and compare the predecessor,
    /// including publication history and all staged/cancelled retained rows.
    /// Native user-file changes are outside this metadata view; the conversion
    /// driver must independently revalidate its retained complete image.
    pub fn revalidate(
        &self,
        cancel: &Cancellation,
    ) -> std::result::Result<(), ProviderActiveOpenFailure> {
        let current = self.session.read_transition_source(cancel)?;
        self.state.require_same_state(&current).map_err(|cause| {
            ProviderActiveOpenFailure::from_failure(
                "revalidate local transition predecessor",
                preparation_report::Failure::private(cause),
            )
        })
    }
}

#[cfg(test)]
mod tests;
