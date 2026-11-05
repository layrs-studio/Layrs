//! Owned native fixture setup, not a production bootstrap writer. The three
//! completed receipt files are constructed directly for reader qualification;
//! this does not qualify receipt interruption/publication durability.
use super::super::super::{database_io::tests::support, validation::tests::fixture::Fixture};
use super::super::*;
use crate::{
    lock::{SessionMode, StoreLock},
    projection::{PartialStoreIdentity, SharedQuota},
    secure_fs::PinnedDirectory,
};
use std::{
    fs,
    io::Write,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};

pub(in crate::projection::provider) struct NativeFixture {
    pub(in crate::projection::provider) outer: PathBuf,
    pub(in crate::projection::provider) family: PathBuf,
    pub(in crate::projection::provider) checkout: PathBuf,
    pub(in crate::projection::provider) metadata: PathBuf,
    pub(in crate::projection::provider) allocation: AllocationRecord,
    pub(in crate::projection::provider) prepared: Option<PreparedRecord>,
    pub(in crate::projection::provider) guard: Option<layrs_vfs::ControlDirectoryGuard>,
}
struct StopPrepared;
impl layrs_vfs::ControlDirectoryPublishFaults for StopPrepared {
    fn at(&self, point: layrs_vfs::ControlDirectoryPublishPoint) -> std::io::Result<()> {
        if point == layrs_vfs::ControlDirectoryPublishPoint::PreRename {
            Err(std::io::Error::other(
                "owned fixture stop after real prepared callback",
            ))
        } else {
            Ok(())
        }
    }
}
fn file_id(path: &Path) -> String {
    crate::projection::file::identity(&crate::layout::open_read_no_follow(path).unwrap()).unwrap()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// Deterministic fixture restriction only; not SQLite/production long-path support.
fn require_fixture_sqlite_path_budget(database: &Path) {
    let name_units = database.as_os_str().encode_wide().count();
    for (suffix, suffix_units) in [
        ("", 0usize),
        ("-journal", 8),
        ("-wal", 4),
        ("-shm", 4),
        // SQLite 3.46 formats -mj%06X9%02X into 12 ASCII units.
        ("super-journal reserve", 12),
    ] {
        let including_nul = name_units
            .checked_add(suffix_units)
            .and_then(|units| units.checked_add(1));
        assert!(
            including_nul.is_some_and(|units| units <= 260),
            "UNQUALIFIED: named SQLite fixture path budget exceeded: database={database:?}, \
             suffix={suffix:?}, name_utf16_units={name_units}, suffix_utf16_units={suffix_units}, \
             including_nul={including_nul:?}, limit_including_nul=260"
        );
    }
}

impl NativeFixture {
    pub(in crate::projection::provider) fn create(phase: MountPhase) -> Self {
        Self::create_with_family_setup(phase, |_, _| {})
    }
    pub(in crate::projection::provider) fn create_with_family_setup(
        phase: MountPhase,
        setup: impl FnOnce(&PartialStoreFamily, &Path),
    ) -> Self {
        let outer = support::owned_root("b");
        let family = outer.join("family");
        let checkout = outer.join("checkout");
        let stages = outer.join("stages");
        for path in [&family, &checkout, &stages] {
            fs::create_dir(path).unwrap();
        }
        let mut f = Fixture::new("G1", false);
        let identity = &f.allocation.format.identity;
        let member_name = blake3::hash(identity.worktree_id.as_str().as_bytes())
            .to_hex()
            .to_string();
        let metadata = family.join("worktrees").join(&member_name).join("metadata");
        // All family members use the same 64-ASCII-hex hash component, so this
        // also bounds the ordinary-member setup callback without changing it.
        // Check before family creation performs the first disk SQLite operation.
        for database in [
            family.join("metadata/catalog.sqlite3"),
            metadata.join("projection.sqlite3"),
            outer.join("external.sqlite3"),
        ] {
            require_fixture_sqlite_path_budget(&database);
        }
        let created_family = PartialStoreFamily::create(
            &family,
            PartialStoreIdentity {
                store_id: identity.store_id.clone(),
                space_id: identity.space_id.clone(),
                origin: identity.origin.clone(),
                principal_id: identity.principal_id.clone(),
            },
            SharedQuota {
                bytes: 64 * 1024 * 1024,
            },
        )
        .unwrap();
        // Test setup precedes the provider owner. Ordinary capture/GC must never
        // be asked to interpret the subsequently populated provider backend.
        setup(&created_family, &outer);
        let family_marker_bytes = fs::read(family.join("FORMAT")).unwrap();
        let family_marker: serde_json::Value =
            serde_json::from_slice(&family_marker_bytes).unwrap();
        let family_pin = PinnedDirectory::open(&family).unwrap();
        let worktrees = family_pin.open_private_child("worktrees".as_ref()).unwrap();
        let member = worktrees
            .create_new_private_child(member_name.as_ref())
            .unwrap();
        let metadata_pin = member
            .create_new_private_child("metadata".as_ref())
            .unwrap();
        let lock_creator = metadata_pin
            .create_new_private_file("layrs.lock".as_ref())
            .unwrap();
        lock_creator.file().sync_all().unwrap();
        drop(lock_creator);
        let lifecycle = StoreLock::acquire_existing(
            &family.join("metadata/lifecycle.lock"),
            SessionMode::Shared,
        )
        .unwrap();
        let owner =
            StoreLock::acquire_existing(&metadata.join("layrs.lock"), SessionMode::Exclusive)
                .unwrap();
        let control = metadata_pin
            .create_new_private_child("provider-control".as_ref())
            .unwrap();
        let database_creator = metadata_pin
            .create_new_private_file("projection.sqlite3".as_ref())
            .unwrap();
        database_creator.file().sync_all().unwrap();
        drop(database_creator);
        let preflight = layrs_vfs::ControlRootPreflight::open(&checkout).unwrap();
        let stage_parent = PinnedDirectory::open(&stages).unwrap();
        let stage_creator = stage_parent
            .create_new_private_child("control".as_ref())
            .unwrap();
        let mut format_creator = stage_creator
            .create_new_private_file("FORMAT".as_ref())
            .unwrap();
        let stage = layrs_vfs::observe_control_directory_stage(&preflight, &stages.join("control"))
            .unwrap();
        assert_eq!(
            stage.root_filesystem_identity(),
            preflight.filesystem_identity()
        );
        assert_eq!(
            stage.stage_parent_filesystem_identity(),
            stage_parent.identity_token()
        );
        assert_eq!(
            stage.directory_filesystem_identity(),
            stage_creator.identity_token()
        );
        assert_eq!(
            stage.format_binding_identity(),
            crate::projection::file::identity(format_creator.file()).unwrap()
        );
        let mut marker = serde_json::to_value(&f.allocation.format).unwrap();
        marker["checkoutRoot"] = preflight.root().to_str().unwrap().into();
        marker["checkoutIdentity"] = preflight.filesystem_identity().into();
        marker["storeIdentity"] = stage.directory_filesystem_identity().into();
        marker["metadataIdentity"] = metadata_pin.identity_token().into();
        marker["shared"] = serde_json::json!({"root":family,"incarnation":family_marker["incarnation"],"rootIdentity":family_marker["rootIdentity"],"metadataPath":metadata,"databaseIdentity":file_id(&metadata.join("projection.sqlite3"))});
        let marker: crate::projection::format::Marker = serde_json::from_value(marker).unwrap();
        let format = marker.bytes().unwrap();
        let mut allocation_file = control
            .create_new_private_file("allocation.json".as_ref())
            .unwrap();
        let mut family_digest = blake3::Hasher::new();
        family_digest.update(b"layrs-projection-recovery-family-v1\0");
        family_digest.update(&family_marker_bytes);
        f.allocation=serde_json::from_value(serde_json::json!({
            "schemaVersion":"layrs.projection-provider-allocation.v1","transactionId":f.allocation.transaction_id,
            "receiptFileIdentity":crate::projection::file::identity(allocation_file.file()).unwrap(),
            "family":{"root":family,"canonicalRoot":crate::projection::shared::provider_access::canonical_path(&family).unwrap(),
                "incarnation":family_marker["incarnation"],"rootIdentity":family_marker["rootIdentity"],
                "markerFileIdentity":file_id(&family.join("FORMAT")),"markerDigest":format!("blake3:{}",family_digest.finalize()),
                "metadataIdentity":family_marker["metadataIdentity"],"catalogFileIdentity":family_marker["databaseIdentity"]},
            "private":{"metadataPath":metadata,"canonicalMetadataPath":crate::projection::shared::provider_access::canonical_path(&metadata).unwrap(),
                "metadataIdentity":metadata_pin.identity_token(),"providerControlIdentity":control.identity_token(),"databaseFileIdentity":file_id(&metadata.join("projection.sqlite3"))},
            "stageIdentityHex":hex(&stage.encode()),"format":marker,"formatBytes":format.len(),"formatFileId":layrs_core::scm::FileId::from_bytes(&format),"source":f.source.commitment()
        })).unwrap();
        allocation_file
            .file_mut()
            .write_all(&f.allocation.encode().unwrap())
            .unwrap();
        allocation_file.file().sync_all().unwrap();
        drop(allocation_file);
        {
            let _catalog_lock = StoreLock::acquire_existing(
                &family.join("metadata/catalog.lock"),
                SessionMode::Exclusive,
            )
            .unwrap();
            let c = rusqlite::Connection::open(family.join("metadata/catalog.sqlite3")).unwrap();
            c.execute(
                "INSERT INTO worktrees VALUES(?1,?2,?3,?4,'initializing')",
                rusqlite::params![
                    f.allocation.format.identity.worktree_id.as_str(),
                    metadata.to_str().unwrap(),
                    f.allocation.private.database_file_identity,
                    format
                ],
            )
            .unwrap();
            c.close().unwrap();
        }
        let c = rusqlite::Connection::open_with_flags(
            metadata.join("projection.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        assert_eq!(
            c.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "delete"
        );
        f.populate(&c, MountPhase::Allocating);
        c.close().unwrap(); // Known initialization commit precedes the only FORMAT write.
        format_creator.file_mut().write_all(&format).unwrap();
        format_creator.file().sync_all().unwrap();
        drop(format_creator);
        drop(stage_creator);
        drop(stage_parent); // ALL construction FORMAT/stage handles before publisher.
        let mut guard = None;
        if phase != MountPhase::Allocating {
            let publish = layrs_vfs::publish_control_directory_recoverable(
                &preflight,
                stage.stage(),
                &stage,
                &format,
                None,
                |identity| {
                    let mut file = control
                        .create_new_private_file("prepared.json".as_ref())
                        .unwrap();
                    f.prepared = PreparedRecord {
                        schema_version: "layrs.projection-provider-prepared.v1".into(),
                        transaction_id: f.allocation.transaction_id.clone(),
                        receipt_file_identity: crate::projection::file::identity(file.file())
                            .unwrap(),
                        allocation: f.allocation.data_reference().unwrap(),
                        control_identity_hex: hex(&identity.encode()),
                    };
                    file.file_mut()
                        .write_all(&f.prepared.encode().unwrap())
                        .unwrap();
                    file.file().sync_all().unwrap();
                    drop(file);
                    set_mount(&metadata, &f, MountPhase::ControlPrepared);
                    Ok(())
                },
                if phase == MountPhase::ControlPrepared {
                    &StopPrepared as &dyn layrs_vfs::ControlDirectoryPublishFaults
                } else {
                    &layrs_vfs::NoControlDirectoryPublishFaults
                },
            );
            if phase == MountPhase::ControlPrepared {
                assert!(matches!(
                    publish,
                    Err(layrs_vfs::Error::ControlDirectory {
                        kind: layrs_vfs::ControlDirectoryFailure::Interrupted,
                        point: layrs_vfs::ControlDirectoryPublishPoint::PreRename,
                        ..
                    })
                ));
            } else {
                let published = publish.unwrap();
                published.revalidate_bootstrap_root().unwrap();
                set_mount(&metadata, &f, MountPhase::ControlPublished);
                published.revalidate_bootstrap_root().unwrap();
                guard = Some(published);
            }
        }
        drop(owner);
        drop(lifecycle);
        Self {
            outer,
            family,
            checkout: preflight.root().to_path_buf(),
            metadata,
            allocation: f.allocation,
            prepared: if phase == MountPhase::Allocating {
                None
            } else {
                Some(f.prepared)
            },
            guard,
        }
    }
    pub(in crate::projection::provider) fn inspect(&self) -> ProviderBootstrapInspection {
        PartialStoreFamily::inspect_provider_bootstrap(
            &self.family,
            &self.allocation.format.identity.worktree_id,
            &self.allocation.transaction_id,
        )
        .unwrap()
    }
    pub(in crate::projection::provider) fn terminal(&self, registry: RegistryAtDecision) {
        let inspection = self.inspect();
        assert!(matches!(
            inspection.state,
            ProviderBootstrapState::Allocating
                | ProviderBootstrapState::ControlPrepared
                | ProviderBootstrapState::ControlPublished
        ));
        let c = rusqlite::Connection::open(&self.metadata.join("projection.sqlite3")).unwrap();
        let payload: Vec<u8> = c
            .query_row("SELECT payload FROM projection_provider_mounts", [], |r| {
                r.get(0)
            })
            .unwrap();
        c.close().unwrap();
        let mount = MountRecord::decode(&payload).unwrap();
        let metadata = PinnedDirectory::open(&self.metadata).unwrap();
        let control = metadata
            .open_private_child("provider-control".as_ref())
            .unwrap();
        let mut file = control
            .create_new_private_file("abandoned.json".as_ref())
            .unwrap();
        let eligibility = EligibilityRecord {
            schema_version: "layrs.projection-provider-bootstrap-eligibility.v1".into(),
            registry_at_decision: registry,
            private_state: PrivateState::InitialSchema8 {
                catalog_id: self.allocation.source.catalog_id.clone(),
                snapshot_digest: self.allocation.source.snapshot_digest.clone(),
                selection_digest: self.allocation.source.selection_digest.clone(),
                mount_digest: mount.digest().unwrap(),
            },
        };
        let t = AbandonedRecord {
            schema_version: "layrs.projection-provider-abandoned.v1".into(),
            transaction_id: self.allocation.transaction_id.clone(),
            receipt_file_identity: crate::projection::file::identity(file.file()).unwrap(),
            allocation: self.allocation.data_reference().unwrap(),
            prepared: self
                .prepared
                .as_ref()
                .map(Receipt::data_reference)
                .transpose()
                .unwrap(),
            eligibility_digest: eligibility.digest().unwrap(),
            eligibility,
        };
        file.file_mut().write_all(&t.encode().unwrap()).unwrap();
        file.file().sync_all().unwrap();
    }
}
fn set_mount(metadata: &Path, f: &Fixture, phase: MountPhase) {
    let mount = f.mount(phase);
    let phase = match phase {
        MountPhase::Allocating => "allocating",
        MountPhase::ControlPrepared => "control_prepared",
        MountPhase::ControlPublished => "control_published",
    };
    let c = rusqlite::Connection::open(metadata.join("projection.sqlite3")).unwrap();
    c.execute(
        "UPDATE projection_provider_mounts SET phase=?1,payload=?2,digest=?3",
        rusqlite::params![phase, mount.encode().unwrap(), mount.digest().unwrap()],
    )
    .unwrap();
    c.close().unwrap();
}
