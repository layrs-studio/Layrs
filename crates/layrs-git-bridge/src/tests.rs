use super::*;
use layrs_client_core::native_scm::NativeWorkspace;
use std::path::PathBuf;

struct Temp(PathBuf);
#[path = "case_matrix_tests.rs"]
mod case_matrix;
#[path = "expected_state_tests.rs"]
mod expected_state;
#[path = "export_selection_tests.rs"]
mod export_selection;
#[path = "import_recovery_tests.rs"]
mod import_recovery;
#[path = "pack_export_tests.rs"]
mod pack_exports;
#[path = "review_boundary_tests.rs"]
mod review_boundary;
#[path = "scan_checkpoint_tests.rs"]
mod scan_checkpoint;
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("layrs-git-bridge-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn directory(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir(&path).unwrap();
        path
    }
}

#[test]
fn progress_cancellation_is_explicit_and_sha256_import_is_verified() {
    let temp = Temp::new();
    let source = temp.directory("sha256");
    let git = process::Git::new(&source, GitBridgeLimits::default()).unwrap();
    git.run(
        &[
            "init",
            "--bare",
            "--template=",
            "--object-format=sha256",
            ".",
        ],
        None,
        4096,
    )
    .unwrap();
    let tree = tree(&git, &[(b"hello.txt", b"hello", "100644")]);
    let first = commit(&git, &tree, &[], "first");
    let second = commit(&git, &tree, &[&first], "second");
    reference(&git, "refs/heads/main", &second);
    let mut events = Vec::new();
    let error = scan_git_with_progress(&source, GitBridgeLimits::default(), &mut |event| {
        events.push(event.clone());
        event.phase != BridgePhase::ImportCommits || event.completed < 1
    })
    .err()
    .unwrap();
    assert!(matches!(error, BridgeError::Cancelled));
    assert!(
        !events
            .iter()
            .any(|e| e.phase == BridgePhase::ImportComplete)
    );
    let mut events = Vec::new();
    let plan = scan_git_with_progress(&source, GitBridgeLimits::default(), &mut |e| {
        events.push(e.clone());
        true
    })
    .unwrap();
    assert_eq!(plan.report().object_format, "sha256");
    assert_eq!(plan.report().commits.len(), 2);
    assert_eq!(events.last().unwrap().phase, BridgePhase::ImportComplete);
    assert!(
        events.last().unwrap().git_processes < 15,
        "batch reading should not launch a process per object"
    );
    let native = NativeWorkspace::initialize(temp.directory("native"), "SHA256").unwrap();
    plan.install(&native).unwrap();
    let output = temp.0.join("cancelled-export");
    assert!(matches!(
        export_workspace_with_progress(
            &native,
            &output,
            ExportMode::History,
            GitBridgeLimits::default(),
            &mut |e| e.phase != BridgePhase::ExportCommits || e.completed < 1
        ),
        Err(BridgeError::Cancelled)
    ));
    assert!(output.join("layrs-export.incomplete").is_file());
    let checkpoint = std::fs::read(output.join("layrs-export.incomplete")).unwrap();
    assert!(
        resume_export_workspace(
            &native,
            &output,
            ExportMode::Snapshot {
                step_id: plan.report().commits[0].step_id.clone(),
                reference: "refs/heads/snapshot".into()
            },
            GitBridgeLimits::default(),
            &mut |_| true
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read(output.join("layrs-export.incomplete")).unwrap(),
        checkpoint
    );
    std::fs::write(output.join("foreign-file"), b"must not alter").unwrap();
    assert!(
        resume_export_workspace(
            &native,
            &output,
            ExportMode::History,
            GitBridgeLimits::default(),
            &mut |_| true
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read(output.join("foreign-file")).unwrap(),
        b"must not alter"
    );
    std::fs::remove_file(output.join("foreign-file")).unwrap();
    let partial = process::Git::new(&output, GitBridgeLimits::default()).unwrap();
    let empty = partial.write_object("tree", b"").unwrap();
    let foreign = commit(&partial, &empty, &[], "foreign");
    reference(&partial, "refs/heads/foreign", &foreign);
    assert!(
        resume_export_workspace(
            &native,
            &output,
            ExportMode::History,
            GitBridgeLimits::default(),
            &mut |_| true
        )
        .unwrap_err()
        .to_string()
        .contains("unexpected reference")
    );
    partial
        .run(
            &["update-ref", "-d", "refs/heads/foreign", &foreign],
            None,
            128,
        )
        .unwrap();
    reference(&partial, "refs/heads/main", &foreign);
    assert!(
        resume_export_workspace(
            &native,
            &output,
            ExportMode::History,
            GitBridgeLimits::default(),
            &mut |_| true
        )
        .unwrap_err()
        .to_string()
        .contains("differs from expected")
    );
    partial
        .run(
            &["update-ref", "-d", "refs/heads/main", &foreign],
            None,
            128,
        )
        .unwrap();
    let resumed = resume_export_workspace(
        &native,
        &output,
        ExportMode::History,
        GitBridgeLimits::default(),
        &mut |_| true,
    )
    .unwrap();
    assert!(resumed.references.contains_key("refs/heads/main"));
    assert!(!output.join("layrs-export.incomplete").exists());
    // Simulate interruption after atomic ref publication but before marker removal.
    std::fs::write(output.join("layrs-export.incomplete"), &checkpoint).unwrap();
    let replay = resume_export_workspace(
        &native,
        &output,
        ExportMode::History,
        GitBridgeLimits::default(),
        &mut |_| true,
    )
    .unwrap();
    assert_eq!(replay.references, resumed.references);
    let stale = temp.0.join("stale-export");
    assert!(matches!(
        export_workspace_with_progress(
            &native,
            &stale,
            ExportMode::History,
            GitBridgeLimits::default(),
            &mut |event| event.phase != BridgePhase::ExportCommits || event.completed < 1
        ),
        Err(BridgeError::Cancelled)
    ));
    let original_checkpoint = std::fs::read(stale.join("layrs-export.incomplete")).unwrap();
    std::fs::write(temp.0.join("native/later.txt"), b"new capture").unwrap();
    native.capture().unwrap();
    assert!(
        resume_export_workspace(
            &native,
            &stale,
            ExportMode::History,
            GitBridgeLimits::default(),
            &mut |_| true
        )
        .unwrap_err()
        .to_string()
        .contains("checkpoint does not match")
    );
    assert_eq!(
        std::fs::read(stale.join("layrs-export.incomplete")).unwrap(),
        original_checkpoint
    );
    assert!(
        resume_export_workspace(
            &native,
            &output,
            ExportMode::History,
            GitBridgeLimits::default(),
            &mut |_| true
        )
        .is_err()
    );
}

#[test]
#[ignore = "explicit performance recipe: 2,000 commits, native installation, export and read-only pilot"]
fn volume_and_read_only_pilot_measurements() {
    let temp = Temp::new();
    let limits = GitBridgeLimits {
        max_duration_ms: 600_000,
        max_total_bytes: 256 * 1024 * 1024,
        ..GitBridgeLimits::default()
    };
    let source = temp.directory("scale-source");
    let git = process::Git::new(&source, limits).unwrap();
    git.run(&["init", "--bare", "--template=", "."], None, 4096)
        .unwrap();
    let count = 2000;
    let mut stream = Vec::new();
    for index in 0..count {
        let body = format!("revision {index}\n");
        stream.extend_from_slice(format!("commit refs/heads/main\nmark :{}\nauthor Scale <scale@example.invalid> {} +0000\ncommitter Scale <scale@example.invalid> {} +0000\ndata {}\n{}\n",index+1,1_000_000_000+index,1_000_000_000+index,body.len(),body).as_bytes());
        if index > 0 {
            stream.extend_from_slice(format!("from :{index}\n").as_bytes());
        }
        stream.extend_from_slice(
            format!(
                "M 100644 inline docs/counter.txt\ndata {}\n{}\n",
                body.len(),
                body
            )
            .as_bytes(),
        );
        if index == 0 {
            stream.extend_from_slice(b"M 100755 inline run.sh\ndata 19\n#!/bin/sh\necho run\n\nM 100644 inline assets/blob.bin\ndata 4\n\x00\x01\x02\xff\n");
        }
        stream.push(b'\n');
    }
    stream.extend_from_slice(format!("reset refs/heads/maintenance\nfrom :1000\n\nreset refs/tags/v1\nfrom :{count}\n\ndone\n").as_bytes());
    git.run(&["fast-import", "--quiet"], Some(&stream), 4096)
        .unwrap();
    let before = git.run(&["show-ref"], None, 4096).unwrap();
    let mut scan_metrics = None;
    let start = std::time::Instant::now();
    let plan = scan_git_with_progress(&source, limits, &mut |e| {
        if e.completed % 250 == 0 {
            eprintln!("scan {:?}: {}/{:?}", e.phase, e.completed, e.total);
        }
        scan_metrics = Some(e.clone());
        true
    })
    .unwrap();
    let scan_ms = start.elapsed().as_millis();
    assert_eq!(plan.report().commits.len(), count);
    let objects = plan.canonical_objects().len();
    let native = NativeWorkspace::initialize(temp.directory("scale-native"), "Scale").unwrap();
    let start = std::time::Instant::now();
    plan.install(&native).unwrap();
    let install_ms = start.elapsed().as_millis();
    assert!(plan.install(&native).unwrap().already_imported);
    let mut export_metrics = None;
    let start = std::time::Instant::now();
    let destination = temp.0.join("scale-export");
    let exported = export_workspace_with_progress(
        &native,
        &destination,
        ExportMode::History,
        limits,
        &mut |e| {
            if e.completed % 250 == 0 {
                eprintln!("export {:?}: {}/{:?}", e.phase, e.completed, e.total);
            }
            export_metrics = Some(e.clone());
            true
        },
    )
    .unwrap();
    let export_ms = start.elapsed().as_millis();
    assert_eq!(
        exported.references.len(),
        4,
        "main native base plus two branches and tag"
    );
    let output = process::Git::new(&destination, limits).unwrap();
    assert_eq!(
        output
            .run(&["rev-list", "--count", "refs/heads/main"], None, 32)
            .unwrap(),
        b"2000\n"
    );
    assert_eq!(
        output
            .run(&["show", "refs/heads/main:docs/counter.txt"], None, 128)
            .unwrap(),
        b"revision 1999\n"
    );
    assert_eq!(before, git.run(&["show-ref"], None, 4096).unwrap());
    let mut pilot = serde_json::Value::Null;
    if let Some(path) = std::env::var_os("LAYRS_GIT_PILOT_SOURCE") {
        let source = PathBuf::from(path);
        let git = process::Git::new(&source, limits).unwrap();
        let before = git
            .run(&["show-ref"], None, limits.max_total_bytes)
            .unwrap();
        let mut last = None;
        let start = std::time::Instant::now();
        let plan = scan_git_with_progress(&source, limits, &mut |e| {
            last = Some(e.clone());
            true
        })
        .unwrap();
        assert_eq!(
            before,
            git.run(&["show-ref"], None, limits.max_total_bytes)
                .unwrap()
        );
        pilot = serde_json::json!({"commits":plan.report().commits.len(),"canonicalObjects":plan.canonical_objects().len(),"sourceBytes":plan.report().source_bytes,"scanMs":start.elapsed().as_millis(),"progress":last});
    }
    let metrics = serde_json::json!({"schemaVersion":"layrs.git-bridge.measurement.v1","synthetic":{"commits":count,"canonicalObjects":objects,"scanMs":scan_ms,"installMs":install_ms,"exportMs":export_ms,"scan":scan_metrics,"export":export_metrics},"pilot":pilot,"peakWorkingSetBytes":peak_working_set()});
    if let Some(path) = std::env::var_os("LAYRS_BRIDGE_MEASURE_PATH") {
        std::fs::write(path, serde_json::to_vec_pretty(&metrics).unwrap()).unwrap();
    }
    eprintln!("{}", serde_json::to_string_pretty(&metrics).unwrap());
    if std::env::var("LAYRS_BRIDGE_KEEP_FIXTURE").as_deref() == Ok("1") {
        eprintln!("Retained benchmark fixture: {}", temp.0.display());
        std::mem::forget(temp);
    }
}

#[test]
#[ignore = "requires LAYRS_GIT_PILOT_SOURCE; reads existing repository without modifying refs"]
fn read_only_pilot_measurement() {
    let source =
        PathBuf::from(std::env::var_os("LAYRS_GIT_PILOT_SOURCE").expect("explicit pilot source"));
    let limits = GitBridgeLimits {
        max_duration_ms: 600_000,
        max_total_bytes: 256 * 1024 * 1024,
        ..GitBridgeLimits::default()
    };
    let git = process::Git::new(&source, limits).unwrap();
    let before = git
        .run(&["show-ref"], None, limits.max_total_bytes)
        .unwrap();
    let mut last = None;
    let plan = scan_git_with_progress(&source, limits, &mut |event| {
        last = Some(event.clone());
        true
    });
    assert_eq!(
        before,
        git.run(&["show-ref"], None, limits.max_total_bytes)
            .unwrap()
    );
    let outcome = match plan {
        Ok(plan) => {
            serde_json::json!({"accepted":true,"commits":plan.report().commits.len(),"canonicalObjects":plan.canonical_objects().len(),"sourceBytes":plan.report().source_bytes})
        }
        Err(BridgeError::Unsupported(reason)) => {
            serde_json::json!({"accepted":false,"reason":reason})
        }
        Err(error) => panic!("unexpected pilot failure: {error}"),
    };
    let metrics = serde_json::json!({"schemaVersion":"layrs.git-bridge.measurement.v1","mode":"read_only_pilot","outcome":outcome,"sourceReferencesUnchanged":true,"progress":last,"peakWorkingSetBytes":peak_working_set()});
    if let Some(path) = std::env::var_os("LAYRS_BRIDGE_MEASURE_PATH") {
        std::fs::write(path, serde_json::to_vec_pretty(&metrics).unwrap()).unwrap();
    }
    eprintln!("{}", serde_json::to_string_pretty(&metrics).unwrap());
}

fn peak_working_set() -> Option<u64> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let output = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-Process -Id {}).PeakWorkingSet64", std::process::id()),
            ])
            .creation_flags(0x08000000)
            .output()
            .ok()?;
        String::from_utf8(output.stdout).ok()?.trim().parse().ok()
    }
    #[cfg(not(windows))]
    {
        None
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        if self.0.parent() == Some(std::env::temp_dir().as_path()) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
fn bare(path: &Path) -> process::Git {
    let git = process::Git::new(path, GitBridgeLimits::default()).unwrap();
    git.run(&["init", "--bare", "--template=", "."], None, 4096)
        .unwrap();
    git
}
fn tree(git: &process::Git, files: &[(&[u8], &[u8], &str)]) -> String {
    let mut input = vec![];
    for (name, bytes, mode) in files {
        let id = git.write_object("blob", bytes).unwrap();
        input.extend_from_slice(format!("{mode} blob {id}\t").as_bytes());
        input.extend_from_slice(name);
        input.push(0);
    }
    utf8(&git.run(&["mktree", "-z"], Some(&input), 80).unwrap())
        .unwrap()
        .trim()
        .to_owned()
}
fn commit(git: &process::Git, tree: &str, parents: &[&str], message: &str) -> String {
    let mut bytes = format!("tree {tree}\n");
    for parent in parents {
        bytes.push_str(&format!("parent {parent}\n"));
    }
    bytes.push_str("author Test Author <author@example.invalid> 1000000000 +0000\ncommitter Test Committer <committer@example.invalid> 1000000001 +0000\n\n");
    bytes.push_str(message);
    git.write_object("commit", bytes.as_bytes()).unwrap()
}
fn reference(git: &process::Git, name: &str, id: &str) {
    git.run(&["update-ref", name, id], None, 1024).unwrap();
}

#[test]
fn git_native_git_roundtrip_preserves_content_modes_parents_refs_and_provenance() {
    let temp = Temp::new();
    let source = temp.directory("source");
    let git = bare(&source);
    let base_tree = tree(
        &git,
        &[
            (b"code.ts", b"base\r\n", "100644"),
            (b"empty", b"", "100644"),
            (b"binary", &[0, 255, 10, 0], "100644"),
            (b"script", b"echo ok\n", "100755"),
            (b"link", b"code.ts", "120000"),
            ("café\nfile".as_bytes(), b"unicode path", "100644"),
        ],
    );
    let base = commit(&git, &base_tree, &[], "root\n");
    let a = commit(&git, &base_tree, &[&base], "empty commit retained\n");
    let variant_tree = tree(&git, &[(b"code.ts", b"changed", "100644")]);
    let b = commit(&git, &variant_tree, &[&base], "branch\n");
    let merge = commit(&git, &base_tree, &[&a, &b], "merge order\n");
    reference(&git, "refs/heads/main", &merge);
    reference(&git, "refs/heads/feature", &b);
    reference(&git, "refs/tags/light", &a);
    let tag=git.write_object("tag",format!("object {merge}\ntype commit\ntag v1\ntagger Tag Author <tag@example.invalid> 1000000002 +0000\n\nannotated release note\n").as_bytes()).unwrap();
    reference(&git, "refs/tags/v1", &tag);
    let before = git.run(&["show-ref"], None, 8192).unwrap();
    let plan = scan_git(&source, GitBridgeLimits::default()).unwrap();
    assert_eq!(plan.report().commits.len(), 4);
    assert_eq!(plan.report().references.len(), 4);
    assert_eq!(
        scan_git(&source, GitBridgeLimits::default())
            .unwrap()
            .report
            .import_id,
        plan.report.import_id
    );
    assert_eq!(git.run(&["show-ref"], None, 8192).unwrap(), before);
    let checkout = temp.directory("native");
    let workspace = NativeWorkspace::initialize(&checkout, "imported").unwrap();
    let outcome = plan.install(&workspace).unwrap();
    assert!(!outcome.already_imported);
    assert_eq!(outcome.layer_ids.len(), 2);
    assert_eq!(outcome.marker_ids.len(), 2);
    assert!(plan.install(&workspace).unwrap().already_imported);
    assert!(!checkout.join("code.ts").exists());
    let reopened = NativeWorkspace::open(&checkout).unwrap();
    let destination = temp.0.join("exported");
    let exported = export_workspace(
        &reopened,
        &destination,
        ExportMode::History,
        GitBridgeLimits::default(),
    )
    .unwrap();
    let output = process::Git::new(&destination, GitBridgeLimits::default()).unwrap();
    output
        .run(&["fsck", "--strict", "--full"], None, 8192)
        .unwrap();
    for record in &plan.report.commits {
        let after = &exported.commits[&record.step_id];
        let original = git
            .object("commit", &record.git_id, GitBridgeLimits::default())
            .unwrap();
        let rebuilt = output
            .object("commit", after, GitBridgeLimits::default())
            .unwrap();
        assert_eq!(
            import::header(&import::parse_headers(&original).unwrap(), "tree").unwrap(),
            import::header(&import::parse_headers(&rebuilt).unwrap(), "tree").unwrap()
        );
        assert_eq!(
            original, rebuilt,
            "unique provenance should preserve metadata and parent order on this fixture"
        );
    }
    assert_eq!(
        output
            .run(
                &["cat-file", "-t", &exported.references["refs/tags/v1"]],
                None,
                32
            )
            .unwrap(),
        b"tag\n"
    );
    assert_eq!(
        output
            .run(&["ls-tree", "-rz", "refs/heads/main"], None, 8192)
            .unwrap(),
        git.run(&["ls-tree", "-rz", "refs/heads/main"], None, 8192)
            .unwrap()
    );
    assert!(!destination.join("layrs-export.incomplete").exists());
    assert!(
        export_workspace(
            &reopened,
            &destination,
            ExportMode::History,
            GitBridgeLimits::default()
        )
        .is_err()
    );
}

#[test]
fn duplicate_git_commits_keep_distinct_provenance_without_duplicate_native_steps() {
    let temp = Temp::new();
    let source = temp.directory("source");
    let git = bare(&source);
    let tree = tree(&git, &[(b"file", b"same", "100644")]);
    let a = commit(&git, &tree, &[], "message a");
    let b = commit(&git, &tree, &[], "message b");
    reference(&git, "refs/heads/a", &a);
    reference(&git, "refs/heads/b", &b);
    let plan = scan_git(&source, GitBridgeLimits::default()).unwrap();
    assert_eq!(plan.report.commits.len(), 2);
    assert_eq!(
        plan.report.commits[0].step_id,
        plan.report.commits[1].step_id
    );
    assert_ne!(
        plan.report.commits[0].raw_commit_base64,
        plan.report.commits[1].raw_commit_base64
    );
    let checkout = temp.directory("native");
    let workspace = NativeWorkspace::initialize(&checkout, "collapsed identities").unwrap();
    plan.install(&workspace).unwrap();
    let report = export_workspace(
        &workspace,
        temp.0.join("export"),
        ExportMode::History,
        GitBridgeLimits::default(),
    )
    .unwrap();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("no unique Git provenance"))
    );
}

