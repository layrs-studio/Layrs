use super::*;
use crate::projection::{
    provider::{database_io::with_reference, owned_test as child},
    shared::schema_v2,
};
use journal::{Active, Piece};
use layrs_core::scm::{Chunk, ChunkingDescriptor};
use rusqlite::params;

fn binding() -> ExactBinding {
    let token = format!("blake3:{}", "1".repeat(64));
    let operation = format!("provider-transition:{}", "2".repeat(64));
    ExactBinding {
        family_incarnation: "family".into(),
        family_root_identity: "fs:v1:0000000000000001:0000000000000002".into(),
        family_marker_digest: token.clone(),
        family_catalog_identity: "1:0:3".into(),
        plan_digest: token.clone(),
        prefix_id: token.clone(),
        destination_proof_digest: token,
        transaction_id: "0".repeat(32),
        owner: WorktreeId::parse("worktree:exact").unwrap(),
        database_identity: "1:0:4".into(),
        reservation: operation.clone(),
        operation,
        quota_bytes: 1024 * 1024,
    }
}
fn manifest() -> FileManifest {
    let bytes = Chunk::new(b"abc".to_vec()).unwrap();
    FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(3).unwrap(),
        &[bytes.clone(), bytes],
    )
    .unwrap()
}

#[test]
fn codec_binds_phase_cursor_and_exact_representations() {
    let manifest = manifest();
    let record = ExactRecord::declared(binding(), &manifest, "1-2-3".into());
    let bytes = record.encode(&manifest).unwrap();
    assert_eq!(ExactRecord::decode(&bytes, &manifest).unwrap(), record);
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["spoolCursor"] = 1.into();
    assert!(ExactRecord::decode(&canonical_json(&value).unwrap(), &manifest).is_err());
    value["spoolCursor"] = 0.into();
    value["unrecognized"] = true.into();
    assert!(ExactRecord::decode(&canonical_json(&value).unwrap(), &manifest).is_err());
    let other = FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(6).unwrap(),
        &[Chunk::new(b"abcabc".to_vec()).unwrap()],
    )
    .unwrap();
    assert_eq!(other.file_id(), manifest.file_id());
    assert_ne!(other.id(), manifest.id());
    assert!(ExactRecord::decode(&bytes, &other).is_err());
}

fn populate(c: &Connection, binding: &ExactBinding) {
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
}
fn replace(c: &Connection, record: &mut ExactRecord, next: ExactRecord, manifest: &FileManifest) {
    sql::replace(c, record, &next, manifest).unwrap();
    *record = next;
}
fn active(c: &Connection, record: &mut ExactRecord, piece: Piece, manifest: &FileManifest) {
    let mut next = record.clone();
    next.active = Some(Active {
        piece,
        inode: "1:0:77".into(),
        from_cas: false,
    });
    replace(c, record, next, manifest);
}

fn assert_usage_matches(c: &Connection, binding: &ExactBinding, manifest: &FileManifest) {
    let exact = sql::validate_installations(c, binding, &[manifest.clone()], None).unwrap();
    let shared = crate::projection::shared::usage(c).unwrap();
    assert_eq!(shared.content_bytes, exact.content_bytes);
    assert_eq!(shared.reserved_bytes, exact.reserved_bytes);
    assert_eq!(shared.limit_bytes, exact.limit_bytes);
    assert_eq!(
        shared.content_bytes + shared.reserved_bytes,
        binding.quota_bytes,
        "prepare/verify/publication transfers charge atomically, never releases it early"
    );
}

