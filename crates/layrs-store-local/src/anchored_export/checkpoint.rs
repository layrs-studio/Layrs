//! Single-file Git acquisition checkpoint publication under a retained parent.
use super::*;
use crate::secure_fs::PinnedFile;
use std::sync::atomic::{AtomicU64, Ordering};

static SERIAL: AtomicU64 = AtomicU64::new(0);

/// Capture before acquisition, so a moved/replaced parent cannot redirect the
/// later temporary file or its final no-replace publication.
pub struct ScanCheckpointDestination {
    path: PathBuf,
    parent: PinnedDirectory,
    _locks: Vec<File>,
}
impl ScanCheckpointDestination {
    pub fn capture(path: &Path) -> io::Result<Self> {
        let path = std::path::absolute(path)?;
        let parent_path = path.parent().ok_or_else(stale)?;
        let parent = PinnedDirectory::open(parent_path)?;
        let locks = lock_paths(&[parent_path.to_path_buf()])?;
        parent.verify_path_identity()?;
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
            Ok(_) => return Err(io::ErrorKind::AlreadyExists.into()),
        }
        parent.verify_path_identity()?;
        Ok(Self {
            path,
            parent,
            _locks: locks,
        })
    }
    pub fn begin(&self) -> io::Result<ScanCheckpointWriter<'_>> {
        self.parent.verify_path_identity()?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let name = format!(
            ".layrs-git-scan-{}-{nonce}-{}.incomplete",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        );
        let file = self.parent.create_file(OsStr::new(&name))?;
        self.parent.verify_path_identity()?;
        Ok(ScanCheckpointWriter {
            destination: self,
            file,
        })
    }
}

/// Interrupted private siblings remain explicitly incomplete. Publication never
/// overwrites a caller file; filesystem-specific operations stay in secure_fs.
pub struct ScanCheckpointWriter<'a> {
    destination: &'a ScanCheckpointDestination,
    file: PinnedFile,
}
impl Write for ScanCheckpointWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.destination.parent.verify_path_identity()?;
        self.file.file_mut().write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.file_mut().flush()
    }
}
impl ScanCheckpointWriter<'_> {
    pub fn publish(self) -> io::Result<()> {
        let parent = &self.destination.parent;
        parent.verify_path_identity()?;
        self.file.file().sync_all()?;
        let name = self.destination.path.file_name().ok_or_else(stale)?;
        // rename_to is handle-relative and explicitly no-replace on Windows and
        // supported Unix platforms. No path-based sibling cleanup is necessary.
        if self.file.rename_to(parent, name)? {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        let published = parent.open_file(name)?;
        if !self.file.same_file(&published) {
            return Err(stale());
        }
        // The reopened handle verifies identity and intentionally has no write
        // access. Flush through the retained writable handle after publication.
        self.file.file().sync_all()?;
        parent.sync()?;
        parent.verify_path_identity()
    }
}
