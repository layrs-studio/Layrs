use super::*;
use crate::projection::{
    provider::{database_io::with_reference, owned_test as child},
    shared::{
        exact_content::{PREPARE_CHARGE, validate_installations, validate_protected_installations},
        schema_v2,
    },
};
use layrs_core::scm::{Chunk, ChunkingDescriptor};
use serde_json::json;

fn binding() -> ExactBinding {
    let digest = format!("blake3:{}", "1".repeat(64));
    let operation = format!("provider-transition:{}", "2".repeat(64));
    ExactBinding {
        family_incarnation: "family".into(),
        family_root_identity: "fs:v1:0000000000000001:0000000000000002".into(),
        family_marker_digest: digest.clone(),
        family_catalog_identity: "1:0:3".into(),
        plan_digest: digest.clone(),
        prefix_id: digest.clone(),
        destination_proof_digest: digest,
        transaction_id: "0".repeat(32),
        owner: WorktreeId::parse("worktree:protected").unwrap(),
        database_identity: "1:0:4".into(),
        reservation: operation.clone(),
        operation,
        quota_bytes: 1024 * 1024,
    }
}

fn manifests() -> Vec<FileManifest> {
    let abc = Chunk::new(b"abc".to_vec()).unwrap();
    vec![
        FileManifest::from_chunks(
            ChunkingDescriptor::deterministic_fixed(3).unwrap(),
            &[abc.clone(), abc],
        )
        .unwrap(),
        FileManifest::from_chunks(
            ChunkingDescriptor::deterministic_fixed(6).unwrap(),
            &[Chunk::new(b"abcabc".to_vec()).unwrap()],
        )
        .unwrap(),
    ]
}
fn expected(binding: &ExactBinding, required: &[FileManifest]) -> ExactProtection {
    ExactProtection::new(
        &binding.family_incarnation,
        &binding.owner,
        &binding.database_identity,
        &binding.transaction_id,
        required,
        None,
    )
    .unwrap()
}

#[test]
fn canonical_protection_preserves_exact_representations_and_claim_digest() {
    let binding = binding();
    let required = manifests();
    assert_eq!(required[0].file_id(), required[1].file_id());
    assert_ne!(required[0].id(), required[1].id());
    let protection = expected(&binding, &required);
    assert_eq!(
        protection,
        expected(&binding, &[required[1].clone(), required[0].clone()])
    );
    assert_ne!(protection.id(), expected(&binding, &required[..1]).id());
    assert_eq!(
        protection.operation(),
        format!("provider-import:{}", binding.transaction_id)
    );
    let decoded: serde_json::Value = serde_json::from_slice(protection.payload()).unwrap();
    assert_eq!(decoded["manifests"].as_array().unwrap().len(), 2);
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-partial-retention-v1\0");
    hash.update(protection.payload());
    assert_eq!(protection.digest(), hash.finalize().to_hex().as_str());
    assert!(
        ExactProtection::new(
            "family",
            &binding.owner,
            "1:0:4",
            &binding.transaction_id,
            &[required[0].clone(), required[0].clone()],
            None
        )
        .is_err()
    );
    let mut wrong = binding.clone();
    wrong.database_identity = "1:0:99".into();
    assert!(protection.require_binding(&wrong, &required, None).is_err());
    let floor = 1234;
    let minimum = protection.payload().len() as u64 + 2 * 256 + 65536 + floor;
    assert!(protection.require_envelope(minimum - 1, floor).is_err());
    protection.require_envelope(minimum, floor).unwrap();
    assert!(
        protection
            .require_envelope(u64::MAX, 1024 * 1024 * 1024 + 1)
            .is_err()
    );
}

fn populate(c: &Connection, binding: &ExactBinding, required: &[FileManifest]) {
    c.execute("INSERT INTO family VALUES(1,?1)", [b"family".as_slice()])
        .unwrap();
    c.execute("INSERT INTO quota VALUES(1,10000000)", [])
        .unwrap();
    c.execute(
        "INSERT INTO worktrees VALUES(?1,'member','database',?2,'initializing')",
        params![binding.owner.as_str(), b"marker".as_slice()],
    )
    .unwrap();
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
    let mut debit = 0;
    for (ordinal, manifest) in required.iter().enumerate() {
        let payload = canonical_json(manifest).unwrap();
        let unique: BTreeMap<_, _> = manifest
            .chunks()
            .iter()
            .map(|p| (p.chunk_id().as_str(), p.byte_length()))
            .collect();
        let charge = PREPARE_CHARGE + 2 * payload.len() as u64 + unique.values().sum::<u64>();
        debit += charge;
        c.execute(
            "INSERT INTO files VALUES(?1,?2) ON CONFLICT DO NOTHING",
            params![manifest.file_id().as_str(), manifest.byte_length()],
        )
        .unwrap();
        c.execute(
            "INSERT INTO manifests VALUES(?1,?2,?3,?4)",
            params![
                manifest.id().as_str(),
                manifest.file_id().as_str(),
                manifest.byte_length(),
                payload
            ],
        )
        .unwrap();
        for (id, bytes) in unique {
            c.execute("INSERT INTO chunks VALUES(?1,?2)", params![id, bytes])
                .unwrap();
        }
        for (n, part) in manifest.chunks().iter().enumerate() {
            c.execute(
                "INSERT INTO manifest_chunks VALUES(?1,?2,?3)",
                params![manifest.id().as_str(), n, part.chunk_id().as_str()],
            )
            .unwrap();
        }
        // Fixture DATA is checked through the production typed decoder below;
        // it grants no byte authority and no native filesystem proof.
        let record = json!({
            "schemaVersion":"layrs.shared-exact-installation.v1", "binding":binding,
            "manifestId":manifest.id(), "fileId":manifest.file_id(), "bytes":manifest.byte_length(),
            "directory":format!("1-2-{ordinal}"),
            "directoryIdentity":"fs:v1:0000000000000001:0000000000000009",
            "phase":"published", "spoolCursor":manifest.chunks().len(), "publishCursor":manifest.chunks().len(),
            "active":null, "chargedChunks":format!("1{}","0".repeat(255)),
            "residentChunks":"0".repeat(256), "chargedManifest":true,"debitBytes":charge,
        });
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
    }
    c.execute("UPDATE reservations SET remaining=remaining-?1", [debit])
        .unwrap();
}

