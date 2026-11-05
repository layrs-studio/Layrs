use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectionHistoricalSelection {
    pub step_id: StepId,
    /// Empty selects every currently writable regular leaf, including observed absences.
    pub paths: Vec<RepoPath>,
}
impl NativeRemote {
    /// Restore authorized historical values as a new local capture; keep the
    /// current remote base. Empty paths selects the complete writable projection.
    pub fn preview_projection_historical_restore(
        &self,
        store: &ProjectionStore,
        sequence: u64,
        layer: &LayerId,
        selection: &ProjectionHistoricalSelection,
    ) -> RemoteResult<NativeProjectionRebasePreview> {
        self.preview_projection_rebase_kind(store, sequence, layer, Some(selection))
    }
    pub(super) fn historical_entries(
        &self,
        base: &SpaceBootstrap,
        target: &ProjectionSnapshot,
        selection: &ProjectionHistoricalSelection,
    ) -> RemoteResult<BTreeMap<RepoPath, ProjectionEntry>> {
        if selection.paths.len() > 1024 || selection.paths.windows(2).any(|p| p[0] >= p[1]) {
            return Err(NativeRemoteError::Invalid(
                "historical restoration needs sorted unique paths, at most1024",
            ));
        }
        let eligible: BTreeSet<_> = target
            .entries
            .iter()
            .filter(|e| {
                e.writable && matches!(e.state, EntryState::Materialized | EntryState::Absent)
            })
            .map(|e| e.path.clone())
            .collect();
        let paths = if selection.paths.is_empty() {
            eligible.iter().cloned().collect::<Vec<_>>()
        } else {
            selection.paths.clone()
        };
        if paths.is_empty() || paths.len() > 1024 || paths.iter().any(|p| !eligible.contains(p)) {
            return Err(NativeRemoteError::Invalid(
                "historical restoration targets only currently writable regular leaves",
            ));
        }
        let mut entries = BTreeMap::new();
        let mut historical_root = None;
        for chunk in paths.chunks(200) {
            let access = self.projection_access(
                &base.space_id,
                &ProjectionAccessRequest {
                    step_id: selection.step_id.clone(),
                    expected_policy_set_id: base.current_policy_set_id.clone(),
                    expected_authorization_epoch: base.authorization_epoch,
                    paths: chunk.to_vec(),
                },
            )?;
            if access.principal_id != base.principal_id
                || historical_root
                    .as_ref()
                    .is_some_and(|id| id != &access.root_tree_id)
            {
                return Err(NativeRemoteError::Invalid(
                    "historical authorization correlation changed",
                ));
            }
            historical_root = Some(access.root_tree_id);
            for e in access.entries {
                if !e.content_readable
                    || (cfg!(windows) && e.executable)
                    || !matches!(e.target.0, None | Some(TreeEntryTarget::File { .. }))
                {
                    return Err(NativeRemoteError::Invalid(
                        "historical value is protected or unsupported",
                    ));
                }
                // Writable describes the current destination, checked above.
                let entry = ProjectionEntry {
                    path: e.path,
                    target: e.target.0.clone(),
                    executable: e.executable,
                    writable: true,
                    state: if e.target.is_some() {
                        EntryState::Materialized
                    } else {
                        EntryState::Absent
                    },
                };
                entries.insert(entry.path.clone(), entry);
            }
        }
        Ok(entries)
    }
}
