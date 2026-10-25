use super::*;
use layrs_store_local::projection::{RestorePreview, RestoreReceipt, RestoreSource};

fn preview(f: &Fixture, output: &Path, version: Option<&str>, success: bool) -> Value {
    let mut args = vec![
        "partial",
        "offline",
        "restore-preview",
        "a.bin",
        "--expected-generation",
        "1",
        "--output",
        text(output),
    ];
    if let Some(version) = version {
        args.extend(["--version", version]);
    }
    f.run(&args, success)
}

fn apply(f: &Fixture, root: &Path, artifact: &Path, digest: &str, success: bool) -> Value {
    f.at(
        root,
        &[
            "partial",
            "offline",
            "restore-apply",
            text(artifact),
            "--expected-digest",
            digest,
        ],
        success,
    )
}

#[test]
fn reviewed_current_restore_keeps_preview_read_only_and_retains_overwritten_bytes() {
    let (f, store) = Fixture::new(true);
    let file = f.root.join("a.bin");
    fs::write(&file, b"unsaved preimage").unwrap();
    let state = store.state().unwrap();
    let artifact = f.outer.join("reviewed.json");
    let result = preview(&f, &artifact, None, true);
    let document: RestorePreview = serde_json::from_slice(&fs::read(&artifact).unwrap()).unwrap();
    assert_eq!(
        result["data"]["preview"],
        serde_json::to_value(&document).unwrap()
    );
    assert_eq!(
        document.source,
        RestoreSource::Version {
            version: store.current_version().unwrap(),
            paths: vec![RepoPath::parse("a.bin").unwrap()],
        }
    );
    assert_eq!(
        document.before[&RepoPath::parse("a.bin").unwrap()]
            .as_ref()
            .unwrap()
            .file_id,
        FileId::from_bytes(b"unsaved preimage")
    );
    assert_eq!(
        document.after[&RepoPath::parse("a.bin").unwrap()]
            .as_ref()
            .unwrap()
            .file_id,
        FileId::from_bytes(BASE)
    );
    assert_eq!(fs::read(&file).unwrap(), b"unsaved preimage");
    assert_eq!(store.state().unwrap(), state);
    assert!(store.latest_captured_patch().unwrap().is_none());
    let original_artifact = fs::read(&artifact).unwrap();
    preview(&f, &artifact, None, false);
    assert_eq!(fs::read(&artifact).unwrap(), original_artifact);

    let applied = apply(&f, &f.root, &artifact, &document.digest, true);
    let receipt: RestoreReceipt =
        serde_json::from_value(applied["data"]["restoration"].clone()).unwrap();
    assert_eq!(receipt.operation_id, document.operation_id);
    assert_eq!(fs::read(&file).unwrap(), BASE);
    assert_eq!(store.state().unwrap(), state);
    assert_eq!(
        store.latest_captured_patch().unwrap(),
        Some(receipt.capture.clone())
    );
    assert!(!f.root.join("later.txt").exists());
    // Exact retries return the accepted receipt without another capture.
    let repeated = apply(&f, &f.root, &artifact, &document.digest, true);
    assert_eq!(
        repeated["data"]["restoration"],
        applied["data"]["restoration"]
    );

    // The Store's existing inverse API proves the CLI apply retained the preimage.
    let inverse = store
        .preview_restore(
            1,
            &RestoreSource::Inverse {
                operation_id: receipt.operation_id,
            },
        )
        .unwrap();
    store.apply_restore(&inverse).unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"unsaved preimage");
}

#[test]
fn historical_capture_restore_uses_the_exact_stored_version() {
    let (f, store) = Fixture::new(true);
    let file = f.root.join("a.bin");
    fs::write(&file, b"historical capture").unwrap();
    let capture = store.capture_patch(1).unwrap();
    fs::write(&file, b"later working bytes").unwrap();
    let selector = format!("capture:{}", capture.sequence);
    let artifact = f.outer.join("historical.json");
    let result = preview(&f, &artifact, Some(&selector), true);
    assert_eq!(
        result["data"]["preview"]["source"]["version"],
        json!({
            "kind":"capture", "worktreeId":store.identity().worktree_id,
            "sequence":capture.sequence, "digest":capture.digest,
        })
    );
    assert_eq!(
        store.latest_captured_patch().unwrap(),
        Some(capture.clone())
    );
    assert_eq!(fs::read(&file).unwrap(), b"later working bytes");
    let digest = result["data"]["preview"]["digest"].as_str().unwrap();
    apply(&f, &f.root, &artifact, digest, true);
    assert_eq!(fs::read(&file).unwrap(), b"historical capture");
    assert_eq!(store.captured_patch(capture.sequence).unwrap(), capture);
    assert!(!f.root.join("later.txt").exists());
}

