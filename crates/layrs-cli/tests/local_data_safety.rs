mod support;

use std::fs;
use support::local_data_safety::*;

#[test]
fn init_refuses_existing_space_files_and_discovers_from_child_directory() {
    let space = init_empty("init-refusals");
    let duplicate = run_err(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "init",
            "Duplicate",
            "--path",
            &space.space_arg(),
        ],
    );
    assert!(duplicate.contains("existing Local Space"));

    let file_target = space.root.join("plain-file.txt");
    fs::write(&file_target, "not a directory\n").expect("write file target");
    let file_error = run_err(
        &space,
        [
            "--json",
            "init",
            "File Target",
            "--path",
            &file_target.display().to_string(),
        ],
    );
    assert!(file_error.contains("Local Space in a file"));

    let child = space.space.join("nested").join("child");
    fs::create_dir_all(&child).expect("create child");
    let status = read_json(&run_ok_in(&space, &child, ["--json", "status"]));
    assert_eq!(status["changed"].as_bool(), Some(false));
    assert_eq!(status["pending_steps"].as_u64(), Some(0));

    space.pass();
}

#[test]
fn init_existing_realistic_tree_preserves_nested_text_and_binary_assets() {
    let space = TestSpace::new("init-realistic");
    let image_bytes = deterministic_png_like_bytes();
    fs::create_dir_all(space.space.join("src")).expect("create src");
    fs::create_dir_all(space.space.join("Assets").join("Textures")).expect("create assets");
    fs::write(space.space.join("README.md"), "# Game\n").expect("write readme");
    fs::write(
        space.space.join("src").join("game.ts"),
        "export const game = true;\n",
    )
    .expect("write game");
    fs::write(
        space.space.join("Assets").join("Textures").join("hero.png"),
        &image_bytes,
    )
    .expect("write image");

    let init = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "init",
            "Realistic",
            "--path",
            &space.space_arg(),
        ],
    );
    assert_eq!(init["scanned_files"].as_u64(), Some(3));
    let diff = run_json(&space, ["--json", "--space", &space.space_arg(), "diff"]);
    assert_json_array_contains(&diff["files"], "Assets/Textures/hero.png");
    assert_json_array_contains(&diff["files"], "README.md");
    assert_json_array_contains(&diff["files"], "src/game.ts");
    assert_file(&space.space, "README.md", "# Game\n");
    assert_file(&space.space, "src/game.ts", "export const game = true;\n");
    assert_file_bytes(&space.space, "Assets/Textures/hero.png", &image_bytes);

    space.pass();
}

#[test]
fn layrsignore_excludes_temp_git_and_generated_paths() {
    let space = TestSpace::new("ignore");
    fs::create_dir_all(space.space.join("target").join("debug")).expect("create target");
    fs::create_dir_all(space.space.join(".git").join("objects")).expect("create git");
    fs::create_dir_all(space.space.join("logs")).expect("create logs");
    fs::write(space.space.join(".layrsignore"), "target/\n*.tmp\nlogs/\n").expect("write ignore");
    fs::write(space.space.join("keep.txt"), "tracked\n").expect("write keep");
    fs::write(space.space.join("scratch.tmp"), "ignored\n").expect("write tmp");
    fs::write(
        space.space.join("target").join("debug").join("build.txt"),
        "ignored\n",
    )
    .expect("write target");
    fs::write(space.space.join(".git").join("HEAD"), "ignored\n").expect("write git");
    fs::write(space.space.join("logs").join("run.log"), "ignored\n").expect("write log");

    let init = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "init",
            "Ignore",
            "--path",
            &space.space_arg(),
        ],
    );
    assert_eq!(init["scanned_files"].as_u64(), Some(2));
    let diff = run_json(&space, ["--json", "--space", &space.space_arg(), "diff"]);
    assert_json_array_contains(&diff["files"], ".layrsignore");
    assert_json_array_contains(&diff["files"], "keep.txt");
    assert_json_array_not_contains(&diff["files"], "scratch.tmp");
    assert_json_array_not_contains(&diff["files"], "target/debug/build.txt");
    assert_json_array_not_contains(&diff["files"], ".git/HEAD");
    assert_json_array_not_contains(&diff["files"], "logs/run.log");

    space.pass();
}

