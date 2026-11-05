use super::super::Receipt;
use super::*;
use crate::projection::{provider::validation::tests::fixture::Fixture, RecoveryBinding};
use layrs_core::scm::FileId;

fn binding(f: &Fixture) -> ProviderBootstrapBinding {
    let a = &f.allocation;
    let p = &f.prepared;
    ProviderBootstrapBinding {
        schema_version: "layrs.projection-provider-bootstrap-binding.v1".into(),
        recovery: RecoveryBinding {
            identity: a.format.identity.clone(),
            checkout_identity: a.format.checkout_identity().into(),
            marker_digest: codec::hash(
                "layrs-projection-recovery-marker-v1",
                &a.format.bytes().unwrap(),
            ),
            database_identity: a.private.database_file_identity.clone(),
            family_marker_digest: Some(a.family.marker_digest.clone()),
        },
        transaction_id: a.transaction_id.clone(),
        allocation: a.data_reference().unwrap(),
        prepared: Some(super::super::PreparedBinding {
            receipt: p.data_reference().unwrap(),
            control_identity_hex: p.control_identity_hex.clone(),
        }),
        abandoned: None,
    }
}

/// Independent literal V2 wire construction for synthetic, pure DATA checks.
/// The actual codec and control binding must accept every byte before use.
fn ticket(f: &Fixture) -> Vec<u8> {
    let control = f.prepared.control_identity().unwrap();
    let mut digest_input = b"layrs-vfs-control-directory-registration-v2\0".to_vec();
    digest_input.extend_from_slice(&control.encode());
    let digest = FileId::from_bytes(&digest_input);
    let stage = control.stage_identity();
    let root = stage.root().to_str().unwrap().as_bytes();
    let identity = stage.root_filesystem_identity();
    let parts: Vec<_> = identity.split(':').collect();
    assert_eq!(parts.len(), 4);
    let volume = u32::from_str_radix(parts[2], 16).unwrap();
    let root_index = u64::from_str_radix(parts[3], 16).unwrap();
    let mut wire = b"LYRTICK2".to_vec();
    wire.extend_from_slice(&[0x11; 16]);
    wire.extend_from_slice(f.source.commitment().vfs_catalog_id.as_str().as_bytes());
    wire.extend_from_slice(&volume.to_le_bytes());
    wire.extend_from_slice(&root_index.to_le_bytes());
    wire.extend_from_slice(digest.as_str().as_bytes());
    wire.extend_from_slice(&1u32.to_le_bytes());
    wire.extend_from_slice(&(root.len() as u32).to_le_bytes());
    wire.extend_from_slice(root);
    let parsed = Registration::decode_control_directory(&wire).unwrap();
    parsed.matches_control_identity(&control).unwrap();
    assert_eq!(parsed.encode(), wire);
    wire
}

fn case() -> (Fixture, ProviderPreparationRecord) {
    let f = Fixture::new("G1", false);
    let source =
        serde_json::from_slice(&serde_json::to_vec(f.source.commitment()).unwrap()).unwrap();
    let record = ProviderPreparationRecord {
        schema_version: SCHEMA.into(),
        bootstrap: f.mount(MountPhase::ControlPublished),
        binding: binding(&f),
        source,
        root_id_hex: "11".repeat(16),
        registration_ticket_hex: codec::encode_hex(&ticket(&f)),
        state: PreparationState::RegistrationPending,
    };
    record
        .require_references(&f.allocation, &f.prepared, &f.source, [0x11; 16])
        .unwrap();
    (f, record)
}

fn copy(record: &ProviderPreparationRecord) -> ProviderPreparationRecord {
    ProviderPreparationRecord::decode(&record.encode().unwrap()).unwrap()
}

fn row<'a>(
    record: &'a ProviderPreparationRecord,
    payload: &'a [u8],
    digest: &'a str,
) -> MountRowData<'a> {
    MountRowData {
        id: &record.bootstrap.transaction_id,
        catalog_id: &record.bootstrap.catalog_id,
        phase: record.sql_phase(),
        payload,
        digest,
    }
}

