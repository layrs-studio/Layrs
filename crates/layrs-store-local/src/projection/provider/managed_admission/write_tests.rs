//! Owned SQL writer recipes only. SourcePublished and all identities are
//! synthetic SQL DATA; no independently opened durable-source/physical driver
//! qualification or ordinary admission is asserted by these memory fixtures.
use super::super::*;
use super::*;
use crate::projection::provider::{
    checkout_journal::{self, CheckoutFile, ExpectedCheckout, RowKey},
    database_io::BareConnection,
    destination_ready::*,
    destination_sql,
    import_validation::{self, ImportDatabaseKind, VerifiedImportClosure},
    process_profile::tests::child,
    published_handoff::{self, PublicationSource},
    records::{activation::ProviderActivationRecord, ProviderBootstrapSource, Receipt},
    validation::{private_active_capture_with_source, tests::fixture::Fixture as SourceFixture},
};
use crate::projection::provider::{destination_ready::read_managed_ready, managed_budget};
use crate::projection::{capture::CapturedPatch, database, selection, VersionedProjection};
use layrs_core::{
    projection::ProjectionSnapshot,
    scm::{canonical_json, FileManifest},
};
use rusqlite::params;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn admission_representation_comparison_ignores_family_order_but_keeps_exact_union() {
    use layrs_core::scm::{Chunk, ChunkingDescriptor};
    let mut manifests: Vec<_> = [b"first content".as_slice(), b"second content".as_slice()]
        .into_iter()
        .map(|bytes| {
            FileManifest::from_chunks(
                ChunkingDescriptor::single(),
                &[Chunk::new(bytes.to_vec()).unwrap()],
            )
            .unwrap()
        })
        .collect();
    let mut rows: Vec<_> = manifests
        .iter()
        .map(|manifest| RequiredRepresentation {
            file_id: manifest.file_id().clone(),
            manifest_id: manifest.id().clone(),
            bytes: manifest.byte_length(),
        })
        .collect();
    rows.sort_unstable();
    assert!(same_representations(&rows, &manifests));
    manifests.reverse();
    assert!(same_representations(&rows, &manifests));
    assert!(!same_representations(&rows[..1], &manifests));
    rows[0].bytes += 1;
    assert!(!same_representations(&rows, &manifests));
}

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
    #[ignore="UNQUALIFIED private8 to private9 writer SQL DATA; ROOT source review/release child120 required"]
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

fn original_rows(c: &Connection) -> Vec<(String, String, String, Vec<u8>, String)> {
    c.prepare("SELECT path,operation,digest,payload,state FROM projection_prejournal ORDER BY path COLLATE BINARY").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap()
}

owned!(
    admission_writer_copies_exact_archive_and_replays_without_source_or_family,
    {
        with_ready8(32 * 1024 * 1024, |s, c, fc, f| {
            let original = original_rows(c);
            let ready = read_managed_ready(c, &f.marker, fc, None).unwrap();
            let published = observe(s, f);
            let source_changes = s.total_changes();
            let family_changes = fc.total_changes();
            let family_reservation: u64 = fc
                .query_row("SELECT remaining FROM reservations", [], |r| r.get(0))
                .unwrap();
            assert_eq!(
                admit_ready(c, &f.marker, fc, s, &ready, &published, None).unwrap(),
                AdmissionWriteEffect::Admitted
            );
            assert!(!c.is_autocommit());
            assert_eq!(original_rows(c).len(), 0);
            let admitted = read_admitted_import(c, &f.marker, None).unwrap();
            assert_eq!(admitted.archive().rows(), ready.journal().rows());
            assert_eq!(admitted.source_published(), published.published());
            let archived:Vec<(String,String,String,Vec<u8>,String)>=c.prepare("SELECT path,operation,digest,payload,state FROM projection_import_checkout ORDER BY path COLLATE BINARY").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
            assert_eq!(archived, original);
            assert_eq!(s.total_changes(), source_changes);
            assert_eq!(fc.total_changes(), family_changes);
            assert_eq!(
                fc.query_row("SELECT remaining FROM reservations", [], |r| r
                    .get::<_, u64>(0))
                    .unwrap(),
                family_reservation
            );
            c.execute_batch("COMMIT; PRAGMA query_only=ON; BEGIN DEFERRED")
                .unwrap();
            s.execute_batch("ROLLBACK").unwrap();
            fc.execute_batch("ROLLBACK").unwrap();
            assert!(published.require(s, None).is_err());
            assert!(ready.require(c, fc, None).is_err());
            let before = c.total_changes();
            assert_eq!(
                admit_ready(c, &f.marker, fc, s, &ready, &published, None).unwrap(),
                AdmissionWriteEffect::AlreadyAdmitted
            );
            assert_eq!(c.total_changes(), before);
            read_admitted_import(c, &f.marker, None).unwrap();
        });
    }
);

