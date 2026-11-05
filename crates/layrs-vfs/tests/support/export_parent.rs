//! Retained export destination identity, including the gap after a point probe.
use super::{
    compatibility::{raw_tag, Disguise},
    disposable_run, native_file_id, Fixture,
};
use layrs_vfs::{Error, Result, Session};
use std::{
    io::Write,
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{atomic::Ordering, Arc},
};
use windows_sys::Win32::Storage::FileSystem::*;

fn identity(path: &Path) -> String {
    let (volume, index) = native_file_id(path);
    format!("fs:v1:{volume:016x}:{index:016x}")
}

fn persist(path: PathBuf) -> impl FnOnce(&layrs_vfs::Registration) -> Result<()> {
    move |ticket| {
        let mut file = std::fs::File::create(path)?;
        file.write_all(&ticket.encode())?;
        file.sync_all()?;
        Ok(())
    }
}

fn assert_delete_access_refused(path: &Path) {
    let result = std::fs::OpenOptions::new()
        .access_mode(DELETE | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path);
    assert_eq!(
        result.unwrap_err().raw_os_error(),
        Some(32),
        "DELETE {}",
        path.display()
    );
}

fn short_alias(path: &Path) -> Option<PathBuf> {
    let wide: Vec<u16> = format!("\\\\?\\{}", path.display())
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut output = vec![0u16; 32_768];
    let length =
        unsafe { GetShortPathNameW(wide.as_ptr(), output.as_mut_ptr(), output.len() as u32) };
    if length == 0 {
        eprintln!(
            "SHORT-NAME ARM UNAVAILABLE: GetShortPathNameW {}; no volume settings changed",
            std::io::Error::last_os_error()
        );
        return None;
    }
    assert!(
        (length as usize) < output.len(),
        "short-name result truncated"
    );
    let value = String::from_utf16(&output[..length as usize]).unwrap();
    let value = value.strip_prefix("\\\\?\\").unwrap_or(&value);
    if !value.contains('~') || value.eq_ignore_ascii_case(path.to_str().unwrap()) {
        eprintln!("SHORT-NAME ARM UNAVAILABLE: no genuinely distinct 8.3 alias; no volume settings changed");
        return None;
    }
    Some(PathBuf::from(value))
}

