//! Disconnected metadata must not depend on Windows' ambient disguise policy.
use super::{disposable_run, native_open, placeholder_probe, Fixture};
use layrs_core::scm::RepoPath;
use layrs_vfs::{CaptureEvidence, Error, Result, Session};
use std::{
    io::Write,
    marker::PhantomData,
    os::windows::io::AsRawHandle,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{atomic::Ordering, Arc},
};
use windows_sys::Win32::{
    Foundation::{FreeLibrary, HMODULE},
    Storage::FileSystem::*,
    System::LibraryLoader::*,
};

type SetMode = unsafe extern "system" fn(i8) -> i8;
type QueryMode = unsafe extern "system" fn() -> i8;
pub(super) struct Disguise {
    module: HMODULE,
    set: SetMode,
    thread: QueryMode,
    process: QueryMode,
    previous: i8,
    process_before: i8,
    active: bool,
    _same_thread: PhantomData<Rc<()>>,
}
impl Disguise {
    pub(super) fn enter() -> Self {
        let name: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
        let module = unsafe {
            LoadLibraryExW(
                name.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        assert!(!module.is_null());
        let set = unsafe {
            GetProcAddress(
                module,
                c"RtlSetThreadPlaceholderCompatibilityMode".as_ptr().cast(),
            )
        }
        .unwrap();
        let thread = unsafe {
            GetProcAddress(
                module,
                c"RtlQueryThreadPlaceholderCompatibilityMode"
                    .as_ptr()
                    .cast(),
            )
        }
        .unwrap();
        let process = unsafe {
            GetProcAddress(
                module,
                c"RtlQueryProcessPlaceholderCompatibilityMode"
                    .as_ptr()
                    .cast(),
            )
        }
        .unwrap();
        let set =
            unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, SetMode>(set) };
        let thread = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, QueryMode>(thread)
        };
        let process = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, QueryMode>(process)
        };
        let process_before = unsafe { process() };
        let previous = unsafe { set(1) };
        assert!(previous >= 0);
        Self {
            module,
            set,
            thread,
            process,
            previous,
            process_before,
            active: true,
            _same_thread: PhantomData,
        }
    }
    pub(super) fn assert_preserved(&self) {
        assert_eq!(unsafe { (self.thread)() }, 1, "caller thread mode changed");
        assert_eq!(
            unsafe { (self.process)() },
            self.process_before,
            "process mode changed"
        );
    }
    fn restore(&mut self) {
        assert!(unsafe { (self.set)(self.previous) } >= 0);
        self.active = false;
        assert_eq!(unsafe { (self.thread)() }, self.previous);
    }
}
impl Drop for Disguise {
    fn drop(&mut self) {
        if self.active {
            unsafe { (self.set)(self.previous) };
        }
        unsafe { FreeLibrary(self.module) };
    }
}

pub(super) fn raw_tag(path: &Path) -> FILE_ATTRIBUTE_TAG_INFO {
    let file = native_open(path);
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    assert_ne!(
        unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileAttributeTagInfo,
                (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                std::mem::size_of_val(&info) as u32,
            )
        },
        0
    );
    info
}