#[test]
fn restore_refuses_wrong_digest_tampering_malformed_artifacts_and_missing_content() {
    let (f, store) = Fixture::new(true);
    let file = f.root.join("a.bin");
    fs::write(&file, b"preserve me").unwrap();
    let artifact = f.outer.join("reviewed.json");
    let result = preview(&f, &artifact, None, true);
    let digest = result["data"]["preview"]["digest"].as_str().unwrap();
    let wrong = format!("blake3:{}", "0".repeat(64));
    apply(&f, &f.root, &artifact, &wrong, false);
    let mut tampered = result["data"]["preview"].clone();
    tampered["after"]["a.bin"]["bytes"] = json!(12345);
    for bytes in [
        serde_json::to_vec(&tampered).unwrap(),
        br#"{"ok":false,"error":{"message":"not a preview"}}"#.to_vec(),
        b"{".to_vec(),
        vec![b' '; 1024 * 1024 + 1],
    ] {
        fs::write(&artifact, bytes).unwrap();
        apply(&f, &f.root, &artifact, digest, false);
        assert_eq!(fs::read(&file).unwrap(), b"preserve me");
        assert!(store.latest_captured_patch().unwrap().is_none());
    }
    let unavailable = f.outer.join("missing-content.json");
    f.run(
        &[
            "partial",
            "offline",
            "restore-preview",
            "later.txt",
            "--expected-generation",
            "1",
            "--output",
            text(&unavailable),
        ],
        false,
    );
    assert!(!unavailable.exists());
    assert!(!f.root.join("later.txt").exists());
    assert_eq!(fs::read(&file).unwrap(), b"preserve me");
}

#[test]
fn restore_refuses_stale_checkout_generation_and_foreign_collision() {
    for scenario in ["checkout", "generation", "collision"] {
        let (f, store) = Fixture::new(true);
        let file = f.root.join("a.bin");
        fs::write(&file, b"reviewed bytes").unwrap();
        let artifact = f.outer.join("reviewed.json");
        let result = preview(&f, &artifact, None, true);
        let digest = result["data"]["preview"]["digest"].as_str().unwrap();
        match scenario {
            "checkout" => fs::write(&file, b"new writer bytes").unwrap(),
            "generation" => {
                let mut next = store.state().unwrap().snapshot;
                next.observed_authorization_epoch += 1;
                store.compare_and_swap(1, &next).unwrap();
            }
            "collision" => {
                // Fixture-owned path only: replace the reviewed file with foreign directory data.
                fs::remove_file(&file).unwrap();
                fs::create_dir(&file).unwrap();
                fs::write(file.join("foreign.txt"), b"foreign contents").unwrap();
            }
            _ => unreachable!(),
        }
        apply(&f, &f.root, &artifact, digest, false);
        assert!(store.latest_captured_patch().unwrap().is_none());
        match scenario {
            "checkout" => assert_eq!(fs::read(&file).unwrap(), b"new writer bytes"),
            "generation" => {
                assert_eq!(fs::read(&file).unwrap(), b"reviewed bytes");
                assert_eq!(store.state().unwrap().generation, 2);
            }
            "collision" => assert_eq!(
                fs::read(file.join("foreign.txt")).unwrap(),
                b"foreign contents"
            ),
            _ => unreachable!(),
        }
    }
}

#[test]
fn reviewed_artifact_cannot_apply_in_another_shared_worktree() {
    let (f, store) = Fixture::new(true);
    let family = PartialStoreFamily::open(f.outer.join("family")).unwrap();
    let other_root = f.empty("other");
    let installed = store.state().unwrap().snapshot;
    let mut initial = installed.clone();
    initial.entries[0].state = EntryState::Unavailable;
    let other = family
        .create_worktree(
            &other_root,
            WorktreeId::parse("worktree:other").unwrap(),
            initial,
            ProjectionSelection::All,
        )
        .unwrap();
    other
        .install_projection(0, &installed, &mut |_, out| out.write_all(BASE))
        .unwrap();
    for root in [&f.root, &other_root] {
        fs::write(root.join("a.bin"), b"same working bytes").unwrap();
    }
    let artifact = f.outer.join("reviewed.json");
    let result = preview(&f, &artifact, None, true);
    let digest = result["data"]["preview"]["digest"].as_str().unwrap();
    apply(&f, &other_root, &artifact, digest, false);
    for (root, store) in [(&f.root, &store), (&other_root, &other)] {
        assert_eq!(fs::read(root.join("a.bin")).unwrap(), b"same working bytes");
        assert!(store.latest_captured_patch().unwrap().is_none());
    }
}

