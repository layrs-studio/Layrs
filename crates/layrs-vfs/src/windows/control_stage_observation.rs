//! Retained observation of an independently allocated stage, never publication
//! authority or authentication of its receipt or intended FORMAT bytes.
use super::{
    api::Api,
    control_directory::match_root,
    control_directory_io as io,
    control_directory_privacy::{Privacy, READ_CONTROL},
    fs,
};
use crate::{ControlDirectoryStageIdentity, Error, Result};
use std::{fs::File, io::Read, path::Path};
use windows_sys::Win32::{Foundation::ERROR_FILE_NOT_FOUND, Storage::FileSystem::*};

/// A point-in-time, nonrecursive observation of entries in an ordinary root. It does
/// not prove absence of registrations beneath unrelated user directories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObservedRootNamespace {
    Empty,
    Nonempty,
}

/// Keeps root/parent pins, a SHARE_ALL stage observer and a strong FORMAT reader.
/// Drop this object before publishing: its FORMAT reader can refuse the native
/// directory rename. No conversion to a guard or expected receipt is provided.
pub struct ObservedAllocatedControlStage {
    _root: io::RootPins,
    _parent: io::RootPins,
    _directory: File,
    _format: File,
    identity: ControlDirectoryStageIdentity,
    bytes: Vec<u8>,
    namespace: ObservedRootNamespace,
}
impl ObservedAllocatedControlStage {
    pub fn identity(&self) -> ObservedAllocatedControlStageIdentity<'_> {
        ObservedAllocatedControlStageIdentity(&self.identity)
    }
    pub fn format_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn root_namespace(&self) -> ObservedRootNamespace {
        self.namespace
    }
}

/// Borrowed comparison only: all eight physical/path fields, including exact
/// case, are compared even when the supplied receipt differs from the input.
pub struct ObservedAllocatedControlStageIdentity<'a>(&'a ControlDirectoryStageIdentity);
impl ObservedAllocatedControlStageIdentity<'_> {
    pub fn matches_expected(&self, expected: &ControlDirectoryStageIdentity) -> bool {
        self.0 == expected
    }
}

/// Read 0..=65536 bytes from the exact independently allocated FORMAT. The
/// caller authenticates allocation ownership and decides what these bytes mean.
/// This reads ordinary controls only; no registration, connection, hydration,
/// repair, publication, SQL or intended-byte authority is acquired.
///
/// Exact .layrs absence is checked with no-follow native opens. A distinct
/// case-sensitive .LAYRS is merely a root entry; resolving aliases are collisions.
/// Registration and namespace checks are boundaries, not atomic snapshots.
pub fn observe_allocated_control_stage(
    expected: &ControlDirectoryStageIdentity,
) -> Result<ObservedAllocatedControlStage> {
    expected.validate()?; // Pure bounds/lexical checks precede even native loading.
    let api = Api::load()?;
    let privacy = Privacy::load()?;
    let root = io::RootPins::open(expected.root(), false)?;
    match_allocated_root(&root, expected)?;
    root.outside_sync_roots(&api)?;
    io::stream_inventory(root.file(), true)?;
    let parent = io::RootPins::open(expected.stage_parent(), false)?;
    if parent.root != expected.stage_parent()
        || parent.identity() != (expected.volume, expected.parent_index)
        || parent.contains(root.identity())
    {
        return Err(Error::IdentityMismatch);
    }
    parent.outside_sync_roots(&api)?;
    final_absent(&root)?;
    let directory = io::directory_observer(expected.stage())?;
    stage_boundary(&api, &privacy, &directory, expected)?;
    let stage_attributes = fs::attributes(&directory)?.FileAttributes;
    // All containing directories are bound and positively NOT_UNDER before
    // reading any directory entries, including the closed control layout.
    io::only_child(expected.stage(), "FORMAT")?;
    let path = expected.stage().join("FORMAT");
    let metadata = format_observer(&path)?;
    let initial = format_boundary_one(&api, &privacy, &metadata, expected)?;
    #[cfg(test)]
    fault::at(fault::Point::BeforeStrong, None)?;
    let format = io::format_pin(&path)?;
    if format_boundary_one(&api, &privacy, &format, expected)? != initial {
        return Err(Error::IdentityMismatch);
    }
    drop(metadata);
    #[cfg(test)]
    fault::at(fault::Point::BeforeRead, Some(&format))?;
    let bytes = read_allocated_format(&format, &privacy)?;
    if bytes.len() as u64 != initial.size
        || format_boundary_one(&api, &privacy, &format, expected)? != initial
    {
        return Err(Error::IdentityMismatch);
    }
    let current = format_observer(&path)?;
    if format_boundary_one(&api, &privacy, &current, expected)? != initial {
        return Err(Error::IdentityMismatch);
    }
    stage_boundary(&api, &privacy, &directory, expected)?;
    if fs::attributes(&directory)?.FileAttributes != stage_attributes {
        return Err(Error::IdentityMismatch);
    }
    parent.outside_sync_roots(&api)?;
    root.outside_sync_roots(&api)?;
    io::only_child(expected.stage(), "FORMAT")?;
    let namespace = if std::fs::read_dir(&root.root)?.next().transpose()?.is_some() {
        ObservedRootNamespace::Nonempty
    } else {
        ObservedRootNamespace::Empty
    };
    #[cfg(test)]
    fault::at(fault::Point::BeforeFinalBoundary, None)?;
    final_absent(&root)?;
    stage_boundary(&api, &privacy, &directory, expected)?;
    io::only_child(expected.stage(), "FORMAT")?;
    stage_boundary(&api, &privacy, &directory, expected)?;
    if fs::attributes(&directory)?.FileAttributes != stage_attributes {
        return Err(Error::IdentityMismatch);
    }
    if format_boundary_one(&api, &privacy, &format, expected)? != initial {
        return Err(Error::IdentityMismatch);
    }
    let current = format_observer(&path)?;
    if format_boundary_one(&api, &privacy, &current, expected)? != initial {
        return Err(Error::IdentityMismatch);
    }
    io::stream_inventory(root.file(), true)?;
    parent.outside_sync_roots(&api)?;
    root.outside_sync_roots(&api)?; // Last registration check, after every open/read.
    match_allocated_root(&root, expected)?;
    Ok(ObservedAllocatedControlStage {
        _root: root,
        _parent: parent,
        _directory: directory,
        _format: format,
        identity: expected.clone(),
        bytes,
        namespace,
    })
}

