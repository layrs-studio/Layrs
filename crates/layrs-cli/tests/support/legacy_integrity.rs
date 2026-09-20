use crate::support::local_data_safety::*;
use std::fs;

#[test]
fn moved_file_is_delete_plus_add_without_losing_content_across_layers() {
    let space = init_legacy_empty("move-file");
    fs::create_dir_all(space.space.join("src")).expect("create src");
    fs::write(space.space.join("src").join("a.txt"), "move me\n").expect("write a");
    run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);

    fs::rename(
        space.space.join("src").join("a.txt"),
        space.space.join("src").join("b.txt"),
    )
    .expect("rename");
    let diff = run_json(&space, ["--json", "--space", &space.space_arg(), "diff"]);
    assert_json_array_contains(&diff["files"], "src/a.txt");
    assert_json_array_contains(&diff["files"], "src/b.txt");
    let step = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    let step_id = step["step_id"].as_str().expect("step id").to_string();
    let by_id = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "diff", &step_id],
    );
    assert_json_array_contains(&by_id["files"], "src/a.txt");
    assert_json_array_contains(&by_id["files"], "src/b.txt");

    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "create",
            "Move Check",
        ],
    );
    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "use",
            "Main",
        ],
    );
    assert!(!space.space.join("src").join("a.txt").exists());
    assert_file(&space.space, "src/b.txt", "move me\n");

    space.pass();
}

#[test]
fn binary_asset_steps_compact_and_layer_switch_restore_exact_bytes() {
    let space = init_legacy_empty("binary-assets");
    let original = deterministic_png_like_bytes();
    let mut changed = original.clone();
    changed.extend_from_slice(&[0, 255, 17, 99, 1, 2, 3]);
    fs::create_dir_all(space.space.join("Assets")).expect("create assets");
    fs::write(space.space.join("Assets").join("hero.png"), &original).expect("write original");
    run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);

    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "create",
            "Art",
        ],
    );
    fs::write(space.space.join("Assets").join("hero.png"), &changed).expect("write changed");
    let diff = run_json(&space, ["--json", "--space", &space.space_arg(), "diff"]);
    assert_json_array_contains(&diff["files"], "Assets/hero.png");
    assert!(
        diff["text"]
            .as_str()
            .unwrap_or_default()
            .contains("diff --layrs Assets/hero.png")
    );
    run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    let resume_error = run_err(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "compact",
            "--resume",
        ],
    );
    assert!(resume_error.contains("requires a native Layrs checkout"));
    assert_file_bytes(&space.space, "Assets/hero.png", &changed);
    run_json(&space, ["--json", "--space", &space.space_arg(), "compact"]);

    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "use",
            "Main",
        ],
    );
    assert_file_bytes(&space.space, "Assets/hero.png", &original);

    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "use",
            "Art",
        ],
    );
    assert_file_bytes(&space.space, "Assets/hero.png", &changed);

    space.pass();
}

#[test]
fn multiple_steps_timeline_limit_status_and_diff_options_are_stable() {
    let space = init_legacy_empty("multi-step-options");
    let mut step_ids = Vec::new();
    for index in 1..=5 {
        fs::write(
            space.space.join("story.txt"),
            format!("line {index}\n{}", "x".repeat(260)),
        )
        .expect("write story");
        let step = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
        step_ids.push(step["step_id"].as_str().expect("step id").to_string());
    }

    let status = run_json(&space, ["--json", "--space", &space.space_arg(), "status"]);
    assert_eq!(status["changed"].as_bool(), Some(false));
    assert_eq!(status["pending_steps"].as_u64(), Some(5));

    let timeline = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "timeline",
            "--limit",
            "3",
        ],
    );
    assert_eq!(json_steps(&timeline).len(), 3);

    let name_only = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "diff",
            "--name-only",
            &step_ids[2],
        ],
    );
    assert_eq!(name_only["text"].as_str(), Some("story.txt"));

    let stat = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "diff",
            "--stat",
            &step_ids[4],
        ],
    );
    assert!(
        stat["text"]
            .as_str()
            .unwrap_or_default()
            .contains("story.txt | +")
    );

    let window = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "diff",
            "--window",
            "1:4",
            "--wrap",
            &step_ids[4],
        ],
    );
    assert!(
        window["text"]
            .as_str()
            .unwrap_or_default()
            .contains(&"x".repeat(260))
    );
    assert_eq!(window["truncated"].as_bool(), Some(false));

    let no_wrap = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "diff",
            "--no-wrap",
            &step_ids[4],
        ],
    );
    assert!(
        no_wrap["text"]
            .as_str()
            .unwrap_or_default()
            .contains(&"x".repeat(260))
    );

    space.pass();
}

