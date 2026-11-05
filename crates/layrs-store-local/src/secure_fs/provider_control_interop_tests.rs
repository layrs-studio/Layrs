//! Opt-in metadata-layout interoperability, using actual Store allocations and
//! actual provider control proofs. No Session, registration, SQL, FORMAT4
//! interpretation, Store bootstrap, crash durability, or readiness is tested.
//! Every fixture and receipt is retained, including failed attempts.
#![cfg(windows)]

use super::*;
use layrs_vfs::{
    CONTROL_DIRECTORY_MAX_ENCODED_BYTES, CONTROL_DIRECTORY_STAGE_MAX_ENCODED_BYTES,
    ControlDirectoryFailure, ControlDirectoryGuard, ControlDirectoryIdentity,
    ControlDirectoryPublishPoint, ControlDirectoryStageIdentity, ControlRootPreflight,
    Error as ProviderError, NoControlDirectoryPublishFaults, discover_control_directory,
    observe_control_directory_stage, publish_control_directory_recoverable,
};
use std::{
    io::{Read, Write},
    os::windows::fs::OpenOptionsExt,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
    FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_WRITE_DATA,
};

// Deliberately opaque bounded bytes, not a semantic projection FORMAT marker.
const FORMAT_BYTES: &[u8] = b"Store/provider metadata-only interoperability fixture v1\n";
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const READ_CONTROL_ACCESS: u32 = 0x0002_0000;
const DELETE_ACCESS: u32 = 0x0001_0000;

fn filesystem_identity(file: &File) -> String {
    let id = file_identity(file).unwrap();
    format!("fs:v1:{:016x}:{:016x}", id.volume, id.file)
}

// The same existing Store identity fields used by projection/file.rs, expressed
// as its decimal volume:high:low binding. No new native identity reader.
fn format_identity(file: &File) -> String {
    let id = file_identity(file).unwrap();
    assert!(u32::try_from(id.volume).is_ok());
    format!("{}:{}:{}", id.volume, id.file >> 32, id.file as u32)
}

fn observer(path: &Path, directory: bool) -> File {
    fs::OpenOptions::new()
        .access_mode(
            FILE_READ_ATTRIBUTES
                | READ_CONTROL_ACCESS
                | if directory {
                    FILE_LIST_DIRECTORY
                } else {
                    FILE_READ_DATA
                },
        )
        .share_mode(SHARE_ALL)
        .custom_flags(
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
        )
        .open(path)
        .unwrap()
}

fn read_bounded(file: &File, limit: usize) -> Vec<u8> {
    assert!(file.metadata().unwrap().len() <= limit as u64);
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes).unwrap();
    assert!(bytes.len() <= limit);
    bytes
}

fn entries(path: &Path) -> Vec<std::ffi::OsString> {
    let mut names: Vec<_> = fs::read_dir(path)
        .unwrap()
        .take(9)
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(names.len() <= 8, "fixture namespace exceeds test bound");
    names.sort();
    names
}

struct Receipt {
    name: &'static str,
    identity: FileIdentity,
    bytes: Vec<u8>,
}

impl Receipt {
    fn write(parent: &PinnedDirectory, name: &'static str, bytes: &[u8]) -> Self {
        assert!(!bytes.is_empty() && bytes.len() <= CONTROL_DIRECTORY_MAX_ENCODED_BYTES);
        let mut file = parent.create_new_private_file(name.as_ref()).unwrap();
        file.file_mut().write_all(bytes).unwrap();
        file.file().sync_all().unwrap();
        parent.sync().unwrap();
        let result = Self {
            name,
            identity: file_identity(file.file()).unwrap(),
            bytes: bytes.to_vec(),
        };
        drop(file);
        assert_eq!(result.read(parent), bytes);
        result
    }

    fn read(&self, parent: &PinnedDirectory) -> Vec<u8> {
        let file = parent.open_file(self.name.as_ref()).unwrap();
        assert_eq!(file_identity(file.file()).unwrap(), self.identity);
        let bytes = read_bounded(file.file(), CONTROL_DIRECTORY_MAX_ENCODED_BYTES);
        assert_eq!(bytes, self.bytes, "owned receipt changed");
        bytes
    }
}

