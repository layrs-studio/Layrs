//! Exclusive initial allocation. Failure retains every allocated object.
use super::*;
use crate::secure_fs::{PinnedDirectory, PinnedFile};
use layrs_core::scm::{FileId, canonical_json};
use std::{
    ffi::OsStr,
    io::{Read, Seek, SeekFrom, Write},
};

pub(super) struct Initial {
    pub(super) family: FamilyAccess,
    pub(super) artifacts: Artifacts,
    pub(super) preflight: layrs_vfs::ControlRootPreflight,
    pub(super) owner: crate::lock::StoreLock,
    pub(super) lifecycle: crate::lock::StoreLock,
    stage_parent: PinnedDirectory,
    stage: PinnedDirectory,
    format: PinnedFile,
    ancestry: Vec<PinnedDirectory>,
}
fn directory(
    parent: &PinnedDirectory,
    name: &OsStr,
    target: &Path,
    kind: ProviderBootstrapCreate,
    ctx: &mut WriteContext,
) -> CauseResult<PinnedDirectory> {
    let index = ctx.arm(ProviderBootstrapWriteStep::ExclusiveCreate(kind), target)?;
    let child = parent.create_new_private_child(name)?;
    ctx.confirm(index)?;
    Ok(child)
}
fn file(
    parent: &PinnedDirectory,
    name: &OsStr,
    target: &Path,
    kind: ProviderBootstrapCreate,
    ctx: &mut WriteContext,
) -> CauseResult<PinnedFile> {
    let index = ctx.arm(ProviderBootstrapWriteStep::ExclusiveCreate(kind), target)?;
    let child = parent.create_new_private_file(name)?;
    ctx.confirm(index)?;
    Ok(child)
}

/// Retain every ancestor used for the native separation comparison. Common
/// ancestors are permitted; checkout/family nesting and stage-under-either are not.
fn ancestry(root: &Path, family: &Path, stage_parent: &Path) -> CauseResult<Vec<PinnedDirectory>> {
    let checkout = PinnedDirectory::open(root)?;
    let family_pin = PinnedDirectory::open(family)?;
    let stage_pin = PinnedDirectory::open(stage_parent)?;
    if !checkout.same_volume(&stage_pin) {
        return Err(invalid("provider stage and checkout must share a volume"));
    }
    let root_id = checkout.identity_token();
    let family_id = family_pin.identity_token();
    let mut pins = vec![checkout, family_pin, stage_pin];
    for (at, forbidden) in [
        (root, vec![family_id.clone()]),
        (family, vec![root_id.clone()]),
        (stage_parent, vec![root_id, family_id]),
    ] {
        for ancestor in at.ancestors() {
            let pin = PinnedDirectory::open(ancestor)?;
            if forbidden.iter().any(|id| id == &pin.identity_token()) {
                return Err(invalid(
                    "provider checkout/family/stage native ancestry overlaps",
                ));
            }
            pins.push(pin);
        }
    }
    Ok(pins)
}

