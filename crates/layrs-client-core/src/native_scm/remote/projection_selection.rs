//! Explicit expansion keeps the observed Step and exact authorization unchanged.
use super::*;
use layrs_store_local::projection::VersionedProjection;

impl NativeRemote {
    /// Expand intent and hydrate available selected leaves atomically. If policy
    /// changed, receive/reauthorize first. Existing files/captures are preserved;
    /// pending publication must finish before changing its bound generation.
    pub fn expand_projection_selection(
        &self,
        store: &ProjectionStore,
        expected_generation: u64,
        selection: &ProjectionSelection,
    ) -> RemoteResult<VersionedProjection> {
        if self.origin().trim_end_matches('/') != store.identity().origin {
            return Err(NativeRemoteError::Invalid("projection origin mismatch"));
        }
        if store.pending_publication().map_err(local)?.is_some() {
            return Err(NativeRemoteError::PublicationPending);
        }
        let (before, old_selection) = store.state_with_selection().map_err(local)?;
        if before.generation != expected_generation {
            return Err(NativeRemoteError::Invalid(
                "selection expansion generation changed",
            ));
        }
        let selection = selection
            .normalized(before.snapshot.path_semantics)
            .map_err(local)?;
        if !selection.is_expansion_of(&old_selection) {
            return Err(NativeRemoteError::Invalid(
                "selection shrink is not supported",
            ));
        }
        selection
            .validate_snapshot(&before.snapshot)
            .map_err(local)?;
        let base = self.bootstrap(&store.identity().space_id)?;
        if base.principal_id.as_str() != store.identity().principal_id
            || base.current_policy_set_id != before.snapshot.observed_policy_set_id
            || base.authorization_epoch != before.snapshot.observed_authorization_epoch
        {
            return Err(NativeRemoteError::Invalid(
                "receive current authorization before expanding selection",
            ));
        }
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
        let mut target = before.snapshot.clone();
        let mut graph = GraphObjects::default();
        let mut files = BTreeMap::new();
        let mut total = 0u64;
        for entry in &mut target.entries {
            if !selection.includes(&entry.path)
                || entry.state != EntryState::Unavailable
                || (cfg!(windows) && entry.executable)
            {
                continue;
            }
            let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
                continue;
            };
            let actual = self.fetch_authorized_file(
                &binding,
                &target.base_step_id,
                &entry.path,
                &mut graph,
            )?;
            if &actual != file_id {
                return Err(NativeRemoteError::Invalid(
                    "selection hydration target changed",
                ));
            }
            graph.verify_file(graph.manifest(file_id)?)?;
            total = total
                .checked_add(graph.manifest(file_id)?.byte_length())
                .filter(|n| *n <= MAX_STREAM_BYTES)
                .ok_or(NativeRemoteError::Invalid(
                    "selection hydration exceeds transfer budget",
                ))?;
            files.insert(entry.path.to_string(), file_id.clone());
            entry.state = EntryState::Materialized;
        }
        let current = self.bootstrap(&base.space_id)?;
        if current.principal_id != base.principal_id
            || current.current_policy_set_id != base.current_policy_set_id
            || current.authorization_epoch != base.authorization_epoch
        {
            return Err(NativeRemoteError::Invalid(
                "selection hydration authorization changed",
            ));
        }
        store
            .expand_selection(
                expected_generation,
                &selection,
                &target,
                &mut |path, out| {
                    let file = files
                        .get(path)
                        .ok_or_else(|| std::io::Error::other("missing verified selection file"))?;
                    graph
                        .write_file(file, out)
                        .map(|_| ())
                        .map_err(std::io::Error::other)
                },
            )
            .map_err(local)
    }
}
