//! Exclusive directory relocation through pinned parents. No recursive copy,
//! deletion, or adoption of a replacement directory is permitted.
use super::*;

impl PinnedDirectory {
    /// Relocate a recorded directory without replacing the destination; true
    /// means a destination collision. Err can follow a physical rename when an
    /// external writer changes a source or ancestor concurrently. The caller
    /// must persist intent first, retain both locations on any error, and accept
    /// only after rechecking identity and the complete subtree. Handles pin
    /// identities, not namespace locations against external ancestor renames.
    pub(crate) fn move_child_directory(
        &self,
        name: &OsStr,
        expected_identity: &str,
        destination: &Self,
        destination_name: &OsStr,
    ) -> io::Result<bool> {
        validate_name(name)?;
        validate_name(destination_name)?;
        self.verify_path_identity()?;
        destination.verify_path_identity()?;
        let file = platform::open_directory_for_move(&self.file, name)?;
        validate_directory_metadata(&self.display_path.join(name), &file.metadata()?)?;
        let identity = file_identity(&file)?;
        let token = format!("fs:v1:{:016x}:{:016x}", identity.volume, identity.file);
        if token != expected_identity {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "directory changed before relocation",
            ));
        }
        // This internal handle carries a directory, so never expose it through
        // APIs which assume a regular PinnedFile. rename_at supports both kinds.
        let entry = PinnedFile {
            file,
            parent: self.file.try_clone()?,
            parent_identity: self.identity,
            identity,
            #[cfg(unix)]
            name: name.to_os_string(),
        };
        if platform::rename_at(&entry, &destination.file, destination_name)? {
            return Ok(true);
        }
        self.sync()?;
        destination.sync()?;
        self.verify_path_identity()?;
        destination.verify_path_identity()?;
        let moved = destination.open_child_for_traversal(destination_name)?;
        if moved.identity != identity {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "directory changed during relocation; moved entry retained",
            ));
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "layrs-pinned-directory-move-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                fs::remove_dir_all(&self.0).unwrap();
            }
        }
    }
    #[test]
    fn moves_exact_subtree_and_preserves_every_byte() {
        let f = Fixture::new();
        fs::create_dir_all(f.0.join("source/nested")).unwrap();
        fs::create_dir(f.0.join("target-parent")).unwrap();
        fs::write(f.0.join("source/nested/file"), b"preserved").unwrap();
        let root = PinnedDirectory::open(&f.0).unwrap();
        let source = root.open_child("source".as_ref()).unwrap();
        let identity = source.identity_token();
        let parent = root.open_child("target-parent".as_ref()).unwrap();
        assert!(
            !root
                .move_child_directory("source".as_ref(), &identity, &parent, "moved".as_ref())
                .unwrap()
        );
        assert!(!f.0.join("source").exists());
        assert_eq!(
            parent
                .open_child("moved".as_ref())
                .unwrap()
                .identity_token(),
            identity
        );
        assert_eq!(
            fs::read(f.0.join("target-parent/moved/nested/file")).unwrap(),
            b"preserved"
        );
    }
    #[test]
    fn collision_and_replaced_source_never_overwrite() {
        let f = Fixture::new();
        for directory in ["source", "occupied"] {
            fs::create_dir(f.0.join(directory)).unwrap();
            fs::write(f.0.join(directory).join("data"), directory).unwrap();
        }
        let root = PinnedDirectory::open(&f.0).unwrap();
        let identity = root.open_child("source".as_ref()).unwrap().identity_token();
        assert!(
            root.move_child_directory("source".as_ref(), &identity, &root, "occupied".as_ref())
                .unwrap()
        );
        assert_eq!(fs::read(f.0.join("occupied/data")).unwrap(), b"occupied");
        fs::rename(f.0.join("source"), f.0.join("original")).unwrap();
        fs::create_dir(f.0.join("source")).unwrap();
        fs::write(f.0.join("source/data"), b"external").unwrap();
        assert!(
            root.move_child_directory("source".as_ref(), &identity, &root, "free".as_ref())
                .is_err()
        );
        assert_eq!(fs::read(f.0.join("source/data")).unwrap(), b"external");
        assert_eq!(fs::read(f.0.join("original/data")).unwrap(), b"source");
        assert!(!f.0.join("free").exists());
    }
    #[test]
    fn regular_file_and_wrong_parent_identity_are_rejected() {
        let f = Fixture::new();
        fs::write(f.0.join("file"), b"untouched").unwrap();
        fs::create_dir_all(f.0.join("parent/source")).unwrap();
        let root = PinnedDirectory::open(&f.0).unwrap();
        assert!(
            root.move_child_directory("file".as_ref(), "irrelevant", &root, "other".as_ref())
                .is_err()
        );
        let parent = root.open_child("parent".as_ref()).unwrap();
        let identity = parent
            .open_child("source".as_ref())
            .unwrap()
            .identity_token();
        fs::rename(f.0.join("parent"), f.0.join("original-parent")).unwrap();
        fs::create_dir_all(f.0.join("parent/source")).unwrap();
        assert!(
            parent
                .move_child_directory("source".as_ref(), &identity, &root, "other".as_ref())
                .is_err()
        );
        assert!(f.0.join("original-parent/source").exists());
        assert!(f.0.join("parent/source").exists());
        assert_eq!(fs::read(f.0.join("file")).unwrap(), b"untouched");
    }
    #[cfg(unix)]
    #[test]
    fn replacement_between_identity_check_and_rename_is_retained_and_rejected() {
        let f = Fixture::new();
        fs::create_dir(f.0.join("source")).unwrap();
        fs::write(f.0.join("source/data"), b"original").unwrap();
        let root = PinnedDirectory::open(&f.0).unwrap();
        let identity = root.open_child("source".as_ref()).unwrap().identity_token();
        let path = f.0.clone();
        platform::RENAME_TEST_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::rename(path.join("source"), path.join("original")).unwrap();
                fs::create_dir(path.join("source")).unwrap();
                fs::write(path.join("source/data"), b"external").unwrap();
            }));
        });
        assert!(
            root.move_child_directory("source".as_ref(), &identity, &root, "moved".as_ref())
                .is_err()
        );
        assert_eq!(fs::read(f.0.join("original/data")).unwrap(), b"original");
        assert_eq!(fs::read(f.0.join("moved/data")).unwrap(), b"external");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_directory_is_never_followed() {
        let f = Fixture::new();
        fs::create_dir(f.0.join("real")).unwrap();
        std::os::unix::fs::symlink("real", f.0.join("link")).unwrap();
        let root = PinnedDirectory::open(&f.0).unwrap();
        let identity = root.open_child("real".as_ref()).unwrap().identity_token();
        assert!(
            root.move_child_directory("link".as_ref(), &identity, &root, "moved".as_ref())
                .is_err()
        );
        assert!(
            fs::symlink_metadata(f.0.join("link"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(f.0.join("real").exists());
    }

    #[cfg(windows)]
    #[test]
    fn intermediate_junction_is_rejected_on_open_and_revalidation() {
        fn junction(link: &Path, target: &Path) {
            let result = std::process::Command::new("cmd.exe")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "junction fixture failed: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let f = Fixture::new();
        fs::create_dir_all(f.0.join("parent/child")).unwrap();
        junction(&f.0.join("alias"), &f.0.join("parent"));
        assert!(PinnedDirectory::open(&f.0.join("alias/child")).is_err());
        fs::remove_dir(f.0.join("alias")).unwrap();

        let mut pinned = PinnedDirectory::open(&f.0.join("parent/child")).unwrap();
        junction(&f.0.join("alias"), &f.0.join("parent"));
        // Model the same recorded final identity now reachable through an
        // ancestor junction. Ordinary Windows rename may prohibit renaming a
        // parent while its child is open; verification must reject this path
        // regardless, rather than merely accepting the identical final handle.
        pinned.display_path = f.0.join("alias/child");
        assert!(pinned.verify_path_identity().is_err());
        assert!(f.0.join("parent/child").is_dir());
        fs::remove_dir(f.0.join("alias")).unwrap();
    }
}
