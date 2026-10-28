//! Owned ordinary source for managed export. SQL DATA is reauthenticated inside
//! each retained snapshot; no ContentScope or transaction escapes its callback.
//! Dedicated child adapters retain allocation pins and journal Copying; content
//! transfer, destination readiness and autonomous admission remain separate.
pub(super) mod allocation;
mod budget;
mod checkout;
mod content;
mod copy;
mod destination;
mod image;
mod installing;
mod preallocation;
mod protection;
mod reservation;
mod reserve_plan;
mod resident;
mod staging;
#[cfg(test)]
pub(super) mod verification_cancellation;

use super::{
    bootstrap::{self, ImageDeletion, ImageFile, ManagedPlanData, RequiredManifest},
    import_validation::{plan_admitted_import, VerifiedImportClosure},
    managed_admission::read_admitted_import,
    records::{
        import::ProviderImportPrefixRecord,
        transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
    },
    recovery::error::ProviderBootstrapCause,
};
use crate::projection::{
    capture::MAX_CAPTURE_FILE_BYTES, database, format, invalid, selection, ProjectionIdentity,
    ProjectionStore, ProjectionStoreError, Result, SessionMode,
};
use crate::secure_fs::PinnedDirectory;
use image::OrdinaryExportImage;
use layrs_core::scm::{
    canonical_json, FileId, FileManifest, FileManifestId, RepoPath, TreeEntryTarget,
};
use layrs_vfs::Cancellation;
use rusqlite::{Connection, Transaction, TransactionBehavior};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
    path::Path,
};

type Mappings = BTreeMap<FileId, (FileManifestId, u64)>;

/// An exclusive ordinary owner and immutable current-file handles. External
/// writers must remain quiescent: Windows file sharing does not freeze absent
/// names or every namespace mutation. Every transfer rechecks the complete image.
///
/// Copies may write a prefix to their sink on failure. The caller must keep that
/// output unpublished, and explicitly close the holder before acknowledging a
/// terminal operation. Dropping it is not a successful close acknowledgement.
#[must_use = "close the managed export owner to observe private database finalization"]
pub struct OrdinaryManagedExport<'s> {
    store: &'s ProjectionStore,
    access: Option<format::Access>,
    image: OrdinaryExportImage,
    prefix: ProviderImportPrefixRecord,
    mappings: Mappings,
    retained: BTreeMap<FileId, FileManifest>,
}

impl ProjectionStore {
    /// Open an admitted source for explicit managed-export retry/cancellation.
    /// Ordinary mutations remain fenced while its outgoing intent is live.
    /// Discovery is shared/read-only and does not settle an ordinary budget.
    pub fn open_for_managed_export(root: &Path) -> Result<Self> {
        let root = std::path::absolute(root)?;
        let marker = format::read_marker(&root)?;
        let store = Self { root, marker };
        format::access_for_managed_export_retry(&store, SessionMode::Shared)?.run(|access| {
            access.with_content(|c, scope| {
                let tx = c.transaction()?;
                let result = (|| {
                    read_admitted_import(&tx, &store.marker, None)?;
                    let state = database::current(&tx)?;
                    if state.snapshot.path_semantics
                        != layrs_core::projection::PathSemantics::PortableWindows
                    {
                        return Err(invalid(
                            "Windows export requires portable Windows path semantics",
                        ));
                    }
                    scope.verify(&tx, &store)
                })();
                finish_snapshot(tx, result)
            })
        })?;
        Ok(store)
    }