pub(super) fn run() {
    let (run, nonce) = disposable_run("visibility");
    let root = run.join("root");
    let foreign = run.join("foreign");
    let ordinary = run.join("ordinary");
    for path in [&root, &foreign, &ordinary] {
        std::fs::create_dir(path).unwrap();
    }
    let source = Arc::new(Fixture::new());
    source.deny_reads.store(true, Ordering::SeqCst);
    let persist = |path: PathBuf| {
        move |ticket: &layrs_vfs::Registration| -> Result<()> {
            let mut file = std::fs::File::create(path)?;
            file.write_all(&ticket.encode())?;
            file.sync_all()?;
            Ok(())
        }
    };
    let mut session = Session::create_with_registration_hook(
        &root,
        nonce.to_le_bytes(),
        source.clone(),
        persist(run.join("root.ticket")),
    )
    .unwrap();
    let mut other = Session::create_with_registration_hook(
        &foreign,
        (nonce + 1).to_le_bytes(),
        source.clone(),
        persist(run.join("foreign.ticket")),
    )
    .unwrap();
    session.disconnect().unwrap();
    other.disconnect().unwrap();
    std::fs::write(root.join("ordinary.txt"), b"private ordinary bytes").unwrap();
    let mut disguise = Disguise::enter();
    let cold = RepoPath::parse("untouched.bin").unwrap();
    let native = placeholder_probe(&root.join(cold.as_str()));
    let masked = raw_tag(&root.join(cold.as_str()));
    eprintln!(
        "DISGUISE CONTROL: attrs=0x{:08X} tag=0x{:08X}; native={native:?}",
        masked.FileAttributes, masked.ReparseTag
    );
    assert_eq!(masked.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT, 0);
    assert_eq!(masked.ReparseTag, 0);
    assert_eq!(native.on_disk, 0);
    assert_eq!(native.modified, 0);
    let entry = session.inspect(&cold).unwrap();
    assert!(matches!(entry.evidence, CaptureEvidence::Immutable { .. }));
    let metadata = entry.placeholder.as_ref().unwrap();
    assert!(metadata.partial);
    assert_eq!(metadata.file_id as u64, native.native_id.1);
    assert_eq!(metadata.on_disk_bytes, 0);
    drop(entry);
    disguise.assert_preserved();
    let entry = session
        .inspect(&RepoPath::parse("ordinary.txt").unwrap())
        .unwrap();
    assert!(entry.placeholder.is_none());
    assert!(matches!(
        entry.evidence,
        CaptureEvidence::NeedsHash {
            base: None,
            size: 22
        }
    ));
    drop(entry);
    {
        let mut checkpoint = session.checkpoint().unwrap();
        assert!(checkpoint
            .entries()
            .iter()
            .find(|entry| entry.path == cold)
            .unwrap()
            .placeholder
            .is_some());
        assert!(checkpoint
            .with_reader::<()>(&cold, |_| panic!("clean read callback"))
            .is_err());
        checkpoint.revalidate_namespace().unwrap();
        disguise.assert_preserved();
    }
    {
        let mut export = session.export_checkpoint().unwrap();
        assert!(export
            .entries()
            .iter()
            .find(|entry| entry.path == cold)
            .unwrap()
            .placeholder
            .is_some());
        assert!(export
            .with_reader::<()>(&cold, |_| panic!("clean export callback"))
            .is_err());
        export.revalidate_namespace().unwrap();
        disguise.assert_preserved();
    }
    session.registration().verify_root_identity().unwrap();
    assert!(session.is_owned_root_directory(&root).unwrap());
    assert!(session
        .is_owned_root_directory(&PathBuf::from(root.to_str().unwrap().to_ascii_uppercase()))
        .unwrap());
    assert!(!session
        .is_owned_root_directory(root.ancestors().last().unwrap())
        .unwrap());
    assert!(!session.is_owned_root_directory(&ordinary).unwrap());
    for path in [&foreign, &foreign.join("nested")] {
        assert!(matches!(
            session.is_owned_root_directory(path),
            Err(Error::Unsupported(
                "owned-root probe refuses a foreign or unsupported reparse point"
            ))
        ));
        disguise.assert_preserved();
    }
    assert!(session
        .is_owned_root_directory(&root.join("nested"))
        .is_err());
    assert!(session
        .is_owned_root_directory(&root.join("ordinary.txt"))
        .is_err());
    assert!(
        matches!(session.is_owned_root_directory(&ordinary.join("absent")), Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound)
    );
    for alias in [
        "relative",
        r"C:relative",
        r"\\?\C:\",
        r"\\server\share",
        r"C:\.",
        r"C:\..",
    ] {
        assert!(session.is_owned_root_directory(Path::new(alias)).is_err());
    }
    disguise.assert_preserved();
    assert!(
        matches!(session.inspect(&RepoPath::parse("absent.txt").unwrap()), Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound)
    );
    disguise.assert_preserved();

    // Disguised metadata must preserve the strict capture/export distinction.
    std::fs::rename(root.join(cold.as_str()), root.join("relocated.bin")).unwrap();
    let relocated = RepoPath::parse("relocated.bin").unwrap();
    super::assert_rename_capture_refused(&session, &relocated);
    let export = session.export_checkpoint().unwrap();
    let entry = export
        .entries()
        .iter()
        .find(|entry| entry.path == relocated)
        .unwrap();
    assert!(entry.placeholder.is_some());
    assert!(matches!(entry.evidence, CaptureEvidence::NeedsHash { .. }));
    export.revalidate_namespace().unwrap();
    disguise.assert_preserved();
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);
    assert_eq!(placeholder_probe(&root.join(relocated.as_str())).on_disk, 0);
    assert_eq!(
        placeholder_probe(&root.join(relocated.as_str())).native_id,
        native.native_id
    );
    disguise.restore();
    eprintln!("VISIBILITY/ROOT PROBE: forced DISGUISE; disconnected inspect/checkpoint/export preserve CF identity; ordinary file remains NeedsHash; own root/case alias true; volume/ordinary false; foreign root/ancestor refused; thread/process mode preserved; metadata source_reads0; retained fixtures");
}
