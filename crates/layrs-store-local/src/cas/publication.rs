use std::ffi::OsStr;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use crate::fault::{self, FaultPoint};
use crate::layout::{StoreLayout, validate_store_boundaries};
use crate::secure_fs::{PinnedDirectory, PinnedFile};

use super::PendingObject;

pub(super) fn install_pending(
    layout: &StoreLayout,
    pending: &PendingObject,
    target: &Path,
    verify: impl Fn(&[u8]) -> io::Result<()>,
) -> io::Result<bool> {
    fault::hit(FaultPoint::BeforeTempHash)?;
    validate_store_boundaries(layout)?;
    let pending_name = contained_file_name(&layout.tmp_dir, &pending.path)?;
    let target_relative = target.strip_prefix(&layout.cas_dir).map_err(|_| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "CAS publication target escapes the CAS root",
        )
    })?;

    let root = PinnedDirectory::open(&layout.root)?;
    let temporary = root.open_child(required_name(&layout.tmp_dir)?)?;
    let mut source = temporary.open_file(pending_name)?;
    verify(&read_pinned(&mut source)?)?;

    let cas = root.open_child(required_name(&layout.cas_dir)?)?;
    let (destination, target_name) = cas.components_to_parent(target_relative)?;
    match destination.open_file(&target_name) {
        Ok(mut existing) => {
            verify(&read_pinned(&mut existing)?)?;
            cleanup_pending(&source)?;
            return Ok(true);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    fault::hit(FaultPoint::BeforeRename)?;
    destination.verify_path_identity()?;
    let result = publish(layout, &source, &destination, &target_name, &verify);
    source.remove_replacement()?;
    result
}

#[cfg(unix)]
fn publish(
    _layout: &StoreLayout,
    source: &PinnedFile,
    destination: &PinnedDirectory,
    target_name: &OsStr,
    verify: &impl Fn(&[u8]) -> io::Result<()>,
) -> io::Result<bool> {
    if source.hard_link_to(destination, target_name)? {
        return finish_existing(source, destination, target_name, verify);
    }

    verify_installed(source, destination, target_name, verify)?;
    fault::hit(FaultPoint::AfterCasLink)?;
    fault::hit(FaultPoint::AfterRename)?;
    fault::hit(FaultPoint::BeforeCasDirectorySync)?;
    fault::hit(FaultPoint::BeforeDirectorySync)?;
    destination.sync()?;
    fault::hit(FaultPoint::AfterCasDirectorySync)?;
    fault::hit(FaultPoint::BeforePendingUnlink)?;
    match source.unlink() {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fault::hit(FaultPoint::AfterPendingUnlink)?;
    fault::hit(FaultPoint::BeforePendingDirectorySync)?;
    source.sync_parent()?;
    fault::hit(FaultPoint::AfterPendingDirectorySync)?;
    destination.verify_path_identity()?;
    Ok(false)
}

#[cfg(windows)]
fn publish(
    _layout: &StoreLayout,
    source: &PinnedFile,
    destination: &PinnedDirectory,
    target_name: &OsStr,
    verify: &impl Fn(&[u8]) -> io::Result<()>,
) -> io::Result<bool> {
    if source.rename_to(destination, target_name)? {
        return finish_existing(source, destination, target_name, verify);
    }
    verify_installed(source, destination, target_name, verify)?;
    fault::hit(FaultPoint::AfterRename)?;
    fault::hit(FaultPoint::BeforeDirectorySync)?;
    destination.sync()?;
    source.sync_parent()?;
    destination.verify_path_identity()?;
    Ok(false)
}

#[cfg(not(any(unix, windows)))]
fn publish(
    _layout: &StoreLayout,
    _source: &PinnedFile,
    _destination: &PinnedDirectory,
    _target_name: &OsStr,
    _verify: &impl Fn(&[u8]) -> io::Result<()>,
) -> io::Result<bool> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-clobber publication is unsupported on this platform",
    ))
}

fn finish_existing(
    source: &PinnedFile,
    destination: &PinnedDirectory,
    target_name: &OsStr,
    verify: &impl Fn(&[u8]) -> io::Result<()>,
) -> io::Result<bool> {
    let mut existing = destination.open_file(target_name)?;
    let bytes = read_pinned(&mut existing)?;
    verify(&bytes)?;
    cleanup_pending(source)?;
    Ok(true)
}

fn verify_installed(
    source: &PinnedFile,
    destination: &PinnedDirectory,
    target_name: &OsStr,
    verify: &impl Fn(&[u8]) -> io::Result<()>,
) -> io::Result<()> {
    let mut installed = match destination.open_file(target_name) {
        Ok(installed) => installed,
        Err(error) => {
            remove_failed_install_by_name(destination, target_name)?;
            return Err(error);
        }
    };
    if !source.same_file(&installed) {
        remove_failed_install(&installed, destination)?;
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "pending source changed during CAS publication",
        ));
    }
    if let Err(error) = read_pinned(&mut installed).and_then(|bytes| verify(&bytes)) {
        remove_failed_install(&installed, destination)?;
        return Err(error);
    }
    Ok(())
}

fn remove_failed_install(installed: &PinnedFile, destination: &PinnedDirectory) -> io::Result<()> {
    installed.unlink()?;
    destination.sync()
}

fn remove_failed_install_by_name(
    destination: &PinnedDirectory,
    target_name: &OsStr,
) -> io::Result<()> {
    destination.unlink_file(target_name)?;
    destination.sync()
}

fn cleanup_pending(source: &PinnedFile) -> io::Result<()> {
    source.verify_parent()?;
    source.remove_replacement()?;
    match source.unlink() {
        Ok(()) => source.sync_parent(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn read_pinned(file: &mut PinnedFile) -> io::Result<Vec<u8>> {
    let file = file.file_mut();
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn contained_file_name<'a>(parent: &Path, path: &'a Path) -> io::Result<&'a OsStr> {
    if path.parent() != Some(parent) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "pending object path escapes the temporary directory",
        ));
    }
    path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "pending object has no file name",
        )
    })
}

fn required_name(path: &Path) -> io::Result<&OsStr> {
    path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("store directory has no file name: {}", path.display()),
        )
    })
}
