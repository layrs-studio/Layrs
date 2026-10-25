//! Three-way reconciliation of authorized regular leaves. All network reads and
//! merge work precede the local journal's exclusive lock.
mod historical;
mod merge;
use super::*;
pub use historical::ProjectionHistoricalSelection;
use layrs_core::projection::{
    AvailabilityRequest, EntryState, OfflineOperation, ProjectionEntry, ProjectionSnapshot,
    ProjectionVersion,
};
use layrs_store_local::projection::{
    CapturedFile, CapturedPatch, ProjectionIdentity, ProjectionStore, RebasePreview,
};
use std::io::{Seek, SeekFrom};

fn local(e: impl std::fmt::Display) -> NativeRemoteError {
    NativeRemoteError::Io(std::io::Error::other(e.to_string()))
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectionMergeChoice {
    Base,
    Local,
    Remote,
    Delete,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectionMergeFile {
    pub path: RepoPath,
    pub base: Option<CapturedFile>,
    pub local: Option<CapturedFile>,
    pub remote: Option<CapturedFile>,
    pub conflict: bool,
    /// None is an automatically resolved deletion when conflict is false.
    pub automatic: Option<CapturedFile>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeProjectionRebasePreview {
    pub schema_version: String,
    pub identity: ProjectionIdentity,
    pub layer_id: LayerId,
    #[serde(default)]
    pub historical: Option<ProjectionHistoricalSelection>,
    pub repository_view_id: RepositoryViewId,
    pub repository_view_generation: u64,
    pub files: Vec<ProjectionMergeFile>,
    pub local: RebasePreview,
}
struct Prepared {
    preview: NativeProjectionRebasePreview,
    graph: GraphObjects,
    spools: BTreeMap<FileId, File>,
}
impl Prepared {
    fn write(&mut self, file: &FileId, out: &mut dyn Write) -> RemoteResult<()> {
        if let Some(spool) = self.spools.get_mut(file) {
            spool.seek(SeekFrom::Start(0))?;
            std::io::copy(spool, out)?;
        } else {
            self.graph.write_file(file, out)?;
        }
        Ok(())
    }
}
impl NativeRemote {
    pub fn preview_projection_rebase(
        &self,
        store: &ProjectionStore,
        sequence: u64,
        layer: &LayerId,
    ) -> RemoteResult<NativeProjectionRebasePreview> {
        self.preview_projection_rebase_kind(store, sequence, layer, None)
    }
    fn preview_projection_rebase_kind(
        &self,
        store: &ProjectionStore,
        sequence: u64,
        layer: &LayerId,
        historical: Option<&ProjectionHistoricalSelection>,
    ) -> RemoteResult<NativeProjectionRebasePreview> {
        let prepared = self.prepare_projection_rebase(store, sequence, layer, None, historical)?;
        store
            .save_rebase_draft(
                prepared.preview.local.projection_generation,
                &prepared.preview.local.projection_digest,
                sequence,
                &canonical_json(&prepared.preview)?,
            )
            .map_err(local)?;
        Ok(prepared.preview)
    }
    pub fn apply_projection_rebase(
        &self,
        store: &ProjectionStore,
        preview: &NativeProjectionRebasePreview,
        choices: &BTreeMap<RepoPath, ProjectionMergeChoice>,
    ) -> RemoteResult<CapturedPatch> {
        self.rebase_identity(store)?;
        let stored = store
            .rebase_draft()
            .map_err(local)?
            .ok_or(NativeRemoteError::Invalid("no saved partial merge draft"))?;
        if canonical_json(preview)? != stored {
            return Err(NativeRemoteError::Invalid(
                "partial merge differs from saved draft",
            ));
        }
        if choices
            .keys()
            .any(|p| !preview.files.iter().any(|f| &f.path == p && f.conflict))
            || preview
                .files
                .iter()
                .any(|f| f.conflict && !choices.contains_key(&f.path))
        {
            return Err(NativeRemoteError::Invalid(
                "choose exactly one value for every merge conflict",
            ));
        }
        // The durable operation binds both the draft and every explicit choice.
        let bytes = canonical_json(&(preview, choices))?;
        let digest = blake3::hash(&bytes);
        let operation = format!(
            "{}-{}",
            u128::from_be_bytes(digest.as_bytes()[..16].try_into().unwrap()),
            u128::from_be_bytes(digest.as_bytes()[16..].try_into().unwrap())
        );
        if let Some(done) = store.completed_rebase(&operation).map_err(local)? {
            return Ok(done);
        }
        let mut prepared = self.prepare_projection_rebase(
            store,
            preview.local.capture_sequence,
            &preview.layer_id,
            Some(&preview.local.operation_id),
            preview.historical.as_ref(),
        )?;
        if prepared.preview != *preview {
            return Err(NativeRemoteError::Invalid(
                "partial merge draft is stale; preview again",
            ));
        }
        let resolutions: BTreeMap<_, _> = preview
            .files
            .iter()
            .map(|f| {
                let file = match choices.get(&f.path) {
                    Some(ProjectionMergeChoice::Base) => f.base.clone(),
                    Some(ProjectionMergeChoice::Local) => f.local.clone(),
                    Some(ProjectionMergeChoice::Remote) => f.remote.clone(),
                    Some(ProjectionMergeChoice::Delete) => None,
                    None => f.automatic.clone(),
                };
                (f.path.clone(), file)
            })
            .collect();
        let plan = store
            .preview_rebase(
                preview.local.projection_generation,
                preview.local.capture_sequence,
                &preview.local.target,
                &resolutions,
            )
            .map_err(local)?
            .with_operation_id(&operation)
            .map_err(local)?;
        if store.is_shared() {
            // The callback below writes resolved checkout values. Preserve the
            // independently verified incoming base first, including read-only
            // selected leaves, so the two versions cannot be confused.
            let (_, selection) = store.state_with_selection().map_err(local)?;
            let version = store
                .record_version_observation_at_generation(
                    preview.local.projection_generation,
                    &plan.target,
                    &selection,
                )
                .map_err(local)?;
            let paths: Vec<_> = plan
                .target
                .entries
                .iter()
                .filter(|e| {
                    e.state == EntryState::Materialized
                        && matches!(e.target, Some(TreeEntryTarget::File { .. }))
                })
                .map(|e| e.path.clone())
                .collect();
            for paths in paths.chunks(1024) {
                let request = AvailabilityRequest {
                    version: version.clone(),
                    other: None,
                    paths: paths.to_vec(),
                    operation: OfflineOperation::Read,
                };
                store
                    .prepare_offline_at_generation(
                        preview.local.projection_generation,
                        &request,
                        &mut |actual, path, out| {
                            if actual != &version {
                                return Err(std::io::Error::other("incoming base version changed"));
                            }
                            let entry = plan
                                .target
                                .entries
                                .binary_search_by(|e| e.path.cmp(path))
                                .ok()
                                .map(|n| &plan.target.entries[n])
                                .ok_or_else(|| {
                                    std::io::Error::other("incoming base path missing")
                                })?;
                            let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
                                return Err(std::io::Error::other(
                                    "incoming base is not a regular file",
                                ));
                            };
                            prepared
                                .graph
                                .write_file(file_id, out)
                                .map_err(std::io::Error::other)?;
                            Ok(())
                        },
                    )
                    .map_err(local)?;
            }
        }
        store
            .apply_rebase(&plan, &mut |path, out| {
                let file = resolutions
                    .get(&RepoPath::parse(path).map_err(std::io::Error::other)?)
                    .and_then(Option::as_ref)
                    .ok_or_else(|| std::io::Error::other("missing resolved partial file"))?;
                prepared
                    .write(&file.file_id, out)
                    .map_err(std::io::Error::other)
            })
            .map_err(local)
    }
    fn rebase_identity(&self, store: &ProjectionStore) -> RemoteResult<()> {
        if self.origin().trim_end_matches('/') != store.identity().origin {
            return Err(NativeRemoteError::Invalid("partial merge origin mismatch"));
        }
        let principal: PrincipalId =
            serde_json::from_value(self.whoami()?["principal_id"].clone())?;
        if principal.as_str() != store.identity().principal_id {
            return Err(NativeRemoteError::Invalid(
                "partial merge principal changed",
            ));
        }
        if store.pending_publication().map_err(local)?.is_some() {
            return Err(NativeRemoteError::PublicationPending);
        }
        Ok(())
    }
    fn prepare_projection_rebase(
        &self,
        store: &ProjectionStore,
        sequence: u64,
        layer: &LayerId,
        operation: Option<&str>,
        historical: Option<&ProjectionHistoricalSelection>,
    ) -> RemoteResult<Prepared> {
        self.rebase_identity(store)?;
        let (before, selection) = store.state_with_selection().map_err(local)?;
        let capture = store.captured_patch(sequence).map_err(local)?;
        if capture.projection_generation != before.generation
            || capture.projection_digest != before.digest
        {
            return Err(NativeRemoteError::Invalid(
                "capture belongs to another partial base",
            ));
        }
        let base = self.bootstrap(&store.identity().space_id)?;
        let heads = &base
            .layers
            .iter()
            .find(|l| l.record.id() == layer)
            .ok_or(NativeRemoteError::Invalid("partial merge Layer missing"))?
            .head_step_ids;
        if historical.is_none() && heads.len() != 1 {
            return Err(NativeRemoteError::Invalid(
                "partial merge requires one selected remote head",
            ));
        }
        let step = if historical.is_some() {
            &before.snapshot.base_step_id
        } else {
            &heads[0]
        };
        let (root, entries) = self.collect_projection(&base, step)?;
        let mut target = ProjectionSnapshot {
            schema_version: before.snapshot.schema_version.clone(),
            base_step_id: step.clone(),
            base_root_tree_id: root,
            observed_policy_set_id: base.current_policy_set_id.clone(),
            observed_authorization_epoch: base.authorization_epoch,
            path_semantics: before.snapshot.path_semantics,
            entries: entries
                .into_iter()
                .filter(|e| !matches!(e.target, TreeEntryTarget::Tree { .. }))
                .map(|e| ProjectionEntry {
                    path: e.path.clone(),
                    target: Some(e.target.clone()),
                    executable: e.executable,
                    writable: e.writable && e.content_readable,
                    state: if !e.content_readable {
                        EntryState::Reserved
                    } else if matches!(e.target, TreeEntryTarget::File { .. })
                        && selection.includes(&e.path)
                        && !(cfg!(windows) && e.executable)
                    {
                        EntryState::Materialized
                    } else {
                        EntryState::Unavailable
                    },
                })
                .collect(),
        };
        // Deleted and explicitly absent local leaves must be reclassified under
        // current authority; local absence is never permission to create.
        let missing: Vec<_> = before
            .snapshot
            .entries
            .iter()
            .filter(|e| {
                selection.includes(&e.path) && !target.entries.iter().any(|n| n.path == e.path)
            })
            .map(|e| e.path.clone())
            .collect();
        for paths in missing.chunks(200) {
            let access = self.projection_access(
                &base.space_id,
                &ProjectionAccessRequest {
                    step_id: step.clone(),
                    expected_policy_set_id: base.current_policy_set_id.clone(),
                    expected_authorization_epoch: base.authorization_epoch,
                    paths: paths.to_vec(),
                },
            )?;
            if access.root_tree_id != target.base_root_tree_id
                || access.principal_id != base.principal_id
            {
                return Err(NativeRemoteError::Invalid(
                    "partial merge absence authority mismatch",
                ));
            }
            for e in access.entries {
                if e.target.is_some() || !e.content_readable || !e.writable {
                    return Err(NativeRemoteError::Invalid(
                        "partial merge removed path is protected or structural",
                    ));
                }
                target.entries.push(ProjectionEntry {
                    path: e.path,
                    target: None,
                    executable: false,
                    writable: true,
                    state: EntryState::Absent,
                });
            }
        }
        target.entries.sort_by(|a, b| a.path.cmp(&b.path));
        target.validate().map_err(local)?;
        selection.validate_snapshot(&target).map_err(local)?;
        let historic_entries = historical
            .map(|selection| self.historical_entries(&base, &target, selection))
            .transpose()?;
        let binding = Binding {
            origin: self.origin().into(),
            space_id: base.space_id.clone(),
            client_id: ClientId::parse(format!("client:{}", uuid::Uuid::new_v4()))
                .map_err(local)?,
            clone_pending: false,
            observed: Some(base.clone()),
            pending: None,
            last_receipt: None,
        };
        let mut graph = GraphObjects::default();
        let mut spools = BTreeMap::new();
        let mut files = Vec::new();
        let mut total = 0u64;
        for next in target.entries.iter().filter(|e| {
            e.writable && matches!(e.state, EntryState::Materialized | EntryState::Absent)
        }) {
            let old = before.snapshot.entries.iter().find(|e| e.path == next.path);
            let original = if store.is_shared() {
                retained_base(store, &before, old, &mut spools, &mut total)?
            } else {
                self.rebase_file(
                    &binding,
                    &before.snapshot.base_step_id,
                    old,
                    &mut graph,
                    &mut total,
                )?
            };
            let remote = if let (Some(selection), Some(entries)) = (historical, &historic_entries) {
                if let Some(entry) = entries.get(&next.path) {
                    self.rebase_file(
                        &binding,
                        &selection.step_id,
                        Some(entry),
                        &mut graph,
                        &mut total,
                    )?
                } else {
                    original.clone()
                }
            } else {
                self.rebase_file(&binding, step, Some(next), &mut graph, &mut total)?
            };
            let changed = capture.changes.iter().find(|c| c.path == next.path);
            let local_file = changed.map_or_else(|| original.clone(), |c| c.replacement.clone());
            if let Some(file) = changed.and_then(|c| c.replacement.as_ref()) {
                let mut spool = layrs_store_local::private_spool_file()?;
                store
                    .write_captured_file(sequence, &next.path, &mut spool)
                    .map_err(local)?;
                spools.insert(file.file_id.clone(), spool);
            }
            if let Some(entries) = &historic_entries {
                files.push(ProjectionMergeFile {
                    path: next.path.clone(),
                    base: original,
                    local: local_file.clone(),
                    remote: remote.clone(),
                    conflict: false,
                    automatic: if entries.contains_key(&next.path) {
                        remote
                    } else {
                        local_file
                    },
                });
            } else {
                files.push(merge::reconcile(
                    &next.path,
                    original,
                    local_file,
                    remote,
                    &graph,
                    &mut spools,
                )?);
            }
        }
        if store.is_shared() {
            for entry in target
                .entries
                .iter()
                .filter(|e| e.state == EntryState::Materialized)
            {
                let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
                    continue;
                };
                // Prove the incoming Step/path even when the same FileId was
                // fetched for the original or historical comparison operand.
                let actual = self.fetch_authorized_file(
                    &binding,
                    &target.base_step_id,
                    &entry.path,
                    &mut graph,
                )?;
                if &actual != file_id
                    || graph
                        .steps
                        .get(&target.base_step_id)
                        .is_none_or(|s| s.root_tree_id() != &target.base_root_tree_id)
                {
                    return Err(NativeRemoteError::Invalid(
                        "incoming rebase base identity mismatch",
                    ));
                }
                let manifest = graph.manifest(file_id)?;
                graph.verify_file(manifest)?;
                total = total
                    .checked_add(manifest.byte_length())
                    .filter(|n| *n <= MAX_STREAM_BYTES)
                    .ok_or(NativeRemoteError::Invalid(
                        "partial merge transfer exceeds4GiB",
                    ))?;
            }
        }
        let current = self.bootstrap(&base.space_id)?;
        if current.principal_id != base.principal_id
            || current.principal_id.as_str() != store.identity().principal_id
            || current.authorization_epoch != base.authorization_epoch
            || current.current_policy_set_id != base.current_policy_set_id
            || current.current_repository_view.repository_view_id
                != base.current_repository_view.repository_view_id
            || current.current_repository_view_generation != base.current_repository_view_generation
        {
            return Err(NativeRemoteError::Invalid(
                "partial merge remote authority or View changed",
            ));
        }
        let provisional = files
            .iter()
            .map(|f| {
                (
                    f.path.clone(),
                    if f.conflict {
                        f.local.clone()
                    } else {
                        f.automatic.clone()
                    },
                )
            })
            .collect();
        let mut local_plan = store
            .preview_rebase(before.generation, sequence, &target, &provisional)
            .map_err(local)?;
        if let Some(operation) = operation {
            local_plan = local_plan.with_operation_id(operation).map_err(local)?;
        }
        Ok(Prepared {
            preview: NativeProjectionRebasePreview {
                schema_version: "layrs.native-partial-rebase.v1".into(),
                identity: store.identity().clone(),
                layer_id: layer.clone(),
                historical: historical.cloned(),
                repository_view_id: base.current_repository_view.repository_view_id,
                repository_view_generation: base.current_repository_view_generation,
                files,
                local: local_plan,
            },
            graph,
            spools,
        })
    }
    fn rebase_file(
        &self,
        binding: &Binding,
        step: &StepId,
        entry: Option<&ProjectionEntry>,
        graph: &mut GraphObjects,
        total: &mut u64,
    ) -> RemoteResult<Option<CapturedFile>> {
        let Some(entry) = entry else { return Ok(None) };
        if entry.state == EntryState::Absent && entry.target.is_none() {
            return Ok(None);
        }
        let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
            return Err(NativeRemoteError::Invalid(
                "partial merge requires regular files",
            ));
        };
        if entry.state != EntryState::Materialized || !entry.writable {
            return Err(NativeRemoteError::Invalid(
                "partial merge changes protected path",
            ));
        }
        // Always obtain a current exact-path authorization even for deduped files.
        let actual = self.fetch_authorized_file(binding, step, &entry.path, graph)?;
        if &actual != file_id {
            return Err(NativeRemoteError::Invalid(
                "partial merge file identity changed",
            ));
        }
        let bytes = graph.manifest(file_id)?.byte_length();
        graph.verify_file(graph.manifest(file_id)?)?;
        *total = total
            .checked_add(bytes)
            .filter(|v| *v <= MAX_STREAM_BYTES)
            .ok_or(NativeRemoteError::Invalid(
                "partial merge transfer exceeds4GiB",
            ))?;
        Ok(Some(CapturedFile {
            file_id: file_id.clone(),
            executable: entry.executable,
            bytes,
        }))
    }
}

