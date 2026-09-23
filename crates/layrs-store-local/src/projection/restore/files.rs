use super::*;
use crate::secure_fs::PinnedFile;

pub(super) fn present(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
pub(super) fn verified(
    root: &PinnedDirectory,
    path: &str,
    file: &CapturedFile,
    identity: &str,
) -> Result<PinnedFile> {
    let pin = root.open_relative_file(Path::new(path))?;
    if file.bytes > MAX_CAPTURE_FILE_BYTES
        || super::super::file::identity(pin.file())? != identity
        || pin.file().metadata()?.len() != file.bytes
    {
        return Err(invalid("restore file identity or size changed"));
    }
    let mut input = pin.file();
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-file-v1\0");
    let mut buffer = [0u8; 64 * 1024];
    let mut count = 0u64;
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        count += read as u64;
        if count > file.bytes {
            return Err(invalid("restore file grew"));
        }
        hash.update(&buffer[..read]);
    }
    if count != file.bytes || format!("blake3:{}", hash.finalize()) != file.file_id.as_str() {
        return Err(invalid("restore file bytes changed"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if (pin.file().metadata()?.permissions().mode() & 0o111 != 0) != file.executable {
            return Err(invalid("restore file mode changed"));
        }
    }
    pin.verify_parent()?;
    Ok(pin)
}
