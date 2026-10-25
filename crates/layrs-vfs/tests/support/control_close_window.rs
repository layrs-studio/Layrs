//! Separate TEST-ONLY close-window feasibility recipe. It does not relax any
//! production guard or change either previous deliberately failing recipe.
//! The sole FORMAT handle is closed only after a synced external exact intent;
//! directory/root/ancestor pins live through the entire attempt and result.
//! Fault points are synchronous test hooks, not process-crash qualification.
use super::{disposable_run, native_info};
use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    mem::{offset_of, size_of, size_of_val},
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::{Component, Path, PathBuf, Prefix},
};
use windows_sys::Win32::{
    Foundation::ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT,
    Storage::{CloudFilters::*, FileSystem::*},
};

#[path = "../../src/windows/api.rs"]
mod api;

// Canonical bytes for this test fixture only, not a product FORMAT document.
const BYTES: &[u8] = b"{\"kind\":\"layrs.close-window-fixture.v1\",\"marker\":\"original\"}\n";
const EXTRA: &[u8] = b"unexpected child: preserve these bytes\n";
type NativeId = (u32, u64);

fn open(path: &Path, access: u32, sharing: u32) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(access)
        .share_mode(sharing)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

fn native_id(file: &File) -> NativeId {
    let info = native_info(file);
    (
        info.dwVolumeSerialNumber,
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    )
}

fn size(file: &File) -> u64 {
    let info = native_info(file);
    (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow)
}

fn plain(file: &File, directory: bool) -> io::Result<()> {
    let mut attrs = FILE_ATTRIBUTE_TAG_INFO::default();
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileAttributeTagInfo,
            (&mut attrs as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of_val(&attrs) as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if attrs.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || attrs.ReparseTag != 0
        || (attrs.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ordinary no-follow object required",
        ));
    }
    Ok(())
}

fn not_under_sync_root(api: &api::Api, file: &File) {
    let mut info = CF_SYNC_ROOT_BASIC_INFO::default();
    let hr = unsafe {
        (api.root_info)(
            file.as_raw_handle(),
            CF_SYNC_ROOT_INFO_BASIC,
            (&mut info as *mut CF_SYNC_ROOT_BASIC_INFO).cast(),
            size_of_val(&info) as u32,
            std::ptr::null_mut(),
        )
    };
    assert_eq!(
        hr,
        (0x80070000u32 | ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT) as i32,
        "only exact not-under-sync-root is accepted, HRESULT=0x{:08x}",
        hr as u32,
    );
}

fn pins(api: &api::Api, path: &Path) -> Vec<File> {
    let mut components = path.components();
    assert!(
        matches!(components.next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_)))
    );
    assert!(matches!(components.next(), Some(Component::RootDir)));
    assert!(components.all(|part| matches!(part, Component::Normal(_))));
    let spelling = path.to_str().unwrap().replace('/', "\\");
    assert!(!spelling[3..].split('\\').any(|part| {
        matches!(part, "." | "..") || part.ends_with(['.', ' ']) || part.contains(':')
    }));
    let mut held = Vec::new();
    for ancestor in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let file = open(
            ancestor,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        )
        .unwrap();
        plain(&file, true).unwrap();
        // This also rejects a compatibility-disguised cloud ancestor before
        // opening its child. The recipe never registers/connects any sync root.
        not_under_sync_root(api, &file);
        held.push(file);
    }
    held
}

fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn read_exact_bytes(mut file: &File, expected: &[u8]) -> io::Result<bool> {
    if size(file) != expected.len() as u64 {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = vec![0u8; expected.len()];
    file.read_exact(&mut bytes)?;
    Ok(bytes == expected && file.read(&mut [0u8; 1])? == 0)
}

fn exact_layout(path: &Path, name: &str) -> io::Result<bool> {
    let mut entries = std::fs::read_dir(path)?;
    let Some(first) = entries.next().transpose()? else {
        return Ok(false);
    };
    Ok(first.file_name() == OsString::from(name) && entries.next().transpose()?.is_none())
}

fn rename_no_replace(file: &File, destination: &Path) -> io::Result<()> {
    let path = destination.to_str().unwrap().replace('/', "\\");
    assert!(!path.starts_with("\\\\"));
    let name: Vec<u16> = format!("\\\\?\\{path}")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let length = (offset_of!(FILE_RENAME_INFO, FileName) + name.len() * 2)
        .max(size_of::<FILE_RENAME_INFO>());
    let mut storage = vec![0u64; length.div_ceil(8)];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = std::ptr::null_mut();
        (*info).FileNameLength = ((name.len() - 1) * 2) as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            storage
                .as_mut_ptr()
                .cast::<u8>()
                .add(offset_of!(FILE_RENAME_INFO, FileName))
                .cast::<u16>(),
            name.len(),
        );
        if SetFileInformationByHandle(
            file.as_raw_handle(),
            FileRenameInfo,
            info.cast(),
            length as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Point {
    PreClose,
    PostClose,
    PreRename,
    PostRename,
    PreStrongPin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    RootIdentity,
    DirectoryIdentity,
    FormatIdentity,
    Size,
    Links,
    BytesOrEof,
    Layout,
}

#[derive(Debug)]
enum Failure {
    Native {
        point: Point,
        operation: &'static str,
        error: io::Error,
    },
    Proof {
        point: Point,
        reason: Reason,
    },
}
fn native<T>(point: Point, operation: &'static str, result: io::Result<T>) -> Result<T, Failure> {
    result.map_err(|error| Failure::Native {
        point,
        operation,
        error,
    })
}
fn require(point: Point, condition: bool, reason: Reason) -> Result<(), Failure> {
    if condition {
        Ok(())
    } else {
        Err(Failure::Proof { point, reason })
    }
}

#[derive(Debug, Clone, Copy)]
struct Expected {
    root: NativeId,
    directory: NativeId,
    format: NativeId,
    length: u64,
    links: u32,
}

struct Fixture {
    base: PathBuf,
    root: PathBuf,
    stage: PathBuf,
    _parent_pins: Vec<File>,
    root_pins: Vec<File>,
    directory: File,
    expected: Expected,
}
impl Fixture {
    fn new(run: &Path, api: &api::Api, name: &str) -> (Self, File) {
        let base = run.join(name);
        std::fs::create_dir(&base).unwrap();
        let parents = pins(api, &base);
        let root = base.join("root");
        let stage = base.join("unique-stage");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&stage).unwrap();
        let root_pins = pins(api, &root);
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
        write_new(&stage.join("FORMAT"), BYTES).unwrap();
        let directory = open(
            &stage,
            DELETE | FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        )
        .unwrap();
        plain(&directory, true).unwrap();
        let format = open(
            &stage.join("FORMAT"),
            FILE_READ_DATA | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ,
        )
        .unwrap();
        let expected = Expected {
            root: native_id(root_pins.last().unwrap()),
            directory: native_id(&directory),
            format: native_id(&format),
            length: BYTES.len() as u64,
            links: 1,
        };
        (
            Self {
                base,
                root,
                stage,
                _parent_pins: parents,
                root_pins,
                directory,
                expected,
            },
            format,
        )
    }

    fn format_path(&self, published: bool) -> PathBuf {
        if published {
            self.root.join(".layrs/FORMAT")
        } else {
            self.stage.join("FORMAT")
        }
    }

    fn intent(&self) -> Vec<u8> {
        // Closed fixed-order test encoding. Actual canonical bytes are an exact
        // suffix, not just a digest or timestamps. This is no product journal.
        let mut bytes = format!(
            "LAYRS-CLOSE-WINDOW-TEST-INTENT-v1\nroot={}\nstage={}\nroot_id={:?}\ndirectory_id={:?}\nformat_id={:?}\nlength={}\nlinks={}\nfile_id={}\nBYTES\n",
            self.root.display(), self.stage.display(), self.expected.root,
            self.expected.directory, self.expected.format, self.expected.length,
            self.expected.links, layrs_core::scm::FileId::from_bytes(BYTES),
        ).into_bytes();
        bytes.extend_from_slice(BYTES);
        bytes
    }

    fn persist(&self) -> io::Result<()> {
        write_new(&self.base.join("original-bytes.witness"), BYTES)?;
        write_new(&self.base.join("exact-intent.receipt"), &self.intent())
    }

    fn preserved_intent(&self) {
        for (path, bytes) in [
            (self.base.join("original-bytes.witness"), BYTES.to_vec()),
            (self.base.join("exact-intent.receipt"), self.intent()),
        ] {
            let file = open(
                &path,
                FILE_READ_DATA | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ,
            )
            .unwrap();
            assert!(
                read_exact_bytes(&file, &bytes).unwrap(),
                "intent/witness changed: {}",
                path.display()
            );
        }
    }

    fn validate_root(&self, point: Point) -> Result<(), Failure> {
        let held = self.root_pins.last().unwrap();
        native(point, "root attributes", plain(held, true))?;
        let observed = native(
            point,
            "reopen pinned root",
            open(
                &self.root,
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
            ),
        )?;
        native(point, "reopened root attributes", plain(&observed, true))?;
        require(
            point,
            native_id(held) == self.expected.root && native_id(&observed) == self.expected.root,
            Reason::RootIdentity,
        )
    }

    fn validate_directory(&self, point: Point, published: bool) -> Result<(), Failure> {
        self.validate_root(point)?;
        native(
            point,
            "held directory attributes",
            plain(&self.directory, true),
        )?;
        let path = if published {
            self.root.join(".layrs")
        } else {
            self.stage.clone()
        };
        // The retained owner requests DELETE. A temporary proof observer must
        // share DELETE; it never replaces the effective no-delete owner.
        let observed = native(
            point,
            "reopen held control directory",
            open(
                &path,
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ),
        )?;
        native(
            point,
            "reopened directory attributes",
            plain(&observed, true),
        )?;
        require(
            point,
            native_id(&self.directory) == self.expected.directory
                && native_id(&observed) == self.expected.directory,
            Reason::DirectoryIdentity,
        )
    }

    fn validate_format(&self, point: Point, file: &File) -> Result<(), Failure> {
        native(point, "FORMAT attributes", plain(file, false))?;
        require(
            point,
            native_id(file) == self.expected.format,
            Reason::FormatIdentity,
        )?;
        require(point, size(file) == self.expected.length, Reason::Size)?;
        require(
            point,
            native_info(file).nNumberOfLinks == self.expected.links && self.expected.links == 1,
            Reason::Links,
        )?;
        let exact = native(
            point,
            "FORMAT bounded full bytes and EOF",
            read_exact_bytes(file, BYTES),
        )?;
        require(point, exact, Reason::BytesOrEof)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arm {
    Clean,
    SameBytesReplacement,
    ChangedBytes,
    ChangedLength,
    ExtraChild,
    MissingFile,
}
impl Arm {
    fn name(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::SameBytesReplacement => "same-bytes-replacement",
            Self::ChangedBytes => "same-object-changed-bytes",
            Self::ChangedLength => "same-object-changed-length",
            Self::ExtraChild => "extra-child",
            Self::MissingFile => "missing-file",
        }
    }
}

fn changed_bytes() -> Vec<u8> {
    let mut bytes = BYTES.to_vec();
    let offset = bytes
        .windows(b"original".len())
        .position(|value| value == b"original")
        .unwrap();
    bytes[offset] = b'X';
    bytes
}

struct Faults {
    arm: Arm,
    trace: Vec<Point>,
}
impl Faults {
    fn point(&mut self, point: Point, fixture: &Fixture) -> io::Result<()> {
        self.trace.push(point);
        match (self.arm, point) {
            (Arm::ChangedBytes, Point::PostClose) => {
                let mut file = open(
                    &fixture.format_path(false),
                    FILE_GENERIC_WRITE | FILE_READ_ATTRIBUTES,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                )?;
                assert_eq!(native_id(&file), fixture.expected.format);
                file.seek(SeekFrom::Start(0))?;
                file.write_all(&changed_bytes())?;
                file.sync_all()?;
                assert_eq!(native_id(&file), fixture.expected.format);
                assert_eq!(size(&file), fixture.expected.length);
            }
            (Arm::ChangedLength, Point::PreRename) => {
                let mut file = open(
                    &fixture.format_path(false),
                    FILE_GENERIC_WRITE | FILE_READ_ATTRIBUTES,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                )?;
                assert_eq!(native_id(&file), fixture.expected.format);
                file.seek(SeekFrom::End(0))?;
                file.write_all(b"!")?;
                file.sync_all()?;
                assert_eq!(native_id(&file), fixture.expected.format);
                assert_eq!(size(&file), fixture.expected.length + 1);
            }
            (Arm::ExtraChild, Point::PostRename) => {
                write_new(&fixture.root.join(".layrs/EXTRA"), EXTRA)?;
            }
            (Arm::SameBytesReplacement | Arm::MissingFile, Point::PreStrongPin) => {
                let format = fixture.format_path(true);
                let moving = open(
                    &format,
                    DELETE | FILE_READ_ATTRIBUTES,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                )?;
                assert_eq!(native_id(&moving), fixture.expected.format);
                rename_no_replace(&moving, &fixture.base.join("retained-original-FORMAT"))?;
                drop(moving);
                if self.arm == Arm::SameBytesReplacement {
                    write_new(&format, BYTES)?;
                    let replacement = open(
                        &format,
                        FILE_READ_DATA | FILE_READ_ATTRIBUTES,
                        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    )?;
                    assert_ne!(native_id(&replacement), fixture.expected.format);
                    assert!(read_exact_bytes(&replacement, BYTES)?);
                }
            }
            _ => {}
        }
        // All temporary mutation handles close before returning to publication.
        Ok(())
    }
}

// This test-only proof borrows every retained root/directory pin. It is not a
// provider capability, never serializes, and grants no registration/readiness.
struct FinalPin<'a> {
    fixture: &'a Fixture,
    format: File,
}

fn strong_protection(path: &Path, phase: &str) {
    for access in [FILE_WRITE_DATA, FILE_APPEND_DATA, DELETE] {
        let error = open(
            path,
            access | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        )
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(32), "{phase}: access={access:x}");
    }
}

