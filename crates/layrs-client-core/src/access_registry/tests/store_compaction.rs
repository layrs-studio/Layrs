    #[test]
    fn repeated_text_steps_reuse_and_compress_chunks() {
        let root = unique_test_dir("store-size-text");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);
        fs::write(space.join("big.txt"), "a".repeat(187_000)).unwrap();

        let initialized =
            init_local_space("Size Optimized".to_string(), space.display().to_string()).unwrap();
        for index in 0..8 {
            let mut content = "a".repeat(186_960);
            content.push_str(&format!("step-{index:032}"));
            fs::write(space.join("big.txt"), content).unwrap();
            save_local_step(initialized.local_space.local_space_id.clone()).unwrap();
        }

        let chunks_dir = space.join(LAYRS_DIR).join("objects").join("chunks");
        let stored_bytes = directory_file_size(&chunks_dir, Some("chunk"));
        assert!(
            stored_bytes < 250_000,
            "expected optimized chunks below 250 KiB, got {stored_bytes}"
        );

        let scan = scan_working_tree(initialized.local_space.local_space_id).unwrap();
        assert_eq!(scan.steps.len(), 9);
        assert_eq!(scan.pending_publish_count, 9);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn compact_packs_loose_chunks_and_keeps_file_readable() {
        let root = unique_test_dir("compact-store");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);
        let expected = "hello compact\n".repeat(10_000);
        fs::write(space.join("big.txt"), &expected).unwrap();

        let initialized =
            init_local_space("Compact".to_string(), space.display().to_string()).unwrap();
        let layrs_dir = space.join(LAYRS_DIR);
        let before = directory_file_count(&layrs_dir.join("objects").join("chunks"), Some("chunk"));
        assert!(before > 0);

        let compacted = compact_local_space(initialized.local_space.local_space_id).unwrap();
        assert!(compacted.packed_chunks > 0);
        assert!(compacted.loose_chunks_removed > 0);
        assert!(compacted.pack_path.is_some());
        let after = directory_file_count(&layrs_dir.join("objects").join("chunks"), Some("chunk"));
        assert_eq!(after, 0);

        let state = read_layer_state(&layrs_dir, "local_layer_main").unwrap();
        let file = state
            .files
            .iter()
            .find(|file| file.path == "big.txt")
            .unwrap();
        let bytes = read_snapshot_object_bytes(&layrs_dir, file).unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), expected);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn save_local_step_clean_does_not_duplicate_step() {
        let root = unique_test_dir("save-step-clean");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);
        fs::write(space.join("note.txt"), "base\n").unwrap();

        let created = create_local_space(
            "space-save-clean".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        fs::write(space.join("note.txt"), "changed\n").unwrap();
        save_local_step(created.local_space.local_space_id.clone()).unwrap();

        let clean = save_local_step(created.local_space.local_space_id.clone()).unwrap();
        let scan = scan_working_tree(created.local_space.local_space_id).unwrap();

        assert_eq!(clean.status, "clean");
        assert_eq!(clean.changed_files, 0);
        assert_eq!(clean.pending_publish_count, 1);
        assert_eq!(scan.steps.len(), 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_includes_layer_step_activity_for_inactive_layers() {
        let root = unique_test_dir("layer-step-activity");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);
        fs::write(space.join("note.txt"), "main\n").unwrap();

        let created = create_local_space(
            "space-layer-activity".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        fs::write(space.join("note.txt"), "feature\n").unwrap();
        let feature = create_layer_from_current(
            created.local_space.local_space_id.clone(),
            "Feature".to_string(),
        )
        .unwrap();
        let feature_state = capture_working_state(&space, &feature.active_layer_id, true).unwrap();
        write_step(
            &space.join(LAYRS_DIR),
            &feature.active_layer_id,
            &feature_state,
        )
        .unwrap();
        switch_layer(
            created.local_space.local_space_id.clone(),
            "main".to_string(),
        )
        .unwrap();

        let scan = scan_working_tree(created.local_space.local_space_id).unwrap();
        let feature_activity = scan
            .layer_activities
            .iter()
            .find(|activity| activity.layer_id == feature.active_layer_id)
            .unwrap();

        assert_eq!(scan.steps.len(), 1);
        assert_eq!(scan.steps[0].layer_id, "main");
        assert_eq!(feature_activity.step_count, 1);
        assert!(feature_activity.latest_step_at > 0);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn step_summary_uses_recorded_base_after_index_advances() {
        let root = unique_test_dir("step-base-after-publish");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);

        let created = create_local_space(
            "space-step-after-publish".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        fs::write(space.join("note.txt"), "published\n").unwrap();
        let state = capture_working_state(&space, "main", true).unwrap();
        write_step(&space.join(LAYRS_DIR), "main", &state).unwrap();
        write_layer_state(&space.join(LAYRS_DIR), "main", &state).unwrap();

        let scan = scan_working_tree(created.local_space.local_space_id).unwrap();

        assert_eq!(scan.steps.len(), 1);
        assert_eq!(scan.steps[0].changed_files, 1);
        assert_eq!(scan.steps[0].diff_stats.files, 1);
        assert!(scan.steps[0].diff_stats.additions > 0);
        assert_eq!(scan.steps[0].diff_stats.deletions, 0);
        assert_eq!(scan.steps[0].diffs[0].path, "note.txt");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn step_v2_persists_tree_ids_without_file_duplication() {
        let root = unique_test_dir("step-v2");
        let config = root.join("config");
        let space = root.join("space");
        fs::create_dir_all(&space).unwrap();
        env::set_var("APPDATA", &config);
        fs::write(space.join("note.txt"), "base\n").unwrap();

        let created = create_local_space(
            "space-step-v2".to_string(),
            space.display().to_string(),
            Some("main".to_string()),
        )
        .unwrap();
        fs::write(space.join("note.txt"), "changed\n").unwrap();
        let state = capture_working_state(&space, "main", true).unwrap();
        let step_id = write_step(&space.join(LAYRS_DIR), "main", &state).unwrap();

        let layrs_dir = space.join(LAYRS_DIR);
        let working_state: Value = read_json(
            &layrs_dir
                .join("layers")
                .join("main")
                .join("working-state.json"),
        )
        .unwrap();
        assert_eq!(working_state["schema"], WORKING_STATE_SCHEMA);
        assert!(working_state["rootTreeId"].as_str().is_some());
        assert!(working_state["rootTreeId"]
            .as_str()
            .unwrap()
            .starts_with("blake3:"));
        assert!(working_state.get("files").is_none());

        let step: Value = read_json(
            &layrs_dir
                .join("layers")
                .join("main")
                .join("steps")
                .join(format!("{step_id}.json")),
        )
        .unwrap();
        assert_eq!(step["schema"], STEP_SCHEMA);
        assert!(step["rootTreeId"].as_str().is_some());
        assert!(step["rootTreeId"].as_str().unwrap().starts_with("blake3:"));
        assert_eq!(step["changedPaths"], serde_json::json!(["note.txt"]));
        assert!(step.get("files").is_none());

        assert!(layrs_dir.join("objects").join("trees").exists());
        assert!(layrs_dir.join("objects").join("files").exists());
        assert!(layrs_dir.join("objects").join("chunks").exists());
        assert_eq!(
            scan_working_tree(created.local_space.local_space_id)
                .unwrap()
                .steps
                .len(),
            1
        );

        let _ = fs::remove_dir_all(root);
    }

