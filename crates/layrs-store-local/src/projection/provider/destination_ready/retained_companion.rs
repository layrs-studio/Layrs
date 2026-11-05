//! Autonomous C-local SQL DATA. Construction reads actual retained rows and
//! validates C's own receipt/protection. It never recreates B/native/remote
//! authority and never opens a source path. Unprovable image forms refuse.
use super::*;
use crate::projection::{
    offline,
    provider::{
        checkout_journal::ExpectedCheckout,
        import_validation::VerifiedDestinationPrefix,
        records::{
            companion_v2::{CompanionHeaderRecord, CompanionHeaderV2},
            import::{ProviderCompanionDeletionRecord, ProviderCompanionFileRecord},
        },
        transition_reconcile::{companions::sql_bytes, source_sql},
    },
};
use layrs_core::projection::{EntryState, ProjectionSelection};
use layrs_core::scm::TreeEntryTarget;
use serde::Serialize;

pub(super) struct RetainedCompanionReadyData {
    header: CompanionHeaderV2,
    files: Vec<ProviderCompanionFileRecord>,
    deletions: Vec<ProviderCompanionDeletionRecord>,
    mappings: BTreeMap<FileId, (FileManifestId, u64)>,
    lengths: BTreeMap<FileId, u64>,
    observation: ProjectionSnapshot,
    placement: ProjectionSnapshot,
    selection: ProjectionSelection,
}
impl RetainedCompanionReadyData {
    pub(super) fn read(
        c: &Connection,
        marker: &format::Marker,
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
        Self::read_representations(c, marker, prefix, &rows, expected, cancel)
    }

