use super::*;

#[test]
fn guarded_accept_checks_before_cas_and_before_commit_with_rollback_and_replay() {
    let root = unique_root("guarded");
    let batch = initialize(&root, "guarded");
    let operation = OperationId::parse("operation:guarded").unwrap();
    let mut repository = NativeScmRepository::new(&root);
    let failure = || layrs_store::scm::StoreError::Backend {
        message: "filesystem guard refused".into(),
    };
    assert!(
        repository
            .apply_batch_guarded(&batch, || Err(failure()))
            .is_err()
    );
    let store = open(&root).unwrap();
    let object = batch.canonical_objects()[0].object_id();
    assert!(
        !NativeCas::open(&store)
            .unwrap()
            .object_path(&object)
            .unwrap()
            .exists()
    );
    drop(store);
    let mut checks = 0;
    assert!(
        repository
            .apply_batch_guarded(&batch, || {
                checks += 1;
                if checks == 2 { Err(failure()) } else { Ok(()) }
            })
            .is_err()
    );
    assert_eq!(checks, 2);
    assert!(repository.get_operation(&operation).unwrap().is_none());
    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    assert_eq!(
        connection
            .query_row::<i64, _, _>("SELECT COUNT(*) FROM applied_transactions", [], |row| row
                .get(0))
            .unwrap(),
        0
    );
    drop(connection);
    assert!(matches!(
        repository.apply_batch_guarded(&batch, || Ok(())).unwrap(),
        ApplyBatchOutcome::Applied { .. }
    ));
    let mut replay_checks = 0;
    assert!(matches!(
        repository
            .apply_batch_guarded(&batch, || {
                replay_checks += 1;
                Ok(())
            })
            .unwrap(),
        ApplyBatchOutcome::AlreadyApplied { .. }
    ));
    assert_eq!(replay_checks, 2);
    assert!(repository.get_operation(&operation).unwrap().is_some());
    assert_clean_db(&root);
    cleanup(&root);
}

