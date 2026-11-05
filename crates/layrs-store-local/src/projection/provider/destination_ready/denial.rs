//! Fresh denial observation without a companion group. Live DATA still needs
//! the retained source proof; the autonomous form reads only actual C rows.
use super::*;
use crate::projection::{
    offline,
    provider::{
        checkout_journal::ExpectedCheckout, import_validation::VerifiedDestinationPrefix,
        records::denied_coverage::TransitionDeniedCoverageRecord,
    },
};
use layrs_core::projection::{EntryState, ProjectionSelection};
use layrs_core::scm::TreeEntryTarget;
use serde::Serialize;

fn require_mode(ready: &ProviderTransitionIntentRecord) -> Result<()> {
    ready.validate_fields()?;
    if ready.plan.denial_coverage.is_none()
        || ready.plan.companion_id.is_some()
        || ready.plan.companion_files.count != 0
        || ready.plan.companion_deletions.count != 0
        || !matches!(
            ready.plan.source_prefix.source,
            ImportSource::Provider {
                private_schema: 8,
                ..
            }
        )
    {
        return Err(invalid("plain denial Ready mode differs"));
    }
    Ok(())
}

fn require_observation(
    body: &TransitionDeniedCoverageRecord,
    data: &ManagedReadyData<'_>,
    selected: &ProjectionSelection,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    require_frozen_observation(
        body,
        data.ready,
        data.observation,
        data.placement,
        selected,
        cancel,
    )
}

fn require_frozen_observation(
    body: &TransitionDeniedCoverageRecord,
    ready: &ProviderTransitionIntentRecord,
    observation: &ProjectionSnapshot,
    actual_placement: &ProjectionSnapshot,
    selected: &ProjectionSelection,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    require_mode(ready)?;
    body.require_intent(ready, cancel)?;
    let plan = &ready.plan;
    body.require_observation_rows(
        observation,
        selected,
        plan.observation.generation,
        &plan.observation.snapshot_digest,
        &plan.observation.selection_digest,
        cancel,
    )?;
    let mut placement = observation.clone();
    for entry in &mut placement.entries {
        cancellation::check(cancel)?;
        if selected.includes(&entry.path)
            && entry.state != EntryState::Reserved
            && matches!(entry.target, Some(TreeEntryTarget::File { .. }))
        {
            entry.state = EntryState::Materialized;
        }
    }
    if &placement != actual_placement {
        return Err(invalid("plain denial Ready exact placement differs"));
    }
    Ok(())
}

pub(super) fn require_live(
    mode: &ManagedDenialReadyData<'_>,
    data: &ManagedReadyData<'_>,
    prefix: &ProviderImportPrefixRecord,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    require_observation(
        mode.coverage.record(),
        data,
        mode.observation.selection(),
        cancel,
    )?;
    if mode.source_context.pending.is_some()
        || mode.rows.header().is_some()
        || mode.rows.manifests() != data.required
        || mode.rows.overlay() != data.capture.changes
        || mode.observation.snapshot() != data.observation
    {
        return Err(invalid(
            "plain denial Ready closed rows or source mode differs",
        ));
    }
    mode.rows
        .require_denial_plan_observation(prefix, mode.observation)?;
    let observed = TransitionDeniedCoverageRecord::from_observation(
        prefix,
        &data.ready.transaction_id,
        mode.observation,
        cancel,
    )?;
    if observed.encode()? != mode.coverage.record().encode()? {
        return Err(invalid("plain denial Ready observation body differs"));
    }
    Ok(())
}

pub(super) struct RetainedDenialReadyData {
    body: TransitionDeniedCoverageRecord,
    observation: ProjectionSnapshot,
    placement: ProjectionSnapshot,
    selection: ProjectionSelection,
    lengths: BTreeMap<FileId, u64>,
}
impl RetainedDenialReadyData {
    pub(super) fn read(
        c: &Connection,
        prefix: &VerifiedDestinationPrefix<'_>,
        required: &[FileManifest],
        expected: &ExpectedCheckout,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        let rows: Vec<_> = required
            .iter()
            .map(
                |m| super::super::managed_admission::RequiredRepresentation {
                    file_id: m.file_id().clone(),
                    manifest_id: m.id().clone(),
                    bytes: m.byte_length(),
                },
            )
            .collect();
        Self::read_representations(c, prefix, &rows, expected, cancel)
    }