fn publish<'a>(
    fixture: &'a Fixture,
    format: File,
    faults: &mut Faults,
) -> Result<FinalPin<'a>, Failure> {
    fixture.validate_directory(Point::PreClose, false)?;
    fixture.validate_format(Point::PreClose, &format)?;
    require(
        Point::PreClose,
        native(
            Point::PreClose,
            "stage exact layout",
            exact_layout(&fixture.stage, "FORMAT"),
        )?,
        Reason::Layout,
    )?;
    strong_protection(&fixture.format_path(false), "before-intent");
    native(
        Point::PreClose,
        "persist exact external intent",
        fixture.persist(),
    )?;
    fixture.preserved_intent();
    // Reread the actual pinned bytes after persistence, without using the
    // receipt's mere presence as proof of the current native object.
    fixture.validate_format(Point::PreClose, &format)?;
    strong_protection(&fixture.format_path(false), "after-intent-before-close");
    fixture.validate_directory(Point::PreClose, false)?;
    require(
        Point::PreClose,
        native(
            Point::PreClose,
            "stage layout after intent",
            exact_layout(&fixture.stage, "FORMAT"),
        )?,
        Reason::Layout,
    )?;
    let empty = native(
        Point::PreClose,
        "last pinned empty-root check",
        std::fs::read_dir(&fixture.root),
    )?
    .next()
    .transpose()
    .map_err(|error| Failure::Native {
        point: Point::PreClose,
        operation: "last pinned root entry",
        error,
    })?
    .is_none();
    require(Point::PreClose, empty, Reason::Layout)?;
    native(
        Point::PreClose,
        "PreClose hook",
        faults.point(Point::PreClose, fixture),
    )?;
    drop(format); // ONLY the FORMAT pin closes. All directory pins remain held.
    native(
        Point::PostClose,
        "PostClose hook",
        faults.point(Point::PostClose, fixture),
    )?;
    native(
        Point::PreRename,
        "PreRename hook",
        faults.point(Point::PreRename, fixture),
    )?;
    native(
        Point::PreRename,
        "same-handle no-replace directory rename",
        rename_no_replace(&fixture.directory, &fixture.root.join(".layrs")),
    )?;
    native(
        Point::PostRename,
        "PostRename hook",
        faults.point(Point::PostRename, fixture),
    )?;
    native(
        Point::PreStrongPin,
        "PreStrongPin hook",
        faults.point(Point::PreStrongPin, fixture),
    )?;
    // In the clean arm the two hooks above only append enum values to memory.
    // No extra disk log/read or fallback reopen precedes this strong open.
    let strong = native(
        Point::PreStrongPin,
        "open final strong FORMAT",
        open(
            &fixture.format_path(true),
            FILE_READ_DATA | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ,
        ),
    )?;
    fixture.validate_directory(Point::PreStrongPin, true)?;
    fixture.validate_format(Point::PreStrongPin, &strong)?;
    require(
        Point::PreStrongPin,
        native(
            Point::PreStrongPin,
            "final root layout",
            exact_layout(&fixture.root, ".layrs"),
        )?,
        Reason::Layout,
    )?;
    require(
        Point::PreStrongPin,
        native(
            Point::PreStrongPin,
            "final control layout",
            exact_layout(&fixture.root.join(".layrs"), "FORMAT"),
        )?,
        Reason::Layout,
    )?;
    Ok(FinalPin {
        fixture,
        format: strong,
    })
}