    /// Cancel only a Planned export that has not allocated destination resources.
    /// This preserves external checkout edits and leaves a terminal journal row.
    /// Later transfer phases require their own resource-aware recovery driver.
    pub fn cancel_managed_export_plan(
        &self,
        transaction_id: &str,
        cancel: &Cancellation,
    ) -> Result<()> {
        check_cancel(cancel)?;
        format::access_for_managed_export_retry(self, SessionMode::Exclusive)?.run(|access| {
            access.with_content(|c, scope| {
                let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let mut absence = None;
                let result = (|| {
                    scope.validate(&tx, self)?;
                    let admitted = read_admitted_import(&tx, &self.marker, Some(cancel))?;
                    if let Some(planned) = admitted.outgoing().iter().find(|row| {
                        row.transaction_id == transaction_id
                            && row.state == TransitionState::Planned
                    }) {
                        absence = Some(preallocation::Absence::open(
                            &tx,
                            &self.marker,
                            planned,
                            cancel,
                        )?);
                    }
                    super::ordinary_transition::cancel_planned(
                        &tx,
                        &self.marker,
                        transaction_id,
                        Some(cancel),
                    )?;
                    scope.verify(&tx, self)?;
                    if let Some(absence) = &absence {
                        absence.verify(cancel)?;
                    }
                    check_cancel(cancel)
                })();
                let result = finish_writer(tx, result);
                drop(absence);
                result
            })
        })
    }

    /// Retain a genuine admitted ordinary source and its selected physical
    /// image for export to the named destination identity. No destination
    /// resources or remote rights are created by this operation.
    pub fn begin_managed_export(
        &self,
        expected_generation: u64,
        destination: ProjectionIdentity,
        cancel: &Cancellation,
    ) -> Result<OrdinaryManagedExport<'_>> {
        check_cancel(cancel)?;
        format::validate_identity(&destination)?;
        if destination.worktree_id == self.marker.identity.worktree_id
            || destination.store_id != self.marker.identity.store_id
            || destination.space_id != self.marker.identity.space_id
            || destination.origin != self.marker.identity.origin
            || destination.principal_id != self.marker.identity.principal_id
        {
            return Err(invalid(
                "ordinary export destination identity is outside source context",
            ));
        }
        let mut access = format::access(self, SessionMode::Exclusive)?;
        let built = access.with_content(|c, scope| {
            let tx = c.transaction()?;
            let result = (|| {
                // Refuse non-admitted backends before reading checkout bytes.
                read_admitted_import(&tx, &self.marker, Some(cancel))?;
                let state = database::current(&tx)?;
                if state.generation != expected_generation {
                    return Err(ProjectionStoreError::Stale {
                        expected: expected_generation,
                        actual: state.generation,
                    });
                }
                let mut image =
                    OrdinaryExportImage::build(self, &tx, scope, &state.snapshot, cancel)?;
                let (prefix, proof) = plan_admitted_import(
                    &tx,
                    &self.marker,
                    destination,
                    image.digest(),
                    Some(cancel),
                )?;
                let mappings = proof.copied_content_mappings(&tx, &prefix, Some(cancel))?;
                let retained_mappings = include_selected_bases(&tx, &mappings, cancel)?;
                let retained = read_manifests(self, &tx, scope, &retained_mappings, cancel)?;
                image.revalidate(self, &tx, scope, &state.snapshot, cancel)?;
                scope.verify(&tx, self)?;
                Ok((image, prefix, mappings, retained))
            })();
            finish_snapshot(tx, result)
        });
        match built {
            Ok((image, prefix, mappings, retained)) => Ok(OrdinaryManagedExport {
                store: self,
                access: Some(access),
                image,
                prefix,
                mappings,
                retained,
            }),
            Err(error) => access.run(|_| Err(error)),
        }
    }
}

