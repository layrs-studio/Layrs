//! One bounded complete SQL DATA recipe. B9 is interpreted by the real
//! admitted reader; image, C identities, Published journals and checkout inode
//! strings are declared DATA. No physical bytes, owner or durable ACK is proved.
use super::*;
use crate::projection::{
    provider::{
        admitted_ordinary,
        bootstrap::{
            self, ImageFile, ProviderRecoveryExportClass, ProviderRecoveryExportFile,
            RequiredManifest,
        },
        checkout_journal::{self, CheckoutFile, ExpectedCheckout, RowKey},
        database_io::BareConnection,
        import_validation::plan_admitted_import,
        managed_admission::{read_admitted_import, tests::with_admitted_source_family_sql},
        managed_budget,
        ordinary_destination::{self, OrdinaryCopyInputs},
        ordinary_transition, owned_test as child,
        records::{
            import::ImportRowset,
            transition::{TransitionBackendKind, TransitionDestinationProof},
        },
    },
    shared::{
        admitted_catalog::{AdmittedFamilyContext, ExportReserveOnlyTarget},
        exact_content::ExactBinding,
        exact_protection::{ExactProtection, ProtectionPresence},
    },
};
use layrs_core::scm::{TreeEntryTarget, WorktreeId};
use rusqlite::types::ValueRef;
use serde_json::json;

const TX: &str = "000000000000000000000000000ce101";
const LEAF: &str = "000000000000000000000000000ce102";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}
fn dump(c: &Connection) -> BTreeMap<String, Vec<Vec<Cell>>> {
    let mut names = c
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name LIMIT 65")
        .unwrap();
    let names = names
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(names.len() <= 64);
    let mut result = BTreeMap::new();
    let mut bytes = 0;
    for name in names {
        assert!(name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
        let mut q = c
            .prepare(&format!("SELECT * FROM \"{name}\" LIMIT 129"))
            .unwrap();
        let columns = q.column_count();
        assert!(columns <= 32);
        let mut query = q.query([]).unwrap();
        let mut rows = Vec::new();
        while let Some(row) = query.next().unwrap() {
            assert!(rows.len() < 128);
            rows.push(
                (0..columns)
                    .map(|n| match row.get_ref(n).unwrap() {
                        ValueRef::Null => Cell::Null,
                        ValueRef::Integer(v) => Cell::Integer(v),
                        ValueRef::Real(v) => Cell::Real(v.to_bits()),
                        ValueRef::Text(v) => {
                            bytes += v.len();
                            Cell::Text(v.to_vec())
                        }
                        ValueRef::Blob(v) => {
                            bytes += v.len();
                            Cell::Blob(v.to_vec())
                        }
                    })
                    .collect(),
            );
            assert!(bytes <= 16 * 1024 * 1024);
        }
        rows.sort();
        result.insert(name, rows);
    }
    result
}

fn published_group(
    c: &Connection,
    binding: &ExactBinding,
    required: &[FileManifest],
    insert_reservation: bool,
) {
    if insert_reservation {
        c.execute(
            "INSERT INTO reservations VALUES(?1,?2,?3,?4)",
            params![
                binding.reservation,
                binding.owner.as_str(),
                binding.operation,
                binding.quota_bytes
            ],
        )
        .unwrap();
    }
    let mut debit = 0;
    for (ordinal, manifest) in required.iter().enumerate() {
        let payload = canonical_json(manifest).unwrap();
        let mut cost = 8192 + payload.len() as u64;
        let mut charged = vec![b'0'; 256];
        for (n, chunk) in manifest.chunks().iter().enumerate() {
            if c.execute(
                "INSERT INTO chunks VALUES(?1,?2) ON CONFLICT DO NOTHING",
                params![chunk.chunk_id().as_str(), chunk.byte_length()],
            )
            .unwrap()
                == 1
            {
                let digit = (charged[n / 4] as char).to_digit(16).unwrap() as usize;
                charged[n / 4] = b"0123456789abcdef"[digit | (1 << (n % 4))];
                cost += chunk.byte_length();
            }
        }
        c.execute(
            "INSERT INTO files VALUES(?1,?2) ON CONFLICT DO NOTHING",
            params![manifest.file_id().as_str(), manifest.byte_length()],
        )
        .unwrap();
        let added = c
            .execute(
                "INSERT INTO manifests VALUES(?1,?2,?3,?4) ON CONFLICT DO NOTHING",
                params![
                    manifest.id().as_str(),
                    manifest.file_id().as_str(),
                    manifest.byte_length(),
                    payload
                ],
            )
            .unwrap()
            == 1;
        if added {
            cost += payload.len() as u64;
        }
        for (n, chunk) in manifest.chunks().iter().enumerate() {
            c.execute(
                "INSERT INTO manifest_chunks VALUES(?1,?2,?3) ON CONFLICT DO NOTHING",
                params![manifest.id().as_str(), n, chunk.chunk_id().as_str()],
            )
            .unwrap();
        }
        let record = json!({"schemaVersion":"layrs.shared-exact-installation.v1","binding":binding,
            "manifestId":manifest.id(),"fileId":manifest.file_id(),"bytes":manifest.byte_length(),
            "directory":format!("12-34-{ordinal}"),"directoryIdentity":"fs:v1:0000000000000001:0000000000000020",
            "phase":"published","spoolCursor":manifest.chunks().len(),"publishCursor":manifest.chunks().len(),"active":null,
            "chargedChunks":String::from_utf8(charged).unwrap(),"residentChunks":"0".repeat(256),
            "chargedManifest":added,"debitBytes":cost});
        c.execute(
            "INSERT INTO exact_installations VALUES(?1,?2,?3,?4,?5,?6,1)",
            params![
                binding.reservation,
                manifest.id().as_str(),
                manifest.file_id().as_str(),
                manifest.byte_length(),
                canonical_json(&record).unwrap(),
                payload
            ],
        )
        .unwrap();
        debit += cost;
    }
    c.execute(
        "UPDATE reservations SET remaining=remaining-?1 WHERE id=?2",
        params![debit, binding.reservation],
    )
    .unwrap();
}

fn complete(c: &Connection, expected: &ExpectedCheckout) {
    checkout_journal::declare(c, expected, None).unwrap();
    assert!(expected.directories().is_empty());
    let previous = checkout_journal::read(c, expected, None)
        .unwrap()
        .row(RowKey::Stage)
        .unwrap()
        .clone();
    let next = previous
        .owned_directory("fs:v1:0000000000000001:0000000000000030")
        .unwrap();
    checkout_journal::advance(c, expected, &previous, &next, None).unwrap();
    for n in 0..expected.files().len() {
        let mut previous = checkout_journal::read(c, expected, None)
            .unwrap()
            .row(RowKey::File(n as u32))
            .unwrap()
            .clone();
        let next = previous.owned_file(&format!("1:0:{}", 8000 + n)).unwrap();
        checkout_journal::advance(c, expected, &previous, &next, None).unwrap();
        previous = next;
        let next = previous.verified_file().unwrap();
        checkout_journal::advance(c, expected, &previous, &next, None).unwrap();
        previous = next;
        let next = previous
            .publishing_file(&expected.header().checkout_identity)
            .unwrap();
        checkout_journal::advance(c, expected, &previous, &next, None).unwrap();
        previous = next;
        let next = previous.published_file().unwrap();
        checkout_journal::advance(c, expected, &previous, &next, None).unwrap();
    }
}

#[test]
#[ignore = "ROOT source release and audited ordinary Ready SQL child120/outer180 required"]
fn ordinary_ready_writer_preserves_restore_history_replays_readonly_and_refuses_corruption() {
    let test = concat!(
        module_path!(),
        "::ordinary_ready_writer_preserves_restore_history_replays_readonly_and_refuses_corruption"
    );
    child::run(test, 120, || {
        child::declare();
        exercise_ready_sql(false);
    });
}

#[test]
#[ignore = "ROOT source release and audited ordinary publication SQL child120/outer180 required"]
fn ordinary_family_publication_authenticates_actual_ready_changes_one_row_and_replays_readonly() {
    let test = concat!(module_path!(), "::ordinary_family_publication_authenticates_actual_ready_changes_one_row_and_replays_readonly");
    child::run(test, 120, || {
        child::declare();
        exercise_ready_sql(true);
    });
}

#[test]
#[ignore = "ROOT source release and audited ordinary C10 floor SQL child120/outer180 required; proposed unmeasured"]
fn ordinary_checkout_progress_isolates_future_c10_metadata_floor_at_one_byte_boundary() {
    let test = concat!(
        module_path!(),
        "::ordinary_checkout_progress_isolates_future_c10_metadata_floor_at_one_byte_boundary"
    );
    child::run(test, 120, || {
        child::declare();
        // Each call creates its own genuine admitted-B SQL fixture. Neither
        // case reuses a mutated plan, receipt, reservation or journal binding.
        for at_floor in [false, true] {
            exercise_ready_sql_phases(false, false, false, Some(at_floor));
        }
    });
}

// Independent layout arithmetic for this exact one-file/no-directory recipe.
// Do not call AdmissionBudget: its minimum is the guard under test.
fn independent_c10_floor(ready_floor: u64, representations: usize) -> u64 {
    assert!((1..=100_000).contains(&representations));
    let archive = 3 * (8192u64 + 128 + 64 + 71 + 5);
    let admission = 4 * 71 + 4 * 8 + 262_144 + 65_536;
    assert_eq!(2 * archive + admission, 378_756);
    ready_floor
        .checked_add(378_756)
        .and_then(|bytes| bytes.checked_add(150 * representations as u64))
        .filter(|bytes| *bytes <= 1024 * 1024 * 1024)
        .expect("one-file C10 metadata bound must remain representable")
}

#[allow(clippy::too_many_arguments)]
fn exercise_c10_floor_sql(
    source: &Connection,
    destination: &Connection,
    family: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
    base_floor: u64,
    q0: u64,
    at_floor: bool,
) {
    assert_eq!(expected.files().len(), 1);
    assert!(expected.directories().is_empty());
    assert!(expected.deletions().is_empty());
    assert_eq!(
        expected.header().image_digest,
        inputs
            .source_staging
            .plan
            .source_prefix
            .source
            .version()
            .effective_image_digest
    );
    assert_eq!(expected.header().file_bytes, expected.files()[0].bytes);
    assert_eq!(inputs.data.capture.changes.len(), 1);
    let threshold = independent_c10_floor(inputs.metadata_floor, inputs.data.required.len());
    let quota = inputs.source_staging.plan.quota_bytes;
    let metadata = quota.checked_sub(q0).unwrap();
    assert_eq!(metadata, threshold - u64::from(!at_floor));

    // Recompute Q0 independently of managed_budget::quota before using it to
    // isolate the metadata guard. Content may already exist in B's CAS group.
    let mut exact_q0 = 65_536u64 + expected.header().file_bytes;
    for manifest in inputs.data.required {
        let payload = canonical_json(manifest).unwrap().len() as u64;
        let overhead = 2 * payload + 8192 + 2048 + 256 * manifest.chunks().len() as u64;
        exact_q0 = exact_q0
            .checked_add(manifest.byte_length())
            .unwrap()
            .checked_add(overhead)
            .unwrap();
    }
    assert_eq!(q0, exact_q0);
    let h = (inputs.protection.payload().len() as u64)
        .checked_add(256 * inputs.data.required.len() as u64)
        .unwrap()
        .checked_add(65_536)
        .unwrap();
    assert!(
        metadata >= h.checked_add(base_floor).unwrap(),
        "the earlier target guard must pass"
    );

    // Derive D from this fixture's durable exact-journal records, then require
    // the precise reservation identity Q-D. Never edit remaining in isolation.
    let debit = {
        let mut statement = family.prepare(
            "SELECT payload FROM exact_installations WHERE reservation=?1 ORDER BY manifest_id LIMIT 100001",
        ).unwrap();
        let payloads = statement
            .query_map([&inputs.target.binding().reservation], |row| {
                row.get::<_, Vec<u8>>(0)
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(payloads.len(), inputs.data.required.len());
        payloads.iter().fold(0u64, |sum, payload| {
            let record: serde_json::Value = serde_json::from_slice(payload).unwrap();
            assert_eq!(record["phase"], "published");
            sum.checked_add(record["debitBytes"].as_u64().unwrap())
                .unwrap()
        })
    };
    let remaining: u64 = family
        .query_row(
            "SELECT remaining FROM reservations WHERE id=?1 AND owner=?2 AND operation=?3",
            params![
                inputs.target.binding().reservation,
                inputs.target.binding().owner.as_str(),
                inputs.target.binding().operation
            ],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, quota.checked_sub(debit).unwrap());
    let after_checkout = q0
        .checked_add(metadata)
        .unwrap()
        .checked_sub(debit)
        .unwrap()
        .checked_sub(expected.header().file_bytes)
        .unwrap();
    assert_eq!(
        after_checkout,
        remaining.checked_sub(expected.header().file_bytes).unwrap()
    );
    let ready_needed = h
        .checked_add(inputs.metadata_floor)
        .unwrap()
        .checked_add(expected.maximum_encoded_bytes())
        .unwrap();
    assert!(
        after_checkout >= ready_needed,
        "the current Ready8 envelope must pass"
    );
    assert!(
        after_checkout >= h.checked_add(threshold).unwrap(),
        "both cases must already satisfy the later C10 remaining guard"
    );

    let source_before = dump(source);
    let destination_before = dump(destination);
    let family_before = dump(family);
    destination
        .execute_batch("SAVEPOINT ready8_positive_control")
        .unwrap();
    assert_eq!(
        finalize_ordinary_checkout_or_replay(source, destination, family, inputs, expected, None)
            .unwrap(),
        ReadyEffect::Finalized
    );
    read_ordinary_checkout_ready(source, destination, family, inputs, expected, None).unwrap();
    destination
        .execute_batch("ROLLBACK TO ready8_positive_control; RELEASE ready8_positive_control")
        .unwrap();
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), destination_before);
    assert_eq!(dump(family), family_before);

    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    source.execute_batch("PRAGMA query_only=ON").unwrap();
    destination.execute_batch("PRAGMA query_only=ON").unwrap();
    family.execute_batch("PRAGMA query_only=ON").unwrap();
    let observed =
        read_ordinary_checkout_progress(source, destination, family, inputs, expected, None);
    if at_floor {
        let journal = observed
            .unwrap()
            .expect("the complete SQL journal is present");
        assert!(journal.is_complete());
        assert_eq!(
            journal.rows(),
            checkout_journal::read(destination, expected, None)
                .unwrap()
                .rows()
        );
    } else {
        let error = observed
            .err()
            .expect("T-1 must reach the C10 metadata guard");
        assert!(matches!(
            &error,
            super::super::recovery::error::ProviderBootstrapCause::Validation(_)
        ));
        assert_eq!(
            error.to_string(),
            invalid("admission fixed metadata envelope insufficient or exceeds 1GiB").to_string()
        );
    }
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), destination_before);
    assert_eq!(dump(family), family_before);
    source.execute_batch("PRAGMA query_only=OFF").unwrap();
    destination.execute_batch("PRAGMA query_only=OFF").unwrap();
    family.execute_batch("PRAGMA query_only=OFF").unwrap();
}

