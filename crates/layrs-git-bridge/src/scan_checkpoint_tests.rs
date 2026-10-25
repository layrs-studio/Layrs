use super::*;
use sha2::{Digest, Sha256};
const MAGIC_LEN: usize = b"LAYRS-GIT-SCAN\0\x01".len();
const HEADER_START: usize = MAGIC_LEN + 8;

fn fixture(temp: &Temp, format: &str) -> (PathBuf, process::Git) {
    let source = temp.directory("source");
    let git = process::Git::new(&source, GitBridgeLimits::default()).unwrap();
    git.run(
        &[
            "init",
            "--bare",
            "--template=",
            &format!("--object-format={format}"),
            ".",
        ],
        None,
        4096,
    )
    .unwrap();
    let tree_id = tree(
        &git,
        &[
            (b"binary", &[0, 255, 1], "100644"),
            (b"hello", b"base\n", "100755"),
        ],
    );
    let first = commit(&git, &tree_id, &[], "first");
    let last_tree = tree(
        &git,
        &[
            (b"binary", &[0, 255, 2], "100644"),
            (b"hello", b"last\n", "100755"),
        ],
    );
    let last = commit(&git, &last_tree, &[&first], "last");
    reference(&git, "refs/heads/main", &last);
    let tag = git.write_object("tag", format!("object {first}\ntype commit\ntag base\ntagger Example <example@example.invalid> 1000000002 +0000\n\nannotation\n").as_bytes()).unwrap();
    reference(&git, "refs/tags/base", &tag);
    (source, git)
}

#[test]
fn checkpoint_parent_replacement_during_acquisition_cannot_redirect_publication() {
    let temp = Temp::new();
    let (source, _) = fixture(&temp, "sha1");
    let parent = temp.directory("checkpoint-parent");
    let moved = temp.0.join("original-checkpoint-parent");
    let mut attempted = false;
    let mut replaced = false;
    let outcome = prepare_git_scan_checkpoint(
        &source,
        parent.join("scan"),
        &ImportSelection::AllLocalReferences,
        GitBridgeLimits::default(),
        &mut |_| {
            if !attempted {
                attempted = true;
                match std::fs::rename(&parent, &moved) {
                    Ok(()) => {
                        replaced = true;
                        std::fs::create_dir(&parent).unwrap();
                    }
                    Err(error) => {
                        assert!(cfg!(windows));
                        assert!(matches!(error.raw_os_error(), Some(5 | 32)));
                    }
                }
            }
            true
        },
    );
    assert!(attempted);
    if replaced {
        assert!(outcome.is_err());
        assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(&moved).unwrap().count(), 0);
    } else {
        let checkpoint = outcome.unwrap();
        resume_git_scan_checkpoint(
            &checkpoint.path,
            &checkpoint.digest,
            GitBridgeLimits::default(),
            &mut |_| true,
        )
        .unwrap();
    }
}

#[test]
fn checkpoint_publication_race_preserves_existing_destination() {
    let temp = Temp::new();
    let (source, _) = fixture(&temp, "sha1");
    let destination = temp.0.join("raced.scan");
    let mut written = false;
    assert!(
        prepare_git_scan_checkpoint(
            &source,
            &destination,
            &ImportSelection::AllLocalReferences,
            GitBridgeLimits::default(),
            &mut |_| {
                if !written {
                    std::fs::write(&destination, b"user collision").unwrap();
                    written = true;
                }
                true
            }
        )
        .is_err()
    );
    assert!(written);
    assert_eq!(std::fs::read(destination).unwrap(), b"user collision");
}

#[test]
fn checkpoint_larger_than_serialization_buffer_preserves_exact_plan_and_trailer() {
    let temp = Temp::new();
    let (source, git) = fixture(&temp, "sha1");
    let bytes: Vec<u8> = (0..1024 * 1024 + 37).map(|i| (i % 251) as u8).collect();
    let tree_id = tree(
        &git,
        &[
            (b"large.bin", &bytes, "100644"),
            (b"tail.txt", b"after buffer boundary\n", "100644"),
        ],
    );
    let commit_id = commit(&git, &tree_id, &[], "buffer boundary");
    reference(&git, "refs/heads/large", &commit_id);
    let direct = scan_git(&source, GitBridgeLimits::default()).unwrap();
    let checkpoint = prepare_git_scan_checkpoint(
        &source,
        temp.0.join("buffered.scan"),
        &ImportSelection::AllLocalReferences,
        GitBridgeLimits::default(),
        &mut |_| true,
    )
    .unwrap();
    assert!(std::fs::metadata(&checkpoint.path).unwrap().len() > 1024 * 1024);
    let resumed = resume_git_scan_checkpoint(
        &checkpoint.path,
        &checkpoint.digest,
        GitBridgeLimits::default(),
        &mut |_| true,
    )
    .unwrap();
    assert_eq!(direct.canonical_objects(), resumed.canonical_objects());
    assert_eq!(
        canonical_json(direct.report()).unwrap(),
        canonical_json(resumed.report()).unwrap()
    );
}