fn witness(file: &Path, expected: &[u8], expected_id: Option<NativeId>) {
    let file = open(file, FILE_READ_DATA | FILE_READ_ATTRIBUTES, FILE_SHARE_READ).unwrap();
    plain(&file, false).unwrap();
    if let Some(expected_id) = expected_id {
        assert_eq!(native_id(&file), expected_id);
    }
    assert_eq!(native_info(&file).nNumberOfLinks, 1);
    assert!(read_exact_bytes(&file, expected).unwrap());
}

fn preserved_rejection(fixture: &Fixture, arm: Arm, published: bool) {
    fixture.preserved_intent();
    fixture
        .validate_directory(Point::PreStrongPin, published)
        .unwrap();
    let format = fixture.format_path(published);
    match arm {
        Arm::SameBytesReplacement | Arm::MissingFile if published => {
            witness(
                &fixture.base.join("retained-original-FORMAT"),
                BYTES,
                Some(fixture.expected.format),
            );
            if arm == Arm::SameBytesReplacement {
                let file = open(
                    &format,
                    FILE_READ_DATA | FILE_READ_ATTRIBUTES,
                    FILE_SHARE_READ,
                )
                .unwrap();
                assert_ne!(native_id(&file), fixture.expected.format);
                assert!(read_exact_bytes(&file, BYTES).unwrap());
            } else {
                let error = open(
                    &format,
                    FILE_READ_DATA | FILE_READ_ATTRIBUTES,
                    FILE_SHARE_READ,
                )
                .unwrap_err();
                assert_eq!(error.raw_os_error(), Some(2));
            }
        }
        Arm::ChangedBytes => witness(&format, &changed_bytes(), Some(fixture.expected.format)),
        Arm::ChangedLength => {
            let mut bytes = BYTES.to_vec();
            bytes.push(b'!');
            witness(&format, &bytes, Some(fixture.expected.format));
        }
        Arm::ExtraChild if published => {
            witness(&format, BYTES, Some(fixture.expected.format));
            witness(&fixture.root.join(".layrs/EXTRA"), EXTRA, None);
        }
        _ => witness(&format, BYTES, Some(fixture.expected.format)),
    }
}

