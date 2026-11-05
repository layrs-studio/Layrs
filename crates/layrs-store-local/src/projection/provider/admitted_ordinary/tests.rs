//! Owned bounded SQL DATA recipes; no family/physical access permission is
//! fabricated. ROOT alone source-releases and executes these child120 tests.
use super::*;
use crate::projection::{
    ProjectionStore, VersionedProjection, conversion, database,
    provider::{
        checkout_journal::{self, CheckoutFile, ExpectedCheckout, RowKey},
        database_io::BareConnection,
        destination_ready::*,
        destination_sql,
        import_validation::{self, ImportDatabaseKind, VerifiedImportClosure},
        managed_admission::{AdmissionWriteEffect, admit_ready},
        managed_budget,
        process_profile::tests::child,
        published_handoff::{self, PublicationSource},
        records::{
            ProviderBootstrapSource, Receipt,
            activation::ProviderActivationRecord,
            import::ImportSource,
            transition::{TransitionRole, TransitionState},
        },
        validation::{
            private_active_capture_with_source, tests::fixture::Fixture as SourceFixture,
        },
    },
    selection,
};
use layrs_core::{projection::ProjectionSnapshot, scm::FileManifest};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
mod family {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    pub(super) struct FamilyMarker {
        pub schema_version: String,
        pub magic: String,
        pub format_version: u32,
        pub min_reader_version: u32,
        pub incarnation: String,
        pub identity: crate::projection::shared::PartialStoreIdentity,
        pub root_identity: String,
        pub metadata_identity: String,
        pub database_identity: String,
    }
}
#[path = "../destination_ready/tests/fixture.rs"]
mod fixture;
#[path = "../destination_ready/tests/source.rs"]
mod source;
use fixture::Fixture;

macro_rules! owned {($name:ident,$body:block)=>{
    #[test]
    #[ignore="UNQUALIFIED admitted ordinary adapter SQL DATA; ROOT source review/release child120 required"]
    fn $name(){child::run(concat!(module_path!(),"::",stringify!($name)),120,||{child::declare();$body});}
};}

fn complete(c: &Connection, expected: &ExpectedCheckout) {
    checkout_journal::declare(c, expected, None).unwrap();
    let row = checkout_journal::read(c, expected, None)
        .unwrap()
        .row(RowKey::Stage)
        .unwrap()
        .clone();
    checkout_journal::advance(
        c,
        expected,
        &row,
        &row.owned_directory("fs:v1:0000000000000001:0000000000001000")
            .unwrap(),
        None,
    )
    .unwrap();
    assert!(expected.directories().is_empty());
    for n in 0..expected.files().len() {
        let mut row = checkout_journal::read(c, expected, None)
            .unwrap()
            .row(RowKey::File(n as u32))
            .unwrap()
            .clone();
        for next in [row.owned_file(&format!("1:0:{}", 2001 + n)).unwrap()] {
            checkout_journal::advance(c, expected, &row, &next, None).unwrap();
            row = next;
        }
        let next = row.verified_file().unwrap();
        checkout_journal::advance(c, expected, &row, &next, None).unwrap();
        row = next;
        let next = row
            .publishing_file(&expected.header().checkout_identity)
            .unwrap();
        checkout_journal::advance(c, expected, &row, &next, None).unwrap();
        row = next;
        let next = row.published_file().unwrap();
        checkout_journal::advance(c, expected, &row, &next, None).unwrap();
    }
}

