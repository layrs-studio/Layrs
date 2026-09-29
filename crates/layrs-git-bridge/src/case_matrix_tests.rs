use super::*;

#[test]
fn case_matrix_tree_cache_and_bare_export_preserve_names_checkout_obeys_platform() {
    let temp = Temp::new();
    let source = temp.directory("source");
    let git = bare(&source);
    let limits = GitBridgeLimits::default();
    let upper = git.write_object("blob", b"UPPER\0\xff").unwrap();
    let lower = git.write_object("blob", b"lower\r\n").unwrap();
    let dir_upper = tree(&git, &[(b"left", b"first", "100644")]);
    let dir_lower = tree(&git, &[(b"right", b"second", "100644")]);
    let mut raw = Vec::new();
    for (mode, kind, id, name) in [
        ("100644", "blob", &upper, "A"),
        ("100644", "blob", &lower, "a"),
        ("040000", "tree", &dir_upper, "D"),
        ("040000", "tree", &dir_lower, "d"),
        ("100644", "blob", &upper, "Kind"),
        ("040000", "tree", &dir_lower, "kind"),
        ("100644", "blob", &upper, "É"),
        ("100644", "blob", &lower, "é"),
    ] {
        raw.extend_from_slice(format!("{mode} {kind} {id}\t{name}\0").as_bytes());
    }
    let root = utf8(&git.run(&["mktree", "-z"], Some(&raw), 128).unwrap())
        .unwrap()
        .trim()
        .to_owned();
    let head = commit(&git, &root, &[], "case matrix");
    reference(&git, "refs/heads/main", &head);
    let refs_before = git.run(&["show-ref"], None, 4096).unwrap();
    let checkpoint = prepare_git_scan_checkpoint(
        &source,
        temp.0.join("cache"),
        &ImportSelection::AllLocalReferences,
        limits,
        &mut |_| true,
    )
    .unwrap();
    let plan =
        resume_git_scan_checkpoint(&checkpoint.path, &checkpoint.digest, limits, &mut |_| true)
            .unwrap();
    assert!(
        plan.report()
            .warnings
            .iter()
            .filter(|w| w.contains("Case collision retained"))
            .count()
            >= 4
    );
    let step = plan.report().commits[0].step_id.clone();
    let checkout = temp.directory("native");
    let native = NativeWorkspace::initialize(&checkout, "Case matrix").unwrap();
    plan.install(&native).unwrap();
    let destination = temp.0.join("bare-output");
    let report = export_workspace(&native, &destination, ExportMode::History, limits).unwrap();
    assert_eq!(report.references["refs/heads/main"], head);
    let output = process::Git::new(&destination, limits).unwrap();
    assert_eq!(
        output.object("tree", &root, limits).unwrap(),
        git.object("tree", &root, limits).unwrap()
    );
    for (path, bytes) in [
        ("A", b"UPPER\0\xff".as_slice()),
        ("a", b"lower\r\n"),
        ("D/left", b"first"),
        ("d/right", b"second"),
        ("Kind", b"UPPER\0\xff"),
        ("kind/right", b"second"),
        ("É", b"UPPER\0\xff"),
        ("é", b"lower\r\n"),
    ] {
        assert_eq!(
            output
                .run(&["show", &format!("refs/heads/main:{path}")], None, 4096)
                .unwrap(),
            bytes
        );
    }
    std::fs::write(checkout.join("local-sentinel"), b"pending local bytes").unwrap();
    let before = native.state().unwrap().operation.id().clone();
    let preview = native.preview_restore_step(&step, None);
    #[cfg(windows)]
    {
        let error = preview.err().expect("Windows case aliases must be refused");
        assert!(
            error.to_string().contains("represented faithfully"),
            "{error}"
        );
        assert_eq!(native.state().unwrap().operation.id(), &before);
        assert_eq!(
            std::fs::read(checkout.join("local-sentinel")).unwrap(),
            b"pending local bytes"
        );
        assert!(!checkout.join("A").exists());
    }
    #[cfg(unix)]
    {
        let preview = preview.unwrap();
        assert!(preview.collisions.is_empty());
        native
            .restore_step_guarded(&step, None, &preview.expected)
            .unwrap();
        assert_ne!(native.state().unwrap().operation.id(), &before);
        for (path, bytes) in [
            ("A", b"UPPER\0\xff".as_slice()),
            ("a", b"lower\r\n"),
            ("D/left", b"first"),
            ("d/right", b"second"),
            ("Kind", b"UPPER\0\xff"),
            ("kind/right", b"second"),
            ("É", b"UPPER\0\xff"),
            ("é", b"lower\r\n"),
        ] {
            assert_eq!(std::fs::read(checkout.join(path)).unwrap(), bytes);
        }
        assert!(!checkout.join("local-sentinel").exists());
    }
    assert_eq!(git.run(&["show-ref"], None, 4096).unwrap(), refs_before);
}

#[test]
fn case_matrix_ref_aliases_remain_in_cache_and_export_never_renames_them() {
    let temp = Temp::new();
    let source = temp.directory("source");
    let git = bare(&source);
    let limits = GitBridgeLimits::default();
    let first = commit(
        &git,
        &tree(&git, &[(b"file", b"one", "100644")]),
        &[],
        "first",
    );
    let second = commit(
        &git,
        &tree(&git, &[(b"file", b"two", "100644")]),
        &[&first],
        "second",
    );
    // Packed refs represent both names even on a case-insensitive filesystem.
    let packed =
        format!("# pack-refs with: sorted\n{first} refs/heads/Main\n{second} refs/heads/main\n");
    std::fs::write(source.join("packed-refs"), &packed).unwrap();
    let checkpoint = prepare_git_scan_checkpoint(
        &source,
        temp.0.join("cache"),
        &ImportSelection::AllLocalReferences,
        limits,
        &mut |_| true,
    )
    .unwrap();
    assert_eq!(
        checkpoint.references,
        ["refs/heads/Main", "refs/heads/main"]
    );
    let plan =
        resume_git_scan_checkpoint(&checkpoint.path, &checkpoint.digest, limits, &mut |_| true)
            .unwrap();
    let native = NativeWorkspace::initialize(temp.directory("native"), "Ref case matrix").unwrap();
    plan.install(&native).unwrap();
    let destination = temp.0.join("output");
    #[cfg(windows)]
    {
        let error =
            export_workspace(&native, &destination, ExportMode::History, limits).unwrap_err();
        assert!(error.to_string().contains("case collision"), "{error}");
        assert!(!destination.exists());
        let selected = export_workspace(
            &native,
            &destination,
            ExportMode::SelectedHistory {
                references: vec!["refs/heads/Main".into()],
            },
            limits,
        )
        .unwrap();
        assert_eq!(selected.references.len(), 1);
        assert_eq!(selected.references["refs/heads/Main"], first);
    }
    #[cfg(unix)]
    {
        let exported =
            export_workspace(&native, &destination, ExportMode::History, limits).unwrap();
        // The newly initialized workspace also has its own empty native Layer.
        assert_eq!(exported.references.len(), 3);
        assert_eq!(exported.references["refs/heads/Main"], first);
        assert_eq!(exported.references["refs/heads/main"], second);
    }
    let output = process::Git::new(&destination, limits).unwrap();
    output
        .run(&["fsck", "--strict", "--full"], None, 4096)
        .unwrap();
    assert_eq!(
        std::fs::read(source.join("packed-refs")).unwrap(),
        packed.as_bytes()
    );
}
