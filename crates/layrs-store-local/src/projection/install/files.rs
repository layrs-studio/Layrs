use super::*;
use std::io::Read;
pub(super) fn destination_absent(root: &PinnedDirectory, base: &Path, path: &str) -> Result<()> {
    let mut parent = String::new();
    for component in path.split('/').take(path.split('/').count() - 1) {
        if !parent.is_empty() {
            parent.push('/');
        }
        parent.push_str(component);
        match fs::symlink_metadata(base.join(&parent)) {
            Ok(m) => {
                if !m.is_dir() || m.file_type().is_symlink() {
                    return Err(invalid("projection parent collision"));
                }
                PinnedDirectory::open(&base.join(&parent))?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => break,
            Err(e) => return Err(e.into()),
        }
    }
    root.verify_path_identity()?;
    match fs::symlink_metadata(base.join(path)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
        Ok(_) => Err(invalid(
            "projection destination exists; unowned files are never overwritten",
        )),
    }
}
pub(super) fn verify_file(path: &Path, record: &FileRecord) -> Result<()> {
    let pinned = super::super::file::DatabasePin::open(path)?;
    if pinned.identity() != record.identity || pinned.file.metadata()?.len() != record.bytes {
        return Err(invalid("projection file identity or length changed"));
    }
    let mut file = &pinned.file;
    let mut h = blake3::Hasher::new();
    h.update(b"layrs-file-v1\0");
    let mut buffer = [0u8; 64 * 1024];
    let mut count = 0u64;
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > record.bytes {
            return Err(invalid("projection file grew"));
        }
        h.update(&buffer[..n]);
    }
    if count != record.bytes || format!("blake3:{}", h.finalize()) != record.file_id.as_str() {
        return Err(invalid("projection file content changed"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if (pinned.file.metadata()?.permissions().mode() & 0o111 != 0) != record.executable {
            return Err(invalid("projection executable mode changed"));
        }
    }
    pinned.verify(path)?;
    Ok(())
}
pub(super) fn set_mode(file: &fs::File, executable: bool) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(if executable {
            0o700
        } else {
            0o600
        }))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (file, executable);
    }
    Ok(())
}
pub(super) struct FileWriter<'a> {
    out: &'a mut dyn Write,
    hasher: blake3::Hasher,
    count: u64,
    limit: u64,
    failed: bool,
}
impl<'a> FileWriter<'a> {
    pub fn new(out: &'a mut dyn Write, limit: u64) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(b"layrs-file-v1\0");
        Self {
            out,
            hasher: h,
            count: 0,
            limit,
            failed: false,
        }
    }
    pub fn finish(self, expected: &FileId) -> Result<u64> {
        if self.failed || format!("blake3:{}", self.hasher.finalize()) != expected.as_str() {
            return Err(invalid("hydrated file does not match its exact FileId"));
        }
        Ok(self.count)
    }
}
impl Write for FileWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.failed || bytes.len() as u64 > self.limit.saturating_sub(self.count) {
            self.failed = true;
            return Err(io::Error::other("projection file exceeds streaming budget"));
        }
        match self.out.write(bytes) {
            Ok(n) => {
                self.hasher.update(&bytes[..n]);
                self.count += n as u64;
                Ok(n)
            }
            Err(e) => {
                self.failed = true;
                Err(e)
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.out.flush().inspect_err(|_| self.failed = true)
    }
}
