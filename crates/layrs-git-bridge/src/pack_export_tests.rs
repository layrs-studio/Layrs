use super::*;

#[test]
fn packed_export_preserves_exact_git_ids_tree_order_modes_and_binary_names() {
    let temp = Temp::new();
    let source = temp.directory("pack-source");
    let limits = GitBridgeLimits::default();
    let git = process::Git::new(&source, limits).unwrap();
    git.run(&["init", "--bare", "--template=", "."], None, 4096)
        .unwrap();
    let binary = vec![0, 255, 1, 13, 10];
    let blob = git.write_object("blob", &binary).unwrap();
    let link = git.write_object("blob", b"alpha/leaf").unwrap();
    let empty = git.write_object("blob", b"").unwrap();
    let nested = tree(&git, &[(b"leaf", &binary, "100644")]);
    let mut raw = Vec::new();
    for (mode, kind, oid, name) in [
        ("040000", "tree", nested.as_str(), "alpha"),
        ("100644", "blob", blob.as_str(), "alpha.txt"),
        ("100755", "blob", blob.as_str(), "alpha!"),
        ("120000", "blob", link.as_str(), "link"),
        ("100644", "blob", empty.as_str(), "é\nempty"),
    ] {
        raw.extend_from_slice(format!("{mode} {kind} {oid}\t{name}\0").as_bytes());
    }
    let root = utf8(&git.run(&["mktree", "-z"], Some(&raw), 128).unwrap())
        .unwrap()
        .trim()
        .to_owned();
    let head = commit(&git, &root, &[], "metadata and tree byte identity");
    reference(&git, "refs/heads/main", &head);
    let native = NativeWorkspace::initialize(temp.directory("pack-native"), "Pack").unwrap();
    let plan = scan_git(&source, limits).unwrap();
    plan.install(&native).unwrap();
    let destination = temp.0.join("pack-export");
    let mut count = 0;
    let exported = export_workspace_with_progress(
        &native,
        &destination,
        ExportMode::History,
        limits,
        &mut |event| {
            count = event.git_processes;
            true
        },
    )
    .unwrap();
    assert_eq!(exported.references["refs/heads/main"], head);
    assert!(count <= 10, "pack export used {count} processes");
    let output = process::Git::new(&destination, limits).unwrap();
    assert_eq!(
        output
            .run(&["cat-file", "tree", &root], None, 4096)
            .unwrap(),
        git.run(&["cat-file", "tree", &root], None, 4096).unwrap()
    );
    assert_eq!(
        output
            .run(&["show", "refs/heads/main:alpha/leaf"], None, 4096)
            .unwrap(),
        binary
    );
    assert_eq!(
        output
            .run(&["show", "refs/heads/main:link"], None, 4096)
            .unwrap(),
        b"alpha/leaf"
    );
    assert_eq!(
        std::fs::read_dir(destination.join("objects/pack"))
            .unwrap()
            .filter(|entry| entry
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "pack"))
            .count(),
        1
    );
}
