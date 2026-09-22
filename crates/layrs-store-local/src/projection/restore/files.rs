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
    if file.bytes > MAX_PATCH_BYTES as u64
        || super::super::file::identity(pin.file())? != identity
        || pin.file().metadata()?.len() != file.bytes
    {
        return Err(invalid("restore file identity or size changed"));
    }
    let mut input = pin.file();
    let mut data = Vec::new();
    Read::by_ref(&mut input)
        .take(file.bytes + 1)
        .read_to_end(&mut data)?;
    if data.len() as u64 != file.bytes || FileId::from_bytes(&data) != file.file_id {
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
pub(super) fn bytes(
    root: &PinnedDirectory,
    path: &str,
    file: &CapturedFile,
    identity: &str,
) -> Result<Vec<u8>> {
    let pin = verified(root, path, file, identity)?;
    let mut result = Vec::new();
    let mut input = pin.file();
    // Verification consumed this handle; preserve from its beginning.
    use std::io::{Seek, SeekFrom};
    input.seek(SeekFrom::Start(0))?;
    input.take(file.bytes + 1).read_to_end(&mut result)?;
    if result.len() as u64 != file.bytes || FileId::from_bytes(&result) != file.file_id {
        return Err(invalid("restore source changed during preservation"));
    }
    Ok(result)
}
