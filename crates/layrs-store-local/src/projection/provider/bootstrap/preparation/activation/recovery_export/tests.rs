use super::*;
use crate::projection::provider::validation::tests::fixture::Fixture;

fn source() -> ProviderBootstrapSource {
    Fixture::new("G1", false).source
}

#[test]
fn unknown_ordinary_file_is_recovery_only_without_capture_authority() {
    let source = source();
    let activated =
        crate::projection::provider::records::activation::activation_snapshot(&source).unwrap();
    let path = RepoPath::parse("untracked.txt").unwrap();
    let mut observations: Vec<_> = source
        .descriptors()
        .iter()
        .map(|descriptor| Observed {
            path: descriptor.path.clone(),
            original_path: Some(descriptor.path.clone()),
            evidence: CaptureEvidence::Immutable {
                file_id: descriptor.file_id.clone(),
                size: descriptor.bytes,
                original_path: descriptor.path.clone(),
            },
        })
        .collect();
    observations.push(Observed {
        path: path.clone(),
        original_path: None,
        evidence: CaptureEvidence::NeedsHash {
            base: None,
            size: 3,
        },
    });
    observations.sort_by(|a, b| a.path.cmp(&b.path));
    let planned = plan(&observations, &source, &activated).unwrap();
    let ordinary = planned.files.iter().find(|f| f.path == path).unwrap();
    assert_eq!(
        ordinary.class,
        ProviderRecoveryExportClass::RecoveryOnlyUnknown
    );
    assert_eq!(ordinary.reader, ReaderKind::Native);
}

#[test]
fn missing_source_recognition_is_typed() {
    let missing = layrs_vfs::Error::MissingBytes {
        file_id: FileId::from_bytes(b"cold"),
        offset: 0,
        length: 3,
    };
    assert!(source_missing(&missing));
    let wrapped = layrs_vfs::Error::ExportRead {
        path: RepoPath::parse("cold.txt").unwrap(),
        backing: None,
        cause: Box::new(missing),
    };
    assert!(source_missing(&wrapped));
    assert!(!source_missing(&layrs_vfs::Error::IdentityMismatch));
}

#[test]
fn size_budget_rejects_excess_before_hash() {
    assert!(add_size(0, FILE_LIMIT + 1).is_err());
    assert!(add_size(TOTAL_LIMIT, 1).is_err());
}

#[test]
fn protected_file_is_candidate_only_when_verified_target_and_mode_match() {
    let base = FileId::from_bytes(b"base");
    let changed = FileId::from_bytes(b"changed");
    let planned = PlannedFile {
        path: RepoPath::parse("README.md").unwrap(),
        backing: Some(base.clone()),
        size: 4,
        executable: Some(false),
        class: ProviderRecoveryExportClass::ManagedCandidate,
        expected: Some((base.clone(), false)),
        writable: false,
        reader: ReaderKind::Native,
    };
    assert_eq!(
        verified_class(&planned, &base),
        ProviderRecoveryExportClass::ManagedCandidate
    );
    assert_eq!(
        verified_class(&planned, &changed),
        ProviderRecoveryExportClass::RecoveryOnlyProtected
    );
    let changed_mode = PlannedFile {
        executable: Some(true),
        ..planned
    };
    assert_eq!(
        verified_class(&changed_mode, &base),
        ProviderRecoveryExportClass::RecoveryOnlyProtected
    );
}

#[test]
fn equal_file_id_relocation_uses_original_path() {
    use crate::projection::provider::records::ProviderSourceDescriptor;
    let old = source();
    let readme = &old.descriptors()[0];
    let other_path = RepoPath::parse("other.txt").unwrap();
    let mut initial = old.initial().clone();
    let mut second = initial.entries[0].clone();
    second.path = other_path.clone();
    initial.entries.push(second);
    initial.entries.sort_by(|a, b| a.path.cmp(&b.path));
    let source = ProviderBootstrapSource::validate(
        initial,
        old.selection().clone(),
        vec![
            ProviderSourceDescriptor {
                path: readme.path.clone(),
                manifest: readme.manifest.clone(),
            },
            ProviderSourceDescriptor {
                path: other_path.clone(),
                manifest: readme.manifest.clone(),
            },
        ],
    )
    .unwrap();
    let activated =
        crate::projection::provider::records::activation::activation_snapshot(&source).unwrap();
    let base = readme.file_id.clone();
    let observations = [
        Observed {
            path: readme.path.clone(),
            original_path: Some(readme.path.clone()),
            evidence: CaptureEvidence::Immutable {
                file_id: base.clone(),
                size: readme.bytes,
                original_path: readme.path.clone(),
            },
        },
        Observed {
            path: other_path.clone(),
            original_path: Some(readme.path.clone()),
            evidence: CaptureEvidence::NeedsHash {
                base: Some(base.clone()),
                size: readme.bytes,
            },
        },
    ];
    let planned = plan(&observations, &source, &activated).unwrap();
    let moved = planned
        .files
        .iter()
        .find(|file| file.path == other_path)
        .unwrap();
    assert_eq!(
        moved.class,
        ProviderRecoveryExportClass::RecoveryOnlyRelocated
    );
    let same_path = [
        observations[0].clone(),
        Observed {
            path: other_path.clone(),
            original_path: Some(other_path.clone()),
            evidence: CaptureEvidence::NeedsHash {
                base: Some(base),
                size: readme.bytes,
            },
        },
    ];
    let planned = plan(&same_path, &source, &activated).unwrap();
    let same = planned
        .files
        .iter()
        .find(|file| file.path == other_path)
        .unwrap();
    assert_eq!(same.class, ProviderRecoveryExportClass::ManagedCandidate);
}

#[test]
fn ordinary_replacement_at_catalog_path_uses_path_bound_base() {
    let source = source();
    let activated =
        crate::projection::provider::records::activation::activation_snapshot(&source).unwrap();
    let descriptor = &source.descriptors()[0];
    let observations = [Observed {
        path: descriptor.path.clone(),
        original_path: None,
        evidence: CaptureEvidence::NeedsHash {
            base: Some(descriptor.file_id.clone()),
            size: 7,
        },
    }];
    let planned = plan(&observations, &source, &activated).unwrap();
    assert_eq!(
        planned.files[0].class,
        ProviderRecoveryExportClass::ManagedCandidate
    );
    assert_eq!(planned.files[0].reader, ReaderKind::Native);
}
