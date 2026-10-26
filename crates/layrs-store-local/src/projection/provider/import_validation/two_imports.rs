use super::*;
use crate::projection::{
    capture::CapturedPatch,
    provider::{
        database_io::BareConnection,
        process_profile::tests::child,
        records::{
            ProviderBootstrapBinding, Receipt, Record,
            activation::ProviderActivationRecord,
            import::{
                ImportClosure, ImportRowset, ImportSource, ImportSourceReferences, ImportVersion,
                ImportedSegment, ProviderImportKind, ProviderImportPrefixRecord,
                ProviderTransitionReceiptRecord,
            },
        },
        validation::active_fixture,
    },
};
use layrs_core::scm::{WorktreeId, canonical_json};
use rusqlite::params;

const ZERO: &str = "blake3:0000000000000000000000000000000000000000000000000000000000000000";
const IMAGE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
fn row(count: u64) -> ImportRowset {
    ImportRowset {
        count,
        digest: ZERO.into(),
    }
}

fn segments_digest(segments: &[ImportedSegment]) -> String {
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-projection-provider-import-segments-v1\0");
    hash.update(&(segments.len() as u64).to_be_bytes());
    for segment in segments {
        let bytes = canonical_json(segment).unwrap();
        hash.update(&(bytes.len() as u64).to_be_bytes());
        hash.update(&bytes);
    }
    format!("blake3:{}", hash.finalize())
}

fn closure(generation: u64, captures: u64, receipts: u64) -> ImportClosure {
    ImportClosure {
        snapshots: row(generation + 1),
        selections: row(1),
        captures: row(captures),
        publications: row(0),
        catalogs: row(1),
        descriptors: row(1),
        activation_maps: row(1),
        transition_receipts: row(receipts),
        companions: row(0),
        companion_files: row(0),
        companion_deletions: row(0),
        content_mappings: row(0),
    }
}

fn destination_identity(
    marker: &format::Marker,
    name: &str,
) -> crate::projection::ProjectionIdentity {
    let mut identity = marker.identity.clone();
    identity.worktree_id = WorktreeId::parse(format!("worktree:{name}")).unwrap();
    identity
}

