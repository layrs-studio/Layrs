use std::fs;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use layrs_core::scm::{
    Metadata, Operation, OperationId, OperationKind, RepositoryView, Space, SpaceId, StoreId,
    Timestamp,
};
use layrs_store::scm::{
    ApplyBatchOutcome, CanonicalObject, EntityRecord, OperationHeadsCas, RepositoryViewCas,
    ScmRepository, SpaceObjectAdmission, StoreMutation, StoreObjectId, TransactionId, WriteBatch,
};
use layrs_store_local::scm_v1::{FsckOptions, NativeScmRepository, fsck_existing, init};

#[test]
fn concurrent_identical_writers_publish_once_and_replay_once() {
    let root = std::env::temp_dir().join(format!(
        "layrs-concurrency-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let store = StoreId::parse("store:concurrent").unwrap();
    drop(init(&root, store).unwrap());
    let t = Timestamp::parse("2026-07-20T00:00:00Z").unwrap();
    let space = Space::new(
        SpaceId::parse("space:concurrent").unwrap(),
        "Space",
        t.clone(),
        Metadata::empty(),
    )
    .unwrap();
    let view = RepositoryView::new(space.id().clone(), vec![], vec![], vec![], vec![]).unwrap();
    let op = Operation::new(
        OperationId::parse("operation:concurrent").unwrap(),
        space.id().clone(),
        vec![],
        view.id().clone(),
        OperationKind::Initialize,
        "Initialize",
        vec![],
        None,
        None,
        t,
        Metadata::empty(),
    )
    .unwrap();
    let batch = Arc::new(
        WriteBatch::new(
            TransactionId::new("transaction:concurrent").unwrap(),
            vec![CanonicalObject::RepositoryView(view.clone())],
            vec![
                EntityRecord::Space(space.clone()),
                EntityRecord::Operation(op.clone()),
            ],
            vec![SpaceObjectAdmission::new(
                space.id().clone(),
                StoreObjectId::RepositoryView(view.id().clone()),
            )],
            vec![
                StoreMutation::RepositoryView(
                    RepositoryViewCas::new(space.id().clone(), 0, None, view.id().clone()).unwrap(),
                ),
                StoreMutation::OperationHeads(
                    OperationHeadsCas::new(
                        space.id().clone(),
                        vec![],
                        op.id().clone(),
                        vec![op.id().clone()],
                    )
                    .unwrap(),
                ),
            ],
        )
        .unwrap(),
    );
    let barrier = Arc::new(Barrier::new(2));
    let mut joins = vec![];
    for _ in 0..2 {
        let root = root.clone();
        let batch = batch.clone();
        let barrier = barrier.clone();
        joins.push(thread::spawn(move || {
            barrier.wait();
            NativeScmRepository::new(root).apply_batch(&batch)
        }));
    }
    let results = joins
        .into_iter()
        .map(|j| j.join().unwrap().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, ApplyBatchOutcome::Applied { .. }))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, ApplyBatchOutcome::AlreadyApplied { .. }))
            .count(),
        1
    );
    assert_clean(&root);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn distinct_writers_on_the_same_frontier_have_one_cas_winner() {
    let root = std::env::temp_dir().join(format!(
        "layrs-concurrency-distinct-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let store = StoreId::parse("store:race").unwrap();
    drop(init(&root, store).unwrap());
    let t = Timestamp::parse("2026-07-20T00:00:00Z").unwrap();
    let space = Space::new(
        SpaceId::parse("space:race").unwrap(),
        "Space",
        t.clone(),
        Metadata::empty(),
    )
    .unwrap();
    let view = RepositoryView::new(space.id().clone(), vec![], vec![], vec![], vec![]).unwrap();
    let initial = Operation::new(
        OperationId::parse("operation:race-initial").unwrap(),
        space.id().clone(),
        vec![],
        view.id().clone(),
        OperationKind::Initialize,
        "Initialize",
        vec![],
        None,
        None,
        t.clone(),
        Metadata::empty(),
    )
    .unwrap();
    let initial_batch = WriteBatch::new(
        TransactionId::new("transaction:race-initial").unwrap(),
        vec![CanonicalObject::RepositoryView(view.clone())],
        vec![
            EntityRecord::Space(space.clone()),
            EntityRecord::Operation(initial.clone()),
        ],
        vec![SpaceObjectAdmission::new(
            space.id().clone(),
            StoreObjectId::RepositoryView(view.id().clone()),
        )],
        vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(space.id().clone(), 0, None, view.id().clone()).unwrap(),
            ),
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(
                    space.id().clone(),
                    vec![],
                    initial.id().clone(),
                    vec![initial.id().clone()],
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    NativeScmRepository::new(&root)
        .apply_batch(&initial_batch)
        .unwrap();
    let make = |suffix: &str| {
        let operation = Operation::new(
            OperationId::parse(format!("operation:race-{suffix}")).unwrap(),
            space.id().clone(),
            vec![initial.id().clone()],
            view.id().clone(),
            OperationKind::CaptureWorktree,
            format!("Race {suffix}"),
            vec![],
            None,
            None,
            t.clone(),
            Metadata::empty(),
        )
        .unwrap();
        WriteBatch::new(
            TransactionId::new(format!("transaction:race-{suffix}")).unwrap(),
            vec![],
            vec![EntityRecord::Operation(operation.clone())],
            vec![],
            vec![
                StoreMutation::RepositoryView(
                    RepositoryViewCas::new(
                        space.id().clone(),
                        1,
                        Some(view.id().clone()),
                        view.id().clone(),
                    )
                    .unwrap(),
                ),
                StoreMutation::OperationHeads(
                    OperationHeadsCas::new(
                        space.id().clone(),
                        vec![initial.id().clone()],
                        operation.id().clone(),
                        vec![operation.id().clone()],
                    )
                    .unwrap(),
                ),
            ],
        )
        .unwrap()
    };
    let batches = [Arc::new(make("left")), Arc::new(make("right"))];
    let barrier = Arc::new(Barrier::new(2));
    let joins = batches
        .into_iter()
        .map(|batch| {
            let root = root.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                NativeScmRepository::new(root).apply_batch(&batch)
            })
        })
        .collect::<Vec<_>>();
    let results = joins
        .into_iter()
        .map(|j| j.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Ok(ApplyBatchOutcome::Applied { .. })))
            .count(),
        1,
        "{results:#?}"
    );
    assert_eq!(
        results
            .iter()
            .filter(|r| r.as_ref().is_err_and(|e| e.code() == "cas_mismatch"))
            .count(),
        1,
        "{results:#?}"
    );
    assert_clean(&root);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn concurrent_writers_in_distinct_spaces_both_apply_without_overwrite() {
    let root = std::env::temp_dir().join(format!(
        "layrs-concurrency-spaces-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    drop(init(&root, StoreId::parse("store:spaces").unwrap()).unwrap());
    let make = |suffix: &str| {
        let timestamp = Timestamp::parse("2026-07-20T00:00:00Z").unwrap();
        let space = Space::new(
            SpaceId::parse(format!("space:{suffix}")).unwrap(),
            suffix,
            timestamp.clone(),
            Metadata::empty(),
        )
        .unwrap();
        let view = RepositoryView::new(space.id().clone(), vec![], vec![], vec![], vec![]).unwrap();
        let op = Operation::new(
            OperationId::parse(format!("operation:{suffix}")).unwrap(),
            space.id().clone(),
            vec![],
            view.id().clone(),
            OperationKind::Initialize,
            "Initialize",
            vec![],
            None,
            None,
            timestamp,
            Metadata::empty(),
        )
        .unwrap();
        WriteBatch::new(
            TransactionId::new(format!("transaction:{suffix}")).unwrap(),
            vec![CanonicalObject::RepositoryView(view.clone())],
            vec![
                EntityRecord::Space(space.clone()),
                EntityRecord::Operation(op.clone()),
            ],
            vec![SpaceObjectAdmission::new(
                space.id().clone(),
                StoreObjectId::RepositoryView(view.id().clone()),
            )],
            vec![
                StoreMutation::RepositoryView(
                    RepositoryViewCas::new(space.id().clone(), 0, None, view.id().clone()).unwrap(),
                ),
                StoreMutation::OperationHeads(
                    OperationHeadsCas::new(
                        space.id().clone(),
                        vec![],
                        op.id().clone(),
                        vec![op.id().clone()],
                    )
                    .unwrap(),
                ),
            ],
        )
        .unwrap()
    };
    let batches = [Arc::new(make("alpha")), Arc::new(make("beta"))];
    let barrier = Arc::new(Barrier::new(2));
    let joins = batches
        .into_iter()
        .map(|batch| {
            let root = root.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                NativeScmRepository::new(root).apply_batch(&batch)
            })
        })
        .collect::<Vec<_>>();
    let results = joins
        .into_iter()
        .map(|j| j.join().unwrap())
        .collect::<Vec<_>>();
    assert!(
        results
            .iter()
            .all(|r| matches!(r, Ok(ApplyBatchOutcome::Applied { .. }))),
        "{results:#?}"
    );
    use layrs_store::scm::ScmReader;
    let repository = NativeScmRepository::new(&root);
    for suffix in ["alpha", "beta"] {
        assert!(
            repository
                .current_repository_view(&SpaceId::parse(format!("space:{suffix}")).unwrap())
                .unwrap()
                .is_some()
        );
    }
    drop(repository);
    assert_clean(&root);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn concurrent_same_transaction_id_different_digests_has_one_winner_receipt() {
    let root = std::env::temp_dir().join(format!(
        "layrs-concurrency-replay-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    drop(init(&root, StoreId::parse("store:replay-race").unwrap()).unwrap());
    let make = |suffix: &str| {
        let t = Timestamp::parse("2026-07-20T00:00:00Z").unwrap();
        let space = Space::new(
            SpaceId::parse(format!("space:replay-{suffix}")).unwrap(),
            suffix,
            t.clone(),
            Metadata::empty(),
        )
        .unwrap();
        let view = RepositoryView::new(space.id().clone(), vec![], vec![], vec![], vec![]).unwrap();
        let op = Operation::new(
            OperationId::parse(format!("operation:replay-{suffix}")).unwrap(),
            space.id().clone(),
            vec![],
            view.id().clone(),
            OperationKind::Initialize,
            "Init",
            vec![],
            None,
            None,
            t,
            Metadata::empty(),
        )
        .unwrap();
        WriteBatch::new(
            TransactionId::new("transaction:shared-race").unwrap(),
            vec![CanonicalObject::RepositoryView(view.clone())],
            vec![
                EntityRecord::Space(space.clone()),
                EntityRecord::Operation(op.clone()),
            ],
            vec![SpaceObjectAdmission::new(
                space.id().clone(),
                StoreObjectId::RepositoryView(view.id().clone()),
            )],
            vec![
                StoreMutation::RepositoryView(
                    RepositoryViewCas::new(space.id().clone(), 0, None, view.id().clone()).unwrap(),
                ),
                StoreMutation::OperationHeads(
                    OperationHeadsCas::new(
                        space.id().clone(),
                        vec![],
                        op.id().clone(),
                        vec![op.id().clone()],
                    )
                    .unwrap(),
                ),
            ],
        )
        .unwrap()
    };
    let batches = [Arc::new(make("a")), Arc::new(make("b"))];
    let barrier = Arc::new(Barrier::new(2));
    let joins = batches
        .into_iter()
        .map(|batch| {
            let root = root.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                NativeScmRepository::new(root).apply_batch(&batch)
            })
        })
        .collect::<Vec<_>>();
    let results = joins
        .into_iter()
        .map(|j| j.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Ok(ApplyBatchOutcome::Applied { .. })))
            .count(),
        1,
        "{results:#?}"
    );
    assert_eq!(
        results
            .iter()
            .filter(|r| r
                .as_ref()
                .is_err_and(|e| e.code() == "transaction_replay_mismatch"))
            .count(),
        1,
        "{results:#?}"
    );
    let connection = rusqlite::Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    let receipts: i64 = connection
        .query_row("SELECT COUNT(*) FROM applied_transactions", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(receipts, 1);
    let spaces: i64 = connection
        .query_row("SELECT COUNT(*) FROM spaces", [], |r| r.get(0))
        .unwrap();
    assert_eq!(spaces, 1);
    drop(connection);
    assert_clean(&root);
    fs::remove_dir_all(root).unwrap();
}

fn assert_clean(root: &std::path::Path) {
    let connection = rusqlite::Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    assert_eq!(
        connection
            .query_row::<String, _, _>("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap(),
        "ok"
    );
    let foreign_keys: i64 = connection
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(foreign_keys, 0);
    drop(connection);
    let report = fsck_existing(root, FsckOptions::default()).unwrap();
    assert!(report.is_clean(), "{:#?}", report.findings);
}
