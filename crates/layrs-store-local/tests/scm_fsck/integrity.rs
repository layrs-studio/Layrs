use super::*;

#[test]
fn unsupported_or_absent_store_is_reported_without_bootstrap() {
    let root = TestRoot::new("absent");
    let report = fsck_existing(root.path(), FsckOptions::default()).unwrap();
    assert_eq!(report.findings[0].code, FsckFindingCode::FormatInvalid);
    assert!(!root.path().exists());
}

#[test]
fn missing_and_tampered_cas_are_reported_with_stable_codes() {
    let root = TestRoot::new("cas-corruption");
    let handle = init(root.path(), StoreId::parse("store:fsck").unwrap()).unwrap();
    drop(handle);
    let exclusive = open_exclusive(root.path()).unwrap();
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let object = CanonicalObject::ChunkRaw(Chunk::new(b"tamper me".to_vec()).unwrap());
    let outcome = cas.put(&object).unwrap();
    let relative = outcome
        .path
        .strip_prefix(root.path())
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    fs::write(&outcome.path, b"corrupt").unwrap();
    drop(cas);
    drop(exclusive);
    mutate(&root, |connection| {
        connection
            .execute(
                "INSERT INTO objects(object_id,object_kind,storage_path,byte_length,verified,created_at_ms) VALUES(?1,'chunk_raw',?2,?3,1,0)",
                params![outcome.object_id.as_str(), relative, outcome.bytes_len],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO objects(object_id,object_kind,storage_path,byte_length,verified,created_at_ms) VALUES(?1,'tree',?2,1,1,0)",
                params![oid(900), "objects/tree/00/00/".to_owned() + &"0".repeat(64)],
            )
            .unwrap();
    });
    let codes = codes(&root, 64);
    assert!(codes.contains(&FsckFindingCode::CasCorruption));
    assert!(codes.contains(&FsckFindingCode::MissingCasObject));
}

#[test]
fn manifest_is_reconstructed_from_verified_chunk_bytes() {
    let root = TestRoot::new("manifest-reconstruction");
    let handle = init(root.path(), StoreId::parse("store:fsck").unwrap()).unwrap();
    drop(handle);
    let exclusive = open_exclusive(root.path()).unwrap();
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let chunk = Chunk::new(b"manifest payload".to_vec()).unwrap();
    let manifest =
        FileManifest::from_chunks(ChunkingDescriptor::single(), std::slice::from_ref(&chunk))
            .unwrap();
    let outcome = cas
        .put(&CanonicalObject::FileManifest(manifest.clone()))
        .unwrap();
    let relative = outcome
        .path
        .strip_prefix(root.path())
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    drop(cas);
    drop(exclusive);
    mutate(&root, |connection| {
        connection.execute(
            "INSERT INTO objects(object_id,object_kind,storage_path,byte_length,verified,created_at_ms) VALUES(?1,'file_manifest',?2,?3,1,0)",
            params![manifest.id().as_str(), relative, outcome.bytes_len],
        ).unwrap();
        connection
            .execute(
                "INSERT INTO file_availability VALUES(?1,'present',0)",
                [manifest.file_id().as_str()],
            )
            .unwrap();
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
        let entry = &manifest.chunks()[0];
        connection
            .execute(
                "INSERT INTO file_manifest_chunks VALUES(?1,0,?2,?3,?4)",
                params![
                    manifest.id().as_str(),
                    entry.chunk_id().as_str(),
                    entry.offset(),
                    entry.byte_length()
                ],
            )
            .unwrap();
    });
    let codes = codes(&root, 64);
    assert!(codes.contains(&FsckFindingCode::ManifestReconstructionFailure));
}