#[test]
fn lfs_gitlinks_shallow_and_noncanonical_paths_are_explicitly_rejected() {
    for special in [
        "lfs", "gitlink", "shallow", "non-nfc", "non-utf8", "partial",
    ] {
        let temp = Temp::new();
        let source = temp.directory("source");
        let git = bare(&source);
        let name = match special {
            "non-nfc" => "cafe\u{301}".as_bytes(),
            "non-utf8" => &[0xff],
            _ => b"file",
        };
        let bytes = if special == "lfs" {
            b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 3\n".as_slice()
        } else {
            b"bytes"
        };
        let tree = tree(&git, &[(name, bytes, "100644")]);
        let mut id = commit(&git, &tree, &[], "root");
        if special == "gitlink" {
            let listing = format!("160000 commit {id}\tsubmodule\0");
            let tree = utf8(
                &git.run(&["mktree", "-z"], Some(listing.as_bytes()), 80)
                    .unwrap(),
            )
            .unwrap()
            .trim()
            .to_owned();
            id = commit(&git, &tree, &[], "submodule");
        }
        reference(&git, "refs/heads/main", &id);
        if special == "shallow" {
            std::fs::write(source.join("shallow"), format!("{id}\n")).unwrap();
        }
        if special == "partial" {
            git.run(&["config", "remote.origin.promisor", "true"], None, 32)
                .unwrap();
        }
        assert!(
            scan_git(&source, GitBridgeLimits::default()).is_err(),
            "accepted {special}"
        );
    }
}