#[test]
fn init_existing_folder_preserves_content_and_exposes_initial_pending_step() {
    let space = TestSpace::new("init-existing");
    fs::write(space.space.join("hello.txt"), "hello before layrs\n").expect("write hello");

    let init = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "init",
            "Existing",
            "--path",
            &space.space_arg(),
        ],
    );
    let step_id = init["initial_step_id"]
        .as_str()
        .expect("initial step id")
        .to_string();

    assert!(space.space.join(".layrs").is_dir());
    assert_eq!(init["pending_publish_count"].as_u64(), Some(1));
    assert_file(&space.space, "hello.txt", "hello before layrs\n");

    let diff = run_json(&space, ["--json", "--space", &space.space_arg(), "diff"]);
    assert_eq!(diff["source"].as_str(), Some("latestPendingStep"));
    assert_eq!(diff["step_id"].as_str(), Some(step_id.as_str()));
    assert_eq!(
        diff["message"].as_str(),
        Some(
            format!("No working tree changes; displaying latest pending Step {step_id}.").as_str()
        )
    );
    assert_json_array_contains(&diff["files"], "hello.txt");

    let timeline = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "timeline"],
    );
    assert!(
        json_steps(&timeline)
            .iter()
            .any(|step| step["step_id"] == step_id)
    );

    space.pass();
}

#[test]
fn added_file_step_becomes_latest_pending_and_addressable_by_id() {
    let space = init_empty("added-file");
    fs::create_dir_all(space.space.join("src")).expect("create src");
    fs::write(space.space.join("src").join("new-file.txt"), "new file\n").expect("write new file");

    let diff = run_json(&space, ["--json", "--space", &space.space_arg(), "diff"]);
    assert_eq!(diff["source"].as_str(), Some("workingTree"));
    assert_json_array_contains(&diff["files"], "src/new-file.txt");

    let step = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    let step_id = step["step_id"].as_str().expect("step id").to_string();
    assert_eq!(step["changed_files"].as_u64(), Some(1));

    let latest = run_json(&space, ["--json", "--space", &space.space_arg(), "diff"]);
    assert_eq!(latest["source"].as_str(), Some("latestPendingStep"));
    assert_eq!(latest["step_id"].as_str(), Some(step_id.as_str()));
    assert_eq!(
        latest["message"].as_str(),
        Some(
            format!("No working tree changes; displaying latest pending Step {step_id}.").as_str()
        )
    );

    let by_id = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "diff", &step_id],
    );
    assert_eq!(by_id["source"].as_str(), Some("step"));
    assert_eq!(by_id["step_id"].as_str(), Some(step_id.as_str()));
    assert_json_array_contains(&by_id["files"], "src/new-file.txt");

    space.pass();
}

#[test]
fn modifications_deletions_and_additions_step_cleanly_without_touching_files() {
    let space = init_empty("modify-delete-add");
    fs::write(space.space.join("keep.txt"), "keep v1\n").expect("write keep");
    fs::write(space.space.join("edit.txt"), "edit v1\n").expect("write edit");
    let base = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    assert_eq!(base["changed_files"].as_u64(), Some(2));

    fs::write(space.space.join("edit.txt"), "edit v2\n").expect("modify edit");
    fs::write(space.space.join("add.txt"), "add v1\n").expect("write add");
    fs::remove_file(space.space.join("keep.txt")).expect("delete keep");

    let diff = run_json(&space, ["--json", "--space", &space.space_arg(), "diff"]);
    assert_eq!(diff["source"].as_str(), Some("workingTree"));
    assert_json_array_contains(&diff["files"], "add.txt");
    assert_json_array_contains(&diff["files"], "edit.txt");
    assert_json_array_contains(&diff["files"], "keep.txt");

    let step = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    assert_eq!(step["changed_files"].as_u64(), Some(3));

    let status = run_json(&space, ["--json", "--space", &space.space_arg(), "status"]);
    assert_eq!(status["changed"].as_bool(), Some(false));
    assert_file(&space.space, "edit.txt", "edit v2\n");
    assert_file(&space.space, "add.txt", "add v1\n");
    assert!(
        !space.space.join("keep.txt").exists(),
        "deleted file was restored"
    );

    space.pass();
}

#[test]
fn stepped_layer_switches_restore_each_layer_and_keep_timelines_separate() {
    let space = init_empty("stepped-layers");
    fs::write(space.space.join("shared.txt"), "main version\n").expect("write main");
    let main_step = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    let main_step_id = main_step["step_id"]
        .as_str()
        .expect("main step")
        .to_string();

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

    fs::write(space.space.join("shared.txt"), "feature version\n").expect("write feature");
    fs::write(space.space.join("feature-only.txt"), "feature only\n").expect("write feature only");
    let feature_step = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    let feature_step_id = feature_step["step_id"]
        .as_str()
        .expect("feature step")
        .to_string();

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
    assert_file(&space.space, "shared.txt", "main version\n");
    assert!(!space.space.join("feature-only.txt").exists());
    let main_timeline = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "timeline"],
    );
    assert!(
        json_steps(&main_timeline)
            .iter()
            .any(|step| step["step_id"] == main_step_id)
    );
    assert!(
        json_steps(&main_timeline)
            .iter()
            .all(|step| step["origin_step_id"].as_str() != Some(feature_step_id.as_str()))
    );

    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "use",
            "Feature",
        ],
    );
    assert_file(&space.space, "shared.txt", "feature version\n");
    assert_file(&space.space, "feature-only.txt", "feature only\n");
    let feature_timeline = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "timeline"],
    );
    assert_timeline_has_origin(
        &feature_timeline,
        "local_layer_main",
        &main_step_id,
        "inherited",
    );
    assert_timeline_has_origin(
        &feature_timeline,
        feature["layer_id"].as_str().expect("feature layer id"),
        &feature_step_id,
        "native",
    );

    space.pass();
}

