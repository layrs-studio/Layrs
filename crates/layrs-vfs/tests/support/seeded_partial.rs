//! Test-only residency qualification. No production API/policy changes and no
//! widening of either seed window. The original unseeded fixture stays intact.
use super::{
    assert_rename_capture_refused, child, disposable_run, export_cause, native_file_id,
    placeholder_probe,
};
use layrs_core::scm::{FileId, RepoPath};
use layrs_vfs::{
    Cancellation, Catalog, CatalogEntry, Error, PlaceholderEvidence, Result, Session,
    SnapshotSource,
};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    mem::{size_of, size_of_val},
    ops::Range,
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{FreeLibrary, HANDLE, HMODULE},
    Storage::{CloudFilters::*, FileSystem::*},
    System::{LibraryLoader::*, IO::OVERLAPPED},
};

fn probe_child(root: &Path, action: &str, value: &str) -> ProbeResult<()> {
    let mut process = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .args(["--ignored", "--exact", "native_child_io", "--nocapture"])
        .env("LAYRS_VFS_CHILD_ROOT", root)
        .env("LAYRS_VFS_CHILD_ACTION", action)
        .env("LAYRS_VFS_CHILD_VALUE", value)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("{action} spawn: {e}"))?;
    let start = Instant::now();
    loop {
        if let Some(status) = process.try_wait().map_err(|e| e.to_string())? {
            return if status.success() {
                Ok(())
            } else {
                Err(format!("{action} child failed: {status}"))
            };
        }
        if start.elapsed() >= Duration::from_secs(45) {
            process
                .kill()
                .map_err(|e| format!("{action} timeout; kill failed: {e}"))?;
            process.wait().map_err(|e| e.to_string())?;
            return Err(format!(
                "{action} native child timed out after 45 s; stopped and waited"
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

const SIZE: usize = 16 * 1024 * 1024 + 37;
const TARGET: Range<u64> = 65536..69632;
const WITNESS: Range<u64> = 8388608..8392704;
const SEED: usize = 0;
const DENY: usize = 1;
const RESTORE: usize = 2;

type ProbeResult<T> = std::result::Result<T, String>;
type HydrateFn =
    unsafe extern "system" fn(HANDLE, i64, i64, CF_HYDRATE_FLAGS, *mut OVERLAPPED) -> i32;
type RangesFn = unsafe extern "system" fn(
    HANDLE,
    CF_PLACEHOLDER_RANGE_INFO_CLASS,
    i64,
    i64,
    *mut std::ffi::c_void,
    u32,
    *mut u32,
) -> i32;

struct TestApi {
    module: HMODULE,
    hydrate: HydrateFn,
    ranges: RangesFn,
}
impl TestApi {
    fn load() -> ProbeResult<Self> {
        let name: Vec<u16> = "cldapi.dll\0".encode_utf16().collect();
        let module = unsafe {
            LoadLibraryExW(
                name.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        if module.is_null() {
            return Err(format!("load CFAPI: {}", std::io::Error::last_os_error()));
        }
        let hydrate = unsafe { GetProcAddress(module, c"CfHydratePlaceholder".as_ptr().cast()) };
        let ranges =
            unsafe { GetProcAddress(module, c"CfGetPlaceholderRangeInfo".as_ptr().cast()) };
        let (Some(hydrate), Some(ranges)) = (hydrate, ranges) else {
            unsafe {
                FreeLibrary(module);
            }
            return Err("required test-only CFAPI range/hydration entry point absent".into());
        };
        Ok(Self {
            module,
            hydrate: unsafe {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, HydrateFn>(hydrate)
            },
            ranges: unsafe {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, RangesFn>(ranges)
            },
        })
    }
}
impl Drop for TestApi {
    fn drop(&mut self) {
        unsafe {
            FreeLibrary(self.module);
        }
    }
}

fn original_byte(offset: u64) -> u8 {
    ((offset * 17 + offset / 251) % 251) as u8
}

#[derive(Debug)]
struct ReadEvent {
    mode: usize,
    offset: u64,
    length: usize,
    allowed: bool,
}
#[derive(Default)]
struct Events {
    rows: Vec<ReadEvent>,
    dropped: usize,
    allowed: usize,
    denied: usize,
}
struct WindowSource {
    catalog: Catalog,
    bytes: Vec<u8>,
    window: Range<u64>,
    mode: AtomicUsize,
    reads: AtomicUsize,
    supplied: AtomicUsize,
    events: Mutex<Events>,
}
impl WindowSource {
    fn new(window: Range<u64>) -> Self {
        let bytes: Vec<u8> = (0..SIZE as u64).map(original_byte).collect();
        let catalog = Catalog::new(
            1,
            vec![CatalogEntry {
                path: RepoPath::parse("mixed.bin").unwrap(),
                file_id: FileId::from_bytes(&bytes),
                size: SIZE as u64,
            }],
        )
        .unwrap();
        Self {
            catalog,
            bytes,
            window,
            mode: AtomicUsize::new(SEED),
            reads: AtomicUsize::new(0),
            supplied: AtomicUsize::new(0),
            events: Mutex::new(Events::default()),
        }
    }
    fn log(&self, label: &str, phase: &str) {
        let mut events = self.events.lock().unwrap();
        eprintln!(
            "SEEDED SOURCE {label}/{phase}: window={:?} mode={} total_reads={} supplied={} allowed={} denied={} omitted_events={}",
            self.window,
            self.mode.load(Ordering::SeqCst),
            self.reads.load(Ordering::SeqCst),
            self.supplied.load(Ordering::SeqCst),
            events.allowed,
            events.denied,
            events.dropped
        );
        for row in &events.rows {
            eprintln!(
                "  source mode={} offset={} length={} allowed={}",
                row.mode, row.offset, row.length, row.allowed
            );
        }
        *events = Events::default();
    }
}
impl SnapshotSource for WindowSource {
    fn catalog(&self) -> &Catalog {
        &self.catalog
    }
    fn read_exact_at(
        &self,
        id: &FileId,
        offset: u64,
        output: &mut [u8],
        cancel: &Cancellation,
    ) -> Result<()> {
        cancel.check()?;
        assert!(output.len() <= layrs_vfs::READ_BUFFER_SIZE);
        if id != &self.catalog.entries()[0].file_id {
            return Err(Error::IdentityMismatch);
        }
        let range = layrs_vfs::checked_range(SIZE as u64, offset, output.len() as u64)?;
        let mode = self.mode.load(Ordering::SeqCst);
        let allowed = mode == RESTORE
            || (mode == SEED && range.start >= self.window.start && range.end <= self.window.end);
        self.reads.fetch_add(1, Ordering::SeqCst);
        {
            let mut events = self.events.lock().unwrap();
            if allowed {
                events.allowed += 1;
            } else {
                events.denied += 1;
            }
            if events.rows.len() < 128 {
                events.rows.push(ReadEvent {
                    mode,
                    offset,
                    length: output.len(),
                    allowed,
                });
            } else {
                events.dropped += 1;
            }
        }
        if !allowed {
            return Err(Error::MissingBytes {
                file_id: id.clone(),
                offset,
                length: output.len(),
            });
        }
        // Reject the ENTIRE request before copying if any byte is disallowed.
        output.copy_from_slice(&self.bytes[range.start as usize..range.end as usize]);
        self.supplied.fetch_add(output.len(), Ordering::SeqCst);
        Ok(())
    }
}

fn ranges(
    api: &TestApi,
    path: &Path,
    class: CF_PLACEHOLDER_RANGE_INFO_CLASS,
) -> ProbeResult<Vec<Range<u64>>> {
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .map_err(|e| format!("range probe open: {e}"))?;
    let mut storage = [CF_FILE_RANGE::default(); 64];
    let mut returned = 0;
    let hr = unsafe {
        (api.ranges)(
            file.as_raw_handle(),
            class,
            0,
            SIZE as i64,
            storage.as_mut_ptr().cast(),
            size_of_val(&storage) as u32,
            &mut returned,
        )
    };
    if hr < 0 {
        return Err(format!(
            "range class {class}: HRESULT 0x{:08X}; returned={returned}; incomplete range proof (including MORE_DATA) is refused",
            hr as u32
        ));
    }
    if returned as usize > size_of_val(&storage)
        || returned as usize % size_of::<CF_FILE_RANGE>() != 0
    {
        return Err(format!(
            "range class {class}: malformed returned length {returned}"
        ));
    }
    let mut output = Vec::new();
    for value in &storage[..returned as usize / size_of::<CF_FILE_RANGE>()] {
        if value.StartingOffset < 0 || value.Length <= 0 {
            return Err(format!("range class {class}: negative/empty interval"));
        }
        let start = value.StartingOffset as u64;
        let end = start
            .checked_add(value.Length as u64)
            .filter(|end| *end <= SIZE as u64)
            .ok_or_else(|| {
                format!("range class {class}: interval overflow/outside logical file")
            })?;
        output.push(start..end);
    }
    output.sort_by_key(|r| r.start);
    if output.windows(2).any(|r| r[0].end > r[1].start) {
        return Err(format!(
            "range class {class}: overlapping returned intervals"
        ));
    }
    Ok(output)
}

fn covers(ranges: &[Range<u64>], wanted: Range<u64>) -> bool {
    let mut next = wanted.start;
    for range in ranges {
        if range.end <= next {
            continue;
        }
        if range.start > next {
            return false;
        }
        next = next.max(range.end);
        if next >= wanted.end {
            return true;
        }
    }
    false
}
fn absent(ranges: &[Range<u64>], wanted: Range<u64>) -> bool {
    ranges
        .iter()
        .all(|range| range.end <= wanted.start || range.start >= wanted.end)
}

#[derive(Debug)]
struct Residency {
    metadata: PlaceholderEvidence,
    on_disk: Vec<Range<u64>>,
    modified: Vec<Range<u64>>,
}
fn residency(api: &TestApi, session: &Session, path: &Path) -> ProbeResult<Residency> {
    // Independent metadata/ranges first: a provider evidence disagreement must
    // not be misreported as a platform residency limitation. Neither probe reads
    // content or connects a provider. Keep both range results for diagnostics.
    let native = placeholder_probe(path);
    let on_disk = ranges(api, path, CF_PLACEHOLDER_RANGE_INFO_ONDISK);
    let modified = ranges(api, path, CF_PLACEHOLDER_RANGE_INFO_MODIFIED);
    eprintln!(
        "SEEDED INDEPENDENT NATIVE path={} connected={} metadata={native:?} on_disk={on_disk:?} modified={modified:?}",
        path.display(),
        session.is_connected(),
    );
    let inspected = session
        .inspect(&RepoPath::parse("mixed.bin").unwrap())
        .map_err(|e| format!("metadata inspection: {e}"))?;
    eprintln!(
        "SEEDED PROVIDER INSPECTION path={:?} evidence={:?} placeholder={:?}",
        inspected.path, inspected.evidence, inspected.placeholder,
    );
    let Some(metadata) = inspected.placeholder.as_ref() else {
        let root = session.registration().root();
        eprintln!(
            "SEEDED EVIDENCE DISAGREEMENT registration_root={} native_path={}",
            root.display(),
            path.display(),
        );
        match std::fs::read_dir(root) {
            Ok(entries) => {
                // This fixture has one file; bound diagnostics without treating
                // an incomplete listing as evidence of absence.
                for (index, entry) in entries.take(17).enumerate() {
                    if index == 16 {
                        eprintln!("SEEDED EXACT NAMES: diagnostic limit reached");
                        break;
                    }
                    match entry {
                        Ok(entry) => eprintln!(
                            "SEEDED EXACT NAME {:?} native_id={:?}",
                            entry.file_name(),
                            native_file_id(&entry.path()),
                        ),
                        Err(error) => eprintln!("SEEDED EXACT NAME error={error:?}"),
                    }
                }
            }
            Err(error) => eprintln!("SEEDED EXACT NAMES open error={error:?}"),
        }
        panic!(
            "native CFAPI placeholder exists but provider returned no placeholder evidence: {:?}; fixture retained",
            inspected.evidence,
        );
    };
    assert_eq!(metadata.file_id as u64, native.native_id.1);
    assert_eq!(metadata.on_disk_bytes, native.on_disk as u64);
    assert_eq!(metadata.modified_bytes, native.modified as u64);
    // All transient Inspection/range handles close before the native write.
    Ok(Residency {
        metadata: metadata.clone(),
        on_disk: on_disk?,
        modified: modified?,
    })
}
fn require_residency(state: &Residency, dirty: bool) -> ProbeResult<()> {
    if !state.metadata.partial
        || state.metadata.on_disk_bytes == 0
        || state.metadata.on_disk_bytes >= SIZE as u64
    {
        return Err("file is not proven partially resident".into());
    }
    if !covers(&state.on_disk, TARGET) || !absent(&state.on_disk, WITNESS) {
        return Err("target page not fully resident or witness page is present".into());
    }
    if dirty {
        if state.metadata.modified_bytes == 0 || !covers(&state.modified, TARGET) {
            return Err("target page is not proven modified".into());
        }
    } else if state.metadata.modified_bytes != 0 || !state.modified.is_empty() {
        return Err("seed unexpectedly produced modified data".into());
    }
    Ok(())
}

fn log_native(session: &Session, label: &str, phase: &str) {
    eprintln!(
        "SEEDED CALLBACK SNAPSHOT {label}/{phase}: {:?}; failures={:?} (API exposes bounded recent history)",
        session.hydration_requests(),
        session.callback_failures()
    );
}

pub(super) fn run() {
    let (run, nonce) = disposable_run("seeded-dirty-partial");
    let mut qualified = 0;
    for (index, (label, window)) in [("A-page", TARGET), ("B-prefix", 0..262144)]
        .into_iter()
        .enumerate()
    {
        let case = run.join(label);
        std::fs::create_dir(&case).unwrap();
        match run_arm(&case, nonce + index as u128, label, window) {
            Ok(()) => {
                qualified += 1;
                eprintln!("SEEDED ARM QUALIFIED {label}; retained={}", case.display());
            }
            Err(reason) => eprintln!(
                "UNQUALIFIED SEEDED ARM {label}: {reason}; retained={}",
                case.display()
            ),
        }
    }
    eprintln!(
        "SEEDED MIXED-STATE RESULT: {qualified}/2 arms qualified; qualification is a missing-state boundary, NOT page-residency savings"
    );
}

fn run_arm(case: &Path, nonce: u128, label: &str, window: Range<u64>) -> ProbeResult<()> {
    let root = case.join("root");
    std::fs::create_dir(&root).unwrap();
    let source = Arc::new(WindowSource::new(window));
    let mut session = Session::create_with_registration_hook(
        &root,
        nonce.to_le_bytes(),
        source.clone(),
        |ticket| {
            let mut file = File::create(case.join("ticket"))?;
            file.write_all(&ticket.encode())?;
            file.sync_all()?;
            Ok(())
        },
    )
    .unwrap();
    let api = TestApi::load()?;
    let path = root.join("mixed.bin");
    let identity = placeholder_probe(&path);
    assert_eq!(identity.on_disk, 0);
    assert_eq!(identity.modified, 0);
    let hydrate_child = probe_child(&root, "seed-hydrate", "mixed.bin");
    let hydrate_result =
        std::fs::read_to_string(case.join("seed-hydrate-result")).unwrap_or_else(|error| {
            format!("no completed child result: {error}; helper={hydrate_child:?}")
        });
    eprintln!("SEEDED HYDRATE {label}: {hydrate_result}");
    log_native(&session, label, "seed-before-disconnect");
    session.disconnect().unwrap(); // drains all callback work before mode changes
    source.mode.store(DENY, Ordering::SeqCst);
    source.log(label, "seed-after-disconnect-drain");
    hydrate_child?;
    let supplied = source.supplied.load(Ordering::SeqCst);
    let seed = residency(&api, &session, &path)?;
    eprintln!("SEEDED RESIDENCY {label}: {seed:?}");
    require_residency(&seed, false)?;
    let now = placeholder_probe(&path);
    assert_eq!(now.native_id, identity.native_id);
    assert_eq!(now.backing_identity, identity.backing_identity);
    assert_eq!(now.size, identity.size);
    probe_child(&root, "seed-read", "mixed.bin|original")?;
    let original_read = std::fs::read_to_string(case.join("seed-read-original-result")).unwrap();
    if original_read != "SUCCESS" {
        return Err(format!(
            "resident original-page byte proof: {original_read}"
        ));
    }
    assert_eq!(source.supplied.load(Ordering::SeqCst), supplied);
    // No seed callback survives disconnect/drain. A new connection has DENY
    // source policy, and every metadata/data proof handle above has been closed.
    session.connect().unwrap();
    let write_child = probe_child(&root, "seed-write", "mixed.bin");
    let write_result =
        std::fs::read_to_string(case.join("seed-write-result")).unwrap_or_else(|error| {
            format!("no completed child result: {error}; helper={write_child:?}")
        });
    eprintln!("SEEDED WRITE {label}: {write_result}");
    log_native(&session, label, "denied-source-write");
    let dirty = residency(&api, &session, &path);
    eprintln!("SEEDED DIRTY RESIDENCY {label}: {dirty:?}");
    source.log(label, "denied-source-write");
    write_child?;
    assert_eq!(
        source.supplied.load(Ordering::SeqCst),
        supplied,
        "source transferred bytes after denial"
    );
    if write_result != "SUCCESS" {
        return Err(format!("resident-page write refused: {write_result}"));
    }
    let dirty = dirty?;
    require_residency(&dirty, true)?;
    let now = placeholder_probe(&path);
    assert_eq!(now.native_id, identity.native_id);
    assert_eq!(now.backing_identity, identity.backing_identity);
    assert_eq!(now.size, identity.size);
    probe_child(&root, "seed-read", "mixed.bin|dirty")?;
    let read = std::fs::read_to_string(case.join("seed-read-dirty-result")).unwrap();
    if read != "SUCCESS" {
        return Err(format!("resident private-page byte proof: {read}"));
    }
    assert_eq!(source.supplied.load(Ordering::SeqCst), supplied);
    eprintln!(
        "REAL MIXED PROOF {label}: target resident+modified+exact0xA5, witness absent, partial metadata, unchanged identities/size, no source bytes since seed drain"
    );

    // The mixed prerequisite is demonstrated. Export/data-integrity failures
    // are test failures; an unavailable subsequent range proof is still an
    // explicit unqualified outcome and cannot authorize restoration.
    session.disconnect().unwrap();
    std::fs::rename(&path, root.join("relocated-mixed.bin")).unwrap();
    session.connect().unwrap();
    let renamed = RepoPath::parse("relocated-mixed.bin").unwrap();
    let mut export = session.export_checkpoint().unwrap();
    let reads = source.reads.load(Ordering::SeqCst);
    export.revalidate_namespace().unwrap();
    assert_eq!(source.reads.load(Ordering::SeqCst), reads);
    let mut called = false;
    let error = export
        .with_reader(&renamed, |_| {
            called = true;
            Ok(())
        })
        .unwrap_err();
    assert!(!called);
    assert!(
        matches!(export_cause(&error), Error::Windows { operation: "CfHydratePlaceholder(export)", hresult } if *hresult < 0)
    );
    eprintln!("SEEDED EXPORT REFUSAL {label}: {error}");
    log_native(&session, label, "missing-export");
    source.log(label, "missing-export");
    let modified = ranges(
        &api,
        &root.join(renamed.as_str()),
        CF_PLACEHOLDER_RANGE_INFO_MODIFIED,
    )?;
    let on_disk = ranges(
        &api,
        &root.join(renamed.as_str()),
        CF_PLACEHOLDER_RANGE_INFO_ONDISK,
    )?;
    if !covers(&modified, TARGET) || !absent(&on_disk, WITNESS) {
        return Err(format!(
            "post-refusal mixed range proof unavailable: on_disk={on_disk:?} modified={modified:?}"
        ));
    }
    child(&root, "verify-mixed-page", renamed.as_str());
    assert_eq!(source.supplied.load(Ordering::SeqCst), supplied);
    export.revalidate_namespace().unwrap();
    source.mode.store(RESTORE, Ordering::SeqCst); // only after mixed proof + refusal
    let actual = export
        .with_reader(&renamed, |reader| {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes)?;
            Ok(bytes)
        })
        .unwrap();
    let mut expected = source.bytes.clone();
    expected[TARGET.start as usize..TARGET.end as usize].fill(0xa5);
    assert_eq!(actual, expected);
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(case.join("exported-mixed"))
        .unwrap();
    export
        .with_reader(&renamed, |reader| {
            std::io::copy(reader, &mut output)?;
            Ok(())
        })
        .unwrap();
    output.sync_all().unwrap();
    drop(output);
    assert_eq!(
        std::fs::read(case.join("exported-mixed")).unwrap(),
        expected
    );
    export.revalidate_namespace().unwrap();
    assert_rename_capture_refused(&session, &renamed);
    let final_identity = placeholder_probe(&root.join(renamed.as_str()));
    assert_eq!(final_identity.native_id, identity.native_id);
    assert_eq!(final_identity.backing_identity, identity.backing_identity);
    eprintln!(
        "SEEDED MERGED EXPORT {label}: bytes={} FileId={}; source-original ranges + retained private page verified",
        expected.len(),
        FileId::from_bytes(&expected)
    );
    Ok(())
}

fn write_result(root: &Path, name: &str, outcome: &str) {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.parent().unwrap().join(name))
        .unwrap();
    file.write_all(outcome.as_bytes()).unwrap();
    file.sync_all().unwrap();
    eprintln!("{name}: {outcome}");
}

pub(super) fn child_action(root: &Path, action: &str, value: &str) {
    match action {
        "seed-hydrate" => {
            let outcome = (|| -> ProbeResult<String> {
                let api = TestApi::load()?;
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                    .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
                    .open(root.join(value))
                    .map_err(|e| format!("phase=open raw={:?}: {e}", e.raw_os_error()))?;
                let hr = unsafe {
                    (api.hydrate)(
                        file.as_raw_handle(),
                        TARGET.start as i64,
                        (TARGET.end - TARGET.start) as i64,
                        CF_HYDRATE_FLAG_NONE,
                        std::ptr::null_mut(),
                    )
                };
                Ok(format!(
                    "phase=CfHydratePlaceholder offset={} length={} HRESULT=0x{:08X}",
                    TARGET.start,
                    TARGET.end - TARGET.start,
                    hr as u32
                ))
            })()
            .unwrap_or_else(|error| format!("ERROR {error}"));
            write_result(root, "seed-hydrate-result", &outcome);
        }
        "seed-write" => {
            let mut phase = "open";
            let result = (|| -> std::io::Result<()> {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .open(root.join(value))?;
                phase = "seek";
                file.seek(SeekFrom::Start(TARGET.start))?;
                phase = "write";
                file.write_all(&[0xa5; 4096])?;
                phase = "flush";
                file.sync_all()
            })();
            let outcome = match result {
                Ok(()) => "SUCCESS".into(),
                Err(error) => format!(
                    "ERROR phase={phase} raw={:?}: {error}",
                    error.raw_os_error()
                ),
            };
            write_result(root, "seed-write-result", &outcome);
        }
        "seed-read" => {
            let (file, expected) = value.split_once('|').unwrap();
            let mut phase = "open";
            let result = (|| -> std::io::Result<()> {
                let mut file = File::open(root.join(file))?;
                phase = "seek";
                file.seek(SeekFrom::Start(TARGET.start))?;
                let mut bytes = [0u8; 4096];
                phase = "read";
                file.read_exact(&mut bytes)?;
                phase = "compare";
                for (index, actual) in bytes.iter().enumerate() {
                    let wanted = if expected == "original" {
                        original_byte(TARGET.start + index as u64)
                    } else {
                        assert_eq!(expected, "dirty");
                        0xa5
                    };
                    if *actual != wanted {
                        return Err(std::io::Error::other(format!(
                            "page mismatch at {index}: {actual} != {wanted}"
                        )));
                    }
                }
                Ok(())
            })();
            let outcome = match result {
                Ok(()) => "SUCCESS".into(),
                Err(error) => format!(
                    "ERROR phase={phase} raw={:?}: {error}",
                    error.raw_os_error()
                ),
            };
            write_result(root, &format!("seed-read-{expected}-result"), &outcome);
        }
        _ => unreachable!(),
    }
}
