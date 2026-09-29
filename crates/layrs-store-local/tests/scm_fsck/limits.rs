use super::*;

#[test]
fn invalid_core_payload_never_yields_a_projection_rebuild_action() {
    let root = TestRoot::new("typed-rebuild");
    initialize(&root);
    mutate(&root, |connection| {
        connection.execute(
            "INSERT INTO layers(layer_id,space_id,layer_kind,layer_status,payload_json,created_at_ms) VALUES('layer:invalid','space:missing','base','active','{}',0)",
            [],
        ).unwrap();
    });
    let report = fsck_existing(root.path(), FsckOptions::default()).unwrap();
    assert!(report.findings.iter().any(|finding| {
        finding.code == FsckFindingCode::ProjectionDivergence
            && finding.entity_id.as_deref() == Some("layer:invalid")
            && finding.message.contains("rejected by Core")
    }));
    assert!(
        !report
            .projection_rebuild_plan
            .actions
            .iter()
            .any(|action| { action.entity_id == "layer:invalid" })
    );
}

#[test]
fn uncheckpointed_wal_and_hostile_row_volume_are_bounded_reports() {
    let wal_root = TestRoot::new("wal-finding");
    initialize(&wal_root);
    let wal_path = PathBuf::from(format!("{}-wal", wal_root.database().display()));
    fs::write(&wal_path, b"uncheckpointed").unwrap();
    let report = fsck_existing(wal_root.path(), FsckOptions { max_findings: 1 }).unwrap();
    assert_eq!(report.findings[0].code, FsckFindingCode::UncheckpointedWal);

    let hostile = TestRoot::new("bounded-hostile");
    initialize(&hostile);
    mutate(&hostile, |connection| {
        for value in 0..100 {
            connection
                .execute(
                    "INSERT INTO gc_roots VALUES('pin',?1,?2,0)",
                    params![format!("pin:{value}"), oid(10_000 + value)],
                )
                .unwrap();
        }
    });
    let report = fsck_existing(hostile.path(), FsckOptions { max_findings: 1 }).unwrap();
    assert!(report.is_truncated());
    assert!(report.findings.len() <= 1);
    assert_eq!(report.findings[0].code, FsckFindingCode::WorkBudgetExceeded);
    assert_eq!(report.checked_objects, 0);
}

#[test]
fn hostile_quadratic_head_pairs_exhaust_the_hard_cpu_budget() {
    let root = TestRoot::new("bounded-head-pairs");
    initialize(&root);
    let view = oid(20_000);
    mutate(&root, |connection| {
        connection.execute(
            r#"INSERT INTO spaces VALUES(
                'space:pairs',
                '{"schemaVersion":"layrs.space.v1","spaceId":"space:pairs","name":"Pairs","createdAt":"1970-01-01T00:00:00Z","metadata":[]}',
                0
            )"#,
            [],
        ).unwrap();
        connection.execute(
            "INSERT INTO layers(layer_id,space_id,layer_kind,layer_status,payload_json,created_at_ms) VALUES('layer:pairs','space:pairs','base','active','{}',0)",
            [],
        ).unwrap();
        let view_digest = view.trim_start_matches("blake3:");
        connection
            .execute(
                "INSERT INTO objects VALUES(?1,'repository_view',?2,1,1,0)",
                params![
                    view,
                    format!(
                        "objects/repository_view/{}/{}/{}",
                        &view_digest[..2],
                        &view_digest[2..4],
                        view_digest
                    )
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO repository_views VALUES(?1,'space:pairs')",
                [&view],
            )
            .unwrap();
        for ordinal in 0..100_u64 {
            let step = oid(21_000 + ordinal);
            let digest = step.trim_start_matches("blake3:");
            connection
                .execute(
                    "INSERT INTO objects VALUES(?1,'step',?2,1,1,0)",
                    params![
                        step,
                        format!("objects/step/{}/{}/{}", &digest[..2], &digest[2..4], digest)
                    ],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO steps VALUES(?1,?2,0)",
                    params![step, oid(30_000)],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO repository_view_layer_heads VALUES(?1,'layer:pairs',?2,?3)",
                    params![view, step, ordinal],
                )
                .unwrap();
        }
    });
    let report = fsck_existing(root.path(), FsckOptions { max_findings: 16 }).unwrap();
    assert!(report.is_truncated());
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.code == FsckFindingCode::WorkBudgetExceeded)
    );
}

