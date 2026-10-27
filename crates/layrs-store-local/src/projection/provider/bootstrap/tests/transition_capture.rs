//! Owned native regression: a cancelled source transition keeps the Active
//! reader usable and leaves later managed capture authorized by its own proof.
use super::*;
use crate::projection::provider::{
    database_io::{self, BareConnection, WriterCloseObserver},
    import_validation::{self, ImportDatabaseKind},
    records::{
        import::{
            ImportClosure, ImportRowset, ImportSource, ImportSourceReferences, ImportVersion,
            ProviderImportKind, ProviderImportPrefixRecord,
        },
        transition::{
            ProviderTransitionIntentRecord, TransitionObservation, TransitionPlan, TransitionRole,
            TransitionRowset, TransitionState,
        },
        AllocationRecord, PreparedRecord, ProviderBootstrapBinding, Receipt, Record,
    },
    transition_sql::{self, SourceIntentEffect},
    validation,
};
use crate::projection::shared::provider_access::FamilyAccess;
use layrs_core::scm::{FileId, WorktreeId};
use layrs_vfs::Cancellation;
use rusqlite::{ffi, Connection, OpenFlags};
use std::{fs, path::Path};

const ROOT_ID: [u8; 16] = [0x63; 16];
const INTENT_ID: &str = "00000000000000000000000000000002";
const EMPTY: &str = "blake3:0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Default)]
struct Close {
    closed: bool,
}
impl WriterCloseObserver for Close {
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        self.closed = succeeded;
        None
    }
}

fn empty(count: u64) -> ImportRowset {
    ImportRowset {
        count,
        digest: EMPTY.into(),
    }
}
fn transition_empty() -> TransitionRowset {
    TransitionRowset {
        count: 0,
        digest: EMPTY.into(),
    }
}

fn planned(
    f: &Fixture,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    binding: &ProviderBootstrapBinding,
) -> ProviderTransitionIntentRecord {
    planned_for_image(
        f,
        allocation,
        prepared,
        binding,
        &blake3::hash(b"native-transition-capture-planned-image")
            .to_hex()
            .to_string(),
    )
}