owned!(
    admission_writer_rollback_restores_private8_and_original_journal,
    {
        with_ready8(32 * 1024 * 1024, |s, c, fc, f| {
            let original = original_rows(c);
            let ready = read_managed_ready(c, &f.marker, fc, None).unwrap();
            let published = observe(s, f);
            c.execute_batch("SAVEPOINT owned_rollback").unwrap();
            admit_ready(c, &f.marker, fc, s, &ready, &published, None).unwrap();
            c.execute_batch("ROLLBACK TO owned_rollback; RELEASE owned_rollback")
                .unwrap();
            assert_eq!(
                c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                    .unwrap(),
                8
            );
            assert_eq!(original_rows(c), original);
            assert!(!c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name IN ('projection_managed_admission','projection_import_checkout'))",[],|r|r.get::<_,bool>(0)).unwrap());
            ready.require(c, fc, None).unwrap();
            assert_eq!(
                admit_ready(c, &f.marker, fc, s, &ready, &published, None).unwrap(),
                AdmissionWriteEffect::Admitted
            );
        });
    }
);

owned!(
    admission_writer_refuses_changed_journal_or_historical_ack_before_effects,
    {
        with_ready8(32 * 1024 * 1024, |s, c, fc, f| {
            let ready = read_managed_ready(c, &f.marker, fc, None).unwrap();
            let published = observe(s, f);
            c.execute_batch("SAVEPOINT owned_corrupt").unwrap();
            assert_eq!(c.execute("UPDATE projection_prejournal SET payload=X'7b7d' WHERE path LIKE '%:file:000000'",[]).unwrap(),1);
            let before = c.total_changes();
            assert!(admit_ready(c, &f.marker, fc, s, &ready, &published, None).is_err());
            assert_eq!(c.total_changes(), before);
            assert_eq!(
                c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                    .unwrap(),
                8
            );
            c.execute_batch("ROLLBACK TO owned_corrupt; RELEASE owned_corrupt")
                .unwrap();
            s.execute_batch("SAVEPOINT owned_corrupt").unwrap();
            assert_eq!(
                s.execute(
                    "UPDATE projection_provider_transitions SET digest=?1 WHERE id=?2",
                    params![format!("blake3:{}", "f".repeat(64)), f.ready.transaction_id]
                )
                .unwrap(),
                1
            );
            let before = c.total_changes();
            assert!(admit_ready(c, &f.marker, fc, s, &ready, &published, None).is_err());
            assert_eq!(c.total_changes(), before);
            assert!(!c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='projection_import_checkout')",[],|r|r.get::<_,bool>(0)).unwrap());
            s.execute_batch("ROLLBACK TO owned_corrupt; RELEASE owned_corrupt")
                .unwrap();
            assert_eq!(
                admit_ready(c, &f.marker, fc, s, &ready, &published, None).unwrap(),
                AdmissionWriteEffect::Admitted
            );
            c.execute(
                "UPDATE projection_managed_admission SET source_published=?1",
                [f.ready.encode().unwrap()],
            )
            .unwrap();
            let before = c.total_changes();
            assert!(admit_ready(c, &f.marker, fc, s, &ready, &published, None).is_err());
            assert_eq!(c.total_changes(), before);
        });
    }
);

owned!(
    admission_writer_refuses_fixed_legacy_budget_without_resizing,
    {
        with_ready8(256 * 1024, |s, c, fc, f| {
            let ready = read_managed_ready(c, &f.marker, fc, None).unwrap();
            let published = observe(s, f);
            let original = original_rows(c);
            let quota = f.ready.plan.quota_bytes;
            let reserved: u64 = fc
                .query_row("SELECT remaining FROM reservations", [], |r| r.get(0))
                .unwrap();
            let before = c.total_changes();
            let error = admit_ready(c, &f.marker, fc, s, &ready, &published, None).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("fixed metadata envelope insufficient"),
                "{error}"
            );
            assert_eq!(c.total_changes(), before);
            assert_eq!(original_rows(c), original);
            assert_eq!(f.ready.plan.quota_bytes, quota);
            assert_eq!(
                fc.query_row("SELECT remaining FROM reservations", [], |r| r
                    .get::<_, u64>(0))
                    .unwrap(),
                reserved
            );
            assert_eq!(
                c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                    .unwrap(),
                8
            );
            assert!(!c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='projection_import_checkout')",[],|r|r.get::<_,bool>(0)).unwrap());
            ready.require(c, fc, None).unwrap();
        });
    }
);
