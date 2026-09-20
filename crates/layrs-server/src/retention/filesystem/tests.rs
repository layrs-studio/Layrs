use super::*;
fn fixture() -> PathBuf {
    #[cfg(windows)]
    let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join("tmp");
    #[cfg(not(windows))]
    let parent = std::env::temp_dir();
    std::fs::create_dir_all(&parent).unwrap();
    let root = parent.join(format!("layrs-retention-fs-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    root
}
#[test]
fn metadata_scan_preserves_bytes_and_enforces_bounds() {
    let root = fixture();
    std::fs::create_dir(root.join("objects")).unwrap();
    std::fs::write(root.join("objects/value"), [0, 255, 10, 13]).unwrap();
    let files = scan(&root, AuditLimits::default()).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].relative_path, "objects/value");
    assert_eq!(files[0].bytes, 4);
    assert_eq!(
        std::fs::read(root.join("objects/value")).unwrap(),
        [0, 255, 10, 13]
    );
    assert!(
        scan(
            &root,
            AuditLimits {
                file_bytes: 3,
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(
        scan(
            &root,
            AuditLimits {
                filesystem_entries: 1,
                ..Default::default()
            }
        )
        .is_err()
    );
    std::fs::create_dir(root.join("objects/nested")).unwrap();
    assert!(
        scan(
            &root,
            AuditLimits {
                depth: 1,
                ..Default::default()
            }
        )
        .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn linked_directory_and_linked_cas_root_are_refused() {
    let root = fixture();
    let outside = fixture();
    std::fs::write(outside.join("untouched"), b"outside").unwrap();
    let link = root.join("escape");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    #[cfg(windows)]
    {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }
    assert!(scan(&root, AuditLimits::default()).is_err());
    assert!(scan(&link, AuditLimits::default()).is_err());
    assert_eq!(
        std::fs::read(outside.join("untouched")).unwrap(),
        b"outside"
    );
    #[cfg(windows)]
    std::fs::remove_dir(&link).unwrap();
    #[cfg(unix)]
    std::fs::remove_file(&link).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(outside).unwrap();
}
#[cfg(target_os = "linux")]
#[test]
fn pinned_directory_never_follows_a_replaced_ancestor() {
    let root = fixture();
    let outside = fixture();
    std::fs::create_dir(root.join("inside")).unwrap();
    std::fs::write(root.join("inside/kept"), b"kept").unwrap();
    std::fs::write(outside.join("secret"), b"must not observe").unwrap();
    let pinned = Directory::root(&root.join("inside")).unwrap();
    std::fs::rename(root.join("inside"), root.join("moved")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("inside")).unwrap();
    let mut entries = 0;
    let mut files = vec![];
    visit(
        &pinned,
        Path::new(""),
        0,
        Default::default(),
        &mut entries,
        &mut files,
    )
    .unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].relative_path, "kept");
    drop(pinned);
    std::fs::remove_file(root.join("inside")).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(outside).unwrap();
}