fn reengage(
    c: &Connection,
    f: &mut Fixture,
    files: &[CheckoutFile],
    metadata: u64,
) -> VerifiedImportClosure {
    f.capture.changes[0]
        .replacement
        .as_mut()
        .unwrap()
        .executable = false;
    f.capture.digest = f.capture.computed_digest().unwrap();
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    enum Image<'a> {
        File {
            file_id: &'a FileId,
            length: u64,
            executable: Option<bool>,
        },
    }
    let image: BTreeMap<_, _> = files
        .iter()
        .map(|file| {
            (
                &file.path,
                Image::File {
                    file_id: &file.file_id,
                    length: file.bytes,
                    executable: Some(file.executable),
                },
            )
        })
        .collect();
    let image = crate::projection::provider::records::codec::hash(
        "layrs-provider-recovery-image-v1",
        &canonical_json(&image).unwrap(),
    )[7..]
        .to_owned();
    let ImportSource::Provider { version, .. } = &mut f.prefix.source else {
        unreachable!()
    };
    version.effective_image_digest = image.clone();
    f.marker.import_digest = Some(f.prefix.digest().unwrap());
    f.copying.plan.source_prefix =
        ProviderImportPrefixRecord::decode(&f.prefix.encode().unwrap()).unwrap();
    f.copying.plan.source_prefix_id = f.prefix.digest().unwrap();
    f.copying.plan.capture_digest = f.capture.digest.clone();
    let payload = canonical_json(
        &json!({"change":&f.capture.changes[0],"manifestId":&f.overlay[0].manifest_id}),
    )
    .unwrap();
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-provider-managed-overlay-v1\0");
    hash.update(&1u64.to_be_bytes());
    hash.update(&(payload.len() as u64).to_be_bytes());
    hash.update(&payload);
    f.copying.plan.working_overlay.digest = format!("blake3:{}", hash.finalize());
    f.copying.plan.quota_bytes =
        managed_budget::quota(&f.required, files.iter().map(|file| file.bytes), metadata).unwrap();
    f.copying.destination.as_mut().unwrap().format = f.marker.clone();
    f.staging = ProviderTransitionIntentRecord::decode(&f.copying.encode().unwrap()).unwrap();
    f.staging.state = TransitionState::Staging;
    f.staging.reservation = Some(f.staging.reservation_operation().unwrap());
    f.receipt.source_prefix =
        ProviderImportPrefixRecord::decode(&f.prefix.encode().unwrap()).unwrap();
    f.receipt.source_prefix_id = f.prefix.digest().unwrap();
    f.receipt.transition_plan_digest = f.staging.plan.digest().unwrap();
    let proof = f.staging.destination.as_ref().unwrap();
    f.receipt.destination_proof_digest = proof.digest(&f.staging.plan).unwrap();
    f.receipt.owner_identity_digest = proof.owner_digest(&f.staging.plan).unwrap();
    f.receipt.destination_capture_digest = f.capture.digest.clone();
    f.ready = ProviderTransitionIntentRecord::decode(&f.staging.encode().unwrap()).unwrap();
    f.ready.state = TransitionState::Ready;
    f.ready.receipt_id = Some(f.receipt.digest().unwrap());
    f.ready.require_receipt(&f.receipt).unwrap();
    let proof = private_active_capture_with_source(
        c,
        &f.source.allocation,
        &f.source.prepared,
        &f.activation.binding,
        Some([0x41; 16]),
        None,
    )
    .unwrap();
    import_validation::verify_source(
        c,
        &f.source.allocation.format,
        ImportDatabaseKind::Provider,
        &f.prefix,
        crate::projection::provider::records::import::ImportSourceReferences::Provider {
            allocation: &f.source.allocation,
            prepared: &f.source.prepared,
            activation: &f.activation,
        },
        Some(&proof),
        &image,
        None,
    )
    .unwrap()
}

