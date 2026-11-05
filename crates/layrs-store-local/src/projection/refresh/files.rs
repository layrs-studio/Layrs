use super::*;
use crate::secure_fs::PinnedFile;
use std::io::Read;

pub(super) fn inspect(
    root: &PinnedDirectory,
    path: &str,
    entry: &ProjectionEntry,
    name: String,
) -> Result<Record> {
    let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
        return Err(invalid("refresh materializes regular files only"));
    };
    let pinned = root.open_relative_file(Path::new(path))?;
    let bytes = pinned.file().metadata()?.len();
    if bytes > MAX_FILE {
        return Err(invalid("refresh file exceeds1GiB"));
    }
    let record = Record {
        path: path.into(),
        file_id: file_id.clone(),
        executable: entry.executable,
        bytes,
        identity: file::identity(pinned.file())?,
        name,
    };
    verify(&pinned, &record)?;
    Ok(record)
}
pub(super) fn verify(pinned: &PinnedFile, record: &Record) -> Result<()> {
    if file::identity(pinned.file())? != record.identity
        || pinned.file().metadata()?.len() != record.bytes
    {
        return Err(invalid("refresh file identity or length changed"));
    }
    let mut input = pinned.file();
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-file-v1\0");
    let mut buffer = [0u8; 64 * 1024];
    let mut count = 0u64;
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > record.bytes {
            return Err(invalid("refresh file grew"));
        }
        hash.update(&buffer[..n]);
    }
    if count != record.bytes || format!("blake3:{}", hash.finalize()) != record.file_id.as_str() {
        return Err(invalid("local edits prevent projection refresh"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if (pinned.file().metadata()?.permissions().mode() & 0o111 != 0) != record.executable {
            return Err(invalid("local mode change prevents projection refresh"));
        }
    }
    pinned.verify_parent()?;
    Ok(())
}
pub(super) fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
pub(super) fn read_verify(
    root: &PinnedDirectory,
    path: &str,
    record: &Record,
) -> Result<PinnedFile> {
    let pinned = root.open_relative_file(Path::new(path))?;
    verify(&pinned, record)?;
    Ok(pinned)
}

/// Preflight eventual empty destinations, allowing only ancestor leaves already
/// scheduled for quarantine. Every other existing leaf/directory is a collision.
pub(super) fn destination(
    store: &ProjectionStore,
    path: &str,
    removed: &BTreeSet<&str>,
) -> Result<()> {
    let components: Vec<_> = path.split('/').collect();
    let mut relative = String::new();
    for (index, part) in components.iter().enumerate() {
        if !relative.is_empty() {
            relative.push('/');
        }
        relative.push_str(part);
        if removed.contains(relative.as_str()) {
            return Ok(());
        }
        match fs::symlink_metadata(store.root.join(&relative)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
            Ok(meta)
                if index + 1 < components.len()
                    && meta.is_dir()
                    && !meta.file_type().is_symlink() =>
            {
                PinnedDirectory::open(&store.root.join(&relative))?;
            }
            Ok(_) => {
                return Err(invalid(
                    "refresh destination collision; unowned content preserved",
                ));
            }
        }
    }
    Ok(())
}
