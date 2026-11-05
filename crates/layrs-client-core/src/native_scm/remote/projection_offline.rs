//! Explicit exact-version acquisition. All HTTP precedes private Store acceptance.
use super::*;
use layrs_core::projection::{
    AvailabilityBlocker, AvailabilityRequest, EntryState, OfflineOperation, ProjectionAvailability,
    ProjectionEntry, ProjectionSelection, ProjectionSnapshot, ProjectionVersion,
    VersionPathAvailability,
};
use layrs_store_local::projection::{OfflinePreparation, ProjectionStore, VersionedProjection};

#[cfg(test)]
#[path = "projection_offline_tests.rs"]
mod tests;

fn local(error: impl std::fmt::Display) -> NativeRemoteError {
    NativeRemoteError::Io(std::io::Error::other(error.to_string()))
}

struct Acquisition {
    graph: GraphObjects,
    files: BTreeMap<(Vec<u8>, RepoPath), FileId>,
}
struct OfflineOperand {
    version: ProjectionVersion,
    snapshot: ProjectionSnapshot,
    paths: Vec<RepoPath>,
}
fn needs_content(
    request: &AvailabilityRequest,
    coverage: &ProjectionAvailability,
    row: &VersionPathAvailability,
) -> bool {
    match request.operation {
        OfflineOperation::CompareMetadata => false,
        OfflineOperation::CompareContent => coverage.paths.iter().any(|other| {
            other.path == row.path && other.operand != row.operand && other.file_id != row.file_id
        }),
        _ => true,
    }
}
impl Acquisition {
    fn write(
        &self,
        version: &ProjectionVersion,
        path: &RepoPath,
        out: &mut dyn Write,
    ) -> std::io::Result<()> {
        // Missing captured/protected content is corruption, never a network gap.
        let key = (
            canonical_json(version).map_err(std::io::Error::other)?,
            path.clone(),
        );
        let file = self.files.get(&key).ok_or_else(|| {
            std::io::Error::other("exact version content was not prefetched; no remote fallback")
        })?;
        self.graph
            .write_file(file, out)
            .map_err(std::io::Error::other)?;
        Ok(())
    }
}

fn current(
    store: &ProjectionStore,
    remote: &NativeRemote,
    generation: u64,
) -> RemoteResult<VersionedProjection> {
    if !store.is_shared() {
        return Err(NativeRemoteError::Invalid(
            "offline acquisition requires a shared partial Store",
        ));
    }
    if remote.origin().trim_end_matches('/') != store.identity().origin {
        return Err(NativeRemoteError::Invalid(
            "offline acquisition origin mismatch",
        ));
    }
    let state = store.state().map_err(local)?;
    if state.generation != generation {
        return Err(NativeRemoteError::Invalid(
            "offline acquisition generation changed",
        ));
    }
    if store.pending_publication().map_err(local)?.is_some() {
        return Err(NativeRemoteError::PublicationPending);
    }
    Ok(state)
}

fn binding(remote: &NativeRemote, base: &SpaceBootstrap) -> RemoteResult<Binding> {
    Ok(Binding {
        origin: remote.origin().into(),
        space_id: base.space_id.clone(),
        client_id: ClientId::parse(format!("client:{}", uuid::Uuid::new_v4())).map_err(local)?,
        clone_pending: false,
        observed: Some(base.clone()),
        pending: None,
        last_receipt: None,
    })
}

fn authority(
    store: &ProjectionStore,
    base: &SpaceBootstrap,
    snapshot: &ProjectionSnapshot,
) -> RemoteResult<()> {
    if base.principal_id.as_str() != store.identity().principal_id
        || base.space_id != store.identity().space_id
        || base.current_policy_set_id != snapshot.observed_policy_set_id
        || base.authorization_epoch != snapshot.observed_authorization_epoch
    {
        return Err(NativeRemoteError::Invalid(
            "offline version authority changed; observe the exact version again",
        ));
    }
    Ok(())
}

