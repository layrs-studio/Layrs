use super::*;
use crate::Result;
use layrs_client_core::native_scm::{MutationStamp, NativeWorkspace};
use layrs_store::scm::*;
use layrs_store_local::scm_v1::NativeScmReader;

#[path = "install_replay.rs"]
mod replay;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    pub import_id: String,
    pub already_imported: bool,
    pub operation_id: OperationId,
    pub layer_ids: Vec<LayerId>,
    pub marker_ids: Vec<MarkerId>,
}
pub(crate) fn import_operation_id(id: &str, space: &SpaceId) -> Result<OperationId> {
    OperationId::parse(format!(
        "operation:git-import-{}",
        named_id(space.as_str(), id)
    ))
    .map_err(invalid)
}
fn named_id(import: &str, name: &str) -> String {
    blake3::hash(format!("{import}\0{name}").as_bytes())
        .to_hex()
        .to_string()
}
pub(crate) fn report_metadata(report: &ImportReport) -> Result<Metadata> {
    let text = serde_json::to_string(report).map_err(invalid)?;
    let mut entries = vec![MetadataEntry::new("git.importId", &report.import_id).map_err(invalid)?];
    let chars = text.chars().collect::<Vec<_>>();
    for (index, chunk) in chars.chunks(12000).enumerate() {
        entries.push(
            MetadataEntry::new(
                format!("git.report.{index:06}"),
                chunk.iter().collect::<String>(),
            )
            .map_err(invalid)?,
        );
    }
    Metadata::new(entries).map_err(invalid)
}
pub(crate) fn read_report(metadata: &Metadata) -> Result<ImportReport> {
    let text = metadata
        .entries()
        .iter()
        .filter(|entry| entry.key().starts_with("git.report."))
        .map(MetadataEntry::value)
        .collect::<String>();
    let report: ImportReport = serde_json::from_str(&text).map_err(invalid)?;
    if metadata_value(metadata, "git.importId") != Some(report.import_id.as_str()) {
        return Err(invalid("Git provenance import ID mismatch"));
    }
    let mut projection = report.clone();
    projection.import_id.clear();
    if blake3::hash(&canonical_json(&projection).map_err(invalid)?)
        .to_hex()
        .as_str()
        != report.import_id
    {
        return Err(invalid("Git provenance report digest mismatch"));
    }
    Ok(report)
}
impl ImportPlan {
    /// Installs one complete import atomically. The source is never changed;
    /// checkout state remains at its current snapshot. Stable import identity
    /// makes retries/restarts return AlreadyImported without duplicate refs.
    pub fn install(&self, workspace: &NativeWorkspace) -> Result<ImportOutcome> {
        self.install_expected(workspace, &ExpectedBridgeState::capture(workspace)?)
    }

    /// Uses the reviewed state to prepare every CAS expectation, never a fresh base.
    pub fn install_expected(
        &self,
        workspace: &NativeWorkspace,
        expected: &ExpectedBridgeState,
    ) -> Result<ImportOutcome> {
        self.install_expected_with(workspace, expected, |batch| {
            expected.apply_batch(workspace, batch)
        })
    }

    // A single writer boundary also lets fault recipes exercise the exact
    // production batch, including its immutable transaction receipt.
    #[cfg(test)]
    pub(crate) fn install_with(
        &self,
        workspace: &NativeWorkspace,
        apply: impl FnOnce(&WriteBatch) -> Result<ApplyBatchOutcome>,
    ) -> Result<ImportOutcome> {
        self.install_expected_with(workspace, &ExpectedBridgeState::capture(workspace)?, apply)
    }

