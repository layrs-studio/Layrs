//! Opt-in native qualification. All roots, receipts and registrations retained.
//! Run ignored tests by exact name with LAYRS_VFS_TEST_PARENT; root owns execution.
use super::{
    api::Api,
    control_directory_io as io, fs,
    population::{self, Point},
    stage2_fixture as fixture,
};
use crate::*;
use layrs_core::scm::{FileId, RepoPath};
use std::{
    fs::File,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::Storage::{CloudFilters::*, FileSystem::*};
#[path = "stage2_acquisition.rs"]
mod acquisition;
#[path = "stage2_lifetime.rs"]
mod lifetime;
#[path = "stage2_metadata.rs"]
mod metadata;
#[path = "stage2_negatives.rs"]
mod negatives;

// Count production call sites, not ProviderStatus or guessed cloud state.
std::thread_local! { static NATIVE_CALLS: std::cell::Cell<(usize,usize)> = const { std::cell::Cell::new((0,0)) }; }
pub(super) fn connection_call() {
    NATIVE_CALLS.with(|c| {
        let (a, b) = c.get();
        c.set((a + 1, b));
    });
}
pub(super) fn hydration_call() {
    NATIVE_CALLS.with(|c| {
        let (a, b) = c.get();
        c.set((a, b + 1));
    });
}
fn calls() -> (usize, usize) {
    NATIVE_CALLS.with(|c| c.get())
}
fn data() -> Vec<u8> {
    (0..16384 + 37).map(|index| (index % 251) as u8).collect()
}
fn catalog(empty: bool) -> Arc<Catalog> {
    Arc::new(
        Catalog::new(
            7,
            if empty {
                vec![]
            } else {
                vec![
                    CatalogEntry {
                        path: RepoPath::parse("Dir/data").unwrap(),
                        file_id: FileId::from_bytes(&data()),
                        size: data().len() as u64,
                    },
                    CatalogEntry {
                        path: RepoPath::parse("empty").unwrap(),
                        file_id: FileId::from_bytes(b""),
                        size: 0,
                    },
                ]
            },
        )
        .unwrap(),
    )
}
struct Source {
    catalog: Arc<Catalog>,
    catalogs: AtomicUsize,
    bytes: AtomicU64,
    block: AtomicBool,
    entered: AtomicBool,
    cancelled: AtomicBool,
    // A real lifetime ownership pin remains alive while acquisition is blocked.
    _ownership: Option<File>,
}
impl Source {
    fn new(catalog: Arc<Catalog>) -> Arc<Self> {
        Arc::new(Self {
            catalog,
            catalogs: AtomicUsize::new(0),
            bytes: AtomicU64::new(0),
            block: AtomicBool::new(false),
            entered: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            _ownership: None,
        })
    }
}
impl SnapshotSource for Source {
    fn catalog(&self) -> &Catalog {
        assert_eq!(
            self.catalogs.fetch_add(1, Ordering::SeqCst),
            0,
            "source.catalog called after binding"
        );
        &self.catalog
    }
    fn read_exact_at(
        &self,
        id: &FileId,
        offset: u64,
        buffer: &mut [u8],
        cancel: &Cancellation,
    ) -> Result<()> {
        self.entered.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(15);
        while self.block.load(Ordering::Acquire) {
            if cancel.is_cancelled() {
                self.cancelled.store(true, Ordering::Release);
                return Err(Error::Cancelled);
            }
            if Instant::now() > deadline {
                return Err(Error::Busy);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        cancel.check()?;
        let bytes = data();
        if id != &FileId::from_bytes(&bytes) {
            return Err(Error::IdentityMismatch);
        }
        let range = checked_range(bytes.len() as u64, offset, buffer.len() as u64)?;
        buffer.copy_from_slice(&bytes[range.start as usize..range.end as usize]);
        self.bytes.fetch_add(buffer.len() as u64, Ordering::SeqCst);
        Ok(())
    }
}

fn queries(control: &ControlDirectoryGuard) {
    let api = Api::load().unwrap();
    for (label, held, path, access) in [
        (
            "root",
            control.root.file(),
            control.root().to_path_buf(),
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
        ),
        (
            ".layrs",
            &control.directory,
            control.root().join(".layrs"),
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
        ),
        (
            "FORMAT",
            &control.format,
            control.root().join(".layrs/FORMAT"),
            FILE_READ_ATTRIBUTES,
        ),
    ] {
        let fresh = fs::open(&path, io::ALL_SHARING, access).unwrap();
        assert_eq!(io::id(held).unwrap(), io::id(&fresh).unwrap());
        for (kind, file) in [("held", held), ("fresh", &fresh)] {
            let mut buffer = [0u64; 1024];
            let mut returned = 0;
            let hr = unsafe {
                (api.root_info)(
                    fs::handle(file),
                    CF_SYNC_ROOT_INFO_STANDARD,
                    buffer.as_mut_ptr().cast(),
                    size_of_val(&buffer) as u32,
                    &mut returned,
                )
            };
            let attrs = fs::attributes(file).unwrap();
            eprintln!(
                "GATE {label}/{kind}: HRESULT=0x{:08X}, returned={returned}, id={:?}, attributes=0x{:08X}, tag=0x{:08X}",
                hr as u32,
                io::id(file).unwrap(),
                attrs.FileAttributes,
                attrs.ReparseTag
            );
            if hr >= 0
                && returned as usize
                    >= std::mem::offset_of!(CF_SYNC_ROOT_STANDARD_INFO, SyncRootIdentity)
            {
                let info = unsafe { &*buffer.as_ptr().cast::<CF_SYNC_ROOT_STANDARD_INFO>() };
                eprintln!(
                    "  rootID={:016x}, identityLength={}, hydration={}/{}, population={}/{}, insync={}, hardlink={}, status={} (diagnostic only)",
                    info.SyncRootFileId,
                    info.SyncRootIdentityLength,
                    info.HydrationPolicy.Primary,
                    info.HydrationPolicy.Modifier,
                    info.PopulationPolicy.Primary,
                    info.PopulationPolicy.Modifier,
                    info.InSyncPolicy,
                    info.HardLinkPolicy,
                    info.ProviderStatus
                );
            }
        }
    }
}
fn create(
    f: &fixture::Fixture,
    control: &ControlDirectoryGuard,
    catalog: Arc<Catalog>,
) -> ProviderRootGuard {
    match ProviderRootGuard::create_with_control_directory(control, f.root_id, catalog, |ticket| {
        f.persist(ticket)
    }) {
        Ok(proof) => {
            queries(control);
            proof
        }
        Err(error) => {
            queries(control);
            panic!(
                "Stage2 GO/NO-GO failed with retained fixture {}: {error:?}; no held-query or pin fallback permitted",
                f.base.display()
            );
        }
    }
}

#[test]
#[ignore = "real accepted caller-case alias through Stage1 publication and Stage2 create/reacquire; explicit disposable parent"]
fn stage2_mixed_case_root_registration() {
    let run = fixture::run("mixed-case");
    NATIVE_CALLS.with(|c| c.set((0, 0)));
    let (f, control, requested) = fixture::Fixture::create_case_alias(&run, "caller-case");
    control.revalidate_bootstrap_root().unwrap();
    let (held_path, held, identity) = control.root.held_paths().last().unwrap();
    assert_eq!(held_path, requested);
    assert_ne!(held_path, control.root());
    assert_eq!(io::id(held).unwrap(), identity);
    assert_eq!(identity, control.root.identity());
    assert_eq!(control.root(), control.identity().stage_identity().root());
    eprintln!(
        "CASE Stage1 publication SUCCESS: requested={}, canonical={}, retained_pin={}, native_id={identity:?}",
        requested.display(),
        control.root().display(),
        held_path.display()
    );

    // This asserts the required successful behavior. The old string-based root
    // classification must fail here; no rejection/normalization fallback passes.
    let catalog = catalog(false);
    let proof = create(&f, &control, catalog.clone());
    let ticket = proof.registration().clone();
    let expected = control.identity().clone();
    assert_eq!(ticket.root(), expected.stage_identity().root());
    assert_eq!(
        proof.filesystem_identity(),
        control.root_filesystem_identity()
    );
    proof.revalidate().unwrap();
    drop(proof);
    drop(control);

    // Fresh owner acquired using the same accepted caller spelling, with no
    // prior directory-owner handle. Expected canonical bytes remain unchanged.
    let reopened = ControlDirectoryGuard::open(&requested, &expected).unwrap();
    assert_eq!(reopened.root.held_paths().last().unwrap().0, requested);
    assert_ne!(
        reopened.root.held_paths().last().unwrap().0,
        reopened.root()
    );
    assert_eq!(reopened.root(), ticket.root());
    let reacquired = ticket
        .guard_with_control_directory(catalog, &reopened)
        .unwrap();
    reacquired.revalidate().unwrap();
    assert_eq!(calls(), (0, 0));
    eprintln!(
        "PASS mixed-case caller input: Stage1 accepted/published; Stage2 source-free creation and fresh proof reacquisition retain exact canonical root/native binding; connections=0 hydration=0."
    );
}

#[test]
#[ignore = "real Stage2 held+fresh registration/full-audit READY UPDATE gates; explicit disposable parent"]
fn stage2_native_root_proof() {
    let run = fixture::run("native");
    NATIVE_CALLS.with(|c| c.set((0, 0)));
    for empty in [false, true] {
        let (f, control) = fixture::Fixture::create(
            &run,
            if empty {
                "empty-catalog"
            } else {
                "files-and-empty-file"
            },
        );
        let catalog = catalog(empty);
        let proof = create(&f, &control, catalog.clone());
        assert_eq!(
            calls(),
            (0, 0),
            "source-free factory performed connection/hydration"
        );
        assert_eq!(
            proof.filesystem_identity(),
            control.root_filesystem_identity()
        );
        let ticket = proof.registration().clone();
        let clone = proof.try_clone().unwrap();
        clone.revalidate().unwrap();
        drop(clone);
        let mut legacy = ticket.clone();
        legacy.flavor = crate::registration::RegistrationFlavor::Legacy;
        assert!(matches!(
            legacy.guard_with_control_directory(catalog.clone(), &control),
            Err(Error::Unsupported(_))
        ));
        let source = Source::new(catalog.clone());
        let mut session =
            Session::reconnect_disconnected_with_control_directory(proof, source.clone()).unwrap();
        assert!(!session.is_connected());
        assert_eq!(source.catalogs.load(Ordering::SeqCst), 1);
        assert_eq!(
            session.control.observer_access(),
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES
        );
        let observer = fs::open(
            &f.root.join(".layrs"),
            io::ALL_SHARING,
            session.control.observer_access(),
        )
        .unwrap();
        assert_eq!(
            io::id(&observer).unwrap(),
            io::id(&control.directory).unwrap()
        );
        assert!(session.is_control(".layrs", &observer).unwrap());
        assert!(!session.is_control(".LAYRS", &observer).unwrap());
        drop(observer);
        fixture::denied_delete(&f.root.join(".layrs"));
        let checkpoint = session.checkpoint().unwrap();
        assert!(checkpoint.root_guard().is_some());
        assert_eq!(checkpoint.entries().len(), catalog.entries().len());
        assert_eq!(session.namespace().unwrap().len(), catalog.entries().len());
        for entry in checkpoint.entries() {
            assert!(!entry.path.as_str().starts_with(".layrs"));
            let metadata = entry
                .placeholder
                .as_ref()
                .expect("must be CFAPI placeholder including empty file");
            assert_eq!(metadata.on_disk_bytes, 0);
            assert_eq!(metadata.modified_bytes, 0);
            assert!(metadata.in_sync);
        }
        checkpoint.revalidate_namespace().unwrap();
        drop(checkpoint);
        session
            .export_checkpoint()
            .unwrap()
            .revalidate_namespace()
            .unwrap();
        assert_eq!(source.bytes.load(Ordering::SeqCst), 0);
        assert_eq!(calls(), (0, 0));
        assert!(matches!(
            session.publish_control_file(b"wrong flavor"),
            Err(Error::Unsupported(_))
        ));
        let fake = crate::ControlFileIdentity {
            volume: 0,
            root_index: 1,
            file_index: 2,
            size: 1,
            content: FileId::from_bytes(b"x"),
        };
        assert!(matches!(
            session.attach_control_file(&fake),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            session.publish_control_file_recoverable(
                b"x",
                &f.base,
                None,
                |_| panic!("V2 called legacy persist"),
                &NoControlPublishFaults
            ),
            Err(Error::Unsupported(_))
        ));
        assert_eq!(calls(), (0, 0));
        assert_eq!(source.bytes.load(Ordering::SeqCst), 0);
        if !empty {
            session.connect().unwrap();
            assert_eq!(calls().0, 1);
            owned_child(&f.base, "read", "", 0);
            assert!(
                source.bytes.load(Ordering::SeqCst) > 0,
                "positive native read did not reach source"
            );
            let path = RepoPath::parse("Dir/data").unwrap();
            let mut export = session.export_checkpoint().unwrap();
            let bytes = export
                .with_reader(&path, |reader| {
                    let mut out = Vec::new();
                    reader.read_to_end(&mut out)?;
                    Ok(out)
                })
                .unwrap();
            assert_eq!(bytes, data());
            export.revalidate_namespace().unwrap();
            drop(export);
            assert!(
                calls().1 > 0,
                "positive export did not exercise explicit hydration counter"
            );
            assert_eq!(source.catalogs.load(Ordering::SeqCst), 1);
            session.disconnect().unwrap();
            // Complete proof validates initial READY; later ordinary edits/deletions
            // must survive reacquisition without a namespace equality requirement.
            std::fs::remove_file(f.root.join("Dir/data")).unwrap();
            std::fs::write(f.root.join("private-new"), b"private bytes").unwrap();
            session.root_guard().unwrap().revalidate().unwrap();
        }
        drop(session);
        drop(control);
        // No old directory-owner handle survives this expected-open.
        let (_f, reopened) = fixture::Fixture::reopen(f.base.clone());
        let proof = ticket
            .guard_with_control_directory(catalog, &reopened)
            .unwrap();
        proof.revalidate().unwrap();
        if !empty {
            assert!(!f.root.join("Dir/data").exists());
            assert_eq!(
                std::fs::read(f.root.join("private-new")).unwrap(),
                b"private bytes"
            );
        }
        NATIVE_CALLS.with(|c| c.set((0, 0)));
    }
    eprintln!(
        "PASS Stage2 source-free complete proof; strict held+fresh query gate; retained audit READY update; empty file/catalog; exact control exclusion; frozen source catalog; counters0 before explicit connect and positive native read after; fresh layout/proof reopening preserves edits."
    );
}

#[test]
fn stage2_namespace_budget_counts_user_entries_only() {
    let mut budget = crate::catalog::NamespaceBudget::default();
    for _ in 0..crate::catalog::MAX_ENTRIES {
        budget.user_entry().unwrap();
    }
    budget.verified_control_entry().unwrap();
    budget.finish(true).unwrap();
    assert!(budget.user_entry().is_err());
    assert!(budget.verified_control_entry().is_err());
    assert!(
        crate::catalog::NamespaceBudget::default()
            .finish(true)
            .is_err()
    );
    assert_eq!(
        super::attached::AttachedControl::None.observer_access(),
        FILE_READ_ATTRIBUTES
    );
    // FORMAT is internal to the already verified closed control layout. It is
    // not visited by either walker and spends no user or additional control slot.
}

#[test]
fn stage2_bounded_acquisition_cancels_with_ownership_and_queued_writer_retained() {
    acquisition::unit_gate();
}

const CHILD: &str = "windows::stage2_tests::stage2_owned_child";
const EXIT_CUT: i32 = 117;
struct OwnedChild(Option<Child>);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
fn spawn_child(base: &Path, mode: &str, point: &str) -> OwnedChild {
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", CHILD, "--nocapture"])
        .env("LAYRS_VFS_STAGE2_BASE", base)
        .env("LAYRS_VFS_STAGE2_MODE", mode)
        .env("LAYRS_VFS_STAGE2_POINT", point)
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    OwnedChild(Some(child))
}
fn finish_child(mut child: OwnedChild, expected: i32) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.0.as_mut().unwrap().try_wait().unwrap() {
            assert_eq!(status.code(), Some(expected));
            child.0.take();
            return;
        }
        assert!(
            Instant::now() < deadline,
            "owned Stage2 child timed out (will be killed/reaped)"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn owned_child(base: &Path, mode: &str, point: &str, expected: i32) {
    finish_child(spawn_child(base, mode, point), expected);
}
fn point_name(point: Point) -> &'static str {
    match point {
        Point::BeforeRegistration => "before-os",
        Point::DuringPopulation => "mid-population",
        Point::BeforeAudit => "before-audit",
        Point::AfterAudit => "after-audit",
        Point::AfterUpdate => "after-update",
        Point::AfterReady => "after-ready",
    }
}

#[test]
#[ignore = "real five process cuts and new-process expected recovery; explicit disposable parent"]
fn stage2_process_recovery() {
    let run = fixture::run("process");
    for point in [
        "before-os",
        "mid-population",
        "after-audit",
        "after-update",
        "after-ready",
    ] {
        let (f, control) = fixture::Fixture::create(&run, point);
        drop(control);
        owned_child(&f.base, "cut", point, EXIT_CUT);
        let before = f.ticket().encode();
        owned_child(&f.base, "resume", point, 0);
        assert_eq!(before, f.ticket().encode());
        eprintln!(
            "PASS process boundary {point}: receipt unchanged, explicit expected outcome, no cleanup/repopulation"
        );
    }
}

#[test]
#[ignore = "real in-flight callback, injected disconnect failure and native pin retention until process exit"]
fn stage2_failed_disconnect_retains_actual_proof() {
    let run = fixture::run("disconnect");
    let (f, control) = fixture::Fixture::create(&run, "leaked-context");
    drop(control);
    let child = spawn_child(&f.base, "leak", "");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !f.base.join("leak-pins-held").exists() {
        assert!(
            Instant::now() < deadline,
            "leak child did not reach proof-retention boundary"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    fixture::denied_delete(&f.root);
    eprintln!(
        "root DELETE32 observed with live native connection; not isolated evidence of leaked root pins"
    );
    fixture::denied_delete(&f.root.join(".layrs"));
    fixture::denied_delete(&f.root.join(".layrs/FORMAT"));
    fixture::raw32(
        fs::open(
            &f.root.join(".layrs/FORMAT"),
            io::ALL_SHARING,
            FILE_WRITE_DATA,
        )
        .unwrap_err(),
    );
    fixture::raw32(
        fs::open(
            &f.base.join("source-ownership"),
            io::ALL_SHARING,
            FILE_WRITE_DATA,
        )
        .unwrap_err(),
    );
    fixture::write_new(
        &f.base.join("release-child"),
        b"parent checked retained sharing",
    );
    finish_child(child, 0);
    // Old live owner and leaked callback handles have all died with the child.
    let (_f, control) = fixture::Fixture::reopen(f.base.clone());
    f.ticket()
        .guard_with_control_directory(catalog(false), &control)
        .unwrap()
        .revalidate()
        .unwrap();
    fs::open(
        &f.base.join("source-ownership"),
        io::ALL_SHARING,
        FILE_WRITE_DATA,
    )
    .unwrap();
    eprintln!(
        "PASS real acquisition attempted/WouldBlock, bounded cancellation latch proves callback active across SessionDrop; INJECTED disconnect-return failure retains .layrs/FORMAT/source ownership32 until child exit; root32 observed but live connection is a confounder; fresh proof reacquired afterward."
    );
}

unsafe extern "system" fn injected_disconnect_failure(_: CF_CONNECTION_KEY) -> i32 {
    0x80004005u32 as i32
}

#[test]
#[ignore = "internal bounded child; invoked only by Stage2 parent recipes"]
fn stage2_owned_child() {
    let base =
        PathBuf::from(std::env::var_os("LAYRS_VFS_STAGE2_BASE").expect("owned fixture required"));
    let mode = std::env::var("LAYRS_VFS_STAGE2_MODE").unwrap();
    let point = std::env::var("LAYRS_VFS_STAGE2_POINT").unwrap();
    if mode == "read" || mode == "read-cancel" {
        let ticket = Registration::decode_control_directory(
            &std::fs::read(base.join("ticket.bin")).unwrap(),
        )
        .unwrap();
        let result = std::fs::read(ticket.root().join("Dir/data"));
        if mode == "read" {
            assert_eq!(result.unwrap(), data());
        } else {
            assert!(
                result.is_err(),
                "cancelled source unexpectedly satisfied native read"
            );
            eprintln!(
                "cancelled child native read result={:?}",
                result.unwrap_err()
            );
        }
        return;
    }
    let (f, control) = fixture::Fixture::reopen(base);
    let catalog = catalog(false);
    match mode.as_str() {
        "leak" => {
            let proof = create(&f, &control, catalog.clone());
            drop(control);
            fixture::write_new(&f.base.join("source-ownership"), b"lifetime source lease");
            let ownership = fs::open(
                &f.base.join("source-ownership"),
                FILE_SHARE_READ,
                FILE_READ_DATA | FILE_READ_ATTRIBUTES,
            )
            .unwrap();
            let source = acquisition::BlockedSource::new(catalog, Some(ownership));
            let witness = source.witness.clone();
            witness.hold_cancel.store(true, Ordering::Release);
            let weak = Arc::downgrade(&source);
            let mut session =
                Session::reconnect_disconnected_with_control_directory(proof, source.clone())
                    .unwrap();
            session.connect().unwrap();
            let reader = spawn_child(&f.base, "read-cancel", "");
            acquisition::assert_blocked(&witness);
            assert!(witness.active.load(Ordering::Acquire) > 0);
            let mut injected = Api::load().unwrap();
            injected.disconnect = injected_disconnect_failure;
            session.api = Arc::new(injected);
            assert!(
                matches!(session.disconnect(),Err(Error::Windows{hresult,..}) if hresult==0x80004005u32 as i32)
            );
            let deadline = Instant::now() + Duration::from_secs(3);
            while !witness.cancelled.load(Ordering::Acquire) {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
            assert!(witness.active.load(Ordering::Acquire) > 0);
            assert_eq!(witness.completed.load(Ordering::Acquire), 0);
            drop(session); // A second injected refusal exercises actual Drop leak.
            drop(source);
            assert!(
                weak.upgrade().is_some(),
                "uncertain teardown dropped the source"
            );
            assert!(
                witness.active.load(Ordering::Acquire) > 0,
                "callback ended before SessionDrop witness"
            );
            assert_eq!(witness.completed.load(Ordering::Acquire), 0);
            assert!(witness.owner_live.load(Ordering::Acquire));
            assert!(!witness.writer_acquired.load(Ordering::Acquire));
            fixture::denied_delete(&f.root.join(".layrs"));
            fixture::denied_delete(&f.root.join(".layrs/FORMAT"));
            fixture::raw32(
                fs::open(
                    &f.root.join(".layrs/FORMAT"),
                    io::ALL_SHARING,
                    FILE_WRITE_DATA,
                )
                .unwrap_err(),
            );
            fixture::raw32(
                fs::open(
                    &f.base.join("source-ownership"),
                    io::ALL_SHARING,
                    FILE_WRITE_DATA,
                )
                .unwrap_err(),
            );
            // Retest active after the native probes: no timing assumption stands
            // in for the bounded cancellation latch.
            assert!(witness.active.load(Ordering::Acquire) > 0);
            assert_eq!(witness.completed.load(Ordering::Acquire), 0);
            eprintln!(
                "INJECTED disconnect HRESULT80004005; real callback still active AFTER SessionDrop: active={}, actual try_read attempts={}, WouldBlock={}, lifetime owner retained=true",
                witness.active.load(Ordering::Acquire),
                witness.attempts.load(Ordering::Acquire),
                witness.would_block.load(Ordering::Acquire)
            );
            witness.allow_cancel_return.store(true, Ordering::Release);
            let deadline = Instant::now() + Duration::from_secs(6);
            while witness.active.load(Ordering::Acquire) != 0 {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
            assert!(witness.completed.load(Ordering::Acquire) > 0);
            assert!(witness.owner_live.load(Ordering::Acquire));
            assert!(!witness.writer_acquired.load(Ordering::Acquire));
            finish_child(reader, 0);
            fixture::write_new(
                &f.base.join("leak-pins-held"),
                b"injected HRESULT80004005; active-after-SessionDrop witnessed; actual control/FORMAT/source ownership32",
            );
            let deadline = Instant::now() + Duration::from_secs(15);
            while !f.base.join("release-child").exists() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        "cut" => {
            population::test_hooks::scoped(
                move |at| {
                    if point_name(at) == point {
                        eprintln!("CUT {} after durable ticket: exit117", point_name(at));
                        std::process::exit(EXIT_CUT);
                    }
                    Ok(())
                },
                || {
                    create(&f, &control, catalog);
                },
            );
            panic!("configured cut not reached");
        }
        "resume" => {
            let ticket = f.ticket();
            match point.as_str() {
                "before-os" => {
                    control.revalidate_bootstrap_root().unwrap();
                    fixture::not_under(control.root.file());
                    // Explicit externally checked idempotent hook, never an automatic retry.
                    let saved = ticket.encode();
                    let proof = ProviderRootGuard::create_with_control_directory(
                        &control,
                        f.root_id,
                        catalog,
                        |candidate| {
                            assert_eq!(candidate.encode(), saved);
                            Ok(())
                        },
                    )
                    .unwrap();
                    proof.revalidate().unwrap();
                }
                "mid-population" | "after-audit" => {
                    assert!(matches!(
                        ticket.guard_with_control_directory(catalog.clone(), &control),
                        Err(Error::NeedsRepair(_))
                    ));
                    let before: Vec<_> = std::fs::read_dir(&f.root)
                        .unwrap()
                        .map(|e| e.unwrap().file_name())
                        .collect();
                    assert!(
                        ProviderRootGuard::create_with_control_directory(
                            &control,
                            f.root_id,
                            catalog,
                            |_| panic!("registered incomplete retry reached persist")
                        )
                        .is_err()
                    );
                    let after: Vec<_> = std::fs::read_dir(&f.root)
                        .unwrap()
                        .map(|e| e.unwrap().file_name())
                        .collect();
                    assert_eq!(before, after);
                }
                "after-ready" | "after-update" => {
                    let proof = ticket
                        .guard_with_control_directory(catalog.clone(), &control)
                        .unwrap();
                    drop(proof);
                    std::fs::remove_file(f.root.join("Dir/data")).unwrap();
                    ticket
                        .guard_with_control_directory(catalog, &control)
                        .unwrap()
                        .revalidate()
                        .unwrap();
                    assert!(!f.root.join("Dir/data").exists());
                }
                _ => panic!("unknown boundary"),
            }
            assert_eq!(calls(), (0, 0));
        }
        _ => panic!("unknown child mode"),
    }
}