pub(super) fn run() {
    let (run, nonce) = disposable_run("export-parent-guard");
    let root = run.join("provider");
    std::fs::create_dir(&root).unwrap();
    let source = Arc::new(Fixture::new());
    source.deny_reads.store(true, Ordering::SeqCst);
    let mut session = Session::create_with_registration_hook(
        &root,
        nonce.to_le_bytes(),
        source.clone(),
        persist(run.join("provider.ticket")),
    )
    .unwrap();
    session.disconnect().unwrap();

    let ancestor = run.join("guarded-ancestor");
    let parent = ancestor.join("guarded-parent");
    let leaf = parent.join("leaf");
    std::fs::create_dir_all(&leaf).unwrap();
    {
        let disguise = Disguise::enter();
        assert!(!session.is_owned_root_directory(&leaf).unwrap());
        let guard = session.guard_export_parent(&leaf).unwrap();
        assert_eq!(guard.root(), leaf.as_path());
        assert_eq!(guard.filesystem_identity(), identity(&leaf));
        for (index, path) in [&ancestor, &parent, &leaf].into_iter().enumerate() {
            assert_delete_access_refused(path);
            let moved = path.with_file_name(format!("rename-must-fail-{index}"));
            assert_eq!(
                std::fs::rename(path, moved).unwrap_err().raw_os_error(),
                Some(32)
            );
        }
        assert_eq!(
            std::fs::remove_dir(&leaf).unwrap_err().raw_os_error(),
            Some(32)
        );
        guard.revalidate().unwrap();
        disguise.assert_preserved();
        assert_eq!(source.reads.load(Ordering::SeqCst), 0);
        eprintln!("RETAINED EXPORT PARENT: three DELETE opens + three renames + empty-leaf deletion refused32; exact identity revalidated; source_reads0");
    }
    // Release really ends the sharing protection; retain the renamed fixture.
    let released = parent.join("released-leaf");
    std::fs::rename(&leaf, &released).unwrap();

    let volume = root.ancestors().last().unwrap();
    let volume_guard = session.guard_export_parent(volume).unwrap();
    assert_eq!(volume_guard.filesystem_identity(), identity(volume));
    volume_guard.revalidate().unwrap();
    drop(volume_guard);
    assert!(session.is_owned_root_directory(&root).unwrap());
    assert!(matches!(
        session.guard_export_parent(&root),
        Err(Error::Unsupported(
            "export parent must be outside the provider checkout"
        ))
    ));
    assert!(matches!(
        session.guard_export_parent(&root.join("nested")),
        Err(Error::Unsupported(
            "export parent must be outside the provider checkout"
        ))
    ));

    let unicode = run.join("École destination");
    let unicode_alias = run.join("éCOLE DESTINATION");
    std::fs::create_dir(&unicode).unwrap();
    assert_eq!(identity(&unicode), identity(&unicode_alias));
    assert!(!session.is_owned_root_directory(&unicode_alias).unwrap());
    let guard = session.guard_export_parent(&unicode_alias).unwrap();
    assert_eq!(guard.root(), unicode_alias.as_path());
    assert_eq!(guard.filesystem_identity(), identity(&unicode));
    guard.revalidate().unwrap();
    drop(guard);
    eprintln!(
        "UNICODE EXPORT PARENT: NTFS case alias resolves to identical retained native identity"
    );

    let long = run.join("owned long export parent for short alias");
    std::fs::create_dir(&long).unwrap();
    if let Some(short) = short_alias(&long) {
        assert_eq!(identity(&long), identity(&short));
        assert!(!session.is_owned_root_directory(&short).unwrap());
        let guard = session.guard_export_parent(&short).unwrap();
        assert_eq!(guard.filesystem_identity(), identity(&long));
        guard.revalidate().unwrap();
        eprintln!(
            "SHORT-NAME ARM QUALIFIED: {} -> same retained native identity",
            short.display()
        );
    }

    // Deterministic ordinary replacement after the bool probe: only the actual
    // newly acquired identity is returned. A consumer's older pin must mismatch.
    let replace = run.join("ordinary-replacement");
    std::fs::create_dir(&replace).unwrap();
    assert!(!session.is_owned_root_directory(&replace).unwrap());
    let previous = identity(&replace);
    std::fs::rename(&replace, run.join("retained-old-ordinary")).unwrap();
    std::fs::create_dir(&replace).unwrap();
    let guard = session.guard_export_parent(&replace).unwrap();
    assert_ne!(guard.filesystem_identity(), previous);
    assert_eq!(guard.filesystem_identity(), identity(&replace));
    guard.revalidate().unwrap();
    drop(guard);

    // Deterministic foreign replacement in the same gap. Register only a fresh
    // empty disposable directory, retaining the original directory and ticket.
    let replace = run.join("foreign-replacement");
    std::fs::create_dir(&replace).unwrap();
    assert!(!session.is_owned_root_directory(&replace).unwrap());
    let previous = identity(&replace);
    std::fs::rename(&replace, run.join("retained-old-before-foreign")).unwrap();
    std::fs::create_dir(&replace).unwrap();
    let mut foreign = Session::create_with_registration_hook(
        &replace,
        (nonce + 1).to_le_bytes(),
        source.clone(),
        persist(run.join("foreign.ticket")),
    )
    .unwrap();
    foreign.disconnect().unwrap();
    assert_ne!(identity(&replace), previous);
    {
        let disguise = Disguise::enter();
        let attrs = raw_tag(&replace);
        assert_eq!(
            attrs.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT,
            0,
            "foreign-root disguise control"
        );
        for path in [&replace, &replace.join("never-open-this-child")] {
            assert!(matches!(
                session.guard_export_parent(path),
                Err(Error::Unsupported("export parent contains a reparse point"))
            ));
            assert!(matches!(
                session.is_owned_root_directory(path),
                Err(Error::Unsupported(
                    "owned-root probe refuses a foreign or unsupported reparse point"
                ))
            ));
            disguise.assert_preserved();
        }
    }
    for alias in [
        "relative",
        r"C:relative",
        r"\\?\C:\",
        r"\\server\share",
        r"C:\.",
        r"C:\..",
        r"C:\bad:stream",
    ] {
        assert!(session.guard_export_parent(Path::new(alias)).is_err());
    }
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);
    eprintln!("EXPORT PARENT RACE: initial probe -> foreign/disguised replacement refused at leaf/ancestor; ordinary replacement bound only to newly acquired ID; volume/sibling/Unicode verified; all fixtures retained; source_reads0");
}