#[test]
fn linked_child_layer_inherits_and_receives_parent_steps_in_order() {
    let space = init_empty("linked-step-propagation");
    fs::write(space.space.join("main-1.txt"), "main 1\n").expect("write main 1");
    let main_1 = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    fs::write(space.space.join("main-2.txt"), "main 2\n").expect("write main 2");
    let main_2 = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);

    let chunk_count_before_layer = loose_chunk_count(&space.space);
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
    assert_eq!(loose_chunk_count(&space.space), chunk_count_before_layer);
    let feature_layer_id = feature["layer_id"]
        .as_str()
        .expect("feature layer id")
        .to_string();

    let inherited = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "timeline"],
    );
    assert_timeline_has_origin(
        &inherited,
        "local_layer_main",
        main_1["step_id"].as_str().expect("main 1 step"),
        "inherited",
    );
    assert_timeline_has_origin(
        &inherited,
        "local_layer_main",
        main_2["step_id"].as_str().expect("main 2 step"),
        "inherited",
    );

    fs::write(space.space.join("feature.txt"), "feature B\n").expect("write feature");
    let feature_b = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);

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
    fs::write(space.space.join("main-3.txt"), "main 3\n").expect("write main 3");
    let main_3 = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);

    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "use",
            "Feature",
        ],
    );
    assert_file(&space.space, "main-1.txt", "main 1\n");
    assert_file(&space.space, "main-2.txt", "main 2\n");
    assert_file(&space.space, "main-3.txt", "main 3\n");
    assert_file(&space.space, "feature.txt", "feature B\n");

    let feature_timeline = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "timeline"],
    );
    assert_timeline_origin_order(
        &feature_timeline,
        &[
            (
                "local_layer_main",
                main_1["step_id"].as_str().expect("main 1 step"),
                "inherited",
            ),
            (
                "local_layer_main",
                main_2["step_id"].as_str().expect("main 2 step"),
                "inherited",
            ),
            (
                &feature_layer_id,
                feature_b["step_id"].as_str().expect("feature step"),
                "native",
            ),
            (
                "local_layer_main",
                main_3["step_id"].as_str().expect("main 3 step"),
                "inherited",
            ),
        ],
    );
    assert_eq!(
        timeline_step_by_origin(
            &feature_timeline,
            "local_layer_main",
            main_1["step_id"].as_str().expect("main 1 step"),
        )["origin_layer_name"]
            .as_str(),
        Some("Main")
    );
    assert_eq!(
        timeline_step_by_origin(
            &feature_timeline,
            &feature_layer_id,
            feature_b["step_id"].as_str().expect("feature step"),
        )["origin_layer_name"]
            .as_str(),
        Some("Feature")
    );

    space.pass();
}

#[test]
fn layer_disconnect_stops_future_parent_step_propagation() {
    let space = init_empty("cli-layer-disconnect");
    fs::write(space.space.join("main-1.txt"), "main 1\n").expect("write main 1");
    run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    run_json(
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
    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "disconnect",
            "Feature",
            "--yes",
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
    fs::write(space.space.join("main-2.txt"), "main 2\n").expect("write main 2");
    let main_2 = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);

    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "use",
            "Feature",
        ],
    );
    let feature_timeline = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "timeline"],
    );
    assert!(
        json_steps(&feature_timeline)
            .iter()
            .all(|step| step["origin_step_id"].as_str() != main_2["step_id"].as_str()),
        "disconnected layer should not receive the later parent step"
    );

    space.pass();
}

#[test]
fn layer_clear_steps_hides_history_but_keeps_files() {
    let space = init_empty("cli-layer-clear-steps");
    fs::write(space.space.join("keep.txt"), "do not lose me\n").expect("write keep");
    run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    let clear = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "clear-steps",
            "Main",
            "--yes",
        ],
    );
    assert!(clear["archived_steps_path"].as_str().is_some());
    assert_file(&space.space, "keep.txt", "do not lose me\n");
    let timeline = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "timeline"],
    );
    assert_eq!(json_steps(&timeline).len(), 0);

    space.pass();
}