#[derive(Debug, PartialEq, Eq)]
struct AllocatedIds {
    root: String,
    parent: String,
    directory: String,
    format: String,
}

impl AllocatedIds {
    fn compare(&self, allocation: &ControlDirectoryStageIdentity) {
        assert_eq!(allocation.root_filesystem_identity(), self.root);
        assert_eq!(allocation.stage_parent_filesystem_identity(), self.parent);
        assert_eq!(allocation.directory_filesystem_identity(), self.directory);
        assert_eq!(allocation.format_binding_identity(), self.format);
    }
}

fn checked_allocation(
    preflight: &ControlRootPreflight,
    parent: &PinnedDirectory,
    stage: &PinnedDirectory,
    format: &PinnedFile,
) -> (ControlDirectoryStageIdentity, AllocatedIds) {
    let root = PinnedDirectory::open(preflight.root()).unwrap();
    assert_eq!(format.file().metadata().unwrap().len(), 0);
    let ids = AllocatedIds {
        root: root.identity_token(),
        parent: parent.identity_token(),
        directory: stage.identity_token(),
        format: format_identity(format.file()),
    };
    assert_eq!(ids.root, preflight.filesystem_identity());
    // Observation is DATA only. Independently retained Store handles establish
    // allocation identities before the encoded record is persisted or used.
    let allocation = observe_control_directory_stage(preflight, &stage.display_path).unwrap();
    ids.compare(&allocation);
    assert_eq!(allocation.root(), preflight.root());
    assert_eq!(allocation.stage().parent(), Some(allocation.stage_parent()));
    assert!(allocation.encode().len() <= CONTROL_DIRECTORY_STAGE_MAX_ENCODED_BYTES);
    root.verify_path_identity().unwrap();
    parent.verify_path_identity().unwrap();
    stage.verify_path_identity().unwrap();
    format.verify_parent().unwrap();
    (allocation, ids)
}

struct Fixture {
    preflight: ControlRootPreflight,
    root: PathBuf,
    parent: PathBuf,
    stage: PathBuf,
    metadata: PathBuf,
    metadata_identity: String,
    allocation: Receipt,
    ids: AllocatedIds,
}

