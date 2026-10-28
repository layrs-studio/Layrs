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

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
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
            let known: Vec<_> = supplied.iter().cloned().chain([preimage]).collect();
            let current = database::current(source).unwrap();
            let selected = selection::at(source, current.generation).unwrap();
            let entry = &current.snapshot.entries[0];
            let Some(TreeEntryTarget::File { file_id: base_id }) = &entry.target else {
                panic!("fixture file required")
            };
            let image_manifest = supplied.last().unwrap();
            assert_ne!(base_id, image_manifest.file_id());
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
            let derived = bootstrap::derive_ordinary_data(
                &prefix,
                &current.snapshot,
                &selected,
                &version.selection_digest,
                &[base],
                &files,
                &[],
                destination_identity.clone(),
                "C:\\ordinary-ready-parent",
                "fs:v1:0000000000000001:0000000000000040",
                LEAF,
                TX,
                &historical,
                &required,
                4 * 1024 * 1024,
            )
            .unwrap();
            assert_eq!(derived.capture.changes.len(), 1);
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
            ordinary_transition::apply_planned(source, marker, &prefix, &planned, None).unwrap();
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
            let target_marker =
                format::parse_marker_bytes(&canonical_json(&value).unwrap()).unwrap();
            let mut copying =
                ProviderTransitionIntentRecord::decode(&planned.encode().unwrap()).unwrap();
            copying.state = TransitionState::Copying;
            copying.destination = Some(TransitionDestinationProof {
                backend: TransitionBackendKind::Ordinary,
                format: target_marker.clone(),
                private_database_identity: "1:0:9000".into(),
                format_file_identity: "1:0:9001".into(),
                family_marker_digest: retained.binding().family_marker_digest.clone(),
                native_control_identity_hex: None,
                native_root_identity: None,
            });
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
            let staging = ordinary_transition::proposed_staging(&copying).unwrap();
            ordinary_transition::apply_staging(source, marker, &prefix, &staging, None).unwrap();
            let source_input = OrdinaryCopyInputs::new(source, marker, &staging, &closure);
            ordinary_destination::staging::apply_staging(c, &target_marker, &source_input, None)
                .unwrap();
            let destination_staging = source_input
                .authenticate_staging_source(c, &target_marker, None)
                .unwrap();
            let checkout_files = [CheckoutFile {
                path: entry.path.clone(),
                manifest_id: image_manifest.id().clone(),
                file_id: image_manifest.file_id().clone(),
                bytes: image_manifest.byte_length(),
                executable: entry.executable,
            }];
            let expected =
                ExpectedCheckout::ordinary(&destination_staging, &checkout_files, &[], image, None)
                    .unwrap();
            complete(c, &expected);
            let base_floor = managed_budget::base_metadata_floor(
                closure.metadata_bytes(source, &prefix, None).unwrap(),
                &derived.observation,
                &derived.placement,
                &derived.capture,
                &derived.plan,
                &selected,
                managed_budget::intent_size(&staging).unwrap(),
                managed_budget::intent_size(&destination_staging).unwrap(),
            )
            .unwrap();
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
            let proof = copying.destination.as_ref().unwrap();
            let binding = target.binding();
            let protection = ExactProtection::new(
                &binding.family_incarnation,
                &binding.owner,
                &binding.database_identity,
                &binding.transaction_id,
                &required,
                None,
            )
            .unwrap();
            retained
                .protect_export(family, family_marker, &target, &required, &protection, None)
                .unwrap();
            let rowset = |r: &super::super::records::transition::TransitionRowset| ImportRowset {
                count: r.count,
                digest: r.digest.clone(),
            };
            let receipt = ProviderTransitionReceiptRecord {
                schema_version: "layrs.projection-provider-transition-receipt.v1".into(),
                owner_identity_digest: proof.owner_digest(&derived.plan).unwrap(),
                transaction_id: TX.into(),
                transition_plan_digest: derived.plan.digest().unwrap(),
                destination_proof_digest: proof.digest(&derived.plan).unwrap(),
                source_prefix_id: prefix.digest().unwrap(),
                source_prefix: ProviderImportPrefixRecord::decode(&prefix.encode().unwrap())
                    .unwrap(),
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
                ProviderTransitionIntentRecord::decode(&destination_staging.encode().unwrap())
                    .unwrap();
            ready.state = TransitionState::Ready;
            ready.receipt_id = Some(receipt.digest().unwrap());
            let overlay = [ManagedReadyOverlay {
                path: entry.path.clone(),
                manifest_id: Some(image_manifest.id().clone()),
            }];
            let claims = [ManagedReadyClaim {
                root_id: protection.id().into(),
                digest: protection.id().into(),
                kind: "provider-import".into(),
                owner: TX.into(),
            }];
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
            let floor = managed_budget::complete_metadata_floor(
                base_floor, &ready, &receipt, &claims[0], &required, None,
            )
            .unwrap();
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
            let source_before = dump(source);
            let family_before = dump(family);
            let before = dump(c);
            let changes = c.total_changes();
            inputs.metadata_floor += 1;
            assert!(finalize_ordinary_checkout_or_replay(
                source, c, family, &inputs, &expected, None
            )
            .is_err());
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
            assert!(finalize_ordinary_checkout_or_replay(
                source, c, family, &inputs, &expected, None
            )
            .is_err());
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
                    read_ordinary_checkout_ready(source, c, family, &inputs, &expected, None)
                        .is_err()
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
            c.execute_batch(
                "COMMIT; PRAGMA foreign_keys=OFF; PRAGMA query_only=ON; BEGIN DEFERRED",
            )
            .unwrap();
            let before = dump(c);
            let changes = c.total_changes();
            assert_eq!(
                finalize_ordinary_checkout_or_replay(source, c, family, &inputs, &expected, None)
                    .unwrap(),
                ReadyEffect::AlreadyReady
            );
            read_ordinary_checkout_ready(source, c, family, &inputs, &expected, None).unwrap();
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
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        });
    });
}