#[test]
fn frozen_scan_resumes_without_source_or_git_and_keeps_exact_plan_and_import_identity() {
    for format in ["sha1", "sha256"] {
        let temp = Temp::new();
        let (source, _) = fixture(&temp, format);
        let direct = scan_git(&source, GitBridgeLimits::default()).unwrap();
        let checkpoint = prepare_git_scan_checkpoint(
            &source,
            temp.0.join("frozen.scan"),
            &ImportSelection::AllLocalReferences,
            GitBridgeLimits::default(),
            &mut |_| true,
        )
        .unwrap();
        let unavailable = temp.0.join("unavailable-source");
        assert!(source.starts_with(&temp.0) && unavailable.starts_with(&temp.0));
        std::fs::rename(&source, &unavailable).unwrap();
        assert!(!source.exists());
        assert!(matches!(
            resume_git_scan_checkpoint(
                &checkpoint.path,
                &checkpoint.digest,
                GitBridgeLimits::default(),
                &mut |event| event.completed < 1
            ),
            Err(BridgeError::Cancelled)
        ));
        let mut events = Vec::new();
        let resumed = resume_git_scan_checkpoint(
            &checkpoint.path,
            &checkpoint.digest,
            GitBridgeLimits::default(),
            &mut |event| {
                events.push(event.clone());
                true
            },
        )
        .unwrap();
        assert!(!events.is_empty());
        assert!(events.iter().all(|event| event.git_processes == 0));
        assert_eq!(
            canonical_json(direct.report()).unwrap(),
            canonical_json(resumed.report()).unwrap()
        );
        assert_eq!(direct.canonical_objects(), resumed.canonical_objects());
        let native = NativeWorkspace::initialize(temp.directory("native"), "Frozen scan").unwrap();
        let first = resumed.install(&native).unwrap();
        assert!(!first.already_imported);
        assert!(resumed.install(&native).unwrap().already_imported);
        let reread = resume_git_scan_checkpoint(
            &checkpoint.path,
            &checkpoint.digest,
            GitBridgeLimits::default(),
            &mut |_| true,
        )
        .unwrap();
        assert!(reread.install(&native).unwrap().already_imported);
    }
}

#[test]
fn checkpoint_rejects_wrong_pin_truncation_corruption_counts_and_inconsistent_graph_even_when_resealed()
 {
    let temp = Temp::new();
    let (source, _) = fixture(&temp, "sha1");
    let checkpoint = prepare_git_scan_checkpoint(
        &source,
        temp.0.join("frozen.scan"),
        &ImportSelection::AllLocalReferences,
        GitBridgeLimits::default(),
        &mut |_| true,
    )
    .unwrap();
    let original = std::fs::read(&checkpoint.path).unwrap();
    assert!(
        resume_git_scan_checkpoint(
            &checkpoint.path,
            &format!("sha256:{}", "0".repeat(64)),
            GitBridgeLimits::default(),
            &mut |_| true
        )
        .is_err()
    );
    let bad = temp.0.join("bad.scan");
    let assert_bad = |bytes: &[u8], digest: &str| {
        std::fs::write(&bad, bytes).unwrap();
        assert!(
            resume_git_scan_checkpoint(&bad, digest, GitBridgeLimits::default(), &mut |_| true)
                .is_err()
        );
    };
    for length in [0, 8, original.len() - 33, original.len() - 1] {
        assert_bad(&original[..length], &checkpoint.digest);
    }
    let mut corrupt = original.clone();
    corrupt[25] ^= 1;
    assert_bad(&corrupt, &checkpoint.digest);
    let mut extra = original.clone();
    extra.push(0);
    assert_bad(&extra, &checkpoint.digest);
    for mutate in [
        |meta: &mut serde_json::Value| {
            meta["commits"].as_array_mut().unwrap().reverse();
        },
        |meta: &mut serde_json::Value| {
            meta["references"][0][1] = serde_json::json!("0".repeat(40));
        },
        |meta: &mut serde_json::Value| {
            meta["references"][0][0] = serde_json::json!("refs/heads/../bad");
        },
        |meta: &mut serde_json::Value| {
            meta["tag_bytes"] = serde_json::json!(0);
        },
    ] {
        let (bytes, digest) = change_header(&original, mutate);
        assert_bad(&bytes, &digest);
    }
    // Recomputing the envelope pin cannot hide wrong raw Git object content.
    let mut corrupt = original.clone();
    let count_offset = header_end(&corrupt);
    let first = count_offset + 8;
    let content = first + 2 + corrupt[first + 1] as usize + 8;
    corrupt[content] ^= 1;
    let digest = reseal(&mut corrupt);
    assert_bad(&corrupt, &digest);
    let mut huge = original.clone();
    huge[MAGIC_LEN..HEADER_START].copy_from_slice(&u64::MAX.to_be_bytes());
    let digest = reseal(&mut huge);
    assert_bad(&huge, &digest);
    assert!(
        resume_git_scan_checkpoint(
            &checkpoint.path,
            &checkpoint.digest,
            GitBridgeLimits {
                max_objects: 1,
                ..GitBridgeLimits::default()
            },
            &mut |_| true
        )
        .is_err()
    );
    assert!(
        resume_git_scan_checkpoint(
            &checkpoint.path,
            &checkpoint.digest,
            GitBridgeLimits {
                max_object_bytes: 1,
                ..GitBridgeLimits::default()
            },
            &mut |_| true
        )
        .is_err()
    );
}

