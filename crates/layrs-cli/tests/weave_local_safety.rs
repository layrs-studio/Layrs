mod support;

use std::fs;
use support::local_data_safety::*;

#[test]
fn clean_weave_reorders_linked_history_without_losing_parent_steps() {
    let space = init_empty("weave-clean-linked");
    fs::write(space.space.join("main-1.txt"), "main 1\n").expect("write main 1");
    let main_1 = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    fs::write(space.space.join("main-2.txt"), "main 2\n").expect("write main 2");
    let main_2 = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);

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
    let pre_weave_main = snapshot_working_tree(&space.space);

    let preview = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "Feature",
            "--target",
            "Main",
            "--preview",
        ],
    );
    assert_eq!(preview["session"]["status"].as_str(), Some("preview"));
    assert_eq!(
        preview["session"]["conflicts"]
            .as_array()
            .expect("preview conflicts")
            .len(),
        0
    );
    assert_working_tree_snapshot(&space.space, &pre_weave_main);

    let weave = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "Feature",
            "--target",
            "Main",
        ],
    );
    assert_eq!(weave["session"]["status"].as_str(), Some("applied"));
    assert_file(&space.space, "main-1.txt", "main 1\n");
    assert_file(&space.space, "main-2.txt", "main 2\n");
    assert_file(&space.space, "main-3.txt", "main 3\n");
    assert_file(&space.space, "feature.txt", "feature B\n");

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
    assert_file(&space.space, "main-3.txt", "main 3\n");
    assert_file(&space.space, "feature.txt", "feature B\n");
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
    assert_file(&space.space, "main-3.txt", "main 3\n");
    assert_file(&space.space, "feature.txt", "feature B\n");

    let main_timeline = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "timeline"],
    );
    assert_timeline_origin_order(
        &main_timeline,
        &[
            (
                "local_layer_main",
                main_1["step_id"].as_str().expect("main 1"),
                "native",
            ),
            (
                "local_layer_main",
                main_2["step_id"].as_str().expect("main 2"),
                "native",
            ),
            (
                feature_layer_id.as_str(),
                feature_b["step_id"].as_str().expect("feature B"),
                "woven",
            ),
            (
                "local_layer_main",
                main_3["step_id"].as_str().expect("main 3"),
                "native",
            ),
        ],
    );
    let woven_count = json_steps(&main_timeline)
        .iter()
        .filter(|step| {
            step["origin_layer_id"].as_str() == Some(feature_layer_id.as_str())
                && step["origin_step_id"].as_str()
                    == Some(feature_b["step_id"].as_str().expect("feature B"))
        })
        .count();
    assert_eq!(woven_count, 1);

    space.pass();
}

#[test]
fn weave_parent_uses_active_layer_parent_without_source_target_selection() {
    let space = init_empty("weave-parent-shortcut");
    fs::write(space.space.join("main.txt"), "main\n").expect("write main");
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
    fs::write(space.space.join("feature.txt"), "feature\n").expect("write feature");
    let feature_step = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);

    let weave = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "weave", "parent"],
    );
    assert_eq!(weave["session"]["status"].as_str(), Some("applied"));

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
    assert_file(&space.space, "feature.txt", "feature\n");
    let timeline = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "timeline"],
    );
    let woven_count = json_steps(&timeline)
        .iter()
        .filter(|step| {
            step["origin_step_id"].as_str()
                == Some(feature_step["step_id"].as_str().expect("feature step"))
                && step["step_kind"].as_str() == Some("woven")
        })
        .count();
    assert_eq!(woven_count, 1);

    space.pass();
}

#[test]
fn weave_conflict_abort_restores_pre_weave_target() {
    let space = init_conflicting_weave_space("weave-abort");

    let weave = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "Feature",
            "--target",
            "Main",
        ],
    );
    assert_eq!(weave["session"]["status"].as_str(), Some("conflicted"));
    let marked = fs::read_to_string(space.space.join("story.txt")).expect("read marked conflict");
    assert!(marked.contains("<<<<<<< target:local_layer_main"));
    assert!(marked.contains(">>>>>>> source:feature-"));
    assert_latest_weave_conflict_files(&space.space);

    let conflicts = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "conflicts",
        ],
    );
    let conflicts = conflicts.as_array().expect("conflicts");
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["path"].as_str(), Some("story.txt"));

    let premature = run_err(
        &space,
        ["--json", "--space", &space.space_arg(), "weave", "continue"],
    );
    assert!(premature.contains("Resolve all Weave conflicts"));

    let aborted = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "weave", "abort"],
    );
    assert_eq!(aborted["session"]["status"].as_str(), Some("aborted"));
    assert_file(&space.space, "story.txt", "main\n");

    space.pass();
}