    pub(crate) fn install_expected_with(
        &self,
        workspace: &NativeWorkspace,
        expected: &ExpectedBridgeState,
        apply: impl FnOnce(&WriteBatch) -> Result<ApplyBatchOutcome>,
    ) -> Result<ImportOutcome> {
        expected.validate(workspace)?;
        let state = expected.state();
        let operation_id = import_operation_id(&self.report.import_id, state.space.id())?;
        let scope = format!("{}:{}", state.space.id(), self.report.import_id);
        let store = workspace.open_store().map_err(invalid)?;
        let reader = NativeScmReader::new(&store).map_err(invalid)?;
        let mut outcome = ImportOutcome {
            import_id: self.report.import_id.clone(),
            already_imported: false,
            operation_id: operation_id.clone(),
            layer_ids: vec![],
            marker_ids: vec![],
        };
        for reference in &self.report.references {
            let id = named_id(&scope, &reference.git_ref);
            if reference.git_ref.starts_with("refs/heads/") {
                outcome
                    .layer_ids
                    .push(LayerId::parse(format!("layer:git-{id}")).map_err(invalid)?);
            } else {
                outcome
                    .marker_ids
                    .push(MarkerId::parse(format!("marker:git-{id}")).map_err(invalid)?);
            }
        }
        if let Some(existing) = reader.get_operation(&operation_id).map_err(invalid)? {
            replay::validate(&reader, &existing, state.space.id(), &scope, &self.report)?;
            outcome.already_imported = true;
            return Ok(outcome);
        }
        let current = &expected.repository_view;
        let op_heads = &expected.operation_heads;
        // The shared CAS frontier may contain several independent Worktrees.
        // This import has only its own current Operation as a parent.
        let mut next_heads: std::collections::BTreeSet<_> = op_heads
            .iter()
            .filter(|id| *id != state.operation.id())
            .cloned()
            .collect();
        next_heads.insert(operation_id.clone());
        let stamp = MutationStamp::fresh().map_err(invalid)?;
        let mut records = vec![];
        let mut layer_heads = state.view.layer_heads().to_vec();
        let mut marker_ids = state.view.marker_ids().to_vec();
        for reference in &self.report.references {
            let suffix = reference
                .git_ref
                .strip_prefix("refs/heads/")
                .or_else(|| reference.git_ref.strip_prefix("refs/tags/"))
                .ok_or_else(|| invalid("unsupported import reference"))?;
            let name = format!("git/{suffix}");
            let id = named_id(&scope, &reference.git_ref);
            let metadata = Metadata::new(vec![
                MetadataEntry::new("git.importId", &self.report.import_id).map_err(invalid)?,
                MetadataEntry::new("git.ref", &reference.git_ref).map_err(invalid)?,
                MetadataEntry::new("git.commitId", &reference.git_id).map_err(invalid)?,
            ])
            .map_err(invalid)?;
            if reference.git_ref.starts_with("refs/heads/") {
                if state.layers.iter().any(|layer| layer.name() == name) {
                    return Err(invalid(format!("native Layer name collision: {name}")));
                }
                let layer = Layer::new(
                    LayerId::parse(format!("layer:git-{id}")).map_err(invalid)?,
                    state.space.id().clone(),
                    name,
                    LayerKind::Exploration,
                    LayerStatus::Active,
                    vec![],
                    LayerSettings::new(true, 500).map_err(invalid)?,
                    stamp.timestamp.clone(),
                    metadata,
                )
                .map_err(invalid)?;
                layer_heads.push(
                    LayerHeadSet::new(layer.id().clone(), vec![reference.step_id.clone()])
                        .map_err(invalid)?,
                );
                records.push(EntityRecord::Layer(layer));
            } else {
                for id in state.view.marker_ids() {
                    if reader
                        .get_marker(id)
                        .map_err(invalid)?
                        .is_some_and(|marker| marker.name() == name)
                    {
                        return Err(invalid(format!("native Marker name collision: {name}")));
                    }
                }
                let marker = Marker::new(
                    MarkerId::parse(format!("marker:git-{id}")).map_err(invalid)?,
                    state.space.id().clone(),
                    reference.step_id.clone(),
                    MarkerKind::Checkpoint,
                    &name,
                    &name,
                    "Imported Git tag; exact annotation retained in import provenance",
                    None,
                    stamp.timestamp.clone(),
                    vec![],
                    metadata,
                )
                .map_err(invalid)?;
                marker_ids.push(marker.id().clone());
                records.push(EntityRecord::Marker(marker));
            }
        }
        let view = RepositoryView::new(
            state.space.id().clone(),
            layer_heads,
            marker_ids,
            state.view.release_ids().to_vec(),
            state.view.worktrees().to_vec(),
        )
        .map_err(invalid)?;
        let operation = Operation::new(
            operation_id.clone(),
            state.space.id().clone(),
            vec![state.operation.id().clone()],
            view.id().clone(),
            OperationKind::ReceiveRemoteState,
            "Import Git history",
            vec![],
            None,
            None,
            stamp.timestamp.clone(),
            report_metadata(&self.report)?,
        )
        .map_err(invalid)?;
        records.push(EntityRecord::Operation(operation));
        for commit in &self.report.commits {
            let id = named_id(&scope, &commit.git_id);
            records.push(EntityRecord::StepObservation(StepObservation::new(
                ObservationId::parse(format!("observation:git-{id}")).map_err(invalid)?,
                commit.step_id.clone(),
                Some(state.space.id().clone()),
                Some(operation_id.clone()),
                None,
                None,
                None,
                stamp.timestamp.clone(),
                CaptureReason::Received,
                Metadata::new(vec![
                    MetadataEntry::new("git.commitId", &commit.git_id).map_err(invalid)?,
                    MetadataEntry::new("git.importId", &self.report.import_id).map_err(invalid)?,
                ])
                .map_err(invalid)?,
            )));
        }
        let mut objects = self.objects.clone();
        objects.push(CanonicalObject::RepositoryView(view.clone()));
        let admissions = objects
            .iter()
            .map(|object| SpaceObjectAdmission::new(state.space.id().clone(), object.object_id()))
            .collect();
        let batch = WriteBatch::new(
            TransactionId::new(format!(
                "transaction:git-import-{}",
                named_id(state.space.id().as_str(), &self.report.import_id)
            ))
            .map_err(invalid)?,
            objects,
            records,
            admissions,
            vec![
                StoreMutation::RepositoryView(
                    RepositoryViewCas::new(
                        state.space.id().clone(),
                        current.generation(),
                        current.view_id().cloned(),
                        view.id().clone(),
                    )
                    .map_err(invalid)?,
                ),
                StoreMutation::OperationHeads(
                    OperationHeadsCas::new(
                        state.space.id().clone(),
                        op_heads.clone(),
                        operation_id.clone(),
                        next_heads.into_iter().collect(),
                    )
                    .map_err(invalid)?,
                ),
                StoreMutation::WorktreeActiveOperation(
                    WorktreeActiveOperationCas::new(
                        state.worktree.id().clone(),
                        state.operation.id().clone(),
                        operation_id,
                    )
                    .map_err(invalid)?,
                ),
            ],
        )
        .map_err(invalid)?;
        drop(reader);
        drop(store);
        apply(&batch)?;
        Ok(outcome)
    }
}