#[test]
fn layer_lifecycle_lists_parents_and_refuses_unsafe_deletes() {
    let space = init_legacy_empty("layer-lifecycle");
    let layers = run_json(&space, ["--json", "--space", &space.space_arg(), "layers"]);
    assert_eq!(layers["layers"].as_array().expect("layers").len(), 1);
    assert_eq!(layers["layers"][0]["name"].as_str(), Some("Main"));
    assert_eq!(layers["layers"][0]["active"].as_bool(), Some(true));

    let feature = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "create",
            "Feature",
        ],
    );
    assert_eq!(feature["name"].as_str(), Some("Feature"));
    assert_eq!(feature["active"].as_bool(), Some(true));
    assert!(feature["parent_layer_id"].as_str().is_some());

    let no_yes = run_err(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "delete",
            "Feature",
        ],
    );
    assert!(no_yes.contains("without --yes"));

    let active_delete = run_err(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "delete",
            "Feature",
            "--yes",
        ],
    );
    assert!(active_delete.contains("Switch to another Layer"));

    let nested = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "create",
            "Nested",
        ],
    );
    assert_eq!(nested["name"].as_str(), Some("Nested"));

    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "use",
            "Main",
        ],
    );
    let parent_delete = run_err(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "delete",
            "Feature",
            "--yes",
        ],
    );
    assert!(parent_delete.contains("child Layers"));

    let deleted_nested = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "delete",
            "Nested",
            "--yes",
        ],
    );
    assert_eq!(deleted_nested["deleted"].as_bool(), Some(true));

    let deleted = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "delete",
            "Feature",
            "--yes",
        ],
    );
    assert_eq!(deleted["deleted"].as_bool(), Some(true));
    let final_layers = run_json(&space, ["--json", "--space", &space.space_arg(), "layers"]);
    assert_eq!(final_layers["layers"].as_array().expect("layers").len(), 1);

    space.pass();
}

#[test]
fn corrupted_store_fails_loudly_without_deleting_current_files() {
    let space = init_legacy_empty("corrupt-store");
    fs::write(space.space.join("main.txt"), "main safe\n").expect("write main");
    run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "create",
            "Broken",
        ],
    );
    fs::write(space.space.join("main.txt"), "broken target\n").expect("write broken");
    run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    let broken_status = run_json(&space, ["--json", "--space", &space.space_arg(), "status"]);
    let broken_layer_id = broken_status["active_layer_id"]
        .as_str()
        .expect("active layer")
        .to_string();
    let chunk_path = first_chunk_path(&space.space, &broken_layer_id, "main.txt");
    let saved_chunk = fs::read(&chunk_path).expect("save encoded chunk for repair check");

    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "use",
            "Main",
        ],
    );
    assert_file(&space.space, "main.txt", "main safe\n");

    fs::remove_file(&chunk_path)
        .unwrap_or_else(|error| panic!("remove corrupt chunk {}: {error}", chunk_path.display()));
    let switch_error = run_err(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "use",
            "Broken",
        ],
    );
    assert!(switch_error.contains("could not find chunk object"));
    assert_file(&space.space, "main.txt", "main safe\n");
    // The SQLite projection validates all referenced objects when opened. It
    // also refuses status while corrupt, rather than reporting a healthy store.
    assert!(
        run_err(&space, ["--json", "--space", &space.space_arg(), "status"])
            .contains("could not find chunk object")
    );
    fs::write(&chunk_path, saved_chunk).expect("restore exact missing bytes");
    let still_main = run_json(&space, ["--json", "--space", &space.space_arg(), "status"]);
    assert_eq!(
        still_main["active_layer_id"].as_str(),
        Some("local_layer_main")
    );

    space.pass();
}

#[test]
fn compact_keeps_large_repeated_steps_small_and_diffable() {
    let space = TestSpace::legacy("compact-weight");
    let mut content = deterministic_large_content(0);
    fs::write(space.space.join("big.txt"), &content).expect("write big");
    let init = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "init",
            "Compact Weight",
            "--path",
            &space.space_arg(),
        ],
    );
    assert_eq!(init["pending_publish_count"].as_u64(), Some(1));

    let mut latest_step_id = init["initial_step_id"]
        .as_str()
        .expect("initial step")
        .to_string();
    let mut raw_cumulative = content.len() as u64;
    for index in 1..=10 {
        content = deterministic_large_content(index);
        raw_cumulative += content.len() as u64;
        fs::write(space.space.join("big.txt"), &content).expect("modify big");
        let step = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
        assert_eq!(step["changed_files"].as_u64(), Some(1));
        latest_step_id = step["step_id"].as_str().expect("step id").to_string();
    }

    let step_dir = space
        .space
        .join(".layrs")
        .join("layers")
        .join("local_layer_main")
        .join("steps");
    for entry in fs::read_dir(&step_dir).expect("read step dir") {
        let path = entry.expect("step entry").path();
        let body = fs::read_to_string(&path).expect("read step json");
        assert!(
            body.len() < 16 * 1024,
            "step JSON should stay small: {} has {} bytes",
            path.display(),
            body.len()
        );
        assert!(
            !body.contains(&"a".repeat(4096)),
            "step JSON unexpectedly contains full repeated file content: {}",
            path.display()
        );
    }

    let compact = run_json(&space, ["--json", "--space", &space.space_arg(), "compact"]);
    let threshold = raw_cumulative * 35 / 100;
    let stored_bytes = compact["stored_bytes"].as_u64().expect("stored_bytes");
    let layrs_bytes = space_size_bytes(&space.space.join(".layrs"));
    assert!(
        stored_bytes < threshold,
        "compact stored_bytes {stored_bytes} should be under 35% of raw cumulative {raw_cumulative}"
    );
    // SQLite projections replaced loose JSON metadata. Keep the original CAS
    // compression guarantee and bound the metadata separately, rather than
    // pretending its fixed pages are uncompressed copies of file contents.
    let metadata_bytes = space_size_bytes(&space.space.join(".layrs/metadata"));
    assert!(
        metadata_bytes < 1024 * 1024,
        "metadata projection exceeds 1 MiB"
    );
    assert!(
        layrs_bytes - metadata_bytes < threshold,
        "CAS and remaining metadata exceed 35% of raw cumulative history"
    );

    let diff = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "diff",
            &latest_step_id,
        ],
    );
    assert_eq!(diff["source"].as_str(), Some("step"));
    assert_json_array_contains(&diff["files"], "big.txt");

    space.pass();
}
