//! Authored, unrun DATA/SQL recipes. No fixture here constructs a C private
//! Ready, a CAS byte proof, or a physical source-publication capability.
use super::*;
use crate::projection::{
    provider::{database_io::with_reference, owned_test as child},
    shared::schema_v2,
};
use layrs_core::scm::{Chunk, ChunkingDescriptor};

fn binding() -> SourceCompanionBinding {
    let digest = format!("blake3:{}", "1".repeat(64));
    let op = format!("provider-transition:{}", "2".repeat(64));
    SourceCompanionBinding {
        source_owner: WorktreeId::parse("worktree:source-companion").unwrap(),
        source_database: "1:0:5".into(),
        source_schema: 8,
        companion_id: digest.clone(),
        destination: ExactBinding {
            family_incarnation: "family".into(),
            family_root_identity: "fs:v1:0000000000000001:0000000000000002".into(),
            family_marker_digest: digest.clone(),
            family_catalog_identity: "1:0:3".into(),
            plan_digest: digest.clone(),
            prefix_id: digest.clone(),
            destination_proof_digest: digest,
            transaction_id: "0".repeat(32),
            owner: WorktreeId::parse("worktree:destination-companion").unwrap(),
            database_identity: "1:0:4".into(),
            reservation: op.clone(),
            operation: op,
            quota_bytes: 12345,
        },
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

#[test]
fn canonical_source_root_binds_both_owners_and_all_alternates_without_large_minimum() {
    let b = binding();
    let required = manifests();
    assert_eq!(required[0].file_id(), required[1].file_id());
    assert_ne!(required[0].id(), required[1].id());
    let root = SourceCompanionProtection::new(b.clone(), &required, 4096, None).unwrap();
    let swapped = SourceCompanionProtection::new(
        b.clone(),
        &[required[1].clone(), required[0].clone()],
        4096,
        None,
    )
    .unwrap();
    assert_eq!(root, swapped);
    assert!(root.reservation_bytes() > 4096 + root.payload.len() as u64);
    assert!(
        root.reservation_bytes() < 65536,
        "tiny metadata must not reserve a fixed 16MiB"
    );
    root.require_union(&required, None).unwrap();
    assert!(root.require_union(&required[..1], None).is_err());
    root.require_envelope(4096).unwrap();
    assert!(root.require_envelope(4097).is_err());
    assert_eq!(
        root.id(),
        hash(b"layrs-source-companion-protection-v1\0", &root.payload)
    );
    assert!(root.operation().starts_with(NAMESPACE));
    assert!(!root.operation().starts_with("provider-import:"));
    let mut other = b.clone();
    other.source_database = "1:0:99".into();
    assert_ne!(
        root.operation(),
        SourceCompanionProtection::new(other, &required, 4096, None)
            .unwrap()
            .operation()
    );
    let mut other = b.clone();
    other.destination.plan_digest = format!("blake3:{}", "3".repeat(64));
    assert_ne!(
        root.id(),
        SourceCompanionProtection::new(other, &required, 4096, None)
            .unwrap()
            .id()
    );
    assert!(
        SourceCompanionProtection::new(b, &[required[0].clone(), required[0].clone()], 4096, None)
            .is_err()
    );
}

#[test]
fn source_root_checks_bounds_before_aggregate_encoding() {
    assert!(SourceCompanionProtection::new(binding(), &[], MAX_METADATA + 1, None).is_err());
    let mut b = binding();
    b.source_database = "x".repeat(513);
    assert!(SourceCompanionProtection::new(b, &[], 0, None).is_err());
    let mut b = binding();
    b.source_owner = b.destination.owner.clone();
    assert!(SourceCompanionProtection::new(b, &[], 0, None).is_err());
    let mut count = Count(MAX_ROOT);
    assert!(count.write(b"x").is_err());
}

#[test]
#[ignore = "unrun owned audited SQL recipe; B reservation kernel only, no fabricated C Ready"]
fn source_fixed_reservation_counts_b_plus_c_and_refuses_changed_remaining_without_writes() {
    child::run(
        concat!(
            module_path!(),
            "::source_fixed_reservation_counts_b_plus_c_and_refuses_changed_remaining_without_writes"
        ),
        120,
        || {
            child::declare();
            with_reference("", |c| {
                schema_v2::initialize_reference(c)?;
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                let b = binding();
                let root = SourceCompanionProtection::new(b.clone(), &[], 4096, None)?;
                c.execute("INSERT INTO family VALUES(1,?1)", [b"family".as_slice()])?;
                let exact = root.reservation_bytes() + b.destination.quota_bytes;
                c.execute("INSERT INTO quota VALUES(1,?1)", [exact - 1])?;
                c.execute(
                    "INSERT INTO worktrees VALUES(?1,'b',?2,?3,'ready')",
                    params![b.source_owner.as_str(), b.source_database, b"b".as_slice()],
                )?;
                c.execute(
                    "INSERT INTO worktrees VALUES(?1,'c',?2,?3,'initializing')",
                    params![
                        b.destination.owner.as_str(),
                        b.destination.database_identity,
                        b"c".as_slice()
                    ],
                )?;
                let changes = c.total_changes();
                assert!(root.reserve(c, None).is_err());
                assert_eq!(c.total_changes(), changes);
                assert_eq!(root.inspect(c, None)?, SourceCompanionPresence::Absent);
                c.execute("UPDATE quota SET bytes=?1", [exact])?;
                assert_eq!(root.reserve(c, None)?, ProtectionEffect::Inserted);
                let changes = c.total_changes();
                assert_eq!(root.reserve(c, None)?, ProtectionEffect::AlreadyPresent);
                assert_eq!(c.total_changes(), changes);
                assert_eq!(
                    root.recovery(c, TransitionState::Staging, false, None)?,
                    SourceCompanionRecovery::ReservationOnly
                );
                assert!(
                    root.recovery(c, TransitionState::Ready, true, None)
                        .is_err()
                );
                c.execute(
                    "UPDATE reservations SET remaining=remaining-1 WHERE id=?1",
                    [root.operation()],
                )?;
                let changes = c.total_changes();
                assert!(root.reserve(c, None).is_err());
                assert_eq!(c.total_changes(), changes);
                c.execute_batch("ROLLBACK")?;
                Ok(())
            })
            .unwrap();
        },
    );
}

#[test]
#[ignore = "owned SQL source-namespace traversal; no physical C Ready or CAS claim"]
fn source_namespace_validates_each_root_before_reading_the_next_payload() {
    child::run(
        concat!(
            module_path!(),
            "::source_namespace_validates_each_root_before_reading_the_next_payload"
        ),
        120,
        || {
            child::declare();
            with_reference("", |c| {
            schema_v2::initialize_reference(c)?;
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
            let b = binding();
            let root = SourceCompanionProtection::new(b.clone(), &[], 4096, None)?;
            c.execute("INSERT INTO family VALUES(1,?1)", [b"family".as_slice()])?;
            c.execute("INSERT INTO quota VALUES(1,1073741824)", [])?;
            c.execute("INSERT INTO worktrees VALUES(?1,'b',?2,?3,'ready')", params![b.source_owner.as_str(), b.source_database, b"b".as_slice()])?;
            c.execute("INSERT INTO worktrees VALUES(?1,'c',?2,?3,'initializing')", params![b.destination.owner.as_str(), b.destination.database_identity, b"c".as_slice()])?;
            root.reserve(c, None)?;
            root.protect(c, None)?;
            // This later malformed root is deliberate corruption, never a
            // fabricated valid protection. An eager prepass would fail on it
            // before delivering the first valid root to its caller.
            let later = "f".repeat(64);
            assert!(root.id() < later.as_str());
            c.execute_batch("SAVEPOINT later_root_corruption")?;
            c.execute("INSERT INTO roots(id,owner,database_identity,operation,digest,payload) VALUES(?1,?2,?3,'provider-companion:corrupt-later',?1,?4)",
                params![later, b.source_owner.as_str(), b.source_database, b"invalid-json".as_slice()])?;
            let changes = c.total_changes();
            let mut visited = 0;
            let refusal = retained_namespace(c, None, |seen| {
                visited += 1;
                assert_eq!(seen.id(), root.id());
                Err(invalid("caller-stop-before-later-root"))
            }).unwrap_err();
            assert_eq!(visited, 1);
            assert!(refusal.to_string().contains("caller-stop-before-later-root"));
            assert_eq!(c.total_changes(), changes);
            visited = 0;
            assert!(retained_namespace(c, None, |_| { visited += 1; Ok(()) }).is_err());
            assert_eq!(visited, 1, "later malformed root must not reach the caller");
            assert_eq!(c.total_changes(), changes);
            // End this injected-corruption SQL case. This is not a repair,
            // adoption or recovery operation on a production/native fixture.
            c.execute_batch("ROLLBACK TO later_root_corruption; RELEASE later_root_corruption")?;
            visited = 0;
            retained_namespace(c, None, |_| { visited += 1; Ok(()) })?;
            assert_eq!(visited, 1);
            c.execute("INSERT INTO reservations VALUES('provider-companion:orphan',?1,'provider-companion:orphan',1)", [b.source_owner.as_str()])?;
            let changes = c.total_changes();
            assert!(retained_namespace(c, None, |_| Ok(())).is_err());
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK")?;
            Ok(())
        }).unwrap();
        },
    );
}
