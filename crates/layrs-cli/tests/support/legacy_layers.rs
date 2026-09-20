use crate::support::local_data_safety::*;
use std::fs;

#[test]
fn stepped_layer_switches_restore_each_layer_and_keep_timelines_separate() {
    let space = init_legacy_empty("stepped-layers");
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
    let space = init_legacy_empty("linked-step-propagation");
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
    let space = init_legacy_empty("cli-layer-disconnect");
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
    let space = init_legacy_empty("cli-layer-clear-steps");
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
    let space = init_legacy_empty("parent-auto-step-history");
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
    let space = init_legacy_empty("unstepped-layer-switch");
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