impl Fixture {
    fn new(label: &str, keep_writer: bool) -> (Self, Option<File>) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let explicit_parent = PathBuf::from(
            std::env::var_os("LAYRS_VFS_TEST_PARENT")
                .expect("explicit disposable LAYRS_VFS_TEST_PARENT is required"),
        );
        assert!(explicit_parent.is_absolute() && explicit_parent.is_dir());
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = explicit_parent.join(format!(
            "layrs-store-control-{label}-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&base).unwrap();
        eprintln!("RETAINED Store/provider interop fixture {}", base.display());
        // Ordinary fixture scaffolding; control stage/FORMAT/receipt allocation
        // below exclusively uses the real Store private creation helpers.
        let root_path = base.join("checkout");
        let parent_path = base.join("stage-parent");
        fs::create_dir(&root_path).unwrap();
        fs::create_dir(&parent_path).unwrap();
        let preflight = ControlRootPreflight::open(&root_path).unwrap();
        let root = preflight.root().to_path_buf();
        let base_pin = PinnedDirectory::open(&base).unwrap();
        let metadata = base_pin
            .create_new_private_child("receipts".as_ref())
            .unwrap();
        let parent = PinnedDirectory::open(&parent_path).unwrap();
        let stage = parent
            .create_new_private_child("control-stage".as_ref())
            .unwrap();
        let mut format = stage.create_new_private_file("FORMAT".as_ref()).unwrap();
        let (allocated, ids) = checked_allocation(&preflight, &parent, &stage, &format);
        let allocation = Receipt::write(&metadata, "allocation.bin", &allocated.encode());
        let recorded = ControlDirectoryStageIdentity::decode(&allocation.read(&metadata)).unwrap();
        assert_eq!(recorded, allocated);
        ids.compare(&recorded);
        // Exact allocation receipt exists and is read back BEFORE FORMAT bytes.
        assert_eq!(format.file().metadata().unwrap().len(), 0);
        format.file_mut().write_all(FORMAT_BYTES).unwrap();
        format.file().sync_all().unwrap();
        stage.sync().unwrap();
        parent.sync().unwrap();
        let writer = keep_writer.then(|| format.file().try_clone().unwrap());
        let value = Self {
            root,
            parent: allocated.stage_parent().to_path_buf(),
            stage: allocated.stage().to_path_buf(),
            metadata: metadata.display_path.clone(),
            metadata_identity: metadata.identity_token(),
            preflight,
            allocation,
            ids,
        };
        // PinnedFile also owns a parent duplicate. Close every construction pin
        // before publication; only the explicit negative keeps a raw writer.
        drop(format);
        drop(stage);
        drop(parent);
        drop(metadata);
        drop(base_pin);
        (value, writer)
    }

    fn metadata_pin(&self) -> PinnedDirectory {
        let pin = PinnedDirectory::open(&self.metadata).unwrap();
        assert_eq!(pin.identity_token(), self.metadata_identity);
        pin
    }

    fn recorded_allocation(&self) -> ControlDirectoryStageIdentity {
        let value =
            ControlDirectoryStageIdentity::decode(&self.allocation.read(&self.metadata_pin()))
                .unwrap();
        self.ids.compare(&value);
        assert_eq!(value.root(), self.root);
        assert_eq!(value.stage(), self.stage);
        value
    }

    fn expected(&self, prepared: &Receipt) -> ControlDirectoryIdentity {
        let expected =
            ControlDirectoryIdentity::decode(&prepared.read(&self.metadata_pin())).unwrap();
        assert_eq!(expected.stage_identity(), &self.recorded_allocation());
        expected
    }

    fn publish(&self) -> (ControlDirectoryGuard, Receipt) {
        let allocated = self.recorded_allocation();
        let mut calls = 0;
        let mut receipt = None;
        let guard = publish_control_directory_recoverable(
            &self.preflight,
            allocated.stage(),
            &allocated,
            FORMAT_BYTES,
            None,
            |expected| {
                calls += 1;
                assert_eq!(expected.stage_identity(), &allocated);
                receipt = Some(Receipt::write(
                    &self.metadata_pin(),
                    "prepared.bin",
                    &expected.encode(),
                ));
                Ok(())
            },
            &NoControlDirectoryPublishFaults,
        )
        .unwrap();
        assert_eq!(calls, 1);
        let receipt = receipt.unwrap();
        self.assert_guard(&guard, &receipt);
        (guard, receipt)
    }

    fn assert_guard(&self, guard: &ControlDirectoryGuard, prepared: &Receipt) {
        let expected = self.expected(prepared);
        assert_eq!(guard.identity(), &expected);
        assert_eq!(guard.root(), self.root);
        assert_eq!(guard.root_filesystem_identity(), self.ids.root);
        assert_eq!(guard.directory_filesystem_identity(), self.ids.directory);
        assert_eq!(guard.format_binding_identity(), self.ids.format);
        assert_eq!(guard.format_bytes(), FORMAT_BYTES);
        guard.revalidate().unwrap();
        guard.revalidate_bootstrap_root().unwrap();
        let observed = discover_control_directory(&self.root).unwrap();
        assert_eq!(observed.format_bytes(), FORMAT_BYTES);
        // Final observation is compared to the authenticated test receipt;
        // it is never promoted to expected identity or a retained guard.
        assert!(observed.identity().matches_expected(&expected));
    }

    fn unpublished(&self) -> StagedWitness {
        assert_eq!(entries(&self.root), Vec::<std::ffi::OsString>::new());
        assert_eq!(entries(&self.stage), [std::ffi::OsString::from("FORMAT")]);
        let stage = observer(&self.stage, true);
        let format = observer(&self.stage.join("FORMAT"), false);
        let bytes = read_bounded(&format, FORMAT_BYTES.len());
        assert_eq!(bytes, FORMAT_BYTES);
        StagedWitness {
            stage: file_identity(&stage).unwrap(),
            format: file_identity(&format).unwrap(),
            bytes,
            parent_names: entries(&self.parent),
            metadata_names: entries(&self.metadata),
            allocation: self.allocation.read(&self.metadata_pin()),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct StagedWitness {
    stage: FileIdentity,
    format: FileIdentity,
    bytes: Vec<u8>,
    parent_names: Vec<std::ffi::OsString>,
    metadata_names: Vec<std::ffi::OsString>,
    allocation: Vec<u8>,
}

fn refused_before_persist(fixture: &Fixture, operation: &str, writer: bool) {
    let before = fixture.unpublished();
    let allocated = fixture.recorded_allocation();
    let mut calls = 0;
    let error = match publish_control_directory_recoverable(
        &fixture.preflight,
        allocated.stage(),
        &allocated,
        FORMAT_BYTES,
        None,
        |_| {
            calls += 1;
            Err(io::Error::other("unexpected persist in refusal fixture").into())
        },
        &NoControlDirectoryPublishFaults,
    ) {
        Ok(_) => panic!("provider adopted a refused allocation"),
        Err(error) => error,
    };
    assert_eq!(calls, 0);
    match &error {
        ProviderError::ControlDirectory {
            kind,
            point,
            operation: actual,
            cause,
            ..
        } => {
            assert_eq!(*kind, ControlDirectoryFailure::NeedsRepair);
            assert_eq!(*point, ControlDirectoryPublishPoint::StageValidated);
            assert_eq!(*actual, operation);
            if writer {
                match cause.as_ref() {
                    ProviderError::Io(error) => assert_eq!(error.raw_os_error(), Some(32)),
                    cause => panic!("expected native writer sharing refusal, got {cause:?}"),
                }
            } else {
                assert!(matches!(cause.as_ref(), ProviderError::IdentityMismatch));
            }
        }
        error => panic!("expected scoped publication refusal, got {error:?}"),
    }
    assert_eq!(fixture.unpublished(), before);
    assert!(!fixture.metadata.join("prepared.bin").exists());
}

fn assert_sharing_refusal(path: &Path, directory: bool, access: u32) {
    let error = fs::OpenOptions::new()
        .access_mode(access)
        .share_mode(SHARE_ALL)
        .custom_flags(
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
        )
        .open(path)
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(32));
}

#[test]
#[ignore = "requires explicit disposable LAYRS_VFS_TEST_PARENT; real Store/provider control handoff, no registration"]
fn store_private_allocation_publishes_provider_control_and_reopens() {
    let (fixture, writer) = Fixture::new("clean", false);
    assert!(writer.is_none());
    let (guard, prepared) = fixture.publish();
    assert!(!fixture.stage.exists());
    let control = fixture.root.join(".layrs");
    let observed_directory = observer(&control, true);
    let observed_format = observer(&control.join("FORMAT"), false);
    assert_eq!(
        filesystem_identity(&observed_directory),
        fixture.ids.directory
    );
    assert_eq!(format_identity(&observed_format), fixture.ids.format);
    assert_eq!(
        read_bounded(&observed_format, FORMAT_BYTES.len()),
        FORMAT_BYTES
    );
    let cloned = guard.try_clone().unwrap();
    drop(guard);
    fixture.assert_guard(&cloned, &prepared);
    assert_sharing_refusal(&control, true, DELETE_ACCESS | FILE_LIST_DIRECTORY);
    assert_sharing_refusal(&control.join("FORMAT"), false, FILE_WRITE_DATA);
    drop(observed_format);
    drop(observed_directory);
    drop(cloned);
    // No final control/FORMAT guard or observer survives. Reopen from the exact
    // persisted prepared receipt, never from discover_control_directory data.
    let fresh = ControlDirectoryGuard::open(&fixture.root, &fixture.expected(&prepared)).unwrap();
    fixture.assert_guard(&fresh, &prepared);
}

#[test]
#[ignore = "requires explicit disposable LAYRS_VFS_TEST_PARENT; actual Store writer duplicate and exact retry"]
fn store_format_writer_duplicate_refuses_before_persist_then_retries() {
    let (fixture, writer) = Fixture::new("writer-duplicate", true);
    let writer = writer.expect("retained actual Store FORMAT writer duplicate");
    assert_eq!(format_identity(&writer), fixture.ids.format);
    refused_before_persist(&fixture, "acquire strong FORMAT", true);
    drop(writer);
    // Retry uses the same independently recorded allocation and exact bytes.
    let (guard, prepared) = fixture.publish();
    fixture.assert_guard(&guard, &prepared);
}

#[test]
#[ignore = "requires explicit disposable LAYRS_VFS_TEST_PARENT; preserves owned original/replacement objects"]
fn store_recorded_allocation_rejects_same_byte_stage_and_format_replacements() {
    for replace_directory in [true, false] {
        let label = if replace_directory {
            "replace-stage"
        } else {
            "replace-format"
        };
        let (fixture, writer) = Fixture::new(label, false);
        assert!(writer.is_none());
        let original = fixture.unpublished();
        let retained = fixture.metadata.join(if replace_directory {
            "original-stage"
        } else {
            "original-FORMAT"
        });
        assert!(!retained.exists());
        // Controlled rename of our exact disposable allocation, never delete.
        fs::rename(
            if replace_directory {
                fixture.stage.clone()
            } else {
                fixture.stage.join("FORMAT")
            },
            &retained,
        )
        .unwrap();
        let parent = PinnedDirectory::open(&fixture.parent).unwrap();
        let stage = if replace_directory {
            parent
                .create_new_private_child("control-stage".as_ref())
                .unwrap()
        } else {
            PinnedDirectory::open(&fixture.stage).unwrap()
        };
        let mut format = stage.create_new_private_file("FORMAT".as_ref()).unwrap();
        let (replacement, replacement_ids) =
            checked_allocation(&fixture.preflight, &parent, &stage, &format);
        let replacement_receipt = Receipt::write(
            &fixture.metadata_pin(),
            "replacement-allocation.bin",
            &replacement.encode(),
        );
        assert_eq!(
            ControlDirectoryStageIdentity::decode(
                &replacement_receipt.read(&fixture.metadata_pin())
            )
            .unwrap(),
            replacement
        );
        assert_ne!(replacement_ids.format, fixture.ids.format);
        if replace_directory {
            assert_ne!(replacement_ids.directory, fixture.ids.directory);
        } else {
            assert_eq!(replacement_ids.directory, fixture.ids.directory);
        }
        format.file_mut().write_all(FORMAT_BYTES).unwrap();
        format.file().sync_all().unwrap();
        stage.sync().unwrap();
        parent.sync().unwrap();
        drop(format);
        drop(stage);
        drop(parent);
        refused_before_persist(
            &fixture,
            if replace_directory {
                "bind allocated stage"
            } else {
                "bind allocated FORMAT"
            },
            false,
        );
        // The new receipt never replaces the publication key. Both old and new
        // allocation records and both sets of physical bytes survive refusal.
        assert_eq!(
            replacement_receipt.read(&fixture.metadata_pin()),
            replacement.encode()
        );
        let retained_format = if replace_directory {
            retained.join("FORMAT")
        } else {
            retained.clone()
        };
        if replace_directory {
            assert_eq!(
                file_identity(&observer(&retained, true)).unwrap(),
                original.stage
            );
            assert_eq!(entries(&retained), [std::ffi::OsString::from("FORMAT")]);
        }
        let retained_file = observer(&retained_format, false);
        assert_eq!(file_identity(&retained_file).unwrap(), original.format);
        assert_eq!(
            read_bounded(&retained_file, FORMAT_BYTES.len()),
            original.bytes
        );
    }
}