fn append_boundary(
    c: &Connection,
    parent_generation: u64,
    sequence: u64,
    previous_patch: Option<&str>,
    destination: crate::projection::ProjectionIdentity,
) -> (String, String, CapturedPatch) {
    let (parent_digest, payload): (String, Vec<u8>) = c
        .query_row(
            "SELECT digest,payload FROM projection_snapshots WHERE generation=?1",
            [parent_generation],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let observation = parent_generation + 1;
    let observation_digest = database::digest(observation, Some(&parent_digest), &payload);
    c.execute(
        "INSERT INTO projection_snapshots VALUES(?1,?2,?3,?4)",
        params![observation, observation_digest, parent_digest, payload],
    )
    .unwrap();
    let placement = observation + 1;
    let placement_digest = database::digest(placement, Some(&observation_digest), &payload);
    c.execute(
        "INSERT INTO projection_snapshots VALUES(?1,?2,?3,?4)",
        params![placement, placement_digest, observation_digest, payload],
    )
    .unwrap();
    c.execute(
        "UPDATE projection_head SET generation=?1 WHERE singleton=1",
        [placement],
    )
    .unwrap();
    let mut patch = CapturedPatch {
        schema_version: "layrs.projection-capture.v1".into(),
        sequence,
        previous_patch_digest: previous_patch.map(str::to_owned),
        projection_generation: placement,
        projection_digest: placement_digest.clone(),
        identity: destination,
        changes: Vec::new(),
        digest: String::new(),
    };
    patch.digest = patch.computed_digest().unwrap();
    c.execute(
        "INSERT INTO projection_captures VALUES(?1,?2,?3,?4)",
        params![
            sequence,
            patch.digest,
            placement,
            canonical_json(&patch).unwrap()
        ],
    )
    .unwrap();
    (observation_digest, placement_digest, patch)
}

fn receipt(
    prefix: &ProviderImportPrefixRecord,
    transaction: &str,
    observation_digest: String,
    placement_digest: String,
    patch: &CapturedPatch,
) -> ProviderTransitionReceiptRecord {
    let (source_identity, version) = match &prefix.source {
        ImportSource::Ordinary {
            identity, version, ..
        }
        | ImportSource::Provider {
            identity, version, ..
        } => (identity, version),
    };
    ProviderTransitionReceiptRecord {
        schema_version: "layrs.projection-provider-transition-receipt.v1".into(),
        owner_identity_digest: ZERO.into(),
        transaction_id: transaction.into(),
        transition_plan_digest: ZERO.into(),
        destination_proof_digest: ZERO.into(),
        source_prefix_id: prefix.digest().unwrap(),
        source_prefix: ProviderImportPrefixRecord::decode(&prefix.encode().unwrap()).unwrap(),
        parent_receipt_id: prefix.ancestry_root_receipt.clone(),
        source_identity: source_identity.clone(),
        destination_identity: prefix.destination_identity.clone(),
        imported_max_generation: version.generation,
        destination_observation_generation: patch.projection_generation - 1,
        destination_observation_digest: observation_digest,
        destination_placement_generation: patch.projection_generation,
        destination_placement_digest: placement_digest,
        imported_capture_count: version.capture_count,
        destination_capture_sequence: patch.sequence,
        destination_capture_digest: patch.digest.clone(),
        protected_manifests: row(0),
        content_mappings: prefix.closure.content_mappings.clone(),
        companion_id: None,
        companion_files: row(0),
        companion_deletions: row(0),
    }
}

fn insert_receipt(c: &Connection, receipt: &ProviderTransitionReceiptRecord) {
    c.execute(
        "INSERT INTO projection_provider_transition_receipts VALUES(?1,?2,?3,NULL,?4)",
        params![
            receipt.digest().unwrap(),
            receipt.owner_identity_digest,
            receipt.transaction_id,
            receipt.encode().unwrap()
        ],
    )
    .unwrap();
    c.execute("INSERT INTO projection_imports VALUES(1,?1,?2) ON CONFLICT(singleton) DO UPDATE SET digest=excluded.digest,payload=excluded.payload",
        params![receipt.source_prefix_id,receipt.source_prefix.encode().unwrap()]).unwrap();
}

#[test]
#[ignore = "owned audited Windows SQLite process; two complete imported SQL prefixes"]
fn two_imports_preserve_historical_closures() {
    child::run(
        concat!(module_path!(), "::two_imports_preserve_historical_closures"),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            let (f, _, _, activation, activated) = active_fixture(c, false);
            let original = &f.allocation.format;
            let intermediate_identity = destination_identity(original, "imported-one");
            let final_identity = destination_identity(original, "imported-two");
            let next_identity = destination_identity(original, "imported-three");
            c.execute_batch("BEGIN DEFERRED").unwrap();
            let mut first_prefix = ProviderImportPrefixRecord {
                schema_version: "layrs.projection-provider-import-prefix.v1".into(),
                kind: ProviderImportKind::ManagedOrdinary,
                source: ImportSource::Provider {
                    identity: original.identity.clone(),
                    format_digest: super::super::records::codec::hash(
                        "layrs-projection-provider-import-format-v1",
                        &original.bytes().unwrap(),
                    ),
                    private_schema: 8,
                    allocation: f.allocation.data_reference().unwrap(),
                    prepared: f.prepared.data_reference().unwrap(),
                    binding: ProviderBootstrapBinding::decode(
                        &activation.binding.encode().unwrap(),
                    )
                    .unwrap(),
                    activation_map_id: activation.digest().unwrap(),
                    version: ImportVersion {
                        generation: 1,
                        snapshot_digest: activated.digest,
                        selection_digest: activation.selection_digest.clone(),
                        capture_count: 0,
                        capture_head: None,
                        effective_image_digest: IMAGE.into(),
                    },
                },
                destination_identity: intermediate_identity.clone(),
                inherited_segments: ImportRowset {
                    count: 0,
                    digest: segments_digest(&[]),
                },
                ancestry_root_receipt: None,
                closure: closure(1, 0, 0),
            };
            let first_data = read_source_metadata(
                c,
                original,
                ImportDatabaseKind::Provider,
                &first_prefix,
                ImportSourceReferences::Provider {
                    allocation: &f.allocation,
                    prepared: &f.prepared,
                    activation: &activation,
                },
                IMAGE,
                None,
            )
            .unwrap();
            first_prefix.closure = scoped::compute_scoped_closure(
                c,
                ImportDatabaseKind::Provider,
                &first_prefix,
                &first_data,
                None,
            )
            .unwrap();
            c.execute_batch("COMMIT").unwrap();

            let (obs1, place1, patch1) =
                append_boundary(c, 1, 1, None, intermediate_identity.clone());
            let first_receipt = receipt(&first_prefix, "import-one", obs1, place1.clone(), &patch1);
            insert_receipt(c, &first_receipt);
            c.execute(
                "UPDATE projection_backend SET backend='ordinary',mount_id=NULL WHERE singleton=1",
                [],
            )
            .unwrap();
            c.execute("DELETE FROM projection_provider_mounts", [])
                .unwrap();
            let mut intermediate_marker = original.clone();
            intermediate_marker.identity = intermediate_identity.clone();
            intermediate_marker.import_digest = Some(first_prefix.digest().unwrap());
            c.execute(
                "UPDATE projection_meta SET identity=?1 WHERE singleton=1",
                [intermediate_marker.bytes().unwrap()],
            )
            .unwrap();
            let first_segment = ImportedSegment::from_receipt(&first_receipt, None).unwrap();
            let mut second_prefix = ProviderImportPrefixRecord {
                schema_version: "layrs.projection-provider-import-prefix.v1".into(),
                kind: ProviderImportKind::ManagedOrdinary,
                source: ImportSource::Ordinary {
                    identity: intermediate_identity,
                    format_digest: super::super::records::codec::hash(
                        "layrs-projection-provider-import-format-v1",
                        &intermediate_marker.bytes().unwrap(),
                    ),
                    private_schema: 8,
                    version: ImportVersion {
                        generation: 3,
                        snapshot_digest: place1,
                        selection_digest: activation.selection_digest.clone(),
                        capture_count: 1,
                        capture_head: Some(patch1.digest.clone()),
                        effective_image_digest: IMAGE.into(),
                    },
                },
                destination_identity: final_identity.clone(),
                inherited_segments: ImportRowset {
                    count: 1,
                    digest: segments_digest(&[first_segment.clone()]),
                },
                ancestry_root_receipt: Some(first_receipt.digest().unwrap()),
                closure: closure(3, 1, 1),
            };
            c.execute_batch("BEGIN DEFERRED").unwrap();
            let second_data = read_source_metadata(
                c,
                &intermediate_marker,
                ImportDatabaseKind::Provider,
                &second_prefix,
                ImportSourceReferences::Ordinary(&intermediate_marker),
                IMAGE,
                None,
            )
            .unwrap();
            second_prefix.closure = scoped::compute_scoped_closure(
                c,
                ImportDatabaseKind::Provider,
                &second_prefix,
                &second_data,
                None,
            )
            .unwrap();
            verify_source(
                c,
                &intermediate_marker,
                ImportDatabaseKind::Provider,
                &second_prefix,
                ImportSourceReferences::Ordinary(&intermediate_marker),
                None,
                IMAGE,
                None,
            )
            .unwrap();
            c.execute_batch("COMMIT").unwrap();

            let (obs2, place2, patch2) =
                append_boundary(c, 3, 2, Some(&patch1.digest), final_identity.clone());
            let second_receipt =
                receipt(&second_prefix, "import-two", obs2, place2.clone(), &patch2);
            insert_receipt(c, &second_receipt);
            let mut final_marker = original.clone();
            final_marker.identity = final_identity.clone();
            final_marker.import_digest = Some(second_prefix.digest().unwrap());
            c.execute(
                "UPDATE projection_meta SET identity=?1 WHERE singleton=1",
                [final_marker.bytes().unwrap()],
            )
            .unwrap();
            let second_segment =
                ImportedSegment::from_receipt(&second_receipt, Some(&first_receipt)).unwrap();
            let mut third_prefix = ProviderImportPrefixRecord {
                schema_version: "layrs.projection-provider-import-prefix.v1".into(),
                kind: ProviderImportKind::ManagedOrdinary,
                source: ImportSource::Ordinary {
                    identity: final_identity,
                    format_digest: super::super::records::codec::hash(
                        "layrs-projection-provider-import-format-v1",
                        &final_marker.bytes().unwrap(),
                    ),
                    private_schema: 8,
                    version: ImportVersion {
                        generation: 5,
                        snapshot_digest: place2,
                        selection_digest: activation.selection_digest.clone(),
                        capture_count: 2,
                        capture_head: Some(patch2.digest.clone()),
                        effective_image_digest: IMAGE.into(),
                    },
                },
                destination_identity: next_identity,
                inherited_segments: ImportRowset {
                    count: 2,
                    digest: segments_digest(&[first_segment, second_segment]),
                },
                ancestry_root_receipt: Some(second_receipt.digest().unwrap()),
                closure: closure(5, 2, 2),
            };
            c.execute_batch("BEGIN DEFERRED").unwrap();
            let third_data = read_source_metadata(
                c,
                &final_marker,
                ImportDatabaseKind::Provider,
                &third_prefix,
                ImportSourceReferences::Ordinary(&final_marker),
                IMAGE,
                None,
            )
            .unwrap();
            third_prefix.closure = scoped::compute_scoped_closure(
                c,
                ImportDatabaseKind::Provider,
                &third_prefix,
                &third_data,
                None,
            )
            .unwrap();
            let verified = verify_source(
                c,
                &final_marker,
                ImportDatabaseKind::Provider,
                &third_prefix,
                ImportSourceReferences::Ordinary(&final_marker),
                None,
                IMAGE,
                None,
            )
            .unwrap();
            assert_eq!(verified.closure().transition_receipts.count, 2);
            let (origin, first, last) = verified.capture_origin(1, 5).unwrap();
            assert_eq!(origin, &destination_identity(original, "imported-one"));
            assert_eq!((first, last), (2, 3));
            for import_digest in [None, Some(ZERO.to_owned())] {
                c.execute_batch("SAVEPOINT marker_mismatch").unwrap();
                let mut bad_marker = final_marker.clone();
                bad_marker.import_digest = import_digest;
                c.execute(
                    "UPDATE projection_meta SET identity=?1 WHERE singleton=1",
                    [bad_marker.bytes().unwrap()],
                )
                .unwrap();
                let mut bad_prefix =
                    ProviderImportPrefixRecord::decode(&third_prefix.encode().unwrap()).unwrap();
                let ImportSource::Ordinary { format_digest, .. } = &mut bad_prefix.source else {
                    panic!("ordinary source expected")
                };
                *format_digest = super::super::records::codec::hash(
                    "layrs-projection-provider-import-format-v1",
                    &bad_marker.bytes().unwrap(),
                );
                let refusal = verify_source(
                    c,
                    &bad_marker,
                    ImportDatabaseKind::Provider,
                    &bad_prefix,
                    ImportSourceReferences::Ordinary(&bad_marker),
                    None,
                    IMAGE,
                    None,
                )
                .err()
                .expect("FORMAT import mismatch must refuse");
                assert!(refusal.to_string().contains("FORMAT digest"));
                c.execute_batch("ROLLBACK TO marker_mismatch; RELEASE marker_mismatch")
                    .unwrap();
            }
            for (forge_mode, expected_reason) in [
                (0, "ancestor immutable SQL closure"),
                (1, "ancestor terminal snapshot"),
                (2, "destination escaped source high-water"),
                (3, "activation source or placement differs"),
            ] {
                c.execute_batch("SAVEPOINT forge_ancestor").unwrap();
                let mut forged_first_prefix =
                    ProviderImportPrefixRecord::decode(&first_prefix.encode().unwrap()).unwrap();
                if forge_mode == 1 {
                    let ImportSource::Provider { version, .. } = &mut forged_first_prefix.source
                    else {
                        panic!("provider ancestor expected")
                    };
                    version.snapshot_digest = "0".repeat(64);
                } else if forge_mode == 0 {
                    forged_first_prefix.closure.catalogs.digest = ZERO.into();
                } else if forge_mode == 3 {
                    let mut forged_map =
                        ProviderActivationRecord::decode(&activation.encode().unwrap()).unwrap();
                    forged_map.activated_digest = "0".repeat(64);
                    let forged_map_id = forged_map.digest().unwrap();
                    c.execute(
                        "UPDATE projection_provider_activations SET id=?1,activated_digest=?2,payload=?3 WHERE id=?4",
                        params![&forged_map_id,&forged_map.activated_digest,forged_map.encode().unwrap(),activation.digest().unwrap()],
                    ).unwrap();
                    let ImportSource::Provider {
                        activation_map_id, ..
                    } = &mut forged_first_prefix.source
                    else {
                        panic!("provider ancestor expected")
                    };
                    *activation_map_id = forged_map_id;
                }
                let mut forged_first_receipt =
                    ProviderTransitionReceiptRecord::decode(&first_receipt.encode().unwrap())
                        .unwrap();
                forged_first_receipt.source_prefix_id = forged_first_prefix.digest().unwrap();
                forged_first_receipt.source_prefix = forged_first_prefix;
                if forge_mode == 2 {
                    forged_first_receipt.destination_observation_generation =
                        second_receipt.destination_observation_generation;
                    forged_first_receipt.destination_observation_digest =
                        second_receipt.destination_observation_digest.clone();
                    forged_first_receipt.destination_placement_generation =
                        second_receipt.destination_placement_generation;
                    forged_first_receipt.destination_placement_digest =
                        second_receipt.destination_placement_digest.clone();
                    forged_first_receipt.destination_capture_sequence =
                        second_receipt.destination_capture_sequence;
                    forged_first_receipt.destination_capture_digest =
                        second_receipt.destination_capture_digest.clone();
                }
                let forged_first_id = forged_first_receipt.digest().unwrap();
                let forged_first_segment =
                    ImportedSegment::from_receipt(&forged_first_receipt, None).unwrap();
                let mut forged_second_prefix =
                    ProviderImportPrefixRecord::decode(&second_prefix.encode().unwrap()).unwrap();
                forged_second_prefix.ancestry_root_receipt = Some(forged_first_id.clone());
                forged_second_prefix.inherited_segments.digest =
                    segments_digest(&[forged_first_segment.clone()]);
                let mut forged_second_receipt =
                    ProviderTransitionReceiptRecord::decode(&second_receipt.encode().unwrap())
                        .unwrap();
                forged_second_receipt.parent_receipt_id = Some(forged_first_id);
                forged_second_receipt.source_prefix_id = forged_second_prefix.digest().unwrap();
                forged_second_receipt.source_prefix = forged_second_prefix;
                let forged_second_segment = ImportedSegment::from_receipt(
                    &forged_second_receipt,
                    Some(&forged_first_receipt),
                )
                .unwrap();
                let mut forged_third_prefix =
                    ProviderImportPrefixRecord::decode(&third_prefix.encode().unwrap()).unwrap();
                forged_third_prefix.ancestry_root_receipt =
                    Some(forged_second_receipt.digest().unwrap());
                forged_third_prefix.inherited_segments.digest =
                    segments_digest(&[forged_first_segment, forged_second_segment]);
                let mut forged_marker = final_marker.clone();
                forged_marker.import_digest = Some(forged_second_receipt.source_prefix_id.clone());
                c.execute(
                    "UPDATE projection_meta SET identity=?1 WHERE singleton=1",
                    [forged_marker.bytes().unwrap()],
                )
                .unwrap();
                let ImportSource::Ordinary { format_digest, .. } = &mut forged_third_prefix.source
                else {
                    panic!("ordinary source expected")
                };
                *format_digest = super::super::records::codec::hash(
                    "layrs-projection-provider-import-format-v1",
                    &forged_marker.bytes().unwrap(),
                );
                c.execute("DELETE FROM projection_provider_transition_receipts", [])
                    .unwrap();
                insert_receipt(c, &forged_first_receipt);
                insert_receipt(c, &forged_second_receipt);
                c.execute(
                    "UPDATE projection_imports SET digest=?1,payload=?2 WHERE singleton=1",
                    params![
                        &forged_second_receipt.source_prefix_id,
                        forged_second_receipt.source_prefix.encode().unwrap()
                    ],
                )
                .unwrap();
                let refusal = verify_source(
                    c,
                    &forged_marker,
                    ImportDatabaseKind::Provider,
                    &forged_third_prefix,
                    ImportSourceReferences::Ordinary(&forged_marker),
                    None,
                    IMAGE,
                    None,
                )
                .err()
                .expect("forged ancestor must refuse");
                assert!(refusal.to_string().contains(expected_reason), "{refusal}");
                c.execute_batch("ROLLBACK TO forge_ancestor; RELEASE forge_ancestor")
                    .unwrap();
            }
            c.execute_batch("COMMIT").unwrap();
            bare.finish(Ok(())).unwrap();
        },
    );
}