fn with_ready8(metadata: u64, body: impl FnOnce(&Connection, &Connection, &Connection, &Fixture)) {
    let source = BareConnection::reference().unwrap();
    let destination = BareConnection::reference().unwrap();
    let family = BareConnection::reference().unwrap();
    let s = source.connection();
    let c = destination.connection();
    let fc = family.connection();
    let (mut f, old) = Fixture::new(s);
    drop(old);
    let files = [CheckoutFile {
        path: f.capture.changes[0].path.clone(),
        manifest_id: f.current.id().clone(),
        file_id: f.current.file_id().clone(),
        bytes: f.current.byte_length(),
        executable: false,
    }];
    let closure = reengage(s, &mut f, &files, metadata);
    f.stage(s, c, &closure);
    f.populate_family(fc);
    fc.execute("UPDATE quota SET bytes=67108864", []).unwrap();
    c.execute_batch("BEGIN IMMEDIATE").unwrap();
    let image = match &f.prefix.source {
        ImportSource::Provider { version, .. } => &version.effective_image_digest,
        _ => unreachable!(),
    };
    let expected = ExpectedCheckout::new(&f.staging, &files, &[], image, None).unwrap();
    complete(c, &expected);
    let data = f.data();
    finalize_checkout_or_replay(s, c, fc, &f.inputs(&closure, &data), &expected, None).unwrap();
    fc.execute(
        "UPDATE worktrees SET state='ready' WHERE id=?1",
        [f.marker.identity.worktree_id.as_str()],
    )
    .unwrap();
    let proof = f.ready.destination.as_ref().unwrap();
    let shared = f.marker.shared.as_ref().unwrap();
    let marker: serde_json::Value = serde_json::from_slice(&f.family_marker).unwrap();
    let operation = f.ready.reservation_operation().unwrap();
    let binding = ExactBinding {
        family_incarnation: shared.incarnation.clone(),
        family_root_identity: shared.root_identity.clone(),
        family_marker_digest: proof.family_marker_digest.clone(),
        family_catalog_identity: marker["databaseIdentity"].as_str().unwrap().into(),
        plan_digest: f.ready.plan.digest().unwrap(),
        prefix_id: f.prefix.digest().unwrap(),
        destination_proof_digest: proof.digest(&f.ready.plan).unwrap(),
        transaction_id: f.ready.transaction_id.clone(),
        owner: f.marker.identity.worktree_id.clone(),
        database_identity: proof.private_database_identity.clone(),
        reservation: operation.clone(),
        operation,
        quota_bytes: f.ready.plan.quota_bytes,
    };
    binding.validate().unwrap();
    fc.execute(
        "INSERT INTO reservations VALUES(?1,?2,?1,?3)",
        params![
            binding.reservation,
            binding.owner.as_str(),
            binding.quota_bytes
        ],
    )
    .unwrap();
    let mut debit = 0u64;
    for (ordinal, manifest) in f.required.iter().enumerate() {
        let chunks = manifest.chunks().len();
        assert!(chunks <= 1024);
        let resident: String = (0..256)
            .map(|n| {
                let bits = chunks.saturating_sub(n * 4).min(4);
                b"0123456789abcdef"[(1usize << bits) - 1] as char
            })
            .collect();
        let payload = canonical_json(manifest).unwrap();
        let charge = 8192 + payload.len() as u64;
        debit = debit.checked_add(charge).unwrap();
        let row = json!({"schemaVersion":"layrs.shared-exact-installation.v1","binding":&binding,"manifestId":manifest.id(),"fileId":manifest.file_id(),"bytes":manifest.byte_length(),"directory":format!("1-2-{ordinal}"),"directoryIdentity":"fs:v1:0000000000000001:0000000000000009","phase":"published","spoolCursor":chunks,"publishCursor":chunks,"active":null,"chargedChunks":"0".repeat(256),"residentChunks":resident,"chargedManifest":false,"debitBytes":charge});
        fc.execute(
            "INSERT INTO exact_installations VALUES(?1,?2,?3,?4,?5,?6,1)",
            params![
                binding.reservation,
                manifest.id().as_str(),
                manifest.file_id().as_str(),
                manifest.byte_length(),
                canonical_json(&row).unwrap(),
                payload
            ],
        )
        .unwrap();
    }
    fc.execute(
        "UPDATE reservations SET remaining=remaining-?1 WHERE id=?2",
        params![debit, binding.reservation],
    )
    .unwrap();
    // The fixture places a real canonical source Published row, then asks the
    // existing full source interpreter for the opaque historical SQL proof.
    let published = ProviderTransitionIntentRecord {
        schema_version: "layrs.projection-provider-transition-intent.v1".into(),
        transaction_id: f.staging.transaction_id.clone(),
        role: TransitionRole::Source,
        state: TransitionState::Published,
        local_generation: 1,
        plan: serde_json::from_slice(&canonical_json(&f.copying.plan).unwrap()).unwrap(),
        destination: serde_json::from_slice(&canonical_json(&f.copying.destination).unwrap())
            .unwrap(),
        reservation: f.staging.reservation.clone(),
        receipt_id: Some(f.receipt.digest().unwrap()),
    };
    published.require_receipt(&f.receipt).unwrap();
    s.execute(
        "INSERT INTO projection_provider_transition_receipts VALUES(?1,?2,?3,NULL,?4)",
        params![
            f.receipt.digest().unwrap(),
            f.receipt.owner_identity_digest,
            f.receipt.transaction_id,
            f.receipt.encode().unwrap()
        ],
    )
    .unwrap();
    s.execute(
        "INSERT INTO projection_provider_transitions VALUES(?1,'source','published',1,?2,?3,?4,?5)",
        params![
            published.transaction_id,
            published.reservation,
            published.receipt_id,
            published.encode().unwrap(),
            published.digest().unwrap()
        ],
    )
    .unwrap();
    s.execute_batch("COMMIT; BEGIN DEFERRED").unwrap();
    // Both opaque prerequisites must be constructible before testing writer
    // refusal. In particular a legacy-capacity test is still a valid Ready8.
    read_managed_ready(c, &f.marker, fc, None).unwrap();
    observe(s, &f);
    drop(closure);
    body(s, c, fc, &f);
    for db in [s, c, fc] {
        if !db.is_autocommit() {
            db.execute_batch("ROLLBACK").unwrap();
        }
    }
    source.finish(Ok(())).unwrap();
    destination.finish(Ok(())).unwrap();
    family.finish(Ok(())).unwrap();
}

