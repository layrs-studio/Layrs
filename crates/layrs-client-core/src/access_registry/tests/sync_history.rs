#[test]
fn first_child_layer_step_diffs_against_parent_layer() {
    if !isolated_legacy_test("first_child_layer_step_diffs_against_parent_layer") {
        return;
    }
    let root = unique_test_dir("child-step-parent");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("note.txt"), "parent\n").unwrap();

    let created = create_local_space(
        "space-child-step".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    let child = create_layer_from_current(
        created.local_space.local_space_id.clone(),
        "Child".to_string(),
    )
    .unwrap();
    fs::write(space.join("note.txt"), "child\n").unwrap();
    switch_layer(
        created.local_space.local_space_id.clone(),
        "main".to_string(),
    )
    .unwrap();
    switch_layer(
        created.local_space.local_space_id.clone(),
        child.active_layer_id.clone(),
    )
    .unwrap();

    let scan = scan_working_tree(created.local_space.local_space_id).unwrap();
    assert_eq!(scan.steps.len(), 1);
    assert_eq!(scan.steps[0].changed_files, 1);
    let lines = &scan.steps[0].diffs[0].diff.hunks[0].lines;
    assert!(lines
        .iter()
        .any(|line| line.op == "delete" && line.text == "parent"));
    assert!(lines
        .iter()
        .any(|line| line.op == "insert" && line.text == "child"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn delete_layer_removes_non_active_local_layer() {
    if !isolated_legacy_test("delete_layer_removes_non_active_local_layer") {
        return;
    }
    let root = unique_test_dir("delete-layer");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("note.txt"), "main\n").unwrap();

    let created = create_local_space(
        "space-delete-layer".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    let child = create_layer_from_current(
        created.local_space.local_space_id.clone(),
        "Scratch".to_string(),
    )
    .unwrap();
    switch_layer(
        created.local_space.local_space_id.clone(),
        "main".to_string(),
    )
    .unwrap();

    let result = delete_layer(
        created.local_space.local_space_id.clone(),
        child.active_layer_id.clone(),
    )
    .unwrap();

    assert_eq!(result.local_space.layers.len(), 1);
    assert_eq!(result.local_space.active_layer_id.as_deref(), Some("main"));
    let handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    assert!(handle
        .store
        .list::<Value>(
            &layer_dir(&handle.layrs_dir, &child.active_layer_id),
            ".json"
        )
        .unwrap()
        .is_empty());
    drop(handle);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn delete_linked_layer_is_queued_for_next_sync_when_studio_delete_is_unavailable() {
    if !isolated_legacy_test(
        "delete_linked_layer_is_queued_for_next_sync_when_studio_delete_is_unavailable",
    ) {
        return;
    }
    let root = unique_test_dir("delete-linked-layer-pending");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    env::remove_var("LAYRS_DESKTOP_TEST_TOKEN");
    fs::write(space.join("note.txt"), "main\n").unwrap();

    let created = create_local_space(
        "space-delete-linked-layer".to_string(),
        space.display().to_string(),
        Some("layer_main".to_string()),
    )
    .unwrap();
    let child = create_layer_from_current(
        created.local_space.local_space_id.clone(),
        "Scratch".to_string(),
    )
    .unwrap();
    switch_layer(
        created.local_space.local_space_id.clone(),
        "layer_main".to_string(),
    )
    .unwrap();

    let mut handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    let old_child_layer_id = child.active_layer_id.clone();
    let server_child_layer_id = "layer_child_to_delete".to_string();
    let old_child_dir = layer_dir(&handle.layrs_dir, &old_child_layer_id);
    let server_child_dir = layer_dir(&handle.layrs_dir, &server_child_layer_id);
    fs::rename(&old_child_dir, &server_child_dir).unwrap();
    handle.meta.state = LOCAL_SPACE_STATE_LINKED.to_string();
    handle.meta.workspace_id = "workspace_1".to_string();
    handle.meta.space_id = "space_1".to_string();
    handle.meta.server_space_id = Some("space_1".to_string());
    for layer in &mut handle.meta.layers {
        if layer.layer_id == old_child_layer_id {
            layer.layer_id = server_child_layer_id.clone();
            layer.parent_layer_id = Some("layer_main".to_string());
        }
    }
    write_json(
        &mut handle.store,
        &handle.layrs_dir.join("local-space.json"),
        &handle.meta,
    )
    .unwrap();
    drop(handle);

    let result = delete_layer(
        created.local_space.local_space_id.clone(),
        server_child_layer_id.clone(),
    )
    .unwrap();

    assert_eq!(result.local_space.layers.len(), 1);
    let handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    assert!(handle
        .store
        .list::<Value>(&server_child_dir, ".json")
        .unwrap()
        .is_empty());
    let pending = read_pending_layer_deletions(&handle.store, &handle.layrs_dir).unwrap();
    assert_eq!(pending.deleted_layers.len(), 1);
    assert_eq!(pending.deleted_layers[0].layer_id, server_child_layer_id);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn sync_weave_replays_pending_local_step_after_received_server_step() {
    if !isolated_legacy_test("sync_weave_replays_pending_local_step_after_received_server_step") {
        return;
    }
    let root = unique_test_dir("sync-weave-auto");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("note.txt"), "a\nb\nc\n").unwrap();

    let created = create_local_space(
        "space-sync-weave-auto".to_string(),
        space.display().to_string(),
        Some("layer_server".to_string()),
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
    let base_state = read_layer_index(&handle.store, &handle.layrs_dir, "layer_server").unwrap();

    fs::write(space.join("note.txt"), "a\nb\nstudio-c\n").unwrap();
    let studio_state = capture_working_state(&mut handle.store, &space, "layer_server").unwrap();
    let server_step = StepFile {
        schema: STEP_SCHEMA.to_string(),
        step_id: "server-step-1".to_string(),
        layer_id: "layer_server".to_string(),
        parent_step_id: None,
        base_layer_id: Some("layer_server".to_string()),
        base_tree_id: base_state.root_tree_id.clone(),
        root_tree_id: studio_state.root_tree_id.clone(),
        changed_paths: vec!["note.txt".to_string()],
        timeline_position: Some(0),
        origin_layer_id: Some("layer_server".to_string()),
        origin_layer_name: Some("Main".to_string()),
        origin_step_id: Some("server-step-1".to_string()),
        step_kind: Some("native".to_string()),
        captured_at_unix: unix_now(),
        files: Vec::new(),
    };
    write_json(
        &mut handle.store,
        &layer_dir(&handle.layrs_dir, "layer_server")
            .join("steps")
            .join("server-step-1.json"),
        &server_step,
    )
    .unwrap();
    fs::write(space.join("note.txt"), "a\nlocal-b\nc\n").unwrap();
    let local_state = capture_working_state(&mut handle.store, &space, "layer_server").unwrap();
    let local_step_id = write_step(
        &mut handle.store,
        &handle.layrs_dir,
        "layer_server",
        &local_state,
    )
    .unwrap();
    let local_step = read_step_file(
        &handle.store,
        &handle.layrs_dir,
        "layer_server",
        &local_step_id,
    )
    .unwrap();
    write_pending_publish(&mut handle.store, &handle.layrs_dir, &local_step).unwrap();
    let sync_path = handle.layrs_dir.join("sync-state.json");

    let pending_steps =
        pending_publish_steps(&handle.store, &handle.layrs_dir, "layer_server").unwrap();
    let result = weave_local_state_over_received_sync(
        &mut handle,
        Some(&base_state),
        &local_state,
        &studio_state,
        &pending_steps,
        &sync_path,
        "sync",
    )
    .unwrap();

    assert!(result.is_none());
    assert_eq!(
        fs::read_to_string(space.join("note.txt")).unwrap(),
        "a\nlocal-b\nstudio-c\n"
    );
    assert_eq!(
        read_pending_publish_files(&handle.store, &handle.layrs_dir, "layer_server")
            .unwrap()
            .len(),
        1
    );
    let steps = sorted_steps(&handle.store, &handle.layrs_dir, "layer_server").unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].step_id, "server-step-1");
    assert_eq!(steps[0].step_kind.as_deref(), Some("native"));
    assert_eq!(steps[1].step_kind.as_deref(), Some("woven"));
    assert_eq!(
        steps[1].origin_step_id.as_deref(),
        Some(local_step_id.as_str())
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn sync_weave_conflict_is_abortable_without_losing_local_step() {
    if !isolated_legacy_test("sync_weave_conflict_is_abortable_without_losing_local_step") {
        return;
    }
    let root = unique_test_dir("sync-weave-conflict");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("note.txt"), "base\n").unwrap();

    let created = create_local_space(
        "space-sync-weave-conflict".to_string(),
        space.display().to_string(),
        Some("layer_server".to_string()),
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
    let base_state = read_layer_index(&handle.store, &handle.layrs_dir, "layer_server").unwrap();

    fs::write(space.join("note.txt"), "studio\n").unwrap();
    let studio_state = capture_working_state(&mut handle.store, &space, "layer_server").unwrap();
    fs::write(space.join("note.txt"), "local\n").unwrap();
    let local_state = capture_working_state(&mut handle.store, &space, "layer_server").unwrap();
    let local_step_id = write_step(
        &mut handle.store,
        &handle.layrs_dir,
        "layer_server",
        &local_state,
    )
    .unwrap();
    let local_step = read_step_file(
        &handle.store,
        &handle.layrs_dir,
        "layer_server",
        &local_step_id,
    )
    .unwrap();
    write_pending_publish(&mut handle.store, &handle.layrs_dir, &local_step).unwrap();
    let sync_path = handle.layrs_dir.join("sync-state.json");

    let pending_steps =
        pending_publish_steps(&handle.store, &handle.layrs_dir, "layer_server").unwrap();
    let result = weave_local_state_over_received_sync(
        &mut handle,
        Some(&base_state),
        &local_state,
        &studio_state,
        &pending_steps,
        &sync_path,
        "sync",
    )
    .unwrap()
    .expect("conflicted sync");

    assert_eq!(result.status, "conflicted");
    let marked = fs::read_to_string(space.join("note.txt")).unwrap();
    assert!(marked.contains("<<<<<<< target:layer_server"));
    assert!(marked.contains(">>>>>>> source:local-sync:layer_server"));

    drop(handle);
    abort_weave(created.local_space.local_space_id).unwrap();
    assert_eq!(
        fs::read_to_string(space.join("note.txt")).unwrap(),
        "local\n"
    );

    let _ = fs::remove_dir_all(root);
}
