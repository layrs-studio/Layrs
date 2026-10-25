use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "layrs-anchored-export-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn directory(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir(&path).unwrap();
        path
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        if let (Ok(parent), Ok(root)) = (std::env::temp_dir().canonicalize(), self.0.canonicalize())
        {
            if root.parent() == Some(parent.as_path())
                && root
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("layrs-anchored-export-")
            {
                let _ = std::fs::remove_dir_all(root);
            }
        }
    }
}

#[test]
fn checkpoint_publication_is_durable_and_never_replaces_a_collision() {
    let temp = Temp::new();
    let path = temp.0.join("scan");
    let destination = ScanCheckpointDestination::capture(&path).unwrap();
    let mut writer = destination.begin().unwrap();
    writer.write_all(b"complete checkpoint").unwrap();
    writer.publish().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"complete checkpoint");
    assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 1);
    assert!(ScanCheckpointDestination::capture(&path).is_err());

    let collision = temp.0.join("collision");
    let destination = ScanCheckpointDestination::capture(&collision).unwrap();
    let mut writer = destination.begin().unwrap();
    writer.write_all(b"pending checkpoint").unwrap();
    std::fs::write(&collision, b"user collision").unwrap();
    assert_eq!(
        writer.publish().unwrap_err().kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(std::fs::read(&collision).unwrap(), b"user collision");
}