pub(super) fn begin(
    expected_family: &PartialStoreFamily,
    root: &Path,
    stage_parent: &Path,
    worktree: &WorktreeId,
    transaction: &str,
    source: &ProviderBootstrapSource,
    ctx: &mut WriteContext,
) -> CauseResult<Initial> {
    records::codec::transaction(transaction)?;
    super::super::process_profile::gate()?;
    let family = FamilyAccess::open(expected_family.root())?;
    family.writer_matches(expected_family)?;
    // Reject every already-known receipt context bound before consuming a member.
    // The later writer_record/require_allocation checks still bind fresh identities.
    family.writer_record()?.validate()?;
    let preflight = layrs_vfs::ControlRootPreflight::open(root)?;
    let stage_canonical = crate::projection::shared::provider_access::canonical_path(stage_parent)?;
    let stage_parent = PathBuf::from(stage_canonical);
    let family_canonical =
        crate::projection::shared::provider_access::canonical_path(family.root())?;
    let ancestry = ancestry(
        preflight.root(),
        Path::new(&family_canonical),
        &stage_parent,
    )?;
    let stage_parent_pin = PinnedDirectory::open(&stage_parent)?;
    let identity = family.writer_identity(worktree);
    crate::projection::format::validate_identity(&identity)?;
    // Source is opaque and validated by its only public constructor; recheck the
    // accepted logical commitment before any exclusive allocation.
    source.commitment().validate_fields()?;
    let metadata_path = family.member(worktree);
    let member_path = metadata_path
        .parent()
        .ok_or_else(|| invalid("provider member path missing"))?;
    let stage_name = format!(
        "pb-control-{transaction}-{}",
        crate::projection::shared::unique_id()?
    );
    let stage_path = stage_parent.join(&stage_name);
    for locator in [
        preflight.root(),
        stage_parent.as_path(),
        member_path,
        metadata_path.as_path(),
        stage_path.as_path(),
        &stage_path.join("FORMAT"),
    ] {
        crate::projection::shared::provider_access::exact_path(locator)?;
    }
    database_io::validate_writer_filename(family.catalog.path())?;
    database_io::validate_writer_filename(&metadata_path.join("projection.sqlite3"))?;
    *ctx = WriteContext::current(
        ProviderBootstrapWriterOperation::Begin,
        ProviderBootstrapAttempt {
            family_root: family.root().to_owned(),
            checkout_root: preflight.root().to_owned(),
            stage_parent: stage_parent.clone(),
            worktree_id: worktree.clone(),
            transaction_id: transaction.to_owned(),
        },
    );
    let lifecycle = family.writer_lifecycle(false)?;
    let worktrees = family.writer_worktrees()?;
    let member = directory(
        &worktrees,
        member_path
            .file_name()
            .ok_or_else(|| invalid("member name absent"))?,
        member_path,
        ProviderBootstrapCreate::Member,
        ctx,
    )?;
    let metadata = directory(
        &member,
        OsStr::new("metadata"),
        &metadata_path,
        ProviderBootstrapCreate::Metadata,
        ctx,
    )?;
    let lock_path = metadata_path.join("layrs.lock");
    let creator = file(
        &metadata,
        OsStr::new("layrs.lock"),
        &lock_path,
        ProviderBootstrapCreate::OwnerLock,
        ctx,
    )?;
    creator.file().sync_all()?;
    metadata.sync()?;
    drop(creator); // Every creator/duplicate closes before acquire_existing.
    #[cfg(test)]
    tests::hooks::before_owner(&lock_path)?;
    let owner = family.writer_owner(worktree)?;
    #[cfg(test)]
    tests::hooks::owner_acquired()?;
    let control_path = metadata_path.join("provider-control");
    let control = directory(
        &metadata,
        OsStr::new("provider-control"),
        &control_path,
        ProviderBootstrapCreate::ReceiptDirectory,
        ctx,
    )?;
    let database_path = metadata_path.join("projection.sqlite3");
    let db = file(
        &metadata,
        OsStr::new("projection.sqlite3"),
        &database_path,
        ProviderBootstrapCreate::PrivateDatabase,
        ctx,
    )?;
    let database_identity = crate::projection::file::identity(db.file())?;
    drop(db); // DELETE creator must not coexist with a SQLite connection.
    let stage = directory(
        &stage_parent_pin,
        OsStr::new(&stage_name),
        &stage_path,
        ProviderBootstrapCreate::ControlStage,
        ctx,
    )?;
    let format = file(
        &stage,
        OsStr::new("FORMAT"),
        &stage_path.join("FORMAT"),
        ProviderBootstrapCreate::Format,
        ctx,
    )?;
    // Qualified attribute/READ_CONTROL EMPTY observation: creators remain open.
    // This is the NOT_UNDER check for external parent/stage/FORMAT before A,
    // SQL initialization or FORMAT bytes. Earlier empty allocations stay inert.
    let observed = layrs_vfs::observe_control_directory_stage(&preflight, &stage_path)?;
    if observed.root_filesystem_identity() != preflight.filesystem_identity()
        || observed.stage_parent_filesystem_identity() != stage_parent_pin.identity_token()
        || observed.directory_filesystem_identity() != stage.identity_token()
        || observed.format_binding_identity() != crate::projection::file::identity(format.file())?
    {
        return Err(invalid(
            "provider empty allocation observation differs from Store creators",
        ));
    }
    for pin in &ancestry {
        pin.verify_path_identity()?;
    }
    preflight.revalidate()?;
    let family_record = family.writer_record()?;
    let mut marker = serde_json::json!({
        "schemaVersion":"layrs.projection-store-format.v1", "magic":"LAYRS_NATIVE_STORE",
        "formatVersion":4,"minReaderVersion":4,"identity":identity,
        "checkoutRoot":preflight.root().to_str().ok_or_else(||invalid("non-Unicode provider root"))?,
        "checkoutIdentity":preflight.filesystem_identity(),"storeIdentity":stage.identity_token(),
        "metadataIdentity":metadata.identity_token(),"initialSnapshotDigest":source.commitment().snapshot_digest,
        "shared":{"root":family.root(),"incarnation":family_record.incarnation,"rootIdentity":family_record.root_identity,
            "metadataPath":metadata_path,"databaseIdentity":database_identity}
    });
    if !matches!(
        source.selection(),
        layrs_core::projection::ProjectionSelection::All
    ) {
        marker["initialSelectionDigest"] =
            source.commitment().selection_payload_digest.clone().into();
    }
    let marker = crate::projection::format::parse_marker_bytes(
        &canonical_json(&marker).map_err(crate::projection::invalid)?,
    )?;
    let format_bytes = marker.bytes()?;
    let pending = receipt_io::PendingReceipt::create(
        &control,
        &control_path,
        transaction,
        ProviderBootstrapReceipt::Allocation,
        ctx,
    )?;
    let allocation = AllocationRecord {
        schema_version: "layrs.projection-provider-allocation.v1".into(),
        transaction_id: transaction.into(),
        receipt_file_identity: pending.identity()?,
        family: family_record,
        private: PrivateRecord {
            metadata_path: crate::projection::shared::provider_access::exact_path(&metadata_path)?
                .into(),
            canonical_metadata_path: crate::projection::shared::provider_access::canonical_path(
                &metadata_path,
            )?,
            metadata_identity: metadata.identity_token(),
            provider_control_identity: control.identity_token(),
            database_file_identity: database_identity,
        },
        stage_identity_hex: hex(&observed.encode()),
        format: marker,
        format_bytes: format_bytes.len() as u64,
        format_file_id: FileId::from_bytes(&format_bytes),
        source: {
            let commitment = source.commitment();
            SourceCommitment {
                source_generation: commitment.source_generation,
                snapshot_bytes: commitment.snapshot_bytes,
                snapshot_digest: commitment.snapshot_digest.clone(),
                selection_bytes: commitment.selection_bytes,
                selection_payload_digest: commitment.selection_payload_digest.clone(),
                selection_digest: commitment.selection_digest.clone(),
                catalog_bytes: commitment.catalog_bytes,
                catalog_id: commitment.catalog_id.clone(),
                catalog_aggregate_bytes: commitment.catalog_aggregate_bytes,
                descriptor_count: commitment.descriptor_count,
                descriptor_bytes: commitment.descriptor_bytes,
                descriptor_wire_bytes: commitment.descriptor_wire_bytes,
                descriptors_digest: commitment.descriptors_digest.clone(),
                vfs_catalog_version: commitment.vfs_catalog_version,
                vfs_catalog_id: commitment.vfs_catalog_id.clone(),
            }
        },
    };
    allocation.require_source_and_member(source, &metadata_path)?;
    family.require_allocation(&allocation)?;
    pending.publish(&control, &allocation, ctx)?;
    let artifacts = Artifacts::open(&family, worktree, transaction)?;
    if artifacts.allocation != allocation
        || artifacts.prepared.is_some()
        || artifacts.abandoned.is_some()
    {
        return Err(invalid("new provider allocation readback differs"));
    }
    Ok(Initial {
        family,
        artifacts,
        preflight,
        owner,
        lifecycle,
        stage_parent: stage_parent_pin,
        stage,
        format,
        ancestry,
    })
}
impl Initial {
    pub(super) fn write_format(
        mut self,
        ctx: &mut WriteContext,
    ) -> CauseResult<(
        FamilyAccess,
        Artifacts,
        layrs_vfs::ControlRootPreflight,
        crate::lock::StoreLock,
        crate::lock::StoreLock,
    )> {
        self.artifacts.verify(&self.family)?;
        self.preflight.revalidate()?;
        for pin in &self.ancestry {
            pin.verify_path_identity()?;
        }
        let bytes = self.artifacts.allocation.format.bytes()?;
        let stage = self.artifacts.allocation.stage_identity()?;
        if self.format.file().metadata()?.len() != 0
            || crate::projection::file::identity(self.format.file())?
                != stage.format_binding_identity()
        {
            return Err(invalid(
                "provider FORMAT creator is not exact empty allocation",
            ));
        }
        let effect = ctx.arm(
            ProviderBootstrapWriteStep::FormatWrite,
            &stage.stage().join("FORMAT"),
        )?;
        #[cfg(test)]
        tests::hooks::format_prefix(&mut self.format, &bytes)?;
        self.format.file_mut().write_all(&bytes)?;
        self.format.file().sync_all()?;
        self.format.file_mut().seek(SeekFrom::Start(0))?;
        let mut observed = Vec::new();
        self.format
            .file_mut()
            .take(bytes.len() as u64 + 1)
            .read_to_end(&mut observed)?;
        if observed != bytes || self.format.file().metadata()?.len() != bytes.len() as u64 {
            return Err(invalid("provider FORMAT creator readback differs"));
        }
        self.stage.sync()?;
        ctx.confirm(effect)?;
        drop(self.format);
        drop(self.stage);
        drop(self.stage_parent);
        drop(self.ancestry);
        Ok((
            self.family,
            self.artifacts,
            self.preflight,
            self.owner,
            self.lifecycle,
        ))
    }
}
pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