fn observe<'s, 'f>(
    s: &'s Connection,
    f: &'f Fixture,
) -> published_handoff::PublishedForReady<'s, 'f> {
    published_handoff::read_published_for_ready(
        s,
        PublicationSource {
            allocation: &f.source.allocation,
            prepared: &f.source.prepared,
            binding: &f.activation.binding,
            root_id: [0x41; 16],
        },
        &f.ready,
        &f.receipt,
        None,
    )
    .unwrap()
}

fn with_admitted9(body: impl FnOnce(&Connection, &Fixture)) {
    with_ready8(32 * 1024 * 1024, |s, c, fc, f| {
        let ready = read_managed_ready(c, &f.marker, fc, None).unwrap();
        let published = observe(s, f);
        assert_eq!(
            admit_ready(c, &f.marker, fc, s, &ready, &published, None).unwrap(),
            AdmissionWriteEffect::Admitted
        );
        c.execute_batch("COMMIT").unwrap();
        s.execute_batch("ROLLBACK").unwrap();
        fc.execute_batch("ROLLBACK").unwrap();
        drop(ready);
        drop(published);
        body(c, f);
    });
}

fn store(f: &Fixture) -> ProjectionStore {
    // DATA handle only. No open/format-access/native permission is fabricated.
    ProjectionStore {
        root: std::path::PathBuf::from(&f.marker.checkout_root),
        marker: f.marker.clone(),
    }
}

