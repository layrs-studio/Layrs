//! Native negative gates. Full factory mutation arms and direct initial-file
//! audit predicates are deliberately distinct. No fabricated kernel records.
use super::*;
use std::{
    io::{Read, Seek, SeekFrom, Write},
    mem::{offset_of, size_of, size_of_val},
};
use windows_sys::{Win32::System::IO::DeviceIoControl, core::GUID};

#[derive(Debug)]
struct Probe {
    native: io::Id,
    size: u64,
    on_disk: i64,
    modified: i64,
    in_sync: bool,
    partial: bool,
    backing: Vec<u8>,
}
fn probe(file: &File) -> Probe {
    let api = Api::load().unwrap();
    let native = fs::identity(file).unwrap();
    let attrs = fs::attributes(file).unwrap();
    let state = unsafe { (api.state)(attrs.FileAttributes, attrs.ReparseTag) };
    assert_ne!(state, CF_PLACEHOLDER_STATE_INVALID);
    assert_ne!(state & CF_PLACEHOLDER_STATE_PLACEHOLDER, 0);
    let mut storage = [0u64; 1024];
    let mut returned = 0;
    super::super::api::check("CfGetPlaceholderInfo(test native witness)", unsafe {
        (api.placeholder_info)(
            fs::handle(file),
            CF_PLACEHOLDER_INFO_STANDARD,
            storage.as_mut_ptr().cast(),
            size_of_val(&storage) as u32,
            &mut returned,
        )
    })
    .unwrap();
    let offset = offset_of!(CF_PLACEHOLDER_STANDARD_INFO, FileIdentity);
    assert!((offset..=size_of_val(&storage)).contains(&(returned as usize)));
    let info = unsafe { &*storage.as_ptr().cast::<CF_PLACEHOLDER_STANDARD_INFO>() };
    assert!(
        info.FileIdentityLength <= 4096
            && offset + info.FileIdentityLength as usize <= returned as usize
    );
    assert_eq!(info.FileId as u64, fs::index(&native));
    let backing = unsafe {
        std::slice::from_raw_parts(
            storage.as_ptr().cast::<u8>().add(offset),
            info.FileIdentityLength as usize,
        )
    }
    .to_vec();
    Probe {
        native: (native.dwVolumeSerialNumber, fs::index(&native)),
        size: fs::size(&native),
        on_disk: info.OnDiskDataSize,
        modified: info.ModifiedDataSize,
        in_sync: info.InSyncState == CF_IN_SYNC_STATE_IN_SYNC
            && state & CF_PLACEHOLDER_STATE_IN_SYNC != 0,
        partial: state & CF_PLACEHOLDER_STATE_PARTIAL != 0,
        backing,
    }
}
fn metadata_file(path: &Path) -> File {
    fs::open(path, io::ALL_SHARING, FILE_READ_ATTRIBUTES).unwrap()
}

