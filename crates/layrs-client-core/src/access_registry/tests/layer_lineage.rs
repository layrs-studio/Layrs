#[test]
fn auto_local_steps_disabled_preserves_layer_work_without_step() {
    if !isolated_legacy_test("auto_local_steps_disabled_preserves_layer_work_without_step") {
        return;
    }
    let root = unique_test_dir("auto-step-disabled");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    let mut desktop_config = DesktopConfig::load_or_create().unwrap();
    desktop_config.auto_local_steps = false;
    desktop_config.save().unwrap();
    fs::write(space.join("note.txt"), "main\n").unwrap();

    let created = create_local_space(
        "space-auto-step-disabled".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    let feature =
        create_layer_from_current(created.local_space.local_space_id.clone(), "Feature".into())
            .unwrap();
    fs::write(space.join("note.txt"), "feature draft\n").unwrap();
    fs::write(space.join("draft.txt"), "unstepped\n").unwrap();

    let switched_to_main = switch_layer(
        created.local_space.local_space_id.clone(),
        "main".to_string(),
    )
    .unwrap();
    assert_eq!(switched_to_main.saved_step_id, None);
    assert_eq!(switched_to_main.changed_files, 2);
    assert_eq!(
        fs::read_to_string(space.join("note.txt")).unwrap(),
        "main\n"
    );
    assert!(!space.join("draft.txt").exists());
    let handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    assert!(
        read_step_files(&handle.store, &handle.layrs_dir, &feature.active_layer_id)
            .unwrap()
            .is_empty()
    );
    drop(handle);

    switch_layer(
        created.local_space.local_space_id,
        feature.active_layer_id.clone(),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(space.join("note.txt")).unwrap(),
        "feature draft\n"
    );
    assert_eq!(
        fs::read_to_string(space.join("draft.txt")).unwrap(),
        "unstepped\n"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn pending_publish_steps_are_separate_per_layer() {
    if !isolated_legacy_test("pending_publish_steps_are_separate_per_layer") {
        return;
    }
    let root = unique_test_dir("pending-per-layer");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("main.txt"), "main v1\n").unwrap();

    let created = create_local_space(
        "space-pending-per-layer".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    fs::write(space.join("main.txt"), "main v2\n").unwrap();
    save_local_step(created.local_space.local_space_id.clone()).unwrap();

    let feature =
        create_layer_from_current(created.local_space.local_space_id.clone(), "Feature".into())
            .unwrap();
    fs::write(space.join("feature.txt"), "feature\n").unwrap();
    save_local_step(created.local_space.local_space_id.clone()).unwrap();

    let layrs_dir = space.join(LAYRS_DIR);
    let handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    assert_eq!(
        read_pending_publish_files(&handle.store, &layrs_dir, "main")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        read_pending_publish_files(&handle.store, &layrs_dir, &feature.active_layer_id)
            .unwrap()
            .len(),
        1
    );
    drop(handle);

    switch_layer(
        created.local_space.local_space_id.clone(),
        "main".to_string(),
    )
    .unwrap();
    let main_scan = scan_working_tree(created.local_space.local_space_id).unwrap();
    assert_eq!(main_scan.pending_publish_count, 1);
    assert!(main_scan.steps.iter().all(|step| step.layer_id == "main"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn child_layer_inherits_parent_steps_without_copying_chunks() {
    if !isolated_legacy_test("child_layer_inherits_parent_steps_without_copying_chunks") {
        return;
    }
    let root = unique_test_dir("inherit-parent-steps");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);

    let created = create_local_space(
        "space-inherit-parent-steps".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    fs::write(space.join("main-1.txt"), "main 1\n").unwrap();
    let main_1 = save_local_step(created.local_space.local_space_id.clone())
        .unwrap()
        .step_id
        .unwrap();
    fs::write(space.join("main-2.txt"), "main 2\n").unwrap();
    let main_2 = save_local_step(created.local_space.local_space_id.clone())
        .unwrap()
        .step_id
        .unwrap();

    let layrs_dir = space.join(LAYRS_DIR);
    let chunk_count_before =
        directory_file_count(&layrs_dir.join("objects").join("chunks"), Some("chunk"));
    let feature =
        create_layer_from_current(created.local_space.local_space_id.clone(), "Feature".into())
            .unwrap();
    let chunk_count_after =
        directory_file_count(&layrs_dir.join("objects").join("chunks"), Some("chunk"));
    assert_eq!(chunk_count_after, chunk_count_before);

    let handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    let mut feature_steps =
        read_step_files(&handle.store, &layrs_dir, &feature.active_layer_id).unwrap();
    feature_steps.sort_by(compare_steps_by_timeline);
    assert_eq!(feature_steps.len(), 2);
    assert_eq!(feature_steps[0].step_kind.as_deref(), Some("inherited"));
    assert_eq!(feature_steps[0].origin_layer_id.as_deref(), Some("main"));
    assert_eq!(feature_steps[0].origin_layer_name.as_deref(), Some("Main"));
    assert_eq!(
        feature_steps[0].origin_step_id.as_deref(),
        Some(main_1.as_str())
    );
    assert_eq!(feature_steps[1].step_kind.as_deref(), Some("inherited"));
    assert_eq!(feature_steps[1].origin_layer_id.as_deref(), Some("main"));
    assert_eq!(feature_steps[1].origin_layer_name.as_deref(), Some("Main"));
    assert_eq!(
        feature_steps[1].origin_step_id.as_deref(),
        Some(main_2.as_str())
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn linked_child_receives_future_parent_steps_after_own_work() {
    if !isolated_legacy_test("linked_child_receives_future_parent_steps_after_own_work") {
        return;
    }
    let root = unique_test_dir("linked-child-propagation");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);

    let created = create_local_space(
        "space-linked-child-propagation".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    fs::write(space.join("main-1.txt"), "main 1\n").unwrap();
    let main_1 = save_local_step(created.local_space.local_space_id.clone())
        .unwrap()
        .step_id
        .unwrap();
    fs::write(space.join("main-2.txt"), "main 2\n").unwrap();
    let main_2 = save_local_step(created.local_space.local_space_id.clone())
        .unwrap()
        .step_id
        .unwrap();

    let feature =
        create_layer_from_current(created.local_space.local_space_id.clone(), "Feature".into())
            .unwrap();
    fs::write(space.join("feature.txt"), "feature B\n").unwrap();
    let feature_b = save_local_step(created.local_space.local_space_id.clone())
        .unwrap()
        .step_id
        .unwrap();

    switch_layer(
        created.local_space.local_space_id.clone(),
        "main".to_string(),
    )
    .unwrap();
    fs::write(space.join("main-3.txt"), "main 3\n").unwrap();
    let main_3 = save_local_step(created.local_space.local_space_id.clone())
        .unwrap()
        .step_id
        .unwrap();

    let layrs_dir = space.join(LAYRS_DIR);
    let handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    let mut feature_steps =
        read_step_files(&handle.store, &layrs_dir, &feature.active_layer_id).unwrap();
    feature_steps.sort_by(compare_steps_by_timeline);
    let provenance = feature_steps
        .iter()
        .map(|step| {
            (
                step.origin_layer_id
                    .as_deref()
                    .unwrap_or(step.layer_id.as_str()),
                step.origin_layer_name.as_deref().unwrap_or("<missing>"),
                step.origin_step_id
                    .as_deref()
                    .unwrap_or(step.step_id.as_str()),
                step.step_kind.as_deref().unwrap_or("native"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        provenance,
        vec![
            ("main", "Main", main_1.as_str(), "inherited"),
            ("main", "Main", main_2.as_str(), "inherited"),
            (
                feature.active_layer_id.as_str(),
                "Feature",
                feature_b.as_str(),
                "native",
            ),
            ("main", "Main", main_3.as_str(), "inherited"),
        ]
    );
    drop(handle);

    switch_layer(
        created.local_space.local_space_id,
        feature.active_layer_id.clone(),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(space.join("main-3.txt")).unwrap(),
        "main 3\n"
    );
    assert_eq!(
        fs::read_to_string(space.join("feature.txt")).unwrap(),
        "feature B\n"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn disconnected_child_does_not_receive_future_parent_steps() {
    if !isolated_legacy_test("disconnected_child_does_not_receive_future_parent_steps") {
        return;
    }
    let root = unique_test_dir("disconnected-child-propagation");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);

    let created = create_local_space(
        "space-disconnected-child-propagation".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    fs::write(space.join("main-1.txt"), "main 1\n").unwrap();
    save_local_step(created.local_space.local_space_id.clone()).unwrap();
    let feature =
        create_layer_from_current(created.local_space.local_space_id.clone(), "Feature".into())
            .unwrap();
    disconnect_layer_from_parent(
        created.local_space.local_space_id.clone(),
        feature.active_layer_id.clone(),
    )
    .unwrap();

    switch_layer(
        created.local_space.local_space_id.clone(),
        "main".to_string(),
    )
    .unwrap();
    fs::write(space.join("main-2.txt"), "main 2\n").unwrap();
    let main_2 = save_local_step(created.local_space.local_space_id.clone())
        .unwrap()
        .step_id
        .unwrap();

    let layrs_dir = space.join(LAYRS_DIR);
    let handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    let feature_steps =
        read_step_files(&handle.store, &layrs_dir, &feature.active_layer_id).unwrap();
    assert!(
        feature_steps
            .iter()
            .all(|step| step.origin_step_id.as_deref() != Some(main_2.as_str())),
        "disconnected child should not receive future parent step"
    );
    drop(handle);
    let summary = open_local_space(created.local_space.local_space_id).unwrap();
    let feature_summary = summary
        .layers
        .iter()
        .find(|layer| layer.layer_id == feature.active_layer_id)
        .unwrap();
    assert_eq!(feature_summary.lineage_status, "unlinked");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn clear_layer_steps_archives_history_without_touching_files_or_chunks() {
    if !isolated_legacy_test("clear_layer_steps_archives_history_without_touching_files_or_chunks")
    {
        return;
    }
    let root = unique_test_dir("clear-layer-steps");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);

    let created = create_local_space(
        "space-clear-layer-steps".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    fs::write(space.join("keep.txt"), "important data\n").unwrap();
    save_local_step(created.local_space.local_space_id.clone()).unwrap();

    let layrs_dir = space.join(LAYRS_DIR);
    let chunk_count_before =
        directory_file_count(&layrs_dir.join("objects").join("chunks"), Some("chunk"));
    let cleared = clear_layer_steps(
        created.local_space.local_space_id.clone(),
        "main".to_string(),
        true,
    )
    .unwrap();

    assert_eq!(
        fs::read_to_string(space.join("keep.txt")).unwrap(),
        "important data\n"
    );
    let handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    assert_eq!(
        read_step_files(&handle.store, &layrs_dir, "main")
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        read_pending_publish_files(&handle.store, &layrs_dir, "main")
            .unwrap()
            .len(),
        0
    );
    let archived_steps =
        PathBuf::from(cleared.archived_steps_path.as_deref().unwrap()).join("steps");
    assert_eq!(
        handle
            .store
            .list::<StepFile>(&archived_steps, ".json")
            .unwrap()
            .len(),
        1
    );
    let chunk_count_after =
        directory_file_count(&layrs_dir.join("objects").join("chunks"), Some("chunk"));
    assert_eq!(chunk_count_after, chunk_count_before);
    drop(handle);

    let scan = scan_working_tree(created.local_space.local_space_id).unwrap();
    assert_eq!(scan.steps.len(), 0);
    assert!(!scan.changed);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn save_desktop_settings_rejects_empty_and_duplicate_shortcuts() {
    if !isolated_legacy_test("save_desktop_settings_rejects_empty_and_duplicate_shortcuts") {
        return;
    }
    let root = unique_test_dir("settings-shortcuts");
    let config = root.join("config");
    env::set_var("APPDATA", &config);
    let settings = DesktopConfig::load_or_create().unwrap().settings();

    let mut empty = settings.clone();
    empty.shortcuts.enabled = true;
    empty.shortcuts.save_step = String::new();
    let empty_error = save_desktop_settings(empty).unwrap_err();
    assert!(empty_error.contains("Shortcut fields cannot be empty"));

    let mut duplicate = settings;
    duplicate.shortcuts.enabled = true;
    duplicate.shortcuts.save_step = "Ctrl+S".to_string();
    duplicate.shortcuts.publish = "ctrl+s".to_string();
    let duplicate_error = save_desktop_settings(duplicate).unwrap_err();
    assert!(duplicate_error.contains("must be different"));

    let _ = fs::remove_dir_all(root);
}