fn retained_base(
    store: &ProjectionStore,
    before: &layrs_store_local::projection::VersionedProjection,
    entry: Option<&ProjectionEntry>,
    spools: &mut BTreeMap<FileId, File>,
    total: &mut u64,
) -> RemoteResult<Option<CapturedFile>> {
    let Some(entry) = entry else {
        return Ok(None);
    };
    if entry.state == EntryState::Absent && entry.target.is_none() {
        return Ok(None);
    }
    let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
        return Err(NativeRemoteError::Invalid(
            "partial merge requires regular base files",
        ));
    };
    if entry.state != EntryState::Materialized || !entry.writable {
        return Err(NativeRemoteError::Invalid(
            "partial merge changes protected base path",
        ));
    }
    let version = ProjectionVersion::Generation {
        worktree_id: store.identity().worktree_id.clone(),
        generation: before.generation,
        digest: before.digest.clone(),
    };
    let mut spool = layrs_store_local::private_spool_file()?;
    // A missing or corrupt retained base is fatal; do not repair it from HTTP.
    store
        .read_version_file(&version, &entry.path, &mut spool)
        .map_err(local)?;
    let bytes = spool.seek(SeekFrom::End(0))?;
    *total = total
        .checked_add(bytes)
        .filter(|n| *n <= MAX_STREAM_BYTES)
        .ok_or(NativeRemoteError::Invalid(
            "partial merge inputs exceed4GiB",
        ))?;
    spools.insert(file_id.clone(), spool);
    Ok(Some(CapturedFile {
        file_id: file_id.clone(),
        executable: entry.executable,
        bytes,
    }))
}