#[test]
#[ignore = "owned audited SQLite child; exact protection complete Published, replay and unchanged fixed charge"]
fn complete_published_union_protects_once_without_releasing_reservation() {
    child::run(
        concat!(
            module_path!(),
            "::complete_published_union_protects_once_without_releasing_reservation"
        ),
        120,
        || {
            child::declare();
            with_reference("", |c| {
                schema_v2::initialize_reference(c)?;
                c.execute_batch("BEGIN IMMEDIATE")?;
                let binding = binding();
                let required = manifests();
                populate(c, &binding, &required);
                let protection = expected(&binding, &required);
                let before = validate_installations(c, &binding, &required, None)?;
                assert_eq!(before.published, 2);
                assert_eq!(protection.inspect(c, None)?, ProtectionPresence::Absent);
                assert_eq!(
                    validate_protected_installations(c, &binding, &required, &protection, None)?,
                    before
                );
                protection.require_envelope(before.remaining, 1234)?;
                assert_eq!(
                    protection.insert_or_replay(c, None)?,
                    ProtectionEffect::Inserted
                );
                assert_eq!(protection.inspect(c, None)?, ProtectionPresence::Present);
                assert!(
                    validate_installations(c, &binding, &required, None).is_err(),
                    "installation reader remains root-absent strict"
                );
                assert_eq!(
                    validate_protected_installations(c, &binding, &required, &protection, None)?,
                    before
                );
                let changes = c.total_changes();
                assert_eq!(
                    protection.insert_or_replay(c, None)?,
                    ProtectionEffect::AlreadyPresent
                );
                assert_eq!(c.total_changes(), changes);
                assert_eq!(
                    validate_protected_installations(c, &binding, &required, &protection, None)?,
                    before
                );
                c.execute_batch("ROLLBACK")?;
                Ok(())
            })
            .unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited SQLite child; exact protection refuses incomplete union and never repairs membership"]
fn protection_refuses_partial_journals_and_root_membership_without_mutation() {
    child::run(
        concat!(
            module_path!(),
            "::protection_refuses_partial_journals_and_root_membership_without_mutation"
        ),
        120,
        || {
            child::declare();
            with_reference("", |c| {
                schema_v2::initialize_reference(c)?;
                c.execute_batch("BEGIN IMMEDIATE")?;
                let binding = binding();
                let required = manifests();
                populate(c, &binding, &required);
                let protection = expected(&binding, &required);
                c.execute_batch("SAVEPOINT incomplete")?;
                let removed_payload: Vec<u8> = c.query_row(
                    "SELECT payload FROM exact_installations WHERE manifest_id=?1",
                    [required[1].id().as_str()],
                    |r| r.get(0),
                )?;
                let removed: crate::projection::shared::exact_content::ExactRecord =
                    serde_json::from_slice(&removed_payload).unwrap();
                c.execute(
                    "DELETE FROM exact_installations WHERE manifest_id=?1",
                    [required[1].id().as_str()],
                )?;
                c.execute(
                    "UPDATE reservations SET remaining=remaining+?1",
                    [removed.debit_bytes],
                )?;
                assert_eq!(
                    validate_installations(c, &binding, &required, None)?.published,
                    1,
                    "partial installation remains a valid installation prefix"
                );
                let changes = c.total_changes();
                assert!(
                    validate_protected_installations(c, &binding, &required, &protection, None)
                        .is_err()
                );
                assert_eq!(c.total_changes(), changes);
                assert_eq!(protection.inspect(c, None)?, ProtectionPresence::Absent);
                c.execute_batch("ROLLBACK TO incomplete; RELEASE incomplete")?;
                protection.insert_or_replay(c, None)?;
                c.execute(
                    "DELETE FROM root_manifests WHERE root_id=?1 AND manifest_id=?2",
                    params![protection.id(), required[0].id().as_str()],
                )?;
                let changes = c.total_changes();
                assert!(protection.insert_or_replay(c, None).is_err());
                assert!(
                    validate_protected_installations(c, &binding, &required, &protection, None)
                        .is_err()
                );
                assert_eq!(
                    c.total_changes(),
                    changes,
                    "partial protection is withheld, never repaired"
                );
                c.execute_batch("ROLLBACK")?;
                Ok(())
            })
            .unwrap();
        },
    );
}