fn set_guid_reparse(file: &File) {
    let mut buffer = [0u8; 32];
    buffer[..4].copy_from_slice(&0x00000042u32.to_le_bytes());
    buffer[4..6].copy_from_slice(&8u16.to_le_bytes());
    buffer[8..24].copy_from_slice(&[
        0x61, 0x2e, 0x40, 0x7a, 0x1a, 0x03, 0x4f, 0x48, 0x92, 0x2d, 0x18, 0x61, 0x0a, 0xa3, 0x47,
        0x52,
    ]);
    buffer[24..].copy_from_slice(b"STAGE2RP");
    let mut returned = 0;
    let ok = unsafe {
        DeviceIoControl(
            fs::handle(file),
            0x900a4,
            buffer.as_ptr().cast(),
            buffer.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        panic!(
            "UNQUALIFIED derived-directory reparse mutation: exact native {:?}; denied mutation is not a refusal pass",
            std::io::Error::last_os_error()
        );
    }
    let attrs = fs::attributes(file).unwrap();
    assert_eq!(attrs.ReparseTag, 0x42);
    assert_ne!(attrs.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT, 0);
}

#[test]
#[ignore = "actual wrong size and derived-directory replacement/reparse before factory audit; witnesses retained"]
fn stage2_additional_factory_metadata_refusals() {
    let run = fixture::run("factory-metadata");
    for mutation in ["wrong-size", "derived-replacement", "derived-reparse"] {
        let (f, control) = fixture::Fixture::create(&run, mutation);
        let root = f.root.clone();
        let base = f.base.clone();
        let changed = Arc::new(AtomicBool::new(false));
        let flag = changed.clone();
        let outcome = population::test_hooks::scoped(
            move |point| {
                if point != Point::BeforeAudit || flag.swap(true, Ordering::SeqCst) {
                    return Ok(());
                }
                let path = root.join("Dir/data");
                let original = metadata_file(&path);
                let before = probe(&original);
                fixture::write_new(
                    &base.join("original-metadata-witness.txt"),
                    format!("{before:?}\n").as_bytes(),
                );
                match mutation {
                    "wrong-size" => {
                        let metadata = CF_FS_METADATA {
                            BasicInfo: FILE_BASIC_INFO::default(),
                            FileSize: before.size as i64 + 1,
                        };
                        super::negatives::update_placeholder(&path, Some(&metadata), None, false);
                        let after = probe(&original);
                        assert_eq!(after.native, before.native);
                        assert_eq!(after.backing, before.backing);
                        assert_eq!(after.size, before.size + 1);
                        eprintln!("ACTUAL wrong-size before={before:?}, after={after:?}");
                    }
                    "derived-replacement" => {
                        let directory = metadata_file(&root.join("Dir"));
                        let old = io::id(&directory).unwrap();
                        // NTFS refuses the parent rename while this child is open.
                        // Retain the recorded witness and the directory observer.
                        drop(original);
                        std::fs::rename(root.join("Dir"), root.join("Dir-retained-witness"))
                            .unwrap();
                        std::fs::create_dir(root.join("Dir")).unwrap();
                        let replacement = metadata_file(&root.join("Dir"));
                        assert_ne!(io::id(&replacement).unwrap(), old);
                        assert_eq!(io::id(&directory).unwrap(), old);
                        assert_eq!(
                            io::id(&metadata_file(&root.join("Dir-retained-witness"))).unwrap(),
                            old
                        );
                        let relocated = metadata_file(&root.join("Dir-retained-witness/data"));
                        let after = probe(&relocated);
                        assert_eq!(after.native, before.native);
                        assert_eq!(after.backing, before.backing);
                        assert_eq!(after.size, before.size);
                        assert_eq!(after.on_disk, before.on_disk);
                        assert_eq!(after.modified, before.modified);
                        assert_eq!(after.in_sync, before.in_sync);
                        assert_eq!(after.partial, before.partial);
                        eprintln!(
                            "ACTUAL derived directory replacement old={old:?}, new={:?}; original directory+placeholder retained",
                            io::id(&replacement).unwrap()
                        );
                    }
                    "derived-reparse" => {
                        let directory = fs::open(
                            &root.join("Dir"),
                            io::ALL_SHARING,
                            FILE_WRITE_ATTRIBUTES | FILE_READ_ATTRIBUTES,
                        )
                        .unwrap();
                        let old = io::id(&directory).unwrap();
                        fixture::write_new(
                            &base.join("original-directory-witness.txt"),
                            format!("{old:?}\n").as_bytes(),
                        );
                        // The denied nonempty-directory mutation is separate evidence.
                        // Retain its child within this sync root; delete nothing.
                        let witness = root.join("Dir-data-retained-witness");
                        assert!(matches!(
                            std::fs::symlink_metadata(&witness),
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound
                        ));
                        drop(original);
                        std::fs::rename(&path, &witness).unwrap_or_else(|error| {
                            panic!(
                                "UNQUALIFIED same-root child relocation before reparse: exact native {error:?}"
                            )
                        });
                        let relocated = metadata_file(&witness);
                        let after = probe(&relocated);
                        eprintln!(
                            "ACTUAL same-root child relocation before={before:?}, after={after:?}"
                        );
                        assert_eq!(after.native, before.native);
                        assert_eq!(after.backing, before.backing);
                        assert_eq!(after.size, before.size);
                        assert_eq!(after.on_disk, before.on_disk);
                        assert_eq!(after.modified, before.modified);
                        // Current native TRACK_ALL relocation clears in-sync; do not reset it.
                        assert!(before.in_sync, "initial placeholder must be in sync");
                        assert!(
                            !after.in_sync,
                            "relocation must clear in-sync under TRACK_ALL"
                        );
                        assert_eq!(after.partial, before.partial);
                        assert!(
                            std::fs::read_dir(root.join("Dir"))
                                .unwrap()
                                .next()
                                .transpose()
                                .unwrap()
                                .is_none(),
                            "derived directory must be empty after retaining its child"
                        );
                        assert_eq!(io::id(&directory).unwrap(), old);
                        set_guid_reparse(&directory);
                        assert_eq!(io::id(&directory).unwrap(), old);
                        assert_eq!(io::id(&metadata_file(&root.join("Dir"))).unwrap(), old);
                        eprintln!(
                            "ACTUAL empty derived directory reparse tag42/nativeID={old:?}; child relocated within root to {} with native/backing/size/on-disk/modified/partial unchanged and in-sync cleared: {after:?}; not an in-place nonempty-directory mutation",
                            witness.display()
                        );
                    }
                    _ => unreachable!(),
                }
                Ok(())
            },
            || {
                ProviderRootGuard::create_with_control_directory(
                    &control,
                    f.root_id,
                    catalog(false),
                    |ticket| f.persist(ticket),
                )
            },
        );
        assert!(changed.load(Ordering::SeqCst));
        assert!(outcome.is_err(), "{mutation} incorrectly reached READY");
        if mutation == "derived-reparse" {
            assert!(
                matches!(
                    &outcome,
                    Err(Error::Unsupported("ordinary control directory required"))
                ),
                "expected actual reparse/plain-directory refusal before namespace comparison, got {:?}",
                outcome.as_ref().err()
            );
            eprintln!(
                "ACTUAL factory reparse refusal: {:?}",
                outcome.as_ref().err()
            );
        }
        assert!(matches!(
            f.ticket()
                .guard_with_control_directory(catalog(false), &control),
            Err(Error::NeedsRepair(_))
        ));
        assert_eq!(calls(), (0, 0));
        eprintln!(
            "PASS full factory {mutation}: actual mutation, audit refusal, exact INCOMPLETE retained"
        );
    }
}

#[test]
#[ignore = "real READY hydration/private write then exact initial-file audit predicate; not source-free factory coverage"]
fn stage2_resident_dirty_initial_audit_predicates() {
    let run = fixture::run("resident-dirty-predicates");
    for dirty in [false, true] {
        let (f, control) = fixture::Fixture::create(&run, if dirty { "dirty" } else { "resident" });
        let catalog = catalog(false);
        let proof = create(&f, &control, catalog.clone());
        let ticket = proof.registration().clone();
        let source = Source::new(catalog.clone());
        let mut session =
            Session::reconnect_disconnected_with_control_directory(proof, source.clone()).unwrap();
        session.connect().unwrap();
        let path = RepoPath::parse("Dir/data").unwrap();
        let before = probe(&metadata_file(&f.root.join(path.as_str())));
        assert_eq!(before.on_disk, 0);
        assert_eq!(before.modified, 0);
        assert!(before.partial && before.in_sync);
        session.hydrate_for_hash(&path).unwrap();
        assert!(source.bytes.load(Ordering::Acquire) > 0);
        assert_eq!(std::fs::read(f.root.join(path.as_str())).unwrap(), data());
        let resident = probe(&metadata_file(&f.root.join(path.as_str())));
        assert!(!resident.partial && resident.in_sync);
        assert!(resident.on_disk >= resident.size as i64);
        assert_eq!(resident.modified, 0);
        let source_bytes = source.bytes.load(Ordering::Acquire);
        let mut expected = data();
        if dirty {
            expected[..4096].fill(0xe3);
            let mut writer = fs::open(
                &f.root.join(path.as_str()),
                io::ALL_SHARING,
                FILE_WRITE_DATA | FILE_READ_ATTRIBUTES,
            )
            .unwrap();
            writer.seek(SeekFrom::Start(0)).unwrap();
            writer.write_all(&expected[..4096]).unwrap();
            writer.sync_all().unwrap();
            drop(writer);
        }
        let file = fs::open(
            &f.root.join(path.as_str()),
            FILE_SHARE_READ,
            FILE_READ_DATA | FILE_READ_ATTRIBUTES,
        )
        .unwrap();
        let actual = probe(&file);
        assert_eq!(actual.native, before.native);
        assert_eq!(actual.size, before.size);
        assert_eq!(actual.backing, before.backing);
        assert!(!actual.partial);
        assert!(actual.on_disk >= actual.size as i64);
        if dirty {
            assert!(actual.modified > 0);
            assert!(!actual.in_sync);
        } else {
            assert_eq!(actual.modified, 0);
            assert!(actual.in_sync);
        }
        let mut read = file.try_clone().unwrap();
        let mut bytes = Vec::new();
        read.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, expected);
        drop(read);
        assert_eq!(source.bytes.load(Ordering::Acquire), source_bytes);
        fixture::write_new(
            &f.base.join("native-state-witness.txt"),
            format!("{actual:?}\n").as_bytes(),
        );
        fixture::write_new(&f.base.join("exact-native-byte-witness"), &bytes);
        eprintln!("ACTUAL initial-audit-predicate dirty={dirty}: {actual:?}");
        let index = catalog
            .entries()
            .iter()
            .position(|entry| entry.path == path)
            .unwrap();
        let api = Api::load().unwrap();
        assert!(matches!(
            population::audit_file_for_test(&api, &ticket, &catalog, index, &file),
            Err(Error::NeedsRepair(_))
        ));
        drop(file);
        session.disconnect().unwrap();
        session.root_guard().unwrap().revalidate().unwrap(); // Valid READY edits are not rejected by complete proof.
        assert_eq!(ticket.encode(), f.ticket().encode());
        eprintln!(
            "PASS native initial-file AUDIT PREDICATE dirty={dirty}; actual size/residency/modified/backing/bytes proved; no initial-factory hydration or per-predicate independence claim; READY registration/private bytes retained"
        );
    }
    eprintln!(
        "LIMIT: root-volume/rootID mismatch predicates remain source-inspected, not separately manufactured native kernel records on the same-volume pinned layout."
    );
}