#[test]
fn linked_parent_auto_step_conflicts_with_child_same_line_change_on_switch() {
    let space = init_empty("linked-parent-auto-step-conflict");
    fs::write(space.space.join("story.txt"), "base\nline 2\n").expect("write base");
    let base_step = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);

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
    let feature_layer_id = feature["layer_id"].as_str().expect("feature layer id");
    fs::write(space.space.join("story.txt"), "feature line 1\nline 2\n")
        .expect("write feature line");
    let feature_step = run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);

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
    fs::write(space.space.join("story.txt"), "main line 1\nline 2\n")
        .expect("write unstepped main line");

    let switch = run_json(
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
    assert_eq!(switch["active"].as_bool(), Some(true));

    let status = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "weave", "status"],
    );
    assert_eq!(status["status"].as_str(), Some("conflicted"));
    assert_eq!(status["source_layer_id"].as_str(), Some("local_layer_main"));
    assert_eq!(status["target_layer_id"].as_str(), Some(feature_layer_id));
    let auto_main_step = status["planned_steps"][0]
        .as_str()
        .expect("conflicted propagation should plan the auto-created Main Step");
    assert_ne!(
        auto_main_step,
        base_step["step_id"].as_str().expect("base step")
    );
    assert_ne!(
        auto_main_step,
        feature_step["step_id"].as_str().expect("feature step")
    );

    let conflicts = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "conflicts",
        ],
    );
    let conflicts = conflicts.as_array().expect("conflicts");
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["path"].as_str(), Some("story.txt"));
    assert_eq!(conflicts[0]["lens_id"].as_str(), Some("layrs.text"));

    let marked = fs::read_to_string(space.space.join("story.txt")).expect("read marked conflict");
    assert!(marked.contains("<<<<<<< target:"));
    assert!(marked.contains("feature line 1"));
    assert!(marked.contains("======="));
    assert!(marked.contains("main line 1"));
    assert!(marked.contains(">>>>>>> source:local_layer_main"));
    assert_latest_weave_conflict_files(&space.space);

    let feature_timeline = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "timeline"],
    );
    assert_timeline_origin_order(
        &feature_timeline,
        &[
            (
                "local_layer_main",
                base_step["step_id"].as_str().expect("base step"),
                "inherited",
            ),
            (
                feature_layer_id,
                feature_step["step_id"].as_str().expect("feature step"),
                "native",
            ),
        ],
    );

    let aborted = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "weave", "abort"],
    );
    assert_eq!(aborted["session"]["status"].as_str(), Some("aborted"));
    assert_file(&space.space, "story.txt", "feature line 1\nline 2\n");

    space.pass();
}

#[test]
fn weave_conflict_resolve_theirs_continue_updates_target_without_losing_source() {
    let space = init_conflicting_weave_space("weave-resolve");

    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "Feature",
            "--target",
            "Main",
        ],
    );
    let marked = fs::read_to_string(space.space.join("story.txt")).expect("read marked conflict");
    assert!(marked.contains("<<<<<<< target:local_layer_main"));
    assert!(marked.contains(">>>>>>> source:feature-"));

    let resolved = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "resolve",
            "story.txt",
            "--theirs",
        ],
    );
    assert_eq!(resolved["session"]["status"].as_str(), Some("resolved"));

    let continued = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "weave", "continue"],
    );
    assert_eq!(continued["session"]["status"].as_str(), Some("applied"));
    assert_file(&space.space, "story.txt", "feature\n");

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
    assert_file(&space.space, "story.txt", "feature\n");

    space.pass();
}