fn match_allocated_root(
    root: &io::RootPins,
    expected: &ControlDirectoryStageIdentity,
) -> Result<()> {
    // Path equality normalizes Windows drive-letter case. This observation
    // binds the exact canonical spelling as well as the existing native IDs.
    if root.root.as_os_str() != expected.root().as_os_str() {
        return Err(Error::IdentityMismatch);
    }
    match_root(root, expected)
}

fn final_absent(root: &io::RootPins) -> Result<()> {
    absence_result(fs::open(
        &root.root.join(".layrs"),
        io::ALL_SHARING,
        FILE_READ_ATTRIBUTES,
    ))
}
fn absence_result<T>(result: Result<T>) -> Result<()> {
    match result {
        Ok(_) => Err(Error::ForeignRoot),
        Err(Error::Io(error)) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => {
            Ok(())
        }
        Err(error) => Err(error),
    }
}
fn stage_boundary(
    api: &Api,
    privacy: &Privacy,
    held: &File,
    expected: &ControlDirectoryStageIdentity,
) -> Result<()> {
    let fresh = io::directory_observer(expected.stage())?;
    for file in [held, &fresh] {
        if io::id(file)? != (expected.volume, expected.directory_index)
            || io::canonical(file)? != expected.stage
        {
            return Err(Error::IdentityMismatch);
        }
        io::not_under(api, file)?;
        io::control_directory_attributes(file)?;
        privacy.verify(file)?;
        io::stream_inventory(file, true)?;
    }
    Ok(())
}
fn format_observer(path: &Path) -> Result<File> {
    fs::open(path, io::ALL_SHARING, FILE_READ_ATTRIBUTES | READ_CONTROL)
}
#[derive(PartialEq, Eq)]
struct FormatSnapshot {
    size: u64,
    attributes: u32,
}
fn format_boundary_one(
    api: &Api,
    privacy: &Privacy,
    file: &File,
    expected: &ControlDirectoryStageIdentity,
) -> Result<FormatSnapshot> {
    if io::id(file)? != (expected.volume, expected.format_index)
        || io::canonical(file)? != format!("{}\\FORMAT", expected.stage)
    {
        return Err(Error::IdentityMismatch);
    }
    io::not_under(api, file)?;
    io::format_attributes(file)?;
    privacy.verify(file)?;
    io::stream_inventory(file, false)?;
    Ok(FormatSnapshot {
        size: fs::size(&fs::identity(file)?),
        attributes: fs::attributes(file)?.FileAttributes,
    })
}

// Do not widen Step1's 1..=65536 FORMAT reader or change its errors. An empty
// allocated file is valid observation data, never a publishable FORMAT.
fn read_allocated_format(mut file: &File, privacy: &Privacy) -> Result<Vec<u8>> {
    if fs::size(&fs::identity(file)?) != 0 {
        return io::read_format(file, privacy);
    }
    io::format_attributes(file)?;
    privacy.verify(file)?;
    io::stream_inventory(file, false)?;
    if file.read(&mut [0; 1])? != 0 || fs::size(&fs::identity(file)?) != 0 {
        return Err(Error::IdentityMismatch);
    }
    io::format_attributes(file)?;
    privacy.verify(file)?;
    io::stream_inventory(file, false)?;
    Ok(Vec::new())
}

#[cfg(test)]
mod fault {
    use super::*;
    use std::cell::RefCell;
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum Point {
        BeforeStrong,
        BeforeRead,
        BeforeFinalBoundary,
    }
    type Hook = Box<dyn Fn(Point, Option<&File>) -> Result<()>>;
    std::thread_local! { static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) }; }
    pub(super) fn at(point: Point, file: Option<&File>) -> Result<()> {
        HOOK.with(|hook| match &*hook.borrow() {
            Some(hook) => hook(point, file),
            None => Ok(()),
        })
    }
    pub(super) fn scoped<T>(hook: Hook, action: impl FnOnce() -> T) -> T {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                HOOK.with(|hook| *hook.borrow_mut() = None);
            }
        }
        HOOK.with(|slot| assert!(slot.borrow_mut().replace(hook).is_none()));
        let _reset = Reset;
        action()
    }
}

#[cfg(test)]
#[path = "control_stage_observation_tests.rs"]
mod tests;