#[test]
fn pending_and_prepared_have_exact_canonical_sql_projections() {
    let (f, pending) = case();
    let pending_bytes = pending.encode().unwrap();
    assert!(pending_bytes.len() <= ProviderPreparationRecord::MAX_BYTES);
    assert_eq!(
        ProviderPreparationRecord::decode(&pending_bytes).unwrap(),
        pending
    );
    let pending_digest = pending.digest().unwrap();
    pending
        .require_row(row(&pending, &pending_bytes, &pending_digest))
        .unwrap();
    assert_eq!(pending.sql_phase(), "control_published");
    let old_bytes = pending.bootstrap.encode().unwrap();
    let old_digest = pending.predecessor_digest().unwrap();
    pending
        .require_predecessor_row(MountRowData {
            id: &pending.bootstrap.transaction_id,
            catalog_id: &pending.bootstrap.catalog_id,
            phase: "control_published",
            payload: &old_bytes,
            digest: &old_digest,
        })
        .unwrap();
    assert!(pending
        .require_row(MountRowData {
            phase: "provider_prepared",
            ..row(&pending, &pending_bytes, &pending_digest)
        })
        .is_err());
    assert!(pending
        .require_predecessor_row(MountRowData {
            digest: &pending_digest,
            ..row(&pending, &old_bytes, &old_digest)
        })
        .is_err());

    let mut prepared = copy(&pending);
    prepared.state = PreparationState::ProviderPrepared { pending_digest };
    let ready_bytes = prepared.encode().unwrap();
    let ready_digest = prepared.digest().unwrap();
    assert_eq!(prepared.sql_phase(), "provider_prepared");
    prepared
        .require_row(row(&prepared, &ready_bytes, &ready_digest))
        .unwrap();
    prepared.require_pending(&pending).unwrap();
    prepared
        .require_references(&f.allocation, &f.prepared, &f.source, [0x11; 16])
        .unwrap();
    assert!(prepared
        .require_row(MountRowData {
            phase: "control_published",
            ..row(&prepared, &ready_bytes, &ready_digest)
        })
        .is_err());
    assert!(prepared
        .require_predecessor_row(MountRowData {
            id: &pending.bootstrap.transaction_id,
            catalog_id: &pending.bootstrap.catalog_id,
            phase: "control_published",
            payload: &old_bytes,
            digest: &old_digest,
        })
        .is_err());
    assert!(pending.require_pending(&prepared).is_err());
}

#[test]
fn prepared_link_cannot_claim_another_pending_document() {
    let (_, pending) = case();
    let mut prepared = copy(&pending);
    prepared.state = PreparationState::ProviderPrepared {
        pending_digest: codec::hash(DOMAIN, b"other"),
    };
    assert!(prepared.encode().is_err());

    let mut other = copy(&pending);
    other.root_id_hex = "22".repeat(16);
    let mut other_ticket = ticket(&Fixture::new("G1", false));
    other_ticket[8..24].copy_from_slice(&[0x22; 16]);
    other.registration_ticket_hex = codec::encode_hex(&other_ticket);
    other.validate_fields().unwrap();
    let mut prepared = copy(&other);
    prepared.state = PreparationState::ProviderPrepared {
        pending_digest: other.digest().unwrap(),
    };
    prepared.validate_fields().unwrap();
    assert!(prepared.require_pending(&pending).is_err());
}

#[test]
fn ticket_and_external_reference_changes_refuse() {
    let (f, pending) = case();
    assert!(pending
        .require_references(&f.allocation, &f.prepared, &f.source, [0x22; 16])
        .is_err());
    let mut wrong = copy(&pending);
    wrong.bootstrap.catalog_id = codec::hash(DOMAIN, b"other catalog");
    assert!(wrong.encode().is_err());
    let mut wrong = copy(&pending);
    wrong.binding.abandoned = Some(wrong.binding.allocation.clone());
    assert!(wrong.encode().is_err());
    let mut wrong = copy(&pending);
    wrong.binding.prepared = None;
    assert!(wrong.encode().is_err());
    let mut wrong = copy(&pending);
    wrong.root_id_hex = "22".repeat(16);
    assert!(wrong.encode().is_err());
    let mut wrong = copy(&pending);
    wrong.registration_ticket_hex = codec::encode_hex(&ticket(&f)[..16]);
    assert!(wrong.encode().is_err());
    for index in [31, 114, 99, 190] {
        let mut wrong = copy(&pending);
        let mut altered = ticket(&f);
        altered[index] = match altered[index] {
            b'2' => b'3',
            b'3' => b'2',
            b'0' => b'1',
            b'1' => b'2',
            b'r' => b'b',
            _ => b'1',
        };
        assert!(Registration::decode_control_directory(&altered).is_ok());
        wrong.registration_ticket_hex = codec::encode_hex(&altered);
        assert!(
            wrong.encode().is_err(),
            "valid V2 ticket mismatch at byte {index}"
        );
    }
    let mut wrong = copy(&pending);
    wrong.source.snapshot_digest = "0".repeat(64);
    assert!(wrong
        .require_references(&f.allocation, &f.prepared, &f.source, [0x11; 16])
        .is_err());

    // All v1 A/P references and the v2 binding remain self-consistent, but
    // the allocation's source commitment differs from the actual source.
    let mut changed_a = AllocationRecord::decode(&f.allocation.encode().unwrap()).unwrap();
    changed_a.source.snapshot_bytes += 1;
    changed_a.validate_fields().unwrap();
    let mut changed_p = PreparedRecord::decode(&f.prepared.encode().unwrap()).unwrap();
    changed_p.allocation = changed_a.data_reference().unwrap();
    changed_p.require_allocation(&changed_a).unwrap();
    let mut changed = copy(&pending);
    changed.bootstrap.allocation = changed_a.data_reference().unwrap();
    changed.bootstrap.prepared = Some(changed_p.data_reference().unwrap());
    changed.binding.allocation = changed.bootstrap.allocation.clone();
    changed.binding.prepared.as_mut().unwrap().receipt = changed_p.data_reference().unwrap();
    changed.validate_fields().unwrap();
    assert!(changed
        .require_references(&changed_a, &changed_p, &f.source, [0x11; 16])
        .is_err());
}