pub(in crate::projection::provider::bootstrap) fn planned_for_image(
    f: &Fixture,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    binding: &ProviderBootstrapBinding,
    image: &str,
) -> ProviderTransitionIntentRecord {
    let c = Connection::open_with_flags(
        f.metadata.join("projection.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    c.execute_batch("BEGIN DEFERRED").unwrap();
    let active = validation::private_active_capture_with_source(
        &c,
        allocation,
        prepared,
        binding,
        Some(ROOT_ID),
        None,
    )
    .unwrap();
    assert!(active.capture.patches.is_empty());
    let source = &active.active.source;
    assert_eq!(source.commitment().catalog_id, allocation.source.catalog_id);
    let activation = &active.active.activation;
    let activated = &active.active.activated;
    let mut destination = allocation.format.identity.clone();
    destination.worktree_id = WorktreeId::parse("worktree:transition-capture-destination").unwrap();
    let mut inherited = blake3::Hasher::new();
    inherited.update(b"layrs-projection-provider-import-segments-v1\0");
    inherited.update(&0u64.to_be_bytes());
    // The image digest and future destination fields below are declared plan
    // DATA, not a native destination or capture-byte witness.
    let image = image.to_owned();
    let mut prefix = ProviderImportPrefixRecord {
        schema_version: "layrs.projection-provider-import-prefix.v1".into(),
        kind: ProviderImportKind::ManagedOrdinary,
        source: ImportSource::Provider {
            identity: allocation.format.identity.clone(),
            format_digest: records::codec::hash(
                "layrs-projection-provider-import-format-v1",
                &allocation.format.bytes().unwrap(),
            ),
            private_schema: 8,
            allocation: allocation.data_reference().unwrap(),
            prepared: prepared.data_reference().unwrap(),
            binding: ProviderBootstrapBinding::decode(&binding.encode().unwrap()).unwrap(),
            activation_map_id: activation.digest().unwrap(),
            version: ImportVersion {
                generation: activated.generation,
                snapshot_digest: activated.digest.clone(),
                selection_digest: activation.selection_digest.clone(),
                capture_count: 0,
                capture_head: None,
                effective_image_digest: image.clone(),
            },
        },
        destination_identity: destination,
        inherited_segments: ImportRowset {
            count: 0,
            digest: format!("blake3:{}", inherited.finalize()),
        },
        ancestry_root_receipt: None,
        closure: ImportClosure {
            restore_history: None,
            snapshots: empty(2),
            selections: empty(1),
            captures: empty(0),
            publications: empty(0),
            catalogs: empty(1),
            descriptors: empty(1),
            activation_maps: empty(1),
            transition_receipts: empty(0),
            companions: empty(0),
            companion_files: empty(0),
            companion_deletions: empty(0),
            content_mappings: empty(0),
        },
    };
    let metadata = import_validation::read_source_metadata(
        &c,
        &allocation.format,
        ImportDatabaseKind::Provider,
        &prefix,
        ImportSourceReferences::Provider {
            allocation,
            prepared,
            activation,
        },
        &image,
        None,
    )
    .unwrap();
    prefix.closure = import_validation::compute_scoped_closure(
        &c,
        ImportDatabaseKind::Provider,
        &prefix,
        &metadata,
        None,
    )
    .unwrap();
    let prefix_id = prefix.digest().unwrap();
    let snapshot = &activated.snapshot;
    let planned = ProviderTransitionIntentRecord {
        schema_version: "layrs.projection-provider-transition-intent.v1".into(),
        transaction_id: INTENT_ID.into(),
        role: TransitionRole::Source,
        state: TransitionState::Planned,
        local_generation: activated.generation,
        plan: TransitionPlan {
            kind: ProviderImportKind::ManagedOrdinary,
            source_prefix_id: prefix_id,
            source_prefix: prefix,
            destination_parent: f.outer.join("future-destination").to_str().unwrap().into(),
            destination_parent_identity: "fs:v1:1111111111111111:2222222222222222".into(),
            destination_leaf_nonce: "00000000000000000000000000000001".into(),
            observation: TransitionObservation {
                step_id: snapshot.base_step_id.clone(),
                root_tree_id: snapshot.base_root_tree_id.clone(),
                policy_set_id: snapshot.observed_policy_set_id.clone(),
                authorization_epoch: snapshot.observed_authorization_epoch,
                generation: activated.generation + 1,
                snapshot_digest: activated.digest.clone(),
                selection_digest: activation.selection_digest.clone(),
            },
            placement_generation: activated.generation + 2,
            placement_digest: activated.digest.clone(),
            capture_sequence: 1,
            capture_digest: activation.digest().unwrap(),
            working_overlay: transition_empty(),
            companion_id: None,
            companion_files: transition_empty(),
            companion_deletions: transition_empty(),
            protected_manifests: transition_empty(),
            quota_bytes: 0,
        },
        destination: None,
        reservation: None,
        receipt_id: None,
    };
    planned.encode().unwrap();
    c.close().unwrap();
    planned
}

fn write_intent(
    f: &Fixture,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    binding: &ProviderBootstrapBinding,
    previous: Option<&ProviderTransitionIntentRecord>,
    next: &ProviderTransitionIntentRecord,
) -> SourceIntentEffect {
    let family = FamilyAccess::open(f.family.root()).unwrap();
    let _locks = family.locks(&f.worktree, true).unwrap();
    let db = database_io::DatabaseFile::open(
        f.metadata.join("projection.sqlite3"),
        &allocation.private.database_file_identity,
    )
    .unwrap();
    let admission = database_io::clean_admission(&db, false, |c| {
        validation::private_active_readable_with_source(
            c,
            allocation,
            prepared,
            binding,
            Some(ROOT_ID),
            None,
        )
        .map(|_| ())
    })
    .unwrap();
    let mut close = Close::default();
    let bare = admission.open(&db, &mut close).unwrap();
    let result = (|| -> CauseResult<SourceIntentEffect> {
        admission.require_geometry(&bare)?;
        let c = bare.connection();
        c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
        let effect = transition_sql::apply_source_intent(
            c, allocation, prepared, binding, ROOT_ID, previous, next, None,
        )?;
        c.execute_batch("COMMIT")?;
        Ok(effect)
    })();
    let effect = bare.finish_writer(result, &mut close).unwrap();
    assert!(
        close.closed,
        "source intent writer close must be observable"
    );
    effect
}

fn remove_placeholder(path: &Path) {
    if fs::remove_file(path).is_err() {
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions).unwrap();
        fs::remove_file(path).unwrap();
    }
}

#[test]
#[ignore = "UNQUALIFIED until ROOT releases exact owned native transition/capture recipe"]
fn cancelled_source_intent_allows_real_capture_and_reopen() {
    child::run(
        concat!(
            module_path!(),
            "::cancelled_source_intent_allows_real_capture_and_reopen"
        ),
        240,
        || {
            child::declare();
            hooks::reset();
            let (f, active, allocation, prepared, binding) =
                capture_family::active_holder_with_writable_file_private();
            let planned = planned(&f, &allocation, &prepared, &binding);
            let mut session = active
                .into_disconnected_session()
                .unwrap_or_else(|error| panic!("session withheld: {}", error.cause()));
            session.connect().unwrap();
            assert_eq!(
                write_intent(&f, &allocation, &prepared, &binding, None, &planned),
                SourceIntentEffect::Inserted
            );
            let private_path = f.metadata.join("projection.sqlite3");
            let before = database_io::tests::support::witness(&private_path);
            let refusal = session
                .capture("blocked-by-source-intent", &Cancellation::default())
                .err()
                .expect("Planned source intent must gate capture before SQL write");
            assert!(
                refusal
                    .cause
                    .to_string()
                    .contains("active source transition"),
                "{refusal}"
            );
            assert_eq!(database_io::tests::support::witness(&private_path), before);
            let mut cancelled =
                ProviderTransitionIntentRecord::decode(&planned.encode().unwrap()).unwrap();
            cancelled.state = TransitionState::Cancelled;
            assert_eq!(
                write_intent(
                    &f,
                    &allocation,
                    &prepared,
                    &binding,
                    Some(&planned),
                    &cancelled
                ),
                SourceIntentEffect::Updated
            );
            let path = f.checkout.join("README.md");
            assert!(path.exists());
            remove_placeholder(&path);
            let replacement = b"capture after cancelled source intent";
            fs::write(&path, replacement).unwrap();
            let accepted = session
                .capture("post-transition-capture", &Cancellation::default())
                .unwrap();
            assert_eq!(accepted.patch.sequence, 1);
            assert_eq!(accepted.patch.changes.len(), 1);
            let change = &accepted.patch.changes[0];
            assert_eq!(change.path.as_str(), "README.md");
            assert_eq!(
                change.replacement.as_ref().unwrap().file_id,
                FileId::from_bytes(replacement)
            );
            let catalog = Connection::open_with_flags(
                f.family.root().join("metadata/catalog.sqlite3"),
                OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let reservations: u64 = catalog
                .query_row(
                    "SELECT count(*) FROM reservations WHERE owner=?1",
                    [f.worktree.as_str()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                reservations, 0,
                "Accepted capture releases its family reservation"
            );
            catalog.close().unwrap();
            session.disconnect().unwrap();
            drop(session);
            let reopened = PartialStoreFamily::open_provider_active(
                f.family.root(),
                &f.worktree,
                &binding.transaction_id,
                &binding,
            )
            .unwrap();
            reopened.revalidate().unwrap();
            let c = Connection::open_with_flags(&private_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
            c.execute_batch("BEGIN DEFERRED").unwrap();
            let readable = validation::private_active_readable_with_source(
                &c,
                &allocation,
                &prepared,
                &binding,
                Some(ROOT_ID),
                None,
            )
            .unwrap();
            assert_eq!(readable.transitions.len(), 1);
            assert_eq!(readable.transitions[0], cancelled);
            assert_eq!(readable.capture.capture.patches.len(), 1);
            assert_eq!(readable.capture.capture.receipts.len(), 1);
            assert_eq!(readable.capture.capture.claims.len(), 1);
            assert_eq!(readable.capture.capture.transactions.len(), 1);
            assert_eq!(readable.capture.capture.patches[0], accepted.patch);
            c.close().unwrap();
        },
    );
}
