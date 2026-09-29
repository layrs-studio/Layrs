//! Restore selected local capture paths under freshly observed current rights.
use super::*;
use layrs_store_local::projection::{
    ProjectionIdentity, ProjectionStore, RestorePreview, RestoreReceipt, RestoreSource,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeProjectionRestorePreview {
    pub schema_version: String,
    pub identity: ProjectionIdentity,
    pub local: RestorePreview,
}
fn local(e: impl std::fmt::Display) -> NativeRemoteError {
    NativeRemoteError::Io(std::io::Error::other(e.to_string()))
}
impl NativeRemote {
    pub fn preview_projection_restore(
        &self,
        store: &ProjectionStore,
        generation: u64,
        source: &RestoreSource,
    ) -> RemoteResult<NativeProjectionRestorePreview> {
        let preview = store.preview_restore(generation, source).map_err(local)?;
        self.authorize_projection_restore(store, &preview)?;
        Ok(NativeProjectionRestorePreview {
            schema_version: "layrs.projection-restore-preview.v1".into(),
            identity: store.identity().clone(),
            local: preview,
        })
    }
    pub fn apply_projection_restore(
        &self,
        store: &ProjectionStore,
        preview: &NativeProjectionRestorePreview,
    ) -> RemoteResult<RestoreReceipt> {
        if preview.schema_version != "layrs.projection-restore-preview.v1"
            || preview.identity != *store.identity()
        {
            return Err(NativeRemoteError::Invalid(
                "restore preview identity mismatch",
            ));
        }
        self.authorize_projection_restore(store, &preview.local)?;
        store.apply_restore(&preview.local).map_err(local)
    }
    fn authorize_projection_restore(
        &self,
        store: &ProjectionStore,
        preview: &RestorePreview,
    ) -> RemoteResult<()> {
        if self.origin().trim_end_matches('/') != store.identity().origin {
            return Err(NativeRemoteError::Invalid("restore origin mismatch"));
        }
        let state = store.state().map_err(local)?;
        if state.generation != preview.projection_generation
            || state.digest != preview.projection_digest
            || preview.after.is_empty()
            || preview.after.len() > 1024
            || preview.before.keys().ne(preview.after.keys())
        {
            return Err(NativeRemoteError::Invalid(
                "restore preview base or paths changed",
            ));
        }
        let paths: Vec<_> = preview.after.keys().cloned().collect();
        for batch in paths.chunks(200) {
            let access = self.projection_access(
                &store.identity().space_id,
                &ProjectionAccessRequest {
                    step_id: state.snapshot.base_step_id.clone(),
                    expected_policy_set_id: state.snapshot.observed_policy_set_id.clone(),
                    expected_authorization_epoch: state.snapshot.observed_authorization_epoch,
                    paths: batch.to_vec(),
                },
            )?;
            if access.principal_id.as_str() != store.identity().principal_id
                || access.root_tree_id != state.snapshot.base_root_tree_id
            {
                return Err(NativeRemoteError::Invalid("restore authority changed"));
            }
            for path in &access.entries {
                let entry = state
                    .snapshot
                    .entries
                    .iter()
                    .find(|e| e.path == path.path)
                    .ok_or(NativeRemoteError::Invalid("restore path is unknown"))?;
                if !path.content_readable
                    || !path.writable
                    || path.target.as_ref() != entry.target.as_ref()
                    || path.executable != entry.executable
                {
                    return Err(NativeRemoteError::Invalid(
                        "restore current path access denied",
                    ));
                }
            }
        }
        // Catch revocation between the first and last page before local mutation.
        let current = self.bootstrap(&store.identity().space_id)?;
        if current.principal_id.as_str() != store.identity().principal_id
            || current.current_policy_set_id != state.snapshot.observed_policy_set_id
            || current.authorization_epoch != state.snapshot.observed_authorization_epoch
        {
            return Err(NativeRemoteError::Invalid("restore authorization changed"));
        }
        Ok(())
    }
}