#[test]
fn snapshot_export_is_explicitly_parentless_and_budgets_reject_work() {
    let temp = Temp::new();
    let checkout = temp.directory("native");
    std::fs::write(checkout.join("x"), b"one").unwrap();
    let workspace = NativeWorkspace::initialize(&checkout, "snapshot").unwrap();
    std::fs::write(checkout.join("x"), b"two").unwrap();
    workspace.capture().unwrap();
    let state = workspace.state().unwrap();
    let step = state.view.layer_heads()[0].step_ids()[0].clone();
    let out = temp.0.join("snapshot");
    let report = export_workspace(
        &workspace,
        &out,
        ExportMode::Snapshot {
            step_id: step,
            reference: "refs/heads/snapshot".into(),
        },
        GitBridgeLimits::default(),
    )
    .unwrap();
    let git = process::Git::new(&out, GitBridgeLimits::default()).unwrap();
    let raw = git
        .object(
            "commit",
            &report.references["refs/heads/snapshot"],
            GitBridgeLimits::default(),
        )
        .unwrap();
    assert!(
        !import::parse_headers(&raw)
            .unwrap()
            .iter()
            .any(|(key, _)| key == "parent")
    );
    assert!(
        scan_git(
            &out,
            GitBridgeLimits {
                max_duration_ms: 0,
                ..GitBridgeLimits::default()
            }
        )
        .is_err()
    );
    assert!(
        scan_git(
            &out,
            GitBridgeLimits {
                max_object_bytes: 2,
                ..GitBridgeLimits::default()
            }
        )
        .is_err()
    );
}

