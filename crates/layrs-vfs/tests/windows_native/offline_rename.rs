//! Offline rename matrix of the native CFAPI tests.
use super::*;

pub(super) fn offline_rename_matrix(export: bool) {
    let (run, nonce) = disposable_run("offline-rename");
    let mut case_index = 0u128;
    for lifecycle in ["disconnect", "process-exit"] {
        for state in ["partial", "full", "dirty"] {
            for kind in ["case-only", "distinct"] {
                case_index += 1;
                let case = run.join(format!("{lifecycle}-{state}-{kind}"));
                std::fs::create_dir(&case).unwrap();
                let root = case.join("root");
                std::fs::create_dir(&root).unwrap();
                let source = Arc::new(Fixture::new());
                let ticket_path = case.join("ticket");
                let mut session = Session::create_with_registration_hook(
                    &root,
                    (nonce + case_index).to_le_bytes(),
                    source.clone(),
                    |ticket| {
                        let mut file = std::fs::File::create(&ticket_path)?;
                        file.write_all(&ticket.encode())?;
                        file.sync_all()?;
                        Ok(())
                    },
                )
                .unwrap();
                let original = if state == "partial" {
                    "untouched.bin"
                } else {
                    "notes.txt"
                };
                let destination = if kind == "case-only" {
                    original.to_ascii_uppercase()
                } else {
                    format!("moved-{original}")
                };
                if state != "partial" {
                    child(&root, "full-notes", "");
                }
                if state == "dirty" {
                    child(&root, "write", "private notes!!\n");
                }
                let before = placeholder_probe(&root.join(original));
                let evidence = session
                    .inspect(&RepoPath::parse(original).unwrap())
                    .unwrap();
                eprintln!(
                    "OFFLINE RENAME baseline {lifecycle}/{state}/{kind}: {:?} {:?}",
                    evidence.evidence, evidence.placeholder
                );
                if state == "partial" {
                    assert!(matches!(
                        evidence.evidence,
                        CaptureEvidence::Immutable { .. }
                    ));
                    assert_eq!(before.on_disk, 0);
                } else {
                    assert!(matches!(
                        evidence.evidence,
                        CaptureEvidence::NeedsHash { .. }
                    ));
                }
                drop(evidence);
                let reads = source.reads.load(Ordering::SeqCst);
                session.disconnect().unwrap();
                let mut retained_session = Some(session);
                if lifecycle == "process-exit" {
                    drop(retained_session.take());
                    child(&root, "exit-before-rename", ticket_path.to_str().unwrap());
                }
                // Bound the native rename in a child, including any kernel wait.
                child(
                    &root,
                    "offline-rename",
                    &format!("{original}|{destination}"),
                );
                let outcome = std::fs::read_to_string(case.join("rename-result")).unwrap();
                eprintln!("OFFLINE RENAME {lifecycle}/{state}/{kind}: {outcome}");
                let renamed = outcome == "SUCCESS";
                let current = if renamed {
                    destination.as_str()
                } else {
                    original
                };
                let after = placeholder_probe(&root.join(current));
                assert_eq!(before.native_id, after.native_id);
                assert_eq!(before.backing_identity, after.backing_identity);
                assert_eq!(before.size, after.size);
                assert_eq!(
                    before.on_disk, after.on_disk,
                    "rename/probe hydrated content"
                );
                assert_eq!(before.modified, after.modified);
                assert!(
                    std::fs::read_dir(&root)
                        .unwrap()
                        .any(|entry| entry.unwrap().file_name() == std::ffi::OsStr::new(current))
                );
                drop(retained_session);
                let session = Session::reconnect(
                    Registration::decode(&std::fs::read(&ticket_path).unwrap()).unwrap(),
                    source.clone(),
                )
                .unwrap();
                if renamed {
                    assert!(matches!(
                        session.inspect(&RepoPath::parse(current).unwrap()),
                        Err(Error::Unsupported(
                            "placeholder renamed: identity is preserved, capture reconciliation required"
                        ))
                    ));
                    assert!(matches!(
                        session.checkpoint(),
                        Err(Error::Unsupported(
                            "placeholder renamed: identity is preserved, capture reconciliation required"
                        ))
                    ));
                } else {
                    session.inspect(&RepoPath::parse(current).unwrap()).unwrap();
                }
                assert_eq!(
                    source.reads.load(Ordering::SeqCst),
                    reads,
                    "metadata reconciliation read immutable bytes"
                );
                if state != "partial" {
                    let expected: &[u8] = if state == "dirty" {
                        b"private notes!!\n"
                    } else {
                        b"immutable notes\n"
                    };
                    assert_eq!(std::fs::read(root.join(current)).unwrap(), expected);
                }
                assert_eq!(
                    source.reads.load(Ordering::SeqCst),
                    reads,
                    "resident-byte verification unexpectedly hydrated"
                );
                eprintln!(
                    "OFFLINE RENAME evidence {lifecycle}/{state}/{kind}: native={:?} identity={} on_disk={} modified={} metadata_source_delta=0; no export support claimed",
                    after.native_id,
                    FileId::from_bytes(&after.backing_identity),
                    after.on_disk,
                    after.modified
                );
                if export {
                    assert!(
                        renamed,
                        "export qualification requires an actual native rename"
                    );
                    let path = RepoPath::parse(current).unwrap();
                    let original_path = RepoPath::parse(original).unwrap();
                    let mut checkpoint = session.export_checkpoint().unwrap();
                    assert_eq!(source.reads.load(Ordering::SeqCst), reads);
                    let clean = RepoPath::parse("missing.bin").unwrap();
                    let clean_error = checkpoint
                        .with_reader(&clean, |_| -> Result<()> {
                            panic!("clean immutable export entry exposed a byte reader")
                        })
                        .unwrap_err();
                    assert!(matches!(
                        export_cause(&clean_error),
                        Error::Unsupported("only NeedsHash export entries may read content")
                    ));
                    assert_eq!(source.reads.load(Ordering::SeqCst), reads);
                    let actual = checkpoint
                        .entries()
                        .iter()
                        .find(|entry| entry.path == path)
                        .unwrap();
                    assert!(
                        matches!(&actual.evidence, CaptureEvidence::NeedsHash { base: Some(id), size } if id == &source.catalog().entry(&original_path).unwrap().file_id && *size == after.size)
                    );
                    assert!(matches!(
                        &checkpoint
                            .entries()
                            .iter()
                            .find(|entry| entry.path == original_path)
                            .unwrap()
                            .evidence,
                        CaptureEvidence::Deleted { .. }
                    ));
                    child(&root, "export-guard-refusals", current);
                    // Resident private bytes must not depend on source availability.
                    source
                        .deny_reads
                        .store(state != "partial", Ordering::SeqCst);
                    let expected = if state == "partial" {
                        pattern()
                    } else if state == "dirty" {
                        b"private notes!!\n".to_vec()
                    } else {
                        b"immutable notes\n".to_vec()
                    };
                    let observed = checkpoint
                        .with_reader(&path, |reader| {
                            let mut bytes = Vec::new();
                            reader.read_to_end(&mut bytes)?;
                            Ok(bytes)
                        })
                        .unwrap();
                    assert_eq!(observed, expected);
                    let short = checkpoint.with_reader(&path, |_| Ok(())).unwrap_err();
                    assert!(
                        matches!(short, Error::ExportConsumer { cause, .. } if matches!(*cause, Error::InvalidRange))
                    );
                    let disk_full = checkpoint
                        .with_reader(&path, |reader| -> Result<()> {
                            reader.read_exact(&mut [0u8; 1])?;
                            Err(std::io::Error::from_raw_os_error(
                                windows_sys::Win32::Foundation::ERROR_DISK_FULL as i32,
                            )
                            .into())
                        })
                        .unwrap_err();
                    assert!(
                        matches!(&disk_full, Error::ExportConsumer { path: actual, cause } if actual == &path && matches!(cause.as_ref(), Error::Io(error) if error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_DISK_FULL as i32)))
                    );
                    assert!(!disk_full.to_string().contains("backing"));
                    let mut output = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(case.join("exported-file"))
                        .unwrap();
                    checkpoint
                        .with_reader(&path, |reader| {
                            std::io::copy(reader, &mut output)?;
                            Ok(())
                        })
                        .unwrap();
                    output.sync_all().unwrap();
                    drop(output);
                    assert_eq!(std::fs::read(case.join("exported-file")).unwrap(), expected);
                    if state != "partial" {
                        assert_eq!(
                            source.reads.load(Ordering::SeqCst),
                            reads,
                            "resident export attempted source reads"
                        );
                    }
                    let after_reads = source.reads.load(Ordering::SeqCst);
                    checkpoint.revalidate_namespace().unwrap();
                    assert_eq!(source.reads.load(Ordering::SeqCst), after_reads);
                    assert!(matches!(
                        session.checkpoint(),
                        Err(Error::Unsupported(
                            "placeholder renamed: identity is preserved, capture reconciliation required"
                        ))
                    ));
                    assert!(matches!(
                        session.inspect(&path),
                        Err(Error::Unsupported(
                            "placeholder renamed: identity is preserved, capture reconciliation required"
                        ))
                    ));
                    let final_probe = placeholder_probe(&root.join(current));
                    assert_eq!(after.native_id, final_probe.native_id);
                    assert_eq!(after.backing_identity, final_probe.backing_identity);
                    eprintln!(
                        "EXPORT QUALIFIED {lifecycle}/{state}/{kind}: actual_path={current} hash={} bytes={} strict_capture_refused; resident_source_fault={}",
                        FileId::from_bytes(&expected),
                        expected.len(),
                        state != "partial"
                    );
                }
            }
        }
    }
}