#[test]
fn malformed_and_oversized_documents_refuse_before_authority() {
    let (_, pending) = case();
    let good = pending.encode().unwrap();
    let mut json: serde_json::Value = serde_json::from_slice(&good).unwrap();
    json["unknown"] = true.into();
    assert!(ProviderPreparationRecord::decode(&serde_json::to_vec(&json).unwrap()).is_err());
    let duplicate = String::from_utf8(good.clone()).unwrap().replacen(
        "\"schemaVersion\":",
        "\"schemaVersion\":\"layrs.projection-provider-mount.v2\",\"schemaVersion\":",
        1,
    );
    assert!(ProviderPreparationRecord::decode(duplicate.as_bytes()).is_err());
    let mut missing: serde_json::Value = serde_json::from_slice(&good).unwrap();
    missing.as_object_mut().unwrap().remove("rootIdHex");
    assert!(ProviderPreparationRecord::decode(&serde_json::to_vec(&missing).unwrap()).is_err());
    let mut trailing = good.clone();
    trailing.push(b' ');
    assert!(ProviderPreparationRecord::decode(&trailing).is_err());
    let mut wrong = copy(&pending);
    wrong
        .registration_ticket_hex
        .push_str(&"00".repeat(TICKET_MAX_BYTES));
    assert!(wrong.encode().is_err());
    assert!(ProviderPreparationRecord::decode(&vec![
        b' ';
        ProviderPreparationRecord::MAX_BYTES + 1
    ])
    .is_err());
}

#[test]
#[ignore = "owned audited Windows SQLite process; exact schema8 projection only"]
fn schema8_accepts_v2_data_while_bootstrap_validator_refuses_it() {
    use crate::projection::provider::{
        database_io::BareConnection, process_profile::tests::child, validation,
    };
    child::run(
        concat!(
            module_path!(),
            "::schema8_accepts_v2_data_while_bootstrap_validator_refuses_it"
        ),
        120,
        || {
            child::declare();
            let (f, pending) = case();
            let database = BareConnection::reference().unwrap();
            let c = database.connection();
            f.populate(c, MountPhase::ControlPublished);
            assert!(matches!(
                validation::private(c, &f.allocation, Some(&f.prepared)).unwrap(),
                validation::PrivateValidation::Initialized(mount)
                    if mount.phase == MountPhase::ControlPublished
            ));
            let read = |c: &rusqlite::Connection| -> (String, String, String, Vec<u8>, String) {
                c.query_row(
                    "SELECT id,catalog_id,phase,payload,digest FROM projection_provider_mounts",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .unwrap()
            };
            let old = read(c);
            pending
                .require_predecessor_row(MountRowData {
                    id: &old.0,
                    catalog_id: &old.1,
                    phase: &old.2,
                    payload: &old.3,
                    digest: &old.4,
                })
                .unwrap();

            for record in [&pending, &{
                let mut prepared = copy(&pending);
                prepared.state = PreparationState::ProviderPrepared {
                    pending_digest: pending.digest().unwrap(),
                };
                prepared
            }] {
                c.execute(
                    "UPDATE projection_provider_mounts SET phase=?1,payload=?2,digest=?3 WHERE id=?4",
                    rusqlite::params![record.sql_phase(),record.encode().unwrap(),record.digest().unwrap(),record.bootstrap.transaction_id],
                ).unwrap();
                let row = read(c);
                record
                    .require_row(MountRowData {
                        id: &row.0,
                        catalog_id: &row.1,
                        phase: &row.2,
                        payload: &row.3,
                        digest: &row.4,
                    })
                    .unwrap();
                assert!(validation::private(c, &f.allocation, Some(&f.prepared)).is_err());
                assert_eq!(row, read(c), "failed v1 inspection must not mutate v2 data");
            }
            database.finish(Ok(())).unwrap();
        },
    );
}