fn exercise_ready_sql(publication: bool) {
    exercise_ready_sql_phases(publication, false, false, None);
}

#[test]
#[ignore = "ROOT source release and audited ordinary source publication SQL child120/outer180 required"]
fn ordinary_source_publication_partitions_receipts_fences_ready_and_replays_historical_published() {
    let test = concat!(module_path!(), "::ordinary_source_publication_partitions_receipts_fences_ready_and_replays_historical_published");
    child::run(test, 120, || {
        child::declare();
        exercise_ready_sql_phases(false, true, false, None);
    });
}

#[test]
#[ignore = "ROOT source release and audited Ordinary10 SQL child120/outer180 required"]
fn ordinary_autonomy_keeps_uncopied_selected_base_and_current_representations_through_admission10()
{
    let test = concat!(module_path!(), "::ordinary_autonomy_keeps_uncopied_selected_base_and_current_representations_through_admission10");
    child::run(test, 120, || {
        child::declare();
        exercise_ready_sql_phases(false, false, true, None);
    });
}

// Pure preparation shared by the existing SQL recipes and the C10 boundary
// recipe. No connection, journal effect or family mutation is accepted here.
struct ReadySqlPreparation {
    derived: bootstrap::ManagedPlanData,
    planned: ProviderTransitionIntentRecord,
    target_marker: format::Marker,
    copying: ProviderTransitionIntentRecord,
    staging: ProviderTransitionIntentRecord,
    destination_staging: ProviderTransitionIntentRecord,
    expected: ExpectedCheckout,
    base_floor: u64,
    protection: ExactProtection,
    receipt: ProviderTransitionReceiptRecord,
    ready: ProviderTransitionIntentRecord,
    overlay: Vec<ManagedReadyOverlay>,
    claims: [ManagedReadyClaim; 1],
    floor: u64,
}