fn append_local(c: &Connection, f: &Fixture) -> CapturedPatch {
    let payload = f.placement.canonical_bytes().unwrap();
    let digest = database::digest(4, Some(&f.ready.plan.placement_digest), &payload);
    c.execute(
        "INSERT INTO projection_snapshots VALUES(4,?1,?2,?3)",
        params![digest, f.ready.plan.placement_digest, payload],
    )
    .unwrap();
    selection::insert(c, 4, f.source.source.selection()).unwrap();
    c.execute("UPDATE projection_head SET generation=4", [])
        .unwrap();
    let mut patch = f.capture.clone();
    patch.sequence = 3;
    patch.previous_patch_digest = Some(f.capture.digest.clone());
    patch.projection_generation = 4;
    patch.projection_digest = digest;
    patch.digest = patch.computed_digest().unwrap();
    c.execute(
        "INSERT INTO projection_captures VALUES(3,?1,4,?2)",
        params![patch.digest, canonical_json(&patch).unwrap()],
    )
    .unwrap();
    patch
}

owned!(
    admitted_inspection_owns_data_and_preserves_autocommit_or_caller_transaction,
    {
        with_admitted9(|c, f| {
            assert!(c.is_autocommit());
            c.execute_batch("PRAGMA query_only=ON").unwrap();
            let changes = c.total_changes();
            let first = inspect(c, &f.marker, None).unwrap();
            assert!(c.is_autocommit());
            assert_eq!(c.total_changes(), changes);
            assert_eq!(first.marker(), &f.marker);
            assert_eq!(first.root_id(), f.claims[0].root_id);
            assert_eq!(first.required().len(), 3);
            assert_eq!(first.archive().count(), 3);
            assert_eq!(first.checkout().files().len(), 1);
            assert_eq!(first.ready_generation(), 3);
            assert_eq!(first.imported_capture_count(), 1);
            assert_eq!(first.ready_capture_sequence(), 2);
            assert_eq!(
                first.binding().database_identity,
                f.marker.shared.as_ref().unwrap().database_identity
            );
            first
                .require_same_import(&inspect(c, &f.marker, None).unwrap())
                .unwrap();
            c.execute_batch("PRAGMA query_only=OFF; BEGIN IMMEDIATE")
                .unwrap();
            let claim = "e".repeat(64);
            c.execute(
                "INSERT INTO projection_claims VALUES(?1,?1,'capture','owned-caller')",
                [&claim],
            )
            .unwrap();
            let changes = c.total_changes();
            first
                .require_same_import(&inspect(c, &f.marker, None).unwrap())
                .unwrap();
            assert!(!c.is_autocommit());
            assert_eq!(c.total_changes(), changes);
            assert!(
                c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM projection_claims WHERE owner='owned-caller')",
                    [],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap()
            );
            c.execute_batch("ROLLBACK").unwrap();
            assert!(c.is_autocommit());
            // Immutable DATA survives closure of the snapshot that constructed it;
            // only a fresh inspect may establish the current private interpretation.
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            append_local(c, f);
            c.execute_batch("COMMIT").unwrap();
            first
                .require_same_import(&inspect(c, &f.marker, None).unwrap())
                .unwrap();
            assert!(c.is_autocommit());
        });
    }
);

