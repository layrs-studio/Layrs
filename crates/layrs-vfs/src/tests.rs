use crate::*;
use layrs_core::scm::{FileId, RepoPath};

fn entry(path: &str, bytes: &[u8]) -> CatalogEntry {
    CatalogEntry {
        path: RepoPath::parse(path).unwrap(),
        file_id: FileId::from_bytes(bytes),
        size: bytes.len() as u64,
    }
}

#[test]
fn catalog_rejects_windows_aliases_before_registration() {
    for path in [
        "CON",
        "dir/NUL.txt",
        "x:stream",
        "trailing.",
        "trailing ",
        "SHORT~1",
        "a?",
        "a*",
        "aux.rs",
        "LPT1.txt",
        "COM9",
        "caf\u{e9}",
    ] {
        if let Ok(path_value) = RepoPath::parse(path) {
            let candidate = CatalogEntry {
                path: path_value,
                file_id: FileId::from_bytes(b"a"),
                size: 1,
            };
            assert!(Catalog::new(1, vec![candidate]).is_err(), "{path}");
        }
    }
    for names in [["a", "A"], ["a", "a/b"], ["dir/a", "DIR/b"], ["a", "a"]] {
        assert!(
            Catalog::new(1, names.iter().map(|name| entry(name, b"a")).collect()).is_err(),
            "{names:?}"
        );
    }
}

#[test]
fn provider_reserves_metadata_components_beyond_repo_path_syntax() {
    for name in [
        ".layrs",
        ".LAYRS",
        ".layrs/FORMAT",
        "nested/.LaYrS",
        "nested/.layrs/data",
        ".git",
        "nested/.GIT/config",
    ] {
        // Core deliberately accepts these as syntactically valid RepoPaths.
        let path = RepoPath::parse(name).unwrap();
        assert!(
            crate::catalog::validate_path(&path).is_err(),
            "accepted {name}"
        );
        assert!(
            Catalog::new(1, vec![entry(name, b"")]).is_err(),
            "admitted {name}"
        );
    }
    // This is an exact metadata-name fence, not a prefix-based exclusion.
    for name in [
        ".gitignore",
        ".layrs-backup",
        "nested/.layrs-copy",
        "notes.layrs",
    ] {
        assert!(
            Catalog::new(1, vec![entry(name, b"")]).is_ok(),
            "rejected {name}"
        );
    }
}

#[test]
fn catalog_is_versioned_order_independent_and_size_consistent() {
    let a = entry("dir/a", b"abc");
    let b = entry("b", b"de");
    let one = Catalog::new(1, vec![a.clone(), b.clone()]).unwrap();
    let reordered = Catalog::new(1, vec![b.clone(), a.clone()]).unwrap();
    assert_eq!(one.identity(), reordered.identity());
    assert_ne!(
        one.identity(),
        Catalog::new(2, vec![a.clone(), b]).unwrap().identity()
    );
    assert_eq!(
        one.directories().iter().cloned().collect::<Vec<_>>(),
        vec!["dir"]
    );
    let mut bad = a.clone();
    bad.path = RepoPath::parse("other").unwrap();
    bad.size += 1;
    assert!(Catalog::new(1, vec![a, bad]).is_err());
}

#[test]
fn placeholder_identity_tracks_original_entry_not_current_name() {
    let a = entry("a", b"same");
    let b = entry("b", b"same");
    let catalog = Catalog::new(1, vec![a.clone(), b]).unwrap();
    let identity = catalog.file_identity(0);
    assert_eq!(catalog.resolve_identity(&identity).unwrap(), &a);
    assert_ne!(identity, catalog.file_identity(1));
    let mut tampered = identity.clone();
    tampered[161] ^= 1;
    assert!(catalog.resolve_identity(&tampered).is_err());
    assert!(catalog.resolve_identity(&identity[..161]).is_err());
    let other = Catalog::new(2, catalog.entries().to_vec()).unwrap();
    assert!(other.resolve_identity(&identity).is_err());
}

#[test]
fn checked_bounds_and_aligned_bounded_transfers() {
    assert_eq!(checked_range(0, 0, 0).unwrap(), 0..0);
    assert!(checked_range(1, u64::MAX, 2).is_err());
    assert!(checked_range(10, 8, 3).is_err());
    assert!(HydrationRange::new(10, -1, 1).is_err());
    assert!(HydrationRange::new(10, 0, 0).is_err());
    assert_eq!(
        HydrationRange::new(9001, 4100, 2)
            .unwrap()
            .collect::<Vec<_>>(),
        vec![4096..8192]
    );
    assert_eq!(
        HydrationRange::new(9001, 8192, -1)
            .unwrap()
            .collect::<Vec<_>>(),
        vec![8192..9001]
    );
    assert!(HydrationRange::new(9001, 9002, -1).is_err());
    for size in [1, 4095, 4096, 4097, 1_048_579, i64::MAX as u64] {
        let start = size.saturating_sub(600_001);
        let ranges = HydrationRange::new(size, start as i64, (size - start) as i64)
            .unwrap()
            .collect::<Vec<_>>();
        assert_eq!(ranges.last().unwrap().end, size);
        for range in ranges {
            assert_eq!(range.start % 4096, 0);
            assert!(range.end == size || range.end % 4096 == 0);
            assert!(range.end - range.start <= READ_BUFFER_SIZE as u64);
        }
    }
}

