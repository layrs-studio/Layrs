#[test]
fn publish_v2_payload_contains_store_objects_and_canonical_ids() {
    if !isolated_legacy_test("publish_v2_payload_contains_store_objects_and_canonical_ids") {
        return;
    }
    let root = unique_test_dir("publish-v2");
    let config_dir = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config_dir);
    fs::write(space.join("note.txt"), "base\n").unwrap();

    let created = create_local_space(
        "space-publish-v2".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    let mut handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    handle.meta.workspace_id = "workspace_1".to_string();
    handle.meta.space_id = "space_1".to_string();
    write_json(
        &mut handle.store,
        &handle.layrs_dir.join("local-space.json"),
        &handle.meta,
    )
    .unwrap();
    let base_state = read_layer_state(&handle.store, &handle.layrs_dir, "main").unwrap();

    fs::write(space.join("note.txt"), "changed\n").unwrap();
    let state = capture_working_state(&mut handle.store, &space, "main").unwrap();
    let config = DesktopConfig {
        server_endpoint: "http://127.0.0.1:8787".to_string(),
        device_id: "client_test".to_string(),
        auto_receive: false,
        auto_publish: false,
        auto_local_steps: true,
        sync_interval_seconds: 300,
        default_local_spaces_folder: root.display().to_string(),
        shortcuts: Default::default(),
        local_spaces: Vec::new(),
    };
    let step_id = write_step(&mut handle.store, &handle.layrs_dir, "main", &state).unwrap();
    let step = read_step_file(&handle.store, &handle.layrs_dir, "main", &step_id).unwrap();
    let body = build_publish_v2_request(
        &handle,
        &config,
        "main",
        base_state.root_tree_id.clone(),
        &state,
        vec!["note.txt".to_string()],
        Vec::new(),
        std::slice::from_ref(&step),
    )
    .unwrap();
    let json = serde_json::to_value(body).unwrap();

    assert_eq!(json["protocol"], SYNC_PROTOCOL_V2);
    assert_eq!(json["layerId"], "main");
    assert_eq!(json["policyEpoch"], 1);
    assert_eq!(json["sourceClientId"], "client_test");
    assert!(json["idempotencyKey"]
        .as_str()
        .unwrap()
        .starts_with("publish-"));
    assert!(json["baseTreeId"].as_str().unwrap().starts_with("blake3:"));
    assert!(json["rootTreeId"].as_str().unwrap().starts_with("blake3:"));
    assert_eq!(json["changedPaths"], serde_json::json!(["note.txt"]));
    assert_eq!(json["deletedPaths"], serde_json::json!([]));
    assert_eq!(json["steps"].as_array().map(Vec::len), Some(1));
    assert_eq!(json["steps"][0]["stepId"].as_str(), Some(step_id.as_str()));
    assert_eq!(
        json["steps"][0]["changedPaths"],
        serde_json::json!(["note.txt"])
    );
    assert_eq!(
        json["steps"][0]["rootTreeId"].as_str(),
        state.root_tree_id.as_deref()
    );
    assert!(json.get("artifacts").is_none());

    let store = &json["storeObjects"];
    assert!(store.is_object());
    let store_keys = store
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        store_keys,
        BTreeSet::from([
            "chunks".to_string(),
            "fileObjects".to_string(),
            "treeObjects".to_string()
        ])
    );
    assert_eq!(store["treeObjects"].as_array().unwrap().len(), 1);
    assert_eq!(store["fileObjects"].as_array().unwrap().len(), 1);
    assert_eq!(store["chunks"].as_array().unwrap().len(), 1);
    assert!(store["treeObjects"][0]["treeId"]
        .as_str()
        .unwrap()
        .starts_with("blake3:"));
    assert!(store["fileObjects"][0]["fileObjectId"]
        .as_str()
        .unwrap()
        .starts_with("blake3:"));
    assert!(store["chunks"][0]["chunkId"]
        .as_str()
        .unwrap()
        .starts_with("blake3:"));
    assert_eq!(store["chunks"][0]["digest"], store["chunks"][0]["chunkId"]);
    assert!(store["chunks"][0].get("data").is_none());
    assert!(store["chunks"][0].get("encoding").is_none());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn publish_v2_payload_contains_every_pending_step_in_order() {
    if !isolated_legacy_test("publish_v2_payload_contains_every_pending_step_in_order") {
        return;
    }
    let root = unique_test_dir("publish-v2-pending-steps");
    let config_dir = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config_dir);
    fs::write(space.join("note.txt"), "base\n").unwrap();

    let created = create_local_space(
        "space-pending-steps".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    let mut handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    handle.meta.workspace_id = "workspace_1".to_string();
    handle.meta.space_id = "space_1".to_string();
    write_json(
        &mut handle.store,
        &handle.layrs_dir.join("local-space.json"),
        &handle.meta,
    )
    .unwrap();
    let base_state = read_layer_state(&handle.store, &handle.layrs_dir, "main").unwrap();

    fs::write(space.join("note.txt"), "step one\n").unwrap();
    let first_state = capture_working_state(&mut handle.store, &space, "main").unwrap();
    let first_step_id =
        write_step(&mut handle.store, &handle.layrs_dir, "main", &first_state).unwrap();
    let first_step =
        read_step_file(&handle.store, &handle.layrs_dir, "main", &first_step_id).unwrap();
    write_pending_publish(&mut handle.store, &handle.layrs_dir, &first_step).unwrap();

    fs::write(space.join("note.txt"), "step two\n").unwrap();
    let second_state = capture_working_state(&mut handle.store, &space, "main").unwrap();
    let second_step_id =
        write_step(&mut handle.store, &handle.layrs_dir, "main", &second_state).unwrap();
    let second_step =
        read_step_file(&handle.store, &handle.layrs_dir, "main", &second_step_id).unwrap();
    write_pending_publish(&mut handle.store, &handle.layrs_dir, &second_step).unwrap();

    let pending_steps = pending_publish_steps(&handle.store, &handle.layrs_dir, "main").unwrap();
    let config = DesktopConfig {
        server_endpoint: "http://127.0.0.1:8787".to_string(),
        device_id: "client_test".to_string(),
        auto_receive: false,
        auto_publish: false,
        auto_local_steps: true,
        sync_interval_seconds: 300,
        default_local_spaces_folder: root.display().to_string(),
        shortcuts: Default::default(),
        local_spaces: Vec::new(),
    };
    let body = build_publish_v2_request(
        &handle,
        &config,
        "main",
        base_state.root_tree_id.clone(),
        &second_state,
        vec!["note.txt".to_string()],
        Vec::new(),
        &pending_steps,
    )
    .unwrap();
    let json = serde_json::to_value(body).unwrap();

    assert_eq!(json["steps"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        json["steps"][0]["stepId"].as_str(),
        Some(first_step_id.as_str())
    );
    assert_eq!(
        json["steps"][1]["stepId"].as_str(),
        Some(second_step_id.as_str())
    );
    assert_eq!(
        json["steps"][0]["rootTreeId"].as_str(),
        first_state.root_tree_id.as_deref()
    );
    assert_eq!(
        json["steps"][1]["rootTreeId"].as_str(),
        second_state.root_tree_id.as_deref()
    );
    let tree_ids = json["storeObjects"]["treeObjects"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tree| tree.get("treeId").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    assert!(tree_ids.contains(first_state.root_tree_id.as_deref().unwrap()));
    assert!(tree_ids.contains(second_state.root_tree_id.as_deref().unwrap()));
    assert_ne!(
        json["steps"][0]["rootTreeId"],
        json["steps"][1]["rootTreeId"]
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn initial_publish_for_new_child_layer_includes_inherited_steps() {
    if !isolated_legacy_test("initial_publish_for_new_child_layer_includes_inherited_steps") {
        return;
    }
    let root = unique_test_dir("publish-v2-initial-child-layer");
    let config_dir = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config_dir);
    fs::write(space.join("note.txt"), "main\n").unwrap();

    let created = create_local_space(
        "space-initial-child-layer".to_string(),
        space.display().to_string(),
        Some("layer_main_initial_publish".to_string()),
    )
    .unwrap();
    let mut handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    handle.meta.workspace_id = "workspace_1".to_string();
    handle.meta.space_id = "space_1".to_string();
    write_json(
        &mut handle.store,
        &handle.layrs_dir.join("local-space.json"),
        &handle.meta,
    )
    .unwrap();

    let parent_layer_id = "layer_main_initial_publish";
    let child_layer_id = "layer_feature_initial_publish";
    let main_state = capture_working_state(&mut handle.store, &space, parent_layer_id).unwrap();
    let main_step_id = write_step(
        &mut handle.store,
        &handle.layrs_dir,
        parent_layer_id,
        &main_state,
    )
    .unwrap();
    handle.meta.layers.push(LocalLayerMetadata {
        layer_id: child_layer_id.to_string(),
        display_name: "Feature".to_string(),
        parent_layer_id: Some(parent_layer_id.to_string()),
        lineage_status: default_layer_lineage_status(),
        access: LayerAccessKind::Open,
        can_open: true,
    });
    write_json(
        &mut handle.store,
        &handle.layrs_dir.join("local-space.json"),
        &handle.meta,
    )
    .unwrap();
    scaffold_layer(
        &mut handle.store,
        &handle.layrs_dir,
        &handle.meta.local_space_id,
        &LayerAccessView {
            layer_id: child_layer_id.to_string(),
            workspace_id: handle.meta.workspace_id.clone(),
            space_id: handle.meta.space_id.clone(),
            display_name: "Feature".to_string(),
            access: LayerAccessKind::Open,
            can_open: true,
            local_path: Some(
                layer_dir(&handle.layrs_dir, child_layer_id)
                    .display()
                    .to_string(),
            ),
            reason: None,
        },
    )
    .unwrap();
    let mut child_state = main_state.clone();
    child_state.layer_id = child_layer_id.to_string();
    write_layer_state(
        &mut handle.store,
        &handle.layrs_dir,
        child_layer_id,
        &child_state,
    )
    .unwrap();
    inherit_parent_steps(&mut handle, parent_layer_id, child_layer_id).unwrap();
    write_linked_layer_sync_state(&mut handle, child_layer_id, Some(parent_layer_id)).unwrap();

    assert!(layer_needs_initial_publish(&handle, child_layer_id).unwrap());
    assert_eq!(
        pending_publish_steps(&handle.store, &handle.layrs_dir, child_layer_id)
            .unwrap()
            .len(),
        0
    );

    let publish_steps =
        publish_steps_for_layer(&handle.store, &handle.layrs_dir, child_layer_id, true).unwrap();
    assert_eq!(publish_steps.len(), 1);
    assert_eq!(publish_steps[0].step_kind.as_deref(), Some("inherited"));
    assert_eq!(
        publish_steps[0].origin_step_id.as_deref(),
        Some(main_step_id.as_str())
    );

    let feature_state = read_layer_state(&handle.store, &handle.layrs_dir, child_layer_id).unwrap();
    let config = DesktopConfig {
        server_endpoint: "http://127.0.0.1:8787".to_string(),
        device_id: "client_test".to_string(),
        auto_receive: false,
        auto_publish: false,
        auto_local_steps: true,
        sync_interval_seconds: 300,
        default_local_spaces_folder: root.display().to_string(),
        shortcuts: Default::default(),
        local_spaces: Vec::new(),
    };
    let changed_paths = diff_state(None, &feature_state)
        .0
        .into_iter()
        .collect::<Vec<_>>();
    let body = build_publish_v2_request(
        &handle,
        &config,
        child_layer_id,
        None,
        &feature_state,
        changed_paths,
        Vec::new(),
        &publish_steps,
    )
    .unwrap();
    let json = serde_json::to_value(body).unwrap();

    assert_eq!(json["steps"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        json["steps"][0]["originStepId"].as_str(),
        Some(main_step_id.as_str())
    );
    assert_eq!(json["steps"][0]["stepKind"].as_str(), Some("inherited"));
    assert_eq!(json["changedPaths"], serde_json::json!(["note.txt"]));
    assert_eq!(
        json["storeObjects"]["fileObjects"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(json["storeObjects"]["chunks"].as_array().unwrap().len(), 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn receive_v2_content_objects_materializes_chunk_bytes() {
    if !isolated_legacy_test("receive_v2_content_objects_materializes_chunk_bytes") {
        return;
    }
    let root = unique_test_dir("receive-v2");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("note.txt"), "local\n").unwrap();

    let created = create_local_space(
        "space-receive-v2".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    let mut handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    let bytes = b"server bytes\n".to_vec();
    let chunk_id = blake3_id(&bytes);
    let file_object_id = blake3_id(&bytes);
    let chunk_dir = handle.layrs_dir.join("objects").join("chunks");
    fs::create_dir_all(&chunk_dir).unwrap();
    fs::write(
        chunk_dir.join(format!("{}.chunk", object_file_stem(&chunk_id))),
        &bytes,
    )
    .unwrap();
    let files = vec![FileSnapshotEntry {
        path: "note.txt".to_string(),
        object: format!("objects/files/{}.json", object_file_stem(&file_object_id)),
        hash: file_object_id.clone(),
        size: bytes.len() as u64,
    }];
    let tree_id = tree_id_for_files(&files);
    let response = ReceiveLocalSpaceResponse {
        workspace_id: handle.meta.workspace_id.clone(),
        space_id: handle.meta.space_id.clone(),
        layer_id: handle.active.layer_id.clone(),
        protocol: Some(SYNC_PROTOCOL_V2.to_string()),
        root_tree_id: Some(tree_id.clone()),
        cursor: Some("cursor_1".to_string()),
        layers: vec![
            ReceivedLayer {
                id: handle.active.layer_id.clone(),
                workspace_id: Some(handle.meta.workspace_id.clone()),
                space_id: Some(handle.meta.space_id.clone()),
                name: "Main".to_string(),
                parent_layer_id: None,
                access: Some("open".to_string()),
            },
            ReceivedLayer {
                id: "layer_without_head".to_string(),
                workspace_id: Some(handle.meta.workspace_id.clone()),
                space_id: Some(handle.meta.space_id.clone()),
                name: "Metadata Only".to_string(),
                parent_layer_id: Some(handle.active.layer_id.clone()),
                access: Some("open".to_string()),
            },
        ],
        access_registries: Vec::new(),
        content_objects: Some(ReceivedContentObjects {
            chunks: vec![ReceivedChunkObject {
                chunk_id: chunk_id.clone(),
                digest: Some(chunk_id.clone()),
                download_url: None,
                size: Some(bytes.len() as u64),
                size_bytes: None,
                raw_size: None,
                stored_size: None,
                compression: None,
            }],
            file_objects: vec![ReceivedFileObject {
                file_object_id: file_object_id.clone(),
                hash: Some(file_object_id.clone()),
                size: Some(bytes.len() as u64),
                chunks: vec![ReceivedChunkRef {
                    chunk_id,
                    size: Some(bytes.len() as u64),
                    size_bytes: None,
                    raw_size: None,
                    stored_size: None,
                    compression: None,
                }],
            }],
            tree_objects: vec![ReceivedTreeObject {
                tree_id: tree_id.clone(),
                layer_id: Some(handle.active.layer_id.clone()),
                entries: vec![ReceivedTreeEntry {
                    path: "note.txt".to_string(),
                    file_object_id: Some(file_object_id),
                    size: Some(bytes.len() as u64),
                }],
            }],
        }),
        timeline: Vec::new(),
        steps: vec![ReceivedStep {
            step_id: "step_server_1".to_string(),
            layer_id: handle.active.layer_id.clone(),
            parent_step_id: None,
            base_layer_id: Some(handle.active.layer_id.clone()),
            base_tree_id: None,
            root_tree_id: Some(tree_id.clone()),
            changed_paths: vec!["note.txt".to_string()],
            captured_at_unix: Some(1_782_910_398),
            timeline_position: None,
            origin_layer_id: None,
            origin_layer_name: None,
            origin_step_id: None,
            step_kind: None,
        }],
    };

    apply_receive_response(&mut handle, response, true).unwrap();

    assert_eq!(fs::read(space.join("note.txt")).unwrap(), bytes);
    assert!(handle
        .meta
        .layers
        .iter()
        .any(|layer| layer.layer_id == "layer_without_head"));
    let received_steps =
        read_step_files(&handle.store, &handle.layrs_dir, &handle.active.layer_id).unwrap();
    let received_step = received_steps
        .iter()
        .find(|step| step.step_id == "step_server_1")
        .expect("received step persisted");
    assert_eq!(received_step.timeline_position, Some(0));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn received_step_without_server_position_is_still_latest_by_time() {
    if !isolated_legacy_test("received_step_without_server_position_is_still_latest_by_time") {
        return;
    }
    let root = unique_test_dir("receive-step-position-order");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("note.txt"), "base\n").unwrap();

    let created = create_local_space(
        "space-receive-step-position-order".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    let mut handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    handle.meta.workspace_id = "workspace_1".to_string();
    handle.meta.space_id = "space_1".to_string();
    write_json(
        &mut handle.store,
        &handle.layrs_dir.join("local-space.json"),
        &handle.meta,
    )
    .unwrap();

    fs::write(space.join("note.txt"), "local older\n").unwrap();
    let local_state =
        capture_working_state(&mut handle.store, &space, &handle.active.layer_id).unwrap();
    let local_step_id = write_step(
        &mut handle.store,
        &handle.layrs_dir,
        &handle.active.layer_id,
        &local_state,
    )
    .unwrap();
    let local_step = read_step_file(
        &handle.store,
        &handle.layrs_dir,
        &handle.active.layer_id,
        &local_step_id,
    )
    .unwrap();
    assert_eq!(local_step.timeline_position, Some(0));

    let bytes = b"server newest\n".to_vec();
    let chunk_id = blake3_id(&bytes);
    let file_object_id = blake3_id(&bytes);
    let chunk_dir = handle.layrs_dir.join("objects").join("chunks");
    fs::create_dir_all(&chunk_dir).unwrap();
    fs::write(
        chunk_dir.join(format!("{}.chunk", object_file_stem(&chunk_id))),
        &bytes,
    )
    .unwrap();
    let files = vec![FileSnapshotEntry {
        path: "note.txt".to_string(),
        object: format!("objects/files/{}.json", object_file_stem(&file_object_id)),
        hash: file_object_id.clone(),
        size: bytes.len() as u64,
    }];
    let tree_id = tree_id_for_files(&files);
    let response = ReceiveLocalSpaceResponse {
        workspace_id: handle.meta.workspace_id.clone(),
        space_id: handle.meta.space_id.clone(),
        layer_id: handle.active.layer_id.clone(),
        protocol: Some(SYNC_PROTOCOL_V2.to_string()),
        root_tree_id: Some(tree_id.clone()),
        cursor: Some("cursor_2".to_string()),
        layers: vec![ReceivedLayer {
            id: handle.active.layer_id.clone(),
            workspace_id: Some(handle.meta.workspace_id.clone()),
            space_id: Some(handle.meta.space_id.clone()),
            name: "Main".to_string(),
            parent_layer_id: None,
            access: Some("open".to_string()),
        }],
        access_registries: Vec::new(),
        content_objects: Some(ReceivedContentObjects {
            chunks: vec![ReceivedChunkObject {
                chunk_id: chunk_id.clone(),
                digest: Some(chunk_id.clone()),
                download_url: None,
                size: Some(bytes.len() as u64),
                size_bytes: None,
                raw_size: None,
                stored_size: None,
                compression: None,
            }],
            file_objects: vec![ReceivedFileObject {
                file_object_id: file_object_id.clone(),
                hash: Some(file_object_id.clone()),
                size: Some(bytes.len() as u64),
                chunks: vec![ReceivedChunkRef {
                    chunk_id,
                    size: Some(bytes.len() as u64),
                    size_bytes: None,
                    raw_size: None,
                    stored_size: None,
                    compression: None,
                }],
            }],
            tree_objects: vec![ReceivedTreeObject {
                tree_id: tree_id.clone(),
                layer_id: Some(handle.active.layer_id.clone()),
                entries: vec![ReceivedTreeEntry {
                    path: "note.txt".to_string(),
                    file_object_id: Some(file_object_id),
                    size: Some(bytes.len() as u64),
                }],
            }],
        }),
        timeline: Vec::new(),
        steps: vec![ReceivedStep {
            step_id: "step_server_newest".to_string(),
            layer_id: handle.active.layer_id.clone(),
            parent_step_id: Some(local_step_id.clone()),
            base_layer_id: Some(handle.active.layer_id.clone()),
            base_tree_id: local_state.root_tree_id.clone(),
            root_tree_id: Some(tree_id.clone()),
            changed_paths: vec!["note.txt".to_string()],
            captured_at_unix: Some(local_step.captured_at_unix + 60),
            timeline_position: None,
            origin_layer_id: None,
            origin_layer_name: None,
            origin_step_id: None,
            step_kind: None,
        }],
    };

    apply_receive_response(&mut handle, response, true).unwrap();

    let received_steps =
        read_step_files(&handle.store, &handle.layrs_dir, &handle.active.layer_id).unwrap();
    let server_step = received_steps
        .iter()
        .find(|step| step.step_id == "step_server_newest")
        .expect("server step persisted");
    assert_eq!(server_step.timeline_position, Some(1));

    drop(handle);
    let scan = scan_working_tree(created.local_space.local_space_id).unwrap();
    assert_eq!(
        scan.steps.last().map(|step| step.step_id.as_str()),
        Some("step_server_newest")
    );

    let _ = fs::remove_dir_all(root);
}