#[cfg(windows)]
#[test]
fn git_source_resolves_relative_locators_from_verbatim_windows_roots() {
    let temp = Temp::new();
    let main = temp.directory("main");
    let git = main.join(".git");
    let linked_git = git.join("worktrees/linked");
    std::fs::create_dir_all(&linked_git).unwrap();
    std::fs::create_dir(git.join("objects")).unwrap();
    std::fs::create_dir(git.join("refs")).unwrap();
    std::fs::write(linked_git.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
    std::fs::write(linked_git.join("commondir"), b"../..\n").unwrap();
    let linked = temp.directory("linked");
    std::fs::write(
        linked.join(".git"),
        b"gitdir: ../main/.git/worktrees/linked\n",
    )
    .unwrap();
    let canonical = linked.canonicalize().unwrap();
    assert!(canonical.to_string_lossy().starts_with(r"\\?\"));
    let source = GitImportSource::capture(&canonical).unwrap();
    assert_eq!(source.common_path(), git.canonicalize().unwrap());
    source.verify().unwrap();
    drop(source);
    for locator in [
        format!("gitdir: {}escape\n", "../".repeat(128)),
        "gitdir: C:relative\n".into(),
        "gitdir: \\rooted-without-drive\n".into(),
    ] {
        std::fs::write(linked.join(".git"), locator).unwrap();
        assert!(GitImportSource::capture(&canonical).is_err());
    }
}

#[test]
fn git_source_accepts_leading_parents_but_refuses_parents_after_normal_components() {
    let temp = Temp::new();
    let main = temp.directory("main");
    let git = main.join(".git");
    let linked_git = git.join("worktrees/linked");
    std::fs::create_dir_all(&linked_git).unwrap();
    std::fs::create_dir(git.join("objects")).unwrap();
    std::fs::create_dir(git.join("refs")).unwrap();
    for path in [&git, &linked_git] {
        std::fs::write(path.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
    }
    std::fs::write(linked_git.join("commondir"), b"../..\n").unwrap();
    let linked = temp.directory("linked");
    for locator in ["../main/.git/worktrees/linked", "../main/.git"] {
        std::fs::write(linked.join(".git"), format!("gitdir: {locator}\n")).unwrap();
        let source = GitImportSource::capture(&linked).unwrap();
        assert_eq!(source.common_path(), git.canonicalize().unwrap());
        source.verify().unwrap();
    }
    for locator in [
        "../main/.git/worktrees/linked/../linked".to_owned(),
        linked_git.join("../linked").to_string_lossy().into_owned(),
    ] {
        std::fs::write(linked.join(".git"), format!("gitdir: {locator}\n")).unwrap();
        assert!(GitImportSource::capture(&linked).is_err());
    }
    std::fs::write(
        linked.join(".git"),
        b"gitdir: ../main/.git/worktrees/linked\n",
    )
    .unwrap();
    std::fs::write(linked_git.join("commondir"), b"../../objects/..\n").unwrap();
    assert!(GitImportSource::capture(&linked).is_err());
}

#[cfg(unix)]
#[test]
fn git_source_refuses_symlink_then_parent_in_gitdir_and_commondir() {
    use std::os::unix::fs::symlink;
    let temp = Temp::new();
    let outside = temp.directory("outside");
    let anchor = outside.join("anchor");
    std::fs::create_dir(&anchor).unwrap();
    let make_git = |path: &Path| {
        std::fs::create_dir_all(path.join("objects")).unwrap();
        std::fs::create_dir(path.join("refs")).unwrap();
        std::fs::write(path.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
    };
    let redirected = outside.join("repo");
    make_git(&redirected);

    let source = temp.directory("source");
    let lexical_git = source.join("repo");
    make_git(&lexical_git);
    symlink(&anchor, source.join("link")).unwrap();
    let actual_git = source.join("link/../repo").canonicalize().unwrap();
    assert_eq!(actual_git, redirected.canonicalize().unwrap());
    assert_ne!(actual_git, lexical_git.canonicalize().unwrap());
    for locator in [
        "link/../repo".to_owned(),
        source.join("link/../repo").to_string_lossy().into_owned(),
    ] {
        std::fs::write(source.join(".git"), format!("gitdir: {locator}\n")).unwrap();
        assert!(GitImportSource::capture(&source).is_err());
    }

    let source = temp.directory("common-source");
    let git = source.join(".git");
    make_git(&git);
    let lexical_common = git.join("repo");
    make_git(&lexical_common);
    symlink(&anchor, git.join("link")).unwrap();
    let actual_common = git.join("link/../repo").canonicalize().unwrap();
    assert_eq!(actual_common, redirected.canonicalize().unwrap());
    assert_ne!(actual_common, lexical_common.canonicalize().unwrap());
    for locator in [
        "link/../repo".to_owned(),
        git.join("link/../repo").to_string_lossy().into_owned(),
    ] {
        std::fs::write(git.join("commondir"), format!("{locator}\n")).unwrap();
        assert!(GitImportSource::capture(&source).is_err());
    }
}

#[test]
fn checkpoint_live_parent_replacement_is_denied_or_refuses_without_redirected_writes() {
    for during_write in [false, true] {
        let temp = Temp::new();
        let parent = temp.directory("parent");
        let moved = temp.0.join("moved");
        let destination = ScanCheckpointDestination::capture(&parent.join("scan")).unwrap();
        let writer = during_write.then(|| destination.begin().unwrap());
        #[cfg(windows)]
        assert_delete_handle_denied(&parent);
        let renamed = std::fs::rename(&parent, &moved);
        if let Err(error) = renamed {
            assert!(cfg!(windows), "unexpected rename refusal: {error}");
            assert!(matches!(error.raw_os_error(), Some(5 | 32)));
            drop(writer);
            drop(destination);
            assert!(!parent.join("scan").exists());
        } else {
            std::fs::create_dir(&parent).unwrap();
            if let Some(mut writer) = writer {
                assert!(writer.write_all(b"must not redirect").is_err());
                assert!(writer.publish().is_err());
            } else {
                assert!(destination.begin().is_err());
            }
            assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
            assert!(!moved.join("scan").exists());
        }
    }
}

#[test]
fn changed_parent_refuses_creation_without_writing_either_directory() {
    let temp = Temp::new();
    let parent = temp.directory("parent");
    let review = ExportDestination::capture(parent.join("out.git"), 100, 65536).unwrap();
    let original = temp.0.join("original");
    std::fs::rename(&parent, &original).unwrap();
    std::fs::create_dir(&parent).unwrap();
    assert!(review.begin(false).is_err());
    assert!(!parent.join("out.git").exists() && !original.join("out.git").exists());
}

#[test]
fn copied_resume_directory_and_modified_bytes_invalidate_exact_review() {
    let temp = Temp::new();
    let path = temp.directory("resume.git");
    std::fs::write(path.join("layrs-export.incomplete"), b"reviewed").unwrap();
    let review = ExportDestination::capture(&path, 100, 65536).unwrap();
    std::fs::write(path.join("layrs-export.incomplete"), b"changed").unwrap();
    assert!(review.begin(true).is_err());
    std::fs::write(path.join("layrs-export.incomplete"), b"reviewed").unwrap();
    std::fs::rename(&path, temp.0.join("retained")).unwrap();
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("layrs-export.incomplete"), b"reviewed").unwrap();
    assert!(review.begin(true).is_err());
    assert_eq!(
        std::fs::read(path.join("layrs-export.incomplete")).unwrap(),
        b"reviewed"
    );
    assert_eq!(std::fs::read_dir(&path).unwrap().count(), 1);
}

#[test]
fn destination_existence_and_tree_budgets_are_reviewed() {
    let temp = Temp::new();
    let path = temp.0.join("out.git");
    let review = ExportDestination::capture(&path, 100, 65536).unwrap();
    assert!(review.begin(true).is_err());
    std::fs::create_dir(&path).unwrap();
    assert!(review.begin(false).is_err());
    std::fs::write(path.join("large"), [42; 65]).unwrap();
    assert!(ExportDestination::capture(&path, 100, 64).is_err());
    assert!(ExportDestination::capture(&path, 0, 65536).is_err());
}

#[test]
fn native_binding_tracks_authority_absence_and_locator_without_store_reopen() {
    let temp = Temp::new();
    let checkout = temp.directory("checkout");
    let store_path = temp.directory("store");
    let checkout_pin = PinnedDirectory::open(&checkout).unwrap();
    let store_pin = PinnedDirectory::open(&store_path).unwrap();
    std::fs::write(checkout.join(".layrs"), b"locator").unwrap();
    let capture = || {
        NativeBridgeFiles::capture(
            &checkout,
            &checkout_pin.identity_token(),
            &store_path,
            &store_pin.identity_token(),
        )
        .unwrap()
    };
    let reviewed = capture();
    std::fs::write(store_path.join("native-remote.json"), b"policy-only-change").unwrap();
    assert!(reviewed.verify().is_err());
    let reviewed = capture();
    std::fs::remove_file(store_path.join("native-remote.json")).unwrap();
    assert!(reviewed.verify().is_err());
    let reviewed = capture();
    std::fs::write(checkout.join(".layrs"), b"changed locator").unwrap();
    assert!(reviewed.verify().is_err());
}

#[test]
fn direct_export_files_are_relative_and_live_parent_replacement_cannot_redirect() {
    let temp = Temp::new();
    let parent = temp.directory("parent");
    let path = parent.join("out.git");
    let review = ExportDestination::capture(&path, 100, 65536).unwrap();
    let lease = review.begin(false).unwrap();
    lease
        .write_file("layrs-export.incomplete", b"checkpoint", false)
        .unwrap();
    assert!(lease.write_file("../escape", b"no", false).is_err());
    assert!(!parent.join("escape").exists());
    let moved = temp.0.join("moved");
    #[cfg(windows)]
    {
        assert_delete_handle_denied(&parent);
        assert!(
            std::fs::rename(&parent, &moved).is_err(),
            "a live export guard must deny parent rename"
        );
        lease
            .write_file("layrs-export-report.json", b"report", false)
            .unwrap();
        lease
            .write_file("layrs-export-report.json", b"new", true)
            .unwrap();
        assert_eq!(
            std::fs::read(path.join("layrs-export-report.json")).unwrap(),
            b"new"
        );
    }
    #[cfg(unix)]
    {
        std::fs::rename(&parent, &moved).unwrap();
        std::fs::create_dir(&parent).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(
            lease
                .write_file("layrs-export-report.json", b"report", false)
                .is_err()
        );
        assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0);
    }
}

#[cfg(windows)]
fn assert_delete_handle_denied(path: &Path) {
    use std::os::windows::fs::OpenOptionsExt;
    let error = std::fs::OpenOptions::new()
        .access_mode(0x0001_0000)
        .share_mode(7)
        .custom_flags(0x0200_0000 | 0x0020_0000)
        .open(path)
        .unwrap_err();
    assert_eq!(
        error.raw_os_error(),
        Some(32),
        "DELETE access must fail with ERROR_SHARING_VIOLATION: {error}"
    );
}

#[test]
fn git_command_keeps_pinned_directory_even_when_path_changes_before_spawn() {
    let temp = Temp::new();
    let parent = temp.directory("parent");
    let path = parent.join("out.git");
    let lease = ExportDestination::capture(&path, 1000, 65536)
        .unwrap()
        .begin(false)
        .unwrap();
    let mut command = Command::new("git");
    command
        .args(["init", "--bare", "--template=", "."])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        );
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_")
            && key != "GIT_CONFIG_NOSYSTEM"
            && key != "GIT_CONFIG_GLOBAL"
        {
            command.env_remove(key);
        }
    }
    let _guard = lease.prepare_command(&mut command).unwrap();
    let moved = temp.0.join("moved");
    #[cfg(windows)]
    {
        assert_delete_handle_denied(&path);
        assert!(std::fs::rename(&parent, &moved).is_err());
    }
    #[cfg(unix)]
    {
        std::fs::rename(&parent, &moved).unwrap();
        std::fs::create_dir(&parent).unwrap();
        std::fs::create_dir(&path).unwrap();
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    #[cfg(windows)]
    assert!(path.join("HEAD").exists());
    #[cfg(unix)]
    {
        assert!(moved.join("out.git/HEAD").exists());
        assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0);
        assert!(lease.verify().is_err());
    }
}
