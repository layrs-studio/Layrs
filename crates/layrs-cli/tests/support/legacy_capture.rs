use crate::support::local_data_safety::*;
use std::fs;

#[test]
fn init_refuses_existing_space_files_and_discovers_from_child_directory() {
    let space = init_legacy_empty("init-refusals");
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
    let space = TestSpace::legacy("init-realistic");
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
    let space = TestSpace::legacy("ignore");
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
    let space = TestSpace::legacy("init-existing");
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
    let space = init_legacy_empty("added-file");
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
    let space = init_legacy_empty("modify-delete-add");
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
