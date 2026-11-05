//! Namespace preparations remain anchored to a complete remote Step and local generation.
use super::*;
use layrs_core::projection::EntryState;
use layrs_store_local::projection::{
    CapturedPatch, NamespaceMutation, NamespacePreview, ProjectionIdentity, ProjectionStore,
    VersionedProjection,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeProjectionNamespacePreview {
    pub schema_version: String,
    pub identity: ProjectionIdentity,
    pub authorization: ProjectionAccess,
    pub mutation: NativeProjectionNamespaceMutation,
    pub local: NamespacePreview,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum NativeProjectionNamespaceMutation {
    Create { path: RepoPath },
    Rename { source: RepoPath, path: RepoPath },
    Move { source: RepoPath, path: RepoPath },
}
impl NativeProjectionNamespaceMutation {
    fn local(&self) -> NamespaceMutation {
        match self {
            Self::Create { path } => NamespaceMutation::Create {
                destination: path.clone(),
            },
            Self::Rename { source, path } | Self::Move { source, path } => {
                NamespaceMutation::Move {
                    source: source.clone(),
                    destination: path.clone(),
                }
            }
        }
    }
    fn destination(&self) -> &RepoPath {
        match self {
            Self::Create { path } | Self::Rename { path, .. } | Self::Move { path, .. } => path,
        }
    }
    fn source(&self) -> Option<&RepoPath> {
        match self {
            Self::Create { .. } => None,
            Self::Rename { source, .. } | Self::Move { source, .. } => Some(source),
        }
    }
    fn paths(&self) -> RemoteResult<Vec<RepoPath>> {
        if self.source() == Some(self.destination()) {
            return Err(invalid("source and destination overlap"));
        }
        let mut paths = vec![self.destination().clone()];
        if let Some(source) = self.source() {
            paths.push(source.clone());
        }
        paths.sort();
        Ok(paths)
    }
}
fn invalid(message: &'static str) -> NativeRemoteError {
    NativeRemoteError::Invalid(message)
}
fn local(error: impl std::fmt::Display) -> NativeRemoteError {
    NativeRemoteError::Io(std::io::Error::other(error.to_string()))
}
fn check_local(
    state: &VersionedProjection,
    mutation: &NativeProjectionNamespaceMutation,
) -> RemoteResult<()> {
    let fences = state.snapshot.reserved_paths().map_err(local)?;
    fences
        .check_path(mutation.destination().as_str())
        .map_err(local)?;
    if let Some(source) = mutation.source() {
        fences
            .check_move(source.as_str(), mutation.destination().as_str())
            .map_err(local)?;
        let entry = state
            .snapshot
            .entries
            .iter()
            .find(|entry| &entry.path == source)
            .ok_or_else(|| invalid("namespace source is not projected"))?;
        if !entry.writable
            || !((entry.state == EntryState::Materialized
                && matches!(entry.target, Some(TreeEntryTarget::File { .. })))
                || (entry.state == EntryState::Absent && entry.target.is_none()))
        {
            return Err(invalid(
                "namespace source is not a writable materialized file",
            ));
        }
    }
    if state.snapshot.entries.iter().any(|entry| {
        &entry.path == mutation.destination()
            && (entry.state != EntryState::Absent || !entry.writable)
    }) {
        return Err(invalid(
            "namespace destination is not an explicitly writable absence",
        ));
    }
    Ok(())
}
fn check_access(
    identity: &ProjectionIdentity,
    state: &VersionedProjection,
    mutation: &NativeProjectionNamespaceMutation,
    access: &ProjectionAccess,
) -> RemoteResult<()> {
    if access.principal_id.as_str() != identity.principal_id
        || access.space_id != identity.space_id
        || access.step_id != state.snapshot.base_step_id
        || access.root_tree_id != state.snapshot.base_root_tree_id
        || access.policy_set_id != state.snapshot.observed_policy_set_id
        || access.authorization_epoch != state.snapshot.observed_authorization_epoch
        || access
            .entries
            .iter()
            .map(|e| e.path.clone())
            .collect::<Vec<_>>()
            != mutation.paths()?
    {
        return Err(invalid("namespace authorization scope changed"));
    }
    for entry in &access.entries {
        if !entry.content_readable || !entry.writable {
            return Err(invalid("namespace path access denied"));
        }
        if &entry.path == mutation.destination() {
            if entry.target.is_some() || entry.executable {
                return Err(invalid("namespace destination is present"));
            }
        } else {
            let source = state
                .snapshot
                .entries
                .iter()
                .find(|e| e.path == entry.path)
                .ok_or_else(|| invalid("namespace source disappeared"))?;
            if entry.target.as_ref() != source.target.as_ref()
                || entry.executable != source.executable
            {
                return Err(invalid("namespace source target changed"));
            }
        }
    }
    Ok(())
}
impl NativeRemote {
    pub fn preview_projection_namespace(
        &self,
        store: &ProjectionStore,
        expected_generation: u64,
        mutation: &NativeProjectionNamespaceMutation,
    ) -> RemoteResult<NativeProjectionNamespacePreview> {
        let before = store.state().map_err(local)?;
        let authorization =
            self.projection_namespace_access(store, expected_generation, mutation)?;
        // The exclusive local write rechecks the generation and digest captured before HTTP.
        let observed = store
            .observe_absent_paths(
                expected_generation,
                &before.digest,
                &[mutation.destination().clone()],
            )
            .map_err(local)?;
        let preview = store
            .preview_namespace(observed.generation, &mutation.local())
            .map_err(local)?;
        Ok(NativeProjectionNamespacePreview {
            schema_version: "layrs.projection-namespace-preview.v1".into(),
            identity: store.identity().clone(),
            authorization,
            mutation: mutation.clone(),
            local: preview,
        })
    }

    /// A preview conveys no enduring authority: reclassify paths before entering the local journal.
    pub fn apply_projection_namespace(
        &self,
        store: &ProjectionStore,
        preview: &NativeProjectionNamespacePreview,
    ) -> RemoteResult<CapturedPatch> {
        if preview.schema_version != "layrs.projection-namespace-preview.v1"
            || &preview.identity != store.identity()
            || preview.local.mutation != preview.mutation.local()
        {
            return Err(invalid("namespace preview identity or mutation mismatch"));
        }
        let state = store.state().map_err(local)?;
        if state.generation != preview.local.projection_generation
            || state.digest != preview.local.projection_digest
        {
            return Err(invalid("namespace projection generation changed"));
        }
        check_access(
            store.identity(),
            &state,
            &preview.mutation,
            &preview.authorization,
        )?;
        let current =
            self.projection_namespace_access(store, state.generation, &preview.mutation)?;
        if current != preview.authorization {
            return Err(invalid("namespace authority changed after preview"));
        }
        let contents = matches!(
            preview.mutation,
            NativeProjectionNamespaceMutation::Create { .. }
        )
        .then_some(&[][..]);
        store
            .apply_namespace(&preview.local, contents)
            .map_err(local)
    }

    /// Read-only classification before observing a new destination in the local registry.
    pub fn projection_namespace_access(
        &self,
        store: &ProjectionStore,
        expected_generation: u64,
        mutation: &NativeProjectionNamespaceMutation,
    ) -> RemoteResult<ProjectionAccess> {
        if self.origin().trim_end_matches('/') != store.identity().origin.trim_end_matches('/') {
            return Err(invalid("namespace origin mismatch"));
        }
        let state = store.state().map_err(local)?;
        if state.generation != expected_generation {
            return Err(invalid("namespace projection generation changed"));
        }
        check_local(&state, mutation)?;
        let request = ProjectionAccessRequest {
            step_id: state.snapshot.base_step_id.clone(),
            expected_policy_set_id: state.snapshot.observed_policy_set_id.clone(),
            expected_authorization_epoch: state.snapshot.observed_authorization_epoch,
            paths: mutation.paths()?,
        };
        let access = self.projection_access(&store.identity().space_id, &request)?;
        check_access(store.identity(), &state, mutation, &access)?;
        let current = self.bootstrap(&store.identity().space_id)?;
        if current.principal_id.as_str() != store.identity().principal_id
            || current.current_policy_set_id != access.policy_set_id
            || current.authorization_epoch != access.authorization_epoch
        {
            return Err(invalid(
                "namespace authorization changed during preparation",
            ));
        }
        let after = store.state().map_err(local)?;
        if after.generation != state.generation || after.digest != state.digest {
            return Err(invalid("namespace projection changed during preparation"));
        }
        Ok(access)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> (
        ProjectionIdentity,
        VersionedProjection,
        NativeProjectionNamespaceMutation,
        ProjectionAccess,
    ) {
        let hash = |n: u8| format!("blake3:{n:064x}");
        let identity:ProjectionIdentity=serde_json::from_value(json!({"storeId":"store:test","worktreeId":"worktree:test","spaceId":"space:test","origin":"https://example.test","principalId":"principal:alice"})).unwrap();
        let state:VersionedProjection=serde_json::from_value(json!({"generation":4,"digest":hash(4),"snapshot":{"schemaVersion":"layrs.projection-snapshot.v1","baseStepId":hash(1),"baseRootTreeId":hash(2),"observedPolicySetId":hash(3),"observedAuthorizationEpoch":7,"pathSemantics":"portable_windows","entries":[{"path":"private.txt","target":{"kind":"file","fileId":hash(5)},"executable":false,"writable":false,"state":"reserved"},{"path":"public/source.txt","target":{"kind":"file","fileId":hash(6)},"executable":false,"writable":true,"state":"materialized"}]}})).unwrap();
        let mutation = NativeProjectionNamespaceMutation::Move {
            source: RepoPath::parse("public/source.txt").unwrap(),
            path: RepoPath::parse("public/target.txt").unwrap(),
        };
        let access:ProjectionAccess=serde_json::from_value(json!({"schemaVersion":"layrs.projection-access.v1","principalId":"principal:alice","spaceId":"space:test","stepId":hash(1),"rootTreeId":hash(2),"policySetId":hash(3),"authorizationEpoch":7,"entries":[{"path":"public/source.txt","target":{"kind":"file","fileId":hash(6)},"executable":false,"contentReadable":true,"writable":true},{"path":"public/target.txt","target":null,"executable":false,"contentReadable":true,"writable":true}]})).unwrap();
        (identity, state, mutation, access)
    }
    #[test]
    fn authenticated_absence_and_exact_source_allow_preparation() {
        let (i, s, m, a) = fixture();
        check_local(&s, &m).unwrap();
        check_access(&i, &s, &m, &a).unwrap();
    }
    #[test]
    fn unpublished_source_requires_exact_absence_and_local_file_verification() {
        let (i, mut s, m, mut a) = fixture();
        s.snapshot.entries[1].state = EntryState::Absent;
        s.snapshot.entries[1].target = None;
        a.entries[0].target = Nullable::null();
        check_local(&s, &m).unwrap();
        check_access(&i, &s, &m, &a).unwrap();
        // The filesystem engine still requires a physically present captured
        // source; an absent remote observation alone never moves any file.
        a.entries[0].target = Nullable::some(TreeEntryTarget::File {
            file_id: FileId::from_bytes(b"remote"),
        });
        assert!(check_access(&i, &s, &m, &a).is_err());
    }
    #[test]
    fn reserved_alias_ancestor_descendant_and_readonly_source_fail() {
        let (_, mut s, _, _) = fixture();
        for path in [
            "private.txt",
            "PRIVATE.TXT",
            "private.txt/child",
            ".git/config",
            "CON .txt",
        ] {
            let m = NativeProjectionNamespaceMutation::Create {
                path: RepoPath::parse(path).unwrap(),
            };
            assert!(check_local(&s, &m).is_err(), "{path}");
        }
        let (_, _, m, _) = fixture();
        s.snapshot.entries[1].writable = false;
        assert!(check_local(&s, &m).is_err());
    }
    #[test]
    fn denied_or_present_destination_and_source_identity_changes_fail() {
        let (i, s, m, a) = fixture();
        let mut b = a.clone();
        b.entries[1].writable = false;
        assert!(check_access(&i, &s, &m, &b).is_err());
        let mut b = a.clone();
        b.entries[1].target = b.entries[0].target.clone();
        assert!(check_access(&i, &s, &m, &b).is_err());
        let mut b = a.clone();
        b.entries[0].executable = true;
        assert!(check_access(&i, &s, &m, &b).is_err());
        let mut b = a;
        b.entries[0].target = Nullable::null();
        assert!(check_access(&i, &s, &m, &b).is_err());
    }
    #[test]
    fn another_root_actor_or_epoch_cannot_be_reused() {
        let (i, s, m, a) = fixture();
        let mut b = a.clone();
        b.root_tree_id = TreeId::parse(format!("blake3:{:064x}", 9)).unwrap();
        assert!(check_access(&i, &s, &m, &b).is_err());
        let mut b = a.clone();
        b.principal_id = PrincipalId::parse("principal:bob").unwrap();
        assert!(check_access(&i, &s, &m, &b).is_err());
        let mut b = a;
        b.authorization_epoch += 1;
        assert!(check_access(&i, &s, &m, &b).is_err());
    }
}