#[allow(clippy::too_many_arguments)]
fn prepare_ready_sql_data(
    marker: &format::Marker,
    prefix: &ProviderImportPrefixRecord,
    current: &ProjectionSnapshot,
    selected: &layrs_core::projection::ProjectionSelection,
    base: &FileManifest,
    files: &[ImageFile<'_>],
    historical: &[RequiredManifest],
    required: &[FileManifest],
    destination_identity: crate::projection::ProjectionIdentity,
    image: &str,
    copied_bytes: u64,
    family_marker_digest: &str,
    metadata: u64,
) -> Box<ReadySqlPreparation> {
    assert_eq!(
        files.len(),
        1,
        "this SQL fixture has exactly one image file"
    );
    let version = prefix.source.version();
    let image_manifest = files[0].manifest;
    let derived = bootstrap::derive_ordinary_data(
        prefix,
        current,
        selected,
        &version.selection_digest,
        &[base],
        files,
        &[],
        destination_identity.clone(),
        "C:\\ordinary-ready-parent",
        "fs:v1:0000000000000001:0000000000000040",
        LEAF,
        TX,
        historical,
        required,
        metadata,
    )
    .unwrap();
    let planned = ProviderTransitionIntentRecord {
        schema_version: "layrs.projection-provider-transition-intent.v1".into(),
        transaction_id: TX.into(),
        role: TransitionRole::Source,
        state: TransitionState::Planned,
        local_generation: version.generation,
        plan: serde_json::from_slice(&canonical_json(&derived.plan).unwrap()).unwrap(),
        destination: None,
        reservation: None,
        receipt_id: None,
    };
    let mut value = serde_json::to_value(marker).unwrap();
    value["identity"] = serde_json::to_value(&destination_identity).unwrap();
    value["checkoutRoot"] = format!("C:\\ordinary-ready-parent\\{LEAF}").into();
    value["checkoutIdentity"] = "fs:v1:0000000000000001:0000000000000041".into();
    value["storeIdentity"] = "fs:v1:0000000000000001:0000000000000042".into();
    value["metadataIdentity"] = "fs:v1:0000000000000001:0000000000000043".into();
    value["importDigest"] = prefix.digest().unwrap().into();
    value["shared"]["metadataPath"] = marker
        .shared
        .as_ref()
        .unwrap()
        .root
        .join("worktrees")
        .join(
            blake3::hash(destination_identity.worktree_id.as_str().as_bytes())
                .to_hex()
                .to_string(),
        )
        .join("metadata")
        .to_str()
        .unwrap()
        .into();
    value["shared"]["databaseIdentity"] = "1:0:9000".into();
    let target_marker = format::parse_marker_bytes(&canonical_json(&value).unwrap()).unwrap();
    let mut copying = ProviderTransitionIntentRecord::decode(&planned.encode().unwrap()).unwrap();
    copying.state = TransitionState::Copying;
    copying.destination = Some(TransitionDestinationProof {
        backend: TransitionBackendKind::Ordinary,
        format: target_marker.clone(),
        private_database_identity: "1:0:9000".into(),
        format_file_identity: "1:0:9001".into(),
        family_marker_digest: family_marker_digest.to_owned(),
        native_control_identity_hex: None,
        native_root_identity: None,
    });
    let staging = ordinary_transition::proposed_staging(&copying).unwrap();
    let mut destination_staging =
        ProviderTransitionIntentRecord::decode(&staging.encode().unwrap()).unwrap();
    destination_staging.role = TransitionRole::Destination;
    destination_staging.local_generation = 0;
    destination_staging.validate_fields().unwrap();
    let checkout_files = [CheckoutFile {
        path: files[0].path.clone(),
        manifest_id: image_manifest.id().clone(),
        file_id: image_manifest.file_id().clone(),
        bytes: image_manifest.byte_length(),
        executable: files[0].executable,
    }];
    let expected =
        ExpectedCheckout::ordinary(&destination_staging, &checkout_files, &[], image, None)
            .unwrap();
    let base_floor = managed_budget::base_metadata_floor(
        copied_bytes,
        &derived.observation,
        &derived.placement,
        &derived.capture,
        &derived.plan,
        selected,
        managed_budget::intent_size(&staging).unwrap(),
        managed_budget::intent_size(&destination_staging).unwrap(),
    )
    .unwrap();
    let proof = copying.destination.as_ref().unwrap();
    let shared = target_marker.shared.as_ref().unwrap();
    let protection = ExactProtection::new(
        &shared.incarnation,
        &target_marker.identity.worktree_id,
        &shared.database_identity,
        TX,
        required,
        None,
    )
    .unwrap();
    let rowset = |r: &super::super::records::transition::TransitionRowset| ImportRowset {
        count: r.count,
        digest: r.digest.clone(),
    };
    let receipt = ProviderTransitionReceiptRecord {
        denial_coverage: None,
        schema_version: "layrs.projection-provider-transition-receipt.v1".into(),
        owner_identity_digest: proof.owner_digest(&derived.plan).unwrap(),
        transaction_id: TX.into(),
        transition_plan_digest: derived.plan.digest().unwrap(),
        destination_proof_digest: proof.digest(&derived.plan).unwrap(),
        source_prefix_id: prefix.digest().unwrap(),
        source_prefix: ProviderImportPrefixRecord::decode(&prefix.encode().unwrap()).unwrap(),
        parent_receipt_id: prefix.ancestry_root_receipt.clone(),
        source_identity: marker.identity.clone(),
        destination_identity,
        imported_max_generation: version.generation,
        destination_observation_generation: derived.plan.observation.generation,
        destination_observation_digest: derived.observation_digest.clone(),
        destination_placement_generation: derived.plan.placement_generation,
        destination_placement_digest: derived.placement_digest.clone(),
        imported_capture_count: version.capture_count,
        destination_capture_sequence: derived.plan.capture_sequence,
        destination_capture_digest: derived.capture.digest.clone(),
        protected_manifests: rowset(&derived.plan.protected_manifests),
        content_mappings: prefix.closure.content_mappings.clone(),
        companion_id: None,
        companion_files: rowset(&derived.plan.companion_files),
        companion_deletions: rowset(&derived.plan.companion_deletions),
    };
    let mut ready =
        ProviderTransitionIntentRecord::decode(&destination_staging.encode().unwrap()).unwrap();
    ready.state = TransitionState::Ready;
    ready.receipt_id = Some(receipt.digest().unwrap());
    let overlay: Vec<_> = derived
        .capture
        .changes
        .iter()
        .map(|change| ManagedReadyOverlay {
            path: change.path.clone(),
            manifest_id: change
                .replacement
                .as_ref()
                .map(|_| image_manifest.id().clone()),
        })
        .collect();
    let claims = [ManagedReadyClaim {
        root_id: protection.id().into(),
        digest: protection.id().into(),
        kind: "provider-import".into(),
        owner: TX.into(),
    }];
    let floor = managed_budget::complete_metadata_floor(
        base_floor, &ready, &receipt, &claims[0], required, None,
    )
    .unwrap();
    Box::new(ReadySqlPreparation {
        derived,
        planned,
        target_marker,
        copying,
        staging,
        destination_staging,
        expected,
        base_floor,
        protection,
        receipt,
        ready,
        overlay,
        claims,
        floor,
    })
}

fn exercise_ready_sql_phases(
    publication: bool,
    source_publication: bool,
    autonomy: bool,
    c10_boundary: Option<bool>,
) {
    with_admitted_source_family_sql(|source, marker, supplied, family, family_marker| {
        let actual = admitted_ordinary::inspect(source, marker, None).unwrap();
        let retained = AdmittedFamilyContext::new(&actual).unwrap();
        family
            .execute(
                "UPDATE worktrees SET state='ready' WHERE id=?1",
                [marker.identity.worktree_id.as_str()],
            )
            .unwrap();
        published_group(family, retained.binding(), supplied, true);
        retained
            .validate_catalog(family, family_marker, None)
            .unwrap();
        let (preimage, history) =
            ordinary_destination::tests::append_restore(source, marker, supplied);
        let mut known: Vec<_> = supplied.iter().cloned().chain([preimage]).collect();
        let alternate = if autonomy {
            // New selected F has no capture/restore mapping. M1 sorts BEFORE
            // B's exact M0, so C's FileId-primary-key mapping cannot recover M0.
            use layrs_core::scm::{Chunk, ChunkingDescriptor};
            // Also force distinct FileIds whose manifest ordering is reversed.
            // The family root orders by ManifestId, whereas the local witness
            // orders by (FileId, ManifestId). Both orders must remain valid.
            let historical = known.last().unwrap();
            let (first, second) = (0..128)
                .find_map(|seed| {
                    let bytes = format!("selected base absent from history {seed}");
                    let first = FileManifest::from_chunks(
                        ChunkingDescriptor::single(),
                        &[Chunk::new(bytes.as_bytes().to_vec()).unwrap()],
                    )
                    .unwrap();
                    let second = FileManifest::from_chunks(
                        ChunkingDescriptor::deterministic_fixed(1048576).unwrap(),
                        &[Chunk::new(bytes.into_bytes()).unwrap()],
                    )
                    .unwrap();
                    let reversed = [&first, &second].iter().any(|candidate| {
                        candidate.file_id() != historical.file_id()
                            && candidate.file_id().cmp(historical.file_id())
                                != candidate.id().cmp(historical.id())
                    });
                    reversed.then_some((first, second))
                })
                .expect("bounded fixture must find opposite file/manifest order");
            assert_eq!(first.file_id(), second.file_id());
            assert_ne!(first.id(), second.id());
            let (base, current) = if first.id() > second.id() {
                (first, second)
            } else {
                (second, first)
            };
            let old = database::current(source).unwrap();
            let mut snapshot = old.snapshot.clone();
            snapshot.entries[0].target = Some(TreeEntryTarget::File {
                file_id: base.file_id().clone(),
            });
            let generation = old.generation + 1;
            let payload = snapshot.canonical_bytes().unwrap();
            let digest = database::digest(generation, Some(&old.digest), &payload);
            source
                .execute(
                    "INSERT INTO projection_snapshots VALUES(?1,?2,?3,?4)",
                    params![generation, digest, old.digest, payload],
                )
                .unwrap();
            source
                .execute("UPDATE projection_head SET generation=?1", [generation])
                .unwrap();
            source
                .execute(
                    "INSERT INTO projection_content VALUES(?1,?2,?3)",
                    params![
                        base.file_id().as_str(),
                        base.id().as_str(),
                        base.byte_length()
                    ],
                )
                .unwrap();
            known.push(base.clone());
            known.push(current.clone());
            Some((base, current))
        } else {
            None
        };
        let current = database::current(source).unwrap();
        let selected = selection::at(source, current.generation).unwrap();
        let entry = &current.snapshot.entries[0];
        let Some(TreeEntryTarget::File { file_id: base_id }) = &entry.target else {
            panic!("fixture file required")
        };
        let image_manifest = alternate
            .as_ref()
            .map(|(_, current)| current)
            .unwrap_or_else(|| supplied.last().unwrap());
        if autonomy {
            assert_eq!(base_id, image_manifest.file_id());
        } else {
            assert_ne!(base_id, image_manifest.file_id());
        }
        let image = bootstrap::recovery_image_digest(
            &[ProviderRecoveryExportFile {
                path: entry.path.clone(),
                file_id: image_manifest.file_id().clone(),
                length: image_manifest.byte_length(),
                executable: Some(entry.executable),
                class: ProviderRecoveryExportClass::ManagedCandidate,
            }],
            &[],
            &[],
        )
        .unwrap();
        let image = image.strip_prefix("blake3:").unwrap();
        let mut destination_identity = marker.identity.clone();
        destination_identity.worktree_id =
            WorktreeId::parse("worktree:ordinary-ready-sql-c").unwrap();
        let (prefix, closure) =
            plan_admitted_import(source, marker, destination_identity.clone(), image, None)
                .unwrap();
        assert_eq!(prefix.closure.restore_history.as_ref().unwrap().count, 1);
        let mappings = closure
            .copied_content_mappings(source, &prefix, None)
            .unwrap();
        if autonomy {
            assert!(!mappings.contains_key(base_id));
        }
        let historical: Vec<_> = mappings
            .iter()
            .map(|(file, (id, bytes))| RequiredManifest {
                file_id: file.clone(),
                manifest_id: id.clone(),
                bytes: *bytes,
            })
            .collect();
        let mut union = BTreeMap::new();
        for (file, (id, bytes)) in &mappings {
            let manifest = known
                .iter()
                .find(|m| m.file_id() == file && m.id() == id && m.byte_length() == *bytes)
                .unwrap();
            union.insert((file.clone(), id.clone()), manifest.clone());
        }
        let base = known.iter().find(|m| m.file_id() == base_id).unwrap();
        for manifest in [base, image_manifest] {
            union.insert(
                (manifest.file_id().clone(), manifest.id().clone()),
                manifest.clone(),
            );
        }
        let required: Vec<_> = union.into_values().collect();
        if autonomy {
            let mut root_order: Vec<_> = required.iter().collect();
            root_order.sort_unstable_by_key(|manifest| manifest.id());
            assert!(root_order
                .windows(2)
                .any(|pair| pair[0].file_id() > pair[1].file_id()));
        }
        let version = prefix.source.version();
        let stored_selection_digest: String = source.query_row(
                "SELECT digest FROM projection_selections WHERE generation<=?1 ORDER BY generation DESC LIMIT 1",
                [current.generation], |row| row.get(0),
            ).unwrap();
        assert_eq!(version.selection_digest, stored_selection_digest);
        assert_ne!(
            selection::initial_digest(&selected).unwrap(),
            version.selection_digest,
            "Ready must accept the authenticated versioned selection digest",
        );
        let files = [ImageFile {
            path: &entry.path,
            manifest: image_manifest,
            executable: entry.executable,
            expected: Some((base_id.clone(), entry.executable)),
        }];
        let copied_bytes = closure.metadata_bytes(source, &prefix, None).unwrap();
        let prepare = |metadata| {
            prepare_ready_sql_data(
                marker,
                &prefix,
                &current.snapshot,
                &selected,
                base,
                &files,
                &historical,
                &required,
                destination_identity.clone(),
                image,
                copied_bytes,
                &retained.binding().family_marker_digest,
                metadata,
            )
        };
        let mut prepared = prepare(4 * 1024 * 1024);
        if let Some(at_floor) = c10_boundary {
            assert!(!publication && !source_publication && !autonomy);
            let mut widths = BTreeSet::new();
            let mut stable = false;
            // Q fits i64, hence has at most 19 decimal digits. All changing
            // digests have fixed width; only Q's decimal width affects F.
            // A nonfinal repeated width would contradict that invariant.
            for _ in 0..20 {
                assert_eq!(prepared.expected.files().len(), 1);
                assert!(prepared.expected.directories().is_empty());
                assert_eq!(prepared.expected.header().image_digest, image);
                let threshold = independent_c10_floor(prepared.floor, required.len());
                let metadata = threshold - u64::from(!at_floor);
                let actual = prepared.derived.quota_bytes
                    - managed_budget::quota(&required, [image_manifest.byte_length()], 0).unwrap();
                if metadata == actual {
                    stable = true;
                    break;
                }
                assert!(
                    widths.insert(prepared.derived.quota_bytes.to_string().len()),
                    "C10 sizing width repeated before stability"
                );
                prepared = prepare(metadata);
            }
            assert!(
                stable,
                "bounded decimal-width sizing must converge before effects"
            );
        }
        let ReadySqlPreparation {
            derived,
            planned,
            target_marker,
            copying,
            staging,
            destination_staging,
            expected,
            base_floor,
            protection,
            receipt,
            ready,
            overlay,
            claims,
            floor,
        } = *prepared;
        assert_eq!(derived.capture.changes.len(), if autonomy { 0 } else { 1 });
        ordinary_transition::apply_planned(source, marker, &prefix, &planned, None).unwrap();
        ordinary_transition::apply_copying(source, marker, &prefix, &copying, None).unwrap();
        let bare = BareConnection::reference().unwrap();
        let c = bare.connection();
        c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
            .unwrap();
        ordinary_destination::initialize_or_replay_copied(
            c,
            &target_marker,
            &OrdinaryCopyInputs::new(source, marker, &copying, &closure),
            None,
        )
        .unwrap();
        ordinary_transition::apply_staging(source, marker, &prefix, &staging, None).unwrap();
        let source_input = OrdinaryCopyInputs::new(source, marker, &staging, &closure);
        ordinary_destination::staging::apply_staging(c, &target_marker, &source_input, None)
            .unwrap();
        assert_eq!(
            source_input
                .authenticate_staging_source(c, &target_marker, None)
                .unwrap(),
            destination_staging
        );
        complete(c, &expected);
        let q0 = managed_budget::quota(&required, [image_manifest.byte_length()], 0).unwrap();
        let target = ExportReserveOnlyTarget::new(
            &retained,
            &staging,
            &destination_staging,
            &required,
            q0,
            base_floor,
            None,
        )
        .unwrap();
        family
            .execute(
                "INSERT INTO worktrees VALUES(?1,?2,?3,?4,'initializing')",
                params![
                    target_marker.identity.worktree_id.as_str(),
                    target_marker
                        .shared
                        .as_ref()
                        .unwrap()
                        .metadata_path
                        .to_str()
                        .unwrap(),
                    target_marker.shared.as_ref().unwrap().database_identity,
                    target_marker.bytes().unwrap()
                ],
            )
            .unwrap();
        retained
            .reserve_export_only(family, family_marker, &target, None)
            .unwrap();
        published_group(family, target.binding(), &required, false);
        protection
            .require_binding(target.binding(), &required, None)
            .unwrap();
        retained
            .protect_export(family, family_marker, &target, &required, &protection, None)
            .unwrap();
        let data = ManagedReadyData {
            observation: &derived.observation,
            placement: &derived.placement,
            capture: &derived.capture,
            overlay: &overlay,
            required: &required,
            receipt: &receipt,
            ready: &ready,
            claims: &claims,
        };
        let mut inputs = OrdinaryReadyInputs {
            marker: &target_marker,
            source_marker: marker,
            source_staging: &staging,
            closure: &closure,
            retained: &retained,
            target: &target,
            protection: &protection,
            family_marker_bytes: family_marker,
            metadata_floor: floor,
            data: &data,
        };
        if let Some(at_floor) = c10_boundary {
            exercise_c10_floor_sql(
                source, c, family, &inputs, &expected, base_floor, q0, at_floor,
            );
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
            return;
        }
        if publication {
            exercise_finalizable_sql(source, c, family, &inputs, &expected);
            assert!(read_ordinary_private_ready(c, &target_marker, family, None).is_err());
            assert!(
                read_ordinary_family_publication(source, c, family, &inputs, &expected, None,)
                    .is_err()
            );
            assert!(
                read_ordinary_checkout_ready_for_publication(
                    source, c, family, &inputs, &expected, None
                )
                .is_err(),
                "canonical Ready DATA must not authenticate an actual Staging C"
            );
        }
        let source_before = dump(source);
        let family_before = dump(family);
        let before = dump(c);
        let changes = c.total_changes();
        inputs.metadata_floor += 1;
        assert!(
            finalize_ordinary_checkout_or_replay(source, c, family, &inputs, &expected, None)
                .is_err()
        );
        inputs.metadata_floor -= 1;
        assert_eq!(dump(c), before);
        assert_eq!(c.total_changes(), changes);
        c.execute_batch("SAVEPOINT incomplete").unwrap();
        assert_eq!(
            c.execute(
                "DELETE FROM projection_prejournal WHERE path=?1",
                [expected.sql_key(RowKey::Header).unwrap()],
            )
            .unwrap(),
            1
        );
        let before = dump(c);
        let changes = c.total_changes();
        assert!(
            finalize_ordinary_checkout_or_replay(source, c, family, &inputs, &expected, None)
                .is_err()
        );
        assert_eq!(dump(c), before);
        assert_eq!(c.total_changes(), changes);
        c.execute_batch("ROLLBACK TO incomplete; RELEASE incomplete")
            .unwrap();
        assert_eq!(
            finalize_ordinary_checkout_or_replay(source, c, family, &inputs, &expected, None)
                .unwrap(),
            ReadyEffect::Finalized
        );
        assert_eq!(dump(source), source_before);
        assert_eq!(dump(family), family_before);
        assert_eq!(
            c.query_row("SELECT payload FROM projection_restore_history", [], |r| {
                r.get::<_, Vec<u8>>(0)
            })
            .unwrap(),
            history
        );
        read_ordinary_checkout_ready(source, c, family, &inputs, &expected, None).unwrap();
        if publication {
            assert!(
                read_ordinary_checkout_progress(source, c, family, &inputs, &expected, None,)
                    .is_err(),
                "progress reader must not reinterpret actual private Ready as Staging"
            );
            assert!(
                read_ordinary_checkout_finalizable(source, c, family, &inputs, &expected, None,)
                    .is_err(),
                "finalizable reader must not reinterpret actual private Ready as Staging"
            );
            let actual =
                read_ordinary_family_publication(source, c, family, &inputs, &expected, None)
                    .unwrap();
            assert_eq!(
                actual.state().member,
                crate::projection::shared::admitted_catalog::ExportPublicationMember::Initializing
            );
            assert_eq!(
                actual.snapshot().journal().rows(),
                checkout_journal::read(c, &expected, None).unwrap().rows()
            );
        }
        for sql in [
            "UPDATE projection_restore_history SET payload=x'00'",
            "UPDATE projection_head SET generation=0",
            "UPDATE projection_provider_transitions SET receipt_id=NULL",
        ] {
            c.execute_batch("SAVEPOINT corrupt").unwrap();
            assert_eq!(c.execute(sql, []).unwrap(), 1);
            let before = dump(c);
            let changes = c.total_changes();
            assert!(
                read_ordinary_checkout_ready(source, c, family, &inputs, &expected, None).is_err()
            );
            assert!(finalize_ordinary_checkout_or_replay(
                source, c, family, &inputs, &expected, None
            )
            .is_err());
            assert_eq!(dump(c), before);
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK TO corrupt; RELEASE corrupt")
                .unwrap();
        }
        c.execute_batch("COMMIT; PRAGMA foreign_keys=OFF; PRAGMA query_only=ON; BEGIN DEFERRED")
            .unwrap();
        let before = dump(c);
        let changes = c.total_changes();
        assert_eq!(
            finalize_ordinary_checkout_or_replay(source, c, family, &inputs, &expected, None)
                .unwrap(),
            ReadyEffect::AlreadyReady
        );
        read_ordinary_checkout_ready(source, c, family, &inputs, &expected, None).unwrap();
        if publication {
            let actual =
                read_ordinary_family_publication(source, c, family, &inputs, &expected, None)
                    .unwrap();
            assert_eq!(
                actual.state().member,
                crate::projection::shared::admitted_catalog::ExportPublicationMember::Initializing
            );
        }
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(finalize_ordinary_checkout_or_replay(
            source,
            c,
            family,
            &inputs,
            &expected,
            Some(&cancel)
        )
        .is_err());
        assert_eq!(dump(c), before);
        assert_eq!(c.total_changes(), changes);
        assert_eq!(dump(source), source_before);
        assert_eq!(dump(family), family_before);
        assert_eq!(
            retained
                .validate_export_protection(
                    family,
                    family_marker,
                    &target,
                    &required,
                    &protection,
                    None
                )
                .unwrap()
                .1
                .root,
            ProtectionPresence::Present
        );
        assert_eq!(
            read_admitted_import(source, marker, None)
                .unwrap()
                .outgoing()
                .iter()
                .find(|r| r.transaction_id == TX)
                .unwrap()
                .state,
            TransitionState::Staging
        );
        if publication {
            exercise_publication(source, c, family, &inputs, &expected);
        }
        if source_publication {
            exercise_source_publication(source, c, family, &inputs, &expected);
        }
        if autonomy {
            exercise_ordinary_admission(
                source,
                c,
                family,
                &inputs,
                &expected,
                alternate.as_ref().unwrap(),
            );
        }
        c.execute_batch("ROLLBACK").unwrap();
        bare.finish(Ok(())).unwrap();
    });
}

fn exercise_finalizable_sql(
    source: &Connection,
    destination: &Connection,
    family: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
) {
    use crate::projection::shared::read_manifest_exact;
    let source_before = dump(source);
    let destination_before = dump(destination);
    let family_before = dump(family);
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    source.execute_batch("PRAGMA query_only=ON").unwrap();
    destination.execute_batch("PRAGMA query_only=ON").unwrap();
    family.execute_batch("PRAGMA query_only=ON").unwrap();
    let journal =
        read_ordinary_checkout_finalizable(source, destination, family, inputs, expected, None)
            .unwrap();
    assert!(journal.is_complete());
    let progress =
        read_ordinary_checkout_progress(source, destination, family, inputs, expected, None)
            .unwrap()
            .expect("complete journal remains observed progress");
    assert!(progress.is_complete());
    assert_eq!(progress.rows(), journal.rows());
    assert_eq!(
        journal.rows(),
        checkout_journal::read(destination, expected, None)
            .unwrap()
            .rows()
    );
    for manifest in inputs.data.required {
        assert_eq!(
            read_manifest_exact(family, manifest.id(), None)
                .unwrap()
                .as_ref(),
            Some(manifest)
        );
    }
    let original = &inputs.data.required[0];
    let alternate = FileManifest::new(
        original.file_id().clone(),
        original.byte_length(),
        layrs_core::scm::ChunkingDescriptor::deterministic_fixed(524288).unwrap(),
        original.chunks().to_vec(),
    )
    .unwrap();
    assert_eq!(alternate.file_id(), original.file_id());
    assert_eq!(alternate.byte_length(), original.byte_length());
    assert_ne!(alternate.id(), original.id());
    assert!(!inputs
        .data
        .required
        .iter()
        .any(|manifest| manifest.id() == alternate.id()));
    assert!(
        read_manifest_exact(family, alternate.id(), None)
            .unwrap()
            .is_none(),
        "an absent exact ManifestId must not fall back to the retained same-FileId body"
    );
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(read_ordinary_checkout_progress(
        source,
        destination,
        family,
        inputs,
        expected,
        Some(&cancel),
    )
    .is_err());
    assert!(read_ordinary_checkout_finalizable(
        source,
        destination,
        family,
        inputs,
        expected,
        Some(&cancel),
    )
    .is_err());
    assert!(read_manifest_exact(family, original.id(), Some(&cancel)).is_err());
    let missing_data = ManagedReadyData {
        required: &inputs.data.required[1..],
        ..*inputs.data
    };
    let missing = OrdinaryReadyInputs {
        data: &missing_data,
        ..*inputs
    };
    assert!(
        read_ordinary_checkout_progress(source, destination, family, &missing, expected, None,)
            .is_err()
    );
    assert!(read_ordinary_checkout_finalizable(
        source,
        destination,
        family,
        &missing,
        expected,
        None,
    )
    .is_err());
    let mut substituted_required = inputs.data.required.to_vec();
    substituted_required[0] = alternate;
    let substituted_data = ManagedReadyData {
        required: &substituted_required,
        ..*inputs.data
    };
    let substituted = OrdinaryReadyInputs {
        data: &substituted_data,
        ..*inputs
    };
    assert!(read_ordinary_checkout_progress(
        source,
        destination,
        family,
        &substituted,
        expected,
        None,
    )
    .is_err());
    assert!(read_ordinary_checkout_finalizable(
        source,
        destination,
        family,
        &substituted,
        expected,
        None,
    )
    .is_err());
    let wrong_floor = OrdinaryReadyInputs {
        metadata_floor: inputs.metadata_floor + 1,
        ..*inputs
    };
    assert!(read_ordinary_checkout_progress(
        source,
        destination,
        family,
        &wrong_floor,
        expected,
        None,
    )
    .is_err());
    let wrong_marker = OrdinaryReadyInputs {
        source_marker: inputs.marker,
        ..*inputs
    };
    assert!(read_ordinary_checkout_progress(
        source,
        destination,
        family,
        &wrong_marker,
        expected,
        None,
    )
    .is_err());
    assert!(read_ordinary_checkout_finalizable(
        source,
        destination,
        family,
        &wrong_floor,
        expected,
        None,
    )
    .is_err());
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), destination_before);
    assert_eq!(dump(family), family_before);
    source.execute_batch("PRAGMA query_only=OFF").unwrap();
    destination.execute_batch("PRAGMA query_only=OFF").unwrap();
    family.execute_batch("PRAGMA query_only=OFF").unwrap();

    // SQL-only absent predecessor. No filesystem state or ownership is
    // synthesized; a physical driver must separately validate an empty root.
    destination
        .execute_batch("SAVEPOINT checkout_absent")
        .unwrap();
    let declared_rows = destination
        .execute("DELETE FROM projection_prejournal", [])
        .unwrap();
    assert!(declared_rows > 0);
    let absent = dump(destination);
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    source.execute_batch("PRAGMA query_only=ON").unwrap();
    destination.execute_batch("PRAGMA query_only=ON").unwrap();
    family.execute_batch("PRAGMA query_only=ON").unwrap();
    assert!(
        read_ordinary_checkout_progress(source, destination, family, inputs, expected, None)
            .unwrap()
            .is_none()
    );
    let copy = OrdinaryCopyInputs::new(
        source,
        inputs.source_marker,
        inputs.source_staging,
        inputs.closure,
    );
    assert!(ordinary_destination::read_checkout_progress(
        destination,
        inputs.marker,
        &copy,
        expected,
        None
    )
    .unwrap()
    .is_none());
    assert!(ordinary_destination::read_checkout_staging(
        destination,
        inputs.marker,
        &copy,
        expected,
        None
    )
    .is_err());
    assert!(read_ordinary_checkout_finalizable(
        source,
        destination,
        family,
        inputs,
        expected,
        None
    )
    .is_err());
    assert!(read_ordinary_checkout_progress(
        source,
        destination,
        family,
        inputs,
        expected,
        Some(&cancel)
    )
    .is_err());
    assert!(
        read_ordinary_checkout_progress(source, destination, family, &missing, expected, None)
            .is_err()
    );
    assert!(read_ordinary_checkout_progress(
        source,
        destination,
        family,
        &substituted,
        expected,
        None
    )
    .is_err());
    assert!(read_ordinary_checkout_progress(
        source,
        destination,
        family,
        &wrong_floor,
        expected,
        None
    )
    .is_err());
    assert!(read_ordinary_checkout_progress(
        source,
        destination,
        family,
        &wrong_marker,
        expected,
        None
    )
    .is_err());
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), absent);
    assert_eq!(dump(family), family_before);
    source.execute_batch("PRAGMA query_only=OFF").unwrap();
    destination.execute_batch("PRAGMA query_only=OFF").unwrap();
    family.execute_batch("PRAGMA query_only=OFF").unwrap();
    // Even without a journal, the full retained/protected family is required.
    family
        .execute_batch("SAVEPOINT checkout_absent_family")
        .unwrap();
    assert_eq!(
        family
            .execute(
                "UPDATE worktrees SET state='ready' WHERE id=?1",
                [inputs.target.binding().owner.as_str()]
            )
            .unwrap(),
        1
    );
    let corrupt_family = dump(family);
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    assert!(
        read_ordinary_checkout_progress(source, destination, family, inputs, expected, None)
            .is_err()
    );
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), absent);
    assert_eq!(dump(family), corrupt_family);
    family
        .execute_batch("ROLLBACK TO checkout_absent_family; RELEASE checkout_absent_family")
        .unwrap();

    // A canonical Copying predecessor is a legitimate staging gap, but must
    // never be accepted as checkout progress merely because its journal is empty.
    destination
        .execute_batch("SAVEPOINT checkout_absent_copying")
        .unwrap();
    let raw: Vec<u8> = destination
        .query_row(
            "SELECT payload FROM projection_provider_transitions WHERE id=?1 AND role='destination'",
            [&inputs.source_staging.transaction_id],
            |r| r.get(0),
        )
        .unwrap();
    let mut copying = ProviderTransitionIntentRecord::decode(&raw).unwrap();
    copying.state = TransitionState::Copying;
    copying.reservation = None;
    copying.validate_fields().unwrap();
    assert_eq!(destination.execute(
        "UPDATE projection_provider_transitions SET state='copying',reservation=NULL,payload=?1,digest=?2 WHERE id=?3 AND role='destination'",
        params![copying.encode().unwrap(), copying.digest().unwrap(), copying.transaction_id],
    ).unwrap(), 1);
    let copy = OrdinaryCopyInputs::new(
        source,
        inputs.source_marker,
        inputs.source_staging,
        inputs.closure,
    );
    assert_eq!(
        ordinary_destination::staging::read_stageable(destination, inputs.marker, &copy, None)
            .unwrap(),
        TransitionState::Copying
    );
    let before = (dump(source), dump(destination), dump(family));
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    source.execute_batch("PRAGMA query_only=ON").unwrap();
    destination.execute_batch("PRAGMA query_only=ON").unwrap();
    family.execute_batch("PRAGMA query_only=ON").unwrap();
    assert!(
        read_ordinary_checkout_progress(source, destination, family, inputs, expected, None)
            .is_err()
    );
    assert!(ordinary_destination::read_checkout_progress(
        destination,
        inputs.marker,
        &copy,
        expected,
        None
    )
    .is_err());
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
    assert_eq!((dump(source), dump(destination), dump(family)), before);
    source.execute_batch("PRAGMA query_only=OFF").unwrap();
    destination.execute_batch("PRAGMA query_only=OFF").unwrap();
    family.execute_batch("PRAGMA query_only=OFF").unwrap();
    destination
        .execute_batch("ROLLBACK TO checkout_absent_copying; RELEASE checkout_absent_copying")
        .unwrap();

    // A single opaque key must select strict journal interpretation, not None.
    destination
        .execute_batch("SAVEPOINT checkout_opaque_only")
        .unwrap();
    let header = journal.row(RowKey::Header).unwrap();
    assert_eq!(destination.execute(
        "INSERT INTO projection_prejournal(path,operation,digest,payload,state) VALUES(?1,?2,?3,?4,'owned')",
        params![format!("{}:foreign", expected.sql_key(RowKey::Header).unwrap()), expected.operation(), header.digest().unwrap(), header.encode().unwrap()],
    ).unwrap(), 1);
    let before = (dump(source), dump(destination), dump(family));
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    source.execute_batch("PRAGMA query_only=ON").unwrap();
    destination.execute_batch("PRAGMA query_only=ON").unwrap();
    family.execute_batch("PRAGMA query_only=ON").unwrap();
    assert!(
        read_ordinary_checkout_progress(source, destination, family, inputs, expected, None)
            .is_err()
    );
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
    assert_eq!((dump(source), dump(destination), dump(family)), before);
    source.execute_batch("PRAGMA query_only=OFF").unwrap();
    destination.execute_batch("PRAGMA query_only=OFF").unwrap();
    family.execute_batch("PRAGMA query_only=OFF").unwrap();
    destination
        .execute_batch("ROLLBACK TO checkout_opaque_only; RELEASE checkout_opaque_only")
        .unwrap();

    // Genuine SQL declaration creates the complete typed inventory, all in
    // Declared phases. This remains SQL DATA with no filesystem ownership.
    assert_eq!(
        checkout_journal::declare(destination, expected, None).unwrap(),
        checkout_journal::DeclareEffect::Inserted
    );
    let before = (dump(source), dump(destination), dump(family));
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    source.execute_batch("PRAGMA query_only=ON").unwrap();
    destination.execute_batch("PRAGMA query_only=ON").unwrap();
    family.execute_batch("PRAGMA query_only=ON").unwrap();
    let declared =
        read_ordinary_checkout_progress(source, destination, family, inputs, expected, None)
            .unwrap()
            .expect("declared journal must remain observable");
    assert!(!declared.is_complete());
    for row in declared.rows() {
        assert_eq!(row, &expected.declared_row(row.key()).unwrap());
    }
    assert!(read_ordinary_checkout_finalizable(
        source,
        destination,
        family,
        inputs,
        expected,
        None
    )
    .is_err());
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
    assert_eq!((dump(source), dump(destination), dump(family)), before);
    source.execute_batch("PRAGMA query_only=OFF").unwrap();
    destination.execute_batch("PRAGMA query_only=OFF").unwrap();
    family.execute_batch("PRAGMA query_only=OFF").unwrap();
    destination
        .execute_batch("ROLLBACK TO checkout_absent; RELEASE checkout_absent")
        .unwrap();
    assert_eq!(dump(destination), destination_before);

    source
        .execute_batch("SAVEPOINT checkout_changed_source")
        .unwrap();
    assert_eq!(source.execute(
        "UPDATE projection_provider_transitions SET state='ready' WHERE id=?1 AND role='source' AND state='staging'",
        [&inputs.source_staging.transaction_id],
    ).unwrap(), 1);
    let changed_source = dump(source);
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    assert!(
        read_ordinary_checkout_progress(source, destination, family, inputs, expected, None)
            .is_err()
    );
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
    assert_eq!(dump(source), changed_source);
    assert_eq!(dump(destination), destination_before);
    assert_eq!(dump(family), family_before);
    source
        .execute_batch("ROLLBACK TO checkout_changed_source; RELEASE checkout_changed_source")
        .unwrap();

    // A complete row inventory with a legal intermediate file phase must be
    // refused specifically by the finalizable entry's completion requirement.
    for phase in 0..3 {
        destination
            .execute_batch("SAVEPOINT finalizable_progress")
            .unwrap();
        let mut incomplete = journal.row(RowKey::File(0)).unwrap().clone();
        let checkout_journal::CheckoutEntry::File { state, .. } = &mut incomplete.entry else {
            panic!("fixture file row required")
        };
        let checkout_journal::FileState::Published {
            inode,
            parent_identity,
        } = state
        else {
            panic!("fixture published file required")
        };
        *state = match phase {
            0 => checkout_journal::FileState::Owned {
                inode: inode.clone(),
            },
            1 => checkout_journal::FileState::Verified {
                inode: inode.clone(),
            },
            _ => checkout_journal::FileState::Publishing {
                inode: inode.clone(),
                parent_identity: parent_identity.clone(),
            },
        };
        assert_eq!(
            destination
                .execute(
                    "UPDATE projection_prejournal SET digest=?1,payload=?2 WHERE path=?3",
                    params![
                        incomplete.digest().unwrap(),
                        incomplete.encode().unwrap(),
                        expected.sql_key(RowKey::File(0)).unwrap()
                    ],
                )
                .unwrap(),
            1
        );
        let progress = dump(destination);
        let changes = (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes(),
        );
        let copy = OrdinaryCopyInputs::new(
            source,
            inputs.source_marker,
            inputs.source_staging,
            inputs.closure,
        );
        source.execute_batch("PRAGMA query_only=ON").unwrap();
        destination.execute_batch("PRAGMA query_only=ON").unwrap();
        family.execute_batch("PRAGMA query_only=ON").unwrap();
        let actual =
            read_ordinary_checkout_progress(source, destination, family, inputs, expected, None)
                .unwrap()
                .expect("legal partial journal must remain observable");
        assert!(!actual.is_complete());
        assert_eq!(actual.row(RowKey::File(0)).unwrap(), &incomplete);
        assert!(!ordinary_destination::read_checkout_staging(
            destination,
            inputs.marker,
            &copy,
            expected,
            None
        )
        .unwrap()
        .is_complete());
        let error =
            read_ordinary_checkout_finalizable(source, destination, family, inputs, expected, None)
                .err()
                .expect("legal intermediate journal must not be finalizable");
        assert!(
            error.to_string().contains("complete typed journal"),
            "{error}"
        );
        assert_eq!(
            (
                source.total_changes(),
                destination.total_changes(),
                family.total_changes()
            ),
            changes
        );
        assert_eq!(dump(destination), progress);
        assert_eq!(dump(source), source_before);
        assert_eq!(dump(family), family_before);
        source.execute_batch("PRAGMA query_only=OFF").unwrap();
        destination.execute_batch("PRAGMA query_only=OFF").unwrap();
        family.execute_batch("PRAGMA query_only=OFF").unwrap();
        destination
            .execute_batch("ROLLBACK TO finalizable_progress; RELEASE finalizable_progress")
            .unwrap();
        assert_eq!(dump(destination), destination_before);
    }

    for sql in [
        "DELETE FROM projection_prejournal WHERE path=?1",
        "UPDATE projection_prejournal SET path=path||':foreign' WHERE path=?1",
        "UPDATE projection_prejournal SET payload=x'00' WHERE path=?1",
    ] {
        destination
            .execute_batch("SAVEPOINT checkout_foreign_journal")
            .unwrap();
        assert_eq!(
            destination
                .execute(sql, [expected.sql_key(RowKey::Header).unwrap()])
                .unwrap(),
            1
        );
        let corrupt = dump(destination);
        let changes = (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes(),
        );
        assert!(
            read_ordinary_checkout_progress(source, destination, family, inputs, expected, None)
                .is_err(),
            "progress accepted {sql}"
        );
        assert!(read_ordinary_checkout_finalizable(
            source,
            destination,
            family,
            inputs,
            expected,
            None
        )
        .is_err());
        assert_eq!(
            (
                source.total_changes(),
                destination.total_changes(),
                family.total_changes()
            ),
            changes
        );
        assert_eq!(dump(source), source_before);
        assert_eq!(dump(destination), corrupt);
        assert_eq!(dump(family), family_before);
        destination
            .execute_batch("ROLLBACK TO checkout_foreign_journal; RELEASE checkout_foreign_journal")
            .unwrap();
    }

    let target_owner = inputs.target.binding().owner.as_str();
    for (sql, owner) in [
        (
            "UPDATE worktrees SET state='ready' WHERE id=?1",
            Some(target_owner),
        ),
        (
            "UPDATE worktrees SET marker=x'00' WHERE id=?1",
            Some(target_owner),
        ),
        (
            "UPDATE roots SET payload=x'00' WHERE owner=?1",
            Some(target_owner),
        ),
        (
            "UPDATE reservations SET remaining=remaining+1 WHERE owner=?1",
            Some(target_owner),
        ),
        (
            "UPDATE roots SET payload=x'00' WHERE owner=?1",
            Some(inputs.source_marker.identity.worktree_id.as_str()),
        ),
        ("UPDATE family SET marker=x'00' WHERE singleton=1", None),
    ] {
        family
            .execute_batch("SAVEPOINT finalizable_family")
            .unwrap();
        let changed = match owner {
            Some(owner) => family.execute(sql, [owner]).unwrap(),
            None => family.execute(sql, []).unwrap(),
        };
        assert_eq!(changed, 1);
        let corrupt = dump(family);
        let changes = (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes(),
        );
        assert!(
            read_ordinary_checkout_progress(source, destination, family, inputs, expected, None,)
                .is_err(),
            "progress reader accepted {sql}"
        );
        assert!(read_ordinary_checkout_finalizable(
            source, destination, family, inputs, expected, None,
        ).is_err(), "finalizable reader accepted {sql}");
        assert_eq!(
            (
                source.total_changes(),
                destination.total_changes(),
                family.total_changes()
            ),
            changes
        );
        assert_eq!(dump(family), corrupt);
        assert_eq!(dump(source), source_before);
        assert_eq!(dump(destination), destination_before);
        family
            .execute_batch("ROLLBACK TO finalizable_family; RELEASE finalizable_family")
            .unwrap();
    }
    for sql in [
        "UPDATE manifests SET payload=x'00' WHERE id=?1",
        "DELETE FROM manifest_chunks WHERE manifest_id=?1 AND ordinal=0",
    ] {
        family
            .execute_batch("SAVEPOINT finalizable_exact_manifest")
            .unwrap();
        assert_eq!(family.execute(sql, [original.id().as_str()]).unwrap(), 1);
        let corrupt = dump(family);
        let changes = family.total_changes();
        assert!(read_manifest_exact(family, original.id(), None).is_err());
        let all_changes = (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes(),
        );
        assert!(read_ordinary_checkout_progress(
            source,
            destination,
            family,
            inputs,
            expected,
            None
        )
        .is_err());
        assert_eq!(
            (
                source.total_changes(),
                destination.total_changes(),
                family.total_changes()
            ),
            all_changes
        );
        assert_eq!(family.total_changes(), changes);
        assert_eq!(dump(family), corrupt);
        assert_eq!(dump(source), source_before);
        assert_eq!(dump(destination), destination_before);
        family
            .execute_batch(
                "ROLLBACK TO finalizable_exact_manifest; RELEASE finalizable_exact_manifest",
            )
            .unwrap();
    }
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), destination_before);
    assert_eq!(dump(family), family_before);
}

