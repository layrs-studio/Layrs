//! Bounded directory moves keep each leaf tied to a fresh path classification.
use super::*;
use layrs_core::projection::{EntryState, PathSemantics};
use layrs_store_local::projection::{
    CapturedPatch, DirectoryMovePreview, ProjectionIdentity, ProjectionStore, VersionedProjection,
};
use std::{collections::BTreeSet, fs, io::ErrorKind};

const SCHEMA: &str = "layrs.native-projection-directory-move-preview.v1";
const MAX_FILES: usize = 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeProjectionDirectoryPreview {
    pub schema_version: String,
    pub identity: ProjectionIdentity,
    pub authorizations: Vec<ProjectionAccess>,
    pub local: DirectoryMovePreview,
}

struct MovePaths {
    sources: BTreeSet<RepoPath>,
    destinations: BTreeSet<RepoPath>,
    all: Vec<RepoPath>,
}

fn invalid(message: &'static str) -> NativeRemoteError {
    NativeRemoteError::Invalid(message)
}
fn local(error: impl std::fmt::Display) -> NativeRemoteError {
    NativeRemoteError::Io(std::io::Error::other(error.to_string()))
}

/// Find candidates only among observed writable leaves. Store3 subsequently
/// scans the entire pinned checkout, including unknown files and directories.
fn move_paths(
    store: &ProjectionStore,
    state: &VersionedProjection,
    source: &RepoPath,
    destination: &RepoPath,
) -> RemoteResult<MovePaths> {
    move_paths_observed(store, state, source, destination, None)
}
fn move_paths_observed(
    store: &ProjectionStore,
    state: &VersionedProjection,
    source: &RepoPath,
    destination: &RepoPath,
    recorded: Option<&BTreeMap<String, layrs_store_local::projection::CapturedFile>>,
) -> RemoteResult<MovePaths> {
    let reserved = state.snapshot.reserved_paths().map_err(local)?;
    reserved
        .check_move(source.as_str(), destination.as_str())
        .map_err(local)?;
    let key = |path: &str| match state.snapshot.path_semantics {
        PathSemantics::PortableWindows => path.to_uppercase(),
        PathSemantics::CaseSensitive => path.to_owned(),
    };
    let from = key(source.as_str());
    let to = key(destination.as_str());
    if from == to || from.starts_with(&format!("{to}/")) || to.starts_with(&format!("{from}/")) {
        return Err(invalid("directory move paths overlap or alias"));
    }
    if state
        .snapshot
        .entries
        .binary_search_by(|entry| entry.path.cmp(source))
        .ok()
        .is_some_and(|index| !state.snapshot.entries[index].writable)
    {
        return Err(invalid("directory source root is not writable"));
    }
    let prefix = format!("{source}/");
    let mut sources = BTreeSet::new();
    let mut destinations = BTreeSet::new();
    for entry in &state.snapshot.entries {
        let Some(suffix) = entry.path.as_str().strip_prefix(&prefix) else {
            continue;
        };
        if matches!(entry.target, Some(TreeEntryTarget::Tree { .. })) {
            if !entry.writable {
                return Err(invalid("directory source contains a read-only directory"));
            }
            continue;
        }
        if !entry.writable
            || !matches!(
                (&entry.state, &entry.target),
                (EntryState::Materialized, Some(TreeEntryTarget::File { .. }))
                    | (EntryState::Absent, None)
            )
        {
            return Err(invalid(
                "directory source contains a non-writable or unknown leaf",
            ));
        }
        if let Some(recorded) = recorded {
            if !recorded.contains_key(entry.path.as_str()) { continue; }
        } else {
            let disk_path = store.checkout_root().join(entry.path.as_str());
            let metadata = match fs::symlink_metadata(&disk_path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => return Err(NativeRemoteError::Io(error)),
            };
            if !metadata.file_type().is_file() {
                return Err(invalid("directory source leaf is not an ordinary file"));
            }
        }
        let mapped = RepoPath::parse(format!("{destination}/{suffix}"))
            .map_err(|_| invalid("invalid directory destination leaf"))?;
        if mapped.as_str().len() > 4096 || mapped.as_str().split('/').count() > 128 {
            return Err(invalid("directory destination exceeds path budget"));
        }
        reserved
            .check_move(entry.path.as_str(), mapped.as_str())
            .map_err(local)?;
        if let Ok(index) = state
            .snapshot
            .entries
            .binary_search_by(|candidate| candidate.path.cmp(&mapped))
        {
            let candidate = &state.snapshot.entries[index];
            if candidate.state != EntryState::Absent
                || candidate.target.is_some()
                || !candidate.writable
            {
                return Err(invalid("directory destination is not a writable absence"));
            }
        }
        sources.insert(entry.path.clone());
        destinations.insert(mapped);
        if sources.len() > MAX_FILES {
            return Err(invalid("directory move exceeds file budget"));
        }
    }
    if sources.is_empty() || destinations.len() != sources.len() {
        return Err(invalid("directory move requires observed source files"));
    }
    let all: Vec<_> = sources.union(&destinations).cloned().collect();
    if all.len() != sources.len() + destinations.len() {
        return Err(invalid("directory source and destination overlap"));
    }
    Ok(MovePaths {
        sources,
        destinations,
        all,
    })
}

