use super::*;
use crate::projection::provider::{
    records::ProviderSourceDescriptor, validation::tests::fixture::Fixture,
};
use layrs_core::{
    projection::{PathSemantics, ProjectionSelection},
    scm::{Chunk, ChunkId, FileChunk, RepoPath},
};
use layrs_vfs::CaptureEvidence;

fn source(writable: bool, selected: bool) -> ProviderBootstrapSource {
    let old = Fixture::new("G1", false).source;
    let mut initial = old.initial().clone();
    initial.entries[0].writable = writable;
    let selection = if selected {
        ProjectionSelection::All
    } else {
        ProjectionSelection::selected(vec![], vec![], PathSemantics::PortableWindows).unwrap()
    };
    let descriptors = if selected {
        old.descriptors()
            .iter()
            .map(|d| ProviderSourceDescriptor {
                path: d.path.clone(),
                manifest: d.manifest.clone(),
            })
            .collect()
    } else {
        vec![]
    };
    ProviderBootstrapSource::validate(initial, selection, descriptors).unwrap()
}
fn immutable(source: &ProviderBootstrapSource) -> Observed {
    let d = &source.descriptors()[0];
    Observed {
        path: d.path.clone(),
        original_path: Some(d.path.clone()),
        evidence: CaptureEvidence::Immutable {
            file_id: d.file_id.clone(),
            size: d.bytes,
            original_path: d.path.clone(),
        },
    }
}
fn manifest(bytes: &[u8]) -> FileManifest {
    FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(1024 * 1024).unwrap(),
        &[Chunk::new(bytes.to_vec()).unwrap()],
    )
    .unwrap()
}

#[test]
fn source_stream_rejects_canonical_false_chunk_with_correct_file_id() {
    let bytes = vec![b'x'; 70 * 1024];
    let authentic = manifest(&bytes);
    let false_chunk = FileChunk::new(
        ChunkId::parse(format!("blake3:{}", "0".repeat(64))).unwrap(),
        0,
        bytes.len() as u64,
    )
    .unwrap();
    let forged = FileManifest::new(
        authentic.file_id().clone(),
        authentic.byte_length(),
        authentic.chunking().clone(),
        vec![false_chunk],
    )
    .unwrap();
    let encoded = layrs_core::scm::canonical_json(&forged).unwrap();
    let forged: FileManifest = serde_json::from_slice(&encoded).unwrap();
    let mut read = |offset: u64, output: &mut [u8], _: &Cancellation| {
        let start = usize::try_from(offset).unwrap();
        output.copy_from_slice(&bytes[start..start + output.len()]);
        Ok(())
    };
    let cancel = Cancellation::default();
    assert_eq!(
        verify_source_manifest_stream(&authentic, &cancel, &mut read).unwrap(),
        authentic.file_id().clone()
    );
    assert!(verify_source_manifest_stream(&forged, &cancel, &mut read).is_err());
}

#[test]
fn borrowed_path_budget_refuses_before_cloning_or_hashing() {
    let short = RepoPath::parse("file.txt").unwrap();
    let cancel = Cancellation::default();
    assert!(bound_observation_paths(
        std::iter::repeat(&short).take(recovery_export::PATH_LIMIT + 1),
        &cancel,
    )
    .is_err());
    let long = RepoPath::parse(vec!["part"; 50].join("/")).unwrap();
    let excess = recovery_export::PATH_BYTES_LIMIT / long.as_str().len() + 1;
    assert!(excess < recovery_export::PATH_LIMIT);
    assert!(bound_observation_paths(std::iter::repeat(&long).take(excess), &cancel,).is_err());
}

#[test]
fn unchanged_protected_file_is_verified_and_modified_file_refused() {
    let source = source(false, true);
    let activated = activation_snapshot(&source).unwrap();
    let d = &source.descriptors()[0];
    let cancel = Cancellation::default();
    let image = interpret(
        &[immutable(&source)],
        &source,
        &activated,
        &cancel,
        |_, _| Ok(d.file_id.clone()),
        |_, _| panic!("immutable file cannot use native hash"),
    )
    .unwrap();
    assert_eq!(image.files.len(), 1);
    assert_eq!(image.files[0].manifest.manifest(), &d.manifest);
    assert_eq!(image.digest.len(), 64);
    let changed = manifest(b"changed protected bytes");
    let observation = Observed {
        path: d.path.clone(),
        original_path: Some(d.path.clone()),
        evidence: CaptureEvidence::NeedsHash {
            base: Some(d.file_id.clone()),
            size: changed.byte_length(),
        },
    };
    assert!(interpret(
        &[observation],
        &source,
        &activated,
        &cancel,
        |_, _| panic!("native file cannot use source hash"),
        |_, _| Ok(changed.clone()),
    )
    .is_err());
}

