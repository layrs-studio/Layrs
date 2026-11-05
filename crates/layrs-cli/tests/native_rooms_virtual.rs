//! Virtual Rooms (ADR 0025) on real Windows CFAPI. Opt-in: run explicitly with
//! LAYRS_VFS_TEST_PARENT set to an existing disposable local NTFS directory.
//! Covers change detection without reopening files (in-sync placeholders and
//! the change index), `.layrsignore`, renames inside the Room, and `drop`.
#![cfg(windows)]
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn run(root: &Path, args: &[&str], success: bool) -> Value {
    let result = Command::new(env!("CARGO_BIN_EXE_layrs"))
        .current_dir(root)
        .arg("--json")
        .args(args)
        .output()
        .unwrap();
    assert_eq!(
        result.status.success(),
        success,
        "{args:?}: {} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let envelope: Value = serde_json::from_slice(&result.stdout).unwrap();
    if success {
        envelope["data"].clone()
    } else {
        envelope["error"].clone()
    }
}

fn fixture() -> PathBuf {
    let parent = PathBuf::from(
        std::env::var_os("LAYRS_VFS_TEST_PARENT")
            .expect("set an explicit disposable LAYRS_VFS_TEST_PARENT"),
    );
    assert!(parent.is_absolute() && parent.is_dir());
    let root = parent.join(format!(
        "virtual-rooms-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join("project/src")).unwrap();
    eprintln!("virtual Room fixture retained: {}", root.display());
    root
}

#[test]
#[ignore = "real CFAPI virtual Room; explicit disposable LAYRS_VFS_TEST_PARENT required"]
fn virtual_room_detects_edits_renames_and_ignores_without_reopening_files() {
    let root = fixture();
    let project = root.join("project");
    fs::write(project.join("src/a.txt"), b"a\n").unwrap();
    fs::write(project.join("src/b.txt"), b"b content\n").unwrap();
    fs::write(project.join("readme.txt"), b"readme\n").unwrap();
    fs::write(project.join(".layrsignore"), b"build/\n").unwrap();
    run(&project, &["init", "Virtual Rooms"], true);
    let dir = root.join("rooms").join("agent-v");
    let created = run(
        &project,
        &["room", "new", "agent-v", "--dir", dir.to_str().unwrap()],
        true,
    );
    assert_eq!(created["room"]["kind"], "virtual");
    let room = PathBuf::from(created["room"]["path"].as_str().unwrap());

    // Reading every file hydrates it; nothing changed.
    for file in ["src/a.txt", "src/b.txt", "readme.txt"] {
        assert_eq!(
            fs::read(room.join(file)).unwrap(),
            fs::read(project.join(file)).unwrap()
        );
    }
    let step = run(&room, &["room", "step"], true);
    assert_eq!(step["step"]["changed"], false, "{step}");

    // A same-size edit clears the placeholder's in-sync state: it is seen.
    fs::write(room.join("src/a.txt"), b"A\n").unwrap();
    assert_eq!(run(&room, &["room", "step"], true)["step"]["changed"], true);
    // Unchanged again: the hashed file is now known to the change index.
    assert_eq!(
        run(&room, &["room", "step"], true)["step"]["changed"],
        false
    );

    // New files: an ignored build output is neither read nor captured.
    fs::create_dir_all(room.join("build")).unwrap();
    fs::write(room.join("build/out.bin"), vec![7u8; 4096]).unwrap();
    fs::write(room.join("src/new.txt"), b"new\n").unwrap();
    // A rename inside the Room is accepted and captured at its new path.
    fs::rename(room.join("src/b.txt"), room.join("src/b2.txt")).unwrap();
    assert_eq!(run(&room, &["room", "step"], true)["step"]["changed"], true);

    let merged = run(&project, &["room", "merge", "agent-v"], true);
    assert_eq!(merged["merged"], true, "{merged}");
    assert_eq!(fs::read(project.join("src/a.txt")).unwrap(), b"A\n");
    assert_eq!(fs::read(project.join("src/new.txt")).unwrap(), b"new\n");
    assert_eq!(
        fs::read(project.join("src/b2.txt")).unwrap(),
        b"b content\n"
    );
    assert!(!project.join("src/b.txt").exists());
    assert!(
        !project.join("build").exists(),
        "ignored output must not be merged"
    );

    // Merged and verified: the drop succeeds and the folder is gone at once.
    let dropped = run(&project, &["room", "drop", "agent-v"], true);
    assert_eq!(dropped["dropped"], true);
    assert!(!room.exists());
    assert_eq!(
        run(&project, &["room", "list"], true)["rooms"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
#[ignore = "real CFAPI virtual Room; explicit disposable LAYRS_VFS_TEST_PARENT required"]
fn virtual_room_drop_keeps_unmerged_work() {
    let root = fixture();
    let project = root.join("project");
    fs::write(project.join("src/a.txt"), b"a\n").unwrap();
    run(&project, &["init", "Virtual Drop"], true);
    let dir = root.join("rooms").join("agent-w");
    let room = PathBuf::from(
        run(
            &project,
            &["room", "new", "agent-w", "--dir", dir.to_str().unwrap()],
            true,
        )["room"]["path"]
            .as_str()
            .unwrap(),
    );
    fs::write(room.join("src/a.txt"), b"unmerged\n").unwrap();
    let refused = run(&project, &["room", "drop", "agent-w"], false);
    assert!(refused["message"].as_str().unwrap().contains("not merged"));
    assert_eq!(fs::read(room.join("src/a.txt")).unwrap(), b"unmerged\n");
    // --force drops the folder; the work stays on the Room's Layer.
    let forced = run(&project, &["room", "drop", "agent-w", "--force"], true);
    assert_eq!(forced["unmergedKept"], true);
    assert!(!room.exists());
}
