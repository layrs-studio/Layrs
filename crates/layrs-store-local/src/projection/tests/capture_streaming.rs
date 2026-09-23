use super::super::capture::CAPTURE_CHUNK_BYTES;
use super::*;
use std::io::Write;

struct RefuseCommit;
impl CaptureFaults for RefuseCommit {
    fn check(&self, point: CapturePoint) -> io::Result<()> {
        if point == CapturePoint::BeforeCommit {
            Err(io::Error::other("interrupted before commit"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn large_capture_streams_immutable_chunks_and_rolls_back_atomically() {
    let (fixture, store) = super::capture::ready(false);
    let path = fixture.0.join("a.txt");
    let mut input = fs::File::create(&path).unwrap();
    let block = vec![0x5au8; CAPTURE_CHUNK_BYTES];
    for _ in 0..17 {
        input.write_all(&block).unwrap();
    }
    input.write_all(b"tail").unwrap();
    drop(input);
    let expected = FileId::from_bytes(&fs::read(&path).unwrap());
    assert!(store.capture_patch_with_faults(1, &RefuseCommit).is_err());
    assert!(store.latest_captured_patch().unwrap().is_none());
    let db =
        rusqlite::Connection::open(fixture.0.join(".layrs/metadata/projection.sqlite3")).unwrap();
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM projection_capture_chunks", [], |r| {
            r.get::<_, u64>(0)
        })
        .unwrap(),
        0
    );
    drop(db);
    let captured = store.capture_patch(1).unwrap();
    assert_eq!(
        captured.changes[0].replacement.as_ref().unwrap().file_id,
        expected
    );
    let repo_path = RepoPath::parse("a.txt").unwrap();
    let manifest = store
        .captured_file_manifest(captured.sequence, &repo_path)
        .unwrap();
    assert_eq!(manifest.file_id(), &expected);
    assert_eq!(manifest.byte_length(), 17 * CAPTURE_CHUNK_BYTES as u64 + 4);
    assert_eq!(manifest.chunks().len(), 18);
    let mut chunk = Vec::new();
    store
        .read_captured_chunk(captured.sequence, &repo_path, 17, &mut chunk)
        .unwrap();
    assert_eq!(chunk, b"tail");
    fs::write(&path, b"subsequent edit").unwrap();
    let reopened = ProjectionStore::open(&fixture.0).unwrap();
    let mut restored = Vec::new();
    reopened
        .write_captured_file(captured.sequence, &repo_path, &mut restored)
        .unwrap();
    assert_eq!(FileId::from_bytes(&restored), expected);
    assert_eq!(restored.len() as u64, manifest.byte_length());
    let db =
        rusqlite::Connection::open(fixture.0.join(".layrs/metadata/projection.sqlite3")).unwrap();
    db.execute(
        "UPDATE projection_capture_chunks SET bytes=X'00' WHERE file_id=?1 AND ordinal=0",
        [expected.as_str()],
    )
    .unwrap();
    drop(db);
    assert!(reopened.captured_patch(captured.sequence).is_err());
    let mut out = Vec::new();
    assert!(
        reopened
            .read_captured_chunk(captured.sequence, &repo_path, 0, &mut out)
            .is_err()
    );
    assert!(out.is_empty());
}

#[test]
fn legacy_blob_capture_survives_explicit_schema_migration() {
    let (fixture, store) = super::capture::ready(false);
    fs::write(fixture.0.join("a.txt"), b"legacy edit").unwrap();
    let captured = store.capture_patch(1).unwrap();
    let id = captured.changes[0]
        .replacement
        .as_ref()
        .unwrap()
        .file_id
        .as_str();
    let db =
        rusqlite::Connection::open(fixture.0.join(".layrs/metadata/projection.sqlite3")).unwrap();
    let bytes: Vec<u8> = db
        .query_row(
            "SELECT bytes FROM projection_capture_chunks WHERE file_id=?1 AND ordinal=0",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    db.execute(
        "INSERT INTO projection_capture_files(file_id,bytes) VALUES(?1,?2)",
        rusqlite::params![id, bytes],
    )
    .unwrap();
    db.execute(
        "DELETE FROM projection_capture_chunks WHERE file_id=?1",
        [id],
    )
    .unwrap();
    db.execute_batch("DROP TABLE projection_rebase_pending;
        DROP TABLE projection_rebase_history;
        DROP TABLE projection_rebase_previews;
        DROP TABLE projection_capture_chunks;
        DROP TABLE projection_namespace_pending;
        DROP TABLE projection_namespace_operations;
        CREATE TABLE projection_namespace_pending(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL CHECK(length(payload)<=65536)) STRICT;
        CREATE TABLE projection_namespace_operations(preview_digest TEXT PRIMARY KEY CHECK(length(preview_digest)=71),capture_sequence INTEGER NOT NULL REFERENCES projection_captures(sequence) ON DELETE RESTRICT,payload BLOB NOT NULL CHECK(length(payload)<=65536)) STRICT;
        PRAGMA user_version=3;")
        .unwrap();
    drop(db);
    let reopened = ProjectionStore::open(&fixture.0).unwrap();
    assert_eq!(
        reopened.captured_patch(captured.sequence).unwrap(),
        captured
    );
    let mut out = Vec::new();
    reopened
        .write_captured_file(
            captured.sequence,
            &RepoPath::parse("a.txt").unwrap(),
            &mut out,
        )
        .unwrap();
    assert_eq!(out, b"legacy edit");
    let db =
        rusqlite::Connection::open(fixture.0.join(".layrs/metadata/projection.sqlite3")).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        5
    );
}