owned!(
    admitted_capture_origin_dispatch_respects_imported_and_local_generation_intervals,
    {
        with_admitted9(|c, f| {
            let store = store(f);
            let inherited = capture_origin(c, &f.marker, 1, None).unwrap();
            assert!(inherited.imported());
            assert_eq!(inherited.identity(), &f.source.allocation.format.identity);
            assert_eq!(
                (
                    inherited.minimum_generation(),
                    inherited.maximum_generation()
                ),
                (0, 1)
            );
            let local = capture_origin(c, &f.marker, 2, None).unwrap();
            assert!(!local.imported());
            assert_eq!(local.identity(), &f.marker.identity);
            assert_eq!(
                (local.minimum_generation(), local.maximum_generation()),
                (3, 3)
            );
            assert!(capture_origin(c, &f.marker, 0, None).is_err());
            assert!(capture_origin(c, &f.marker, 3, None).is_err());
            assert!(!conversion::imported(c, &store, 0).unwrap());
            assert!(conversion::imported(c, &store, 1).unwrap());
            assert!(!conversion::imported(c, &store, 2).unwrap());
            let imported_patch = crate::projection::capture::storage::load_metadata_for_origin(
                c,
                1,
                inherited.identity(),
                1,
            )
            .unwrap();
            conversion::validate_capture_identity(c, &store, &imported_patch).unwrap();
            let mut escaped_import = imported_patch.clone();
            escaped_import.projection_generation = 2;
            assert!(conversion::validate_capture_identity(c, &store, &escaped_import).is_err());
            conversion::validate_capture_identity(c, &store, &f.capture).unwrap();
            let mut invalid = f.capture.clone();
            invalid.projection_generation = 2;
            assert!(conversion::validate_capture_identity(c, &store, &invalid).is_err());
            invalid.projection_generation = 4;
            assert!(conversion::validate_capture_identity(c, &store, &invalid).is_err());
            invalid = f.capture.clone();
            invalid.identity = f.source.allocation.format.identity.clone();
            assert!(conversion::validate_capture_identity(c, &store, &invalid).is_err());
            // Metadata-only loading uses the normal dispatch once. The immutable
            // reader beneath it only uses *_for_origin, so this must terminate.
            crate::projection::capture::storage::load_metadata(c, &store, 1).unwrap();
            crate::projection::capture::storage::load_metadata(c, &store, 2).unwrap();
            assert!(
                conversion::provenance(c, &store)
                    .unwrap_err()
                    .to_string()
                    .contains("typed admitted-import reader")
            );
            assert!(
                conversion::imported_publication_record(c, &store, 0)
                    .unwrap()
                    .is_none()
            );
            assert!(
                conversion::imported_publication_record(c, &store, 1)
                    .unwrap()
                    .is_none()
            );
            assert!(
                conversion::imported_publication_record(c, &store, 2)
                    .unwrap()
                    .is_none()
            );
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            let later = append_local(c, f);
            c.execute_batch("COMMIT").unwrap();
            let resolved = capture_origin(c, &f.marker, 3, None).unwrap();
            assert!(!resolved.imported());
            assert_eq!(resolved.identity(), &f.marker.identity);
            assert_eq!(
                (resolved.minimum_generation(), resolved.maximum_generation()),
                (3, 4)
            );
            conversion::validate_capture_identity(c, &store, &later).unwrap();
            assert!(!conversion::imported(c, &store, 3).unwrap());
            crate::projection::capture::storage::load_metadata(c, &store, 3).unwrap();
            assert!(c.is_autocommit());
        });
    }
);

owned!(
    admitted_inspection_refuses_corruption_without_ending_caller_transaction_or_repair,
    {
        with_admitted9(|c, f| {
            let store = store(f);
            let first = inspect(c, &f.marker, None).unwrap();
            c.execute_batch("BEGIN IMMEDIATE; SAVEPOINT owned_corrupt")
                .unwrap();
            c.execute(
                "UPDATE projection_managed_admission SET source_published=?1",
                [f.ready.encode().unwrap()],
            )
            .unwrap();
            let changes = c.total_changes();
            assert!(inspect(c, &f.marker, None).is_err());
            assert!(conversion::imported(c, &store, 1).is_err());
            assert!(!c.is_autocommit());
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK TO owned_corrupt; RELEASE owned_corrupt; ROLLBACK")
                .unwrap();
            first
                .require_same_import(&inspect(c, &f.marker, None).unwrap())
                .unwrap();
            assert!(c.is_autocommit());
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            assert_eq!(c.execute("UPDATE projection_import_checkout SET payload=X'7b7d' WHERE path LIKE '%:file:000000'",[]).unwrap(),1);
            c.execute_batch("COMMIT").unwrap();
            let changes = c.total_changes();
            assert!(inspect(c, &f.marker, None).is_err());
            assert!(capture_origin(c, &f.marker, 1, None).is_err());
            assert!(conversion::imported_publication_record(c, &store, 1).is_err());
            assert!(c.is_autocommit());
            assert_eq!(c.total_changes(), changes);
        });
    }
);
