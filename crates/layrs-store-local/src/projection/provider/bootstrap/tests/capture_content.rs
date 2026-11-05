//! Native content-engine reuse with a validated, durable private capture intent.
use super::*;
use crate::projection::{
    capture::{CapturedChange, CapturedFile},
    provider::{
        records::{
            Record,
            capture::{
                ProviderCaptureStage, ProviderCaptureState, ProviderCaptureTransactionRecord,
            },
        },
        validation::{self, PrivateCaptureProof},
    },
    shared::{
        provider_access::{FamilyAccess, provider_capture::operation},
        provider_content_lease,
    },
};
use layrs_core::scm::{Chunk, ChunkingDescriptor, FileManifest, RepoPath, WorktreeId};
use rusqlite::{Connection, OpenFlags, params};

fn private_proof(
    f: &Fixture,
    allocation: &crate::projection::provider::records::AllocationRecord,
    prepared: &crate::projection::provider::records::PreparedRecord,
    binding: &ProviderBootstrapBinding,
) -> PrivateCaptureProof {
    let c = Connection::open_with_flags(
        f.metadata.join("projection.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    c.execute_batch("BEGIN DEFERRED").unwrap();
    let proof = validation::private_active_capture_with_source(
        &c,
        allocation,
        prepared,
        binding,
        Some([0x63; 16]),
        None,
    )
    .unwrap()
    .capture;
    c.close().unwrap();
    proof
}

fn private_write(f: &Fixture, operation: impl FnOnce(&Connection)) {
    let c = Connection::open(f.metadata.join("projection.sqlite3")).unwrap();
    c.execute_batch("BEGIN IMMEDIATE").unwrap();
    operation(&c);
    c.execute_batch("COMMIT").unwrap();
    c.close().unwrap();
}

fn family_counts(f: &Fixture) -> (u64, u64, u64) {
    let c = Connection::open_with_flags(
        f.family.root().join("metadata/catalog.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let counts = c.query_row(
        "SELECT (SELECT count(*) FROM reservations), (SELECT count(*) FROM installations), (SELECT count(*) FROM roots)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).unwrap();
    c.close().unwrap();
    counts
}

#[test]
#[ignore = "UNQUALIFIED until ROOT releases this owned native provider content driver recipe"]
fn durable_staging_reserve_install_protect_uses_rebound_proof() {
    child::run(
        concat!(
            module_path!(),
            "::durable_staging_reserve_install_protect_uses_rebound_proof"
        ),
        240,
        || {
            child::declare();
            hooks::reset();
            let (f, allocation, mount, activation, _, prepared, binding) =
                super::capture_family::active_with_writable_private();
            let bytes = b"provider capture installed content";
            let chunk = Chunk::new(bytes.to_vec()).unwrap();
            let expected_manifest = FileManifest::from_chunks(
                ChunkingDescriptor::deterministic_fixed(1024 * 1024).unwrap(),
                &[chunk],
            )
            .unwrap();
            let replacement = CapturedFile {
                file_id: expected_manifest.file_id().clone(),
                bytes: bytes.len() as u64,
                executable: false,
            };
            let reservation = "capture-content-reservation";
            let mut attempt = ProviderCaptureTransactionRecord::begin(
                &allocation,
                &mount,
                &activation,
                "capture-content-attempt".into(),
                0,
                None,
            )
            .unwrap();
            attempt.state = ProviderCaptureState::Staging {
                stage: ProviderCaptureStage {
                    image_digest: "c".repeat(64),
                    image_path_count: 1,
                    changes: vec![CapturedChange {
                        path: RepoPath::parse("README.md").unwrap(),
                        expected: None,
                        replacement: Some(replacement.clone()),
                    }],
                    reservation: reservation.into(),
                },
            };
            private_write(&f, |c| {
                let payload = attempt.encode().unwrap();
                c.execute("INSERT INTO projection_provider_capture_transactions VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![attempt.transaction_id,attempt.mount_id,attempt.capture_generation,attempt.capture_digest,attempt.expected_capture_sequence,attempt.expected_capture_digest,attempt.sql_state(),attempt.reservation(),payload,attempt.digest().unwrap()]).unwrap();
            });
            let proof = private_proof(&f, &allocation, &prepared, &binding);
            assert_eq!(proof.transactions.len(), 1);
            let family = FamilyAccess::open(f.family.root()).unwrap();
            let lease = provider_content_lease(
                family,
                crate::projection::provider::records::AllocationRecord::decode(
                    &allocation.encode().unwrap(),
                )
                .unwrap(),
                proof,
            )
            .unwrap();
            let op = operation("capture-content-attempt");
            let budget =
                crate::projection::retention::budget_for(std::slice::from_ref(&replacement), 0)
                    .unwrap();
            let before = family_counts(&f);
            let foreign_owner = WorktreeId::parse("worktree:foreign").unwrap();
            let foreign_manifest = FileManifest::from_chunks(
                ChunkingDescriptor::deterministic_fixed(1024 * 1024).unwrap(),
                &[Chunk::new(b"foreign content".to_vec()).unwrap()],
            )
            .unwrap();
            let foreign_file = CapturedFile {
                file_id: foreign_manifest.file_id().clone(),
                bytes: foreign_manifest.byte_length(),
                executable: false,
            };
            let wrong_mode = CapturedFile {
                executable: true,
                ..replacement.clone()
            };
            let mut foreign_marker = allocation.format.clone();
            foreign_marker.identity.worktree_id = foreign_owner.clone();
            assert!(
                lease
                    .reserve_exact(reservation, &foreign_owner, &op, budget)
                    .is_err()
            );
            assert!(
                lease
                    .reserve_exact("foreign-reservation", &f.worktree, &op, budget)
                    .is_err()
            );
            assert!(
                lease
                    .reserve_exact(reservation, &f.worktree, "foreign-operation", budget)
                    .is_err()
            );
            assert!(
                lease
                    .reserve_exact(reservation, &f.worktree, &op, budget + 1)
                    .is_err()
            );
            assert!(
                lease
                    .install_file(&mut bytes.as_slice(), &foreign_file, reservation)
                    .is_err()
            );
            assert!(
                lease
                    .install_file(&mut bytes.as_slice(), &wrong_mode, reservation)
                    .is_err()
            );
            assert!(
                lease
                    .install_file(&mut bytes.as_slice(), &replacement, "foreign-reservation")
                    .is_err()
            );
            assert!(lease.protect(&foreign_marker, &op, &[]).is_err());
            assert!(
                lease
                    .protect(&allocation.format, "foreign-operation", &[])
                    .is_err()
            );
            assert!(
                lease
                    .protect(&allocation.format, &op, &[foreign_manifest])
                    .is_err()
            );
            assert_eq!(family_counts(&f), before);
            assert!(
                lease
                    .release_accepted_reservation(
                        reservation,
                        &f.worktree,
                        "capture-content-attempt"
                    )
                    .is_err()
            );
            assert_eq!(family_counts(&f), before);
            lease
                .reserve_exact(reservation, &f.worktree, &op, budget)
                .unwrap();
            lease
                .reserve_exact(reservation, &f.worktree, &op, budget)
                .unwrap();
            let installed = lease
                .install_file(&mut bytes.as_slice(), &replacement, reservation)
                .unwrap();
            assert_eq!(installed, expected_manifest);
            drop(lease);
            private_write(&f, |c| {
                c.execute(
                    "INSERT INTO projection_content VALUES(?1,?2,?3)",
                    params![
                        replacement.file_id.as_str(),
                        installed.id().as_str(),
                        installed.byte_length()
                    ],
                )
                .unwrap();
            });
            let proof = private_proof(&f, &allocation, &prepared, &binding);
            assert_eq!(proof.content.len(), 1);
            let family = FamilyAccess::open(f.family.root()).unwrap();
            let lease = provider_content_lease(
                family,
                crate::projection::provider::records::AllocationRecord::decode(
                    &allocation.encode().unwrap(),
                )
                .unwrap(),
                proof,
            )
            .unwrap();
            let before_protection = family_counts(&f);
            assert!(lease.protect(&allocation.format, &op, &[]).is_err());
            let alternate = FileManifest::from_chunks(
                ChunkingDescriptor::deterministic_fixed(512 * 1024).unwrap(),
                &[Chunk::new(bytes.to_vec()).unwrap()],
            )
            .unwrap();
            assert_eq!(alternate.file_id(), installed.file_id());
            assert_ne!(alternate.id(), installed.id());
            assert!(
                lease
                    .protect(&allocation.format, &op, &[alternate])
                    .is_err()
            );
            assert_eq!(family_counts(&f), before_protection);
            let (root, digest) = lease
                .protect(&allocation.format, &op, &[installed.clone()])
                .unwrap();
            assert_eq!(root, digest);
            assert_eq!(
                lease
                    .verify_protection(&allocation.format, &root, &digest)
                    .unwrap(),
                vec![installed]
            );
            drop(lease);
            private_write(&f, |c| {
                c.execute(
                    "INSERT INTO projection_claims VALUES(?1,?2,'provider-capture',?3)",
                    params![root, digest, "capture-content-attempt"],
                )
                .unwrap();
            });
            let proof = private_proof(&f, &allocation, &prepared, &binding);
            assert_eq!(proof.claims.len(), 1);
            let family = FamilyAccess::open(f.family.root()).unwrap();
            let _final = provider_content_lease(family, allocation, proof).unwrap();
        },
    );
}