#[test]
fn signatures_remain_provenance_and_import_ids_are_scoped_to_space() {
    let temp = Temp::new();
    let source = temp.directory("source");
    let git = bare(&source);
    let root = tree(&git, &[(b"file", b"content", "100644")]);
    let raw = format!(
        "tree {root}\nauthor A <a@example.invalid> 1000000000 +0000\ncommitter A <a@example.invalid> 1000000000 +0000\ngpgsig -----BEGIN SSH SIGNATURE-----\n fake signature\n -----END SSH SIGNATURE-----\n\nsigned commit\n"
    );
    let id = git.write_object("commit", raw.as_bytes()).unwrap();
    reference(&git, "refs/heads/main", &id);
    let tag = format!(
        "object {id}\ntype commit\ntag signed\ntagger A <a@example.invalid> 1000000000 +0000\n\nnote\n-----BEGIN SSH SIGNATURE-----\nfake\n-----END SSH SIGNATURE-----\n"
    );
    let tag_id = git.write_object("tag", tag.as_bytes()).unwrap();
    reference(&git, "refs/tags/signed", &tag_id);
    let plan = scan_git(&source, GitBridgeLimits::default()).unwrap();
    assert_eq!(
        plan.report
            .warnings
            .iter()
            .filter(|warning| warning.contains("signature kept"))
            .count(),
        2
    );
    let first = NativeWorkspace::initialize(temp.directory("first"), "first").unwrap();
    let second = NativeWorkspace::initialize(temp.directory("second"), "second").unwrap();
    let a = plan.install(&first).unwrap();
    let b = plan.install(&second).unwrap();
    assert_ne!(a.operation_id, b.operation_id);
    assert_ne!(a.layer_ids, b.layer_ids);
    assert_ne!(a.marker_ids, b.marker_ids);
    assert!(plan.install(&first).unwrap().already_imported);
    let destination = temp.0.join("export");
    let report = export_workspace(
        &first,
        &destination,
        ExportMode::History,
        GitBridgeLimits::default(),
    )
    .unwrap();
    let output = process::Git::new(&destination, GitBridgeLimits::default()).unwrap();
    let commit = output
        .object(
            "commit",
            &report.references["refs/heads/main"],
            GitBridgeLimits::default(),
        )
        .unwrap();
    let tag = output
        .object(
            "tag",
            &report.references["refs/tags/signed"],
            GitBridgeLimits::default(),
        )
        .unwrap();
    assert!(signature_start(&commit).is_none());
    assert!(signature_start(&tag).is_none());
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("Signature header"))
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("Tag signature removed"))
    );
    let store = layrs_store_local::scm_v1::open(first.store_root()).unwrap();
    let reader = layrs_store_local::scm_v1::NativeScmReader::new(&store).unwrap();
    use layrs_store::scm::ScmReader;
    let operation = reader.get_operation(&a.operation_id).unwrap().unwrap();
    let saved = install::read_report(operation.metadata()).unwrap();
    use base64::Engine;
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&saved.commits[0].raw_commit_base64)
            .unwrap(),
        raw.as_bytes()
    );
}

