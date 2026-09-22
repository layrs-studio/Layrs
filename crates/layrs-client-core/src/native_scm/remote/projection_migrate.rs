//! Non-destructive migration: a new partial checkout beside the complete Store.
//! The source remains the archive of all local Operations and unpublished work.
use super::*;
use layrs_store_local::projection::{ProjectionIdentity, VersionedProjection};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectionMigrationOutcome {
    pub identity: ProjectionIdentity,
    pub projection: VersionedProjection,
    pub source_checkout: std::path::PathBuf,
    pub source_operation_id: OperationId,
    pub source_retained: bool,
    pub source_changed_during_migration: bool,
}
impl NativeRemote {
    /// Select an already published local Layer head, and create its authorized
    /// projection in an empty sibling directory. Never rewrite or discard Store2.
    pub fn migrate_projection_into(
        &self,
        workspace: &NativeWorkspace,
        destination: &Path,
        layer: &LayerId,
    ) -> RemoteResult<ProjectionMigrationOutcome> {
        let source = workspace.checkout_root()?;
        let canonical_destination = std::fs::canonicalize(destination)?;
        let canonical_source = std::fs::canonicalize(&source)?;
        if canonical_destination.starts_with(&canonical_source)
            || canonical_source.starts_with(&canonical_destination)
        {
            return Err(NativeRemoteError::Invalid(
                "partial migration requires a separate checkout directory",
            ));
        }
        let before = workspace.state()?;
        if projection_dirty(workspace, &before)? {
            return Err(NativeRemoteError::Invalid(
                "capture and publish local edits before partial migration",
            ));
        }
        let binding = self.binding(workspace)?;
        if binding.pending.is_some() || binding.clone_pending {
            return Err(NativeRemoteError::PublicationPending);
        }
        let local_heads = before
            .view
            .layer_heads()
            .iter()
            .find(|l| l.layer_id() == layer)
            .ok_or(NativeRemoteError::Invalid(
                "migration Layer is absent locally",
            ))?;
        let base = self.bootstrap(before.space.id())?;
        let remote_heads = &base
            .layers
            .iter()
            .find(|l| l.record.id() == layer)
            .ok_or(NativeRemoteError::Invalid(
                "migration Layer is absent remotely",
            ))?
            .head_step_ids;
        if remote_heads.len() != 1 || local_heads.step_ids() != remote_heads {
            return Err(NativeRemoteError::Invalid(
                "migration requires one identical published local and remote head",
            ));
        }
        let store = self.clone_projection_into(destination, before.space.id(), &remote_heads[0])?;
        let after = workspace.state()?;
        Ok(ProjectionMigrationOutcome {
            identity: store.identity().clone(),
            projection: store
                .state()
                .map_err(|e| NativeRemoteError::Io(std::io::Error::other(e.to_string())))?,
            source_checkout: source,
            source_operation_id: before.operation.id().clone(),
            source_retained: true,
            source_changed_during_migration: after.operation.id() != before.operation.id()
                || projection_dirty(workspace, &after)?,
        })
    }
}