impl NativeRemote {
    /// Records exact path observations without changing the checkout or its selection.
    /// This separate selection describes the explicit offline request, not an ACL.
    pub fn observe_projection_version(
        &self,
        store: &ProjectionStore,
        expected_generation: u64,
        step: &StepId,
        paths: &[RepoPath],
    ) -> RemoteResult<ProjectionVersion> {
        let state = current(store, self, expected_generation)?;
        if paths.is_empty() || paths.len() > 1024 || paths.windows(2).any(|p| p[0] >= p[1]) {
            return Err(NativeRemoteError::Invalid(
                "observation requires 1..1024 sorted unique paths",
            ));
        }
        let selection =
            ProjectionSelection::selected(paths.to_vec(), vec![], state.snapshot.path_semantics)
                .map_err(local)?;
        let base = self.bootstrap(&store.identity().space_id)?;
        if base.principal_id.as_str() != store.identity().principal_id {
            return Err(NativeRemoteError::Invalid(
                "offline observation principal changed",
            ));
        }
        let binding = binding(self, &base)?;
        let mut graph = GraphObjects::default();
        self.fetch_visit(
            &binding,
            FetchClaim::Object {
                object: ObjectReference {
                    kind: ObjectKind::Step,
                    object_id: ObjectId::parse(step.as_str())?,
                },
                root: AuthorizationRoot::Step {
                    step_id: step.clone(),
                },
                repo_path: Nullable::null(),
            },
            |object| graph.insert(object),
        )?;
        let root = graph
            .steps
            .get(step)
            .ok_or(NativeRemoteError::Invalid("observed Step missing"))?
            .root_tree_id()
            .clone();
        let mut snapshot = ProjectionSnapshot {
            schema_version: "layrs.projection-snapshot.v1".into(),
            base_step_id: step.clone(),
            base_root_tree_id: root,
            observed_policy_set_id: base.current_policy_set_id.clone(),
            observed_authorization_epoch: base.authorization_epoch,
            path_semantics: state.snapshot.path_semantics,
            entries: Vec::new(),
        };
        for batch in paths.chunks(200) {
            let response = self.offline_access(&base, &snapshot, batch)?;
            for entry in response.entries {
                // The canonical snapshot cannot represent denied absence; never
                // manufacture an absence grant from an unreadable observation.
                if entry.target.is_none() && !entry.content_readable {
                    return Err(NativeRemoteError::ContentRestricted);
                }
                snapshot.entries.push(ProjectionEntry {
                    path: entry.path,
                    state: if entry.target.is_none() {
                        EntryState::Absent
                    } else if entry.content_readable {
                        EntryState::Unavailable
                    } else {
                        EntryState::Reserved
                    },
                    target: entry.target.0,
                    executable: entry.executable,
                    writable: entry.writable && entry.content_readable,
                });
            }
        }
        selection.validate_snapshot(&snapshot).map_err(local)?;
        authority(store, &self.bootstrap(&base.space_id)?, &snapshot)?;
        store
            .record_version_observation_at_generation(expected_generation, &snapshot, &selection)
            .map_err(local)
    }

    /// Explicit preparation may acquire missing remote bytes. Local read and
    /// compare APIs never call this method or perform HTTP implicitly.
    pub fn prepare_projection_offline(
        &self,
        store: &ProjectionStore,
        expected_generation: u64,
        request: &AvailabilityRequest,
    ) -> RemoteResult<OfflinePreparation> {
        current(store, self, expected_generation)?;
        request.validate().map_err(local)?;
        self.offline_operands(store, request)?;
        let preparation = store
            .begin_preparation_at_generation(expected_generation, request)
            .map_err(local)?;
        self.resume_projection_offline(store, expected_generation, &preparation.id)
    }

    /// The durable preparation supplies the operands and paths; callers cannot
    /// substitute a different request on resume.
    pub fn resume_projection_offline(
        &self,
        store: &ProjectionStore,
        expected_generation: u64,
        id: &str,
    ) -> RemoteResult<OfflinePreparation> {
        let state = current(store, self, expected_generation)?;
        let preparation = store
            .preparation(id)
            .map_err(local)?
            .ok_or(NativeRemoteError::Invalid("offline preparation not found"))?;
        if preparation.status == "released" {
            return Err(NativeRemoteError::Invalid(
                "offline preparation was released",
            ));
        }
        let acquisition = self.prefetch_offline(store, &state, &preparation.request)?;
        store
            .resume_preparation_at_generation(expected_generation, id, &mut |version, path, out| {
                acquisition.write(version, path, out)
            })
            .map_err(local)
    }

    fn offline_operands(
        &self,
        store: &ProjectionStore,
        request: &AvailabilityRequest,
    ) -> RemoteResult<Vec<OfflineOperand>> {
        request.validate().map_err(local)?;
        let coverage = store.availability(request).map_err(local)?;
        if !coverage.blockers.is_empty()
            || coverage.paths.iter().any(|p| {
                p.blockers
                    .iter()
                    .any(|b| *b != AvailabilityBlocker::MissingContent)
            })
        {
            return Err(NativeRemoteError::Invalid(
                "offline preparation is blocked by local availability",
            ));
        }
        let mut operands = Vec::new();
        for version in std::iter::once(&request.version).chain(request.other.as_ref()) {
            // Resolving a Capture verifies every protected replacement first.
            // An unchanged path may legitimately have no retained base bytes.
            let (mut snapshot, _) = store.version_metadata(version).map_err(local)?;
            let mut paths = request.paths.clone();
            if let ProjectionVersion::Capture {
                sequence, digest, ..
            } = version
            {
                let patch = store.captured_patch(*sequence).map_err(local)?;
                if patch.digest != *digest {
                    return Err(NativeRemoteError::Invalid(
                        "capture version digest mismatch",
                    ));
                }
                // Imported patches retain their original identity, while their
                // copied base generation belongs to the destination Worktree.
                let base_version = ProjectionVersion::Generation {
                    worktree_id: store.identity().worktree_id.clone(),
                    generation: patch.projection_generation,
                    digest: patch.projection_digest,
                };
                snapshot = store.version_metadata(&base_version).map_err(local)?.0;
                paths.retain(|path| !patch.changes.iter().any(|change| &change.path == path));
            }
            operands.push(OfflineOperand {
                version: version.clone(),
                snapshot,
                paths,
            });
        }
        Ok(operands)
    }

