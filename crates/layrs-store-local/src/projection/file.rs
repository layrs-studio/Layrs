use super::*;
use std::fs::File;

/// A read handle without DELETE access, compatible with SQLite's Windows share
/// flags. Existing CAS PinnedFile deliberately requests DELETE and cannot be
/// retained while SQLite opens the same file.
pub(super) struct DatabasePin {
    pub file: File,
    identity: String,
}
impl DatabasePin {
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn open(path: &Path) -> Result<Self> {
        let file = crate::layout::open_read_no_follow(path)?;
        let identity = identity(&file)?;
        Ok(Self { file, identity })
    }
    pub fn verify(&self, path: &Path) -> Result<()> {
        let other = Self::open(path)?;
        if other.identity != self.identity || identity(&self.file)? != self.identity {
            return Err(invalid("projection database file replaced"));
        }
        Ok(())
    }
}
#[cfg(unix)]
fn identity(file: &File) -> io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata()?;
    Ok(format!("{}:{}", m.dev(), m.ino()))
}
#[cfg(windows)]
fn identity(file: &File) -> io::Result<String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: file remains live and info is writable for the duration of call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(format!(
        "{}:{}:{}",
        info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow
    ))
}
#[cfg(not(any(unix, windows)))]
fn identity(_: &File) -> io::Result<String> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "projection requires stable file identities",
    ))
}