    /// Authenticate retained SQL DATA using exact triples, without inventing manifest bodies.
    pub(super) fn read_representations(
        c: &Connection,
        prefix: &VerifiedDestinationPrefix<'_>,
        required: &[super::super::managed_admission::RequiredRepresentation],
        expected: &ExpectedCheckout,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        prefix.require_destination_connection(c)?;
        let ready = prefix.ready();
        require_mode(ready)?;
        let body = super::super::destination_coverage::read_current(c, ready, cancel)?
            .ok_or_else(|| invalid("plain denial Ready current body absent"))?;
        let plan = &ready.plan;
        let observation = offline::snapshot_at(
            c,
            plan.observation.generation,
            Some(&plan.observation.snapshot_digest),
        )?
        .snapshot;
        let placement =
            offline::snapshot_at(c, plan.placement_generation, Some(&plan.placement_digest))?
                .snapshot;
        let selection = selection::at(c, plan.observation.generation)?;
        if selection::at(c, plan.placement_generation)? != selection
            || !selection.is_expansion_of(prefix.source()?.selection())
        {
            return Err(invalid("plain denial Ready retained selection differs"));
        }
        let mut lengths = BTreeMap::new();
        for manifest in required {
            cancellation::check(cancel)?;
            if lengths
                .insert(manifest.file_id.clone(), manifest.bytes)
                .is_some_and(|old| old != manifest.bytes)
            {
                return Err(invalid("plain denial Ready FileId lengths differ"));
            }
        }
        let result = Self {
            body,
            observation,
            placement,
            selection,
            lengths,
        };
        result.require_image(prefix.source()?, prefix.prefix(), expected, cancel)?;
        Ok(result)
    }
    pub(super) fn selection(&self) -> &ProjectionSelection {
        &self.selection
    }
    pub(super) fn observation(&self) -> &ProjectionSnapshot {
        &self.observation
    }
    pub(super) fn placement(&self) -> &ProjectionSnapshot {
        &self.placement
    }
    pub(super) fn length(&self, file: &FileId) -> Result<u64> {
        self.lengths
            .get(file)
            .copied()
            .ok_or_else(|| invalid("plain denial Ready unchanged FileId absent in union"))
    }
    pub(super) fn require_data(
        &self,
        data: &ManagedReadyData<'_>,
        prefix: &ProviderImportPrefixRecord,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        if data.observation != &self.observation
            || data.placement != &self.placement
            || &data.ready.plan.source_prefix != prefix
        {
            return Err(invalid("plain denial Ready actual snapshots differ"));
        }
        require_observation(&self.body, data, &self.selection, cancel)
    }
    pub(super) fn require_frozen(
        &self,
        ready: &ProviderTransitionIntentRecord,
        observation: &ProjectionSnapshot,
        placement: &ProjectionSnapshot,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        if observation != &self.observation || placement != &self.placement {
            return Err(invalid("plain denial frozen snapshots differ"));
        }
        require_frozen_observation(
            &self.body,
            ready,
            observation,
            placement,
            &self.selection,
            cancel,
        )
    }
    fn require_image(
        &self,
        source: &ProviderBootstrapSource,
        prefix: &ProviderImportPrefixRecord,
        expected: &ExpectedCheckout,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        // Rebuild only the original selected B namespace. New observed paths
        // are not evidence of B bytes; a hidden original file makes this digest
        // fail rather than being invented from the denial document.
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        enum Item<'a> {
            File {
                file_id: &'a FileId,
                length: u64,
                executable: Option<bool>,
            },
            Deleted {
                base: &'a FileId,
            },
        }
        let mut current = BTreeMap::new();
        for file in expected.files() {
            cancellation::check(cancel)?;
            if source
                .initial()
                .entries
                .iter()
                .any(|e| e.path == file.path && source.selection().includes(&e.path))
            {
                current.insert(
                    &file.path,
                    Item::File {
                        file_id: &file.file_id,
                        length: file.bytes,
                        executable: Some(file.executable),
                    },
                );
            }
        }
        for path in expected.deletions() {
            cancellation::check(cancel)?;
            current.remove(path);
            if let Some(base) = source
                .initial()
                .entries
                .iter()
                .find(|e| &e.path == path)
                .and_then(|e| match &e.target {
                    Some(TreeEntryTarget::File { file_id }) => Some(file_id),
                    _ => None,
                })
            {
                current.insert(path, Item::Deleted { base });
            }
        }
        let payload = canonical_json(&current).map_err(|e| invalid(e.to_string()))?;
        if payload.len() > 32 * 1024 * 1024 {
            return Err(invalid("plain denial Ready image metadata exceeds bound"));
        }
        let digest =
            super::super::records::codec::hash("layrs-provider-recovery-image-v1", &payload);
        if digest.strip_prefix("blake3:")
            != Some(prefix.source.version().effective_image_digest.as_str())
        {
            return Err(invalid("plain denial Ready original image differs"));
        }
        Ok(())
    }
}
