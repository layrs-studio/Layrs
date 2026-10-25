//! Refresh a partial checkout from a fully authorized structural observation.
//! File transfers finish before the durable local materializer takes its lock.
use super::*;
use layrs_core::projection::{EntryState, ProjectionEntry as LocalEntry, ProjectionSnapshot};
use layrs_store_local::projection::{ProjectionStore, VersionedProjection};

fn local(e: impl std::fmt::Display) -> NativeRemoteError {
    NativeRemoteError::Io(std::io::Error::other(e.to_string()))
}
impl NativeRemote {
    pub fn receive_projection(
        &self,
        store: &ProjectionStore,
        layer: &LayerId,
    ) -> RemoteResult<VersionedProjection> {
        if self.origin().trim_end_matches('/') != store.identity().origin {
            return Err(NativeRemoteError::Invalid("projection origin mismatch"));
        }
        if store.pending_publication().map_err(local)?.is_some() {
            return Err(NativeRemoteError::PublicationPending);
        }
        let (before, selection) = store.state_with_selection().map_err(local)?;
        let base = self.bootstrap(&store.identity().space_id)?;
        if base.principal_id.as_str() != store.identity().principal_id {
            return Err(NativeRemoteError::Invalid("projection principal changed"));
        }
        let heads = &base
            .layers
            .iter()
            .find(|l| l.record.id() == layer)
            .ok_or(NativeRemoteError::Invalid("projection Layer is missing"))?
            .head_step_ids;
        if heads.len() != 1 {
            return Err(NativeRemoteError::Invalid(
                "choose one remote head before receiving a partial checkout",
            ));
        }
        let step = &heads[0];
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
                .map(|e| LocalEntry {
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
        // An observed absent destination is valid only for its exact base and
        // authority. Preserve it when nothing remote has changed.
        if before.snapshot.base_step_id == target.base_step_id
            && before.snapshot.observed_policy_set_id == target.observed_policy_set_id
            && before.snapshot.observed_authorization_epoch == target.observed_authorization_epoch
        {
            for entry in &before.snapshot.entries {
                if entry.state == EntryState::Absent
                    && !target.entries.iter().any(|e| e.path == entry.path)
                {
                    target.entries.push(entry.clone());
                }
            }
            target.entries.sort_by(|a, b| a.path.cmp(&b.path));
        }
        target.validate().map_err(local)?;
        selection.validate_snapshot(&target).map_err(local)?;
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
        let mut files = BTreeMap::new();
        let mut total = 0u64;
        for entry in &target.entries {
            if entry.state != EntryState::Materialized {
                continue;
            }
            if before.snapshot.entries.iter().any(|old| {
                old.path == entry.path
                    && old.state == EntryState::Materialized
                    && old.target == entry.target
                    && old.executable == entry.executable
            }) {
                continue;
            }
            let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
                return Err(NativeRemoteError::Invalid(
                    "partial materialization is not a regular file",
                ));
            };
            let actual = self.fetch_authorized_file(&binding, step, &entry.path, &mut graph)?;
            if &actual != file_id {
                return Err(NativeRemoteError::Invalid(
                    "projection transfer identity changed",
                ));
            }
            graph.verify_file(graph.manifest(file_id)?)?;
            total = total
                .checked_add(graph.manifest(file_id)?.byte_length())
                .filter(|n| *n <= MAX_STREAM_BYTES)
                .ok_or(NativeRemoteError::Invalid(
                    "partial receive exceeds transfer budget",
                ))?;
            files.insert(entry.path.to_string(), file_id.clone());
        }
        let current = self.bootstrap(&base.space_id)?;
        if current.principal_id != base.principal_id
            || current.authorization_epoch != base.authorization_epoch
            || current.current_policy_set_id != base.current_policy_set_id
            || current
                .layers
                .iter()
                .find(|l| l.record.id() == layer)
                .map(|l| &l.head_step_ids)
                != Some(heads)
        {
            return Err(NativeRemoteError::Invalid(
                "partial receive base or authorization changed",
            ));
        }
        store
            .refresh_projection(before.generation, &target, &mut |path, out| {
                let file = files
                    .get(path)
                    .ok_or_else(|| std::io::Error::other("missing verified receive file"))?;
                graph
                    .write_file(file, out)
                    .map(|_| ())
                    .map_err(|e| std::io::Error::other(e.to_string()))
            })
            .map_err(local)
    }
}
