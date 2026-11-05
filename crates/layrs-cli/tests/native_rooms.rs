//! `layrs room` (ADR 0025) across process boundaries: physical Rooms, so the
//! test runs on every platform. Virtual Rooms need a Windows CFAPI host.
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "layrs-cli-rooms-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("project")).unwrap();
        Self(root)
    }
    fn project(&self) -> PathBuf {
        self.0.join("project")
    }
    /// Rooms of the test live in the fixture, never in the user's profile.
    fn room_dir(&self, name: &str) -> String {
        self.0.join("rooms").join(name).to_str().unwrap().to_owned()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.0.parent() == Some(std::env::temp_dir().as_path()) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

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
    assert_eq!(envelope["ok"], success);
    if success {
        envelope["data"].clone()
    } else {
        envelope["error"].clone()
    }
}

fn init(fixture: &Fixture) -> PathBuf {
    let project = fixture.project();
    fs::create_dir(project.join("src")).unwrap();
    fs::write(project.join("src/a.txt"), b"a\n").unwrap();
    fs::write(project.join("src/b.txt"), b"b\n").unwrap();
    fs::write(project.join("readme.txt"), b"readme\n").unwrap();
    run(&project, &["init", "Rooms"], true);
    project
}

#[test]
fn physical_room_new_step_merge_drop_and_recreate() {
    let fixture = Fixture::new();
    let project = init(&fixture);
    let dir = fixture.room_dir("agent-a");
    let created = run(
        &project,
        &["room", "new", "agent-a", "--physical", "--dir", &dir],
        true,
    );
    assert_eq!(created["room"]["kind"], "physical");
    assert_eq!(created["room"]["layerName"], "room/agent-a");
    let room = PathBuf::from(created["room"]["path"].as_str().unwrap());
    assert_eq!(fs::read(room.join("src/a.txt")).unwrap(), b"a\n");
    assert_eq!(
        run(&project, &["loom", "path", "agent-a"], true)["room"]["path"],
        created["room"]["path"]
    );

    // Work in the Room; the project is untouched until the merge.
    fs::write(room.join("src/a.txt"), b"a changed by the agent\n").unwrap();
    fs::write(room.join("src/new.txt"), b"new\n").unwrap();
    let step = run(&room, &["room", "step"], true);
    assert_eq!(step["step"]["changed"], true);
    assert_eq!(fs::read(project.join("src/a.txt")).unwrap(), b"a\n");

    // Nothing merged yet: drop refuses and keeps everything.
    let refused = run(&project, &["room", "drop", "agent-a"], false);
    assert!(refused["message"].as_str().unwrap().contains("not merged"));
    assert!(room.join("src/new.txt").exists());

    let merged = run(&project, &["room", "merge", "agent-a"], true);
    assert_eq!(merged["merged"], true);
    assert_eq!(merged["fastForward"], true);
    assert_eq!(
        fs::read(project.join("src/a.txt")).unwrap(),
        b"a changed by the agent\n"
    );
    assert_eq!(fs::read(project.join("src/new.txt")).unwrap(), b"new\n");
    assert_eq!(
        run(&project, &["room", "merge", "agent-a"], true)["merged"],
        false,
        "nothing new to merge"
    );

    run(&project, &["room", "drop", "agent-a"], true);
    assert!(!room.exists());
    assert_eq!(
        run(&project, &["room", "list"], true)["rooms"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    // The dropped Room's Worktree is retired: the same name and folder work again.
    let again = run(
        &project,
        &["room", "new", "agent-a", "--physical", "--dir", &dir],
        true,
    );
    assert_eq!(again["room"]["layerName"], "room/agent-a-2");
    run(&project, &["room", "drop", "agent-a"], true);
}

#[test]
fn second_room_merges_through_a_weave_after_the_first() {
    let fixture = Fixture::new();
    let project = init(&fixture);
    let b_dir = fixture.room_dir("agent-b");
    let c_dir = fixture.room_dir("agent-c");
    let b = PathBuf::from(
        run(&project, &["room", "new", "agent-b", "--physical", "--dir", &b_dir], true)["room"]
            ["path"]
            .as_str()
            .unwrap(),
    );
    let c = PathBuf::from(
        run(&project, &["room", "new", "agent-c", "--physical", "--dir", &c_dir], true)["room"]
            ["path"]
            .as_str()
            .unwrap(),
    );
    fs::write(b.join("src/a.txt"), b"a by b\n").unwrap();
    fs::write(c.join("src/b.txt"), b"b by c\n").unwrap();
    assert_eq!(
        run(&project, &["room", "merge", "agent-b"], true)["fastForward"],
        true
    );
    // main moved: agent-c needs a real Weave, still conflict-free here.
    let merged = run(&project, &["room", "merge", "agent-c"], true);
    assert_eq!(merged["merged"], true);
    assert_eq!(fs::read(project.join("src/a.txt")).unwrap(), b"a by b\n");
    assert_eq!(fs::read(project.join("src/b.txt")).unwrap(), b"b by c\n");
    run(&project, &["room", "drop", "agent-b"], true);
    run(&project, &["room", "drop", "agent-c"], true);
}

/// ADR 0025: a Room's captures trust its change index like git (an edit that
/// keeps the exact size and dates is not seen by `step`), but `drop` reads
/// every byte first, so that edit is captured and the drop refused.
#[test]
fn room_step_trusts_the_index_but_drop_verifies_every_byte() {
    let fixture = Fixture::new();
    let project = init(&fixture);
    let dir = fixture.room_dir("agent-i");
    let created = run(
        &project,
        &["room", "new", "agent-i", "--physical", "--dir", &dir],
        true,
    );
    let room = PathBuf::from(created["room"]["path"].as_str().unwrap());
    // Let the files written at creation leave the racy window, then prove them.
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(run(&room, &["room", "step"], true)["step"]["changed"], false);

    // Same length, same modification time: invisible to a stat comparison.
    let file = room.join("src/a.txt");
    let modified = fs::metadata(&file).unwrap().modified().unwrap();
    {
        use std::io::Write;
        let mut handle = fs::OpenOptions::new().write(true).open(&file).unwrap();
        handle.write_all(b"Z\n").unwrap();
        handle.set_modified(modified).unwrap();
    }
    assert_eq!(fs::metadata(&file).unwrap().modified().unwrap(), modified);
    assert_eq!(
        run(&room, &["room", "step"], true)["step"]["changed"],
        false,
        "a git-like step trusts the unchanged stat"
    );

    // Dropping verifies content: the edit is found, kept, and the drop refused.
    let refused = run(&project, &["room", "drop", "agent-i"], false);
    assert!(refused["message"].as_str().unwrap().contains("not merged"));
    assert_eq!(fs::read(&file).unwrap(), b"Z\n");
    run(&project, &["room", "merge", "agent-i"], true);
    assert_eq!(fs::read(project.join("src/a.txt")).unwrap(), b"Z\n");

    // A real edit (new size) is seen by a plain step.
    fs::write(room.join("src/b.txt"), b"b, edited by the agent\n").unwrap();
    assert_eq!(run(&room, &["room", "step"], true)["step"]["changed"], true);
    run(&project, &["room", "merge", "agent-i"], true);
    run(&project, &["room", "drop", "agent-i"], true);
    assert!(!room.exists());
}

#[test]
fn room_names_and_folders_are_validated() {
    let fixture = Fixture::new();
    let project = init(&fixture);
    run(&project, &["room", "new", "../escape", "--physical"], false);
    let occupied = fixture.room_dir("occupied");
    fs::create_dir_all(&occupied).unwrap();
    fs::write(Path::new(&occupied).join("keep.txt"), b"keep").unwrap();
    run(
        &project,
        &["room", "new", "occupied", "--physical", "--dir", &occupied],
        false,
    );
    assert_eq!(
        fs::read(Path::new(&occupied).join("keep.txt")).unwrap(),
        b"keep"
    );
    run(&project, &["room", "drop", "missing"], false);
}
