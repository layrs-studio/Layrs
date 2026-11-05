//! Admitted private9/copied C8 SQL fixtures; image inventory is declared DATA.
//! These readers grant no physical checkout, family protection or Ready rights.
use super::*;
use crate::projection::provider::checkout_journal::{
    self, AdvanceEffect, CheckoutFile, CheckoutJournal, CheckoutRow, DeclareEffect,
    ExpectedCheckout, RowKey,
};
use rusqlite::types::ValueRef;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}
type State = BTreeMap<String, Vec<Vec<Cell>>>;
fn state(c: &Connection) -> State {
    let tables = {
        let mut q = c
            .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name LIMIT 65")
            .unwrap();
        let names = q
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(names.len() <= 64);
        names
    };
    let mut result = BTreeMap::new();
    let mut bytes = 0usize;
    for table in tables {
        assert!(
            table
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        );
        let mut q = c
            .prepare(&format!("SELECT * FROM \"{table}\" LIMIT 129"))
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
                    .collect::<Vec<_>>(),
            );
            assert!(bytes <= 16 * 1024 * 1024);
        }
        rows.sort();
        result.insert(table, rows);
    }
    result
}
fn incoming(stage: &ProviderTransitionIntentRecord) -> ProviderTransitionIntentRecord {
    let mut result = ProviderTransitionIntentRecord::decode(&stage.encode().unwrap()).unwrap();
    result.role = TransitionRole::Destination;
    result.local_generation = 0;
    result
}
fn fixture(
    body: impl FnOnce(
        &Connection,
        &format::Marker,
        &ProviderTransitionIntentRecord,
        &ProviderTransitionIntentRecord,
        &VerifiedImportClosure,
        &format::Marker,
        &Connection,
        &ExpectedCheckout,
    ),
) {
    with_admitted_source_sql(|source, marker, supplied| {
        let (copying, closure, target, _) = source_copying(source, marker, supplied);
        let bare = BareConnection::reference().unwrap();
        let c = bare.connection();
        c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
            .unwrap();
        let inputs = OrdinaryCopyInputs::new(source, marker, &copying, &closure);
        initialize_or_replay_copied(c, &target, &inputs, None).unwrap();
        let staging = ordinary_transition::proposed_staging(&copying).unwrap();
        ordinary_transition::apply_staging(
            source,
            marker,
            &copying.plan.source_prefix,
            &staging,
            None,
        )
        .unwrap();
        let inputs = OrdinaryCopyInputs::new(source, marker, &staging, &closure);
        super::super::staging::apply_staging(c, &target, &inputs, None).unwrap();
        let current = offline::snapshot_at(source, copying.local_generation, None).unwrap();
        let selected = selection::at(source, copying.local_generation).unwrap();
        let files: Vec<_> = current
            .snapshot
            .entries
            .iter()
            .filter(|e| selected.includes(&e.path))
            .filter_map(|entry| {
                let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
                    return None;
                };
                let manifest = supplied.iter().find(|m| m.file_id() == file_id).unwrap();
                Some(CheckoutFile {
                    path: entry.path.clone(),
                    manifest_id: manifest.id().clone(),
                    file_id: manifest.file_id().clone(),
                    bytes: manifest.byte_length(),
                    executable: entry.executable,
                })
            })
            .collect();
        assert!(!files.is_empty());
        let expected =
            ExpectedCheckout::ordinary(&incoming(&staging), &files, &[], IMAGE, None).unwrap();
        body(
            source, marker, &copying, &staging, &closure, &target, c, &expected,
        );
        c.execute_batch("ROLLBACK").unwrap();
        bare.finish(Ok(())).unwrap();
    });
}
fn inspect(
    c: &Connection,
    target: &format::Marker,
    inputs: &OrdinaryCopyInputs<'_>,
    expected: &ExpectedCheckout,
) -> CheckoutJournal {
    let before = state(c);
    let changes = c.total_changes();
    let actual = super::super::read_checkout_staging(c, target, inputs, expected, None).unwrap();
    assert_eq!(c.total_changes(), changes);
    assert_eq!(state(c), before);
    actual
}
fn refusal(
    source: &Connection,
    c: &Connection,
    target: &format::Marker,
    inputs: &OrdinaryCopyInputs<'_>,
    expected: &ExpectedCheckout,
) {
    let before = (state(source), state(c));
    let changes = (source.total_changes(), c.total_changes());
    assert!(super::super::read_checkout_staging(c, target, inputs, expected, None).is_err());
    assert_eq!((source.total_changes(), c.total_changes()), changes);
    assert_eq!((state(source), state(c)), before);
}
fn advance(
    c: &Connection,
    target: &format::Marker,
    inputs: &OrdinaryCopyInputs<'_>,
    expected: &ExpectedCheckout,
    key: RowKey,
    change: impl FnOnce(&CheckoutRow) -> Result<CheckoutRow>,
) -> (CheckoutRow, CheckoutRow) {
    let previous = inspect(c, target, inputs, expected)
        .row(key)
        .unwrap()
        .clone();
    let next = change(&previous).unwrap();
    assert_eq!(
        checkout_journal::advance(c, expected, &previous, &next, None).unwrap(),
        AdvanceEffect::Updated
    );
    assert_eq!(
        inspect(c, target, inputs, expected).row(key).unwrap(),
        &next
    );
    (previous, next)
}

