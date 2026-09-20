//! Native CLI safety. Historical linked Layers, clear-steps and compaction
//! remain explicit old-engine contracts in legacy_local_data_safety.
mod support;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};
use support::local_data_safety::*;

fn cli(s: &TestSpace, args: &[&str]) -> Value {
    run_json(s, ["--json"].into_iter().chain(args.iter().copied()))
}
fn initialize(s: &TestSpace) {
    cli(s, &["init", "Native safety"]);
    assert!(s.space.join(".layrs/FORMAT").is_file());
}
fn empty(label: &str) -> TestSpace {
    let s = TestSpace::new(label);
    initialize(&s);
    assert_eq!(cli(&s, &["status"])["dirty"], false);
    s
}
fn step(s: &TestSpace) -> String {
    cli(s, &["step"])["stepId"].as_str().unwrap().into()
}
fn changes(s: &TestSpace, id: Option<&str>) -> Vec<Value> {
    let value = match id {
        Some(id) => cli(s, &["diff", id]),
        None => cli(s, &["diff"]),
    };
    value["changes"].as_array().unwrap().clone()
}
fn paths(changes: &[Value]) -> BTreeSet<String> {
    changes
        .iter()
        .map(|v| v["path"].as_str().unwrap().into())
        .collect()
}
fn history(s: &TestSpace, layer: &str) -> Vec<String> {
    cli(
        s,
        &["history", &format!("@layer:{layer}"), "--limit", "100"],
    )["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["stepId"].as_str().unwrap().into())
        .collect()
}
fn files(root: &Path) -> BTreeSet<PathBuf> {
    if !root.exists() {
        return BTreeSet::new();
    }
    fs::read_dir(root)
        .unwrap()
        .flat_map(|e| {
            let p = e.unwrap().path();
            if p.is_dir() {
                files(&p)
            } else {
                BTreeSet::from([p])
            }
        })
        .collect()
}

#[test]
fn init_refuses_existing_metadata_and_files_and_discovers_from_child() {
    let s = empty("native-init-refusals");
    let before = cli(&s, &["status"])["state"].clone();
    run_err(&s, ["--json", "init", "Duplicate"]);
    assert_eq!(cli(&s, &["status"])["state"], before);
    let file = s.root.join("plain-file");
    fs::write(&file, b"preserve").unwrap();
    run_err(
        &s,
        ["--json", "init", "File", "--path", file.to_str().unwrap()],
    );
    assert_eq!(fs::read(file).unwrap(), b"preserve");
    let child = s.space.join("nested/child");
    fs::create_dir_all(&child).unwrap();
    let status = read_json(&run_ok_in(&s, &child, ["--json", "status"]));
    assert_eq!(status["dirty"], false);
    assert_eq!(status["state"]["space"], before["space"]);
    s.pass();
}
#[test]
fn init_preserves_nested_unicode_binary_and_empty_files() {
    let s = TestSpace::new("native-realistic");
    fs::create_dir_all(s.space.join("Assets/Textures")).unwrap();
    fs::create_dir(s.space.join("src")).unwrap();
    fs::write(
        s.space.join("Assets/Textures/hero.png"),
        deterministic_png_like_bytes(),
    )
    .unwrap();
    fs::write(s.space.join("src/été.txt"), "été\n").unwrap();
    fs::write(s.space.join("README.md"), b"# Game\n").unwrap();
    fs::write(s.space.join("empty"), []).unwrap();
    let before = snapshot_working_tree(&s.space);
    initialize(&s);
    let initial = step(&s);
    assert_working_tree_snapshot(&s.space, &before);
    assert_eq!(
        paths(&changes(&s, Some(&initial))),
        BTreeSet::from([
            "Assets/Textures/hero.png".into(),
            "src/été.txt".into(),
            "README.md".into(),
            "empty".into()
        ])
    );
    assert!(changes(&s, None).is_empty());
    s.pass();
}
#[test]
fn layrsignore_excludes_generated_files_from_capture_and_status() {
    let s = TestSpace::new("native-ignore");
    for d in ["target/debug", ".git/objects", "logs"] {
        fs::create_dir_all(s.space.join(d)).unwrap();
    }
    fs::write(s.space.join(".layrsignore"), "target/\n*.tmp\nlogs/\n").unwrap();
    for file in [
        "keep.txt",
        "scratch.tmp",
        "target/debug/build.txt",
        ".git/HEAD",
        "logs/run.log",
    ] {
        fs::write(s.space.join(file), b"bytes\n").unwrap();
    }
    initialize(&s);
    let initial = step(&s);
    assert_eq!(
        paths(&changes(&s, Some(&initial))),
        BTreeSet::from([".layrsignore".into(), "keep.txt".into()])
    );
    fs::write(s.space.join("scratch.tmp"), b"ignored mutation").unwrap();
    assert_eq!(cli(&s, &["status"])["dirty"], false);
    assert!(changes(&s, None).is_empty());
    s.pass();
}
#[test]
fn initial_history_is_immutable_without_a_fake_pending_diff() {
    let s = TestSpace::new("native-initial");
    fs::write(s.space.join("hello"), b"hello\n").unwrap();
    initialize(&s);
    let initial = step(&s);
    assert!(history(&s, "main").contains(&initial));
    assert!(changes(&s, None).is_empty());
    assert_eq!(changes(&s, Some(&initial))[0]["path"], "hello");
    let operations = cli(&s, &["timeline"]);
    assert_eq!(cli(&s, &["step"])["status"], "unchanged");
    assert_eq!(cli(&s, &["timeline"]), operations);
    s.pass();
}
#[test]
fn added_file_remains_addressable_after_capture_and_working_diff_is_clean() {
    let s = empty("native-add");
    fs::create_dir(s.space.join("src")).unwrap();
    fs::write(s.space.join("src/new.txt"), b"new file\n").unwrap();
    assert_eq!(changes(&s, None)[0]["status"], "added");
    let captured = step(&s);
    assert!(changes(&s, None).is_empty());
    assert_eq!(changes(&s, Some(&captured))[0]["path"], "src/new.txt");
    assert_file(&s.space, "src/new.txt", "new file\n");
    s.pass();
}
#[test]
fn edits_deletions_additions_and_empty_files_remain_distinct() {
    let s = empty("native-change-types");
    for file in ["edit", "delete", "empty"] {
        fs::write(s.space.join(file), b"before\n").unwrap();
    }
    step(&s);
    fs::write(s.space.join("edit"), b"after\n").unwrap();
    fs::remove_file(s.space.join("delete")).unwrap();
    fs::write(s.space.join("empty"), []).unwrap();
    fs::write(s.space.join("added"), b"new\n").unwrap();
    let delta = changes(&s, None);
    assert_eq!(delta.len(), 4);
    assert!(delta.iter().find(|v| v["path"] == "delete").unwrap()["afterBytes"].is_null());
    assert_eq!(
        delta.iter().find(|v| v["path"] == "empty").unwrap()["afterBytes"],
        0
    );
    let before = snapshot_working_tree(&s.space);
    let captured = step(&s);
    assert_eq!(changes(&s, Some(&captured)).len(), 4);
    assert_working_tree_snapshot(&s.space, &before);
    assert_eq!(cli(&s, &["status"])["dirty"], false);
    s.pass();
}
#[test]
fn independent_layers_preserve_exact_states_and_separate_new_histories() {
    let s = empty("native-layers");
    fs::write(s.space.join("story"), b"base\n").unwrap();
    let base = step(&s);
    cli(&s, &["layer", "create", "feature"]);
    fs::write(s.space.join("story"), b"main\n").unwrap();
    let main = step(&s);
    cli(&s, &["layer", "use", "feature"]);
    assert_file(&s.space, "story", "base\n");
    fs::write(s.space.join("story"), b"feature\n").unwrap();
    let feature = step(&s);
    assert!(history(&s, "feature").contains(&base));
    assert!(!history(&s, "feature").contains(&main));
    assert!(!history(&s, "main").contains(&feature));
    cli(&s, &["layer", "use", "main"]);
    assert_file(&s.space, "story", "main\n");
    cli(&s, &["layer", "use", "feature"]);
    assert_file(&s.space, "story", "feature\n");
    s.pass();
}
#[test]
fn layer_creation_neither_switches_nor_propagates_later_parent_steps() {
    let s = empty("native-independent");
    let before = cli(&s, &["status"])["state"]["view"]["worktrees"].clone();
    cli(&s, &["layer", "create", "topic"]);
    assert_eq!(cli(&s, &["status"])["state"]["view"]["worktrees"], before);
    fs::write(s.space.join("parent-only"), b"new\n").unwrap();
    let parent = step(&s);
    cli(&s, &["layer", "use", "topic"]);
    assert!(!s.space.join("parent-only").exists());
    assert!(!history(&s, "topic").contains(&parent));
    s.pass();
}
#[test]
fn unsupported_legacy_mutations_preserve_native_files_and_history() {
    let s = empty("native-refuse-legacy");
    fs::write(s.space.join("keep"), b"preserve\n").unwrap();
    step(&s);
    cli(&s, &["layer", "create", "topic"]);
    let before = snapshot_working_tree(&s.space);
    let timeline = cli(&s, &["timeline"]);
    let layers = cli(&s, &["layers"]);
    for args in [
        vec!["--json", "layer", "clear-steps", "main", "--yes"],
        vec!["--json", "layer", "disconnect", "topic", "--yes"],
        vec!["--json", "layer", "delete", "topic", "--yes"],
        vec!["--json", "compact"],
    ] {
        assert!(run_err(&s, args).contains("not yet connected"));
        assert_working_tree_snapshot(&s.space, &before);
        assert_eq!(cli(&s, &["timeline"]), timeline);
        assert_eq!(cli(&s, &["layers"]), layers);
    }
    s.pass();
}
#[test]
fn switch_auto_captures_unstepped_edits_and_untracked_files() {
    let s = empty("native-auto-main");
    fs::write(s.space.join("story"), b"base\n").unwrap();
    step(&s);
    cli(&s, &["layer", "create", "topic"]);
    fs::write(s.space.join("story"), b"unsaved main\n").unwrap();
    fs::write(s.space.join("untracked"), b"also preserve\n").unwrap();
    cli(&s, &["layer", "use", "topic"]);
    assert_file(&s.space, "story", "base\n");
    assert!(!s.space.join("untracked").exists());
    let main_history = history(&s, "main");
    assert!(main_history.len() >= 3);
    cli(&s, &["layer", "use", "main"]);
    assert_file(&s.space, "story", "unsaved main\n");
    assert_file(&s.space, "untracked", "also preserve\n");
    assert!(paths(&changes(&s, Some(&main_history[0]))).contains("untracked"));
    s.pass();
}
#[test]
fn unstepped_feature_work_survives_switch_and_process_restart() {
    let s = empty("native-auto-topic");
    cli(&s, &["layer", "create", "topic"]);
    cli(&s, &["layer", "use", "topic"]);
    fs::write(s.space.join("draft"), b"unsaved topic\n").unwrap();
    cli(&s, &["layer", "use", "main"]);
    assert!(!s.space.join("draft").exists());
    cli(&s, &["layer", "use", "topic"]);
    assert_file(&s.space, "draft", "unsaved topic\n");
    assert_eq!(cli(&s, &["status"])["dirty"], false);
    s.pass();
}
#[test]
fn moving_a_file_is_delete_plus_add_and_switch_preserves_both_names() {
    let s = empty("native-move");
    fs::write(s.space.join("before"), b"exact\n").unwrap();
    step(&s);
    cli(&s, &["layer", "create", "moved"]);
    cli(&s, &["layer", "use", "moved"]);
    fs::rename(s.space.join("before"), s.space.join("after")).unwrap();
    let delta = changes(&s, None);
    assert_eq!(delta.len(), 2);
    assert!(
        delta
            .iter()
            .any(|v| v["path"] == "before" && v["status"] == "deleted")
    );
    assert!(
        delta
            .iter()
            .any(|v| v["path"] == "after" && v["status"] == "added")
    );
    step(&s);
    cli(&s, &["layer", "use", "main"]);
    assert_file(&s.space, "before", "exact\n");
    assert!(!s.space.join("after").exists());
    cli(&s, &["layer", "use", "moved"]);
    assert_file(&s.space, "after", "exact\n");
    assert!(!s.space.join("before").exists());
    s.pass();
}
#[test]
fn binary_capture_switch_restore_and_undo_preserve_every_byte() {
    let s = empty("native-binary");
    let first = deterministic_png_like_bytes();
    fs::write(s.space.join("asset"), &first).unwrap();
    let original = step(&s);
    cli(&s, &["layer", "create", "binary"]);
    cli(&s, &["layer", "use", "binary"]);
    let mut second = first.clone();
    second[90..100].copy_from_slice(&[0, 255, 1, 254, 2, 253, 3, 252, 4, 251]);
    fs::write(s.space.join("asset"), &second).unwrap();
    assert_eq!(changes(&s, None)[0]["binary"], true);
    let changed = step(&s);
    cli(&s, &["layer", "use", "main"]);
    assert_file_bytes(&s.space, "asset", &first);
    cli(&s, &["layer", "use", "binary"]);
    assert_file_bytes(&s.space, "asset", &second);
    cli(&s, &["restore", &original, "--path", "asset"]);
    assert_file_bytes(&s.space, "asset", &first);
    cli(&s, &["undo"]);
    assert_file_bytes(&s.space, "asset", &second);
    assert!(history(&s, "binary").contains(&changed));
    s.pass();
}
#[test]
fn timeline_limit_and_human_diff_flags_do_not_modify_history_or_files() {
    let s = empty("native-display");
    fs::write(s.space.join("text"), b"old\nlast").unwrap();
    step(&s);
    fs::write(s.space.join("text"), b"new\nlast").unwrap();
    let captured = step(&s);
    assert_eq!(
        cli(&s, &["timeline", "--limit", "1"])["operations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let timeline = cli(&s, &["timeline"]);
    let before = snapshot_working_tree(&s.space);
    assert_eq!(
        run_ok(&s, ["diff", &captured, "--name-only"]).trim(),
        "text"
    );
    let text = run_ok(&s, ["diff", &captured]);
    assert!(text.contains("-old"));
    assert!(text.contains("+new"));
    assert!(text.contains("No newline at end of file"));
    assert!(!run_ok(&s, ["diff", &captured, "--stat"]).contains("+new"));
    run_ok(&s, ["diff", &captured, "--window", "0:1", "--no-wrap"]);
    assert_eq!(cli(&s, &["timeline"]), timeline);
    assert_working_tree_snapshot(&s.space, &before);
    s.pass();
}
#[test]
fn missing_target_chunk_refuses_switch_before_deleting_current_files() {
    let s = empty("native-corrupt-chunk");
    fs::write(s.space.join("story"), b"main safe\n").unwrap();
    step(&s);
    cli(&s, &["layer", "create", "broken"]);
    cli(&s, &["layer", "use", "broken"]);
    let chunk_root = s.space.join(".layrs/objects/chunk_raw");
    let old = files(&chunk_root);
    fs::write(s.space.join("story"), b"unique broken target bytes\n").unwrap();
    step(&s);
    let new = files(&chunk_root);
    let added: Vec<_> = new.difference(&old).collect();
    assert_eq!(added.len(), 1);
    let missing = added[0].clone();
    cli(&s, &["layer", "use", "main"]);
    let before = snapshot_working_tree(&s.space);
    let state = cli(&s, &["status"])["state"].clone();
    fs::remove_file(missing).unwrap();
    run_err(&s, ["--json", "layer", "use", "broken"]);
    assert_working_tree_snapshot(&s.space, &before);
    assert_eq!(cli(&s, &["status"])["state"], state);
    s.pass();
}
#[test]
fn large_snapshot_history_stays_readable_and_noop_adds_no_objects() {
    let s = TestSpace::new("native-large-history");
    let original = deterministic_large_content(0);
    fs::write(s.space.join("big"), &original).unwrap();
    initialize(&s);
    let first = step(&s);
    for index in 1..=5 {
        fs::write(s.space.join("big"), deterministic_large_content(index)).unwrap();
        step(&s);
    }
    let objects = files(&s.space.join(".layrs/objects"));
    let timeline = cli(&s, &["timeline"]);
    assert_eq!(cli(&s, &["step"])["status"], "unchanged");
    assert_eq!(files(&s.space.join(".layrs/objects")), objects);
    assert_eq!(cli(&s, &["timeline"]), timeline);
    assert_eq!(changes(&s, Some(&first))[0]["afterBytes"], original.len());
    cli(&s, &["restore", &first, "--path", "big"]);
    assert_file(&s.space, "big", &original);
    assert!(history(&s, "main").contains(&first));
    s.pass();
}
#[test]
fn restore_auto_captures_untracked_replacement_and_undo_recovers_it() {
    let s = empty("native-restore-collision");
    fs::write(s.space.join("file"), b"original\n").unwrap();
    let first = step(&s);
    fs::remove_file(s.space.join("file")).unwrap();
    step(&s);
    fs::write(s.space.join("file"), b"untracked replacement\n").unwrap();
    fs::write(s.space.join("other"), b"independent\n").unwrap();
    cli(&s, &["restore", &first, "--path", "file"]);
    assert_file(&s.space, "file", "original\n");
    assert_file(&s.space, "other", "independent\n");
    cli(&s, &["undo"]);
    assert_file(&s.space, "file", "untracked replacement\n");
    assert_file(&s.space, "other", "independent\n");
    s.pass();
}
#[test]
fn invalid_selectors_and_escape_paths_preserve_checkout_and_history() {
    let s = empty("native-invalid-target");
    fs::write(s.space.join("keep"), b"safe\n").unwrap();
    let first = step(&s);
    let before = snapshot_working_tree(&s.space);
    let timeline = cli(&s, &["timeline"]);
    for args in [
        vec!["--json", "layer", "use", "missing"],
        vec!["--json", "restore", "not-a-step"],
        vec!["--json", "restore", &first, "--path", "../outside"],
    ] {
        run_err(&s, args);
        assert_working_tree_snapshot(&s.space, &before);
        assert_eq!(cli(&s, &["timeline"]), timeline);
    }
    s.pass();
}
