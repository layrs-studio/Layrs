use super::*;

#[test]
fn observation_is_an_explicit_gc_root_for_its_observed_step() {
    let root = TestRoot::new("gc-observation");
    let handle = init(root.path(), StoreId::parse("store:fsck").unwrap()).unwrap();
    drop(handle);
    let tree = Tree::new(Vec::new()).unwrap();
    let step = Step::new(Vec::new(), tree.id().clone()).unwrap();
    let observation = StepObservation::new(
        ObservationId::parse("observation:gc-only").unwrap(),
        step.id().clone(),
        None,
        None,
        None,
        None,
        None,
        Timestamp::parse("2026-07-20T00:00:00Z").unwrap(),
        CaptureReason::ManualSnapshot,
        Metadata::empty(),
    );
    let exclusive = open_exclusive(root.path()).unwrap();
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let outcomes = [
        cas.put(&CanonicalObject::Tree(tree.clone())).unwrap(),
        cas.put(&CanonicalObject::Step(step.clone())).unwrap(),
    ];
    drop(cas);
    drop(exclusive);
    mutate(&root, |connection| {
        for outcome in &outcomes {
            let relative = outcome
                .path
                .strip_prefix(root.path())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            connection
                .execute(
                    "INSERT INTO objects(object_id,object_kind,storage_path,byte_length,verified,created_at_ms) VALUES(?1,?2,?3,?4,1,0)",
                    params![
                        outcome.object_id.as_str(),
                        kind_name(&outcome.object_id),
                        relative,
                        outcome.bytes_len
                    ],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO steps(step_id,root_tree_id,parent_count) VALUES(?1,?2,0)",
                params![step.id().as_str(), tree.id().as_str()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO object_edges(source_object_id,target_object_id,edge_kind,ordinal) VALUES(?1,?2,'step_tree',0)",
                params![step.id().as_str(), tree.id().as_str()],
            )
            .unwrap();
        let payload = String::from_utf8(canonical_json(&observation).unwrap()).unwrap();
        connection
            .execute(
                "INSERT INTO step_observations(observation_id,step_id,space_id,operation_id,worktree_id,actor_id,device_id,observed_at,capture_reason,payload_json) VALUES(?1,?2,NULL,NULL,NULL,NULL,NULL,?3,'manual_snapshot',?4)",
                params![
                    observation.id().as_str(),
                    step.id().as_str(),
                    observation.observed_at().as_str(),
                    payload
                ],
            )
            .unwrap();
    });

    let missing = fsck_existing(root.path(), FsckOptions::default()).unwrap();
    assert!(missing.findings.iter().any(|finding| {
        finding.code == FsckFindingCode::GcRootInvalid
            && finding.object_id.as_deref() == Some(step.id().as_str())
    }));
    assert!(
        missing
            .gc_reachability
            .unreachable_objects
            .contains(&step.id().to_string())
    );

    mutate(&root, |connection| {
        connection
            .execute(
                "INSERT INTO gc_roots(root_kind,root_id,object_id,created_at_ms) VALUES('observation',?1,?2,0)",
                params![observation.id().as_str(), step.id().as_str()],
            )
            .unwrap();
    });
    let retained = fsck_existing(root.path(), FsckOptions::default()).unwrap();
    assert!(!retained.findings.iter().any(|finding| {
        finding.code == FsckFindingCode::GcRootInvalid
            && finding.object_id.as_deref() == Some(step.id().as_str())
    }));
    assert!(
        !retained
            .gc_reachability
            .unreachable_objects
            .contains(&step.id().to_string())
    );
    assert!(
        !retained
            .gc_reachability
            .unreachable_objects
            .contains(&tree.id().to_string())
    );
}

#[test]
fn gc_tree_file_closure_retains_every_manifest_and_all_chunks() {
    let root = TestRoot::new("gc-file-closure");
    let handle = init(root.path(), StoreId::parse("store:fsck").unwrap()).unwrap();
    drop(handle);
    let exclusive = open_exclusive(root.path()).unwrap();
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let whole = Chunk::new(b"abcdef".to_vec()).unwrap();
    let left = Chunk::new(b"abc".to_vec()).unwrap();
    let right = Chunk::new(b"def".to_vec()).unwrap();
    let one = FileManifest::from_chunks(ChunkingDescriptor::single(), std::slice::from_ref(&whole))
        .unwrap();
    let two = FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(3).unwrap(),
        &[left.clone(), right.clone()],
    )
    .unwrap();
    assert_eq!(one.file_id(), two.file_id());
    let tree = Tree::new(vec![TreeEntry::new(
        RepoPathComponent::parse("file.txt").unwrap(),
        TreeEntryTarget::file(one.file_id().clone()),
        false,
    )])
    .unwrap();
    let objects = vec![
        CanonicalObject::ChunkRaw(whole),
        CanonicalObject::ChunkRaw(left),
        CanonicalObject::ChunkRaw(right),
        CanonicalObject::FileManifest(one.clone()),
        CanonicalObject::FileManifest(two.clone()),
        CanonicalObject::Tree(tree.clone()),
    ];
    let outcomes: Vec<_> = objects
        .iter()
        .map(|object| cas.put(object).unwrap())
        .collect();
    drop(cas);
    drop(exclusive);
    mutate(&root, |connection| {
        for outcome in &outcomes {
            let relative = outcome
                .path
                .strip_prefix(root.path())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            connection.execute(
                "INSERT INTO objects(object_id,object_kind,storage_path,byte_length,verified,created_at_ms) VALUES(?1,?2,?3,?4,1,0)",
                params![outcome.object_id.as_str(), kind_name(&outcome.object_id), relative, outcome.bytes_len],
            ).unwrap();
        }
        connection
            .execute(
                "INSERT INTO file_availability VALUES(?1,'present',0)",
                [one.file_id().as_str()],
            )
            .unwrap();
        for manifest in [&one, &two] {
            connection
                .execute(
                    "INSERT INTO file_manifests VALUES(?1,?2,?3,'{}')",
                    params![
                        manifest.id().as_str(),
                        manifest.file_id().as_str(),
                        manifest.byte_length()
                    ],
                )
                .unwrap();
            for (ordinal, entry) in manifest.chunks().iter().enumerate() {
                connection
                    .execute(
                        "INSERT INTO file_manifest_chunks VALUES(?1,?2,?3,?4,?5)",
                        params![
                            manifest.id().as_str(),
                            ordinal as u64,
                            entry.chunk_id().as_str(),
                            entry.offset(),
                            entry.byte_length()
                        ],
                    )
                    .unwrap();
                connection
                    .execute(
                        "INSERT INTO object_edges VALUES(?1,?2,'manifest_chunk',?3)",
                        params![
                            manifest.id().as_str(),
                            entry.chunk_id().as_str(),
                            ordinal as u64
                        ],
                    )
                    .unwrap();
            }
        }
        connection
            .execute(
                "INSERT INTO object_pins VALUES('pin:tree',?1,'test',NULL,0)",
                [tree.id().as_str()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO gc_roots VALUES('pin','pin:tree',?1,0)",
                [tree.id().as_str()],
            )
            .unwrap();
    });
    let report = fsck_existing(root.path(), FsckOptions::default()).unwrap();
    assert!(report.gc_reachability.unreachable_objects.is_empty());
    assert_eq!(
        report.gc_reachability.reachable_objects,
        objects.len() as u64
    );
}

#[test]
fn retained_operation_inventory_rejects_missing_and_substitute_roots() {
    let root = TestRoot::new("gc-operation-history");
    let handle = init(root.path(), StoreId::parse("store:fsck").unwrap()).unwrap();
    drop(handle);
    let view = RepositoryView::new(
        SpaceId::parse("space:history").unwrap(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let exclusive = open_exclusive(root.path()).unwrap();
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let view_outcome = cas
        .put(&CanonicalObject::RepositoryView(view.clone()))
        .unwrap();
    let substitute = Chunk::new(b"not the retained View".to_vec()).unwrap();
    let substitute_outcome = cas
        .put(&CanonicalObject::ChunkRaw(substitute.clone()))
        .unwrap();
    drop(cas);
    drop(exclusive);
    mutate(&root, |connection| {
        for (outcome, kind) in [
            (&view_outcome, "repository_view"),
            (&substitute_outcome, "chunk_raw"),
        ] {
            let relative = outcome
                .path
                .strip_prefix(root.path())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            connection.execute(
                "INSERT INTO objects(object_id,object_kind,storage_path,byte_length,verified,created_at_ms) VALUES(?1,?2,?3,?4,1,0)",
                params![outcome.object_id.as_str(), kind, relative, outcome.bytes_len],
            ).unwrap();
        }
        connection.execute(
            r#"INSERT INTO spaces VALUES(
                'space:history',
                '{"schemaVersion":"layrs.space.v1","spaceId":"space:history","name":"History","createdAt":"1970-01-01T00:00:00Z","metadata":{}}',
                0
            )"#,
            [],
        ).unwrap();
        connection
            .execute(
                "INSERT INTO repository_views VALUES(?1,'space:history')",
                [view.id().as_str()],
            )
            .unwrap();
        connection
            .pragma_update(None, "ignore_check_constraints", "ON")
            .unwrap();
        connection.execute(
            "INSERT INTO operations(operation_id,space_id,resulting_view_id,operation_kind,created_at,payload_json) VALUES('operation:historical','space:history',?1,'initialize','1970-01-01T00:00:00Z','{}')",
            [view.id().as_str()],
        ).unwrap();
        connection
            .pragma_update(None, "ignore_check_constraints", "OFF")
            .unwrap();
    });

    // Retention is never inferred from `operations`: the missing inventory
    // row is corruption and retains neither the View nor an arbitrary object.
    let report = fsck_existing(root.path(), FsckOptions::default()).unwrap();
    assert!(report.findings.iter().any(|finding| {
        finding.code == FsckFindingCode::GcRootInvalid
            && finding.object_id.as_deref() == Some(view.id().as_str())
    }));
    assert_eq!(report.gc_reachability.roots, 0);
    assert_eq!(report.gc_reachability.reachable_objects, 0);

    mutate(&root, |connection| {
        connection
            .execute(
                "INSERT INTO gc_roots VALUES('retained_operation','operation:historical',?1,0)",
                [substitute.id().as_str()],
            )
            .unwrap();
    });
    let report = fsck_existing(root.path(), FsckOptions::default()).unwrap();
    assert!(report.findings.iter().any(|finding| {
        finding.code == FsckFindingCode::GcRootInvalid
            && finding.object_id.as_deref() == Some(substitute.id().as_str())
    }));
    assert_eq!(report.gc_reachability.roots, 0);
    assert_eq!(report.gc_reachability.reachable_objects, 0);

    mutate(&root, |connection| {
        connection.execute("DELETE FROM gc_roots", []).unwrap();
        connection
            .execute(
                "INSERT INTO gc_roots VALUES('retained_operation','operation:historical',?1,0)",
                [view.id().as_str()],
            )
            .unwrap();
    });
    let report = fsck_existing(root.path(), FsckOptions::default()).unwrap();
    assert_eq!(report.gc_reachability.roots, 1);
    assert_eq!(report.gc_reachability.reachable_objects, 1);
    assert!(
        report
            .gc_reachability
            .unreachable_objects
            .contains(&substitute.id().to_string())
    );
}

#[test]
fn retained_operation_roots_resulting_view_and_every_rewrite_step() {
    let root = TestRoot::new("gc-operation-rewrite");
    let handle = init(root.path(), StoreId::parse("store:fsck").unwrap()).unwrap();
    drop(handle);
    let tree = Tree::new(Vec::new()).unwrap();
    let old_step = Step::new(Vec::new(), tree.id().clone()).unwrap();
    let new_step = Step::new(vec![old_step.id().clone()], tree.id().clone()).unwrap();
    let view = RepositoryView::new(
        SpaceId::parse("space:rewrite").unwrap(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let objects = vec![
        CanonicalObject::Tree(tree.clone()),
        CanonicalObject::Step(old_step.clone()),
        CanonicalObject::Step(new_step.clone()),
        CanonicalObject::RepositoryView(view.clone()),
    ];
    let exclusive = open_exclusive(root.path()).unwrap();
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let outcomes: Vec<_> = objects
        .iter()
        .map(|object| cas.put(object).unwrap())
        .collect();
    drop(cas);
    drop(exclusive);
    mutate(&root, |connection| {
        for outcome in &outcomes {
            let relative = outcome
                .path
                .strip_prefix(root.path())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            connection
                .execute(
                    "INSERT INTO objects VALUES(?1,?2,?3,?4,1,0)",
                    params![
                        outcome.object_id.as_str(),
                        kind_name(&outcome.object_id),
                        relative,
                        outcome.bytes_len
                    ],
                )
                .unwrap();
        }
        connection.execute(
            r#"INSERT INTO spaces VALUES(
                'space:rewrite',
                '{"schemaVersion":"layrs.space.v1","spaceId":"space:rewrite","name":"Rewrite","createdAt":"1970-01-01T00:00:00Z","metadata":[]}',
                0
            )"#,
            [],
        ).unwrap();
        connection
            .execute(
                "INSERT INTO steps VALUES(?1,?2,0)",
                params![old_step.id().as_str(), tree.id().as_str()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO steps VALUES(?1,?2,1)",
                params![new_step.id().as_str(), tree.id().as_str()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO step_parents VALUES(?1,?2,0)",
                params![new_step.id().as_str(), old_step.id().as_str()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO repository_views VALUES(?1,'space:rewrite')",
                [view.id().as_str()],
            )
            .unwrap();
        connection
            .pragma_update(None, "ignore_check_constraints", "ON")
            .unwrap();
        connection.execute(
            "INSERT INTO operations VALUES('operation:rewrite','space:rewrite',?1,'rewrite_steps',NULL,NULL,'1970-01-01T00:00:00Z','{}')",
            [view.id().as_str()],
        ).unwrap();
        connection
            .pragma_update(None, "ignore_check_constraints", "OFF")
            .unwrap();
        connection
            .execute(
                "INSERT INTO operation_rewrites VALUES('operation:rewrite',?1,0)",
                [old_step.id().as_str()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO operation_rewrite_targets VALUES('operation:rewrite',?1,?2,0)",
                params![old_step.id().as_str(), new_step.id().as_str()],
            )
            .unwrap();
        for object in [
            view.id().as_str(),
            old_step.id().as_str(),
            new_step.id().as_str(),
        ] {
            connection
                .execute(
                    "INSERT INTO gc_roots VALUES('retained_operation','operation:rewrite',?1,0)",
                    [object],
                )
                .unwrap();
        }
    });
    let report = fsck_existing(root.path(), FsckOptions::default()).unwrap();
    assert!(
        !report
            .findings
            .iter()
            .any(|finding| finding.code == FsckFindingCode::GcRootInvalid)
    );
    assert_eq!(report.gc_reachability.roots, 3);
    assert_eq!(report.gc_reachability.reachable_objects, 3);
    assert!(
        report
            .gc_reachability
            .unreachable_objects
            .contains(&tree.id().to_string())
    );
}

#[test]
fn worktree_roots_active_view_base_step_and_materialized_tree_outside_heads() {
    let root = TestRoot::new("gc-worktree-state");
    let handle = init(root.path(), StoreId::parse("store:fsck").unwrap()).unwrap();
    drop(handle);
    let tree = Tree::new(Vec::new()).unwrap();
    let base_step = Step::new(Vec::new(), tree.id().clone()).unwrap();
    let worktree_id = WorktreeId::parse("worktree:gc-state").unwrap();
    let view = RepositoryView::new(
        SpaceId::parse("space:worktree-gc").unwrap(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![WorktreeViewState::new(
            worktree_id.clone(),
            None,
            Some(base_step.id().clone()),
            Some(tree.id().clone()),
            WorktreeStatus::Ready,
        )],
    )
    .unwrap();
    let objects = vec![
        CanonicalObject::Tree(tree.clone()),
        CanonicalObject::Step(base_step.clone()),
        CanonicalObject::RepositoryView(view.clone()),
    ];
    let exclusive = open_exclusive(root.path()).unwrap();
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let outcomes: Vec<_> = objects
        .iter()
        .map(|object| cas.put(object).unwrap())
        .collect();
    drop(cas);
    drop(exclusive);
    mutate(&root, |connection| {
        for outcome in &outcomes {
            let relative = outcome
                .path
                .strip_prefix(root.path())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            connection
                .execute(
                    "INSERT INTO objects VALUES(?1,?2,?3,?4,1,0)",
                    params![
                        outcome.object_id.as_str(),
                        kind_name(&outcome.object_id),
                        relative,
                        outcome.bytes_len
                    ],
                )
                .unwrap();
        }
        connection.execute(
            r#"INSERT INTO spaces VALUES(
                'space:worktree-gc',
                '{"schemaVersion":"layrs.space.v1","spaceId":"space:worktree-gc","name":"Worktree GC","createdAt":"1970-01-01T00:00:00Z","metadata":[]}',
                0
            )"#,
            [],
        ).unwrap();
        connection
            .execute(
                "INSERT INTO steps VALUES(?1,?2,0)",
                params![base_step.id().as_str(), tree.id().as_str()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO repository_views VALUES(?1,'space:worktree-gc')",
                [view.id().as_str()],
            )
            .unwrap();
        connection
            .pragma_update(None, "ignore_check_constraints", "ON")
            .unwrap();
        connection.execute(
            "INSERT INTO operations VALUES('operation:worktree-gc','space:worktree-gc',?1,'initialize',NULL,NULL,'1970-01-01T00:00:00Z','{}')",
            [view.id().as_str()],
        ).unwrap();
        connection
            .pragma_update(None, "ignore_check_constraints", "OFF")
            .unwrap();
        connection.execute(
            "INSERT INTO worktrees VALUES(?1,'store:fsck','space:worktree-gc','operation:worktree-gc','C:/checkout','filesystem:gc','{}',0)",
            [worktree_id.as_str()],
        ).unwrap();
        connection
            .execute(
                "INSERT INTO repository_view_worktrees VALUES(?1,?2,NULL,?3,?4,'ready',0)",
                params![
                    view.id().as_str(),
                    worktree_id.as_str(),
                    base_step.id().as_str(),
                    tree.id().as_str()
                ],
            )
            .unwrap();
        for (kind, root_id, object) in [
            (
                "retained_operation",
                "operation:worktree-gc",
                view.id().as_str(),
            ),
            ("worktree", worktree_id.as_str(), view.id().as_str()),
            ("worktree", worktree_id.as_str(), base_step.id().as_str()),
            ("worktree", worktree_id.as_str(), tree.id().as_str()),
        ] {
            connection
                .execute(
                    "INSERT INTO gc_roots VALUES(?1,?2,?3,0)",
                    params![kind, root_id, object],
                )
                .unwrap();
        }
    });
    let report = fsck_existing(root.path(), FsckOptions::default()).unwrap();
    assert!(
        !report
            .findings
            .iter()
            .any(|finding| finding.code == FsckFindingCode::GcRootInvalid)
    );
    assert_eq!(report.gc_reachability.roots, 3);
    assert_eq!(report.gc_reachability.reachable_objects, 3);
    assert!(report.gc_reachability.unreachable_objects.is_empty());
}

#[test]
fn observation_operation_must_reach_the_observed_step() {
    let root = TestRoot::new("observation-reachability");
    initialize(&root);
    let observed = oid(220);
    let unrelated_head = oid(221);
    let view = oid(222);
    mutate(&root, |connection| {
        connection.execute(
            r#"INSERT INTO spaces VALUES(
                'space:observation',
                '{"schemaVersion":"layrs.space.v1","spaceId":"space:observation","name":"Observation","createdAt":"1970-01-01T00:00:00Z","metadata":{}}',
                0
            )"#,
            [],
        ).unwrap();
        connection.execute(
            "INSERT INTO layers(layer_id,space_id,layer_kind,layer_status,payload_json,created_at_ms) VALUES('layer:observation','space:observation','base','active','{}',0)",
            [],
        ).unwrap();
        for step in [&observed, &unrelated_head] {
            connection
                .execute(
                    "INSERT INTO steps(step_id,root_tree_id,parent_count) VALUES(?1,?2,0)",
                    params![step, oid(299)],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO repository_views VALUES(?1,'space:observation')",
                [&view],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO repository_view_layer_heads VALUES(?1,'layer:observation',?2,0)",
                params![view, unrelated_head],
            )
            .unwrap();
        connection
            .pragma_update(None, "ignore_check_constraints", "ON")
            .unwrap();
        connection.execute(
            "INSERT INTO operations(operation_id,space_id,resulting_view_id,operation_kind,created_at,payload_json) VALUES('operation:observation','space:observation',?1,'initialize','1970-01-01T00:00:00Z','{}')",
            [&view],
        ).unwrap();
        connection
            .pragma_update(None, "ignore_check_constraints", "OFF")
            .unwrap();
        connection.execute(
            "INSERT INTO step_observations(observation_id,step_id,space_id,operation_id,worktree_id,actor_id,device_id,observed_at,capture_reason,payload_json) VALUES('observation:unreachable',?1,'space:observation','operation:observation',NULL,NULL,NULL,'1970-01-01T00:00:00Z','manual_snapshot','{}')",
            [&observed],
        ).unwrap();
    });
    let report = fsck_existing(root.path(), FsckOptions::default()).unwrap();
    assert!(report.findings.iter().any(|finding| {
        finding.code == FsckFindingCode::OperationHistoryInvalid
            && finding.entity_id.as_deref() == Some("observation:unreachable")
            && finding.message.contains("does not reach observed Step")
    }));
}
