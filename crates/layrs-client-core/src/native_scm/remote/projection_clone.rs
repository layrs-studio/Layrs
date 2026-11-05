//! A distinct format3 checkout. A complete native Workspace is never fabricated.
use super::*;
use layrs_core::projection::{
    EntryState, PathSemantics, ProjectionEntry as LocalEntry, ProjectionSelection,
    ProjectionSnapshot,
};
use layrs_store_local::projection::{ProjectionIdentity, ProjectionStore};
#[path = "projection_selection.rs"]
mod selection;
#[cfg(test)]
#[path = "projection_selection_tests.rs"]
mod selection_tests;

fn local(error: impl std::fmt::Display) -> NativeRemoteError {
    NativeRemoteError::Io(std::io::Error::other(error.to_string()))
}

impl NativeRemote {
    /// Explicit partial checkout of one published Step into an empty directory.
    /// Names remain visible in the registry; only authorized regular files are
    /// downloaded. Unsupported values stay unavailable and reserve their paths.
    pub fn clone_projection_into(
        &self,
        root: &Path,
        space: &SpaceId,
        step: &StepId,
    ) -> RemoteResult<ProjectionStore> {
        self.clone_projection_selected_into(root, space, step, &ProjectionSelection::All)
    }
    /// Download only selected, authorized, supported leaves. Structural metadata
    /// remains complete and bounded, preserving every exact remote identity.
    pub fn clone_projection_selected_into(
        &self,
        root: &Path,
        space: &SpaceId,
        step: &StepId,
        selection: &ProjectionSelection,
    ) -> RemoteResult<ProjectionStore> {
        let semantics = if cfg!(windows) {
            PathSemantics::PortableWindows
        } else {
            PathSemantics::CaseSensitive
        };
        let selection = selection.normalized(semantics).map_err(local)?;
        let metadata = std::fs::symlink_metadata(root)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || std::fs::read_dir(root)?.next().is_some()
        {
            return Err(NativeRemoteError::Invalid(
                "partial clone requires an existing empty directory",
            ));
        }
        let base = self.bootstrap(space)?;
        let mut request = ProjectionPageRequest {
            step_id: step.clone(),
            expected_policy_set_id: base.current_policy_set_id.clone(),
            expected_authorization_epoch: base.authorization_epoch,
            cursor: None,
            limit: 200,
        };
        let mut root_tree = None;
        let mut entries = BTreeMap::new();
        let mut offset = 0u32;
        loop {
            let page = self.projection_page(space, &request)?;
            if page.principal_id != base.principal_id
                || page.offset != offset
                || root_tree
                    .as_ref()
                    .is_some_and(|root| root != &page.root_tree_id)
            {
                return Err(NativeRemoteError::Invalid(
                    "partial clone page identity changed",
                ));
            }
            root_tree = Some(page.root_tree_id);
            offset = offset
                .checked_add(page.entries.len() as u32)
                .ok_or(NativeRemoteError::Invalid("projection count overflow"))?;
            if offset > MAX_OBJECTS as u32 {
                return Err(NativeRemoteError::Invalid(
                    "partial clone exceeds path limit",
                ));
            }
            for entry in page.entries {
                if entries.insert(entry.path.clone(), entry).is_some() {
                    return Err(NativeRemoteError::Invalid("partial clone repeats a path"));
                }
            }
            let Some(cursor) = page.next_cursor else {
                break;
            };
            if request.cursor.as_ref() == Some(&cursor) {
                return Err(NativeRemoteError::Invalid(
                    "partial clone page did not advance",
                ));
            }
            request.cursor = Some(cursor);
        }
        let semantics = if cfg!(windows) {
            PathSemantics::PortableWindows
        } else {
            PathSemantics::CaseSensitive
        };
        // Leaf entries are enough: reserving a directory based on its own rule
        // would incorrectly fence explicitly permitted children of that directory.
        let mut initial = ProjectionSnapshot {
            schema_version: "layrs.projection-snapshot.v1".into(),
            base_step_id: step.clone(),
            base_root_tree_id: root_tree
                .ok_or(NativeRemoteError::Invalid("missing projection root"))?,
            observed_policy_set_id: base.current_policy_set_id.clone(),
            observed_authorization_epoch: base.authorization_epoch,
            path_semantics: semantics,
            entries: entries
                .values()
                .filter(|entry| !matches!(entry.target, TreeEntryTarget::Tree { .. }))
                .map(|entry| LocalEntry {
                    path: entry.path.clone(),
                    target: Some(entry.target.clone()),
                    executable: entry.executable,
                    writable: entry.writable && entry.content_readable,
                    state: if entry.content_readable {
                        EntryState::Unavailable
                    } else {
                        EntryState::Reserved
                    },
                })
                .collect(),
        };
        initial.validate().map_err(local)?;
        selection.validate_snapshot(&initial).map_err(local)?;
        let binding = Binding {
            origin: self.origin().into(),
            space_id: space.clone(),
            client_id: ClientId::parse(format!("client:{}", uuid::Uuid::new_v4()))
                .map_err(local)?,
            clone_pending: false,
            observed: Some(base.clone()),
            pending: None,
            last_receipt: None,
        };
        let mut prepared_files = BTreeMap::new();
        let mut graph = GraphObjects::default();
        let mut total_bytes = 0u64;
        let mut target = initial.clone();
        for entry in &mut target.entries {
            if !selection.includes(&entry.path) {
                continue;
            }
            let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
                continue;
            };
            if entry.state != EntryState::Unavailable || (cfg!(windows) && entry.executable) {
                continue;
            }
            // Always prove authority for this exact path, even for duplicate FileIds.
            let actual = self.fetch_authorized_file(&binding, step, &entry.path, &mut graph)?;
            if &actual != file_id {
                return Err(NativeRemoteError::Invalid(
                    "projection file identity changed",
                ));
            }
            graph.verify_file(graph.manifest(file_id)?)?;
            total_bytes = total_bytes
                .checked_add(graph.manifest(file_id)?.byte_length())
                .ok_or(NativeRemoteError::Invalid("partial clone size overflow"))?;
            if total_bytes > MAX_STREAM_BYTES {
                return Err(NativeRemoteError::Invalid(
                    "partial clone exceeds transfer budget",
                ));
            }
            prepared_files.insert(entry.path.to_string(), file_id.clone());
            entry.state = EntryState::Materialized;
        }
        // No local metadata exists until all required transfers have verified.
        let current = self.bootstrap(space)?;
        if current.principal_id != base.principal_id
            || current.current_policy_set_id != base.current_policy_set_id
            || current.authorization_epoch != base.authorization_epoch
        {
            return Err(NativeRemoteError::Invalid(
                "partial clone authorization changed",
            ));
        }
        if std::fs::read_dir(root)?.next().is_some() {
            return Err(NativeRemoteError::Invalid(
                "partial clone destination changed",
            ));
        }
        // A denied entry never carries a write grant in local storage.
        for entry in &mut initial.entries {
            if entry.state == EntryState::Reserved {
                entry.writable = false;
            }
        }
        let identity = ProjectionIdentity {
            store_id: StoreId::parse(format!("store:{}", uuid::Uuid::new_v4()))?,
            worktree_id: WorktreeId::parse(format!("worktree:{}", uuid::Uuid::new_v4()))?,
            space_id: space.clone(),
            origin: self.origin().trim_end_matches('/').into(),
            principal_id: base.principal_id.to_string(),
        };
        let store =
            ProjectionStore::create_selected(root, identity, initial, selection).map_err(local)?;
        let generation = store.state().map_err(local)?.generation;
        store
            .install_projection(generation, &target, &mut |path, output| {
                let file = prepared_files
                    .get(path)
                    .ok_or_else(|| std::io::Error::other("missing verified projection content"))?;
                graph
                    .write_file(file, output)
                    .map(|_| ())
                    .map_err(|e| std::io::Error::other(e.to_string()))
            })
            .map_err(local)?;
        Ok(store)
    }
}
