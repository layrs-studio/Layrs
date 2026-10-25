//! Recovery entrypoints must inspect legacy schemas without invoking migration.
use super::*;
use std::collections::BTreeMap;

fn schema_five() -> (Fixture, PathBuf) {
    let (fixture, store) = Fixture::new();
    drop(store);
    let database = fixture.root.join(".layrs/metadata/projection.sqlite3");
    let connection = rusqlite::Connection::open(&database).unwrap();
    // Genuine pre-selection schema, not just a changed header on a newer schema.
    connection
        .execute_batch("DROP TABLE projection_selections; PRAGMA user_version=5;")
        .unwrap();
    drop(connection);
    (fixture, database)
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            assert!(
                !kind.is_symlink(),
                "fixture must not traverse external content"
            );
            if kind.is_dir() {
                pending.push(entry.path());
            } else {
                assert!(kind.is_file());
                result.insert(
                    entry.path().strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    result
}

fn assert_unchanged(fixture: &Fixture, database: &Path, before: &BTreeMap<PathBuf, Vec<u8>>) {
    // Includes byte-for-byte raw DB and marker contents, plus sidecar creation/removal.
    assert_eq!(&files(&fixture.root), before);
    let connection =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .unwrap(),
        5
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name='projection_selections'",
                [],
                |row| row.get::<_, u32>(0)
            )
            .unwrap(),
        0
    );
    drop(connection);
    assert_eq!(fs::read(fixture.root.join("allowed.txt")).unwrap(), b"base");
    assert!(!fixture.root.join("private.txt").exists());
}

fn assert_schema_refusal(result: &Value) {
    let message = result["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("schema") && message.contains("migration"),
        "{message}"
    );
}

#[test]
fn schema_five_cancel_reservation_refuses_without_migrating_raw_database() {
    let (fixture, database) = schema_five();
    let nested = fixture.root.join("nested");
    fs::create_dir(&nested).unwrap();
    let before = files(&fixture.root);
    // Exercise cancellation's ancestor discovery: it must not open/migrate normally.
    let result = fixture.run(
        &[
            "--space",
            text(&nested),
            "partial",
            "offline",
            "cancel-reservation",
        ],
        false,
    );
    assert_schema_refusal(&result);
    assert_unchanged(&fixture, &database, &before);
}

#[test]
fn schema_five_absent_journal_resumes_refuse_without_migrating_raw_database() {
    for command in [
        "resume-installation",
        "resume-namespace",
        "resume-refresh",
        "resume-restore",
        "resume-rebase",
    ] {
        let (fixture, database) = schema_five();
        let before = files(&fixture.root);
        let result = fixture.run(&["partial", command], false);
        assert_schema_refusal(&result);
        assert_unchanged(&fixture, &database, &before);
    }
}