#[test]
fn conflict_list_status_and_text_resolve_use_product_methods() {
    let space = init_conflicting_weave_space("conflict-cli-text");
    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "Feature",
            "--target",
            "Main",
        ],
    );

    let status = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "conflict",
            "status",
        ],
    );
    assert_eq!(status["status"].as_str(), Some("conflicted"));

    let conflicts = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "conflict", "list"],
    );
    let conflicts = conflicts.as_array().expect("conflict list");
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["path"].as_str(), Some("story.txt"));
    assert_eq!(conflicts[0]["lens_id"].as_str(), Some("layrs.text"));

    let missing_block = run_err(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "conflict",
            "resolve",
            "incoming",
            "-f",
            "story.txt",
        ],
    );
    assert!(missing_block.contains("requires --block for text conflicts"));

    let resolved = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "conflict",
            "resolve",
            "incoming",
            "-f",
            "story.txt",
            "--block",
            "block-1",
        ],
    );
    assert_eq!(resolved["session"]["status"].as_str(), Some("resolved"));

    let continued = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "conflict",
            "continue",
        ],
    );
    assert_eq!(continued["session"]["status"].as_str(), Some("applied"));
    assert_file(&space.space, "story.txt", "feature\n");

    space.pass();
}

#[test]
fn conflict_text_resolve_both_and_manual_are_block_scoped() {
    let both = init_conflicting_weave_space("conflict-cli-both");
    run_json(
        &both,
        [
            "--json",
            "--space",
            &both.space_arg(),
            "weave",
            "Feature",
            "--target",
            "Main",
        ],
    );
    run_json(
        &both,
        [
            "--json",
            "--space",
            &both.space_arg(),
            "conflict",
            "resolve",
            "both",
            "-f",
            "story.txt",
            "--block",
            "1",
        ],
    );
    run_json(
        &both,
        [
            "--json",
            "--space",
            &both.space_arg(),
            "conflict",
            "continue",
        ],
    );
    assert_file(&both.space, "story.txt", "main\nfeature\n");
    both.pass();

    let manual = init_conflicting_weave_space("conflict-cli-manual");
    run_json(
        &manual,
        [
            "--json",
            "--space",
            &manual.space_arg(),
            "weave",
            "Feature",
            "--target",
            "Main",
        ],
    );
    let resolved = run_json_with_stdin(
        &manual,
        [
            "--json",
            "--space",
            &manual.space_arg(),
            "conflict",
            "resolve",
            "manual",
            "-f",
            "story.txt",
            "--block",
            "1",
        ],
        "manual cli block\n",
    );
    assert_eq!(resolved["session"]["status"].as_str(), Some("resolved"));
    run_json(
        &manual,
        [
            "--json",
            "--space",
            &manual.space_arg(),
            "conflict",
            "continue",
        ],
    );
    assert_file(&manual.space, "story.txt", "manual cli block\n");
    manual.pass();
}

#[test]
fn conflict_raw_resolve_enforces_file_level_existing_incoming_only() {
    let (space, _main_bytes, feature_bytes, _base_bytes) =
        init_conflicting_binary_weave_space("conflict-cli-raw");
    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "Feature",
            "--target",
            "Main",
        ],
    );

    let block_error = run_err(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "conflict",
            "resolve",
            "incoming",
            "-f",
            "Assets/hero.png",
            "--block",
            "1",
        ],
    );
    assert!(block_error.contains("--block cannot be used for raw conflicts"));

    let both_error = run_err(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "conflict",
            "resolve",
            "both",
            "-f",
            "Assets/hero.png",
        ],
    );
    assert!(both_error.contains("cannot be used for raw conflicts"));

    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "conflict",
            "resolve",
            "incoming",
            "-f",
            "Assets/hero.png",
        ],
    );
    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "conflict",
            "continue",
        ],
    );
    assert_file_bytes(&space.space, "Assets/hero.png", &feature_bytes);
    space.pass();
}

#[test]
fn conflict_interactive_resolve_continue_is_scriptable() {
    let space = init_conflicting_weave_space("conflict-cli-interactive-continue");
    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "Feature",
            "--target",
            "Main",
        ],
    );

    let transcript = run_ok_with_stdin(
        &space,
        ["--space", &space.space_arg(), "conflict", "resolve"],
        "i\nc\n",
    );
    assert!(transcript.contains("Choose conflict action"));
    assert!(transcript.contains("Conflict session continued."));
    assert_file(&space.space, "story.txt", "feature\n");
    space.pass();
}

