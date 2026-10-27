//! Owned SQL DATA recipes only. The fixture observes a stored canonical source
//! Published through its historical interpreter, then constructs private9 DATA.
//! This is NOT a production 8->9 writer or durable physical ACK qualification.
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
use crate::projection::{capture::CapturedPatch, database, selection, VersionedProjection};
use layrs_core::{
    projection::ProjectionSnapshot,
    scm::{canonical_json, FileManifest},
};
use rusqlite::params;
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
    #[ignore="UNQUALIFIED private9 immutable provenance SQL DATA; ROOT source review/release child120 required"]
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

fn reengage(c: &Connection, f: &mut Fixture, files: &[CheckoutFile]) -> VerifiedImportClosure {
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

fn with_private9(body: impl FnOnce(&Connection, &Fixture)) {
    let source = BareConnection::reference().unwrap();
    let destination = BareConnection::reference().unwrap();
    let family = BareConnection::reference().unwrap();
    let s = source.connection();
    let c = destination.connection();
    let (mut f, old) = Fixture::new(s);
    drop(old);
    let files = [CheckoutFile {
        path: f.capture.changes[0].path.clone(),
        manifest_id: f.current.id().clone(),
        file_id: f.current.file_id().clone(),
        bytes: f.current.byte_length(),
        executable: false,
    }];
    let closure = reengage(s, &mut f, &files);
    f.stage(s, c, &closure);
    f.populate_family(family.connection());
    c.execute_batch("BEGIN IMMEDIATE").unwrap();
    let image = match &f.prefix.source {
        ImportSource::Provider { version, .. } => &version.effective_image_digest,
        _ => unreachable!(),
    };
    let expected = ExpectedCheckout::new(&f.staging, &files, &[], image, None).unwrap();
    complete(c, &expected);
    let data = f.data();
    finalize_checkout_or_replay(
        s,
        c,
        family.connection(),
        &f.inputs(&closure, &data),
        &expected,
        None,
    )
    .unwrap();
    // Synthetic source fixture row, selected by the existing complete source
    // SQL interpreter. No destination-derived fake proof object is constructed.
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
    let observed = published_handoff::read_published_for_ready(
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
    .unwrap();
    let actual = observed.published().encode().unwrap();
    let source_digest = observed.published().digest().unwrap();
    drop(observed);
    c.execute_batch(schema::SQL).unwrap();
    c.execute(
        "INSERT INTO projection_import_checkout SELECT * FROM projection_prejournal",
        [],
    )
    .unwrap();
    c.execute("DELETE FROM projection_prejournal", []).unwrap();
    c.execute_batch("PRAGMA user_version=9").unwrap();
    let permit = schema::require_schema(c, &f.marker, None).unwrap();
    let (_, _, archive) = checkout_journal::read_archived(c, &f.ready, &[], &permit, None).unwrap();
    let family_record = &f.source.allocation.family;
    let proof = f.ready.destination.as_ref().unwrap();
    let operation = f.ready.reservation_operation().unwrap();
    let binding = ExactBinding {
        family_incarnation: family_record.incarnation.clone(),
        family_root_identity: family_record.root_identity.clone(),
        family_marker_digest: family_record.marker_digest.clone(),
        family_catalog_identity: family_record.catalog_file_identity.clone(),
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
    let (source_format_digest, source_database_identity) = match &f.prefix.source {
        ImportSource::Provider {
            format_digest,
            binding,
            ..
        } => (
            format_digest.clone(),
            binding.recovery.database_identity.clone(),
        ),
        _ => unreachable!(),
    };
    let record = ManagedAdmissionRecord {
        schema_version: "layrs.managed-admission.v1".into(),
        format_digest: crate::projection::provider::records::codec::hash(
            "layrs-managed-admission-format-v1",
            &f.marker.bytes().unwrap(),
        ),
        transaction_id: f.ready.transaction_id.clone(),
        prefix_id: f.prefix.digest().unwrap(),
        ready_intent_digest: f.ready.digest().unwrap(),
        receipt_id: f.receipt.digest().unwrap(),
        source_published_digest: source_digest,
        source_database_identity,
        source_format_digest,
        root_id: f.claims[0].root_id.clone(),
        binding,
        archive,
        boundaries: record::Boundaries {
            imported_generation: 1,
            imported_capture_count: 1,
            observation_generation: 2,
            ready_generation: 3,
            ready_capture_sequence: 2,
        },
        metadata_layout_version: 1,
    };
    c.execute(
        "INSERT INTO projection_managed_admission VALUES(1,?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            record.digest().unwrap(),
            record.source_published_digest,
            actual,
            record.receipt_id,
            record.boundaries.ready_generation,
            record.boundaries.ready_capture_sequence,
            record.archive.count,
            record.archive.digest,
            record.encode().unwrap()
        ],
    )
    .unwrap();
    c.execute_batch("COMMIT; BEGIN DEFERRED").unwrap();
    drop(permit);
    drop(closure);
    s.execute_batch("ROLLBACK").unwrap();
    family.connection().execute_batch("ROLLBACK").unwrap();
    source.finish(Ok(())).unwrap();
    family.finish(Ok(())).unwrap();
    body(c, &f);
    if !c.is_autocommit() {
        c.execute_batch("ROLLBACK").unwrap();
    }
    destination.finish(Ok(())).unwrap();
}

owned!(
    admitted_private_history_survives_local_capture_publication_claims_and_operational_stage,
    {
        with_private9(|c, f| {
            let proof = read_admitted_import(c, &f.marker, None).unwrap();
            assert_eq!(proof.required().len(), 3);
            assert_eq!(proof.archive().rows().len(), 3);
            assert_eq!(proof.source_published().state, TransitionState::Published);
            assert_ne!(f.historical.id(), f.current.id());
            let new_manifest = FileManifest::from_chunks(
                layrs_core::scm::ChunkingDescriptor::single(),
                &[layrs_core::scm::Chunk::new(b"ordinary followup".to_vec()).unwrap()],
            )
            .unwrap();
            let mut patch = f.capture.clone();
            patch.sequence = 3;
            patch.previous_patch_digest = Some(f.capture.digest.clone());
            let file = patch.changes[0].replacement.as_mut().unwrap();
            file.file_id = new_manifest.file_id().clone();
            file.bytes = new_manifest.byte_length();
            patch.digest = patch.computed_digest().unwrap();
            c.execute(
                "INSERT INTO projection_content VALUES(?1,?2,?3)",
                params![
                    new_manifest.file_id().as_str(),
                    new_manifest.id().as_str(),
                    new_manifest.byte_length()
                ],
            )
            .unwrap();
            c.execute(
                "INSERT INTO projection_captures VALUES(3,?1,3,?2)",
                params![patch.digest, canonical_json(&patch).unwrap()],
            )
            .unwrap();
            c.execute(
                "INSERT INTO projection_claims VALUES(?1,?1,'capture','later')",
                ["f".repeat(64)],
            )
            .unwrap();
            c.execute("INSERT INTO projection_prejournal VALUES('ordinary-stage','capture:later','ordinary',X'7b7d','owned')",[]).unwrap();
            let receipt = canonical_json(&json!({"status":"rejected"})).unwrap();
            let receipt_digest = crate::projection::provider::records::codec::hash(
                "layrs-projection-receipt-v1",
                &receipt,
            );
            let request_digest = format!("blake3:{}", "e".repeat(64));
            let state = VersionedProjection {
                generation: 3,
                digest: f.ready.plan.placement_digest.clone(),
                snapshot: f.placement.clone(),
            };
            c.execute(
                "INSERT INTO projection_publications VALUES(2,?1,?2,?3,NULL,3,?4,?5)",
                params![
                    request_digest,
                    receipt_digest,
                    receipt,
                    f.ready.plan.placement_digest,
                    canonical_json(&state).unwrap()
                ],
            )
            .unwrap();
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
            let changes = c.total_changes();
            proof.require(c, None).unwrap();
            let current = read_admitted_import(c, &f.marker, None).unwrap();
            assert_eq!(current.archive().rows(), proof.archive().rows());
            assert_eq!(current.required(), proof.required());
            assert_eq!(
                current.capture_origin(c, 1, None).unwrap().0,
                &f.source.allocation.format.identity
            );
            let local = current.capture_origin(c, 3, None).unwrap();
            assert_eq!(local.0, &f.marker.identity);
            assert_eq!((local.1, local.2), (3, 4));
            assert_eq!(c.total_changes(), changes);
            assert!(
                import_validation::read_destination_prefix(c, &f.marker, None).is_err(),
                "Ready8 route must remain strict"
            );
        });
    }
);

owned!(
    admitted_private_history_refuses_ack_archive_and_schema_corruption_without_repair,
    {
        with_private9(|c, f| {
            let proof = read_admitted_import(c, &f.marker, None).unwrap();
            c.execute_batch("SAVEPOINT corrupt").unwrap();
            c.execute("UPDATE projection_import_checkout SET payload=X'7b7d' WHERE path LIKE '%:file:000000'",[]).unwrap();
            let changes = c.total_changes();
            assert!(read_admitted_import(c, &f.marker, None).is_err());
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK TO corrupt; RELEASE corrupt")
                .unwrap();
            c.execute_batch("SAVEPOINT corrupt").unwrap();
            c.execute(
                "UPDATE projection_managed_admission SET source_published=?1",
                [f.ready.encode().unwrap()],
            )
            .unwrap();
            let changes = c.total_changes();
            assert!(read_admitted_import(c, &f.marker, None).is_err());
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK TO corrupt; RELEASE corrupt")
                .unwrap();
            c.execute_batch("SAVEPOINT corrupt; PRAGMA user_version=8")
                .unwrap();
            let changes = c.total_changes();
            assert!(read_admitted_import(c, &f.marker, None).is_err());
            assert!(import_validation::read_destination_prefix(c, &f.marker, None).is_err());
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK TO corrupt; RELEASE corrupt")
                .unwrap();
            proof.require(c, None).unwrap();
            c.execute_batch("COMMIT; PRAGMA query_only=ON; BEGIN DEFERRED")
                .unwrap();
            let changes = c.total_changes();
            read_admitted_import(c, &f.marker, None).unwrap();
            assert_eq!(c.total_changes(), changes);
        });
    }
);

/// Valid source-plan DATA from this fixture's actual admitted SQL prefix. The
/// declared image and synthetic parent are not native or byte authority.
fn ordinary_source_planned(c: &Connection, f: &Fixture) -> ProviderTransitionIntentRecord {
    use crate::projection::provider::bootstrap::{
        derive_ordinary_data, ImageFile, RequiredManifest,
    };
    use layrs_core::scm::WorktreeId;
    let mut destination = f.marker.identity.clone();
    destination.worktree_id = WorktreeId::parse("worktree:ordinary-data-outgoing").unwrap();
    let image_digest = match &f.prefix.source {
        ImportSource::Provider { version, .. } => &version.effective_image_digest,
        _ => unreachable!(),
    };
    let (prefix, proof) = import_validation::plan_admitted_import(
        c,
        &f.marker,
        destination.clone(),
        image_digest,
        None,
    )
    .unwrap();
    let mappings = proof.copied_content_mappings(c, &prefix, None).unwrap();
    let base = &f.source.source.descriptors()[0].manifest;
    let known = [base, &f.historical, &f.current];
    let historical: Vec<_> = mappings
        .iter()
        .map(|(file, (manifest, bytes))| RequiredManifest {
            file_id: file.clone(),
            manifest_id: manifest.clone(),
            bytes: *bytes,
        })
        .collect();
    let mut supplied = BTreeMap::new();
    for row in &historical {
        let manifest = known
            .iter()
            .find(|manifest| {
                manifest.file_id() == &row.file_id
                    && manifest.id() == &row.manifest_id
                    && manifest.byte_length() == row.bytes
            })
            .unwrap();
        supplied.insert(
            (row.file_id.clone(), row.manifest_id.clone()),
            (**manifest).clone(),
        );
    }
    for manifest in [base, &f.current] {
        supplied.insert(
            (manifest.file_id().clone(), manifest.id().clone()),
            manifest.clone(),
        );
    }
    let supplied: Vec<_> = supplied.into_values().collect();
    let selected = crate::projection::selection::at(c, f.ready.plan.placement_generation).unwrap();
    let (generation, selection_digest) = match &prefix.source {
        ImportSource::Ordinary { version, .. } => {
            (version.generation, version.selection_digest.clone())
        }
        _ => unreachable!(),
    };
    let path = &f.placement.entries[0].path;
    let files = [ImageFile {
        path,
        manifest: &f.current,
        executable: false,
        expected: Some((base.file_id().clone(), f.placement.entries[0].executable)),
    }];
    let data = derive_ordinary_data(
        &prefix,
        &f.placement,
        &selected,
        &selection_digest,
        &[base],
        &files,
        &[],
        destination,
        "C:\\ordinary-data-parent",
        "fs:v1:1111111111111111:2222222222222222",
        "00000000000000000000000000000091",
        "00000000000000000000000000000092",
        &historical,
        &supplied,
        0,
    )
    .unwrap();
    let planned = ProviderTransitionIntentRecord {
        schema_version: "layrs.projection-provider-transition-intent.v1".into(),
        transaction_id: "00000000000000000000000000000092".into(),
        role: TransitionRole::Source,
        state: TransitionState::Planned,
        local_generation: generation,
        plan: data.plan,
        destination: None,
        reservation: None,
        receipt_id: None,
    };
    planned.validate_fields().unwrap();
    planned
}

owned!(
    admitted_outgoing_plan_preserves_ingress_and_invalidates_retained_outgoing_proof,
    {
        with_private9(|c, f| {
            use crate::projection::provider::{
                ordinary_transition, transition_sql::SourceIntentEffect,
            };
            let before = read_admitted_import(c, &f.marker, None).unwrap();
            assert!(before.outgoing().is_empty());
            let admission_digest = before.record().digest().unwrap();
            let incoming = before.ready().encode().unwrap();
            let permit = schema::require_schema(c, &f.marker, None).unwrap();
            let retained_prefix = import_validation::read_admitted_destination_prefix(
                c,
                &f.marker,
                &permit,
                before.record(),
                None,
            )
            .unwrap();
            let planned = ordinary_source_planned(c, f);
            assert_eq!(
                ordinary_transition::apply_planned(
                    c,
                    &f.marker,
                    &planned.plan.source_prefix,
                    &planned,
                    None
                )
                .unwrap(),
                SourceIntentEffect::Inserted
            );
            let changed = read_admitted_import(c, &f.marker, None).unwrap();
            assert_eq!(changed.record().digest().unwrap(), admission_digest);
            assert_eq!(changed.ready().encode().unwrap(), incoming);
            assert_eq!(
                changed.outgoing(),
                &[ProviderTransitionIntentRecord::decode(&planned.encode().unwrap()).unwrap()]
            );
            assert!(
                before.require(c, None).is_err(),
                "old outgoing vector must not silently change"
            );
            retained_prefix.require_prefix_scope(c, None).unwrap();
            let changes = c.total_changes();
            assert_eq!(
                ordinary_transition::apply_planned(
                    c,
                    &f.marker,
                    &planned.plan.source_prefix,
                    &planned,
                    None
                )
                .unwrap(),
                SourceIntentEffect::AlreadyPresent
            );
            assert_eq!(c.total_changes(), changes);
            changed.require(c, None).unwrap();
            assert_eq!(
                ordinary_transition::cancel_planned(c, &f.marker, &planned.transaction_id, None)
                    .unwrap(),
                SourceIntentEffect::Updated
            );
            assert!(changed.require(c, None).is_err());
            let terminal = read_admitted_import(c, &f.marker, None).unwrap();
            assert_eq!(terminal.outgoing()[0].state, TransitionState::Cancelled);
            assert_eq!(terminal.record().digest().unwrap(), admission_digest);
            assert_eq!(terminal.ready().encode().unwrap(), incoming);
            let changes = c.total_changes();
            assert_eq!(
                ordinary_transition::cancel_planned(c, &f.marker, &planned.transaction_id, None)
                    .unwrap(),
                SourceIntentEffect::AlreadyPresent
            );
            assert_eq!(c.total_changes(), changes);
            retained_prefix.require_prefix_scope(c, None).unwrap();
        });
    }
);

owned!(
    admitted_outgoing_malformed_extra_row_refuses_reader_and_writers_before_effects,
    {
        with_private9(|c, f| {
            use crate::projection::provider::ordinary_transition;
            let planned = ordinary_source_planned(c, f);
            c.execute_batch("SAVEPOINT malformed_outgoing").unwrap();
            c.execute("INSERT INTO projection_provider_transitions VALUES(?1,'source','cancelled',3,NULL,NULL,X'7b7d',?2)",
                params!["00000000000000000000000000000093", format!("blake3:{}", "1".repeat(64))]).unwrap();
            let changes = c.total_changes();
            assert!(read_admitted_import(c, &f.marker, None).is_err());
            assert!(ordinary_transition::apply_planned(
                c,
                &f.marker,
                &planned.plan.source_prefix,
                &planned,
                None
            )
            .is_err());
            assert!(ordinary_transition::cancel_planned(
                c,
                &f.marker,
                &planned.transaction_id,
                None
            )
            .is_err());
            assert_eq!(c.total_changes(), changes);
            c.execute(
                "UPDATE projection_provider_transitions SET payload=zeroblob(262145) WHERE id=?1",
                ["00000000000000000000000000000093"],
            )
            .unwrap();
            let changes = c.total_changes();
            let error = read_admitted_import(c, &f.marker, None)
                .err()
                .expect("oversized extra row must refuse");
            assert!(error.to_string().contains("SQL bounds"), "{error}");
            assert!(ordinary_transition::apply_planned(
                c,
                &f.marker,
                &planned.plan.source_prefix,
                &planned,
                None
            )
            .is_err());
            assert!(ordinary_transition::cancel_planned(
                c,
                &f.marker,
                &planned.transaction_id,
                None
            )
            .is_err());
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK TO malformed_outgoing; RELEASE malformed_outgoing")
                .unwrap();
            read_admitted_import(c, &f.marker, None).unwrap();
        });
    }
);

owned!(
    admitted_retained_prefix_refuses_actual_admission_anchor_rebinding,
    {
        with_private9(|c, f| {
            let proof = read_admitted_import(c, &f.marker, None).unwrap();
            let permit = schema::require_schema(c, &f.marker, None).unwrap();
            let retained = import_validation::read_admitted_destination_prefix(
                c,
                &f.marker,
                &permit,
                proof.record(),
                None,
            )
            .unwrap();
            let mut unrelated = proof.record().clone();
            unrelated.transaction_id = "00000000000000000000000000000094".into();
            let changes = c.total_changes();
            assert!(import_validation::read_admitted_destination_prefix(
                c, &f.marker, &permit, &unrelated, None
            )
            .is_err());
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("SAVEPOINT rebound_admission").unwrap();
            let mut actual = proof.record().clone();
            actual.source_database_identity.push_str("-changed");
            c.execute(
                "UPDATE projection_managed_admission SET digest=?1,payload=?2",
                params![actual.digest().unwrap(), actual.encode().unwrap()],
            )
            .unwrap();
            let changes = c.total_changes();
            let error = retained.require_prefix_scope(c, None).unwrap_err();
            assert!(
                error.to_string().contains("admission anchor changed"),
                "{error}"
            );
            assert!(proof.require(c, None).is_err());
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK TO rebound_admission; RELEASE rebound_admission")
                .unwrap();
            retained.require_prefix_scope(c, None).unwrap();
            proof.require(c, None).unwrap();
        });
    }
);
