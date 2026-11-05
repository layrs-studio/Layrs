use super::*;

#[test]
fn converted_capture_selectors_read_check_and_compare_under_destination_identity() {
    let (f, source) = Fixture::new(false);
    // Legacy installation keeps base bytes only in the checkout. Captures below
    // retain replacements, so preserve authorized local input before overwriting it.
    let source_base = fs::read(f.root.join("a.bin")).unwrap();
    assert_eq!(source_base, BASE);
    assert_eq!(
        source.state().unwrap().snapshot.entries[0].target,
        Some(TreeEntryTarget::file(FileId::from_bytes(&source_base)))
    );
    fs::write(f.root.join("a.bin"), b"first source capture").unwrap();
    let first = source.capture_patch(1).unwrap();
    fs::write(f.root.join("a.bin"), b"second source capture").unwrap();
    let second = source.capture_patch(1).unwrap();
    let family = f.empty("family");
    f.run(
        &[
            "partial",
            "offline",
            "family",
            "create",
            text(&family),
            "--quota-bytes",
            "16777216",
        ],
        true,
    );
    let destination = f.empty("converted");
    f.run(
        &[
            "partial",
            "offline",
            "family",
            "convert",
            "--family",
            text(&family),
            "--destination",
            text(&destination),
        ],
        true,
    );
    let store = ProjectionStore::open(&destination).unwrap();
    assert_ne!(store.identity().worktree_id, source.identity().worktree_id);
    let imported = store.captured_patch(first.sequence).unwrap();
    assert_eq!(imported.identity, first.identity);
    assert_eq!(imported.digest, first.digest);
    let before = format!("capture:{}", first.sequence);
    let after = format!("capture:{}", second.sequence);
    let expected_before = json!({
        "kind":"capture", "worktreeId":store.identity().worktree_id,
        "sequence":first.sequence, "digest":first.digest,
    });
    let expected_after = json!({
        "kind":"capture", "worktreeId":store.identity().worktree_id,
        "sequence":second.sequence, "digest":second.digest,
    });
    let output = f.outer.join("imported.bin");
    let read = f.at(
        &destination,
        &[
            "partial",
            "offline",
            "read",
            "a.bin",
            "--version",
            &before,
            "--output",
            text(&output),
        ],
        true,
    );
    assert_eq!(fs::read(output).unwrap(), b"first source capture");
    assert_eq!(read["data"]["version"], expected_before);
    let available = f.at(
        &destination,
        &[
            "partial",
            "offline",
            "check",
            "read",
            "a.bin",
            "--version",
            &before,
        ],
        true,
    );
    assert_eq!(available["data"]["available"], true);
    assert_eq!(available["data"]["request"]["version"], expected_before);
    let comparison_check = f.at(
        &destination,
        &[
            "partial",
            "offline",
            "check",
            "compare-content",
            "a.bin",
            "--version",
            &before,
            "--other",
            &after,
        ],
        true,
    );
    assert_eq!(comparison_check["data"]["available"], true);
    assert_eq!(
        comparison_check["data"]["request"]["version"],
        expected_before
    );
    assert_eq!(comparison_check["data"]["request"]["other"], expected_after);
    let comparison = f.at(
        &destination,
        &[
            "partial",
            "offline",
            "compare",
            &before,
            &after,
            "a.bin",
            "--content",
        ],
        true,
    );
    assert_eq!(
        comparison["data"]["differences"].as_array().unwrap().len(),
        1
    );
    let row = &comparison["data"]["differences"][0];
    assert_eq!(
        row["before"],
        json!(FileId::from_bytes(b"first source capture"))
    );
    assert_eq!(
        row["after"],
        json!(FileId::from_bytes(b"second source capture"))
    );
    let missing_base = f.at(
        &destination,
        &["partial", "offline", "check", "read", "a.bin"],
        true,
    );
    assert_eq!(missing_base["data"]["available"], false);
    assert_eq!(
        missing_base["data"]["paths"][0]["fileId"],
        json!(FileId::from_bytes(&source_base))
    );
    assert_eq!(missing_base["data"]["paths"][0]["retained"], false);
    assert!(missing_base["data"]["paths"][0]["manifestId"].is_null());
    let destination_before = json!(store.state().unwrap());
    let source_before = json!(source.state().unwrap());
    let destination_checkout = checkout_files(&destination);
    let source_checkout = checkout_files(&f.root);
    let imported_second = store.captured_patch(second.sequence).unwrap();
    let unavailable = f.at(
        &destination,
        &[
            "partial",
            "offline",
            "compare",
            "current",
            &before,
            "a.bin",
            "--content",
        ],
        false,
    );
    let message = unavailable["error"]["message"].as_str().unwrap();
    let availability: Value = serde_json::from_str(
        message
            .strip_prefix("offline_compare_unavailable: ")
            .expect("comparison must refuse unavailable content, not fail for another reason"),
    )
    .unwrap();
    assert_eq!(availability["available"], false);
    assert_eq!(availability["prepared"], false);
    assert_eq!(
        availability["request"],
        json!({
            "version":store.current_version().unwrap(), "other":expected_before,
            "paths":["a.bin"], "operation":"compareContent",
        })
    );
    assert_eq!(availability["effectivePaths"], json!(["a.bin"]));
    assert_eq!(availability["blockers"], json!([]));
    assert_eq!(availability["paths"].as_array().unwrap().len(), 2);
    let base = &availability["paths"][0];
    assert_eq!(base["operand"], 0);
    assert_eq!(base["path"], "a.bin");
    assert_eq!(base["fileId"], json!(FileId::from_bytes(&source_base)));
    assert!(base["manifestId"].is_null());
    assert_eq!(base["retained"], false);
    assert_eq!(base["blockers"], json!(["missingContent"]));
    let capture = &availability["paths"][1];
    assert_eq!(capture["operand"], 1);
    assert_eq!(capture["path"], "a.bin");
    assert_eq!(
        capture["fileId"],
        json!(FileId::from_bytes(b"first source capture"))
    );
    assert!(capture["manifestId"].is_string());
    assert_eq!(capture["retained"], true);
    assert_eq!(capture["blockers"], json!([]));
    assert_eq!(checkout_files(&destination), destination_checkout);
    assert_eq!(checkout_files(&f.root), source_checkout);
    assert_eq!(json!(store.state().unwrap()), destination_before);
    assert_eq!(json!(source.state().unwrap()), source_before);
    assert_eq!(store.captured_patch(first.sequence).unwrap(), imported);
    assert_eq!(
        store.captured_patch(second.sequence).unwrap(),
        imported_second
    );
    assert_eq!(
        store.latest_captured_patch().unwrap(),
        Some(imported_second)
    );
    assert_eq!(source.captured_patch(first.sequence).unwrap(), first);
    assert_eq!(source.captured_patch(second.sequence).unwrap(), second);
    assert_eq!(
        source.latest_captured_patch().unwrap(),
        Some(second.clone())
    );

    // Fixture-only library preparation proves the prepared-state CLI consumer.
    // This callback is not a shipped offline repair command. An overwritten,
    // never-retained legacy base needs explicit authorized online preparation;
    // the core TLS tests cover that acquisition, not this CLI test.
    let request = AvailabilityRequest {
        version: store.current_version().unwrap(),
        other: None,
        paths: vec![RepoPath::parse("a.bin").unwrap()],
        operation: OfflineOperation::Read,
    };
    let mut supplied = 0;
    let preparation = store
        .prepare_offline_at_generation(1, &request, &mut |version, path, out| {
            assert_eq!(version, &request.version);
            assert_eq!(path.as_str(), "a.bin");
            supplied += 1;
            out.write_all(&source_base)
        })
        .unwrap();
    assert_eq!(supplied, 1);
    assert_eq!(preparation.status, "ready");
    let base_output = f.outer.join("prepared-base.bin");
    f.at(
        &destination,
        &[
            "partial",
            "offline",
            "read",
            "a.bin",
            "--output",
            text(&base_output),
        ],
        true,
    );
    assert_eq!(fs::read(base_output).unwrap(), source_base);
    let current = f.at(
        &destination,
        &[
            "partial",
            "offline",
            "compare",
            "current",
            &before,
            "a.bin",
            "--content",
        ],
        true,
    );
    assert_eq!(
        current["data"]["differences"][0]["before"],
        json!(FileId::from_bytes(BASE))
    );
    assert_eq!(store.captured_patch(first.sequence).unwrap(), imported);
    assert_eq!(source.captured_patch(first.sequence).unwrap(), first);
    assert_eq!(source.captured_patch(second.sequence).unwrap(), second);
}

fn checkout_files(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_name() != ".layrs")
        .map(|entry| {
            // This fixture contains only top-level files outside its metadata.
            assert!(entry.file_type().unwrap().is_file());
            (
                PathBuf::from(entry.file_name()),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}
