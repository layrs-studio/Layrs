use super::*;

#[test]
fn empty_native_store_is_clean_and_fsck_changes_no_bytes_or_paths() {
    let root = TestRoot::new("read-only");
    initialize(&root);
    let before = snapshot(root.path());
    let audit = open_audit(root.path()).unwrap();
    let report = NativeFsck::run(&audit, FsckOptions::default()).unwrap();
    assert!(report.is_clean(), "{:?}", report.findings);
    drop(audit);
    assert_eq!(snapshot(root.path()), before);
}

#[test]
fn missing_bootstrap_store_identity_is_corruption() {
    let root = TestRoot::new("missing-store-identity");
    initialize(&root);
    mutate(&root, |connection| {
        connection.execute("DELETE FROM stores", []).unwrap();
    });
    assert!(codes(&root, 8).contains(&FsckFindingCode::StoreIdMismatch));
}

#[test]
fn store_projection_accepts_only_the_exact_canonical_format_payload() {
    let root = TestRoot::new("store-payload-exact");
    initialize(&root);
    for invalid in [
        r#"{"extra":true,"storeId":"store:fsck"}"#,
        r#"{"storeId":"store:replacement"}"#,
    ] {
        mutate(&root, |connection| {
            connection
                .execute("UPDATE stores SET payload_json=?1", [invalid])
                .unwrap();
        });
        let report = fsck_existing(root.path(), FsckOptions::default()).unwrap();
        assert!(report.findings.iter().any(|finding| {
            finding.code == FsckFindingCode::StoreIdMismatch
                && finding
                    .message
                    .contains("exactly the canonical FORMAT Store payload")
        }));
    }
}

#[test]
fn space_created_at_millis_is_checked_and_rebuildable_from_typed_payload() {
    let root = TestRoot::new("space-created-at");
    initialize(&root);
    mutate(&root, |connection| {
        connection.execute(
            r#"INSERT INTO spaces VALUES(
                'space:timestamp',
                '{"schemaVersion":"layrs.space.v1","spaceId":"space:timestamp","name":"Space","createdAt":"1970-01-01T00:00:01Z","metadata":[]}',
                999
            )"#,
            [],
        ).unwrap();
    });
    assert_timestamp_rebuild(&root, "spaces", "space:timestamp");
}

#[test]
fn fractional_entity_timestamp_roundtrips_and_fsck_is_clean_for_its_projection() {
    let root = TestRoot::new("fractional-created-at");
    initialize(&root);
    let space = Space::new(
        SpaceId::parse("space:fractional").unwrap(),
        "Fractional",
        Timestamp::parse("1970-01-01T00:00:00.1239Z").unwrap(),
        Metadata::empty(),
    )
    .unwrap();
    let payload = String::from_utf8(canonical_json(&space).unwrap()).unwrap();
    mutate(&root, |connection| {
        connection
            .execute(
                "INSERT INTO spaces(space_id,payload_json,created_at_ms) VALUES(?1,?2,123)",
                params![space.id().as_str(), payload],
            )
            .unwrap();
    });

    let store = open(root.path()).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert_eq!(reader.get_space(space.id()).unwrap(), Some(space.clone()));
    drop(reader);
    drop(store);

    let report = fsck_existing(root.path(), FsckOptions::default()).unwrap();
    assert!(!report.findings.iter().any(|finding| {
        finding.code == FsckFindingCode::ProjectionDivergence
            && finding.entity_id.as_deref() == Some(space.id().as_str())
    }));
}

#[test]
fn operation_nullable_actor_and_device_projections_accept_none_and_some() {
    let root = TestRoot::new("operation-nullable-identity");
    initialize(&root);
    let space_id = SpaceId::parse("space:operation-nullable").unwrap();
    let view = RepositoryView::new(
        space_id.clone(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let timestamp = Timestamp::parse("2026-07-20T02:03:04.1239+02:00").unwrap();
    let operations = [
        Operation::new(
            OperationId::parse("operation:null-identities").unwrap(),
            space_id.clone(),
            Vec::new(),
            view.id().clone(),
            OperationKind::CaptureWorktree,
            "null identities",
            Vec::new(),
            None,
            None,
            timestamp.clone(),
            Metadata::empty(),
        )
        .unwrap(),
        Operation::new(
            OperationId::parse("operation:present-identities").unwrap(),
            space_id,
            Vec::new(),
            view.id().clone(),
            OperationKind::CaptureWorktree,
            "present identities",
            Vec::new(),
            Some(ActorId::parse("actor:fsck").unwrap()),
            Some(DeviceId::parse("device:fsck").unwrap()),
            timestamp,
            Metadata::empty(),
        )
        .unwrap(),
    ];
    mutate(&root, |connection| {
        for operation in &operations {
            let payload = String::from_utf8(canonical_json(operation).unwrap()).unwrap();
            connection
                .execute(
                    "INSERT INTO operations(operation_id,space_id,resulting_view_id,operation_kind,actor_id,device_id,created_at,payload_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![
                        operation.id().as_str(),
                        operation.space_id().as_str(),
                        operation.resulting_view_id().as_str(),
                        "capture_worktree",
                        operation.actor_id().map(ActorId::as_str),
                        operation.device_id().map(DeviceId::as_str),
                        operation.created_at().as_str(),
                        payload,
                    ],
                )
                .unwrap();
        }
    });

    let report = fsck_existing(root.path(), FsckOptions::default()).unwrap();
    for operation in &operations {
        assert!(!report.findings.iter().any(|finding| {
            finding.code == FsckFindingCode::ProjectionDivergence
                && finding.entity_id.as_deref() == Some(operation.id().as_str())
                && (finding.message.contains("actor_id") || finding.message.contains("device_id"))
        }));
    }
}

#[test]
fn layer_created_at_millis_is_checked_and_rebuildable_from_typed_payload() {
    let root = TestRoot::new("layer-created-at");
    initialize(&root);
    mutate(&root, |connection| {
        connection.execute(
            r#"INSERT INTO layers VALUES(
                'layer:timestamp','space:timestamp','base','active',
                '{"schemaVersion":"layrs.layer.v1","layerId":"layer:timestamp","spaceId":"space:timestamp","name":"Layer","kind":"base","status":"active","parentLayerIds":[],"settings":{"automaticCapture":true,"automaticCaptureIdleMs":1000},"createdAt":"1970-01-01T00:00:01Z","metadata":[]}',
                999
            )"#,
            [],
        ).unwrap();
    });
    assert_timestamp_rebuild(&root, "layers", "layer:timestamp");
}

#[test]
fn worktree_created_at_millis_is_checked_and_rebuildable_from_typed_payload() {
    let root = TestRoot::new("worktree-created-at");
    initialize(&root);
    mutate(&root, |connection| {
        connection.execute(
            r#"INSERT INTO worktrees VALUES(
                'worktree:timestamp','store:fsck','space:timestamp','operation:timestamp',
                'C:/checkout','filesystem:timestamp',
                '{"schemaVersion":"layrs.worktree.v1","worktreeId":"worktree:timestamp","storeId":"store:fsck","spaceId":"space:timestamp","activeOperationId":"operation:timestamp","checkoutRoot":"C:/checkout","filesystemIdentity":"filesystem:timestamp","createdAt":"1970-01-01T00:00:01Z","metadata":[]}',
                999
            )"#,
            [],
        ).unwrap();
    });
    assert_timestamp_rebuild(&root, "worktrees", "worktree:timestamp");
}