#[test]
fn parent_auto_step_created_by_switch_stays_visible_on_parent_timeline() {
    let space = init_empty("parent-auto-step-history");
    fs::write(space.space.join("story.txt"), "line one\nline two\n").expect("write base story");
    let base = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    let base_step_id = base["step_id"].as_str().expect("base step").to_string();

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
    let feature_layer_id = feature["layer_id"]
        .as_str()
        .expect("feature layer id")
        .to_string();

    fs::write(
        space.space.join("story.txt"),
        "feature line one\nline two\n",
    )
    .expect("write feature story");
    let feature_step = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    let feature_step_id = feature_step["step_id"]
        .as_str()
        .expect("feature step")
        .to_string();

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
    assert_file(&space.space, "story.txt", "line one\nline two\n");

    fs::write(space.space.join("story.txt"), "line one\nmain line two\n")
        .expect("write unstepped main story");
    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "use",
            "Feature",
        ],
    );
    assert_file(
        &space.space,
        "story.txt",
        "feature line one\nmain line two\n",
    );

    let feature_timeline = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "timeline"],
    );
    let propagated_parent_step_id = json_steps(&feature_timeline)
        .iter()
        .find(|step| {
            step["origin_layer_id"].as_str() == Some("local_layer_main")
                && step["origin_step_id"].as_str() != Some(base_step_id.as_str())
                && step["step_kind"].as_str() == Some("inherited")
        })
        .and_then(|step| step["origin_step_id"].as_str())
        .expect("feature timeline has inherited parent auto-step")
        .to_string();

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
    assert_file(&space.space, "story.txt", "line one\nmain line two\n");
    let main_timeline = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "timeline"],
    );
    assert_timeline_has_origin(
        &main_timeline,
        "local_layer_main",
        &propagated_parent_step_id,
        "native",
    );
    assert_timeline_origin_order(
        &main_timeline,
        &[
            ("local_layer_main", &base_step_id, "native"),
            ("local_layer_main", &propagated_parent_step_id, "native"),
        ],
    );
    let main_status = run_json(&space, ["--json", "--space", &space.space_arg(), "status"]);
    assert_eq!(
        main_status["pending_steps"].as_u64(),
        Some(2),
        "Main must keep both its explicit Step and its auto-step pending for sync"
    );

    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "use",
            "Feature",
        ],
    );
    let feature_timeline = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "timeline"],
    );
    assert_timeline_origin_order(
        &feature_timeline,
        &[
            ("local_layer_main", &base_step_id, "inherited"),
            (&feature_layer_id, &feature_step_id, "native"),
            ("local_layer_main", &propagated_parent_step_id, "inherited"),
        ],
    );

    space.pass();
}

#[test]
fn switching_away_from_unstepped_layer_work_auto_steps_and_restores_it() {
    let space = init_empty("unstepped-layer-switch");
    fs::write(space.space.join("note.txt"), "base\n").expect("write base");
    run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    run_json(
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
    fs::write(space.space.join("note.txt"), "feature saved\n").expect("write feature");
    run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);

    fs::write(space.space.join("scratch.txt"), "unstepped feature work\n").expect("write scratch");
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
    assert_file(&space.space, "note.txt", "base\n");
    assert!(!space.space.join("scratch.txt").exists());

    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "layer",
            "use",
            "Feature",
        ],
    );
    assert_file(&space.space, "note.txt", "feature saved\n");
    assert_file(&space.space, "scratch.txt", "unstepped feature work\n");

    let latest = run_json(&space, ["--json", "--space", &space.space_arg(), "diff"]);
    let auto_step_id = latest["step_id"]
        .as_str()
        .expect("auto step id")
        .to_string();
    assert_eq!(latest["source"].as_str(), Some("latestPendingStep"));
    assert_eq!(
        latest["message"].as_str(),
        Some(
            format!("No working tree changes; displaying latest pending Step {auto_step_id}.")
                .as_str()
        )
    );
    assert_json_array_contains(&latest["files"], "scratch.txt");

    space.pass();
}

#[test]
fn moved_file_is_delete_plus_add_without_losing_content_across_layers() {
    let space = init_empty("move-file");
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
    let space = init_empty("binary-assets");
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
    let space = init_empty("multi-step-options");
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
    let space = init_empty("layer-lifecycle");
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
    let space = init_empty("corrupt-store");
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

    let chunk_path = first_chunk_path(&space.space, &broken_layer_id, "main.txt");
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
    let still_main = run_json(&space, ["--json", "--space", &space.space_arg(), "status"]);
    assert_eq!(
        still_main["active_layer_id"].as_str(),
        Some("local_layer_main")
    );

    space.pass();
}

#[test]
fn compact_keeps_large_repeated_steps_small_and_diffable() {
    let space = TestSpace::new("compact-weight");
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
    assert!(
        layrs_bytes < threshold,
        ".layrs size {layrs_bytes} should be under 35% of raw cumulative {raw_cumulative}"
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
