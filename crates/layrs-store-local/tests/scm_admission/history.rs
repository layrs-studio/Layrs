use super::*;

#[test]
fn passive_operation_imports_are_separate_from_exact_durable_frontier_publication() {
    let root = unique_root("operation-frontier");
    let fixture = fixture();
    seed(&root, &fixture);
    let result_view_id = fixture.operation.resulting_view_id().clone();
    let timestamp = Timestamp::parse("2026-07-19T12:02:00Z").unwrap();
    let durable_heads: Vec<_> = ["a", "b", "c"]
        .into_iter()
        .map(|suffix| {
            Operation::new(
                OperationId::parse(format!("operation:frontier-{suffix}")).unwrap(),
                fixture.space.id().clone(),
                vec![fixture.operation.id().clone()],
                result_view_id.clone(),
                OperationKind::CaptureWorktree,
                format!("frontier {suffix}"),
                vec![],
                None,
                None,
                timestamp.clone(),
                Metadata::empty(),
            )
            .unwrap()
        })
        .collect();
    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    connection
        .execute("DELETE FROM operation_heads", [])
        .unwrap();
    let durable_outside_frontier = Operation::new(
        OperationId::parse("operation:durable-outside-frontier").unwrap(),
        fixture.space.id().clone(),
        vec![fixture.operation.id().clone()],
        result_view_id.clone(),
        OperationKind::CaptureWorktree,
        "durable historical branch outside current heads",
        vec![],
        None,
        None,
        timestamp.clone(),
        Metadata::empty(),
    )
    .unwrap();
    connection
        .execute(
            "INSERT INTO operations VALUES (?1, ?2, ?3, 'capture_worktree', NULL, NULL, ?4, ?5)",
            params![
                durable_outside_frontier.id().as_str(),
                fixture.space.id().as_str(),
                result_view_id.as_str(),
                durable_outside_frontier.created_at().as_str(),
                json(&durable_outside_frontier)
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO operation_parents VALUES (?1, ?2, 0)",
            params![
                durable_outside_frontier.id().as_str(),
                fixture.operation.id().as_str()
            ],
        )
        .unwrap();
    for (ordinal, operation) in durable_heads.iter().enumerate() {
        connection
            .execute(
                "INSERT INTO operations VALUES (?1, ?2, ?3, 'capture_worktree', NULL, NULL, ?4, ?5)",
                params![
                    operation.id().as_str(),
                    fixture.space.id().as_str(),
                    result_view_id.as_str(),
                    operation.created_at().as_str(),
                    json(operation)
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO operation_parents VALUES (?1, ?2, 0)",
                params![operation.id().as_str(), fixture.operation.id().as_str()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO operation_heads VALUES (?1, ?2, ?3, 'frontier-seed')",
                params![
                    fixture.space.id().as_str(),
                    operation.id().as_str(),
                    i64::try_from(ordinal).unwrap()
                ],
            )
            .unwrap();
    }
    checkpoint(&connection);
    drop(connection);

    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let passive = Operation::new(
        OperationId::parse("operation:passive-import").unwrap(),
        fixture.space.id().clone(),
        vec![durable_outside_frontier.id().clone()],
        result_view_id.clone(),
        OperationKind::CaptureWorktree,
        "passive historical import",
        vec![],
        None,
        None,
        timestamp.clone(),
        Metadata::empty(),
    )
    .unwrap();
    let passive_only = WriteBatch::new(
        transaction("passive-import-only"),
        vec![],
        vec![EntityRecord::Operation(passive.clone())],
        vec![],
        vec![],
    )
    .unwrap();
    let passive_resolver = CompositeAdmissionResolver::new(&reader, &passive_only).unwrap();
    passive_resolver.validate_batch().unwrap();
    assert!(
        !passive_resolver
            .operation_dag()
            .contains(durable_outside_frontier.id())
    );
    assert!(!passive_resolver.operation_dag().contains(passive.id()));
    for durable_head in &durable_heads {
        assert!(passive_resolver.operation_dag().contains(durable_head.id()));
    }
    assert_eq!(
        reader.operation_heads(fixture.space.id()).unwrap(),
        durable_heads
            .iter()
            .map(|operation| operation.id().clone())
            .collect::<Vec<_>>()
    );

    let published_normal = Operation::new(
        OperationId::parse("operation:published-normal").unwrap(),
        fixture.space.id().clone(),
        vec![durable_heads[0].id().clone()],
        result_view_id.clone(),
        OperationKind::CaptureWorktree,
        "published normal",
        vec![],
        None,
        None,
        timestamp.clone(),
        Metadata::empty(),
    )
    .unwrap();
    let passive_child_of_publication = Operation::new(
        OperationId::parse("operation:passive-child-of-publication").unwrap(),
        fixture.space.id().clone(),
        vec![published_normal.id().clone()],
        result_view_id.clone(),
        OperationKind::CaptureWorktree,
        "passive child of same-batch publication",
        vec![],
        None,
        None,
        timestamp.clone(),
        Metadata::empty(),
    )
    .unwrap();
    let expected_heads: Vec<_> = durable_heads
        .iter()
        .map(|operation| operation.id().clone())
        .collect();
    let mut published_heads = vec![
        durable_heads[1].id().clone(),
        durable_heads[2].id().clone(),
        published_normal.id().clone(),
    ];
    published_heads.sort();
    let import_and_publication = WriteBatch::new(
        transaction("passive-import-and-publication"),
        vec![],
        vec![
            EntityRecord::Operation(passive),
            EntityRecord::Operation(published_normal.clone()),
            EntityRecord::Operation(passive_child_of_publication),
        ],
        vec![],
        vec![
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(
                    fixture.space.id().clone(),
                    expected_heads.clone(),
                    published_normal.id().clone(),
                    published_heads,
                )
                .unwrap(),
            ),
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(
                    fixture.space.id().clone(),
                    1,
                    Some(result_view_id.clone()),
                    published_normal.resulting_view_id().clone(),
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    CompositeAdmissionResolver::new(&reader, &import_and_publication)
        .unwrap()
        .validate_batch()
        .unwrap();

    let integrate_ab = Operation::new(
        OperationId::parse("operation:integrate-ab").unwrap(),
        fixture.space.id().clone(),
        durable_heads[..2]
            .iter()
            .map(|operation| operation.id().clone())
            .collect(),
        result_view_id.clone(),
        OperationKind::IntegrateOperations,
        "incomplete integration",
        vec![],
        None,
        None,
        timestamp.clone(),
        Metadata::empty(),
    )
    .unwrap();
    let no_cas = WriteBatch::new(
        transaction("integrate-ab-no-cas"),
        vec![],
        vec![EntityRecord::Operation(integrate_ab.clone())],
        vec![],
        vec![],
    )
    .unwrap();
    CompositeAdmissionResolver::new(&reader, &no_cas)
        .unwrap()
        .validate_batch()
        .unwrap();

    let incomplete_cas = StoreMutation::OperationHeads(
        OperationHeadsCas::new(
            fixture.space.id().clone(),
            integrate_ab.parent_operation_ids().to_vec(),
            integrate_ab.id().clone(),
            vec![integrate_ab.id().clone()],
        )
        .unwrap(),
    );
    let incomplete = WriteBatch::new(
        transaction("integrate-ab-incomplete-cas"),
        vec![],
        vec![EntityRecord::Operation(integrate_ab.clone())],
        vec![],
        vec![
            incomplete_cas,
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(
                    fixture.space.id().clone(),
                    1,
                    Some(result_view_id.clone()),
                    integrate_ab.resulting_view_id().clone(),
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    let incomplete_error = match CompositeAdmissionResolver::new(&reader, &incomplete) {
        Ok(resolver) => resolver.validate_batch().unwrap_err(),
        Err(error) => error,
    };
    assert_eq!(
        incomplete_error.code(),
        "invalid_operation_heads_transition"
    );

    let complete_parent_ids = expected_heads;
    let integrate_all = Operation::new(
        OperationId::parse("operation:integrate-all").unwrap(),
        fixture.space.id().clone(),
        complete_parent_ids.clone(),
        result_view_id.clone(),
        OperationKind::IntegrateOperations,
        "complete integration",
        vec![],
        None,
        None,
        timestamp,
        Metadata::empty(),
    )
    .unwrap();
    let exact_cas = StoreMutation::OperationHeads(
        OperationHeadsCas::new(
            fixture.space.id().clone(),
            complete_parent_ids,
            integrate_all.id().clone(),
            vec![integrate_all.id().clone()],
        )
        .unwrap(),
    );
    let exact = WriteBatch::new(
        transaction("integrate-all-exact"),
        vec![],
        vec![EntityRecord::Operation(integrate_all.clone())],
        vec![],
        vec![
            exact_cas.clone(),
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(
                    fixture.space.id().clone(),
                    1,
                    Some(result_view_id.clone()),
                    integrate_all.resulting_view_id().clone(),
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    CompositeAdmissionResolver::new(&reader, &exact)
        .unwrap()
        .validate_batch()
        .unwrap();

    assert_eq!(
        WriteBatch::new(
            transaction("duplicate-heads-cas"),
            vec![],
            vec![EntityRecord::Operation(integrate_all)],
            vec![],
            vec![exact_cas.clone(), exact_cas],
        )
        .unwrap_err()
        .code(),
        "duplicate_mutable_target"
    );
    let mismatched_operation_id = OperationId::parse("operation:not-in-batch").unwrap();
    let mismatched_cas = OperationHeadsCas::new(
        fixture.space.id().clone(),
        durable_heads
            .iter()
            .map(|operation| operation.id().clone())
            .collect(),
        mismatched_operation_id.clone(),
        vec![mismatched_operation_id],
    )
    .unwrap();
    let mismatch_error = WriteBatch::new(
        transaction("operation-heads-mismatch"),
        vec![],
        vec![EntityRecord::Operation(durable_heads[0].clone())],
        vec![],
        vec![StoreMutation::OperationHeads(mismatched_cas)],
    )
    .unwrap_err();
    assert_eq!(
        mismatch_error.code(),
        "operation_heads_cas_missing_operation"
    );
    drop(reader);
    drop(store);
    cleanup(&root);
}

#[test]
fn combined_history_rejects_a_second_initialize_operation() {
    let root = unique_root("combined-history-double-init");
    let fixture = fixture();
    seed(&root, &fixture);
    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let duplicate_initialize = Operation::new(
        OperationId::parse("operation:duplicate-combined-init").unwrap(),
        fixture.space.id().clone(),
        vec![],
        fixture.operation.resulting_view_id().clone(),
        OperationKind::Initialize,
        "duplicate initialize",
        vec![],
        None,
        None,
        Timestamp::parse("2026-07-19T12:03:00Z").unwrap(),
        Metadata::empty(),
    )
    .unwrap();
    let batch = WriteBatch::new(
        transaction("combined-history-double-init"),
        vec![],
        vec![EntityRecord::Operation(duplicate_initialize)],
        vec![],
        vec![],
    )
    .unwrap();
    let error = match CompositeAdmissionResolver::new(&reader, &batch) {
        Ok(resolver) => resolver.validate_batch().unwrap_err(),
        Err(error) => error,
    };
    assert_eq!(error.code(), "admission_rejected");
    assert!(error.to_string().contains("durable Initialize Operation"));
    drop(reader);
    drop(store);
    cleanup(&root);
}

#[test]
fn passive_initialize_is_unique_across_sequential_batches_even_outside_heads() {
    let root = unique_root("sequential-passive-initialize");
    let fixture = fixture();
    seed(&root, &fixture);
    let timestamp = Timestamp::parse("2026-07-19T12:03:30Z").unwrap();
    let target_space = Space::new(
        SpaceId::parse("space:sequential-passive-init").unwrap(),
        "Sequential passive init",
        timestamp.clone(),
        Metadata::empty(),
    )
    .unwrap();
    let target_view =
        RepositoryView::new(target_space.id().clone(), vec![], vec![], vec![], vec![]).unwrap();
    let first = Operation::new(
        OperationId::parse("operation:sequential-passive-init-1").unwrap(),
        target_space.id().clone(),
        vec![],
        target_view.id().clone(),
        OperationKind::Initialize,
        "first passive initializer",
        vec![],
        None,
        None,
        timestamp.clone(),
        Metadata::empty(),
    )
    .unwrap();

    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    connection
        .execute(
            "INSERT INTO spaces VALUES (?1, ?2, CAST(unixepoch(?3, 'subsec') * 1000 AS INTEGER))",
            params![
                target_space.id().as_str(),
                json(&target_space),
                target_space.created_at().as_str()
            ],
        )
        .unwrap();
    checkpoint(&connection);
    drop(connection);

    let first_batch = WriteBatch::new(
        transaction("sequential-passive-init-first"),
        vec![CanonicalObject::RepositoryView(target_view.clone())],
        vec![EntityRecord::Operation(first.clone())],
        vec![SpaceObjectAdmission::new(
            target_space.id().clone(),
            StoreObjectId::RepositoryView(target_view.id().clone()),
        )],
        vec![],
    )
    .unwrap();
    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert!(
        reader
            .initializer_in_space(target_space.id())
            .unwrap()
            .is_none()
    );
    CompositeAdmissionResolver::new(&reader, &first_batch)
        .unwrap()
        .validate_batch()
        .unwrap();
    drop(reader);
    drop(store);

    let exclusive = open_exclusive(&root).unwrap();
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let outcome = cas
        .put(&CanonicalObject::RepositoryView(target_view.clone()))
        .unwrap();
    drop(cas);
    drop(exclusive);
    let relative = outcome
        .path
        .strip_prefix(&root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    connection
        .execute(
            "INSERT INTO objects VALUES (?1, 'repository_view', ?2, ?3, 1, 0)",
            params![
                target_view.id().as_str(),
                relative,
                i64::try_from(outcome.bytes_len).unwrap()
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO repository_views VALUES (?1, ?2)",
            params![target_view.id().as_str(), target_space.id().as_str()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO space_object_admissions VALUES (?1, ?2, NULL, 0)",
            params![target_space.id().as_str(), target_view.id().as_str()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO operations VALUES (?1, ?2, ?3, 'initialize', NULL, NULL, ?4, ?5)",
            params![
                first.id().as_str(),
                target_space.id().as_str(),
                target_view.id().as_str(),
                first.created_at().as_str(),
                json(&first)
            ],
        )
        .unwrap();
    checkpoint(&connection);
    drop(connection);

    let second = Operation::new(
        OperationId::parse("operation:sequential-passive-init-2").unwrap(),
        target_space.id().clone(),
        vec![],
        target_view.id().clone(),
        OperationKind::Initialize,
        "second passive initializer",
        vec![],
        None,
        None,
        timestamp,
        Metadata::empty(),
    )
    .unwrap();
    let second_batch = WriteBatch::new(
        transaction("sequential-passive-init-second"),
        vec![],
        vec![EntityRecord::Operation(second)],
        vec![],
        vec![],
    )
    .unwrap();
    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert_eq!(
        reader.initializer_in_space(target_space.id()).unwrap(),
        Some(first)
    );
    let error = CompositeAdmissionResolver::new(&reader, &second_batch)
        .err()
        .unwrap();
    assert_eq!(error.code(), "admission_rejected");
    assert!(error.to_string().contains("already has durable Initialize"));
    drop(reader);
    drop(store);
    cleanup(&root);
}

#[test]
fn initializer_lookup_reports_multiple_rows_as_corruption_before_selecting_one() {
    let root = unique_root("initializer-corruption");
    let fixture = fixture();
    seed(&root, &fixture);
    let second = Operation::new(
        OperationId::parse("operation:corrupt-second-initializer").unwrap(),
        fixture.space.id().clone(),
        vec![],
        fixture.operation.resulting_view_id().clone(),
        OperationKind::Initialize,
        "corrupt second initializer",
        vec![],
        None,
        None,
        Timestamp::parse("2026-07-19T12:03:45Z").unwrap(),
        Metadata::empty(),
    )
    .unwrap();
    let exclusive = open_exclusive(&root).unwrap();
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let connection = Mutex::new(Connection::open(exclusive.layout().sqlite_path()).unwrap());
    connection
        .lock()
        .unwrap()
        .execute_batch(
            "PRAGMA foreign_keys=ON; BEGIN IMMEDIATE; \
             DROP INDEX operations_single_initialize_per_space_idx; \
             DROP INDEX operations_single_initialize_payload_space_idx;",
        )
        .unwrap();
    connection
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO operations VALUES (?1, ?2, ?3, 'initialize', NULL, NULL, ?4, ?5)",
            params![
                second.id().as_str(),
                fixture.space.id().as_str(),
                second.resulting_view_id().as_str(),
                second.created_at().as_str(),
                json(&second)
            ],
        )
        .unwrap();
    let reader = NativeTransactionalScmReader::new(&connection, &cas).unwrap();
    let error = reader.initializer_in_space(fixture.space.id()).unwrap_err();
    assert_eq!(error.code(), "corruption");
    assert!(error.to_string().contains("multiple durable Initialize"));
    drop(reader);
    connection
        .lock()
        .unwrap()
        .execute_batch("ROLLBACK;")
        .unwrap();
    drop(connection);
    drop(cas);
    drop(exclusive);
    cleanup(&root);
}

#[test]
fn initializer_lookup_never_misses_a_kind_or_space_projection_divergence() {
    for sabotage in ["kind", "space"] {
        let root = unique_root(&format!("initializer-{sabotage}-projection"));
        let fixture = fixture();
        seed(&root, &fixture);
        let exclusive = open_exclusive(&root).unwrap();
        let cas = NativeCas::open_exclusive(&exclusive).unwrap();
        let connection = Mutex::new(Connection::open(exclusive.layout().sqlite_path()).unwrap());
        connection
            .lock()
            .unwrap()
            .execute_batch(
                "PRAGMA ignore_check_constraints=ON; \
                 PRAGMA foreign_keys=ON; BEGIN IMMEDIATE;",
            )
            .unwrap();
        match sabotage {
            "kind" => {
                connection
                    .lock()
                    .unwrap()
                    .execute(
                        "UPDATE operations SET operation_kind='capture_worktree' \
                         WHERE operation_id=?1",
                        [fixture.operation.id().as_str()],
                    )
                    .unwrap();
            }
            "space" => {
                let projected_space = Space::new(
                    SpaceId::parse("space:initializer-projection-sabotage").unwrap(),
                    "Projection sabotage",
                    Timestamp::parse("2026-07-19T12:03:50Z").unwrap(),
                    Metadata::empty(),
                )
                .unwrap();
                connection
                    .lock()
                    .unwrap()
                    .execute(
                        "INSERT INTO spaces VALUES (?1, ?2, CAST(unixepoch(?3, 'subsec') * 1000 AS INTEGER))",
                        params![
                            projected_space.id().as_str(),
                            json(&projected_space),
                            projected_space.created_at().as_str()
                        ],
                    )
                    .unwrap();
                connection
                    .lock()
                    .unwrap()
                    .execute(
                        "UPDATE operations SET space_id=?2 WHERE operation_id=?1",
                        params![
                            fixture.operation.id().as_str(),
                            projected_space.id().as_str()
                        ],
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }

        let reader = NativeTransactionalScmReader::new(&connection, &cas).unwrap();
        let lookup_error = reader.initializer_in_space(fixture.space.id()).unwrap_err();
        assert_eq!(lookup_error.code(), "corruption", "sabotage={sabotage}");

        let second = Operation::new(
            OperationId::parse(format!("operation:initializer-after-{sabotage}-sabotage")).unwrap(),
            fixture.space.id().clone(),
            vec![],
            fixture.operation.resulting_view_id().clone(),
            OperationKind::Initialize,
            "initializer after projection sabotage",
            vec![],
            None,
            None,
            Timestamp::parse("2026-07-19T12:03:55Z").unwrap(),
            Metadata::empty(),
        )
        .unwrap();
        let batch = WriteBatch::new(
            transaction(&format!("initializer-after-{sabotage}-sabotage")),
            vec![],
            vec![EntityRecord::Operation(second)],
            vec![],
            vec![],
        )
        .unwrap();
        let admission_error = CompositeAdmissionResolver::new(&reader, &batch)
            .err()
            .unwrap();
        assert_eq!(admission_error.code(), "corruption", "sabotage={sabotage}");
        drop(reader);
        connection
            .lock()
            .unwrap()
            .execute_batch("ROLLBACK;")
            .unwrap();
        drop(connection);
        drop(cas);
        drop(exclusive);
        cleanup(&root);
    }
}

#[test]
fn worktree_cas_can_advance_only_to_the_operation_published_in_its_space() {
    let root = unique_root("worktree-publication-coupling");
    let fixture = fixture();
    seed(&root, &fixture);
    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let current = fixture.operation.id().clone();
    let current_view = fixture.operation.resulting_view_id().clone();
    let timestamp = Timestamp::parse("2026-07-19T12:04:00Z").unwrap();
    let published = Operation::new(
        OperationId::parse("operation:worktree-published").unwrap(),
        fixture.space.id().clone(),
        vec![current.clone()],
        current_view.clone(),
        OperationKind::CaptureWorktree,
        "published for worktree",
        vec![],
        None,
        None,
        timestamp.clone(),
        Metadata::empty(),
    )
    .unwrap();
    let passive = Operation::new(
        OperationId::parse("operation:worktree-passive").unwrap(),
        fixture.space.id().clone(),
        vec![current.clone()],
        current_view.clone(),
        OperationKind::CaptureWorktree,
        "passive worktree target",
        vec![],
        None,
        None,
        timestamp,
        Metadata::empty(),
    )
    .unwrap();
    let heads = StoreMutation::OperationHeads(
        OperationHeadsCas::new(
            fixture.space.id().clone(),
            vec![current.clone()],
            published.id().clone(),
            vec![published.id().clone()],
        )
        .unwrap(),
    );
    let view = StoreMutation::RepositoryView(
        RepositoryViewCas::new(
            fixture.space.id().clone(),
            1,
            Some(current_view.clone()),
            published.resulting_view_id().clone(),
        )
        .unwrap(),
    );
    let to_passive = StoreMutation::WorktreeActiveOperation(
        WorktreeActiveOperationCas::new(
            fixture.worktree.id().clone(),
            current.clone(),
            passive.id().clone(),
        )
        .unwrap(),
    );

    let passive_without_publication = WriteBatch::new(
        transaction("worktree-passive-without-publication"),
        vec![],
        vec![EntityRecord::Operation(passive.clone())],
        vec![],
        vec![to_passive.clone()],
    )
    .unwrap();
    assert_eq!(
        CompositeAdmissionResolver::new(&reader, &passive_without_publication)
            .unwrap()
            .validate_batch()
            .unwrap_err()
            .code(),
        "admission_rejected"
    );

    let passive_despite_other_publication = WriteBatch::new(
        transaction("worktree-passive-despite-publication"),
        vec![],
        vec![
            EntityRecord::Operation(passive.clone()),
            EntityRecord::Operation(published.clone()),
        ],
        vec![],
        vec![heads.clone(), view.clone(), to_passive],
    )
    .unwrap();
    assert_eq!(
        CompositeAdmissionResolver::new(&reader, &passive_despite_other_publication)
            .unwrap()
            .validate_batch()
            .unwrap_err()
            .code(),
        "admission_rejected"
    );

    let to_published = StoreMutation::WorktreeActiveOperation(
        WorktreeActiveOperationCas::new(
            fixture.worktree.id().clone(),
            current,
            published.id().clone(),
        )
        .unwrap(),
    );
    let accepted = WriteBatch::new(
        transaction("worktree-to-published"),
        vec![],
        vec![
            EntityRecord::Operation(passive),
            EntityRecord::Operation(published),
        ],
        vec![],
        vec![heads, view, to_published],
    )
    .unwrap();
    CompositeAdmissionResolver::new(&reader, &accepted)
        .unwrap()
        .validate_batch()
        .unwrap();
    drop(reader);
    drop(store);
    cleanup(&root);
}