#[test]
fn only_clean_partial_same_size_can_reuse_an_immutable_id() {
    let entry = entry("a", b"abc");
    let info = PlaceholderEvidence {
        on_disk_bytes: 0,
        modified_bytes: 0,
        in_sync: true,
        partial: true,
        file_id: 4,
        sync_root_file_id: 5,
    };
    assert!(matches!(
        classify_placeholder(&entry, 3, &info),
        CaptureEvidence::Immutable { .. }
    ));
    for info in [
        PlaceholderEvidence {
            modified_bytes: 1,
            ..info.clone()
        },
        PlaceholderEvidence {
            in_sync: false,
            ..info.clone()
        },
        PlaceholderEvidence {
            partial: false,
            ..info.clone()
        },
    ] {
        assert!(matches!(
            classify_placeholder(&entry, 3, &info),
            CaptureEvidence::NeedsHash { .. }
        ));
    }
    assert!(matches!(
        classify_placeholder(&entry, 2, &info),
        CaptureEvidence::NeedsHash { .. }
    ));
}

#[test]
fn cancellation_and_missing_data_never_turn_into_success() {
    struct Missing(Catalog);
    impl SnapshotSource for Missing {
        fn catalog(&self) -> &Catalog {
            &self.0
        }
        fn read_exact_at(
            &self,
            id: &FileId,
            offset: u64,
            buffer: &mut [u8],
            _: &Cancellation,
        ) -> Result<()> {
            Err(Error::MissingBytes {
                file_id: id.clone(),
                offset,
                length: buffer.len(),
            })
        }
    }
    let entry = entry("a", b"abc");
    let source = Missing(Catalog::new(1, vec![entry.clone()]).unwrap());
    let mut buffer = [0xa5; 3];
    let cancel = Cancellation::default();
    assert!(matches!(
        crate::catalog::read_checked(&source, &entry, 0, &mut buffer, &cancel),
        Err(Error::MissingBytes { .. })
    ));
    assert_eq!(buffer, [0xa5; 3]);
    cancel.cancel();
    assert!(matches!(
        crate::catalog::read_checked(&source, &entry, 0, &mut buffer, &cancel),
        Err(Error::Cancelled)
    ));
}

#[test]
fn registration_ticket_is_bounded_versioned_and_roundtrips() {
    let ticket = Registration {
        root: "C:\\disposable\\root".into(),
        root_id: [7; 16],
        catalog_id: FileId::from_bytes(b"catalog"),
        volume: 9,
        file_index: 11,
        flavor: crate::registration::RegistrationFlavor::Legacy,
    };
    let decoded = Registration::decode(&ticket.encode()).unwrap();
    assert_eq!(decoded.encode(), ticket.encode());
    assert_eq!(
        ticket.filesystem_identity(),
        "fs:v1:0000000000000009:000000000000000b"
    );
    assert_ne!(ticket.root_blob(false), ticket.root_blob(true));
    for bytes in [vec![], vec![0; 100], vec![0; 33_000]] {
        assert!(Registration::decode(&bytes).is_err());
    }
}

#[test]
fn control_capability_binds_root_native_file_and_content() {
    let control = ControlFileIdentity {
        volume: 1,
        root_index: 2,
        file_index: 3,
        size: 7,
        content: FileId::from_bytes(b"locator"),
    };
    assert_eq!(
        ControlFileIdentity::decode(&control.encode()).unwrap(),
        control
    );
    for bytes in [vec![], vec![0; 103], control.encode()[..102].to_vec()] {
        assert!(ControlFileIdentity::decode(&bytes).is_err());
    }
    let mut changed = control.clone();
    changed.file_index += 1;
    assert_ne!(control.encode(), changed.encode());
    changed = control.clone();
    changed.content = FileId::from_bytes(b"another");
    assert_ne!(control.encode(), changed.encode());
}

#[test]
fn exact_namespace_budget_has_one_separately_verified_control_slot() {
    use crate::catalog::{MAX_ENTRIES, NamespaceBudget};
    for control_position in [0, MAX_ENTRIES / 2, MAX_ENTRIES] {
        let mut budget = NamespaceBudget::default();
        for position in 0..=MAX_ENTRIES {
            if position == control_position {
                budget.verified_control_entry().unwrap();
            } else {
                budget.user_entry().unwrap();
            }
        }
        assert!(budget.user_entry().is_err());
        assert!(budget.verified_control_entry().is_err());
    }
    // An unverified control-looking name is an ordinary counted entry; the
    // filesystem adapter must additionally reject reserved/foreign paths.
    let mut budget = NamespaceBudget::default();
    for _ in 0..MAX_ENTRIES {
        budget.user_entry().unwrap();
    }
    assert!(budget.user_entry().is_err());
}

#[test]
fn catalog_limit_counts_user_descriptors_and_derived_directories_not_root() {
    use crate::catalog::MAX_ENTRIES;
    let id = FileId::from_bytes(b"");
    let mut entries: Vec<_> = (0..MAX_ENTRIES)
        .map(|index| CatalogEntry {
            path: RepoPath::parse(format!("f{index:06}")).unwrap(),
            file_id: id.clone(),
            size: 0,
        })
        .collect();
    let catalog = Catalog::new(1, entries.clone()).unwrap();
    assert_eq!(catalog.entries().len(), MAX_ENTRIES);
    assert!(catalog.directories().is_empty());
    drop(catalog);
    entries.push(entry("extra", b""));
    assert!(Catalog::new(1, entries.clone()).is_err());
    entries.pop();
    // One additional directory consumes exactly one user namespace slot.
    entries[0].path = RepoPath::parse("dir/file").unwrap();
    assert!(Catalog::new(1, entries.clone()).is_err());
    entries.pop();
    let catalog = Catalog::new(1, entries).unwrap();
    assert_eq!(
        catalog.entries().len() + catalog.directories().len(),
        MAX_ENTRIES
    );
}
