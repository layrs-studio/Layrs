//! Bounded administrative intent journal. Its caller holds the remote lock.
use std::{
    fs,
    fs::OpenOptions,
    io::{self, Read, Write},
    path::Path,
};

fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "unsafe policy journal path",
    )
}
fn check(path: &Path) -> io::Result<()> {
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        let metadata = match fs::symlink_metadata(ancestor) {
            Ok(metadata) => metadata,
            Err(error) if ancestor == path && error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if metadata.file_type().is_symlink() {
            return Err(denied());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(denied());
            }
        }
        if ancestor == path {
            if !metadata.is_file() {
                return Err(denied());
            }
        } else if !metadata.is_dir() {
            return Err(denied());
        }
    }
    Ok(())
}
fn no_follow(options: &mut OpenOptions) {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x20000);
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x100);
    }
}
fn sync_parent(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::File::open(path.parent().ok_or_else(denied)?)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
pub(super) fn read(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    check(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    no_follow(&mut options);
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(denied());
    }
    check(path)?;
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(denied());
    }
    Ok(bytes)
}
pub(super) fn write(path: &Path, bytes: &[u8], limit: u64) -> io::Result<()> {
    if bytes.len() as u64 > limit {
        return Err(denied());
    }
    check(path)?;
    let parent = path.parent().ok_or_else(denied)?;
    let staging = parent.join(format!("native-policy-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        no_follow(&mut options);
        let mut file = options.open(&staging)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        check(path)?;
        fs::rename(&staging, path)?;
        sync_parent(path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staging);
    }
    result
}
pub(super) fn remove(path: &Path) -> io::Result<()> {
    check(path)?;
    fs::remove_file(path)?;
    sync_parent(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replaces_existing_intent_and_rejects_nonfiles_and_oversize() {
        let root =
            std::env::temp_dir().join(format!("layrs-policy-journal-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("intent.json");
        write(&path, b"first", 64).unwrap();
        write(&path, b"second", 64).unwrap();
        assert_eq!(read(&path, 64).unwrap(), b"second");
        assert!(read(&path, 2).is_err());
        assert!(write(&path, b"oversized", 2).is_err());
        assert_eq!(read(&path, 64).unwrap(), b"second");
        assert!(write(&root, b"directory", 64).is_err());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        remove(&path).unwrap();
        fs::remove_dir(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn rejects_linked_leaf_and_parent() {
        use std::os::unix::fs::symlink;
        let root =
            std::env::temp_dir().join(format!("layrs-policy-links-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let original = root.join("original");
        write(&original, b"unchanged", 64).unwrap();
        let link = root.join("link");
        symlink(&original, &link).unwrap();
        assert!(read(&link, 64).is_err());
        assert!(write(&link, b"bad", 64).is_err());
        assert!(remove(&link).is_err());
        let parent = root.join("parent");
        symlink(&root, &parent).unwrap();
        assert!(write(&parent.join("new"), b"bad", 64).is_err());
        assert_eq!(read(&original, 64).unwrap(), b"unchanged");
        fs::remove_file(link).unwrap();
        fs::remove_file(parent).unwrap();
        remove(&original).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