#[test]
fn conflict_interactive_abort_requires_confirmation_and_supports_quit() {
    let space = init_conflicting_weave_space("conflict-cli-interactive-abort");
    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "Feature",
            "--target",
            "Main",
        ],
    );

    let quit = run_ok_with_stdin(
        &space,
        ["--space", &space.space_arg(), "conflict", "resolve"],
        "a\nn\nq\n",
    );
    assert!(quit.contains("Abort active conflict session?"));
    let still_active = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "conflict",
            "status",
        ],
    );
    assert_eq!(still_active["status"].as_str(), Some("conflicted"));

    let aborted = run_ok_with_stdin(
        &space,
        ["--space", &space.space_arg(), "conflict", "resolve"],
        "a\ny\n",
    );
    assert!(aborted.contains("Conflict session aborted."));
    assert_file(&space.space, "story.txt", "main\n");
    space.pass();
}

#[test]
fn weave_text_conflict_resolution_modes_are_durable() {
    assert_text_conflict_resolution("weave-resolve-ours", "--ours", "main\n");
    assert_text_conflict_resolution("weave-resolve-base", "--base", "base\n");
}

#[test]
fn weave_text_conflict_resolves_independent_blocks_without_losing_context() {
    let space = init_empty("weave-resolve-blocks");
    fs::write(
        space.space.join("story.txt"),
        "top\nfirst base\nmiddle\nsecond base\nbottom\n",
    )
    .expect("write base");
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
    fs::write(
        space.space.join("story.txt"),
        "top\nfirst feature\nmiddle\nsecond feature\nbottom\n",
    )
    .expect("write feature");
    run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);

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
    fs::write(
        space.space.join("story.txt"),
        "top\nfirst main\nmiddle\nsecond main\nbottom\n",
    )
    .expect("write main");
    run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "weave", "abort"],
    );

    let weave = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "Feature",
            "--target",
            "Main",
        ],
    );
    assert_eq!(weave["session"]["status"].as_str(), Some("conflicted"));
    let blocks = weave["session"]["conflicts"][0]["blocks"]
        .as_array()
        .expect("conflict blocks");
    assert_eq!(blocks.len(), 2, "{blocks:?}");
    assert_eq!(blocks[0]["base"].as_str(), Some("first base\n"));
    assert_eq!(blocks[1]["base"].as_str(), Some("second base\n"));

    let first = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "resolve",
            "story.txt",
            "--block",
            "1",
            "--ours",
        ],
    );
    assert_eq!(first["session"]["status"].as_str(), Some("conflicted"));
    assert_eq!(
        first["session"]["conflicts"][0]["blocks"][0]["status"].as_str(),
        Some("resolved")
    );

    let second = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "resolve",
            "story.txt",
            "--block",
            "2",
            "--theirs",
        ],
    );
    assert_eq!(second["session"]["status"].as_str(), Some("resolved"));

    let continued = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "weave", "continue"],
    );
    assert_eq!(continued["session"]["status"].as_str(), Some("applied"));
    assert_file(
        &space.space,
        "story.txt",
        "top\nfirst main\nmiddle\nsecond feature\nbottom\n",
    );
    space.pass();
}

#[test]
fn weave_text_conflict_block_can_keep_both_sides_in_order() {
    let space = init_conflicting_weave_space("weave-resolve-block-both");
    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "Feature",
            "--target",
            "Main",
        ],
    );
    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "resolve",
            "story.txt",
            "--block",
            "1",
            "--both-ours-first",
        ],
    );
    run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "weave", "continue"],
    );
    assert_file(&space.space, "story.txt", "main\nfeature\n");
    space.pass();
}

#[test]
fn weave_text_conflict_block_accepts_manual_text() {
    let space = init_conflicting_weave_space("weave-resolve-block-manual");
    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "Feature",
            "--target",
            "Main",
        ],
    );
    let manual = space.root.join("manual-block.txt");
    fs::write(&manual, "manual block\n").expect("write manual block");
    let resolved = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "resolve",
            "story.txt",
            "--block",
            "1",
            "--manual-text",
            &manual.display().to_string(),
        ],
    );
    assert_eq!(resolved["session"]["status"].as_str(), Some("resolved"));
    run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "weave", "continue"],
    );
    assert_file(&space.space, "story.txt", "manual block\n");
    space.pass();
}

#[test]
fn weave_text_conflict_resolve_file_uses_manual_replacement() {
    let space = init_conflicting_weave_space("weave-resolve-file");
    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "Feature",
            "--target",
            "Main",
        ],
    );
    let replacement = space.root.join("manual-resolution.txt");
    fs::write(&replacement, "manual\nresolved\n").expect("write replacement");
    let resolved = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "resolve",
            "story.txt",
            "--file",
            &replacement.display().to_string(),
        ],
    );
    assert_eq!(resolved["session"]["status"].as_str(), Some("resolved"));
    run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "weave", "continue"],
    );
    assert_file(&space.space, "story.txt", "manual\nresolved\n");
    space.pass();
}