fn arm(run: &Path, api: &api::Api, scenario: Arm) -> bool {
    let (fixture, format) = Fixture::new(run, api, scenario.name());
    let mut faults = Faults {
        arm: scenario,
        trace: Vec::with_capacity(5),
    };
    let result = publish(&fixture, format, &mut faults);
    // Event traces are diagnostic, written AFTER the attempt. This does not
    // claim a process would have durably recorded its last reached fault point.
    write_new(
        &fixture.base.join("returned-fault-trace.txt"),
        format!("{:?}\n", faults.trace).as_bytes(),
    )
    .unwrap();
    match result {
        Ok(final_pin) => {
            assert_eq!(
                scenario,
                Arm::Clean,
                "mutated window incorrectly issued a test final proof"
            );
            final_pin
                .fixture
                .validate_directory(Point::PreStrongPin, true)
                .unwrap();
            final_pin
                .fixture
                .validate_format(Point::PreStrongPin, &final_pin.format)
                .unwrap();
            let path = fixture.format_path(true);
            strong_protection(&path, "final-proof-held");
            assert_eq!(
                std::fs::remove_file(&path).unwrap_err().raw_os_error(),
                Some(32)
            );
            assert_eq!(
                std::fs::rename(&path, path.with_file_name("rename-must-fail"))
                    .unwrap_err()
                    .raw_os_error(),
                Some(32)
            );
            fixture.preserved_intent();
            eprintln!(
                "CLOSE-WINDOW CLEAN: exact root/directory/FORMAT native IDs {:?}; full canonical fixture bytes+real EOF, size/linkcount1 and closed final layout; WRITE/APPEND/DELETE opens + actual delete/rename refused32; test final proof only",
                fixture.expected
            );
        }
        Err(Failure::Native {
            point,
            operation: "same-handle no-replace directory rename",
            error,
        }) => {
            eprintln!(
                "CLOSE-WINDOW {:?} NATIVE RENAME REFUSED at {:?}: Win32={:?}, HRESULT=0x{:08x}, detail={error}; retained state, no retry/adoption",
                scenario,
                point,
                error.raw_os_error(),
                0x80070000u32 | error.raw_os_error().unwrap_or(0) as u32
            );
            assert!(!fixture.root.join(".layrs").exists());
            preserved_rejection(&fixture, scenario, false);
            let clean_stage = matches!(
                scenario,
                Arm::Clean | Arm::SameBytesReplacement | Arm::ExtraChild | Arm::MissingFile
            );
            eprintln!(
                "rename refusal: exact original stage bytes still match intent={clean_stage}; retry ALWAYS requires fresh receipt/root/stage validation; changed stages require repair"
            );
            return false;
        }
        Err(Failure::Proof {
            point: Point::PreStrongPin,
            reason,
        }) => {
            let expected = match scenario {
                Arm::SameBytesReplacement => Reason::FormatIdentity,
                Arm::ChangedBytes => Reason::BytesOrEof,
                Arm::ChangedLength => Reason::Size,
                Arm::ExtraChild => Reason::Layout,
                _ => panic!("unexpected proof refusal in {:?}: {:?}", scenario, reason),
            };
            assert_eq!(reason, expected);
            preserved_rejection(&fixture, scenario, true);
            eprintln!(
                "CLOSE-WINDOW {:?}: refused {:?} before final proof; exact intent/original byte witness and all native changed objects retained; no rewrite/adoption",
                scenario, reason
            );
        }
        Err(Failure::Native {
            point: Point::PreStrongPin,
            operation: "open final strong FORMAT",
            error,
        }) if scenario == Arm::MissingFile => {
            assert_eq!(error.raw_os_error(), Some(2));
            preserved_rejection(&fixture, scenario, true);
            eprintln!(
                "CLOSE-WINDOW MissingFile: exact Win32=2 before final proof; original FORMAT native ID+bytes retained externally; no recreation"
            );
        }
        Err(error) => panic!(
            "CLOSE-WINDOW {:?}: unexpected failure {error:?}; fixture retained at {}",
            scenario,
            fixture.base.display()
        ),
    }
    assert_eq!(
        faults.trace,
        [
            Point::PreClose,
            Point::PostClose,
            Point::PreRename,
            Point::PostRename,
            Point::PreStrongPin
        ]
    );
    eprintln!(
        "CLOSE-WINDOW {:?}: fault trace={:?}; retained fixture {}",
        scenario,
        faults.trace,
        fixture.base.display()
    );
    true
}

pub(super) fn run() {
    let api = api::Api::load().unwrap();
    let parent = PathBuf::from(
        std::env::var_os("LAYRS_VFS_TEST_PARENT").expect("explicit disposable parent required"),
    );
    let _parent_pins = pins(&api, &parent);
    let (run, _) = disposable_run("control-close-window");
    eprintln!(
        "CLOSE-WINDOW PREFLIGHT: all retained ancestor/root paths outside every sync root; only exact HRESULT=0x80070186 accepted; no registration/connection/source adapter; old failing recipes unchanged"
    );
    let outcomes = [
        Arm::Clean,
        Arm::SameBytesReplacement,
        Arm::ChangedBytes,
        Arm::ChangedLength,
        Arm::ExtraChild,
        Arm::MissingFile,
    ]
    .map(|scenario| arm(&run, &api, scenario));
    eprintln!(
        "CLOSE-WINDOW FEASIBILITY clean/replacement/bytes/length/extra/missing={outcomes:?}; fixtures retained at {}; neither atomic namespace nor process-crash/source/readiness guarantee inferred",
        run.display()
    );
    assert!(
        outcomes.into_iter().all(|passed| passed),
        "close-window prerequisite failed; retain native evidence, no broader success/fallback"
    );
}