#[test]
fn checkpoint_missing_blob_and_unsupported_source_never_bypass_the_converter() {
    let temp = Temp::new();
    let (source, git) = fixture(&temp, "sha1");
    let checkpoint = prepare_git_scan_checkpoint(
        &source,
        temp.0.join("frozen.scan"),
        &ImportSelection::AllLocalReferences,
        GitBridgeLimits::default(),
        &mut |_| true,
    )
    .unwrap();
    let mut bytes = std::fs::read(&checkpoint.path).unwrap();
    let count_at = header_end(&bytes);
    let count = u64::from_be_bytes(bytes[count_at..count_at + 8].try_into().unwrap());
    let mut at = count_at + 8;
    for _ in 0..count {
        let size_at = at + 2 + bytes[at + 1] as usize;
        let size = u64::from_be_bytes(bytes[size_at..size_at + 8].try_into().unwrap()) as usize;
        let end = size_at + 8 + size;
        if bytes[at] == 3 {
            bytes.drain(at..end);
            bytes[count_at..count_at + 8].copy_from_slice(&(count - 1).to_be_bytes());
            break;
        }
        at = end;
    }
    let digest = reseal(&mut bytes);
    let missing = temp.0.join("missing.scan");
    std::fs::write(&missing, bytes).unwrap();
    assert!(
        resume_git_scan_checkpoint(&missing, &digest, GitBridgeLimits::default(), &mut |_| true)
            .is_err()
    );
    let unsupported = tree(
        &git,
        &[(
            b"asset",
            b"version https://git-lfs.github.com/spec/v1\n",
            "100644",
        )],
    );
    let id = commit(&git, &unsupported, &[], "LFS must stay refused");
    reference(&git, "refs/heads/lfs", &id);
    let checkpoint = prepare_git_scan_checkpoint(
        &source,
        temp.0.join("lfs.scan"),
        &ImportSelection::References(vec!["refs/heads/lfs".into()]),
        GitBridgeLimits::default(),
        &mut |_| true,
    )
    .unwrap();
    assert!(
        resume_git_scan_checkpoint(
            &checkpoint.path,
            &checkpoint.digest,
            GitBridgeLimits::default(),
            &mut |_| true
        )
        .err()
        .unwrap()
        .to_string()
        .contains("Git LFS pointer")
    );
}

#[test]
fn checkpoint_never_overwrites_existing_files_or_writes_into_git_source() {
    let temp = Temp::new();
    let (source, git) = fixture(&temp, "sha1");
    let refs = git.run(&["show-ref"], None, 4096).unwrap();
    let existing = temp.0.join("existing");
    std::fs::write(&existing, b"user content").unwrap();
    for path in [&existing, &source.join("checkpoint")] {
        assert!(
            prepare_git_scan_checkpoint(
                &source,
                path,
                &ImportSelection::AllLocalReferences,
                GitBridgeLimits::default(),
                &mut |_| true
            )
            .is_err()
        );
    }
    assert_eq!(std::fs::read(&existing).unwrap(), b"user content");
    assert!(!source.join("checkpoint").exists());
    let cancelled = temp.0.join("cancelled.scan");
    assert!(matches!(
        prepare_git_scan_checkpoint(
            &source,
            &cancelled,
            &ImportSelection::AllLocalReferences,
            GitBridgeLimits::default(),
            &mut |_| false
        ),
        Err(BridgeError::Cancelled)
    ));
    assert!(!cancelled.exists());
    assert_eq!(git.run(&["show-ref"], None, 4096).unwrap(), refs);
}

fn header_end(bytes: &[u8]) -> usize {
    HEADER_START + u64::from_be_bytes(bytes[MAGIC_LEN..HEADER_START].try_into().unwrap()) as usize
}
fn reseal(bytes: &mut [u8]) -> String {
    let end = bytes.len() - 32;
    let digest = Sha256::digest(&bytes[..end]);
    bytes[end..].copy_from_slice(&digest);
    format!("sha256:{digest:x}")
}
fn change_header(bytes: &[u8], modify: fn(&mut serde_json::Value)) -> (Vec<u8>, String) {
    let end = header_end(bytes);
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&bytes[HEADER_START..end]).unwrap();
    modify(&mut metadata);
    let header = canonical_json(&metadata).unwrap();
    let mut changed = bytes[..MAGIC_LEN].to_vec();
    changed.extend_from_slice(&(header.len() as u64).to_be_bytes());
    changed.extend(header);
    changed.extend_from_slice(&bytes[end..]);
    let digest = reseal(&mut changed);
    (changed, digest)
}