#[test]
fn oversized_format_is_bounded_before_read_and_fsck_is_read_only() {
    let root = TestRoot::new("bounded-format-bytes");
    initialize(&root);
    let format_path = root.path().join("FORMAT");
    let file = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&format_path)
        .unwrap();
    file.set_len(128 * 1024).unwrap();
    drop(file);
    let before = snapshot(root.path());
    let report = fsck_existing(root.path(), FsckOptions { max_findings: 1 }).unwrap();
    assert_eq!(report.findings[0].code, FsckFindingCode::WorkBudgetExceeded);
    assert!(report.is_truncated());
    assert_eq!(snapshot(root.path()), before);
}

#[test]
fn oversized_store_payload_is_rejected_before_sqlite_string_extraction() {
    let root = TestRoot::new("bounded-store-payload");
    initialize(&root);
    mutate(&root, |connection| {
        connection.execute(
            "UPDATE stores SET payload_json=json_object('storeId','store:fsck','padding',printf('%.*c',?1,'x'))",
            [17 * 1024 * 1024_u64],
        ).unwrap();
    });
    let database_len = fs::metadata(root.database()).unwrap().len();
    let report = fsck_existing(root.path(), FsckOptions { max_findings: 1 }).unwrap();
    assert_eq!(report.findings[0].code, FsckFindingCode::WorkBudgetExceeded);
    assert!(report.is_truncated());
    assert_eq!(fs::metadata(root.database()).unwrap().len(), database_len);
    assert!(!PathBuf::from(format!("{}-wal", root.database().display())).exists());
    assert!(!PathBuf::from(format!("{}-shm", root.database().display())).exists());
}

#[test]
fn oversized_entity_payload_reserves_budget_finding_when_cap_is_full() {
    let root = TestRoot::new("bounded-entity-payload");
    initialize(&root);
    mutate(&root, |connection| {
        connection
            .execute(
                "INSERT INTO objects VALUES(?1,'tree',?2,1,1,0)",
                params![
                    oid(90_000),
                    "objects/tree/00/00/".to_owned() + &"0".repeat(64)
                ],
            )
            .unwrap();
        connection.execute(
            "INSERT INTO spaces(space_id,payload_json,created_at_ms) VALUES('space:huge',json_object('schemaVersion','layrs.space.v1','spaceId','space:huge','name','Huge','createdAt','1970-01-01T00:00:00Z','metadata',json_array(json_object('key','padding','value',printf('%.*c',?1,'x')))),0)",
            [17 * 1024 * 1024_u64],
        ).unwrap();
    });
    let report = fsck_existing(root.path(), FsckOptions { max_findings: 1 }).unwrap();
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].code, FsckFindingCode::WorkBudgetExceeded);
    assert!(report.is_truncated());
}

#[test]
fn oversized_cas_object_reserves_budget_finding_when_cap_is_full() {
    let root = TestRoot::new("bounded-cas-object");
    let handle = init(root.path(), StoreId::parse("store:fsck").unwrap()).unwrap();
    drop(handle);
    let chunk = Chunk::new(vec![42; 5 * 1024 * 1024]).unwrap();
    let exclusive = open_exclusive(root.path()).unwrap();
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let outcome = cas.put(&CanonicalObject::ChunkRaw(chunk.clone())).unwrap();
    drop(cas);
    drop(exclusive);
    mutate(&root, |connection| {
        // The false path fills the single finding slot before the byte budget
        // check. The budget finding must replace it deterministically.
        connection
            .execute(
                "INSERT INTO objects VALUES(?1,'chunk_raw','objects/chunk_raw/wrong',?2,1,0)",
                params![chunk.id().as_str(), outcome.bytes_len],
            )
            .unwrap();
    });
    let report = fsck_existing(root.path(), FsckOptions { max_findings: 1 }).unwrap();
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].code, FsckFindingCode::WorkBudgetExceeded);
    assert!(report.is_truncated());
    let audit = open_audit(root.path()).unwrap();
    NativeCas::open_audit(&audit)
        .unwrap()
        .verify(&StoreObjectId::ChunkRaw(chunk.id().clone()))
        .unwrap();
}
