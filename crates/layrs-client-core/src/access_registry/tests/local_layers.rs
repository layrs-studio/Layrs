fn create_local_space(
    space_id: String,
    target_folder: String,
    initial_layer_id: Option<String>,
) -> Result<CreateLocalSpaceResult, String> {
    super::create_local_space_internal(space_id, target_folder, initial_layer_id, false)
}

#[test]
fn switch_layer_restores_main_after_round_trip() {
    if !isolated_legacy_test("switch_layer_restores_main_after_round_trip") {
        return;
    }
    let root = unique_test_dir("round-trip");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("note.txt"), "main").unwrap();

    let created = create_local_space(
        "space-a".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();

    fs::write(space.join("note.txt"), "feature").unwrap();
    let feature = create_layer_from_current(
        created.local_space.local_space_id.clone(),
        "Feature".to_string(),
    )
    .unwrap();

    switch_layer(
        created.local_space.local_space_id.clone(),
        "main".to_string(),
    )
    .unwrap();
    assert_eq!(fs::read_to_string(space.join("note.txt")).unwrap(), "main");

    switch_layer(created.local_space.local_space_id, feature.active_layer_id).unwrap();
    assert_eq!(
        fs::read_to_string(space.join("note.txt")).unwrap(),
        "feature"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn draft_local_space_creates_open_main_layer_offline() {
    if !isolated_legacy_test("draft_local_space_creates_open_main_layer_offline") {
        return;
    }
    let root = unique_test_dir("draft");
    let config = root.join("config");
    let space = root.join("draft-space");
    env::set_var("APPDATA", &config);

    let created =
        create_draft_local_space("Local Prototype".to_string(), space.display().to_string())
            .unwrap();

    assert_eq!(created.local_space.state, LOCAL_SPACE_STATE_DRAFT);
    assert_eq!(created.local_space.name, "Local Prototype");
    assert_eq!(created.local_space.layers.len(), 1);
    assert_eq!(created.local_space.layers[0].display_name, "Main");
    assert!(space.join(".layrs/metadata/layrs.sqlite3").exists());
    assert!(!space.join(".layrs/local-space.json").exists());
    let handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    assert!(handle
        .store
        .exists(&handle.layrs_dir.join("local-space.json"))
        .unwrap());
    assert!(handle
        .store
        .exists(&layer_dir(&handle.layrs_dir, "local_layer_main").join("working-state.json"))
        .unwrap());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn forget_local_space_archives_layrs_and_keeps_project_files() {
    if !isolated_legacy_test("forget_local_space_archives_layrs_and_keeps_project_files") {
        return;
    }
    let root = unique_test_dir("forget-local");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("note.txt"), "keep me").unwrap();

    let created = create_local_space(
        "space-forget".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    assert_eq!(list_local_spaces().unwrap().len(), 1);

    let forgotten = forget_local_space(created.local_space.local_space_id).unwrap();

    assert_eq!(
        path_compare_key(&PathBuf::from(&forgotten.root_path)),
        path_compare_key(&space)
    );
    assert!(!space.join(LAYRS_DIR).exists());
    assert!(PathBuf::from(forgotten.archived_layrs_path.unwrap()).exists());
    assert_eq!(
        fs::read_to_string(space.join("note.txt")).unwrap(),
        "keep me"
    );
    assert!(list_local_spaces().unwrap().is_empty());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn forget_local_space_disconnects_when_layrs_metadata_is_missing() {
    if !isolated_legacy_test("forget_local_space_disconnects_when_layrs_metadata_is_missing") {
        return;
    }
    let root = unique_test_dir("forget-missing");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("note.txt"), "still here").unwrap();

    let created = create_local_space(
        "space-forget-missing".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    fs::remove_dir_all(space.join(LAYRS_DIR)).unwrap();

    let forgotten = forget_local_space(created.local_space.local_space_id).unwrap();

    assert!(forgotten.archived_layrs_path.is_none());
    assert_eq!(
        fs::read_to_string(space.join("note.txt")).unwrap(),
        "still here"
    );
    assert!(list_local_spaces().unwrap().is_empty());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn scan_added_text_file_returns_lens_diff_entry() {
    if !isolated_legacy_test("scan_added_text_file_returns_lens_diff_entry") {
        return;
    }
    let root = unique_test_dir("added-diff");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);

    let created = create_local_space(
        "space-diff".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    fs::write(space.join("note.txt"), "hello from desktop\n").unwrap();

    let scan = scan_working_tree(created.local_space.local_space_id).unwrap();

    assert_eq!(scan.added, vec!["note.txt".to_string()]);
    assert_eq!(scan.diffs.len(), 1);
    assert_eq!(scan.diffs[0].lens_id, "layrs.text");
    assert_eq!(scan.diffs[0].diff.kind, "textLines");
    assert!(scan.diffs[0].diff.hunks[0]
        .lines
        .iter()
        .any(|line| line.op == "insert" && line.text == "hello from desktop"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn scan_modified_text_file_returns_unified_lines() {
    if !isolated_legacy_test("scan_modified_text_file_returns_unified_lines") {
        return;
    }
    let root = unique_test_dir("modified-diff");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("note.txt"), "alpha\nbeta\nomega\n").unwrap();

    let created = create_local_space(
        "space-modified-diff".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    fs::write(space.join("note.txt"), "alpha\nbravo\nomega\n").unwrap();

    let scan = scan_working_tree(created.local_space.local_space_id).unwrap();
    let lines = &scan.diffs[0].diff.hunks[0].lines;

    assert_eq!(scan.modified, vec!["note.txt".to_string()]);
    assert_eq!(lines[0].op, "equal");
    assert_eq!(lines[0].old_line, Some(1));
    assert_eq!(lines[0].new_line, Some(1));
    assert_eq!(lines[1].op, "delete");
    assert_eq!(lines[1].old_line, Some(2));
    assert_eq!(lines[2].op, "insert");
    assert_eq!(lines[2].new_line, Some(2));
    assert_eq!(lines[3].op, "equal");
    assert_eq!(lines[3].old_line, Some(3));
    assert_eq!(lines[3].new_line, Some(3));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn scan_modified_large_text_file_preserves_long_diff_lines() {
    if !isolated_legacy_test("scan_modified_large_text_file_preserves_long_diff_lines") {
        return;
    }
    let root = unique_test_dir("modified-large-diff");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("test.txt"), "tiny\n").unwrap();

    let created = create_local_space(
        "space-large-modified-diff".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    fs::write(space.join("test.txt"), "a".repeat(4 * 1024 * 1024)).unwrap();

    let scan = scan_working_tree(created.local_space.local_space_id).unwrap();
    let diff = &scan.diffs[0];
    let lines = &diff.diff.hunks[0].lines;

    assert_eq!(scan.modified, vec!["test.txt".to_string()]);
    assert_ne!(diff.diff.summary, "No text changes");
    assert!(diff.diff.summary.contains("1 additions"));
    assert!(diff.message.is_none());
    assert!(lines.iter().any(|line| line.op == "insert"));
    let inserted = lines.iter().find(|line| line.op == "insert").unwrap();
    assert_eq!(inserted.text.chars().count(), 4 * 1024 * 1024);
    assert!(!inserted.text.contains("Layrs line truncated"));
    assert_eq!(
        diff.diff.fields.get("newTruncated"),
        Some(&Value::Bool(false))
    );
    assert!(!diff.diff.fields.contains_key("lineTextTruncated"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn scan_large_text_change_after_preview_still_reports_change() {
    if !isolated_legacy_test("scan_large_text_change_after_preview_still_reports_change") {
        return;
    }
    let root = unique_test_dir("large-diff-after-preview");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    let prefix = "a".repeat(512 * 1024);
    fs::write(space.join("test.txt"), format!("{prefix}x")).unwrap();

    let created = create_local_space(
        "space-large-tail-diff".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    fs::write(space.join("test.txt"), format!("{prefix}y")).unwrap();

    let scan = scan_working_tree(created.local_space.local_space_id).unwrap();
    let diff = &scan.diffs[0].diff;

    assert_eq!(scan.modified, vec!["test.txt".to_string()]);
    assert_ne!(diff.summary, "No text changes");
    assert!(diff.hunks[0].lines.iter().any(|line| line.op == "delete"));
    assert!(diff.hunks[0].lines.iter().any(|line| line.op == "insert"));
    assert!(!diff.fields.contains_key("lineTextTruncated"));
    assert!(diff.hunks[0]
        .lines
        .iter()
        .filter(|line| line.op == "delete" || line.op == "insert")
        .all(|line| !line.text.contains("Layrs line truncated")));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn scan_large_added_text_file_returns_window_metadata() {
    if !isolated_legacy_test("scan_large_added_text_file_returns_window_metadata") {
        return;
    }
    let root = unique_test_dir("large-added-window");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);

    let created = create_local_space(
        "space-large-added-window".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    fs::write(space.join("large.txt"), numbered_lines("line", 20_000)).unwrap();

    let scan = scan_working_tree(created.local_space.local_space_id).unwrap();
    let diff = &scan.diffs[0].diff;

    assert_eq!(scan.added, vec!["large.txt".to_string()]);
    assert_eq!(diff.hunks[0].lines.len(), TEXT_DIFF_DEFAULT_WINDOW_LIMIT);
    assert_eq!(
        diff.fields.get("totalNewLines").and_then(Value::as_u64),
        Some(20_001)
    );
    assert_eq!(
        diff.fields.get("windowStart").and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        diff.fields.get("hasMore").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        diff.fields.get("largeDiff").and_then(Value::as_bool),
        Some(true)
    );
    assert!(diff.summary.contains("20001 additions"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn scan_large_modified_text_file_returns_window_metadata() {
    if !isolated_legacy_test("scan_large_modified_text_file_returns_window_metadata") {
        return;
    }
    let root = unique_test_dir("large-modified-window");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("large.txt"), numbered_lines("old", 50_000)).unwrap();

    let created = create_local_space(
        "space-large-modified-window".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    fs::write(space.join("large.txt"), numbered_lines("new", 50_000)).unwrap();

    let scan = scan_working_tree(created.local_space.local_space_id).unwrap();
    let diff = &scan.diffs[0].diff;

    assert_eq!(scan.modified, vec!["large.txt".to_string()]);
    assert_eq!(diff.hunks[0].lines.len(), TEXT_DIFF_DEFAULT_WINDOW_LIMIT);
    assert_eq!(
        diff.fields.get("totalOldLines").and_then(Value::as_u64),
        Some(50_001)
    );
    assert_eq!(
        diff.fields.get("totalNewLines").and_then(Value::as_u64),
        Some(50_001)
    );
    assert_eq!(
        diff.fields.get("hasMore").and_then(Value::as_bool),
        Some(true)
    );
    assert_ne!(diff.summary, "No text changes");
    assert!(diff.summary.contains("50000 additions"));
    assert!(diff.summary.contains("50000 deletions"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn load_diff_window_returns_requested_window() {
    if !isolated_legacy_test("load_diff_window_returns_requested_window") {
        return;
    }
    let root = unique_test_dir("load-window");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("large.txt"), numbered_lines("old", 20_000)).unwrap();

    let created = create_local_space(
        "space-load-window".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    fs::write(space.join("large.txt"), numbered_lines("new", 20_000)).unwrap();

    let diff = load_diff_window(
        created.local_space.local_space_id,
        "large.txt".to_string(),
        Some("workingTree".to_string()),
        500,
        25,
    )
    .unwrap();

    assert_eq!(diff.path, "large.txt");
    assert_eq!(diff.state, "modified");
    assert_eq!(diff.diff.hunks[0].lines.len(), 25);
    assert_eq!(
        diff.diff.fields.get("windowStart").and_then(Value::as_u64),
        Some(500)
    );
    assert_eq!(
        diff.diff.fields.get("windowLimit").and_then(Value::as_u64),
        Some(25)
    );
    assert_eq!(
        diff.diff.fields.get("preview").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        diff.diff.fields.get("source").and_then(Value::as_str),
        Some("workingTree")
    );
    assert!(diff.diff.hunks[0]
        .lines
        .iter()
        .all(|line| line.op == "delete"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn scan_includes_local_step_summaries() {
    if !isolated_legacy_test("scan_includes_local_step_summaries") {
        return;
    }
    let root = unique_test_dir("step-summary");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("note.txt"), "base\n").unwrap();

    let created = create_local_space(
        "space-step-summary".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    fs::write(space.join("note.txt"), "step\n").unwrap();
    let mut handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    let state = capture_working_state(&mut handle.store, &space, "main").unwrap();
    write_step(&mut handle.store, &handle.layrs_dir, "main", &state).unwrap();
    drop(handle);

    let scan = scan_working_tree(created.local_space.local_space_id).unwrap();

    assert_eq!(scan.steps.len(), 1);
    assert_eq!(scan.steps[0].layer_id, "main");
    assert_eq!(scan.steps[0].changed_files, 1);
    assert_eq!(scan.steps[0].diff_stats.files, 1);
    assert_eq!(scan.steps[0].diffs[0].path, "note.txt");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn save_local_step_writes_step_and_pending_publish() {
    if !isolated_legacy_test("save_local_step_writes_step_and_pending_publish") {
        return;
    }
    let root = unique_test_dir("save-step-pending");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("note.txt"), "base\n").unwrap();

    let created = create_local_space(
        "space-save-step".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    fs::write(space.join("note.txt"), "changed\n").unwrap();

    let saved = save_local_step(created.local_space.local_space_id.clone()).unwrap();
    let scan = scan_working_tree(created.local_space.local_space_id.clone()).unwrap();

    assert_eq!(saved.status, "saved");
    assert_eq!(saved.changed_files, 1);
    assert_eq!(saved.pending_publish_count, 1);
    assert_eq!(scan.pending_publish_count, 1);
    assert_eq!(scan.steps.len(), 1);
    let handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    assert!(handle
        .store
        .exists(
            &pending_publish_dir(&handle.layrs_dir, "main")
                .join(format!("{}.json", saved.step_id.unwrap()))
        )
        .unwrap());
    drop(handle);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn init_local_space_records_existing_files_as_pending_step() {
    if !isolated_legacy_test("init_local_space_records_existing_files_as_pending_step") {
        return;
    }
    let root = unique_test_dir("init-existing");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("note.txt"), "hello\n").unwrap();

    let initialized =
        init_local_space("Initialized".to_string(), space.display().to_string()).unwrap();
    let scan = scan_working_tree(initialized.local_space.local_space_id.clone()).unwrap();

    assert_eq!(initialized.scanned_files, 1);
    assert!(initialized.initial_step_id.is_some());
    assert_eq!(initialized.pending_publish_count, 1);
    assert_eq!(scan.pending_publish_count, 1);
    assert_eq!(
        scan.files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        vec!["note.txt"]
    );
    assert_eq!(scan.steps.len(), 1);
    assert!(!space.join(LAYRS_DIR).join("active-layer.json").exists());
    assert!(!space.join(LAYRS_DIR).join("access.json").exists());
    let handle = open_local_space_handle(&initialized.local_space.local_space_id).unwrap();
    assert!(handle
        .store
        .exists(&handle.layrs_dir.join("active-layer.json"))
        .unwrap());
    assert!(handle
        .store
        .exists(&handle.layrs_dir.join("access.json"))
        .unwrap());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn init_local_space_without_files_stays_clean() {
    if !isolated_legacy_test("init_local_space_without_files_stays_clean") {
        return;
    }
    let root = unique_test_dir("init-empty");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);

    let initialized = init_local_space("Empty".to_string(), space.display().to_string()).unwrap();
    let scan = scan_working_tree(initialized.local_space.local_space_id.clone()).unwrap();

    assert_eq!(initialized.scanned_files, 0);
    assert!(initialized.initial_step_id.is_none());
    assert_eq!(initialized.pending_publish_count, 0);
    assert!(!scan.changed);
    assert!(scan.steps.is_empty());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn scan_detects_same_size_edit_even_after_cache_is_warm() {
    if !isolated_legacy_test("scan_detects_same_size_edit_even_after_cache_is_warm") {
        return;
    }
    let root = unique_test_dir("same-size-cache");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("note.txt"), "alpha\n").unwrap();

    let created = create_local_space(
        "space-same-size-cache".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    let clean = scan_working_tree(created.local_space.local_space_id.clone()).unwrap();
    assert!(!clean.changed);

    fs::write(space.join("note.txt"), "bravo\n").unwrap();
    let scan = scan_working_tree(created.local_space.local_space_id).unwrap();

    assert_eq!(scan.modified, vec!["note.txt".to_string()]);
    assert_eq!(scan.diffs[0].path, "note.txt");
    assert!(scan.diffs[0].diff.hunks[0]
        .lines
        .iter()
        .any(|line| line.op == "insert" && line.text == "bravo"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn switch_layer_missing_target_object_keeps_active_layer_and_files() {
    if !isolated_legacy_test("switch_layer_missing_target_object_keeps_active_layer_and_files") {
        return;
    }
    let root = unique_test_dir("switch-missing-object");
    let config = root.join("config");
    let space = root.join("space");
    fs::create_dir_all(&space).unwrap();
    env::set_var("APPDATA", &config);
    fs::write(space.join("note.txt"), "main safe\n").unwrap();

    let created = create_local_space(
        "space-switch-missing-object".to_string(),
        space.display().to_string(),
        Some("main".to_string()),
    )
    .unwrap();
    let feature =
        create_layer_from_current(created.local_space.local_space_id.clone(), "Feature".into())
            .unwrap();
    fs::write(space.join("note.txt"), "feature target\n").unwrap();
    save_local_step(created.local_space.local_space_id.clone()).unwrap();

    switch_layer(
        created.local_space.local_space_id.clone(),
        "main".to_string(),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(space.join("note.txt")).unwrap(),
        "main safe\n"
    );

    let layrs_dir = space.join(LAYRS_DIR);
    let handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    let target_state =
        read_layer_state(&handle.store, &layrs_dir, &feature.active_layer_id).unwrap();
    let target_file = target_state
        .files
        .iter()
        .find(|file| file.path == "note.txt")
        .unwrap();
    let manifest =
        read_object_json::<FileObjectFile>(&layrs_dir.join(&target_file.object)).unwrap();
    let chunk_id = &manifest.chunks[0].chunk_id;
    let chunk_path = layrs_dir
        .join("objects")
        .join("chunks")
        .join(format!("{}.chunk", object_file_stem(chunk_id)));
    fs::remove_file(&chunk_path).unwrap();
    drop(handle);

    let error = switch_layer(
        created.local_space.local_space_id.clone(),
        feature.active_layer_id,
    )
    .unwrap_err();
    assert!(error.contains("could not find chunk object"));
    assert_eq!(
        fs::read_to_string(space.join("note.txt")).unwrap(),
        "main safe\n"
    );

    let handle = open_local_space_handle(&created.local_space.local_space_id).unwrap();
    let active =
        read_json::<ActiveLayerFile>(&handle.store, &layrs_dir.join("active-layer.json")).unwrap();
    assert_eq!(active.layer_id, "main");

    let _ = fs::remove_dir_all(root);
}