owned!(
    ordinary_checkout_staging_reads_complete_inventory_advances_and_replays_read_only,
    {
        fixture(
            |source, marker, copying, staging, closure, target, c, expected| {
                let inputs = OrdinaryCopyInputs::new(source, marker, staging, closure);
                let before_source = state(source);
                let before = state(c);
                refusal(source, c, target, &inputs, expected);
                assert_eq!(
                    super::super::staging::read_stageable(c, target, &inputs, None).unwrap(),
                    TransitionState::Staging
                );
                assert_eq!(
                    checkout_journal::declare(c, expected, None).unwrap(),
                    DeclareEffect::Inserted
                );
                let journal = inspect(c, target, &inputs, expected);
                assert!(
                    !journal.is_complete(),
                    "complete inventory may still carry Declared progress"
                );
                let changes = c.total_changes();
                let declared = state(c);
                assert_eq!(
                    checkout_journal::declare(c, expected, None).unwrap(),
                    DeclareEffect::AlreadyPresent
                );
                assert!(
                    super::super::staging::read_stageable(c, target, &inputs, None).is_err(),
                    "old Staging never accepts checkout progress"
                );
                assert!(
                    super::super::read_copied(
                        c,
                        target,
                        &OrdinaryCopyInputs::new(source, marker, copying, closure),
                        None
                    )
                    .is_err()
                );
                assert_eq!(c.total_changes(), changes);
                assert_eq!(state(c), declared);
                advance(c, target, &inputs, expected, RowKey::Stage, |r| {
                    r.owned_directory("fs:v1:0000000000000001:0000000000006000")
                });
                for n in 0..expected.directories().len() {
                    advance(
                        c,
                        target,
                        &inputs,
                        expected,
                        RowKey::Directory(n as u32),
                        |r| {
                            r.owned_directory(&format!(
                                "fs:v1:0000000000000001:{:016x}",
                                0x6100 + n
                            ))
                        },
                    );
                }
                let mut last = None;
                for (n, file) in expected.files().iter().enumerate() {
                    let key = RowKey::File(n as u32);
                    advance(c, target, &inputs, expected, key, |r| {
                        r.owned_file(&format!("1:0:{}", 30000 + n))
                    });
                    advance(
                        c,
                        target,
                        &inputs,
                        expected,
                        key,
                        CheckoutRow::verified_file,
                    );
                    let parent = match file.path.as_str().rsplit_once('/') {
                        None => expected.header().checkout_identity.clone(),
                        Some((parent, _)) => format!(
                            "fs:v1:0000000000000001:{:016x}",
                            0x6100
                                + expected
                                    .directories()
                                    .iter()
                                    .position(|p| p.as_str() == parent)
                                    .unwrap()
                        ),
                    };
                    advance(c, target, &inputs, expected, key, |r| {
                        r.publishing_file(&parent)
                    });
                    last = Some(advance(
                        c,
                        target,
                        &inputs,
                        expected,
                        key,
                        CheckoutRow::published_file,
                    ));
                }
                assert!(inspect(c, target, &inputs, expected).is_complete());
                let complete = state(c);
                for (table, rows) in &before {
                    if table != "projection_prejournal" {
                        assert_eq!(&complete[table], rows, "checkout changed copied {table}");
                    }
                }
                assert_eq!(state(source), before_source);
                c.execute_batch(
                    "COMMIT; PRAGMA foreign_keys=OFF; PRAGMA query_only=ON; BEGIN DEFERRED",
                )
                .unwrap();
                let changes = c.total_changes();
                assert!(inspect(c, target, &inputs, expected).is_complete());
                assert_eq!(
                    checkout_journal::declare(c, expected, None).unwrap(),
                    DeclareEffect::AlreadyPresent
                );
                let (previous, next) = last.unwrap();
                assert_eq!(
                    checkout_journal::advance(c, expected, &previous, &next, None).unwrap(),
                    AdvanceEffect::AlreadyPresent
                );
                assert_eq!(c.total_changes(), changes);
                assert_eq!(state(c), complete);
                assert_eq!(state(source), before_source);
            },
        );
    }
);