#[test]
fn remote_version_artifact_is_bound_to_destination_with_other_expectations_identical() {
    let (f, a) = Fixture::new(true);
    let family = PartialStoreFamily::open(f.outer.join("family")).unwrap();
    let b_root = f.empty("other");
    let state = a.state().unwrap();
    let mut initial = state.snapshot.clone();
    initial.entries[0].state = EntryState::Unavailable;
    let b = family
        .create_worktree(
            &b_root,
            WorktreeId::parse("worktree:other").unwrap(),
            initial,
            ProjectionSelection::All,
        )
        .unwrap();
    b.install_projection(0, &state.snapshot, &mut |_, out| out.write_all(BASE))
        .unwrap();
    assert_eq!(a.state().unwrap(), b.state().unwrap());
    let remote = a
        .record_version_observation_at_generation(1, &state.snapshot, &ProjectionSelection::All)
        .unwrap();
    assert_eq!(
        remote,
        b.record_version_observation_at_generation(1, &state.snapshot, &ProjectionSelection::All,)
            .unwrap()
    );
    assert!(matches!(
        &remote,
        layrs_core::projection::ProjectionVersion::Remote { .. }
    ));
    for root in [&f.root, &b_root] {
        fs::write(root.join("a.bin"), b"identical working content").unwrap();
    }
    let version_file = f.outer.join("observed-version.json");
    fs::write(&version_file, serde_json::to_vec(&remote).unwrap()).unwrap();
    let selector = format!("version-file:{}", text(&version_file));
    let a_artifact = f.outer.join("a-preview.json");
    let a_result = preview(&f, &a_artifact, Some(&selector), true);
    let b_artifact = f.outer.join("b-preview.json");
    let b_result = f.at(
        &b_root,
        &[
            "partial",
            "offline",
            "restore-preview",
            "a.bin",
            "--version",
            &selector,
            "--expected-generation",
            "1",
            "--output",
            text(&b_artifact),
        ],
        true,
    );
    let a_preview = &a_result["data"]["preview"];
    let b_preview = &b_result["data"]["preview"];
    assert_eq!(
        a_preview["source"]["version"],
        serde_json::to_value(&remote).unwrap()
    );
    assert_eq!(a_preview["destination"]["identity"], json!(a.identity()));
    assert_eq!(b_preview["destination"]["identity"], json!(b.identity()));
    assert_ne!(
        a_preview["destination"]["checkoutIdentity"],
        b_preview["destination"]["checkoutIdentity"]
    );
    // Fresh operation IDs and their hashes differ; every other expectation must
    // match so a version/bytes/generation mismatch cannot mask destination binding.
    let without_destination = |mut value: Value| {
        let object = value.as_object_mut().unwrap();
        for key in ["operationId", "digest", "destination"] {
            object.remove(key);
        }
        value
    };
    assert_eq!(
        without_destination(a_preview.clone()),
        without_destination(b_preview.clone())
    );
    let digest = a_preview["digest"].as_str().unwrap();
    let before_usage = family.usage().unwrap();
    let rejected = apply(&f, &b_root, &a_artifact, digest, false);
    assert!(
        rejected["error"]["message"]
            .as_str()
            .unwrap()
            .contains("destination")
    );
    assert_eq!(family.usage().unwrap(), before_usage);
    for (root, store) in [(&f.root, &a), (&b_root, &b)] {
        assert_eq!(
            fs::read(root.join("a.bin")).unwrap(),
            b"identical working content"
        );
        assert!(store.latest_captured_patch().unwrap().is_none());
    }
    apply(&f, &f.root, &a_artifact, digest, true);
    assert_eq!(fs::read(f.root.join("a.bin")).unwrap(), BASE);
    let rejected_replay = apply(&f, &b_root, &a_artifact, digest, false);
    assert!(
        rejected_replay["error"]["message"]
            .as_str()
            .unwrap()
            .contains("destination")
    );
    assert_eq!(
        fs::read(b_root.join("a.bin")).unwrap(),
        b"identical working content"
    );
    assert!(b.latest_captured_patch().unwrap().is_none());
}