// Publish a deliberately divergent/native-conflicted fixture via the same
// atomic store boundary as remote reception. Never materialize the checkout.
fn publish_heads(
    workspace: &NativeWorkspace,
    layer: LayerId,
    heads: Vec<StepId>,
    mut objects: Vec<CanonicalObject>,
) {
    use layrs_client_core::native_scm::MutationStamp;
    use layrs_store::scm::*;
    use layrs_store_local::scm_v1::{NativeScmReader, NativeScmRepository, open};
    let state = workspace.state().unwrap();
    let store = open(workspace.store_root()).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let current = reader
        .current_repository_view(state.space.id())
        .unwrap()
        .unwrap();
    let mut layers = state.view.layer_heads().to_vec();
    *layers
        .iter_mut()
        .find(|item| item.layer_id() == &layer)
        .unwrap() = LayerHeadSet::new(layer.clone(), heads).unwrap();
    let view = RepositoryView::new(
        state.space.id().clone(),
        layers,
        state.view.marker_ids().to_vec(),
        state.view.release_ids().to_vec(),
        state.view.worktrees().to_vec(),
    )
    .unwrap();
    let stamp = MutationStamp::fresh().unwrap();
    let operation = Operation::new(
        stamp.operation_id.clone(),
        state.space.id().clone(),
        vec![state.operation.id().clone()],
        view.id().clone(),
        OperationKind::ReceiveRemoteState,
        "test received heads",
        vec![],
        None,
        None,
        stamp.timestamp,
        Metadata::empty(),
    )
    .unwrap();
    objects.push(CanonicalObject::RepositoryView(view.clone()));
    let admissions = objects
        .iter()
        .map(|object| SpaceObjectAdmission::new(state.space.id().clone(), object.object_id()))
        .collect();
    let batch = WriteBatch::new(
        stamp.transaction_id,
        objects,
        vec![EntityRecord::Operation(operation.clone())],
        admissions,
        vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(
                    state.space.id().clone(),
                    current.generation(),
                    current.view_id().cloned(),
                    view.id().clone(),
                )
                .unwrap(),
            ),
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(
                    state.space.id().clone(),
                    vec![state.operation.id().clone()],
                    operation.id().clone(),
                    vec![operation.id().clone()],
                )
                .unwrap(),
            ),
            StoreMutation::WorktreeActiveOperation(
                WorktreeActiveOperationCas::new(
                    state.worktree.id().clone(),
                    state.operation.id().clone(),
                    operation.id().clone(),
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    drop(reader);
    drop(store);
    NativeScmRepository::new(workspace.store_root())
        .apply_batch(&batch)
        .unwrap();
}

#[test]
fn divergent_heads_are_all_exported_and_conflicted_history_requires_clean_snapshot() {
    let temp = Temp::new();
    let source = temp.directory("source");
    let git = bare(&source);
    let base_tree = tree(&git, &[(b"file.bin", &[0, 1], "100644")]);
    let a_tree = tree(&git, &[(b"file.bin", &[0, 2], "100644")]);
    let b_tree = tree(&git, &[(b"file.bin", &[0, 3], "100644")]);
    let base = commit(&git, &base_tree, &[], "base");
    let a = commit(&git, &a_tree, &[&base], "a");
    let b = commit(&git, &b_tree, &[&base], "b");
    reference(&git, "refs/heads/a", &a);
    reference(&git, "refs/heads/b", &b);
    let plan = scan_git(&source, GitBridgeLimits::default()).unwrap();
    let native = NativeWorkspace::initialize(temp.directory("native"), "divergence").unwrap();
    plan.install(&native).unwrap();
    let state = native.state().unwrap();
    let layer = state
        .layers
        .iter()
        .find(|layer| layer.name() == "git/a")
        .unwrap()
        .id()
        .clone();
    let a = plan
        .report
        .references
        .iter()
        .find(|r| r.git_ref == "refs/heads/a")
        .unwrap()
        .step_id
        .clone();
    let b = plan
        .report
        .references
        .iter()
        .find(|r| r.git_ref == "refs/heads/b")
        .unwrap()
        .step_id
        .clone();
    publish_heads(&native, layer.clone(), vec![a.clone(), b.clone()], vec![]);
    let report = export_workspace(
        &native,
        temp.0.join("multihead"),
        ExportMode::History,
        GitBridgeLimits::default(),
    )
    .unwrap();
    let divergent = report
        .references
        .keys()
        .filter(|r| r.starts_with("refs/heads/a/head-"))
        .collect::<Vec<_>>();
    assert_eq!(divergent.len(), 2);
    assert_eq!(
        divergent
            .iter()
            .map(|r| report.references[*r].clone())
            .collect::<std::collections::BTreeSet<_>>(),
        [report.commits[&a].clone(), report.commits[&b].clone()]
            .into_iter()
            .collect()
    );
    publish_heads(&native, layer.clone(), vec![a.clone()], vec![]);
    use layrs_weave::reconciliation::{FailureAction, FileMergeLimits, FileMergePolicy};
    let preview = native
        .preview_merge(
            &[b],
            &layer,
            FileMergePolicy {
                unsupported: FailureAction::StructuralFallback,
                quota: FailureAction::Reject,
                limits: FileMergeLimits::default(),
            },
            layrs_weave::graph_merge::GraphMergeLimits::default(),
        )
        .unwrap();
    assert_eq!(preview.plan().conflicts().len(), 1);
    let conflict = preview.plan().step().id().clone();
    let objects = preview.canonical_objects().unwrap();
    publish_heads(&native, layer, vec![conflict.clone()], objects);
    let destination = temp.0.join("conflicted");
    let error = export_workspace(
        &native,
        &destination,
        ExportMode::History,
        GitBridgeLimits::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains(conflict.as_str()));
    assert!(!destination.exists());
    export_workspace(
        &native,
        temp.0.join("clean-snapshot"),
        ExportMode::Snapshot {
            step_id: a,
            reference: "refs/heads/snapshot".into(),
        },
        GitBridgeLimits::default(),
    )
    .unwrap();
}