fn register_foreign_control(path: &Path) {
    let api = Api::load().unwrap();
    let path = fs::native_path(path).unwrap();
    let name: Vec<u16> = "Stage2 foreign nested fixture\0".encode_utf16().collect();
    let version = [b'1' as u16, 0];
    let identity = b"owned-test-foreign-nested-control";
    let registration = CF_SYNC_REGISTRATION {
        StructSize: size_of::<CF_SYNC_REGISTRATION>() as u32,
        ProviderName: name.as_ptr(),
        ProviderVersion: version.as_ptr(),
        SyncRootIdentity: identity.as_ptr().cast(),
        SyncRootIdentityLength: identity.len() as u32,
        FileIdentity: std::ptr::null(),
        FileIdentityLength: 0,
        ProviderId: GUID::from_u128(0xc34d5080_5f37_4981_b726_614bec4cb46c),
    };
    let policies = CF_SYNC_POLICIES {
        StructSize: size_of::<CF_SYNC_POLICIES>() as u32,
        Hydration: CF_HYDRATION_POLICY {
            Primary: CF_HYDRATION_POLICY_PROGRESSIVE,
            Modifier: CF_HYDRATION_POLICY_MODIFIER_NONE,
        },
        Population: CF_POPULATION_POLICY {
            Primary: CF_POPULATION_POLICY_ALWAYS_FULL,
            Modifier: CF_POPULATION_POLICY_MODIFIER_NONE,
        },
        InSync: CF_INSYNC_POLICY_TRACK_ALL,
        HardLink: CF_HARDLINK_POLICY_NONE,
        PlaceholderManagement: CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT,
    };
    let hr = unsafe {
        (api.register)(
            path.as_ptr(),
            &registration,
            &policies,
            CF_REGISTER_FLAG_DISABLE_ON_DEMAND_POPULATION_ON_ROOT,
        )
    };
    eprintln!(
        "ACTUAL foreign nested .layrs registration HRESULT0x{:08X}",
        hr as u32
    );
    assert!(
        hr >= 0,
        "native mutation not established; no refusal qualification"
    );
}