fn check_authorizations(
    identity: &ProjectionIdentity,
    state: &VersionedProjection,
    paths: &MovePaths,
    batches: &[ProjectionAccess],
) -> RemoteResult<()> {
    let mut checked = Vec::new();
    for batch in batches {
        if batch.principal_id.as_str() != identity.principal_id
            || batch.space_id != identity.space_id
            || batch.step_id != state.snapshot.base_step_id
            || batch.root_tree_id != state.snapshot.base_root_tree_id
            || batch.policy_set_id != state.snapshot.observed_policy_set_id
            || batch.authorization_epoch != state.snapshot.observed_authorization_epoch
            || batch.entries.is_empty()
            || batch.entries.len() > 200
        {
            return Err(invalid("directory authorization scope changed"));
        }
        for entry in &batch.entries {
            if !entry.content_readable || !entry.writable {
                return Err(invalid("directory path access denied"));
            }
            if paths.destinations.contains(&entry.path) {
                if entry.target.is_some() || entry.executable {
                    return Err(invalid("directory destination is present"));
                }
            } else if paths.sources.contains(&entry.path) {
                let index = state
                    .snapshot
                    .entries
                    .binary_search_by(|candidate| candidate.path.cmp(&entry.path))
                    .map_err(|_| invalid("directory source disappeared"))?;
                let source = &state.snapshot.entries[index];
                if entry.target.as_ref() != source.target.as_ref()
                    || entry.executable != source.executable
                {
                    return Err(invalid("directory source target changed"));
                }
            } else {
                return Err(invalid("unexpected directory authorization path"));
            }
            checked.push(entry.path.clone());
        }
    }
    if checked != paths.all {
        return Err(invalid("directory authorization path set changed"));
    }
    Ok(())
}

impl NativeRemote {
    fn projection_directory_access(
        &self,
        store: &ProjectionStore,
        state: &VersionedProjection,
        paths: &MovePaths,
    ) -> RemoteResult<Vec<ProjectionAccess>> {
        let mut authorizations = Vec::new();
        for chunk in paths.all.chunks(200) {
            let request = ProjectionAccessRequest {
                step_id: state.snapshot.base_step_id.clone(),
                expected_policy_set_id: state.snapshot.observed_policy_set_id.clone(),
                expected_authorization_epoch: state.snapshot.observed_authorization_epoch,
                paths: chunk.to_vec(),
            };
            authorizations.push(self.projection_access(&store.identity().space_id, &request)?);
        }
        check_authorizations(store.identity(), state, paths, &authorizations)?;
        let current = self.bootstrap(&store.identity().space_id)?;
        if current.principal_id.as_str() != store.identity().principal_id
            || current.current_policy_set_id != state.snapshot.observed_policy_set_id
            || current.authorization_epoch != state.snapshot.observed_authorization_epoch
        {
            return Err(invalid(
                "directory authorization changed during preparation",
            ));
        }
        let after = store.state().map_err(local)?;
        if after.generation != state.generation || after.digest != state.digest {
            return Err(invalid("directory projection changed during preparation"));
        }
        Ok(authorizations)
    }

    pub fn preview_projection_directory_move(
        &self,
        store: &ProjectionStore,
        expected_generation: u64,
        source: &RepoPath,
        destination: &RepoPath,
    ) -> RemoteResult<NativeProjectionDirectoryPreview> {
        if self.origin().trim_end_matches('/') != store.identity().origin.trim_end_matches('/') {
            return Err(invalid("directory origin mismatch"));
        }
        let before = store.state().map_err(local)?;
        if before.generation != expected_generation {
            return Err(invalid("directory projection generation changed"));
        }
        let paths = move_paths(store, &before, source, destination)?;
        let authorizations = self.projection_directory_access(store, &before, &paths)?;
        let observed = store
            .observe_absent_paths(
                before.generation,
                &before.digest,
                &paths.destinations.iter().cloned().collect::<Vec<_>>(),
            )
            .map_err(local)?;
        let local = store
            .preview_directory_move(observed.generation, source, destination)
            .map_err(local)?;
        if local.source_files.keys().cloned().collect::<BTreeSet<_>>()
            != paths.sources.iter().map(ToString::to_string).collect()
        {
            return Err(invalid("directory source changed during preparation"));
        }
        Ok(NativeProjectionDirectoryPreview {
            schema_version: SCHEMA.into(),
            identity: store.identity().clone(),
            authorizations,
            local,
        })
    }

    /// A preview is not enduring authority. Recheck all leaves before the local
    /// engine enters its exclusive, journaled move.
    pub fn apply_projection_directory_move(
        &self,
        store: &ProjectionStore,
        preview: &NativeProjectionDirectoryPreview,
    ) -> RemoteResult<CapturedPatch> {
        if preview.schema_version != SCHEMA || &preview.identity != store.identity() {
            return Err(invalid("directory preview identity mismatch"));
        }
        if self.origin().trim_end_matches('/') != store.identity().origin.trim_end_matches('/') {
            return Err(invalid("directory origin mismatch"));
        }
        let state = store.state().map_err(local)?;
        if state.generation != preview.local.projection_generation
            || state.digest != preview.local.projection_digest
        {
            return Err(invalid("directory projection generation changed"));
        }
        let paths = move_paths_observed(
            store,
            &state,
            &preview.local.source,
            &preview.local.destination,
            Some(&preview.local.source_files),
        )?;
        if preview
            .local
            .source_files
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
            != paths.sources.iter().map(ToString::to_string).collect()
        {
            return Err(invalid("directory source changed since preview"));
        }
        check_authorizations(store.identity(), &state, &paths, &preview.authorizations)?;
        let current = self.projection_directory_access(store, &state, &paths)?;
        if current != preview.authorizations {
            return Err(invalid("directory authority changed after preview"));
        }
        store.apply_directory_move(&preview.local).map_err(local)
    }
}

#[cfg(test)]
#[path = "projection_directory_tests.rs"]
mod tests;