impl OrdinaryManagedExport<'_> {
    /// Persist the source plan under its real owner and physical image. No
    /// destination allocation or reservation is performed by this step.
    pub(in crate::projection::provider) fn begin_plan(
        &mut self,
        parent: &Path,
        leaf_nonce: &str,
        transaction_id: &str,
        cancel: &Cancellation,
    ) -> Result<(
        ProviderTransitionIntentRecord,
        super::transition_sql::SourceIntentEffect,
    )> {
        check_cancel(cancel)?;
        let parent = std::path::absolute(parent)?;
        let parent_pin = PinnedDirectory::open(&parent)?;
        let data = self.derive_plan(
            parent
                .to_str()
                .ok_or_else(|| invalid("export parent is not UTF-8"))?,
            &parent_pin.identity_token(),
            leaf_nonce,
            transaction_id,
            cancel,
        )?;
        let next = ProviderTransitionIntentRecord {
            schema_version: "layrs.projection-provider-transition-intent.v1".into(),
            transaction_id: transaction_id.into(),
            role: TransitionRole::Source,
            state: TransitionState::Planned,
            local_generation: self.source_generation(),
            plan: data.plan,
            destination: None,
            reservation: None,
            receipt_id: None,
        };
        let store = self.store;
        let effect = self.with_source_transaction(cancel, true, |c, _, prefix, _, _| {
            parent_pin.verify_path_identity()?;
            let effect = super::ordinary_transition::apply_planned(
                c,
                &store.marker,
                prefix,
                &next,
                Some(cancel),
            )?;
            parent_pin.verify_path_identity()?;
            Ok(effect)
        })?;
        Ok((next, effect))
    }

    /// Detached descriptive DATA. Transfer methods revalidate the live owner.
    pub fn source_generation(&self) -> u64 {
        self.prefix.source.version().generation
    }
    pub fn image_digest(&self) -> &str {
        self.image.digest()
    }
    pub fn current_manifest(&self, path: &RepoPath) -> Option<&FileManifest> {
        self.image
            .files()
            .iter()
            .find(|file| &file.path == path)
            .map(|file| &file.manifest)
    }
    pub fn retained_manifest(&self, file_id: &FileId) -> Option<&FileManifest> {
        self.retained.get(file_id)
    }

    pub fn revalidate(&mut self, cancel: &Cancellation) -> Result<()> {
        self.with_source(cancel, |_, _, _, _, _| Ok(()))
    }

    /// Copy only a selected current file, verifying its full FileId and chunks.
    pub fn copy_current_file(
        &mut self,
        path: &RepoPath,
        manifest: &FileManifest,
        output: &mut dyn Write,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.with_source(cancel, |_, _, _, _, image| {
            image.copy_file(path, manifest, output, cancel)
        })
    }

    /// Verify one exact current-image chunk. Whole-file assembly still requires
    /// the final FileId check before a destination may publish it.
    pub fn copy_current_chunk(
        &mut self,
        path: &RepoPath,
        manifest: &FileManifest,
        ordinal: usize,
        output: &mut dyn Write,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.with_source(cancel, |_, _, _, _, image| {
            image.copy_chunk(path, manifest, ordinal, output, cancel)
        })
    }

    /// Copy an exact representation retained by authenticated history or a
    /// selected snapshot base, including restore-only historical preimages.
    pub fn copy_retained_file(
        &mut self,
        manifest: &FileManifest,
        output: &mut dyn Write,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.require_retained(manifest)?;
        let store = self.store;
        self.with_source(cancel, |c, scope, _, _, _| {
            let family = format::content_family(c, store, Some(scope))?;
            let lease = family
                .as_ref()
                .ok_or_else(|| invalid("ordinary export lost content lease"))?;
            require_private_mapping(c, manifest)?;
            check_cancel(cancel)?;
            lease.stream(manifest, &mut CancelledSink { output, cancel })?;
            check_cancel(cancel)
        })
    }

    pub fn copy_retained_chunk(
        &mut self,
        manifest: &FileManifest,
        ordinal: usize,
        output: &mut dyn Write,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.require_retained(manifest)?;
        let store = self.store;
        manifest
            .chunks()
            .get(ordinal)
            .ok_or_else(|| invalid("ordinary export historical chunk absent"))?;
        self.with_source(cancel, |c, scope, _, _, _| {
            let family = format::content_family(c, store, Some(scope))?;
            let lease = family
                .as_ref()
                .ok_or_else(|| invalid("ordinary export lost content lease"))?;
            require_private_mapping(c, manifest)?;
            check_cancel(cancel)?;
            lease.stream_chunk(manifest, ordinal, &mut CancelledSink { output, cancel })?;
            check_cancel(cancel)
        })
    }

    fn require_retained(&self, manifest: &FileManifest) -> Result<()> {
        if self.retained.get(manifest.file_id()) != Some(manifest) {
            return Err(invalid(
                "ordinary export representation is outside exact history and selected bases",
            ));
        }
        Ok(())
    }

    /// Fund the complete future metadata envelope before persisting Planned.
    /// Later reservation writers must still rederive the complete source/image
    /// plan and actual Staging floor; validated_metadata_floor remains None.
    pub(in crate::projection::provider) fn derive_plan(
        &mut self,
        parent: &str,
        parent_identity: &str,
        leaf_nonce: &str,
        transaction_id: &str,
        cancel: &Cancellation,
    ) -> Result<ManagedPlanData> {
        let store = self.store;
        self.with_source(cancel, |c, scope, prefix, proof, image| {
            let current = database::current(c)?;
            let selected = selection::at(c, current.generation)?;
            let mappings = proof.copied_content_mappings(c, prefix, Some(cancel))?;
            let retained_mappings = include_selected_bases(c, &mappings, cancel)?;
            let retained = read_manifests(store, c, scope, &retained_mappings, cancel)?;
            let base_ids: BTreeSet<_> = current
                .snapshot
                .entries
                .iter()
                .filter(|entry| selected.includes(&entry.path))
                .filter_map(|entry| match &entry.target {
                    Some(TreeEntryTarget::File { file_id }) => Some(file_id.clone()),
                    _ => None,
                })
                .collect();
            let bases = base_ids
                .iter()
                .map(|id| {
                    retained.get(id).ok_or_else(|| {
                        invalid("ordinary export base lacks authenticated private representation")
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let historical = mappings
                .iter()
                .map(|(id, (manifest, bytes))| RequiredManifest {
                    file_id: id.clone(),
                    manifest_id: manifest.clone(),
                    bytes: *bytes,
                })
                .collect::<Vec<_>>();
            let files = image
                .files()
                .iter()
                .map(|file| ImageFile {
                    path: &file.path,
                    manifest: &file.manifest,
                    executable: file.executable,
                    expected: file.expected.clone(),
                })
                .collect::<Vec<_>>();
            let deletions = image
                .deletions()
                .iter()
                .map(|file| ImageDeletion {
                    path: &file.path,
                    base: &file.base,
                    executable: file.executable,
                })
                .collect::<Vec<_>>();
            let mut union = BTreeMap::new();
            for manifest in retained
                .values()
                .chain(image.files().iter().map(|file| &file.manifest))
            {
                union.insert(
                    (manifest.file_id().clone(), manifest.id().clone()),
                    manifest.clone(),
                );
            }
            let supplied = union.into_values().collect::<Vec<_>>();
            let derive = |metadata| {
                bootstrap::derive_ordinary_data(
                    prefix,
                    &current.snapshot,
                    &selected,
                    &prefix.source.version().selection_digest,
                    &bases,
                    &files,
                    &deletions,
                    prefix.destination_identity.clone(),
                    parent,
                    parent_identity,
                    leaf_nonce,
                    transaction_id,
                    &historical,
                    &supplied,
                    metadata,
                )
            };
            let baseline = derive(0)?;
            let envelope = budget::planning_envelope(
                &baseline,
                &selected,
                &supplied,
                &files,
                &deletions,
                proof.metadata_bytes(c, prefix, Some(cancel))?,
                cancel,
            )?;
            Ok(derive(envelope.metadata_bytes())?)
        })
    }

    fn with_source<T>(
        &mut self,
        cancel: &Cancellation,
        action: impl FnOnce(
            &Connection,
            &format::ContentScope<'_>,
            &ProviderImportPrefixRecord,
            &VerifiedImportClosure,
            &mut OrdinaryExportImage,
        ) -> Result<T>,
    ) -> Result<T> {
        self.with_source_transaction(cancel, false, action)
    }

    fn with_source_transaction<T>(
        &mut self,
        cancel: &Cancellation,
        write: bool,
        action: impl FnOnce(
            &Connection,
            &format::ContentScope<'_>,
            &ProviderImportPrefixRecord,
            &VerifiedImportClosure,
            &mut OrdinaryExportImage,
        ) -> Result<T>,
    ) -> Result<T> {
        check_cancel(cancel)?;
        let Self {
            store,
            access,
            image,
            prefix,
            mappings,
            retained,
        } = self;
        let access = access
            .as_mut()
            .ok_or_else(|| invalid("ordinary export owner already finalized"))?;
        access.with_content(|c, scope| {
            let tx = c.transaction_with_behavior(if write {
                TransactionBehavior::Immediate
            } else {
                TransactionBehavior::Deferred
            })?;
            let result = (|| {
                let state = database::current(&tx)?;
                image.revalidate(store, &tx, scope, &state.snapshot, cancel)?;
                let (fresh, proof) = plan_admitted_import(
                    &tx,
                    &store.marker,
                    prefix.destination_identity.clone(),
                    image.digest(),
                    Some(cancel),
                )?;
                if fresh != *prefix {
                    return Err(invalid("ordinary export source history changed"));
                }
                let fresh_mappings = proof.copied_content_mappings(&tx, &fresh, Some(cancel))?;
                if fresh_mappings != *mappings
                    || read_manifests(
                        store,
                        &tx,
                        scope,
                        &include_selected_bases(&tx, &fresh_mappings, cancel)?,
                        cancel,
                    )? != *retained
                {
                    return Err(invalid("ordinary export retained mappings changed"));
                }
                let value = action(&tx, scope, &fresh, &proof, image)?;
                image.revalidate(store, &tx, scope, &state.snapshot, cancel)?;
                scope.verify(&tx, store)?;
                check_cancel(cancel)?;
                Ok(value)
            })();
            if write {
                finish_writer(tx, result)
            } else {
                finish_snapshot(tx, result)
            }
        })
    }

    pub fn close(self) -> Result<()> {
        self.finish(Ok(()))
    }

    pub(in crate::projection::provider) fn finish<T>(mut self, result: Result<T>) -> Result<T> {
        let access = self
            .access
            .take()
            .ok_or_else(|| invalid("ordinary export owner already finalized"))?;
        // Keep self.image and all file pins alive until observable SQL close.
        access.run(|_| result)
    }
}

/// The historical rowset stays exact. Selected snapshot bases supply separate
/// transfer requirements, authenticated against the private content mapping.
fn include_selected_bases(
    c: &Connection,
    historical: &Mappings,
    cancel: &Cancellation,
) -> Result<Mappings> {
    let current = database::current(c)?;
    let selected = selection::at(c, current.generation)?;
    selected
        .validate_snapshot(&current.snapshot)
        .map_err(invalid)?;
    let mut mappings = historical.clone();
    for entry in &current.snapshot.entries {
        check_cancel(cancel)?;
        if !selected.includes(&entry.path) {
            continue;
        }
        if let Some(TreeEntryTarget::File { file_id }) = &entry.target {
            let mapping = private_mapping(c, file_id)?;
            if mappings
                .insert(file_id.clone(), mapping.clone())
                .is_some_and(|old| old != mapping)
            {
                return Err(invalid(
                    "ordinary export historical and base mappings conflict",
                ));
            }
            if mappings.len() > 100_000 {
                return Err(invalid(
                    "ordinary export retained manifest count exceeds bound",
                ));
            }
        }
    }
    Ok(mappings)
}

fn private_mapping(c: &Connection, id: &FileId) -> Result<(FileManifestId, u64)> {
    let (length, bytes): (u64, u64) = c.query_row(
        "SELECT length(CAST(manifest_id AS BLOB)),bytes FROM projection_content WHERE file_id=?1",
        [id.as_str()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if length != 71 || bytes > MAX_CAPTURE_FILE_BYTES {
        return Err(invalid("ordinary export private mapping exceeds bound"));
    }
    let value: String = c.query_row(
        "SELECT manifest_id FROM projection_content WHERE file_id=?1",
        [id.as_str()],
        |r| r.get(0),
    )?;
    Ok((FileManifestId::parse(value).map_err(invalid)?, bytes))
}

fn require_private_mapping(c: &Connection, manifest: &FileManifest) -> Result<()> {
    if private_mapping(c, manifest.file_id())? != (manifest.id().clone(), manifest.byte_length()) {
        return Err(invalid("ordinary export retained private mapping changed"));
    }
    Ok(())
}

fn read_manifests(
    store: &ProjectionStore,
    c: &Connection,
    scope: &format::ContentScope<'_>,
    mappings: &Mappings,
    cancel: &Cancellation,
) -> Result<BTreeMap<FileId, FileManifest>> {
    if mappings.len() > 100_000 {
        return Err(invalid(
            "ordinary export historical manifest count exceeds bound",
        ));
    }
    let mut result = BTreeMap::new();
    let mut metadata = 0usize;
    let family = format::content_family(c, store, Some(scope))?;
    let lease = family
        .as_ref()
        .ok_or_else(|| invalid("ordinary export lost content lease"))?;
    // Resolve and bound ALL metadata before the expensive byte verification.
    for (id, (expected, bytes)) in mappings {
        check_cancel(cancel)?;
        if private_mapping(c, id)? != (expected.clone(), *bytes) {
            return Err(invalid("ordinary export retained private mapping changed"));
        }
        let manifest = lease.manifest(expected)?;
        if manifest.id() != expected || manifest.file_id() != id || manifest.byte_length() != *bytes
        {
            return Err(invalid(
                "ordinary export historical manifest differs from private mapping",
            ));
        }
        metadata = metadata
            .checked_add(canonical_json(&manifest).map_err(invalid)?.len())
            .filter(|n| *n <= 32 * 1024 * 1024)
            .ok_or_else(|| invalid("ordinary export historical manifests exceed metadata bound"))?;
        result.insert(id.clone(), manifest);
    }
    for manifest in result.values() {
        check_cancel(cancel)?;
        lease.stream(
            manifest,
            &mut CancelledSink {
                output: &mut io::sink(),
                cancel,
            },
        )?;
        check_cancel(cancel)?;
    }
    Ok(result)
}

fn check_cancel(cancel: &Cancellation) -> Result<()> {
    cancel
        .check()
        .map_err(ProviderBootstrapCause::from)
        .map_err(Into::into)
}

fn finish_snapshot<T>(tx: Transaction<'_>, result: Result<T>) -> Result<T> {
    match (result, tx.rollback()) {
        (result, Ok(())) => result,
        (Ok(_), Err(error)) => Err(error.into()),
        (Err(primary), Err(close)) => Err(ProviderBootstrapCause::Cleanup {
            primary: Box::new(primary.into()),
            close: Box::new(close.into()),
        }
        .into()),
    }
}

/// Observe COMMIT explicitly. On failure, also observe rollback instead of
/// relying on Transaction's Drop to discard a possible rollback error.
fn finish_writer<T>(tx: Transaction<'_>, result: Result<T>) -> Result<T> {
    match result {
        Err(error) => finish_snapshot(tx, Err(error)),
        Ok(value) => match tx.execute_batch("COMMIT") {
            Ok(()) => Ok(value),
            Err(error) => finish_snapshot(tx, Err(error.into())),
        },
    }
}

struct CancelledSink<'a> {
    output: &'a mut dyn Write,
    cancel: &'a Cancellation,
}
impl Write for CancelledSink<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        #[cfg(test)]
        verification_cancellation::at_verified_chunk();
        self.cancel.check().map_err(io::Error::other)?;
        self.output.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.cancel.check().map_err(io::Error::other)?;
        self.output.flush()
    }
}