#[test]
#[ignore = "actual ordinary-control PINNED/UNPINNED flips and nested registration refusal; all roots retained"]
fn stage2_control_attribute_and_nested_refusals() {
    let run = fixture::run("control-metadata");
    for leaf in [".layrs", ".layrs/FORMAT"] {
        for flag in [FILE_ATTRIBUTE_PINNED, FILE_ATTRIBUTE_UNPINNED] {
            let label = format!(
                "{}-{flag:x}",
                if leaf == ".layrs" {
                    "directory"
                } else {
                    "format"
                }
            );
            let (f, control) = fixture::Fixture::create(&run, &label);
            let proof = create(&f, &control, catalog(false));
            let path = f.root.join(leaf);
            let file = fs::open(
                &path,
                io::ALL_SHARING,
                FILE_WRITE_ATTRIBUTES | FILE_READ_ATTRIBUTES,
            )
            .unwrap();
            let old = io::id(&file).unwrap();
            let attributes = fs::attributes(&file).unwrap();
            let value = FILE_BASIC_INFO {
                FileAttributes: (attributes.FileAttributes & !FILE_ATTRIBUTE_NORMAL) | flag,
                ..Default::default()
            };
            let ok = unsafe {
                SetFileInformationByHandle(
                    fs::handle(&file),
                    FileBasicInfo,
                    (&value as *const FILE_BASIC_INFO).cast(),
                    size_of::<FILE_BASIC_INFO>() as u32,
                )
            };
            assert_ne!(
                ok,
                0,
                "UNQUALIFIED ordinary-control attribute flip: {:?}",
                std::io::Error::last_os_error()
            );
            let actual = fs::attributes(&file).unwrap();
            assert_ne!(
                actual.FileAttributes & flag,
                0,
                "UNQUALIFIED native mutation did not persist requested flag"
            );
            assert_eq!(io::id(&file).unwrap(), old);
            assert_eq!(actual.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT, 0);
            assert!(proof.revalidate().is_err());
            assert!(control.revalidate().is_err());
            assert_eq!(
                std::fs::read(f.root.join(".layrs/FORMAT")).unwrap(),
                fixture::FORMAT
            );
            eprintln!(
                "PASS actual ordinary-control {leaf} flag0x{flag:x} -> layout/complete proof refusal; bytes/native identity retained"
            );
        }
    }
    let (f, control) = fixture::Fixture::create(&run, "foreign-nested-before-factory");
    register_foreign_control(&f.root.join(".layrs"));
    assert!(
        ProviderRootGuard::create_with_control_directory(
            &control,
            f.root_id,
            catalog(false),
            |_| panic!("foreign nested root reached ticket persistence")
        )
        .is_err()
    );
    assert!(!f.base.join("ticket.bin").exists());
    assert_eq!(calls(), (0, 0));
    eprintln!(
        "PASS actual nested .layrs registration after Stage1 publication/before V2 factory -> refusal before ticket hook; registration retained. LIMITS: ordinary direct attribute flips, not recursive CfSetPinState qualification; no post-READY nested-registration mutation claim; no hydration/dehydration/policy changes."
    );
}
