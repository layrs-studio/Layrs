//! Hybrid virtual Rooms (user decision of 5 November 2026): at creation, small
//! files are written as ordinary files, then converted into hydrated in-sync
//! placeholders as soon as the provider is connected (CFAPI converts only
//! under a connected root). Reading them never needs a hydration callback,
//! whose per-file cost is mostly in Windows (about 1.6 ms of a 1.8 ms first
//! read, measured on 10,000 files).
//!
//! Safety of the in-sync mark: each file is created `CREATE_NEW` through a
//! handle that denies other writers and deleters, filled with bytes the
//! source proved (`read_checked`), and converted through that same handle.
//! No one else can have written it in between, so in-sync always describes
//! the catalog's bytes. A crash before conversion leaves ordinary files, which
//! captures hash like any new file (`NeedsHash`): nothing is ever marked in
//! sync except through the handle that wrote it.
use super::{
    api::{Api, check},
    fs,
};
use crate::{Cancellation, Catalog, Error, Result, SnapshotSource, catalog::read_checked};
use std::{
    collections::BTreeMap,
    ffi::c_void,
    fs::File,
    io::Write,
    os::windows::io::FromRawHandle,
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use windows_sys::Win32::{
    Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE},
    Storage::{CloudFilters::*, FileSystem::*},
};

/// Largest file written physically at creation.
pub const MAX_WARM_FILE_BYTES: u64 = 64 * 1024;
/// Default total of bytes written physically at creation.
pub const DEFAULT_WARM_BUDGET_BYTES: u64 = 256 * 1024 * 1024;
/// Files of one directory written by one worker before it takes another.
const WORK_ITEM_FILES: usize = 64;

/// Catalog indices written physically: non-empty files of at most
/// `MAX_WARM_FILE_BYTES`, in catalog order, while their total stays within
/// `budget`. Larger or later files stay virtual placeholders.
pub(super) fn select(catalog: &Catalog, budget: u64) -> Vec<usize> {
    let mut total = 0_u64;
    let mut selected = Vec::new();
    for (index, entry) in catalog.entries().iter().enumerate() {
        if entry.size == 0 || entry.size > MAX_WARM_FILE_BYTES {
            continue;
        }
        let Some(next) = total.checked_add(entry.size).filter(|t| *t <= budget) else {
            continue;
        };
        total = next;
        selected.push(index);
    }
    selected
}

/// Ordinary files written and still held for conversion, by catalog index,
/// and the summed worker time spent reading the source and writing files.
pub(super) struct Written(
    Vec<(usize, File)>,
    pub std::time::Duration,
    pub std::time::Duration,
);

fn create_new(path: &Path) -> Result<File> {
    let name = fs::native_path(path)?;
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ,
            std::ptr::null(),
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().into());
    }
    // The File owns exactly one successful CreateFileW handle.
    Ok(unsafe { File::from_raw_handle(handle) })
}

fn workers() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8)
}

/// Writes `selected` entries as ordinary files under `root`, in parallel.
/// Every ancestor directory is pinned (`RootGuard::parents`) while its files
/// are created. The first error stops the remaining work and is returned;
/// files already created stay (no rollback deletes anything).
pub(super) fn write(
    root: &fs::RootGuard,
    catalog: &Catalog,
    source: &dyn SnapshotSource,
    selected: &[usize],
) -> Result<Written> {
    let mut by_parent: BTreeMap<&Path, Vec<usize>> = BTreeMap::new();
    for &index in selected {
        let parent = Path::new(catalog.entries()[index].path.as_str())
            .parent()
            .ok_or(Error::IdentityMismatch)?;
        by_parent.entry(parent).or_default().push(index);
    }
    let items: Vec<&[usize]> = by_parent
        .values()
        .flat_map(|indices| indices.chunks(WORK_ITEM_FILES))
        .collect();
    let next = AtomicUsize::new(0);
    let written = Mutex::new(Vec::with_capacity(selected.len()));
    let failure: Mutex<Option<Error>> = Mutex::new(None);
    let cancel = Cancellation::default();
    let reading = std::sync::atomic::AtomicU64::new(0);
    let creating = std::sync::atomic::AtomicU64::new(0);
    std::thread::scope(|scope| {
        for _ in 0..workers().min(items.len()) {
            scope.spawn(|| {
                let mut buffer = Vec::new();
                loop {
                    let position = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(position) else {
                        return;
                    };
                    if cancel.is_cancelled() {
                        return;
                    }
                    let result = (|| -> Result<Vec<(usize, File)>> {
                        let first = catalog.entries()[item[0]].path.as_str();
                        let _parents = root.parents(first)?;
                        let mut files = Vec::with_capacity(item.len());
                        for &index in *item {
                            let entry = &catalog.entries()[index];
                            buffer.resize(entry.size as usize, 0);
                            let started = std::time::Instant::now();
                            read_checked(source, entry, 0, &mut buffer, &cancel)?;
                            let read = started.elapsed();
                            let mut file = create_new(&root.root.join(entry.path.as_str()))?;
                            file.write_all(&buffer)?;
                            reading.fetch_add(read.as_nanos() as u64, Ordering::Relaxed);
                            creating.fetch_add(
                                (started.elapsed() - read).as_nanos() as u64,
                                Ordering::Relaxed,
                            );
                            files.push((index, file));
                        }
                        Ok(files)
                    })();
                    match result {
                        Ok(files) => written.lock().expect("warm writes").extend(files),
                        Err(error) => {
                            cancel.cancel();
                            failure.lock().expect("warm failure").get_or_insert(error);
                            return;
                        }
                    }
                }
            });
        }
    });
    if let Some(error) = failure.into_inner().expect("warm failure") {
        return Err(error);
    }
    Ok(Written(
        written.into_inner().expect("warm writes"),
        std::time::Duration::from_nanos(reading.into_inner()),
        std::time::Duration::from_nanos(creating.into_inner()),
    ))
}

/// Converts every written file into a hydrated in-sync placeholder carrying
/// its catalog identity, through the handle that wrote it. Requires the
/// provider to be connected (CFAPI otherwise reports success without effect).
pub(super) fn convert(api: &Api, catalog: &Catalog, written: Written) -> Result<()> {
    let files = written.0;
    let next = AtomicUsize::new(0);
    let failure: Mutex<Option<Error>> = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..workers().min(files.len()) {
            scope.spawn(|| {
                loop {
                    let position = next.fetch_add(1, Ordering::Relaxed);
                    let Some((index, file)) = files.get(position) else {
                        return;
                    };
                    let identity = catalog.file_identity(*index);
                    let result = check("CfConvertToPlaceholder", unsafe {
                        (api.convert)(
                            fs::handle(file),
                            identity.as_ptr().cast::<c_void>(),
                            identity.len() as u32,
                            CF_CONVERT_FLAG_MARK_IN_SYNC,
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                        )
                    });
                    if let Err(error) = result {
                        failure.lock().expect("warm failure").get_or_insert(error);
                        next.store(files.len(), Ordering::Relaxed);
                        return;
                    }
                }
            });
        }
    });
    match failure.into_inner().expect("warm failure") {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