#[test]
fn writable_deletion_keeps_exact_base_while_protected_deletion_refuses() {
    for writable in [false, true] {
        let source = source(writable, true);
        let activated = activation_snapshot(&source).unwrap();
        let d = &source.descriptors()[0];
        let observation = Observed {
            path: d.path.clone(),
            original_path: None,
            evidence: CaptureEvidence::Deleted {
                base: d.file_id.clone(),
            },
        };
        let result = interpret(
            &[observation],
            &source,
            &activated,
            &Cancellation::default(),
            |_, _| panic!("deletion has no file bytes"),
            |_, _| panic!("deletion has no file bytes"),
        );
        if writable {
            let image = result.unwrap();
            assert!(image.files.is_empty());
            assert_eq!(image.deletions.len(), 1);
            assert_eq!(image.deletions[0].base, d.file_id);
            assert!(!image.deletions[0].executable);
        } else {
            assert!(result.is_err());
        }
    }
}

#[test]
fn unknown_and_excluded_refuse_before_any_hash_callback() {
    let selected = source(false, true);
    let activated = activation_snapshot(&selected).unwrap();
    let mut observations = vec![
        immutable(&selected),
        Observed {
            path: RepoPath::parse("untracked.txt").unwrap(),
            original_path: None,
            evidence: CaptureEvidence::NeedsHash {
                base: None,
                size: 3,
            },
        },
    ];
    observations.sort_by(|a, b| a.path.cmp(&b.path));
    assert!(interpret(
        &observations,
        &selected,
        &activated,
        &Cancellation::default(),
        |_, _| panic!("preflight must run first"),
        |_, _| panic!("preflight must run first"),
    )
    .is_err());

    let excluded = source(false, false);
    let activated = activation_snapshot(&excluded).unwrap();
    let observation = Observed {
        path: RepoPath::parse("README.md").unwrap(),
        original_path: None,
        evidence: CaptureEvidence::NeedsHash {
            base: None,
            size: 3,
        },
    };
    assert!(interpret(
        &[observation],
        &excluded,
        &activated,
        &Cancellation::default(),
        |_, _| panic!("excluded path cannot be hashed as managed"),
        |_, _| panic!("excluded path cannot be hashed as managed"),
    )
    .is_err());
}

#[test]
fn missing_source_and_cancellation_refuse_without_synthesized_content() {
    let readonly = source(false, true);
    let activated = activation_snapshot(&readonly).unwrap();
    let d = &readonly.descriptors()[0];
    let missing = interpret(
        &[immutable(&readonly)],
        &readonly,
        &activated,
        &Cancellation::default(),
        |_, _| {
            Err(layrs_vfs::Error::MissingBytes {
                file_id: d.file_id.clone(),
                offset: 0,
                length: usize::try_from(d.bytes).unwrap(),
            }
            .into())
        },
        |_, _| panic!("source miss cannot use native hash"),
    );
    assert!(matches!(
        missing,
        Err(
            crate::projection::provider::recovery::error::ProviderBootstrapCause::Vfs(
                layrs_vfs::Error::MissingBytes { .. }
            )
        )
    ));
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert!(interpret(
        &[immutable(&readonly)],
        &readonly,
        &activated,
        &cancelled,
        |_, _| panic!("cancel before hash"),
        |_, _| panic!("cancel before hash"),
    )
    .is_err());

    let writable = source(true, true);
    let activated = activation_snapshot(&writable).unwrap();
    let d = &writable.descriptors()[0];
    let changed = manifest(b"writable replacement");
    let observation = Observed {
        path: d.path.clone(),
        original_path: Some(d.path.clone()),
        evidence: CaptureEvidence::NeedsHash {
            base: Some(d.file_id.clone()),
            size: changed.byte_length(),
        },
    };
    let token = Cancellation::default();
    let signal = token.clone();
    assert!(interpret(
        &[observation],
        &writable,
        &activated,
        &token,
        |_, _| panic!("native path cannot use source hash"),
        |_, _| {
            signal.cancel();
            Ok(changed.clone())
        },
    )
    .is_err());
}
