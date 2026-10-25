//! Black-box local-only parity: exact history, bounded bytes and explicit copy conversion.
use layrs_core::{
    projection::{
        AvailabilityRequest, EntryState, OfflineOperation, PathSemantics, ProjectionEntry,
        ProjectionSelection, ProjectionSnapshot,
    },
    scm::{
        FileId, ObjectId, RepoPath, SpaceId, StepId, StoreId, TreeEntryTarget, TreeId, WorktreeId,
    },
};
use layrs_store_local::projection::{
    ConversionFaults, ConversionPoint, PartialStoreFamily, PartialStoreIdentity,
    ProjectionIdentity, ProjectionStore, SharedQuota,
};
use serde_json::{Value, json};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

const BASE: &[u8] = &[0, 255, 27, b'\n'];
#[path = "offline/imported_history.rs"]
mod imported_history;
#[path = "offline/recovery.rs"]
mod recovery;
#[path = "offline/restore.rs"]
mod restore;
struct Fixture {
    outer: PathBuf,
    root: PathBuf,
}
impl Fixture {
    fn new(shared: bool) -> (Self, ProjectionStore) {
        let outer = std::env::temp_dir().join(format!(
            "layrs-cli-offline-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = outer.join("checkout");
        fs::create_dir_all(&root).unwrap();
        let identity = ProjectionIdentity {
            store_id: StoreId::parse("store:offline").unwrap(),
            worktree_id: WorktreeId::parse("worktree:offline").unwrap(),
            space_id: SpaceId::parse("space:offline").unwrap(),
            origin: "https://example.test".into(),
            principal_id: "principal:offline".into(),
        };
        let hash = format!("blake3:{}", "1".repeat(64));
        let initial = ProjectionSnapshot {
            schema_version: "layrs.projection-snapshot.v1".into(),
            base_step_id: StepId::parse(&hash).unwrap(),
            base_root_tree_id: TreeId::parse(&hash).unwrap(),
            observed_policy_set_id: ObjectId::parse(&hash).unwrap(),
            observed_authorization_epoch: 1,
            path_semantics: PathSemantics::PortableWindows,
            entries: vec![entry("a.bin", BASE), entry("later.txt", b"not acquired")],
        };
        let mut installed = initial.clone();
        installed.entries[0].state = EntryState::Materialized;
        let store = if shared {
            let family_root = outer.join("family");
            fs::create_dir(&family_root).unwrap();
            let family = PartialStoreFamily::create(
                family_root,
                PartialStoreIdentity {
                    store_id: identity.store_id.clone(),
                    space_id: identity.space_id.clone(),
                    origin: identity.origin.clone(),
                    principal_id: identity.principal_id.clone(),
                },
                SharedQuota {
                    bytes: 16 * 1024 * 1024,
                },
            )
            .unwrap();
            family
                .create_worktree(
                    &root,
                    identity.worktree_id,
                    initial,
                    ProjectionSelection::All,
                )
                .unwrap()
        } else {
            ProjectionStore::create(&root, identity, initial).unwrap()
        };
        store
            .install_projection(0, &installed, &mut |path, out| {
                assert_eq!(path, "a.bin");
                out.write_all(BASE)
            })
            .unwrap();
        (Self { outer, root }, store)
    }
    fn run(&self, args: &[&str], success: bool) -> Value {
        self.at(&self.root, args, success)
    }
    fn at(&self, root: &Path, args: &[&str], success: bool) -> Value {
        let output = Command::new(env!("CARGO_BIN_EXE_layrs"))
            .current_dir(root)
            .arg("--json")
            .args(args)
            .env("LAYRS_SERVER_ENDPOINT", "http://127.0.0.1:9")
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            success,
            "{args:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let document: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(document["ok"], success);
        document
    }
    fn empty(&self, name: &str) -> PathBuf {
        let p = self.outer.join(name);
        fs::create_dir(&p).unwrap();
        p
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.outer.parent() == Some(std::env::temp_dir().as_path())
        {
            fs::remove_dir_all(&self.outer).unwrap();
        }
    }
}
fn entry(path: &str, bytes: &[u8]) -> ProjectionEntry {
    ProjectionEntry {
        path: RepoPath::parse(path).unwrap(),
        target: Some(TreeEntryTarget::file(FileId::from_bytes(bytes))),
        executable: false,
        writable: true,
        state: EntryState::Unavailable,
    }
}
fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn offline_current_and_history_keep_exact_bytes_without_acquiring_missing_content() {
    let (f, store) = Fixture::new(true);
    let nested = f.root.join("nested");
    fs::create_dir(&nested).unwrap();
    let status = f.at(&nested, &["partial", "offline", "status"], true);
    assert_eq!(
        status["data"]["version"],
        serde_json::to_value(store.current_version().unwrap()).unwrap()
    );
    // Discovery accepts a nested cwd, but capture must reject unobserved paths.
    assert!(
        store
            .capture_patch(1)
            .unwrap_err()
            .to_string()
            .contains("path has no observed write authority: nested")
    );
    // This exact, empty directory was created solely for the discovery assertion.
    fs::remove_dir(&nested).unwrap();
    let missing = f.run(&["partial", "offline", "check", "read", "later.txt"], true);
    assert_eq!(missing["data"]["available"], false);
    assert!(!f.root.join("later.txt").exists());
    fs::write(f.root.join("a.bin"), b"captured bytes").unwrap();
    let capture = store.capture_patch(1).unwrap();
    let selector = format!("capture:{}", capture.sequence);
    for (version, expected, name) in [
        ("current", BASE, "current.bin"),
        (
            selector.as_str(),
            b"captured bytes".as_slice(),
            "captured.bin",
        ),
        ("generation:0", BASE, "generation.bin"),
    ] {
        let output = f.outer.join(name);
        let read = f.run(
            &[
                "partial",
                "offline",
                "read",
                "a.bin",
                "--version",
                version,
                "--output",
                text(&output),
            ],
            true,
        );
        assert_eq!(fs::read(&output).unwrap(), expected);
        assert!(read["data"].get("content").is_none());
        f.run(
            &[
                "partial",
                "offline",
                "read",
                "a.bin",
                "--version",
                version,
                "--output",
                text(&output),
            ],
            false,
        );
        assert_eq!(fs::read(&output).unwrap(), expected);
    }
    let comparison = f.run(
        &[
            "partial",
            "offline",
            "compare",
            "current",
            &selector,
            "a.bin",
            "--content",
        ],
        true,
    );
    assert_eq!(
        comparison["data"]["differences"].as_array().unwrap().len(),
        1
    );
    let bounded = f.outer.join("too-small");
    f.run(
        &[
            "partial",
            "offline",
            "read",
            "a.bin",
            "--output",
            text(&bounded),
            "--max-bytes",
            "1",
        ],
        false,
    );
    assert!(!bounded.exists());
    assert_eq!(store.state().unwrap().generation, 1);
    assert_eq!(
        store.latest_captured_patch().unwrap().unwrap().digest,
        capture.digest
    );
}

#[test]
fn stale_or_foreign_version_files_cannot_retarget_current_or_write_output() {
    let (f, store) = Fixture::new(true);
    let receipt = f.run(&["partial", "offline", "status"], true);
    let file = f.outer.join("version.json");
    fs::write(&file, receipt.to_string()).unwrap();
    let version = format!("version-file:{}", file.display());
    let output = f.outer.join("saved-version.bin");
    f.run(
        &[
            "partial",
            "offline",
            "read",
            "a.bin",
            "--version",
            &version,
            "--output",
            text(&output),
        ],
        true,
    );
    assert_eq!(fs::read(output).unwrap(), BASE);
    for (field, wrong) in [
        ("digest", json!("wrong")),
        ("worktreeId", json!("worktree:foreign")),
    ] {
        let mut changed = receipt.clone();
        changed["data"]["version"][field] = wrong;
        fs::write(&file, changed.to_string()).unwrap();
        let output = f.outer.join(format!("refused-{field}"));
        f.run(
            &[
                "partial",
                "offline",
                "read",
                "a.bin",
                "--version",
                &version,
                "--output",
                text(&output),
            ],
            false,
        );
        assert!(!output.exists());
    }
    assert_eq!(store.state().unwrap().generation, 1);
}

#[test]
fn preparation_release_quota_and_eviction_preserve_required_content() {
    let (f, store) = Fixture::new(true);
    let request = AvailabilityRequest {
        version: store.current_version().unwrap(),
        other: None,
        paths: vec![RepoPath::parse("a.bin").unwrap()],
        operation: OfflineOperation::Read,
    };
    let preparation = store
        .prepare_offline(&request, &mut |_, _, _| panic!("already retained"))
        .unwrap();
    let listed = f.run(&["partial", "offline", "preparations"], true);
    assert_eq!(listed["data"]["preparations"][0]["id"], preparation.id);
    assert_eq!(listed["data"]["preparations"][0]["status"], "ready");
    f.run(&["partial", "offline", "release", &preparation.id], true);
    assert_eq!(store.preparations().unwrap()[0].status, "released");
    let quota = f.run(
        &["partial", "offline", "quota", "--bytes", "16777216"],
        true,
    );
    assert_eq!(quota["data"]["usage"]["limitBytes"], 16777216);
    f.run(&["partial", "offline", "evict"], true);
    let available = f.run(&["partial", "offline", "check", "read", "a.bin"], true);
    assert_eq!(available["data"]["available"], true);
}

#[test]
fn family_conversion_and_resume_require_explicit_paths_and_preserve_source() {
    struct Stop;
    impl ConversionFaults for Stop {
        fn check(&self, point: ConversionPoint) -> io::Result<()> {
            if point == ConversionPoint::DestinationCreated {
                Err(io::Error::other("conversion boundary"))
            } else {
                Ok(())
            }
        }
    }
    let (f, source) = Fixture::new(false);
    fs::write(f.root.join("a.bin"), b"accepted source edit").unwrap();
    let capture = source.capture_patch(1).unwrap();
    let marker = fs::read(f.root.join(".layrs/FORMAT")).unwrap();
    let family_root = f.empty("family");
    f.run(
        &[
            "partial",
            "offline",
            "family",
            "create",
            text(&family_root),
            "--quota-bytes",
            "16777216",
        ],
        true,
    );
    let destination = f.empty("converted");
    let result = f.run(
        &[
            "partial",
            "offline",
            "family",
            "convert",
            "--family",
            text(&family_root),
            "--destination",
            text(&destination),
        ],
        true,
    );
    assert_eq!(result["data"]["receipt"]["sourceRetained"], true);
    assert_eq!(
        fs::read(destination.join("a.bin")).unwrap(),
        b"accepted source edit"
    );
    assert!(ProjectionStore::open(&destination).unwrap().is_shared());
    let family = PartialStoreFamily::open(&family_root).unwrap();
    let interrupted = f.empty("interrupted");
    assert!(
        source
            .convert_into_family_with_faults(
                &family,
                &interrupted,
                WorktreeId::parse("worktree:resume-cli").unwrap(),
                &Stop
            )
            .is_err()
    );
    let resumed = f.run(
        &[
            "partial",
            "offline",
            "family",
            "resume",
            "--destination",
            text(&interrupted),
        ],
        true,
    );
    assert_eq!(resumed["data"]["receipt"]["sourceRetained"], true);
    assert_eq!(
        fs::read(interrupted.join("a.bin")).unwrap(),
        b"accepted source edit"
    );
    assert_eq!(fs::read(f.root.join(".layrs/FORMAT")).unwrap(), marker);
    assert_eq!(
        source.latest_captured_patch().unwrap().unwrap().digest,
        capture.digest
    );
    assert_eq!(
        fs::read(f.root.join("a.bin")).unwrap(),
        b"accepted source edit"
    );
}

#[test]
fn online_preparation_flags_are_exact_and_still_use_https_credential_boundary() {
    let (f, store) = Fixture::new(true);
    let step = store.state().unwrap().snapshot.base_step_id.to_string();
    for args in [
        vec!["remote", "partial-prepare", "read", "a.bin"],
        vec!["remote", "partial-observe", &step, "a.bin"],
        vec!["remote", "partial-resume-preparation", "123-456"],
        vec![
            "remote",
            "partial-prepare",
            "read",
            "a.bin",
            "--expected-generation",
            "1",
            "--path",
            "a.bin",
        ],
    ] {
        f.run(&args, false);
    }
    for args in [
        vec![
            "remote",
            "partial-prepare",
            "read",
            "a.bin",
            "--expected-generation",
            "1",
        ],
        vec![
            "remote",
            "partial-observe",
            &step,
            "a.bin",
            "--expected-generation",
            "1",
        ],
        vec![
            "remote",
            "partial-resume-preparation",
            "123-456",
            "--expected-generation",
            "1",
        ],
    ] {
        let result = f.run(&args, false);
        assert!(
            result["error"]["message"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("https"),
            "{result}"
        );
    }
    assert_eq!(store.state().unwrap().generation, 1);
    assert!(store.preparations().unwrap().is_empty());
}