#[test]
#[ignore = "owned audited SQLite child; exact journal debit, duplicate chunk and replay projections"]
fn exact_journal_debits_duplicate_chunks_once_and_replays_without_mutation() {
    child::run(
        concat!(
            module_path!(),
            "::exact_journal_debits_duplicate_chunks_once_and_replays_without_mutation"
        ),
        120,
        || {
            child::declare();
            with_reference("", |c| {
                schema_v2::initialize_reference(c)?;
                let binding = binding();
                let manifest = manifest();
                let payload = journal::manifest_bytes(&manifest)?;
                populate(c, &binding);
                c.execute_batch("BEGIN IMMEDIATE")?;
                let mut record = ExactRecord::declared(binding.clone(), &manifest, "1-2-3".into());
                sql::insert(c, &record, &manifest)?;
                assert_usage_matches(c, &binding, &manifest);
                assert_eq!(
                    sql::validate_installations(c, &binding, &[manifest.clone()], None)?
                        .debit_bytes,
                    PREPARE_CHARGE
                );
                let mut next = record.clone();
                next.phase = Phase::Allocated;
                next.directory_identity = Some("fs:v1:0000000000000001:0000000000000009".into());
                replace(c, &mut record, next, &manifest);
                for n in 0..2 {
                    active(c, &mut record, Piece::Chunk(n), &manifest);
                    let mut next = record.clone();
                    next.active = None;
                    next.spool_cursor += 1;
                    replace(c, &mut record, next, &manifest);
                }
                active(c, &mut record, Piece::Manifest, &manifest);
                let mut next = record.clone();
                next.active = None;
                next.phase = Phase::Verified;
                next.debit_bytes += payload.len() as u64;
                replace(c, &mut record, next, &manifest);
                let mut next = record.clone();
                next.phase = Phase::Publishing;
                assert_usage_matches(c, &binding, &manifest);
                replace(c, &mut record, next, &manifest);
                active(c, &mut record, Piece::Chunk(0), &manifest);
                c.execute(
                    "INSERT INTO chunks VALUES(?1,3)",
                    [manifest.chunks()[0].chunk_id().as_str()],
                )?;
                let mut next = record.clone();
                next.active = None;
                next.publish_cursor = 1;
                next.charge_chunk(0, 3)?;
                replace(c, &mut record, next, &manifest);
                active(c, &mut record, Piece::Chunk(1), &manifest);
                assert!(sql::chunk_indexed(
                    c,
                    manifest.chunks()[1].chunk_id().as_str(),
                    3
                )?);
                let mut next = record.clone();
                next.active = None;
                next.publish_cursor = 2;
                replace(c, &mut record, next, &manifest);
                let state = sql::validate_installations(c, &binding, &[manifest.clone()], None)?;
                assert_eq!(state.debit_bytes, PREPARE_CHARGE + payload.len() as u64 + 3);
                let mut duplicate = record.clone();
                duplicate.charge_chunk(1, 3)?;
                assert!(
                    duplicate.encode(&manifest).is_err(),
                    "same immutable chunk cannot be charged twice"
                );
                active(c, &mut record, Piece::Manifest, &manifest);
                c.execute(
                    "INSERT INTO files VALUES(?1,6)",
                    [manifest.file_id().as_str()],
                )?;
                c.execute(
                    "INSERT INTO manifests VALUES(?1,?2,6,?3)",
                    params![manifest.id().as_str(), manifest.file_id().as_str(), payload],
                )?;
                for n in 0..2 {
                    c.execute(
                        "INSERT INTO manifest_chunks VALUES(?1,?2,?3)",
                        params![
                            manifest.id().as_str(),
                            n,
                            manifest.chunks()[0].chunk_id().as_str()
                        ],
                    )?;
                }
                let mut next = record.clone();
                next.active = None;
                next.phase = Phase::Published;
                next.charged_manifest = true;
                next.debit_bytes += payload.len() as u64;
                replace(c, &mut record, next, &manifest);
                let before = c.total_changes();
                assert_usage_matches(c, &binding, &manifest);
                sql::replace(c, &record, &record, &manifest)?;
                assert_eq!(c.total_changes(), before);
                let state = sql::validate_installations(c, &binding, &[manifest.clone()], None)?;
                assert_eq!(state.published, 1);
                assert_eq!(
                    state.debit_bytes,
                    PREPARE_CHARGE + 2 * payload.len() as u64 + 3
                );
                assert_eq!(state.remaining, binding.quota_bytes - state.debit_bytes);
                assert_eq!(state.content_bytes, state.debit_bytes);
                c.execute_batch("ROLLBACK")?;
                assert_eq!(
                    c.query_row("SELECT remaining FROM reservations", [], |r| r
                        .get::<_, u64>(0))?,
                    binding.quota_bytes
                );
                assert_eq!(
                    c.query_row("SELECT count(*) FROM exact_installations", [], |r| r
                        .get::<_, u64>(0))?,
                    0
                );
                Ok(())
            })
            .unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited SQLite child; exact journal refuses incomplete binding, quota and foreign rowsets"]
fn exact_journal_refuses_foreign_rows_and_insufficient_fixed_charge() {
    child::run(
        concat!(
            module_path!(),
            "::exact_journal_refuses_foreign_rows_and_insufficient_fixed_charge"
        ),
        120,
        || {
            child::declare();
            with_reference("", |c| {
                schema_v2::initialize_reference(c)?;
                let binding = binding();
                let manifest = manifest();
                populate(c, &binding);
                c.execute_batch("BEGIN IMMEDIATE")?;
                let record = ExactRecord::declared(binding.clone(), &manifest, "1-2-3".into());
                c.execute("UPDATE reservations SET remaining=8191", [])?;
                let before = c.total_changes();
                assert!(sql::insert(c, &record, &manifest).is_err());
                assert_eq!(c.total_changes(), before);
                c.execute(
                    "UPDATE reservations SET remaining=?1",
                    [binding.quota_bytes],
                )?;
                sql::insert(c, &record, &manifest)?;
                let mut unknown = binding.clone();
                unknown.operation = format!("provider-transition:{}", "3".repeat(64));
                unknown.reservation = unknown.operation.clone();
                assert!(sql::load(c, &unknown, &manifest)?.is_none());
                let other = FileManifest::from_chunks(
                    ChunkingDescriptor::deterministic_fixed(6).unwrap(),
                    &[Chunk::new(b"abcabc".to_vec()).unwrap()],
                )
                .unwrap();
                assert!(sql::validate_installations(c, &binding, &[other], None).is_err());
                c.execute("UPDATE reservations SET remaining=remaining+1", [])?;
                assert!(sql::validate_installations(c, &binding, &[manifest], None).is_err());
                c.execute_batch("ROLLBACK")?;
                Ok(())
            })
            .unwrap();
        },
    );
}