#[test]
fn weave_binary_conflict_does_not_inject_markers_and_resolves_exact_bytes() {
    let (space, main_bytes, feature_bytes, _base_bytes) =
        init_conflicting_binary_weave_space("weave-binary");
    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "Feature",
            "--target",
            "Main",
        ],
    );
    assert_file_bytes(&space.space, "Assets/hero.png", &main_bytes);
    assert_latest_weave_conflict_files(&space.space);
    let conflicts = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "conflicts",
        ],
    );
    assert_eq!(conflicts[0]["lens_id"].as_str(), Some("layrs.image"));

    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "resolve",
            "Assets/hero.png",
            "--theirs",
        ],
    );
    run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "weave", "continue"],
    );
    assert_file_bytes(&space.space, "Assets/hero.png", &feature_bytes);
    space.pass();
}

#[test]
fn weave_binary_conflict_resolve_file_uses_replacement_bytes() {
    let (space, _main_bytes, _feature_bytes, _base_bytes) =
        init_conflicting_binary_weave_space("weave-binary-file");
    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "Feature",
            "--target",
            "Main",
        ],
    );
    let replacement = space.root.join("replacement.bin");
    let replacement_bytes = vec![9, 8, 7, 6, 5, 4, 3, 2, 1];
    fs::write(&replacement, &replacement_bytes).expect("write replacement");
    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "resolve",
            "Assets/hero.png",
            "--file",
            &replacement.display().to_string(),
        ],
    );
    run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "weave", "continue"],
    );
    assert_file_bytes(&space.space, "Assets/hero.png", &replacement_bytes);
    space.pass();
}

fn init_conflicting_weave_space(label: &str) -> TestSpace {
    let space = init_empty(label);
    fs::write(space.space.join("story.txt"), "base\n").expect("write base");
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
    fs::write(space.space.join("story.txt"), "feature\n").expect("write feature");
    run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);

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
    fs::write(space.space.join("story.txt"), "main\n").expect("write main");
    run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    let automatic = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "weave", "status"],
    );
    assert_eq!(automatic["status"].as_str(), Some("conflicted"));
    assert_eq!(
        automatic["source_layer_id"].as_str(),
        Some("local_layer_main")
    );
    run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "weave", "abort"],
    );

    space
}

fn assert_text_conflict_resolution(label: &str, flag: &str, expected: &str) {
    let space = init_conflicting_weave_space(label);
    run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "Feature",
            "--target",
            "Main",
        ],
    );
    let resolved = run_json(
        &space,
        [
            "--json",
            "--space",
            &space.space_arg(),
            "weave",
            "resolve",
            "story.txt",
            flag,
        ],
    );
    assert_eq!(resolved["session"]["status"].as_str(), Some("resolved"));
    let continued = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "weave", "continue"],
    );
    assert_eq!(continued["session"]["status"].as_str(), Some("applied"));
    assert_file(&space.space, "story.txt", expected);
    space.pass();
}

fn init_conflicting_binary_weave_space(label: &str) -> (TestSpace, Vec<u8>, Vec<u8>, Vec<u8>) {
    let space = init_empty(label);
    fs::create_dir_all(space.space.join("Assets")).expect("create assets");
    let base = deterministic_png_like_bytes();
    let mut feature = base.clone();
    feature.extend_from_slice(&[1, 3, 5, 7, 9]);
    let mut main = base.clone();
    main.extend_from_slice(&[2, 4, 6, 8, 10]);
    fs::write(space.space.join("Assets").join("hero.png"), &base).expect("write base binary");
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
    fs::write(space.space.join("Assets").join("hero.png"), &feature).expect("write feature");
    run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);

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
    fs::write(space.space.join("Assets").join("hero.png"), &main).expect("write main");
    run_json(&space, ["--json", "--space", &space.space_arg(), "step"]);
    let automatic = run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "weave", "status"],
    );
    assert_eq!(automatic["status"].as_str(), Some("conflicted"));
    run_json(
        &space,
        ["--json", "--space", &space.space_arg(), "weave", "abort"],
    );

    (space, main, feature, base)
}