// This recipe intentionally supplies decoded C Ready and declared capacity
// DATA. It proves the B SQL mutations/partition, not physical C/family or ACK.
fn exercise_ordinary_admission(
    source: &Connection,
    destination: &Connection,
    family: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
    alternate: &(FileManifest, FileManifest),
) {
    use crate::projection::provider::{
        import_validation::{read_destination_prefix, DestinationOriginKind},
        managed_admission::{admit_ordinary_ready, AdmissionWriteEffect},
        ordinary_transition::PublicationCapacity,
        published_handoff::read_ordinary_published_for_ready,
        transition_sql::SourceIntentEffect,
    };
    let (base, current) = alternate;
    assert_eq!(base.file_id(), current.file_id());
    assert!(current.id() < base.id());
    destination
        .execute_batch("ROLLBACK; PRAGMA query_only=OFF; PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
        .unwrap();
    inputs
        .retained
        .publish_export_ready(
            family,
            inputs.family_marker_bytes,
            &inputs.publication_inputs(expected),
            None,
        )
        .unwrap();
    let prefix = read_destination_prefix(destination, inputs.marker, None).unwrap();
    assert_eq!(prefix.origin(), DestinationOriginKind::Ordinary);
    assert!(prefix.source().is_err());
    assert!(prefix.activation().is_err());
    let copied = prefix.copied_content_mappings(destination, None).unwrap();
    assert!(!copied.contains_key(base.file_id()));
    assert_eq!(
        destination
            .query_row(
                "SELECT manifest_id FROM projection_content WHERE file_id=?1",
                [base.file_id().as_str()],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        current.id().as_str()
    );
    assert!(read_managed_ready(destination, inputs.marker, family, None).is_err());
    let ready = read_ordinary_managed_ready(destination, inputs.marker, family, None).unwrap();
    let original_journal = ready.journal().rows().to_vec();
    assert_eq!(ready.origin(), DestinationOriginKind::Ordinary);
    let mut expected_root_order = inputs.data.required.to_vec();
    expected_root_order.sort_unstable_by(|left, right| left.id().cmp(right.id()));
    assert_eq!(ready.required(), expected_root_order);
    assert!(ready
        .required()
        .iter()
        .any(|manifest| manifest.id() == base.id()));
    assert!(ready
        .required()
        .iter()
        .any(|manifest| manifest.id() == current.id()));
    ready.require(destination, family, None).unwrap();
    // The autonomous C8 read is strictly observational.
    let before = dump(destination);
    let changes = destination.total_changes();
    read_ordinary_managed_ready(destination, inputs.marker, family, None).unwrap();
    assert_eq!(destination.total_changes(), changes);
    assert_eq!(dump(destination), before);
    let remaining: u64 = family
        .query_row(
            "SELECT remaining FROM reservations WHERE id=?1",
            [&inputs.target.binding().reservation],
            |row| row.get(0),
        )
        .unwrap();
    let remaining = remaining.checked_sub(expected.header().file_bytes).unwrap();
    let capacity = PublicationCapacity::new(
        inputs.data.ready,
        inputs.data.receipt,
        inputs.metadata_floor + expected.maximum_encoded_bytes(),
        remaining,
    )
    .unwrap();
    inputs
        .protection
        .require_envelope(remaining, capacity.metadata_floor())
        .unwrap();
    assert_eq!(
        ordinary_transition::apply_ready(
            source,
            inputs.source_marker,
            inputs.source_staging,
            ready.ready(),
            ready.receipt(),
            &capacity,
            None,
        )
        .unwrap(),
        SourceIntentEffect::Updated
    );
    let previous =
        ordinary_transition::read_publication_state(source, inputs.source_marker, TX, None)
            .unwrap();
    assert_eq!(
        ordinary_transition::apply_published(
            source,
            inputs.source_marker,
            &previous.intent,
            ready.ready(),
            ready.receipt(),
            &capacity,
            None,
        )
        .unwrap(),
        SourceIntentEffect::Updated
    );
    let published = read_ordinary_published_for_ready(
        source,
        inputs.source_marker,
        ready.ready(),
        ready.receipt(),
        None,
    )
    .unwrap();
    destination
        .execute_batch("SAVEPOINT admission_rollback")
        .unwrap();
    assert_eq!(
        admit_ordinary_ready(
            destination,
            inputs.marker,
            family,
            source,
            &ready,
            &published,
            None
        )
        .unwrap(),
        AdmissionWriteEffect::Admitted
    );
    assert_eq!(
        read_admitted_import(destination, inputs.marker, None)
            .unwrap()
            .private_schema(),
        10
    );
    destination
        .execute_batch("ROLLBACK TO admission_rollback; RELEASE admission_rollback")
        .unwrap();
    assert_eq!(
        destination
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .unwrap(),
        8
    );
    assert!(!destination.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='projection_import_representations')", [], |row| row.get::<_, bool>(0)).unwrap());
    assert_eq!(
        dump(destination),
        before,
        "rollback restores complete C8 footprint and operational journal"
    );
    ready.require(destination, family, None).unwrap();
    assert_eq!(
        admit_ordinary_ready(
            destination,
            inputs.marker,
            family,
            source,
            &ready,
            &published,
            None
        )
        .unwrap(),
        AdmissionWriteEffect::Admitted
    );
    let admitted = read_admitted_import(destination, inputs.marker, None).unwrap();
    assert_eq!(admitted.private_schema(), 10);
    assert_eq!(admitted.required().len(), inputs.data.required.len());
    for manifest in [base, current] {
        assert!(admitted
            .required()
            .iter()
            .any(|row| &row.file_id == manifest.file_id()
                && &row.manifest_id == manifest.id()
                && row.bytes == manifest.byte_length()));
    }
    assert_eq!(admitted.archive().rows(), original_journal.as_slice());
    drop(admitted);
    let terminal_installation =
        crate::projection::shared::admitted_catalog::validate_terminal_ready_catalog(
            family,
            inputs.family_marker_bytes,
            inputs.marker,
            inputs.target.binding(),
            inputs.data.required,
            inputs.protection,
            None,
        )
        .unwrap();
    let detach_terminal = || {
        read_admitted_import(destination, inputs.marker, None)
            .unwrap()
            .into_ordinary_terminal(inputs.data.required, terminal_installation, None)
    };
    let (terminal, terminal_expected, terminal_journal) = detach_terminal().unwrap();
    assert_eq!(terminal.private_schema(), 10);
    assert_eq!(terminal_expected.header(), expected.header());
    assert_eq!(terminal_journal.rows(), original_journal.as_slice());
    crate::projection::provider::managed_admission::tests::assert_legacy_observation_tail(
        destination,
        inputs.marker,
    );
    let terminal_before = dump(destination);
    destination
        .execute_batch("SAVEPOINT coherent_terminal_suffix")
        .unwrap();
    // A canonical local capture may share its predecessor's generation and
    // content. The immutable ingress remains valid; only the terminal fence
    // must reject this later SQL suffix, without needing dirty checkout bytes.
    let mut patch = crate::projection::capture::storage::load_metadata_for_origin(
        destination,
        terminal.ready_capture_sequence(),
        &inputs.marker.identity,
        terminal.ready_generation(),
    )
    .unwrap();
    patch.previous_patch_digest = Some(patch.digest.clone());
    patch.sequence += 1;
    patch.digest = patch.computed_digest().unwrap();
    destination
        .execute(
            "INSERT INTO projection_captures VALUES(?1,?2,?3,?4)",
            params![
                patch.sequence,
                patch.digest,
                patch.projection_generation,
                layrs_core::scm::canonical_json(&patch).unwrap()
            ],
        )
        .unwrap();
    let generic = read_admitted_import(destination, inputs.marker, None).unwrap();
    assert_eq!(generic.private_schema(), 10);
    let changed = dump(destination);
    let changes = destination.total_changes();
    match generic.into_ordinary_terminal(inputs.data.required, terminal_installation, None) {
        Err(error) => assert!(
            error
                .to_string()
                .contains("terminal Ordinary10 table has missing or extra rows"),
            "{error}"
        ),
        Ok(_) => panic!("terminal C10 accepted a coherent ordinary capture suffix"),
    }
    assert_eq!(destination.total_changes(), changes);
    assert_eq!(dump(destination), changed);
    destination
        .execute_batch("ROLLBACK TO coherent_terminal_suffix; RELEASE coherent_terminal_suffix")
        .unwrap();
    assert_eq!(dump(destination), terminal_before);
    detach_terminal().unwrap();
    for sql in [
        "DELETE FROM projection_import_representations WHERE manifest_id=?1",
        "UPDATE projection_import_representations SET bytes=bytes+1 WHERE manifest_id=?1",
    ] {
        destination
            .execute_batch("SAVEPOINT witness_corrupt")
            .unwrap();
        assert_eq!(destination.execute(sql, [base.id().as_str()]).unwrap(), 1);
        let changed = dump(destination);
        let writes = destination.total_changes();
        assert!(read_admitted_import(destination, inputs.marker, None).is_err());
        assert_eq!(destination.total_changes(), writes);
        assert_eq!(dump(destination), changed);
        destination
            .execute_batch("ROLLBACK TO witness_corrupt; RELEASE witness_corrupt")
            .unwrap();
    }
    destination
        .execute_batch("SAVEPOINT witness_extra")
        .unwrap();
    destination
        .execute(
            "INSERT INTO projection_import_representations VALUES(?1,?2,?3)",
            params![
                base.file_id().as_str(),
                format!("blake3:{}", "0".repeat(64)),
                base.byte_length()
            ],
        )
        .unwrap();
    assert!(read_admitted_import(destination, inputs.marker, None).is_err());
    destination
        .execute_batch("ROLLBACK TO witness_extra; RELEASE witness_extra")
        .unwrap();
    destination
        .execute_batch("SAVEPOINT no_migration; PRAGMA user_version=9")
        .unwrap();
    let changed = dump(destination);
    assert!(read_admitted_import(destination, inputs.marker, None).is_err());
    assert_eq!(
        destination
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .unwrap(),
        9
    );
    assert_eq!(dump(destination), changed);
    destination
        .execute_batch("ROLLBACK TO no_migration; RELEASE no_migration")
        .unwrap();
    // Commit C10, invalidate both external snapshots, then prove all permanent
    // provenance and later export planning use C alone. No source proof is
    // refreshed; replay must return before touching these stale holders.
    destination
        .execute_batch("COMMIT; PRAGMA query_only=ON; BEGIN DEFERRED")
        .unwrap();
    source.execute_batch("ROLLBACK").unwrap();
    family.execute_batch("ROLLBACK").unwrap();
    let changes = destination.total_changes();
    assert_eq!(
        admit_ordinary_ready(
            destination,
            inputs.marker,
            family,
            source,
            &ready,
            &published,
            None
        )
        .unwrap(),
        AdmissionWriteEffect::AlreadyAdmitted
    );
    let actual = read_admitted_import(destination, inputs.marker, None).unwrap();
    assert_eq!(actual.private_schema(), 10);
    assert_c10_history_dispatch(destination, inputs.marker);
    let mut target = inputs.marker.identity.clone();
    target.worktree_id = WorktreeId::parse("worktree:ordinary-autonomous-d").unwrap();
    let (outgoing, closure) = plan_admitted_import(
        destination,
        inputs.marker,
        target,
        &inputs
            .data
            .ready
            .plan
            .source_prefix
            .source
            .version()
            .effective_image_digest,
        None,
    )
    .unwrap();
    assert!(matches!(
        outgoing.source,
        ImportSource::Ordinary {
            private_schema: 10,
            admission: Some(_),
            ..
        }
    ));
    closure.assert_current_c10_as_historical_ancestor(destination, &outgoing);
    assert_eq!(destination.total_changes(), changes);
}

fn assert_c10_history_dispatch(c: &Connection, marker: &format::Marker) {
    use crate::projection::{capture, conversion, offline, provider, ProjectionStore};
    use layrs_core::projection::ProjectionVersion;

    // SQL DATA handle only: these calls neither open the checkout nor grant I/O.
    let store = ProjectionStore {
        root: marker.checkout_root.clone().into(),
        marker: marker.clone(),
    };
    let admitted = admitted_ordinary::inspect(c, marker, None).unwrap();
    let count: u64 = c
        .query_row("SELECT count(*) FROM projection_captures", [], |r| r.get(0))
        .unwrap();
    assert!(count > 0);
    for sequence in 1..=count {
        let patch = capture::storage::load_metadata(c, &store, sequence).unwrap();
        conversion::validate_capture_identity(c, &store, &patch).unwrap();
        assert_eq!(
            conversion::imported(c, &store, sequence).unwrap(),
            sequence <= admitted.imported_capture_count()
        );
        conversion::imported_publication_record(c, &store, sequence).unwrap();
        let mut foreign = patch;
        foreign.identity.worktree_id = WorktreeId::parse("worktree:foreign-capture").unwrap();
        assert!(conversion::validate_capture_identity(c, &store, &foreign).is_err());
    }
    let mut history = c
        .prepare("SELECT operation_id,capture_sequence,payload FROM projection_restore_history")
        .unwrap();
    let rows = history
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(!rows.is_empty());
    for (id, sequence, payload) in rows {
        let origin =
            admitted_ordinary::restore_history_origin(c, marker, &id, sequence, &payload, None)
                .unwrap();
        let patch = capture::storage::load_metadata(c, &store, sequence).unwrap();
        assert_eq!(origin.identity, patch.identity);
    }
    assert!(provider::require_ordinary(c).is_err());
    assert!(provider::require_reclamation_safe(c).is_err());
    let current = database::current(c).unwrap();
    let version = ProjectionVersion::Generation {
        worktree_id: marker.identity.worktree_id.clone(),
        generation: current.generation,
        digest: current.digest,
    };
    assert!(offline::resolve(c, &store, &version).is_err());
}

fn exercise_source_publication(
    source: &Connection,
    destination: &Connection,
    family: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
) {
    use crate::projection::provider::{
        ordinary_transition::{self, PublicationCapacity},
        transition_sql::SourceIntentEffect,
    };
    let marker = inputs.source_marker;
    let staging = inputs.source_staging;
    let ready = inputs.data.ready;
    let receipt = inputs.data.receipt;
    let capacity = PublicationCapacity::new(
        ready,
        receipt,
        inputs.metadata_floor,
        staging.plan.quota_bytes,
    )
    .unwrap();
    assert!(capacity.metadata_floor() > inputs.metadata_floor);
    assert!(PublicationCapacity::new(ready, receipt, inputs.metadata_floor, 0).is_err());
    assert!(PublicationCapacity::new(ready, receipt, 0, staging.plan.quota_bytes).is_err());
    let before = dump(source);
    let destination_before = dump(destination);
    let initializing_family = dump(family);
    assert_source_publication_reader(
        source,
        destination,
        family,
        inputs,
        expected,
        TransitionState::Staging,
        false,
    );
    let changes = family.total_changes();
    assert_eq!(
        inputs
            .retained
            .publish_export_ready(
                family,
                inputs.family_marker_bytes,
                &inputs.publication_inputs(expected),
                None,
            )
            .unwrap(),
        crate::projection::shared::admitted_catalog::ExportPublicationEffect::Updated
    );
    assert_eq!(family.total_changes(), changes + 1);
    let mut exact_family_ready = initializing_family;
    let members = exact_family_ready.get_mut("worktrees").unwrap();
    let owner = Cell::Text(inputs.target.binding().owner.as_str().as_bytes().to_vec());
    let row = members
        .iter_mut()
        .find(|row| row.first() == Some(&owner))
        .unwrap();
    assert_eq!(row[4], Cell::Text(b"initializing".to_vec()));
    row[4] = Cell::Text(b"ready".to_vec());
    members.sort();
    assert_eq!(dump(family), exact_family_ready);
    assert_eq!(dump(source), before);
    assert_eq!(dump(destination), destination_before);
    let family_before = dump(family);
    assert_source_publication_reader(
        source,
        destination,
        family,
        inputs,
        expected,
        TransitionState::Staging,
        true,
    );
    let ingress = read_admitted_import(source, marker, None).unwrap();
    let state = ordinary_transition::read_publication_state(source, marker, TX, None).unwrap();
    assert_eq!(&state.intent, staging);
    assert!(state.receipt.is_none());

    // Canonical receipt without its terminal source owner is corruption. The
    // writer cannot adopt it, even when it is exactly the proposed C receipt.
    source.execute_batch("SAVEPOINT orphan_receipt").unwrap();
    source
        .execute(
            "INSERT INTO projection_provider_transition_receipts VALUES(?1,?2,?3,?4,?5)",
            params![
                receipt.digest().unwrap(),
                receipt.owner_identity_digest,
                receipt.transaction_id,
                receipt.companion_id,
                receipt.encode().unwrap()
            ],
        )
        .unwrap();
    let corrupt = dump(source);
    let changes = source.total_changes();
    assert!(read_admitted_import(source, marker, None).is_err());
    assert!(ordinary_transition::apply_ready(
        source, marker, staging, ready, receipt, &capacity, None
    )
    .is_err());
    assert_eq!(source.total_changes(), changes);
    assert_eq!(dump(source), corrupt);
    source
        .execute_batch("ROLLBACK TO orphan_receipt; RELEASE orphan_receipt")
        .unwrap();

    let mut wrong = ProviderTransitionReceiptRecord::decode(&receipt.encode().unwrap()).unwrap();
    wrong.transaction_id = "000000000000000000000000000ce199".into();
    let changes = source.total_changes();
    assert!(ordinary_transition::apply_ready(
        source, marker, staging, ready, &wrong, &capacity, None
    )
    .is_err());
    assert_eq!(source.total_changes(), changes);
    assert_eq!(dump(source), before);
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(ordinary_transition::apply_ready(
        source,
        marker,
        staging,
        ready,
        receipt,
        &capacity,
        Some(&cancel),
    )
    .is_err());
    assert_eq!(source.total_changes(), changes);
    assert_eq!(dump(source), before);
    assert_eq!(
        ordinary_transition::apply_ready(source, marker, staging, ready, receipt, &capacity, None)
            .unwrap(),
        SourceIntentEffect::Updated
    );
    assert_eq!(
        source.total_changes(),
        changes + 2,
        "one receipt INSERT and one source CAS"
    );
    let after_ready = dump(source);
    for (table, rows) in &before {
        if table != "projection_provider_transitions"
            && table != "projection_provider_transition_receipts"
        {
            assert_eq!(&after_ready[table], rows, "Ready changed {table}");
        }
    }
    assert!(
        ingress.require(source, None).is_err(),
        "retained outgoing proof must notice the new phase/receipt"
    );
    let admitted_ready = read_admitted_import(source, marker, None).unwrap();
    assert_eq!(admitted_ready.record(), ingress.record());
    assert_eq!(admitted_ready.ready(), ingress.ready());
    assert_eq!(admitted_ready.receipt(), ingress.receipt());
    let source_ready =
        ordinary_transition::read_publication_state(source, marker, TX, None).unwrap();
    assert_eq!(source_ready.intent.state, TransitionState::Ready);
    assert_eq!(source_ready.receipt.as_ref(), Some(receipt));
    assert!(
        read_ordinary_checkout_ready_for_publication(
            source,
            destination,
            family,
            inputs,
            expected,
            None,
        )
        .is_err(),
        "strict Staging reader must refuse actual B Ready"
    );
    assert_source_publication_reader(
        source,
        destination,
        family,
        inputs,
        expected,
        TransitionState::Ready,
        true,
    );
    let mut exact_ready = before.clone();
    let row = exact_ready
        .get_mut("projection_provider_transitions")
        .unwrap()
        .iter_mut()
        .find(|row| row[0] == Cell::Text(TX.as_bytes().to_vec()))
        .unwrap();
    row[2] = Cell::Text(b"ready".to_vec());
    row[5] = Cell::Text(receipt.digest().unwrap().into_bytes());
    row[6] = Cell::Blob(source_ready.intent.encode().unwrap());
    row[7] = Cell::Text(source_ready.intent.digest().unwrap().into_bytes());
    exact_ready
        .get_mut("projection_provider_transitions")
        .unwrap()
        .sort();
    let receipts = exact_ready
        .get_mut("projection_provider_transition_receipts")
        .unwrap();
    receipts.push(vec![
        Cell::Text(receipt.digest().unwrap().into_bytes()),
        Cell::Text(receipt.owner_identity_digest.as_bytes().to_vec()),
        Cell::Text(receipt.transaction_id.as_bytes().to_vec()),
        receipt
            .companion_id
            .as_ref()
            .map_or(Cell::Null, |id| Cell::Text(id.as_bytes().to_vec())),
        Cell::Blob(receipt.encode().unwrap()),
    ]);
    receipts.sort();
    assert_eq!(
        after_ready, exact_ready,
        "Ready must change exactly one source row and insert C's exact receipt"
    );
    assert!(ordinary_transition::has_live(source).unwrap());
    assert!(database::require_idle(source).is_err());
    assert!(
        ordinary_transition::apply_staging(
            source,
            marker,
            &staging.plan.source_prefix,
            staging,
            None,
        )
        .is_err(),
        "old Staging writer must refuse after actual Ready promotion"
    );
    let mut competing = ProviderTransitionIntentRecord::decode(&staging.encode().unwrap()).unwrap();
    competing.transaction_id = "000000000000000000000000000ce198".into();
    competing.state = TransitionState::Planned;
    competing.destination = None;
    competing.reservation = None;
    assert!(ordinary_transition::apply_planned(
        source,
        marker,
        &competing.plan.source_prefix,
        &competing,
        None
    )
    .is_err());
    let changes = source.total_changes();
    source.execute_batch("PRAGMA query_only=ON").unwrap();
    assert_eq!(
        ordinary_transition::apply_ready(source, marker, staging, ready, receipt, &capacity, None)
            .unwrap(),
        SourceIntentEffect::AlreadyPresent
    );
    assert_eq!(source.total_changes(), changes);
    assert_eq!(dump(source), after_ready);
    source.execute_batch("PRAGMA query_only=OFF").unwrap();

    for sql in [
        "UPDATE projection_provider_transition_receipts SET owner_identity_digest='blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff' WHERE transaction_id='000000000000000000000000000ce101'",
        "UPDATE projection_provider_transition_receipts SET payload=X'00' WHERE transaction_id='000000000000000000000000000ce101'",
        "UPDATE projection_head SET generation=generation-1",
    ] {
        source.execute_batch("SAVEPOINT corrupt_source_ready").unwrap();
        assert_eq!(source.execute(sql, []).unwrap(), 1);
        let corrupt = dump(source);
        let changes = source.total_changes();
        assert!(admitted_ready.require(source, None).is_err());
        assert!(ordinary_transition::read_publication_state(source, marker, TX, None).is_err());
        assert!(ordinary_transition::apply_published(source, marker, &source_ready.intent, ready, receipt, &capacity, None).is_err());
        assert_eq!(source.total_changes(), changes);
        assert_eq!(dump(source), corrupt);
        source.execute_batch("ROLLBACK TO corrupt_source_ready; RELEASE corrupt_source_ready").unwrap();
    }
    // Immutable incoming admission remains valid, but a live Ready source
    // cannot publish or replay Ready with an unrepresented operational row.
    for table in [
        "projection_install",
        "projection_refresh_pending",
        "projection_namespace_pending",
        "projection_restore_pending",
        "projection_rebase_pending",
    ] {
        source
            .execute_batch("SAVEPOINT ready_pending_operation")
            .unwrap();
        source
            .execute(
                &format!("INSERT INTO {table}(singleton,payload) VALUES(1,X'')"),
                [],
            )
            .unwrap();
        let corrupt = dump(source);
        let changes = source.total_changes();
        read_admitted_import(source, marker, None).unwrap();
        let error = ordinary_transition::read_publication_state(source, marker, TX, None)
            .err()
            .expect("live Ready must refuse pending operations");
        assert!(error.to_string().contains(table), "{error}");
        let error = ordinary_transition::apply_published(
            source,
            marker,
            &source_ready.intent,
            ready,
            receipt,
            &capacity,
            None,
        )
        .err()
        .expect("Ready publication must refuse before its SQL effect");
        assert!(error.to_string().contains(table), "{error}");
        assert!(
            ordinary_transition::apply_ready(
                source, marker, staging, ready, receipt, &capacity, None,
            )
            .is_err(),
            "Ready replay must not accept an operational pending row"
        );
        assert_eq!(source.total_changes(), changes);
        assert_eq!(dump(source), corrupt);
        source
            .execute_batch("ROLLBACK TO ready_pending_operation; RELEASE ready_pending_operation")
            .unwrap();
    }
    // A capture-only advance also fences Ready even when the head is current.
    source
        .execute_batch("SAVEPOINT ready_capture_frontier")
        .unwrap();
    append_publication_tail(source, marker, false);
    assert!(read_admitted_import(source, marker, None).is_err());
    source
        .execute_batch("ROLLBACK TO ready_capture_frontier; RELEASE ready_capture_frontier")
        .unwrap();
    let changes = source.total_changes();
    assert_eq!(
        ordinary_transition::apply_published(
            source,
            marker,
            &source_ready.intent,
            ready,
            receipt,
            &capacity,
            None
        )
        .unwrap(),
        SourceIntentEffect::Updated
    );
    assert_eq!(
        source.total_changes(),
        changes + 1,
        "Published only changes one source row"
    );
    let after_published = dump(source);
    let mut published =
        ProviderTransitionIntentRecord::decode(&source_ready.intent.encode().unwrap()).unwrap();
    published.state = TransitionState::Published;
    let mut exact_published = after_ready.clone();
    let row = exact_published
        .get_mut("projection_provider_transitions")
        .unwrap()
        .iter_mut()
        .find(|row| row[0] == Cell::Text(TX.as_bytes().to_vec()))
        .unwrap();
    row[2] = Cell::Text(b"published".to_vec());
    row[6] = Cell::Blob(published.encode().unwrap());
    row[7] = Cell::Text(published.digest().unwrap().into_bytes());
    exact_published
        .get_mut("projection_provider_transitions")
        .unwrap()
        .sort();
    assert_eq!(
        after_published, exact_published,
        "Published changes only state/payload/digest of its source row"
    );
    for (table, rows) in &after_ready {
        if table != "projection_provider_transitions" {
            assert_eq!(&after_published[table], rows, "Published changed {table}");
        }
    }
    assert!(!ordinary_transition::has_live(source).unwrap());
    database::require_idle(source).unwrap();
    assert!(
        read_ordinary_checkout_ready_for_publication(
            source,
            destination,
            family,
            inputs,
            expected,
            None,
        )
        .is_err(),
        "strict Staging reader must refuse actual B Published"
    );
    assert_source_publication_reader(
        source,
        destination,
        family,
        inputs,
        expected,
        TransitionState::Published,
        true,
    );
    // Coherent later B generation and capture preserve the immutable Published
    // SQL handoff. No old image is compared to a new current-prefix planner.
    append_publication_tail(source, marker, true);
    let historical = dump(source);
    let changes = source.total_changes();
    source.execute_batch("PRAGMA query_only=ON").unwrap();
    let state = ordinary_transition::read_publication_state(source, marker, TX, None).unwrap();
    assert_eq!(state.intent.state, TransitionState::Published);
    assert_eq!(state.receipt.as_ref(), Some(receipt));
    assert_eq!(
        ordinary_transition::apply_published(
            source,
            marker,
            &source_ready.intent,
            ready,
            receipt,
            &capacity,
            None
        )
        .unwrap(),
        SourceIntentEffect::AlreadyPresent
    );
    assert!(
        ordinary_transition::apply_ready(source, marker, staging, ready, receipt, &capacity, None)
            .is_err(),
        "Ready API must not downgrade or adopt Published"
    );
    assert_eq!(source.total_changes(), changes);
    assert_eq!(dump(source), historical);
    source.execute_batch("PRAGMA query_only=OFF").unwrap();
    assert_eq!(dump(destination), destination_before);
    assert_eq!(dump(family), family_before);
}

/// Each publication phase gets fresh B provenance, a newly interpreted original
/// copied closure, and a new closed actual-phase source. Old proof reuse must
/// not hide the outgoing row/receipt changes performed by the preceding CAS.
fn assert_source_publication_reader(
    source: &Connection,
    destination: &Connection,
    family: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
    phase: TransitionState,
    family_ready: bool,
) {
    use crate::projection::provider::ordinary_publication::OrdinaryPublicationSource;
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    let admitted = read_admitted_import(source, inputs.source_marker, None).unwrap();
    let actual = admitted_ordinary::inspect(source, inputs.source_marker, None).unwrap();
    let retained = AdmittedFamilyContext::new(&actual).unwrap();
    let original = &inputs.source_staging.plan.source_prefix;
    let (prefix, closure) = plan_admitted_import(
        source,
        inputs.source_marker,
        original.destination_identity.clone(),
        &original.source.version().effective_image_digest,
        None,
    )
    .unwrap();
    assert_eq!(&prefix, original);
    let fresh = OrdinaryReadyInputs {
        marker: inputs.marker,
        source_marker: inputs.source_marker,
        source_staging: inputs.source_staging,
        closure: &closure,
        retained: &retained,
        target: inputs.target,
        protection: inputs.protection,
        family_marker_bytes: inputs.family_marker_bytes,
        metadata_floor: inputs.metadata_floor,
        data: inputs.data,
    };
    let publication = OrdinaryPublicationSource::new(
        source,
        fresh.source_marker,
        fresh.closure,
        fresh.source_staging,
        fresh.data.ready,
        fresh.data.receipt,
        None,
    )
    .unwrap();
    assert_eq!(publication.revalidate(None).unwrap().state, phase);
    let actual = read_ordinary_ready_for_source_publication(
        source,
        destination,
        family,
        &publication,
        &fresh,
        expected,
        None,
    );
    if family_ready {
        let actual = actual.unwrap();
        assert_eq!(actual.ready(), inputs.data.ready);
        assert_eq!(actual.receipt(), inputs.data.receipt);
        assert_eq!(
            actual.ready().encode().unwrap(),
            inputs.data.ready.encode().unwrap()
        );
        assert_eq!(
            actual.receipt().encode().unwrap(),
            inputs.data.receipt.encode().unwrap()
        );

        let mut stale_receipt =
            ProviderTransitionReceiptRecord::decode(&inputs.data.receipt.encode().unwrap())
                .unwrap();
        stale_receipt.transaction_id = "000000000000000000000000000ce197".into();
        assert!(
            OrdinaryPublicationSource::new(
                source,
                fresh.source_marker,
                fresh.closure,
                fresh.source_staging,
                fresh.data.ready,
                &stale_receipt,
                None,
            )
            .is_err(),
            "closed source must reject a stale expected C receipt"
        );
        let stale_data = ManagedReadyData {
            observation: fresh.data.observation,
            placement: fresh.data.placement,
            capture: fresh.data.capture,
            overlay: fresh.data.overlay,
            required: fresh.data.required,
            receipt: &stale_receipt,
            ready: fresh.data.ready,
            claims: fresh.data.claims,
        };
        let stale = OrdinaryReadyInputs {
            data: &stale_data,
            ..fresh
        };
        assert!(
            read_ordinary_ready_for_source_publication(
                source,
                destination,
                family,
                &publication,
                &stale,
                expected,
                None,
            )
            .is_err(),
            "reader must reject stale expected receipt inputs against the closed source"
        );
    } else {
        let error = actual
            .err()
            .expect("Initializing family must refuse source publication");
        assert!(error.to_string().contains("actual family Ready"), "{error}");
    }
    admitted.require(source, None).unwrap();
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
}

fn append_publication_tail(source: &Connection, marker: &format::Marker, advance_head: bool) {
    let mut current = database::current(source).unwrap();
    if advance_head {
        let generation = current.generation + 1;
        let payload = current.snapshot.canonical_bytes().unwrap();
        let digest = database::digest(generation, Some(&current.digest), &payload);
        source
            .execute(
                "INSERT INTO projection_snapshots VALUES(?1,?2,?3,?4)",
                params![generation, digest, current.digest, payload],
            )
            .unwrap();
        let selected = selection::at(source, current.generation).unwrap();
        selection::insert(source, generation, &selected).unwrap();
        source
            .execute("UPDATE projection_head SET generation=?1", [generation])
            .unwrap();
        current = database::current(source).unwrap();
    }
    let (sequence, digest): (u64, String) = source
        .query_row(
            "SELECT sequence,digest FROM projection_captures ORDER BY sequence DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let mut capture = crate::projection::capture::CapturedPatch {
        schema_version: "layrs.projection-capture.v1".into(),
        sequence: sequence + 1,
        previous_patch_digest: Some(digest),
        projection_generation: current.generation,
        projection_digest: current.digest,
        identity: marker.identity.clone(),
        changes: vec![],
        digest: String::new(),
    };
    capture.digest = capture.computed_digest().unwrap();
    source
        .execute(
            "INSERT INTO projection_captures VALUES(?1,?2,?3,?4)",
            params![
                capture.sequence,
                capture.digest,
                capture.projection_generation,
                canonical_json(&capture).unwrap()
            ],
        )
        .unwrap();
}

fn exercise_publication(
    source: &Connection,
    destination: &Connection,
    family: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
) {
    use crate::projection::shared::admitted_catalog::{
        ExportPublicationEffect, ExportPublicationMember,
    };
    let publication = inputs.publication_inputs(expected);
    let retained = inputs.retained;
    let marker = inputs.family_marker_bytes;
    let source_before = dump(source);
    let destination_before = dump(destination);
    let family_before = dump(family);
    let read_changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    let private_before =
        read_ordinary_private_ready(destination, inputs.marker, family, None).unwrap();
    assert_eq!(
        private_before.member(),
        ExportPublicationMember::Initializing
    );
    assert_eq!(private_before.ready(), inputs.data.ready);
    assert_eq!(private_before.receipt(), inputs.data.receipt);
    assert_eq!(private_before.metadata_floor(), inputs.metadata_floor);
    let complete_before =
        read_ordinary_family_publication(source, destination, family, inputs, expected, None)
            .unwrap();
    private_before
        .require_same(complete_before.snapshot())
        .unwrap();
    assert!(
        read_ordinary_managed_ready(destination, inputs.marker, family, None).is_err(),
        "Initializing must not enter the terminal Ready reader",
    );
    read_ordinary_checkout_ready_for_publication(
        source,
        destination,
        family,
        inputs,
        expected,
        None,
    )
    .unwrap();
    let before = retained
        .read_export_publication(family, marker, &publication, None)
        .unwrap()
        .1;
    assert_eq!(before.member, ExportPublicationMember::Initializing);
    assert_eq!(complete_before.state(), before);
    let snapshot_before = complete_before.into_snapshot();

    // The same canonical representations may arrive in a different order.
    // Their actual ManifestIds and complete bodies remain exact.
    let reversed_required: Vec<_> = private_before.required().iter().rev().cloned().collect();
    assert!(reversed_required.len() > 1);
    assert_ne!(reversed_required.as_slice(), private_before.required());
    let reversed_data = ManagedReadyData {
        required: &reversed_required,
        ..*inputs.data
    };
    let reversed = OrdinaryReadyInputs {
        data: &reversed_data,
        ..*inputs
    };
    let reordered =
        read_ordinary_family_publication(source, destination, family, &reversed, expected, None)
            .unwrap();
    assert_eq!(reordered.state(), before);
    snapshot_before
        .require_same(&reordered.into_snapshot())
        .unwrap();

    // A different canonical ManifestId for the same FileId/length is not a
    // substitute, even though its exact chunk bodies describe the same bytes.
    let original = &reversed_required[0];
    let single = layrs_core::scm::ChunkingDescriptor::single();
    let chunking = if original.chunking() == &single {
        layrs_core::scm::ChunkingDescriptor::deterministic_fixed(1048576).unwrap()
    } else {
        single
    };
    let substitute = FileManifest::new(
        original.file_id().clone(),
        original.byte_length(),
        chunking,
        original.chunks().to_vec(),
    )
    .unwrap();
    assert_eq!(substitute.file_id(), original.file_id());
    assert_eq!(substitute.byte_length(), original.byte_length());
    assert_ne!(substitute.id(), original.id());
    let mut substituted_required = reversed_required.clone();
    substituted_required[0] = substitute;
    let substituted_data = ManagedReadyData {
        required: &substituted_required,
        ..*inputs.data
    };
    let substituted = OrdinaryReadyInputs {
        data: &substituted_data,
        ..*inputs
    };
    assert!(read_ordinary_family_publication(
        source,
        destination,
        family,
        &substituted,
        expected,
        None,
    )
    .is_err());
    let mut duplicated_required = reversed_required.clone();
    duplicated_required[0] = duplicated_required[1].clone();
    let duplicated_data = ManagedReadyData {
        required: &duplicated_required,
        ..*inputs.data
    };
    let duplicated = OrdinaryReadyInputs {
        data: &duplicated_data,
        ..*inputs
    };
    assert!(read_ordinary_family_publication(
        source,
        destination,
        family,
        &duplicated,
        expected,
        None,
    )
    .is_err());
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        read_changes
    );
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), destination_before);
    assert_eq!(dump(family), family_before);

    let cancel = Cancellation::default();
    cancel.cancel();
    let changes = family.total_changes();
    assert!(
        read_ordinary_private_ready(destination, inputs.marker, family, Some(&cancel),).is_err()
    );
    assert!(read_ordinary_family_publication(
        source,
        destination,
        family,
        inputs,
        expected,
        Some(&cancel),
    )
    .is_err());
    assert!(retained
        .publish_export_ready(family, marker, &publication, Some(&cancel))
        .is_err());
    assert_eq!(family.total_changes(), changes);
    assert_eq!(dump(family), family_before);
    // A Ready record borrowed from another transaction must not authorize this CAS.
    let mut wrong_receipt =
        ProviderTransitionReceiptRecord::decode(&inputs.data.receipt.encode().unwrap()).unwrap();
    wrong_receipt.transaction_id = "000000000000000000000000000ce199".into();
    let wrong_data = ManagedReadyData {
        receipt: &wrong_receipt,
        ..*inputs.data
    };
    let wrong_inputs = OrdinaryReadyInputs {
        data: &wrong_data,
        ..*inputs
    };
    assert!(read_ordinary_family_publication(
        source,
        destination,
        family,
        &wrong_inputs,
        expected,
        None,
    )
    .is_err());
    let mut wrong = inputs.publication_inputs(expected);
    wrong.receipt = &wrong_receipt;
    assert!(retained
        .publish_export_ready(family, marker, &wrong, None)
        .is_err());
    assert_eq!(family.total_changes(), changes);
    assert_eq!(dump(family), family_before);
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        read_changes
    );
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), destination_before);

    assert_family_publication_corruption(source, destination, family, inputs, expected);
    // C's provisional private DATA does not validate B's retained family root.
    // Losing that whole known group must still fail the complete actual-B gate.
    family
        .execute_batch("SAVEPOINT missing_retained_publication_root")
        .unwrap();
    assert!(
        family
            .execute(
                "DELETE FROM root_manifests WHERE root_id IN (SELECT id FROM roots WHERE owner=?1)",
                [inputs.source_marker.identity.worktree_id.as_str()],
            )
            .unwrap()
            > 0
    );
    assert_eq!(
        family
            .execute(
                "DELETE FROM roots WHERE owner=?1",
                [inputs.source_marker.identity.worktree_id.as_str()],
            )
            .unwrap(),
        1
    );
    let missing = dump(family);
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    let still_private =
        read_ordinary_private_ready(destination, inputs.marker, family, None).unwrap();
    private_before.require_same(&still_private).unwrap();
    assert!(
        read_ordinary_family_publication(source, destination, family, inputs, expected, None,)
            .is_err()
    );
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
    assert_eq!(dump(family), missing);
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), destination_before);
    family.execute_batch("ROLLBACK TO missing_retained_publication_root; RELEASE missing_retained_publication_root").unwrap();
    assert_eq!(dump(family), family_before);

    family
        .execute_batch("SAVEPOINT publication_rollback")
        .unwrap();
    assert_eq!(
        retained
            .publish_export_ready(family, marker, &publication, None)
            .unwrap(),
        ExportPublicationEffect::Updated
    );
    read_ordinary_checkout_ready_for_publication(
        source,
        destination,
        family,
        inputs,
        expected,
        None,
    )
    .unwrap();
    family
        .execute_batch("ROLLBACK TO publication_rollback; RELEASE publication_rollback")
        .unwrap();
    assert_eq!(dump(family), family_before);
    read_ordinary_checkout_ready(source, destination, family, inputs, expected, None).unwrap();
    let changes = family.total_changes();
    assert_eq!(
        retained
            .publish_export_ready(family, marker, &publication, None)
            .unwrap(),
        ExportPublicationEffect::Updated
    );
    assert_eq!(family.total_changes() - changes, 1);
    let after = retained
        .read_export_publication(family, marker, &publication, None)
        .unwrap()
        .1;
    assert_eq!(after.member, ExportPublicationMember::Ready);
    assert_eq!(after.protection, before.protection);
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    let private_after =
        read_ordinary_private_ready(destination, inputs.marker, family, None).unwrap();
    assert_eq!(private_after.member(), ExportPublicationMember::Ready);
    assert!(
        private_before.require_same(&private_after).is_err(),
        "provisional comparison must retain its exact observed member phase"
    );
    let complete_after =
        read_ordinary_family_publication(source, destination, family, inputs, expected, None)
            .unwrap();
    private_after
        .require_same(complete_after.snapshot())
        .unwrap();
    assert_eq!(complete_after.state(), after);
    snapshot_before
        .require_same(&complete_after.into_snapshot())
        .unwrap();
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
    let mut expected_family = family_before;
    let members = expected_family.get_mut("worktrees").unwrap();
    let owner = Cell::Text(inputs.target.binding().owner.as_str().as_bytes().to_vec());
    let row = members
        .iter_mut()
        .find(|row| row.first() == Some(&owner))
        .unwrap();
    assert_eq!(row.len(), 5);
    assert_eq!(row[4], Cell::Text(b"initializing".to_vec()));
    row[4] = Cell::Text(b"ready".to_vec());
    members.sort();
    assert_eq!(dump(family), expected_family);
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), destination_before);
    read_ordinary_checkout_ready_for_publication(
        source,
        destination,
        family,
        inputs,
        expected,
        None,
    )
    .unwrap();
    assert!(
        read_ordinary_checkout_ready(source, destination, family, inputs, expected, None).is_err()
    );
    assert!(finalize_ordinary_checkout_or_replay(
        source,
        destination,
        family,
        inputs,
        expected,
        None
    )
    .is_err());
    assert!(retained
        .validate_export_protection(
            family,
            marker,
            inputs.target,
            inputs.data.required,
            inputs.protection,
            None
        )
        .is_err());
    assert_family_publication_corruption(source, destination, family, inputs, expected);
    family
        .execute_batch("COMMIT; PRAGMA foreign_keys=OFF; PRAGMA query_only=ON; BEGIN DEFERRED")
        .unwrap();
    source.execute_batch("PRAGMA query_only=ON").unwrap();
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    let readonly_private =
        read_ordinary_private_ready(destination, inputs.marker, family, None).unwrap();
    private_after.require_same(&readonly_private).unwrap();
    let readonly =
        read_ordinary_family_publication(source, destination, family, inputs, expected, None)
            .unwrap();
    assert_eq!(readonly.state(), after);
    snapshot_before
        .require_same(&readonly.into_snapshot())
        .unwrap();
    read_ordinary_checkout_ready_for_publication(
        source,
        destination,
        family,
        inputs,
        expected,
        None,
    )
    .unwrap();
    assert_eq!(
        retained
            .publish_export_ready(family, marker, &publication, None)
            .unwrap(),
        ExportPublicationEffect::AlreadyReady
    );
    assert!(read_ordinary_checkout_ready_for_publication(
        source,
        destination,
        family,
        inputs,
        expected,
        Some(&cancel)
    )
    .is_err());
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
    assert_eq!(dump(family), expected_family);
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), destination_before);
    source.execute_batch("PRAGMA query_only=OFF").unwrap();
}