    fn prefetch_offline(
        &self,
        store: &ProjectionStore,
        state: &VersionedProjection,
        request: &AvailabilityRequest,
    ) -> RemoteResult<Acquisition> {
        let operands = self.offline_operands(store, request)?;
        let coverage = store.availability(request).map_err(local)?;
        let mut acquisition = Acquisition {
            graph: GraphObjects::default(),
            files: BTreeMap::new(),
        };
        if request.operation == OfflineOperation::CompareMetadata
            || !coverage
                .paths
                .iter()
                .any(|p| needs_content(request, &coverage, p))
            || operands.iter().all(|operand| operand.paths.is_empty())
        {
            return Ok(acquisition);
        }
        let base = self.bootstrap(&store.identity().space_id)?;
        let binding = binding(self, &base)?;
        let mut total = 0u64;
        for (index, operand) in operands.iter().enumerate() {
            if operand.paths.is_empty() {
                continue;
            }
            let snapshot = &operand.snapshot;
            authority(store, &base, snapshot)?;
            for batch in operand.paths.chunks(200) {
                let access = self.offline_access(&base, snapshot, batch)?;
                for observed in access.entries {
                    let entry = snapshot
                        .entries
                        .binary_search_by(|e| e.path.cmp(&observed.path))
                        .ok()
                        .map(|n| &snapshot.entries[n])
                        .ok_or(NativeRemoteError::Invalid("offline path metadata missing"))?;
                    if observed.target.0 != entry.target || observed.executable != entry.executable
                    {
                        return Err(NativeRemoteError::Invalid(
                            "offline exact path target changed",
                        ));
                    }
                    if !observed.content_readable {
                        return Err(NativeRemoteError::ContentRestricted);
                    }
                    let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
                        continue;
                    };
                    let row = coverage
                        .paths
                        .iter()
                        .find(|p| p.operand as usize == index && p.path == entry.path)
                        .ok_or(NativeRemoteError::Invalid(
                            "offline availability path missing",
                        ))?;
                    if row.manifest_id.is_some() || !needs_content(request, &coverage, row) {
                        continue;
                    }
                    let actual = self.fetch_authorized_file(
                        &binding,
                        &snapshot.base_step_id,
                        &entry.path,
                        &mut acquisition.graph,
                    )?;
                    if &actual != file_id
                        || acquisition
                            .graph
                            .steps
                            .get(&snapshot.base_step_id)
                            .is_none_or(|s| s.root_tree_id() != &snapshot.base_root_tree_id)
                    {
                        return Err(NativeRemoteError::Invalid(
                            "offline acquisition Step/root/file mismatch",
                        ));
                    }
                    let manifest = acquisition.graph.manifest(file_id)?;
                    acquisition.graph.verify_file(manifest)?;
                    total = total
                        .checked_add(manifest.byte_length())
                        .filter(|n| *n <= MAX_STREAM_BYTES)
                        .ok_or(NativeRemoteError::Invalid(
                            "offline acquisition exceeds4GiB",
                        ))?;
                    acquisition.files.insert(
                        (canonical_json(&operand.version)?, entry.path.clone()),
                        file_id.clone(),
                    );
                }
            }
        }
        // Historical writable flags never grant rights at the current destination.
        if matches!(
            request.operation,
            OfflineOperation::Edit | OfflineOperation::Capture | OfflineOperation::Restore
        ) {
            authority(store, &base, &state.snapshot)?;
            for batch in request.paths.chunks(200) {
                let access = self.offline_access(&base, &state.snapshot, batch)?;
                if access
                    .entries
                    .iter()
                    .any(|e| !e.content_readable || !e.writable)
                {
                    return Err(NativeRemoteError::ContentRestricted);
                }
            }
        }
        let after = self.bootstrap(&base.space_id)?;
        if after.principal_id != base.principal_id
            || after.current_policy_set_id != base.current_policy_set_id
            || after.authorization_epoch != base.authorization_epoch
        {
            return Err(NativeRemoteError::Invalid(
                "offline acquisition authority changed before acceptance",
            ));
        }
        Ok(acquisition)
    }

    fn offline_access(
        &self,
        base: &SpaceBootstrap,
        snapshot: &ProjectionSnapshot,
        paths: &[RepoPath],
    ) -> RemoteResult<ProjectionAccess> {
        let access = self.projection_access(
            &base.space_id,
            &ProjectionAccessRequest {
                step_id: snapshot.base_step_id.clone(),
                expected_policy_set_id: base.current_policy_set_id.clone(),
                expected_authorization_epoch: base.authorization_epoch,
                paths: paths.to_vec(),
            },
        )?;
        if access.principal_id != base.principal_id
            || access.root_tree_id != snapshot.base_root_tree_id
        {
            return Err(NativeRemoteError::Invalid(
                "offline exact path authority or root mismatch",
            ));
        }
        Ok(access)
    }
}