#[test]
fn graph_antichain_cross_space_projection_fk_and_recovery_corruption_are_detected() {
    let root = TestRoot::new("logical-corruption");
    initialize(&root);
    let step_a = oid(1);
    let step_b = oid(2);
    let missing = oid(3);
    mutate(&root, |connection| {
        connection.execute("UPDATE stores SET payload_json='{\"schemaVersion\":\"layrs.store.v1\",\"storeId\":\"store:wrong\"}'", []).unwrap();
        connection.execute("INSERT INTO spaces VALUES('space:a','{\"schemaVersion\":\"layrs.space.v1\",\"spaceId\":\"space:a\",\"name\":\"A\",\"metadata\":{}}',0)", []).unwrap();
        connection.execute("INSERT INTO spaces VALUES('space:b','{\"schemaVersion\":\"layrs.space.v1\",\"spaceId\":\"space:b\",\"name\":\"B\",\"metadata\":{}}',0)", []).unwrap();
        for step in [&step_a, &step_b] {
            connection
                .execute(
                    "INSERT INTO steps(step_id,root_tree_id,parent_count) VALUES(?1,?2,1)",
                    params![step, oid(99)],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO step_parents VALUES(?1,?2,0)",
                params![step_a, step_b],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO step_parents VALUES(?1,?2,0)",
                params![step_b, step_a],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO step_parents VALUES(?1,?2,1)",
                params![step_a, missing],
            )
            .unwrap();
        connection.execute("INSERT INTO layers(layer_id,space_id,layer_kind,layer_status,payload_json,created_at_ms) VALUES('layer:b','space:b','base','active','{}',0)", []).unwrap();
        connection
            .execute(
                "INSERT INTO repository_views VALUES(?1,'space:a')",
                [oid(20)],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO repository_view_layer_heads VALUES(?1,'layer:b',?2,0)",
                params![oid(20), step_a],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO repository_view_layer_heads VALUES(?1,'layer:b',?2,1)",
                params![oid(20), step_b],
            )
            .unwrap();
        connection.execute("INSERT INTO layers(layer_id,space_id,layer_kind,layer_status,payload_json,created_at_ms) VALUES('layer:orphan','space:missing','base','active','{}',0)", []).unwrap();
        connection
            .pragma_update(None, "ignore_check_constraints", "ON")
            .unwrap();
        connection.execute("INSERT INTO operations(operation_id,space_id,resulting_view_id,operation_kind,created_at,payload_json) VALUES('operation:mixed','space:a',?1,'initialize','1970-01-01T00:00:00Z','{}')", [oid(20)]).unwrap();
        for (id, kind) in [
            ("operation:a", "weave"),
            ("operation:b", "weave"),
            ("operation:integrate", "integrate_operations"),
        ] {
            connection.execute(
                "INSERT INTO operations(operation_id,space_id,resulting_view_id,operation_kind,created_at,payload_json) VALUES(?1,'space:a',?2,?3,'1970-01-01T00:00:00Z','{}')",
                params![id, oid(20), kind],
            ).unwrap();
        }
        connection
            .pragma_update(None, "ignore_check_constraints", "OFF")
            .unwrap();
        connection
            .execute(
                "INSERT INTO operation_parents VALUES('operation:a','operation:b',0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO operation_parents VALUES('operation:a','operation:missing',1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO operation_parents VALUES('operation:b','operation:a',0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO operation_parents VALUES('operation:integrate','operation:a',0)",
                [],
            )
            .unwrap();
        connection.execute(
            "INSERT INTO recovery_operations(
                recovery_operation_id,idempotency_key,request_digest,worktree_id,operation_id,
                recovery_type,state,payload_json,evidence_json,attempt_count,owner_token,last_error,
                created_at_ms,updated_at_ms,completed_at_ms
             ) VALUES('operation:mixed','recovery:test','digest','worktree:missing','operation:mixed',
                      'restore','pending','{}','{}',0,NULL,NULL,0,0,NULL)",
            [],
        ).unwrap();
        connection
            .execute(
                "INSERT INTO gc_roots VALUES('pin','pin:missing',?1,0)",
                [oid(777)],
            )
            .unwrap();
    });
    let codes = codes(&root, 128);
    for expected in [
        FsckFindingCode::StepCycle,
        FsckFindingCode::StepParentMissing,
        FsckFindingCode::ViewHeadsNotAntichain,
        FsckFindingCode::CrossSpaceReference,
        FsckFindingCode::ProjectionDivergence,
        FsckFindingCode::ForeignKeyViolation,
        FsckFindingCode::RecoveryOperationMixing,
        FsckFindingCode::OperationCycle,
        FsckFindingCode::OperationParentMissing,
        FsckFindingCode::OperationHistoryInvalid,
        FsckFindingCode::GcRootInvalid,
    ] {
        assert!(codes.contains(&expected), "missing {expected:?}: {codes:?}");
    }
}

#[test]
fn findings_are_globally_capped_and_truncation_is_counted() {
    let root = TestRoot::new("cap");
    initialize(&root);
    mutate(&root, |connection| {
        for value in 100..120 {
            let id = oid(value);
            let digest = id.trim_start_matches("blake3:");
            let path = format!("objects/tree/{}/{}/{}", &digest[..2], &digest[2..4], digest);
            connection.execute("INSERT INTO objects(object_id,object_kind,storage_path,byte_length,verified,created_at_ms) VALUES(?1,'tree',?2,1,1,0)", params![id,path]).unwrap();
        }
    });
    let report = fsck_existing(root.path(), FsckOptions { max_findings: 3 }).unwrap();
    assert_eq!(report.findings.len(), 3);
    assert!(report.truncated_count >= 17);
}
