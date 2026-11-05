//! Exact family capture ownership under a durable Active private proof.
use super::*;
use crate::projection::{
    capture::{CapturedChange, CapturedFile, CapturedPatch, CapturedTarget},
    provider::{
        records::{
            AllocationRecord, PreparedRecord, Record,
            activation::{ProviderActivationRecord, ProviderActiveMountRecord},
            capture::{
                ProviderCaptureStage, ProviderCaptureState, ProviderCaptureTransactionRecord,
            },
        },
        validation::{self, CaptureContentProof, PrivateCaptureProof},
    },
    shared::provider_access::{FamilyAccess, provider_capture::operation},
};
use crate::secure_fs::PinnedDirectory;
use layrs_core::{
    projection::{AvailabilityRequest, OfflineOperation},
    scm::{
        Chunk, ChunkingDescriptor, FileManifest, RepoPath, TreeEntryTarget, WorktreeId,
        canonical_json,
    },
};
use rusqlite::{Connection, OpenFlags};
use std::{fs, io::Write};

macro_rules! owned {
    ($name:ident,$body:block) => {
        #[test]
        #[ignore = "UNQUALIFIED until ROOT releases this exact owned audited-host capture family recipe"]
        fn $name() {
            child::run(concat!(module_path!(), "::", stringify!($name)), 240, || {
                child::declare(); hooks::reset(); $body
            });
        }
    };
}

fn active() -> (
    Fixture,
    AllocationRecord,
    ProviderActiveMountRecord,
    ProviderActivationRecord,
    PrivateCaptureProof,
) {
    let (f, allocation, mount, activation, proof, _, _) = active_with_private();
    (f, allocation, mount, activation, proof)
}

pub(super) fn active_with_private() -> (
    Fixture,
    AllocationRecord,
    ProviderActiveMountRecord,
    ProviderActivationRecord,
    PrivateCaptureProof,
    PreparedRecord,
    ProviderBootstrapBinding,
) {
    active_with_private_from_holder(active_holder_with_private())
}

pub(super) fn active_with_writable_private() -> (
    Fixture,
    AllocationRecord,
    ProviderActiveMountRecord,
    ProviderActivationRecord,
    PrivateCaptureProof,
    PreparedRecord,
    ProviderBootstrapBinding,
) {
    active_with_private_from_holder(active_holder_with_writable_private())
}

