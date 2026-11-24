use layrs_client_core::native_scm::NativeWorkspace;
use layrs_store_local::scm_v1::compaction::*;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        // Tests run in parallel: the clock alone can repeat (100 ns on
        // Windows), a counter cannot.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "layrs-cli-secondary-maintenance-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn directory(&self, name: &str) -> PathBuf {
        let root = self.0.join(name);
        fs::create_dir(&root).unwrap();
        root
    }
    fn workspaces(&self) -> (NativeWorkspace, NativeWorkspace) {
        let primary_root = self.directory("primary");
        fs::write(primary_root.join("file"), b"base").unwrap();
        let primary = NativeWorkspace::initialize(&primary_root, "Maintenance").unwrap();
        let state = primary.state().unwrap();
        let own = state
            .view
            .worktrees()
            .iter()
            .find(|entry| entry.worktree_id() == primary.worktree_id())
            .unwrap();
        let secondary = primary
            .create_worktree(
                self.directory("secondary"),
                own.active_layer_id().unwrap(),
                own.base_step_id().unwrap(),
            )
            .unwrap();
        (primary, secondary)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.0.parent() == Some(std::env::temp_dir().as_path()) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn run(root: &Path, args: &[&str], success: bool) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_layrs"))
        .current_dir(root)
        .arg("--json")
        .args(args)
        .output()
        .unwrap();
    assert_eq!(
        output.status.success(),
        success,
        "{args:?}: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], success);
    value["data"].clone()
}
struct Interrupt;
impl CompactionFaults for Interrupt {
    fn check(&self, point: CompactionFaultPoint) -> CompactionResult<()> {
        if point == CompactionFaultPoint::JournalDurable {
            return Err("simulated interruption".into());
        }
        Ok(())
    }
}

struct InterruptAt(CompactionFaultPoint);
impl CompactionFaults for InterruptAt {
    fn check(&self, point: CompactionFaultPoint) -> CompactionResult<()> {
        if point == self.0 {
            Err("simulated interruption".into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn secondary_compaction_and_resume_preserve_both_private_views_and_pending_edits() {
    let fixture = Fixture::new();
    let (primary, secondary) = fixture.workspaces();
    let primary_root = primary.checkout_root().unwrap();
    let secondary_root = secondary.checkout_root().unwrap();
    let nested = secondary_root.join("nested");
    fs::create_dir(&nested).unwrap();
    secondary.create_layer("private").unwrap();
    let before_primary = serde_json::to_value(primary.state().unwrap()).unwrap();
    let before_secondary = serde_json::to_value(secondary.state().unwrap()).unwrap();
    let locator = fs::read(secondary_root.join(".layrs")).unwrap();
    fs::write(primary_root.join("file"), b"primary edits").unwrap();
    fs::write(secondary_root.join("file"), b"secondary edits").unwrap();
    let compacted = run(&nested, &["compact"], true);
    assert_eq!(compacted["historyDeleted"], false);
    assert_eq!(compacted["storeId"], before_primary["worktree"]["storeId"]);
    for point in [
        CompactionFaultPoint::JournalDurable,
        CompactionFaultPoint::DatabasePublished,
    ] {
        assert!(compact_metadata_with_faults(primary.store_root(), &InterruptAt(point)).is_err());
        run(&secondary_root, &["status"], false);
        let resumed = run(&nested, &["compact", "--resume"], true);
        assert_eq!(resumed["storeId"], before_primary["worktree"]["storeId"]);
        assert_eq!(
            serde_json::to_value(primary.state().unwrap()).unwrap(),
            before_primary
        );
        assert_eq!(
            serde_json::to_value(secondary.state().unwrap()).unwrap(),
            before_secondary
        );
    }
    assert_eq!(
        fs::read(primary_root.join("file")).unwrap(),
        b"primary edits"
    );
    assert_eq!(
        fs::read(secondary_root.join("file")).unwrap(),
        b"secondary edits"
    );
    assert_eq!(fs::read(secondary_root.join(".layrs")).unwrap(), locator);
    run(&secondary_root, &["doctor"], true);
    run(&secondary_root, &["compact", "--resume"], false);
}

#[test]
fn secondary_maintenance_refuses_unknown_locators_without_touching_the_store() {
    let fixture = Fixture::new();
    let (primary, secondary) = fixture.workspaces();
    let secondary_root = secondary.checkout_root().unwrap();
    let copied = fixture.directory("copied");
    let unknown = fixture.directory("unknown");
    let original = fs::read(secondary_root.join(".layrs")).unwrap();
    fs::write(copied.join(".layrs"), &original).unwrap();
    fs::write(unknown.join(".layrs"), b"not a supported locator").unwrap();
    let metadata_path = primary.store_root().join("metadata/layrs.sqlite3");
    let before = fs::read(&metadata_path).unwrap();
    for root in [&copied, &unknown] {
        for args in [
            &["compact"][..],
            &["compact", "--resume"][..],
            &["recover"][..],
        ] {
            run(root, args, false);
            assert_eq!(fs::read(&metadata_path).unwrap(), before);
        }
        assert_eq!(fs::read_dir(root).unwrap().count(), 1);
    }
    assert_eq!(fs::read(secondary_root.join(".layrs")).unwrap(), original);
    assert_eq!(fs::read(secondary_root.join("file")).unwrap(), b"base");
    assert_eq!(
        fs::read(primary.checkout_root().unwrap().join("file")).unwrap(),
        b"base"
    );
}

#[test]
fn cli_compacts_from_nested_checkout_and_resumes_a_fenced_store_without_losing_history() {
    let fixture = Fixture(std::env::temp_dir().join(format!(
            "layrs-cli-maintenance-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
    fs::create_dir(&fixture.0).unwrap();
    let nested = fixture.0.join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(fixture.0.join("file"), b"initial").unwrap();
    let workspace = NativeWorkspace::initialize(&fixture.0, "Maintenance").unwrap();
    let marker = workspace.create_marker("initial", "").unwrap();
    fs::write(fixture.0.join("file"), b"released").unwrap();
    let release = workspace.create_release("v1", "").unwrap();
    let before = serde_json::to_value(workspace.state().unwrap()).unwrap();
    let marker_files = workspace.read_step_files(marker.target_step_id()).unwrap();
    let release_files = workspace.read_step_files(release.target_step_id()).unwrap();
    fs::write(fixture.0.join("file"), b"unrecorded").unwrap();
    let report = run(&nested, &["compact"], true);
    assert_eq!(report["historyDeleted"], false);
    // Format 8 cursors are history sequences, which compaction keeps.
    let format = layrs_store_local::scm_v1::open(workspace.store_root())
        .unwrap()
        .format_version();
    assert_eq!(
        report["restartHistoryTraversal"],
        format != layrs_store_local::scm_v1::HISTORY_FORMAT_VERSION
    );
    assert!(report["retainedRows"].as_u64().unwrap() > 0);
    assert!(
        Path::new(report["backupDirectory"].as_str().unwrap())
            .join("before.sqlite3")
            .is_file()
    );
    assert!(compact_metadata_with_faults(workspace.store_root(), &Interrupt).is_err());
    run(&fixture.0, &["status"], false);
    run(&nested, &["compact", "--resume"], true);
    let reopened = NativeWorkspace::open(&fixture.0).unwrap();
    assert_eq!(
        serde_json::to_value(reopened.state().unwrap()).unwrap(),
        before
    );
    assert_eq!(
        reopened.read_step_files(marker.target_step_id()).unwrap(),
        marker_files
    );
    assert_eq!(
        reopened.read_step_files(release.target_step_id()).unwrap(),
        release_files
    );
    assert_eq!(fs::read(fixture.0.join("file")).unwrap(), b"unrecorded");
    assert!(reopened.is_dirty().unwrap());
    run(&fixture.0, &["doctor"], true);
    run(&fixture.0, &["compact", "--resume"], false);
}

/// `layrs maintenance` runs one pack maintenance pass on the Store a
/// checkout uses, from a secondary worktree too (council s17, B2b).
#[test]
fn cli_maintenance_runs_a_pass_on_the_checkout_store() {
    let fixture = Fixture::new();
    let (primary, secondary) = fixture.workspaces();
    let primary = primary.checkout_root().unwrap();
    let secondary = secondary.checkout_root().unwrap();
    for n in 0..4 {
        fs::write(primary.join(format!("f{n}")), format!("content {n}")).unwrap();
        run(&primary, &["step"], true);
    }
    for root in [&primary, &secondary] {
        let data = run(root, &["maintenance"], true);
        assert!(
            matches!(
                data["status"].as_str(),
                Some("nothingToDo" | "consolidated")
            ),
            "{data}"
        );
    }
    run(&primary, &["status"], true);
    run(&secondary, &["status"], true);
    assert!(
        run(&primary, &["doctor"], true)["findings"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );
}

/// Phase 3e (council s35): in Store format 8, `layrs maintenance` rewrites
/// the metadata database once it has grown (and says so), a second run finds
/// it not due, and the detached run rewrites it too; in other formats the
/// output has no `metadata` field.
#[test]
fn format_8_maintenance_rewrites_the_metadata() {
    let fixture = Fixture::new();
    let checkout = fixture.directory("format-8");
    fs::write(checkout.join("file"), b"base").unwrap();
    let layrs_in = |dir: &Path, args: &[&std::ffi::OsStr], format: &str| {
        Command::new(env!("CARGO_BIN_EXE_layrs"))
            .current_dir(dir)
            .env("LAYRS_STORE_FORMAT", format)
            .env("LAYRS_AUTO_MAINTENANCE", "0")
            .args(args)
            .output()
            .unwrap()
    };
    let layrs = |args: &[&std::ffi::OsStr], format: &str| layrs_in(&checkout, args, format);
    let json_in = |dir: &Path, args: &[&str], format: &str| -> Value {
        let mut all: Vec<&std::ffi::OsStr> = vec![std::ffi::OsStr::new("--json")];
        all.extend(args.iter().map(std::ffi::OsStr::new));
        let output = layrs_in(dir, &all, format);
        assert!(
            output.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["data"].clone()
    };
    let json = |args: &[&str]| json_in(&checkout, args, "8");
    json(&["init", "Format 8"]);
    for n in 0..3 {
        fs::write(checkout.join(format!("f{n}")), format!("content {n}")).unwrap();
        json(&["step"]);
    }
    let store = checkout.join(".layrs");
    let note = store.join("metadata").join("metadata-rewritten");
    let data = json(&["maintenance"]);
    assert_eq!(data["metadata"]["status"], "rewritten", "{data}");
    assert!(note.exists());
    let data = json(&["maintenance"]);
    assert_eq!(data["metadata"]["status"], "notDue", "{data}");
    fs::remove_file(&note).unwrap();
    let command = std::ffi::OsStr::new(layrs_cli::native::MAINTENANCE_COMMAND);
    let detached = layrs(&[command, store.as_os_str()], "8");
    assert_eq!(detached.status.code(), Some(0));
    assert!(note.exists(), "the detached run rewrote nothing");
    let data = json(&["status"]);
    assert!(data.is_object(), "{data}");
    // Format 7: no rewrite, no field.
    let other = fixture.directory("format-7");
    fs::write(other.join("file"), b"base").unwrap();
    json_in(&other, &["init", "Format 7"], "7");
    let data = json_in(&other, &["maintenance"], "7");
    assert!(data.get("metadata").is_none(), "{data}");
    assert!(!other.join(".layrs/metadata/metadata-rewritten").exists());
}

/// Format 8 (council s36): an explicit `layrs maintenance` consolidates
/// every pack, inline or not, into one, as `git gc` repacks all into one, and
/// removes the cached projections no Worktree or Room stands on; the next
/// Steps in the project and in a Room on an older Step still find theirs.
/// The detached run trims too; format 7 has neither.
#[test]
fn format_8_explicit_maintenance_packs_all_into_one_and_trims_projections() {
    let fixture = Fixture::new();
    let checkout = fixture.directory("format-8");
    fs::create_dir(checkout.join("src")).unwrap();
    for n in 0..5 {
        fs::write(checkout.join(format!("src/f{n}.txt")), format!("f{n}\n")).unwrap();
    }
    let layrs_in = |dir: &Path, args: &[&str], format: &str, perf: Option<&Path>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_layrs"));
        command
            .current_dir(dir)
            .env("LAYRS_STORE_FORMAT", format)
            .env("LAYRS_AUTO_MAINTENANCE", "0")
            .arg("--json")
            .args(args);
        if let Some(perf) = perf {
            command.env("LAYRS_PERF", "1").env("LAYRS_PERF_OUT", perf);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["data"].clone()
    };
    let json = |args: &[&str]| layrs_in(&checkout, args, "8", None);
    let append = |dir: &Path, n: usize, text: &str| {
        let path = dir.join(format!("src/f{n}.txt"));
        let mut bytes = fs::read(&path).unwrap();
        bytes.extend_from_slice(text.as_bytes());
        fs::write(&path, bytes).unwrap();
    };
    json(&["init", "Format 8"]);
    for k in 0..12 {
        append(&checkout, k % 5, &format!("e{k}\n"));
        json(&["step"]);
    }
    let room = fixture.0.join("room");
    json(&["room", "new", "agent", "--dir", room.to_str().unwrap()]);
    for k in 12..15 {
        append(&checkout, 1, &format!("e{k}\n"));
        json(&["step"]);
    }
    let store = checkout.join(".layrs");
    let metadata = store.join("metadata");
    let projections = || -> i64 {
        rusqlite::Connection::open(metadata.join("derived.sqlite3"))
            .unwrap()
            .query_row("SELECT count(*) FROM projection", [], |row| row.get(0))
            .unwrap()
    };
    let before = projections();
    assert!(before > 2, "{before}");
    let data = json(&["maintenance"]);
    assert_eq!(data["status"], "consolidated", "{data}");
    assert!(data["packs"].as_u64().unwrap() >= 2, "{data}");
    assert_eq!(data["projections"]["status"], "trimmed", "{data}");
    let removed = data["projections"]["removed"].as_i64().unwrap();
    assert!(removed > 0, "{data}");
    assert_eq!(projections(), before - removed);
    // One pack, a file: no inline pack is left.
    let packs: (i64, i64) = rusqlite::Connection::open(metadata.join("layrs.sqlite3"))
        .unwrap()
        .query_row(
            "SELECT count(*), coalesce(sum(stored_inline), 0) FROM cas_packs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(packs, (1, 0));
    // The next Steps find the projections they stand on.
    for (dir, args, label) in [
        (&checkout, &["step"][..], "project"),
        (&room, &["room", "step"][..], "room"),
    ] {
        append(dir, 3, &format!("after maintenance, {label}\n"));
        let perf = fixture.0.join(format!("{label}.perf"));
        layrs_in(dir, args, "8", Some(&perf));
        let report = fs::read_to_string(&perf).unwrap();
        // A hit (debug builds check every hit against a walk).
        assert!(
            report.contains("\"projection_cache.hit\"")
                || report.contains("\"projection_cache.sampled_ok\""),
            "{label}: {report}"
        );
    }
    let data = json(&["status"]);
    assert_eq!(data["dirty"], false, "{data}");
    // The detached run trims too.
    for k in 15..20 {
        append(&checkout, k % 5, &format!("e{k}\n"));
        json(&["step"]);
    }
    let before = projections();
    let detached = Command::new(env!("CARGO_BIN_EXE_layrs"))
        .args([
            std::ffi::OsStr::new(layrs_cli::native::MAINTENANCE_COMMAND),
            store.as_os_str(),
        ])
        .status()
        .unwrap();
    assert_eq!(detached.code(), Some(0));
    assert!(projections() < before);
    // Format 7: no trim, no field.
    let other = fixture.directory("format-7");
    fs::write(other.join("file"), b"base").unwrap();
    layrs_in(&other, &["init", "Format 7"], "7", None);
    let data = layrs_in(&other, &["maintenance"], "7", None);
    assert!(data.get("projections").is_none(), "{data}");
}

/// The detached maintenance run a batch starts takes the Store root as its
/// only argument and exits 0 (council s17, B2c).
#[test]
fn detached_maintenance_runs_on_a_store_root() {
    let fixture = Fixture::new();
    let (primary, _secondary) = fixture.workspaces();
    let status = |args: &[&std::ffi::OsStr]| {
        Command::new(env!("CARGO_BIN_EXE_layrs"))
            .args(args)
            .status()
            .unwrap()
            .code()
    };
    let command = std::ffi::OsStr::new(layrs_cli::native::MAINTENANCE_COMMAND);
    assert_eq!(
        status(&[command, primary.store_root().as_os_str()]),
        Some(0)
    );
    assert_eq!(status(&[command]), Some(2));
    let checkout = primary.checkout_root().unwrap();
    run(&checkout, &["status"], true);
}