fn assert_family_publication_corruption(
    source: &Connection,
    destination: &Connection,
    family: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
) {
    let source_before = dump(source);
    let destination_before = dump(destination);
    let family_before = dump(family);
    for sql in [
        "UPDATE worktrees SET metadata_path='foreign' WHERE id=?1",
        "UPDATE worktrees SET database_identity='foreign' WHERE id=?1",
        "UPDATE worktrees SET marker=x'00' WHERE id=?1",
        "UPDATE roots SET payload=x'00' WHERE owner=?1",
        "UPDATE reservations SET remaining=remaining+1 WHERE owner=?1",
    ] {
        family
            .execute_batch("SAVEPOINT corrupt_publication")
            .unwrap();
        assert_eq!(
            family
                .execute(sql, [inputs.target.binding().owner.as_str()])
                .unwrap(),
            1
        );
        let corrupt = dump(family);
        let changes = (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes(),
        );
        assert!(
            read_ordinary_family_publication(source, destination, family, inputs, expected, None,)
                .is_err(),
            "complete family publication accepted {sql}"
        );
        assert!(read_ordinary_checkout_ready_for_publication(
            source,
            destination,
            family,
            inputs,
            expected,
            None,
        )
        .is_err());
        assert!(inputs
            .retained
            .publish_export_ready(
                family,
                inputs.family_marker_bytes,
                &inputs.publication_inputs(expected),
                None,
            )
            .is_err());
        assert_eq!(
            (
                source.total_changes(),
                destination.total_changes(),
                family.total_changes()
            ),
            changes
        );
        assert_eq!(dump(family), corrupt);
        assert_eq!(dump(source), source_before);
        assert_eq!(dump(destination), destination_before);
        family
            .execute_batch("ROLLBACK TO corrupt_publication; RELEASE corrupt_publication")
            .unwrap();
        assert_eq!(dump(family), family_before);
    }
}