fn active_with_private_from_holder(
    (f, holder, allocation, prepared, binding): (
        Fixture,
        super::super::preparation::ProviderActive,
        AllocationRecord,
        PreparedRecord,
        ProviderBootstrapBinding,
    ),
) -> (
    Fixture,
    AllocationRecord,
    ProviderActiveMountRecord,
    ProviderActivationRecord,
    PrivateCaptureProof,
    PreparedRecord,
    ProviderBootstrapBinding,
) {
    let c = Connection::open_with_flags(
        f.metadata.join("projection.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    c.execute_batch("BEGIN DEFERRED").unwrap();
    let inspected = validation::private_active_capture_with_source(
        &c,
        &allocation,
        &prepared,
        &binding,
        Some([0x63; 16]),
        None,
    )
    .unwrap();
    c.close().unwrap();
    drop(holder);
    (
        f,
        allocation,
        inspected.active.mount,
        inspected.active.activation,
        inspected.capture,
        prepared,
        binding,
    )
}

pub(in crate::projection::provider::bootstrap) fn active_holder_with_private() -> (
    Fixture,
    super::super::preparation::ProviderActive,
    AllocationRecord,
    PreparedRecord,
    ProviderBootstrapBinding,
) {
    active_holder_with_source(None)
}

pub(in crate::projection::provider::bootstrap) fn active_holder_with_writable_private() -> (
    Fixture,
    super::super::preparation::ProviderActive,
    AllocationRecord,
    PreparedRecord,
    ProviderBootstrapBinding,
) {
    let old = Fixture::source();
    let mut snapshot = old.initial().clone();
    let entry = snapshot
        .entries
        .iter_mut()
        .find(|entry| entry.path.as_str() == "README.md")
        .expect("G1 README entry");
    entry.writable = true;
    entry.state = layrs_core::projection::EntryState::Absent;
    entry.target = None;
    let descriptors = vec![];
    let source = crate::projection::provider::records::ProviderBootstrapSource::validate(
        snapshot,
        old.selection().clone(),
        descriptors,
    )
    .unwrap();
    active_holder_with_source(Some(source))
}

pub(in crate::projection::provider::bootstrap) fn active_holder_with_writable_file_private() -> (
    Fixture,
    super::super::preparation::ProviderActive,
    AllocationRecord,
    PreparedRecord,
    ProviderBootstrapBinding,
) {
    active_holder_with_source(Some(Fixture::writable_file_source()))
}

pub(in crate::projection::provider::bootstrap) fn active_holder_with_source(
    source: Option<crate::projection::provider::records::ProviderBootstrapSource>,
) -> (
    Fixture,
    super::super::preparation::ProviderActive,
    AllocationRecord,
    PreparedRecord,
    ProviderBootstrapBinding,
) {
    use super::super::preparation::{ProviderActivationOutcome, ProviderPreparationOutcome};
    let f = Fixture::new();
    let published = match source {
        Some(source) => f.published_with_source(source),
        None => f.published(),
    };
    let allocation =
        AllocationRecord::decode(&published.held.artifacts.allocation.encode().unwrap()).unwrap();
    let prepared = PreparedRecord::decode(
        &published
            .held
            .artifacts
            .prepared
            .as_ref()
            .unwrap()
            .encode()
            .unwrap(),
    )
    .unwrap();
    let binding =
        ProviderBootstrapBinding::decode(&published.held.binding.encode().unwrap()).unwrap();
    let prepared_holder = match published.prepare_provider([0x63; 16]) {
        ProviderPreparationOutcome::Prepared(value) => value,
        ProviderPreparationOutcome::Withheld(value) => {
            panic!("capture prepare withheld: {:?}", value.failure())
        }
    };
    let holder = match prepared_holder.activate() {
        ProviderActivationOutcome::Active(value) => value,
        ProviderActivationOutcome::Withheld(value) => {
            panic!("capture activation withheld: {:?}", value.failure())
        }
    };
    holder.revalidate().unwrap();
    (f, holder, allocation, prepared, binding)
}

fn family_read(f: &Fixture, allocation: &AllocationRecord, proof: &PrivateCaptureProof) -> bool {
    family_error(f, allocation, proof).is_none()
}

fn family_error(
    f: &Fixture,
    allocation: &AllocationRecord,
    proof: &PrivateCaptureProof,
) -> Option<String> {
    let family = FamilyAccess::open(f.family.root()).unwrap();
    let _locks = family.locks(&f.worktree, false).unwrap();
    let c = Connection::open_with_flags(
        f.family.root().join("metadata/catalog.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    c.execute_batch("BEGIN DEFERRED").unwrap();
    let checked = family.validate_capture_catalog(&c, allocation, proof, None);
    c.close().unwrap();
    checked.err().map(|error| error.to_string())
}

fn family_write(f: &Fixture, write: impl FnOnce(&Connection)) {
    family_mutate(f, false, write);
}

fn family_corrupt(f: &Fixture, write: impl FnOnce(&Connection)) {
    family_mutate(f, true, write);
}

fn family_mutate(f: &Fixture, corrupt: bool, write: impl FnOnce(&Connection)) {
    let family = FamilyAccess::open(f.family.root()).unwrap();
    let _locks = family.locks(&f.worktree, true).unwrap();
    let c = Connection::open(f.family.root().join("metadata/catalog.sqlite3")).unwrap();
    if corrupt {
        // Only this fresh owned mutation connection may inject broken FK keys.
        // The production reader must reject them before materializing Strings.
        c.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    }
    c.execute_batch("BEGIN IMMEDIATE").unwrap();
    write(&c);
    c.execute_batch("COMMIT").unwrap();
    c.close().unwrap();
}

fn root_payload(allocation: &AllocationRecord, operation: &str) -> (String, Vec<u8>) {
    root_payload_with(allocation, operation, &[])
}

fn root_payload_with(
    allocation: &AllocationRecord,
    operation: &str,
    manifests: &[FileManifest],
) -> (String, Vec<u8>) {
    let payload = canonical_json(&serde_json::json!({
        "incarnation":allocation.family.incarnation,
        "owner":allocation.format.identity.worktree_id,
        "databaseIdentity":allocation.private.database_file_identity,
        "operation":operation,
        "manifests":manifests,
    }))
    .unwrap();
    let mut h = blake3::Hasher::new();
    h.update(b"layrs-partial-retention-v1\0");
    h.update(&payload);
    (h.finalize().to_hex().to_string(), payload)
}

owned!(
    exact_staged_protection_before_claim_and_family_forgery_refusal,
    {
        let (f, allocation, mount, activation, mut proof) = active();
        assert!(family_read(&f, &allocation, &proof));
        let stage = ProviderCaptureStage {
            image_digest: "a".repeat(64),
            image_path_count: 0,
            changes: vec![],
            reservation: "capture-reservation-one".into(),
        };
        let mut attempt = ProviderCaptureTransactionRecord::begin(
            &allocation,
            &mount,
            &activation,
            "capture-attempt-one".into(),
            0,
            None,
        )
        .unwrap();
        attempt.state = ProviderCaptureState::Staging {
            stage: stage.clone(),
        };
        proof.transactions.push(attempt.clone());
        assert!(family_read(&f, &allocation, &proof)); // durable private intent precedes reservation
        let op = operation(&attempt.transaction_id);
        family_write(&f, |c| {
            c.execute(
                "INSERT INTO reservations VALUES(?1,?2,?3,0)",
                rusqlite::params![stage.reservation, f.worktree.as_str(), op],
            )
            .unwrap();
        });
        assert!(family_read(&f, &allocation, &proof));
        family_write(&f, |c| {
            c.execute(
                "UPDATE reservations SET operation='other' WHERE id=?1",
                [&stage.reservation],
            )
            .unwrap();
        });
        assert!(!family_read(&f, &allocation, &proof));
        family_write(&f, |c| {
            c.execute(
                "UPDATE reservations SET operation=?1 WHERE id=?2",
                rusqlite::params![op, stage.reservation],
            )
            .unwrap();
        });
        let (id, payload) = root_payload(&allocation, &op);
        family_write(&f, |c| {
            c.execute(
                "INSERT INTO roots VALUES(?1,?2,?3,?4,?5,?6)",
                rusqlite::params![
                    id,
                    f.worktree.as_str(),
                    allocation.private.database_file_identity,
                    op,
                    id,
                    payload
                ],
            )
            .unwrap();
        });
        assert!(family_read(&f, &allocation, &proof)); // protect durable before private claim
        family_write(&f, |c| {
            c.execute(
                "UPDATE roots SET database_identity='forged' WHERE id=?1",
                [&id],
            )
            .unwrap();
        });
        assert!(!family_read(&f, &allocation, &proof));
        family_write(&f, |c| {
            c.execute(
                "UPDATE roots SET database_identity=?1 WHERE id=?2",
                rusqlite::params![allocation.private.database_file_identity, id],
            )
            .unwrap();
            c.execute(
                "INSERT INTO roots VALUES('unknown',?1,?2,'other','unknown',x'7b7d')",
                rusqlite::params![
                    f.worktree.as_str(),
                    allocation.private.database_file_identity
                ],
            )
            .unwrap();
        });
        assert!(!family_read(&f, &allocation, &proof)); // unrelated target protection
        family_write(&f, |c| {
            c.execute("DELETE FROM roots WHERE id='unknown'", [])
                .unwrap();
        });
        assert!(family_read(&f, &allocation, &proof));
        family_write(&f, |c| {
            c.execute("DELETE FROM roots WHERE id=?1", [&id]).unwrap();
        });
        attempt.state = ProviderCaptureState::Accepted {
            stage,
            patch: CapturedPatch {
                schema_version: "layrs.projection-capture.v1".into(),
                sequence: 1,
                previous_patch_digest: None,
                projection_generation: 1,
                projection_digest: activation.activated_digest.clone(),
                identity: allocation.format.identity.clone(),
                changes: vec![],
                digest: "blake3:".to_owned() + &"0".repeat(64),
            },
        };
        proof.transactions[0] = attempt;
        assert!(!family_read(&f, &allocation, &proof)); // accepted cannot lose protection
    }
);

owned!(
    staged_private_content_requires_exact_family_manifest_index,
    {
        let (f, allocation, mount, activation, mut proof) = active();
        let bytes = b"capture-family-content";
        let chunk = Chunk::new(bytes.to_vec()).unwrap();
        let manifest = FileManifest::from_chunks(
            ChunkingDescriptor::deterministic_fixed(1024 * 1024).unwrap(),
            &[chunk],
        )
        .unwrap();
        let old = Fixture::source();
        let mut snapshot = old.initial().clone();
        let entry = snapshot.entries.first_mut().unwrap();
        let Some(TreeEntryTarget::File { file_id }) = &mut entry.target else {
            panic!("G1 expected file")
        };
        let expected = CapturedTarget {
            file_id: file_id.clone(),
            executable: entry.executable,
        };
        *file_id = manifest.file_id().clone();
        let ordinary_root = f.outer.join("ordinary");
        fs::create_dir(&ordinary_root).unwrap();
        let ordinary = f
            .family
            .create_worktree(
                &ordinary_root,
                WorktreeId::parse("worktree:capture-content-other").unwrap(),
                snapshot,
                old.selection().clone(),
            )
            .unwrap();
        let request = AvailabilityRequest {
            version: ordinary.current_version().unwrap(),
            other: None,
            paths: vec![RepoPath::parse("README.md").unwrap()],
            operation: OfflineOperation::Read,
        };
        let preparation = ordinary.begin_preparation(&request).unwrap();
        ordinary
            .resume_preparation(&preparation.id, &mut |_, _, out| out.write_all(bytes))
            .unwrap();
        let stage = ProviderCaptureStage {
            image_digest: "b".repeat(64),
            image_path_count: 1,
            changes: vec![CapturedChange {
                path: RepoPath::parse("README.md").unwrap(),
                expected: Some(expected),
                replacement: Some(CapturedFile {
                    file_id: manifest.file_id().clone(),
                    bytes: bytes.len() as u64,
                    executable: false,
                }),
            }],
            reservation: "capture-reservation-index".into(),
        };
        let mut attempt = ProviderCaptureTransactionRecord::begin(
            &allocation,
            &mount,
            &activation,
            "capture-attempt-index".into(),
            0,
            None,
        )
        .unwrap();
        attempt.state = ProviderCaptureState::Staging {
            stage: stage.clone(),
        };
        let op = operation(&attempt.transaction_id);
        proof.transactions.push(attempt);
        proof.content.push(CaptureContentProof {
            file_id: manifest.file_id().as_str().into(),
            manifest_id: manifest.id().as_str().into(),
            bytes: bytes.len() as u64,
        });
        family_write(&f, |c| {
            c.execute(
                "INSERT INTO reservations VALUES(?1,?2,?3,0)",
                rusqlite::params![stage.reservation, f.worktree.as_str(), op],
            )
            .unwrap();
        });
        assert!(family_read(&f, &allocation, &proof));
        family_write(&f, |c| {
            c.execute(
                "UPDATE reservations SET owner='worktree:capture-content-other' WHERE id=?1",
                [&stage.reservation],
            )
            .unwrap();
        });
        assert!(!family_read(&f, &allocation, &proof));
        family_write(&f, |c| {
            c.execute(
                "UPDATE reservations SET owner=?1 WHERE id=?2",
                rusqlite::params![f.worktree.as_str(), stage.reservation],
            )
            .unwrap();
        });
        assert!(family_read(&f, &allocation, &proof));
        let staging = PinnedDirectory::open(&f.family.root().join("staging")).unwrap();
        let directory = staging.ensure_private_child("123456".as_ref()).unwrap();
        let directory_identity = directory.identity_token();
        let intent = canonical_json(&serde_json::json!({
            "incarnation":allocation.family.incarnation,
            "reservation":stage.reservation,
            "owner":f.worktree,
            "databaseIdentity":allocation.private.database_file_identity,
            "operation":op,
            "fileId":manifest.file_id(),
            "bytes":bytes.len(),
            "directory":"123456",
            "directoryIdentity":directory_identity,
        }))
        .unwrap();
        let manifest_bytes = canonical_json(&manifest).unwrap();
        family_write(&f, |c| {
            c.execute(
                "INSERT INTO installations VALUES(?1,?2,?3,?4,0)",
                rusqlite::params![
                    stage.reservation,
                    manifest.file_id().as_str(),
                    intent,
                    manifest_bytes
                ],
            )
            .unwrap();
        });
        // The ordinary member has the same immutable manifest in objects;
        // staging/manifest is absent, as after a move before catalog commit.
        assert!(family_read(&f, &allocation, &proof));
        fs::write(f.family.root().join("staging/123456/manifest"), b"wrong").unwrap();
        assert!(!family_read(&f, &allocation, &proof)); // corrupt staging cannot fall back to object
        fs::remove_file(f.family.root().join("staging/123456/manifest")).unwrap();
        assert!(family_read(&f, &allocation, &proof));
        drop(directory);
        staging.remove_empty_child("123456".as_ref()).unwrap();
        // A verified unpublished attempt cannot lose its pinned directory.
        assert!(!family_read(&f, &allocation, &proof));
        family_write(&f, |c| {
            c.execute(
                "UPDATE installations SET manifest=NULL WHERE reservation=?1",
                [&stage.reservation],
            )
            .unwrap();
        });
        // Exact unverified intent after cleanup, before deleting its SQL row.
        assert!(family_read(&f, &allocation, &proof));
        let oversized = "x".repeat(131072);
        family_corrupt(&f, |c| {
            c.execute(
                "UPDATE manifest_chunks SET chunk_id=?1 WHERE manifest_id=?2",
                rusqlite::params![oversized, manifest.id().as_str()],
            )
            .unwrap();
        });
        assert!(
            family_error(&f, &allocation, &proof)
                .unwrap()
                .contains("chunk index key exceeds bound")
        );
        family_write(&f, |c| {
            c.execute(
                "UPDATE manifest_chunks SET chunk_id=?1 WHERE manifest_id=?2",
                rusqlite::params![
                    manifest.chunks()[0].chunk_id().as_str(),
                    manifest.id().as_str()
                ],
            )
            .unwrap();
        });
        assert!(family_read(&f, &allocation, &proof));
        let (root_id, root_bytes) = root_payload_with(&allocation, &op, &[manifest.clone()]);
        family_corrupt(&f, |c| {
            c.execute(
                "INSERT INTO roots VALUES(?1,?2,?3,?4,?5,?6)",
                rusqlite::params![
                    root_id,
                    f.worktree.as_str(),
                    allocation.private.database_file_identity,
                    op,
                    root_id,
                    root_bytes
                ],
            )
            .unwrap();
            c.execute(
                "INSERT INTO root_manifests VALUES(?1,?2)",
                rusqlite::params![root_id, oversized],
            )
            .unwrap();
        });
        assert!(
            family_error(&f, &allocation, &proof)
                .unwrap()
                .contains("root manifest key exceeds bound")
        );
        family_write(&f, |c| {
            c.execute(
                "UPDATE root_manifests SET manifest_id=?1 WHERE root_id=?2",
                rusqlite::params![manifest.id().as_str(), root_id],
            )
            .unwrap();
        });
        assert!(family_read(&f, &allocation, &proof));
        family_write(&f, |c| {
            c.execute(
                "UPDATE manifest_chunks SET ordinal=1 WHERE manifest_id=?1 AND ordinal=0",
                [manifest.id().as_str()],
            )
            .unwrap();
        });
        assert!(!family_read(&f, &allocation, &proof));
    }
);