    /// Authenticate retained SQL DATA using exact triples, without inventing manifest bodies.
    pub(super) fn read_representations(
        c: &Connection,
        marker: &format::Marker,
        prefix: &VerifiedDestinationPrefix<'_>,
        required: &[super::super::managed_admission::RequiredRepresentation],
        expected: &ExpectedCheckout,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        prefix.require_destination_connection(c)?;
        let ready = prefix.ready();
        if ready
            .destination
            .as_ref()
            .is_none_or(|proof| &proof.format != marker)
        {
            return Err(invalid(
                "retained local companion destination FORMAT differs",
            ));
        }
        let plan = &ready.plan;
        let id = plan
            .companion_id
            .as_ref()
            .ok_or_else(|| invalid("retained local companion absent"))?;
        let (header, files, deletions) = source_sql::read_group(c, id, cancel)?;
        let CompanionHeaderRecord::V2(header) = header else {
            return Err(invalid("retained local companion requires v2 provenance"));
        };
        header.require_prefix(prefix.prefix())?;
        header.require_receipt(prefix.receipt())?;
        // This checks actual scalar digests, source effective preimages,
        // original lineage, sticky mappings and C's satisfied absences.
        let ids = prefix.validate_local_companion(c, id, &header, cancel)?;
        let mut mappings = BTreeMap::new();
        for id in ids {
            let (manifest, bytes): (String, u64) = c.query_row(
                "SELECT manifest_id,bytes FROM projection_content WHERE file_id=?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            mappings.insert(
                FileId::parse(id).map_err(|e| invalid(e.to_string()))?,
                (
                    FileManifestId::parse(manifest).map_err(|e| invalid(e.to_string()))?,
                    bytes,
                ),
            );
        }
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
        if !selection.is_expansion_of(prefix.source()?.selection()) {
            return Err(invalid("retained local companion selection shrank"));
        }
        let mut lengths = BTreeMap::new();
        let exact: BTreeMap<_, _> = required
            .iter()
            .map(|m| ((&m.file_id, &m.manifest_id), m.bytes))
            .collect();
        for m in required {
            if lengths
                .insert(m.file_id.clone(), m.bytes)
                .is_some_and(|old| old != m.bytes)
            {
                return Err(invalid("retained local companion FileId lengths differ"));
            }
        }
        for r in &header.preserved_representations {
            if exact.get(&(&r.file_id, &r.manifest_id)) != Some(&r.bytes) {
                return Err(invalid(
                    "retained local companion representation outside C union",
                ));
            }
        }
        for (file, (manifest, bytes)) in &mappings {
            if exact.get(&(file, manifest)) != Some(bytes) {
                return Err(invalid("retained companion sticky mapping outside C union"));
            }
        }
        let result = Self {
            header,
            files,
            deletions,
            mappings,
            lengths,
            observation,
            placement,
            selection,
        };
        result.require_image(prefix.source()?, prefix.prefix(), expected, cancel)?;
        Ok(result)
    }
    pub(super) fn header(&self) -> &CompanionHeaderV2 {
        &self.header
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
    pub(super) fn mapping(&self, file: &FileId) -> Option<&(FileManifestId, u64)> {
        self.mappings.get(file)
    }
    pub(super) fn length(&self, file: &FileId) -> Result<u64> {
        self.lengths
            .get(file)
            .copied()
            .ok_or_else(|| invalid("retained unchanged FileId absent in C union"))
    }
    pub(super) fn metadata_bytes(&self) -> Result<u64> {
        let base = sql_bytes(&self.header.encode()?, &self.files, &self.deletions)?;
        self.mappings
            .iter()
            .try_fold(base, |sum, (file, (manifest, _))| {
                crate::projection::provider::managed_budget::add_metadata(
                    sum,
                    file.as_str().len() as u64 + manifest.as_str().len() as u64 + 8,
                )
            })
    }
    pub(super) fn require_rows(&self, c: &Connection, cancel: Option<&Cancellation>) -> Result<()> {
        let (header, files, deletions) = source_sql::read_group(c, &self.header.digest()?, cancel)?;
        if header.as_ref().encode()? != self.header.encode()?
            || files != self.files
            || deletions != self.deletions
        {
            return Err(invalid("retained C companion exact rows changed"));
        }
        for (file, (manifest, bytes)) in &self.mappings {
            if !c.query_row("SELECT EXISTS(SELECT 1 FROM projection_content WHERE file_id=?1 AND manifest_id=?2 AND bytes=?3)",params![file.as_str(),manifest.as_str(),bytes],|r|r.get::<_,bool>(0))? {
                return Err(invalid("retained C companion mapping changed"));
            }
        }
        Ok(())
    }
    pub(super) fn require_data(
        &self,
        data: &ManagedReadyData<'_>,
        prefix: &ProviderImportPrefixRecord,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        self.require_frozen(
            data.ready,
            data.receipt,
            prefix,
            data.observation,
            data.placement,
            cancel,
        )
    }
    /// Frozen historical variant; no canonical manifests or live Ready DATA are synthesized.
    pub(super) fn require_frozen(
        &self,
        ready: &ProviderTransitionIntentRecord,
        receipt: &ProviderTransitionReceiptRecord,
        prefix: &ProviderImportPrefixRecord,
        observation: &ProjectionSnapshot,
        placement: &ProjectionSnapshot,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        cancellation::check(cancel)?;
        self.header.require_prefix(prefix)?;
        self.header.require_receipt(receipt)?;
        if observation != &self.observation || placement != &self.placement {
            return Err(invalid(
                "retained companion actual observation/placement differs",
            ));
        }
        let mut placement = self.observation.clone();
        for entry in &mut placement.entries {
            if self.selection.includes(&entry.path)
                && entry.state != EntryState::Reserved
                && matches!(entry.target, Some(TreeEntryTarget::File { .. }))
            {
                entry.state = EntryState::Materialized;
            }
        }
        if placement != self.placement {
            return Err(invalid(
                "retained companion placement is not exact materialization",
            ));
        }
        let plan = &ready.plan;
        if plan.companion_id.as_deref() != Some(self.header.digest()?.as_str())
            || plan.companion_files.count != self.header.file_count
            || plan.companion_files.digest != self.header.files_digest
            || plan.companion_deletions.count != self.header.deletion_count
            || plan.companion_deletions.digest != self.header.deletions_digest
        {
            return Err(invalid(
                "retained companion exact rowset commitment differs",
            ));
        }
        Ok(())
    }
    /// Reconstruct only observable inventory DATA and match the original full
    /// image commitment. New remote-only paths are excluded; ambiguous forms
    /// fail the digest instead of being promoted into a native image witness.
    fn require_image(
        &self,
        source: &ProviderBootstrapSource,
        prefix: &ProviderImportPrefixRecord,
        expected: &ExpectedCheckout,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
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
        for row in &self.files {
            current.insert(
                &row.path,
                Item::File {
                    file_id: &row.file_id,
                    length: row.bytes,
                    executable: Some(row.executable),
                },
            );
        }
        let mut deleted: BTreeSet<_> = expected.deletions().iter().collect();
        deleted.extend(self.deletions.iter().map(|r| &r.path));
        deleted.extend(self.header.intents.iter().map(|r| r.path()));
        for path in deleted {
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
            return Err(invalid("retained complete image metadata exceeds bound"));
        }
        let digest =
            super::super::records::codec::hash("layrs-provider-recovery-image-v1", &payload);
        if digest.strip_prefix("blake3:")
            != Some(prefix.source.version().effective_image_digest.as_str())
        {
            return Err(invalid(
                "retained companion cannot prove exact original complete image inventory",
            ));
        }
        Ok(())
    }
}