#[test]
fn existing_receipt_preserves_cas_before_replay_collision_and_corruption_checks() {
    struct CountAfterCas(AtomicU64);
    impl RepositoryFaultInjector for CountAfterCas {
        fn check(&self, point: RepositoryFaultPoint) -> Result<(), String> {
            if point == RepositoryFaultPoint::AfterAllCas {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }
    let root = unique_root("receipt-before-preflight");
    let original = initialize(&root, "receipt-before-preflight");
    NativeScmRepository::new(&root)
        .apply_batch(&original)
        .unwrap();
    let checks = Arc::new(CountAfterCas(AtomicU64::new(0)));
    let mut repository = NativeScmRepository::with_fault_injector(&root, checks.clone());
    // These original CAS expectations are now stale. An existing receipt must
    // bypass the advisory precheck and still reach the original replay checks.
    assert!(matches!(
        repository.apply_batch(&original).unwrap(),
        ApplyBatchOutcome::AlreadyApplied { .. }
    ));
    assert_eq!(checks.0.load(Ordering::SeqCst), 1);
    let tree = Tree::new(vec![]).unwrap();
    let mut objects = original.canonical_objects().to_vec();
    objects.push(CanonicalObject::Tree(tree.clone()));
    let collision = WriteBatch::new(
        original.transaction_id().clone(),
        objects,
        original.entity_records().to_vec(),
        original.admissions().to_vec(),
        original.mutations().to_vec(),
    )
    .unwrap();
    assert_eq!(
        repository.apply_batch(&collision).unwrap_err().code(),
        "transaction_replay_mismatch"
    );
    assert_eq!(checks.0.load(Ordering::SeqCst), 2);
    let store = open(&root).unwrap();
    assert_eq!(
        NativeCas::open(&store)
            .unwrap()
            .read(&StoreObjectId::Tree(tree.id().clone()))
            .unwrap(),
        CanonicalObject::Tree(tree.clone())
    );
    drop(store);
    assert!(
        repository.get_tree(tree.id()).unwrap().is_none(),
        "collision must not index its CAS orphan"
    );
    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    assert_eq!(
        connection
            .query_row::<u64, _, _>("SELECT count(*) FROM applied_transactions", [], |row| row
                .get(0))
            .unwrap(),
        1
    );
    connection
        .execute(
            "UPDATE applied_transactions SET receipt_json='{}' WHERE transaction_id=?1",
            [original.transaction_id().as_str()],
        )
        .unwrap();
    connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    drop(connection);
    assert_eq!(
        repository.apply_batch(&original).unwrap_err().code(),
        "corruption"
    );
    assert_eq!(checks.0.load(Ordering::SeqCst), 3);
    cleanup(&root);
}

#[test]
fn complete_initialize_is_atomic_readable_and_strictly_replayable() {
    let root = unique_root("apply");
    let batch = initialize(&root, "a");
    let mut repository = NativeScmRepository::new(&root);
    let first = repository.apply_batch(&batch).unwrap();
    assert!(matches!(first, ApplyBatchOutcome::Applied { .. }));
    let second = repository.apply_batch(&batch).unwrap();
    assert!(matches!(second, ApplyBatchOutcome::AlreadyApplied { .. }));

    let view = batch
        .canonical_objects()
        .iter()
        .find_map(|o| match o {
            CanonicalObject::RepositoryView(v) => Some(v),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        repository.get_repository_view(view.id()).unwrap(),
        Some(view.clone())
    );
    let current = repository
        .current_repository_view(view.space_id())
        .unwrap()
        .unwrap();
    assert_eq!(current.generation(), 1);
    assert_eq!(current.view_id(), Some(view.id()));

    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    let receipt: String = connection
        .query_row(
            "SELECT receipt_json FROM applied_transactions WHERE transaction_id=?1",
            [batch.transaction_id().as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        receipt,
        serde_jcs::to_string(&serde_json::from_str::<serde_json::Value>(&receipt).unwrap())
            .unwrap()
    );
    assert!(receipt.contains("layrs.apply-batch-receipt.v1"));
    assert!(receipt.contains("\"outcome\":\"applied\""));
    assert_eq!(
        connection
            .query_row::<String, _, _>("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap(),
        "ok"
    );
    let fk: i64 = connection
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(fk, 0);
    drop(connection);
    let report = fsck_existing(&root, FsckOptions::default()).unwrap();
    assert!(report.is_clean(), "{:#?}", report.findings);
    cleanup(&root);
}

#[test]
fn non_admitted_view_publication_rolls_back_metadata_but_may_leave_verified_cas_orphans() {
    let root = unique_root("rollback");
    let mut batch = initialize(&root, "b");
    batch = WriteBatch::new(
        batch.transaction_id().clone(),
        batch.canonical_objects().to_vec(),
        batch.entity_records().to_vec(),
        vec![],
        batch.mutations().to_vec(),
    )
    .unwrap();
    let mut repository = NativeScmRepository::new(&root);
    assert!(repository.apply_batch(&batch).is_err());
    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    for table in ["objects", "spaces", "operations", "applied_transactions"] {
        let count: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "unexpected metadata in {table}");
    }
    drop(connection);
    assert!(fs::read_dir(root.join("objects")).unwrap().next().is_some());
    cleanup(&root);
}

#[test]
fn faults_before_commit_rollback_and_fault_after_commit_recovers_by_replay() {
    let root = unique_root("faults");
    let batch = initialize(&root, "c");
    let mut repository = NativeScmRepository::with_fault_injector(
        &root,
        Arc::new(OneFault::new(
            RepositoryFaultPoint::AfterEntitiesAndProjections,
        )),
    );
    assert!(repository.apply_batch(&batch).is_err());
    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM applied_transactions", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
    drop(connection);

    let mut repository = NativeScmRepository::with_fault_injector(
        &root,
        Arc::new(OneFault::new(RepositoryFaultPoint::AfterCommit)),
    );
    assert!(repository.apply_batch(&batch).is_err());
    let space = batch
        .entity_records()
        .iter()
        .find_map(|e| {
            if let EntityRecord::Space(v) = e {
                Some(v)
            } else {
                None
            }
        })
        .unwrap();
    let operation = batch
        .entity_records()
        .iter()
        .find_map(|e| {
            if let EntityRecord::Operation(v) = e {
                Some(v)
            } else {
                None
            }
        })
        .unwrap();
    let durable = repository
        .current_repository_view(space.id())
        .unwrap()
        .unwrap();
    assert_eq!(durable.generation(), 1);
    assert_eq!(
        repository.operation_heads(space.id()).unwrap(),
        vec![operation.id().clone()]
    );
    assert_eq!(
        repository.get_operation(operation.id()).unwrap(),
        Some(operation.clone())
    );
    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    let receipts: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM applied_transactions WHERE transaction_id=?1",
            [batch.transaction_id().as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(receipts, 1);
    drop(connection);
    let mut retry = NativeScmRepository::new(&root);
    assert!(matches!(
        retry.apply_batch(&batch).unwrap(),
        ApplyBatchOutcome::AlreadyApplied { .. }
    ));
    assert_eq!(
        retry
            .current_repository_view(space.id())
            .unwrap()
            .unwrap()
            .generation(),
        1
    );
    assert_clean_db(&root);
    drop(retry);
    cleanup(&root);
}

#[test]
fn transaction_id_reuse_and_corrupt_receipts_are_rejected() {
    let root = unique_root("receipt");
    let batch = initialize(&root, "d");
    let mut repository = NativeScmRepository::new(&root);
    repository.apply_batch(&batch).unwrap();

    let other_space = Space::new(
        SpaceId::parse("space:other").unwrap(),
        "Other",
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let different = WriteBatch::new(
        batch.transaction_id().clone(),
        vec![],
        vec![EntityRecord::Space(other_space)],
        vec![],
        vec![],
    )
    .unwrap();
    assert_eq!(
        repository.apply_batch(&different).unwrap_err().code(),
        "transaction_replay_mismatch"
    );

    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    let original: String = connection
        .query_row(
            "SELECT receipt_json FROM applied_transactions WHERE transaction_id=?1",
            [batch.transaction_id().as_str()],
            |row| row.get(0),
        )
        .unwrap();
    connection.execute("UPDATE applied_transactions SET request_digest=?1,receipt_json='{}' WHERE transaction_id=?2",rusqlite::params![format!("blake3:{}","0".repeat(64)),batch.transaction_id().as_str()]).unwrap();
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    drop(connection);
    assert_eq!(
        repository.apply_batch(&batch).unwrap_err().code(),
        "transaction_replay_mismatch"
    );
    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    connection.execute("UPDATE applied_transactions SET request_digest=?1,receipt_json=?2 WHERE transaction_id=?3",rusqlite::params![batch.request_digest().to_string(),original,batch.transaction_id().as_str()]).unwrap();
    connection
        .execute(
            "UPDATE applied_transactions SET receipt_json=?1 WHERE transaction_id=?2",
            rusqlite::params![format!(" {original}"), batch.transaction_id().as_str()],
        )
        .unwrap();
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    drop(connection);
    assert_eq!(
        repository.apply_batch(&batch).unwrap_err().code(),
        "corruption"
    );
    for field in ["schemaVersion", "outcome"] {
        let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
        let mut wrong: serde_json::Value = serde_json::from_str(&original).unwrap();
        wrong[field] = serde_json::Value::String("wrong".into());
        let wrong = serde_jcs::to_string(&wrong).unwrap();
        connection
            .execute(
                "UPDATE applied_transactions SET receipt_json=?1 WHERE transaction_id=?2",
                rusqlite::params![wrong, batch.transaction_id().as_str()],
            )
            .unwrap();
        connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        drop(connection);
        assert_eq!(
            repository.apply_batch(&batch).unwrap_err().code(),
            "corruption",
            "{field}"
        );
    }
    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    let mut tampered: serde_json::Value = serde_json::from_str(&original).unwrap();
    tampered["requestDigest"] = serde_json::Value::String(format!("blake3:{}", "0".repeat(64)));
    let tampered = serde_jcs::to_string(&tampered).unwrap();
    connection
        .execute(
            "UPDATE applied_transactions SET receipt_json=?1 WHERE transaction_id=?2",
            rusqlite::params![tampered, batch.transaction_id().as_str()],
        )
        .unwrap();
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    drop(connection);
    assert_eq!(
        repository.apply_batch(&batch).unwrap_err().code(),
        "corruption"
    );
    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE applied_transactions SET receipt_json='{}' WHERE transaction_id=?1",
            [batch.transaction_id().as_str()],
        )
        .unwrap();
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    drop(connection);
    assert_eq!(
        repository.apply_batch(&batch).unwrap_err().code(),
        "corruption"
    );
    cleanup(&root);
}

#[test]
fn every_reachable_precommit_fault_boundary_leaves_no_logical_metadata() {
    let points = [
        RepositoryFaultPoint::AfterEachCas,
        RepositoryFaultPoint::AfterAllCas,
        RepositoryFaultPoint::AfterWriterOpen,
        RepositoryFaultPoint::AfterBeginImmediate,
        RepositoryFaultPoint::AfterReplayCheck,
        RepositoryFaultPoint::AfterValidation,
        RepositoryFaultPoint::AfterObjects,
        RepositoryFaultPoint::AfterEntitiesAndProjections,
        RepositoryFaultPoint::AfterAdmissions,
        RepositoryFaultPoint::AfterOperationHeadsCas,
        RepositoryFaultPoint::AfterRepositoryViewCas,
        RepositoryFaultPoint::BeforeReceipt,
        RepositoryFaultPoint::AfterReceiptBeforeCommit,
    ];
    for (index, point) in points.into_iter().enumerate() {
        let root = unique_root(&format!("matrix-{index}"));
        let batch = initialize(&root, &format!("matrix{index}"));
        let mut repository =
            NativeScmRepository::with_fault_injector(&root, Arc::new(OneFault::new(point)));
        assert!(repository.apply_batch(&batch).is_err(), "{point:?}");
        let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
        for table in [
            "objects",
            "spaces",
            "operations",
            "current_repository_views",
            "operation_heads",
            "gc_roots",
            "applied_transactions",
        ] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{point:?} leaked {table}");
        }
        drop(connection);
        cleanup(&root);
    }
}

#[test]
fn corrupt_preexisting_cas_target_aborts_before_any_metadata() {
    let root = unique_root("cas-failure");
    let batch = initialize(&root, "casfailure");
    let object_id = batch.canonical_objects()[0].object_id();
    let store = open(&root).unwrap();
    let cas = NativeCas::open(&store).unwrap();
    let path = cas.object_path(&object_id).unwrap();
    drop(cas);
    drop(store);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"deliberately-invalid").unwrap();
    let mut repository = NativeScmRepository::new(&root);
    assert!(repository.apply_batch(&batch).is_err());
    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM objects", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    drop(connection);
    drop(repository);
    cleanup(&root);
}