owned!(
    ordinary_checkout_staging_refuses_foreign_partial_history_and_binding_changes_without_repair,
    {
        fixture(
            |source, marker, copying, staging, closure, target, c, expected| {
                let inputs = OrdinaryCopyInputs::new(source, marker, staging, closure);
                checkout_journal::declare(c, expected, None).unwrap();
                let old = OrdinaryCopyInputs::new(source, marker, copying, closure);
                refusal(source, c, target, &old, expected);
                let mut forged =
                    ProviderTransitionIntentRecord::decode(&staging.encode().unwrap()).unwrap();
                forged.destination.as_mut().unwrap().format_file_identity = "1:0:883".into();
                let wrong = ExpectedCheckout::ordinary(
                    &incoming(&forged),
                    expected.files(),
                    expected.deletions(),
                    IMAGE,
                    None,
                )
                .unwrap();
                refusal(source, c, target, &inputs, &wrong);
                c.execute_batch("SAVEPOINT destination_predecessor")
                    .unwrap();
                let predecessor = incoming(copying);
                c.execute("UPDATE projection_provider_transitions SET state='copying',reservation=NULL,payload=?1,digest=?2 WHERE id=?3", params![predecessor.encode().unwrap(),predecessor.digest().unwrap(),TX]).unwrap();
                refusal(source, c, target, &inputs, expected);
                c.execute_batch(
                    "ROLLBACK TO destination_predecessor; RELEASE destination_predecessor",
                )
                .unwrap();
                refusal(
                    source,
                    c,
                    target,
                    &OrdinaryCopyInputs::new(source, marker, &forged, closure),
                    expected,
                );
                let mut wrong_marker = target.clone();
                wrong_marker.metadata_identity = "fs:v1:0000000000000001:0000000000007000".into();
                refusal(source, c, &wrong_marker, &inputs, expected);
                let mut files = expected.files().to_vec();
                files[0].bytes += 1;
                let wrong =
                    ExpectedCheckout::ordinary(&incoming(staging), &files, &[], IMAGE, None)
                        .unwrap();
                refusal(source, c, target, &inputs, &wrong);
                for sql in [
                    "DELETE FROM projection_prejournal WHERE path LIKE '%:header'",
                    "INSERT INTO projection_prejournal VALUES('foreign','foreign','foreign',x'00','owned')",
                    "UPDATE projection_prejournal SET operation='foreign' WHERE path LIKE '%:stage'",
                    "UPDATE projection_restore_history SET payload=x'00'",
                    "UPDATE projection_provider_transitions SET reservation=NULL",
                    "DELETE FROM projection_imports",
                ] {
                    c.execute_batch("SAVEPOINT corrupt_checkout").unwrap();
                    c.execute_batch(sql).unwrap();
                    refusal(source, c, target, &inputs, expected);
                    c.execute_batch("ROLLBACK TO corrupt_checkout; RELEASE corrupt_checkout")
                        .unwrap();
                    inspect(c, target, &inputs, expected);
                }
                source
                    .execute_batch("SAVEPOINT source_predecessor")
                    .unwrap();
                source.execute("UPDATE projection_provider_transitions SET state='copying',reservation=NULL,payload=?1,digest=?2 WHERE id=?3",params![copying.encode().unwrap(),copying.digest().unwrap(),TX]).unwrap();
                refusal(source, c, target, &inputs, expected);
                source
                    .execute_batch("ROLLBACK TO source_predecessor; RELEASE source_predecessor")
                    .unwrap();
                let cancel = Cancellation::default();
                cancel.cancel();
                let before = (state(source), state(c));
                let changes = (source.total_changes(), c.total_changes());
                assert!(
                    super::super::read_checkout_staging(
                        c,
                        target,
                        &inputs,
                        expected,
                        Some(&cancel)
                    )
                    .is_err()
                );
                assert_eq!((source.total_changes(), c.total_changes()), changes);
                assert_eq!((state(source), state(c)), before);
                inspect(c, target, &inputs, expected);
            },
        );
    }
);
