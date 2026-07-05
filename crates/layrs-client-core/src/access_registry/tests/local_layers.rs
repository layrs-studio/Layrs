    fn create_local_space(
        space_id: String,
        target_folder: String,
        initial_layer_id: Option<String>,
    ) -> Result<CreateLocalSpaceResult, String> {
        super::create_local_space_internal(space_id, target_folder, initial_layer_id, false)
    }

    #[test]
    fn switch_layer_restores_main_after_round_trip() {
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
        assert!(space.join(".layrs").join("local-space.json").exists());
        assert!(space
            .join(".layrs")
            .join("layers")
            .join("local_layer_main")
            .join("working-state.json")
            .exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn forget_local_space_archives_layrs_and_keeps_project_files() {
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
        assert!(diff.diff.fields.get("lineTextTruncated").is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_large_text_change_after_preview_still_reports_change() {
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
        assert!(diff.fields.get("lineTextTruncated").is_none());
        assert!(diff.hunks[0]
            .lines
            .iter()
            .filter(|line| line.op == "delete" || line.op == "insert")
            .all(|line| !line.text.contains("Layrs line truncated")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_large_added_text_file_returns_window_metadata() {
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
        let state = capture_working_state(&space, "main", true).unwrap();
        write_step(&space.join(LAYRS_DIR), "main", &state).unwrap();

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
        let scan = scan_working_tree(created.local_space.local_space_id).unwrap();

        assert_eq!(saved.status, "saved");
        assert_eq!(saved.changed_files, 1);
        assert_eq!(saved.pending_publish_count, 1);
        assert_eq!(scan.pending_publish_count, 1);
        assert_eq!(scan.steps.len(), 1);
        assert!(space
            .join(LAYRS_DIR)
            .join("layers")
            .join("main")
            .join("pending-publish")
            .join(format!("{}.json", saved.step_id.unwrap()))
            .exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn init_local_space_records_existing_files_as_pending_step() {
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
        assert!(space.join(LAYRS_DIR).join("active-layer.json").exists());
        assert!(space.join(LAYRS_DIR).join("access.json").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn init_local_space_without_files_stays_clean() {
        let root = unique_test_dir("init-empty");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);

        let initialized =
            init_local_space("Empty".to_string(), space.display().to_string()).unwrap();
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

        switch_layer(created.local_space.local_space_id.clone(), "main".to_string()).unwrap();
        assert_eq!(fs::read_to_string(space.join("note.txt")).unwrap(), "main safe\n");

        let layrs_dir = space.join(LAYRS_DIR);
        let target_state = read_layer_state(&layrs_dir, &feature.active_layer_id).unwrap();
        let target_file = target_state
            .files
            .iter()
            .find(|file| file.path == "note.txt")
            .unwrap();
        let manifest = read_json::<FileObjectFile>(&layrs_dir.join(&target_file.object)).unwrap();
        let chunk_id = &manifest.chunks[0].chunk_id;
        let chunk_path = layrs_dir
            .join("objects")
            .join("chunks")
            .join(format!("{}.chunk", object_file_stem(chunk_id)));
        fs::remove_file(&chunk_path).unwrap();

        let error = switch_layer(
            created.local_space.local_space_id.clone(),
            feature.active_layer_id,
        )
        .unwrap_err();
        assert!(error.contains("could not find chunk object"));
        assert_eq!(fs::read_to_string(space.join("note.txt")).unwrap(), "main safe\n");

        let active = read_json::<ActiveLayerFile>(&layrs_dir.join("active-layer.json")).unwrap();
        assert_eq!(active.layer_id, "main");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn auto_local_steps_disabled_preserves_layer_work_without_step() {
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

        let switched_to_main =
            switch_layer(created.local_space.local_space_id.clone(), "main".to_string()).unwrap();
        assert_eq!(switched_to_main.saved_step_id, None);
        assert_eq!(switched_to_main.changed_files, 2);
        assert_eq!(fs::read_to_string(space.join("note.txt")).unwrap(), "main\n");
        assert!(!space.join("draft.txt").exists());
        assert!(read_step_files(&space.join(LAYRS_DIR), &feature.active_layer_id)
            .unwrap()
            .is_empty());

        switch_layer(
            created.local_space.local_space_id,
            feature.active_layer_id.clone(),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(space.join("note.txt")).unwrap(),
            "feature draft\n"
        );
        assert_eq!(fs::read_to_string(space.join("draft.txt")).unwrap(), "unstepped\n");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pending_publish_steps_are_separate_per_layer() {
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
        assert_eq!(read_pending_publish_files(&layrs_dir, "main").unwrap().len(), 1);
        assert_eq!(
            read_pending_publish_files(&layrs_dir, &feature.active_layer_id)
                .unwrap()
                .len(),
            1
        );

        switch_layer(created.local_space.local_space_id.clone(), "main".to_string()).unwrap();
        let main_scan = scan_working_tree(created.local_space.local_space_id).unwrap();
        assert_eq!(main_scan.pending_publish_count, 1);
        assert!(main_scan.steps.iter().all(|step| step.layer_id == "main"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn child_layer_inherits_parent_steps_without_copying_chunks() {
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
        let chunk_count_before = directory_file_count(
            &layrs_dir.join("objects").join("chunks"),
            Some("chunk"),
        );
        let feature =
            create_layer_from_current(created.local_space.local_space_id.clone(), "Feature".into())
                .unwrap();
        let chunk_count_after = directory_file_count(
            &layrs_dir.join("objects").join("chunks"),
            Some("chunk"),
        );
        assert_eq!(chunk_count_after, chunk_count_before);

        let mut feature_steps = read_step_files(&layrs_dir, &feature.active_layer_id).unwrap();
        feature_steps.sort_by(compare_steps_by_timeline);
        assert_eq!(feature_steps.len(), 2);
        assert_eq!(feature_steps[0].step_kind.as_deref(), Some("inherited"));
        assert_eq!(
            feature_steps[0].origin_layer_id.as_deref(),
            Some("main")
        );
        assert_eq!(feature_steps[0].origin_layer_name.as_deref(), Some("Main"));
        assert_eq!(feature_steps[0].origin_step_id.as_deref(), Some(main_1.as_str()));
        assert_eq!(feature_steps[1].step_kind.as_deref(), Some("inherited"));
        assert_eq!(
            feature_steps[1].origin_layer_id.as_deref(),
            Some("main")
        );
        assert_eq!(feature_steps[1].origin_layer_name.as_deref(), Some("Main"));
        assert_eq!(feature_steps[1].origin_step_id.as_deref(), Some(main_2.as_str()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn linked_child_receives_future_parent_steps_after_own_work() {
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

        switch_layer(created.local_space.local_space_id.clone(), "main".to_string()).unwrap();
        fs::write(space.join("main-3.txt"), "main 3\n").unwrap();
        let main_3 = save_local_step(created.local_space.local_space_id.clone())
            .unwrap()
            .step_id
            .unwrap();

        let layrs_dir = space.join(LAYRS_DIR);
        let mut feature_steps = read_step_files(&layrs_dir, &feature.active_layer_id).unwrap();
        feature_steps.sort_by(compare_steps_by_timeline);
        let provenance = feature_steps
            .iter()
            .map(|step| {
                (
                    step.origin_layer_id.as_deref().unwrap_or(step.layer_id.as_str()),
                    step.origin_layer_name.as_deref().unwrap_or("<missing>"),
                    step.origin_step_id.as_deref().unwrap_or(step.step_id.as_str()),
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

        switch_layer(
            created.local_space.local_space_id,
            feature.active_layer_id.clone(),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(space.join("main-3.txt")).unwrap(), "main 3\n");
        assert_eq!(
            fs::read_to_string(space.join("feature.txt")).unwrap(),
            "feature B\n"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn disconnected_child_does_not_receive_future_parent_steps() {
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

        switch_layer(created.local_space.local_space_id.clone(), "main".to_string()).unwrap();
        fs::write(space.join("main-2.txt"), "main 2\n").unwrap();
        let main_2 = save_local_step(created.local_space.local_space_id.clone())
            .unwrap()
            .step_id
            .unwrap();

        let layrs_dir = space.join(LAYRS_DIR);
        let feature_steps = read_step_files(&layrs_dir, &feature.active_layer_id).unwrap();
        assert!(
            feature_steps
                .iter()
                .all(|step| step.origin_step_id.as_deref() != Some(main_2.as_str())),
            "disconnected child should not receive future parent step"
        );
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
        let chunk_count_before = directory_file_count(
            &layrs_dir.join("objects").join("chunks"),
            Some("chunk"),
        );
        let cleared =
            clear_layer_steps(created.local_space.local_space_id.clone(), "main".to_string(), true)
                .unwrap();

        assert_eq!(fs::read_to_string(space.join("keep.txt")).unwrap(), "important data\n");
        assert_eq!(read_step_files(&layrs_dir, "main").unwrap().len(), 0);
        assert_eq!(read_pending_publish_files(&layrs_dir, "main").unwrap().len(), 0);
        assert!(cleared.archived_steps_path.as_deref().is_some_and(|path| {
            std::path::Path::new(path).join("steps").exists()
        }));
        let chunk_count_after = directory_file_count(
            &layrs_dir.join("objects").join("chunks"),
            Some("chunk"),
        );
        assert_eq!(chunk_count_after, chunk_count_before);

        let scan = scan_working_tree(created.local_space.local_space_id).unwrap();
        assert_eq!(scan.steps.len(), 0);
        assert!(!scan.changed);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn save_desktop_settings_rejects_empty_and_duplicate_shortcuts() {
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

