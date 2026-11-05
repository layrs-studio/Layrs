//! Snapshot an explicitly trusted executable into an immutable sealed memfd.
//! Holding a Linux file descriptor alone would not prevent in-place rewrites.
use super::*;
use std::{
    ffi::CString,
    fs::File,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    },
    path::{Component, PathBuf},
};

fn opened(fd: libc::c_int) -> io::Result<File> {
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

pub(crate) fn pin_image(path: &Path, expected: &str) -> Result<(PathBuf, File), WorkerError> {
    let absolute = std::path::absolute(path)?;
    let mut source = opened(unsafe {
        libc::open(
            c"/".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    })?;
    let components: Vec<_> = absolute.components().collect();
    for (index, component) in components.iter().enumerate() {
        match component {
            Component::RootDir => continue,
            Component::Normal(name) => {
                let name = CString::new(name.as_bytes())
                    .map_err(|_| io::Error::other("worker path contains NUL"))?;
                let directory = if index + 1 == components.len() {
                    0
                } else {
                    libc::O_DIRECTORY
                };
                source = opened(unsafe {
                    libc::openat(
                        source.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_RDONLY
                            | libc::O_NOFOLLOW
                            | libc::O_CLOEXEC
                            | libc::O_NONBLOCK
                            | directory,
                    )
                })?;
            }
            _ => {
                return Err(WorkerError::Protocol(
                    "worker path must not contain traversal".into(),
                ));
            }
        }
    }
    const MAX_IMAGE: u64 = 256 * 1024 * 1024;
    let metadata = source.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_IMAGE {
        return Err(WorkerError::Protocol(
            "worker image must be a bounded regular file".into(),
        ));
    }
    let mut bytes = Vec::new();
    source.take(MAX_IMAGE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_IMAGE || layrs_lens_sdk::sha256_digest(&bytes) != expected {
        return Err(WorkerError::Protocol("worker image digest mismatch".into()));
    }
    if !bytes.starts_with(b"\x7fELF") {
        return Err(WorkerError::Protocol(
            "Linux worker must be a native ELF image".into(),
        ));
    }
    let mut image = opened(unsafe {
        libc::memfd_create(
            c"layrs-lens-worker".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    })?;
    image.write_all(&bytes)?;
    let seals = libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL;
    if unsafe { libc::fchmod(image.as_raw_fd(), 0o500) } != 0
        || unsafe { libc::fcntl(image.as_raw_fd(), libc::F_ADD_SEALS, seals) } != 0
        || unsafe { libc::fcntl(image.as_raw_fd(), libc::F_GET_SEALS) } != seals
    {
        return Err(io::Error::last_os_error().into());
    }
    // proc lookup happens before CLOEXEC closes this descriptor. Unlike a script,
    // this ELF image does not need the fd to survive into its interpreter.
    Ok((
        PathBuf::from(format!("/proc/self/fd/{}", image.as_raw_fd())),
        image,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sealed_image_cannot_be_rewritten_resized_or_unsealed() {
        let source = std::env::current_exe().unwrap();
        let digest = layrs_lens_sdk::sha256_digest(&std::fs::read(&source).unwrap());
        let (_, mut image) = pin_image(&source, &digest).unwrap();
        assert!(image.write_all(b"change").is_err());
        assert!(image.set_len(0).is_err());
        assert_eq!(
            unsafe {
                libc::fcntl(
                    image.as_raw_fd(),
                    libc::F_ADD_SEALS,
                    libc::F_SEAL_FUTURE_WRITE,
                )
            },
            -1
        );
        assert!(pin_image(&source, "sha256:wrong").is_err());
    }
}
