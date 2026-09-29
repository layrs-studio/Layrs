use super::*;

#[test]
fn remote_dirty_check_streams_files_above_legacy_limit_without_mutation() {
    use std::io::{Seek, SeekFrom, Write};
    struct Directory(std::path::PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let directory = Directory(std::env::temp_dir().join(format!(
        "layrs-remote-streaming-dirty-{}",
        uuid::Uuid::new_v4()
    )));
    std::fs::create_dir(&directory.0).unwrap();
    let path = directory.0.join("above-legacy-limit.bin");
    {
        let mut file = std::fs::File::create(&path).unwrap();
        let mut block = vec![0; 1024 * 1024];
        for index in 0..65u8 {
            block.fill(index);
            file.write_all(&block).unwrap();
        }
        file.sync_all().unwrap();
    }
    let workspace = NativeWorkspace::initialize(&directory.0, "Streaming dirty check").unwrap();
    let state = workspace.state().unwrap();
    assert!(!projection_dirty(&workspace, &state).unwrap());
    let change = |byte| {
        let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(64 * 1024 * 1024 + 5)).unwrap();
        file.write_all(&[byte]).unwrap();
        file.sync_all().unwrap();
    };
    change(255);
    assert!(projection_dirty(&workspace, &state).unwrap());
    change(64);
    assert!(!projection_dirty(&workspace, &state).unwrap());
    let current = workspace.state().unwrap();
    assert_eq!(current.operation, state.operation);
    assert_eq!(current.view, state.view);
    assert_eq!(current.worktree, state.worktree);
    assert!(!workspace.store_root().join("native-remote.json").exists());
    // The old whole-file API keeps its bound; only the metadata/stream path changed.
    let step = state.view.layer_heads()[0].step_ids()[0].clone();
    assert!(matches!(
        workspace.read_step_files(&step),
        Err(NativeClientError::InvalidState(
            "materialization file limit exceeded"
        ))
    ));
}
#[test]
fn error_responses_expose_only_bounded_machine_codes() {
    assert_eq!(
        service_error_code(br#"{"error":{"code":"stale_review_version","message":"secret body"}}"#)
            .as_deref(),
        Some("stale_review_version")
    );
    for code in [
        "",
        "secret with spaces",
        "bad\nheader",
        "https://user:secret@host",
    ] {
        assert!(service_error_code(
            &serde_json::to_vec(&serde_json::json!({"error":{"code":code}})).unwrap()
        )
        .is_none());
    }
    assert!(service_error_code(
        &serde_json::to_vec(&serde_json::json!({"error":{"code":"x".repeat(97)}})).unwrap()
    )
    .is_none());
}
#[test]
fn endpoint_and_capability_urls_cannot_leak_credentials() {
    for url in [
        "http://127.0.0.1:8787",
        "https://user:password@example.com",
        "https://example.com/prefix",
        "https://example.com?token=secret",
        "https://example.com/#fragment",
    ] {
        assert!(NativeRemote::new(url, "secret").is_err());
    }
    let remote = NativeRemote::new("https://example.com", "secret").unwrap();
    for url in [
        "https://other.example/v3/transfers/downloads",
        "https://example.com:444/v3/transfers/downloads",
        "http://example.com/v3/transfers/downloads",
        "https://example.com/v3/transfers/downloads?token=secret",
        "https://user@example.com/v3/transfers/downloads",
    ] {
        assert!(remote.transfer_url(url).is_err());
    }
    assert!(remote
        .transfer_url("https://example.com/v3/transfers/downloads")
        .is_ok());
    assert!(NativeRemote::new("https://example.com", "bad\r\ntoken").is_err());
    assert!(NativeRemote::with_trusted_ca(
        "https://example.com",
        "secret",
        &vec![b'x'; 1024 * 1024 + 1]
    )
    .is_err());
}
#[test]
fn decoded_objects_must_be_canonical_and_content_verified() {
    let tree = Tree::new(vec![]).unwrap();
    let bytes = serde_jcs::to_vec(&tree).unwrap();
    assert_eq!(
        decode(ObjectKind::Tree, &bytes).unwrap().object_id(),
        StoreObjectId::Tree(tree.id().clone())
    );
    let mut noncanonical = bytes.clone();
    noncanonical.push(b' ');
    assert!(decode(ObjectKind::Tree, &noncanonical).is_err());
    let mut forged: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    forged["treeId"] = serde_json::Value::String(format!("blake3:{}", "0".repeat(64)));
    assert!(decode(ObjectKind::Tree, &serde_jcs::to_vec(&forged).unwrap()).is_err());
    assert!(decode(ObjectKind::Evidence, b"{}").is_err());
}
#[test]
fn closure_and_materialization_limits_fail_before_unbounded_allocation() {
    let chunk = Chunk::new(b"content".to_vec()).unwrap();
    let manifest =
        FileManifest::from_chunks(ChunkingDescriptor::single(), std::slice::from_ref(&chunk))
            .unwrap();
    let mut graph = GraphObjects::default();
    graph
        .insert(CanonicalObject::FileManifest(manifest.clone()))
        .unwrap();
    assert!(graph.file_bytes(manifest.file_id()).is_err());
    graph
        .insert(CanonicalObject::ChunkRaw(chunk.clone()))
        .unwrap();
    assert_eq!(graph.file_bytes(manifest.file_id()).unwrap(), b"content");
    graph.metadata_bytes = MAX_TOTAL;
    assert!(graph
        .insert(CanonicalObject::Tree(Tree::new(vec![]).unwrap()))
        .is_err());
    let mut graph = GraphObjects::default();
    let mut tree = Tree::new(vec![]).unwrap();
    graph.insert(CanonicalObject::Tree(tree.clone())).unwrap();
    for _ in 0..130 {
        tree = Tree::new(vec![TreeEntry::new(
            RepoPathComponent::parse("nested").unwrap(),
            TreeEntryTarget::tree(tree.id().clone()),
            false,
        )])
        .unwrap();
        graph.insert(CanonicalObject::Tree(tree.clone())).unwrap();
    }
    let step = Step::new(vec![], tree.id().clone()).unwrap();
    graph.insert(CanonicalObject::Step(step.clone())).unwrap();
    assert!(graph.entries(step.id()).is_err());
}

#[test]
fn manifest_lookup_is_order_independent_and_does_not_hide_missing_chunks() {
    let chunk = Chunk::new(b"index-content".to_vec()).unwrap();
    let single =
        FileManifest::from_chunks(ChunkingDescriptor::single(), std::slice::from_ref(&chunk))
            .unwrap();
    let fixed = FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(64).unwrap(),
        std::slice::from_ref(&chunk),
    )
    .unwrap();
    assert_eq!(single.file_id(), fixed.file_id());
    assert_ne!(single.id(), fixed.id());
    for manifests in [
        [single.clone(), fixed.clone()],
        [fixed.clone(), single.clone()],
    ] {
        let mut graph = GraphObjects::default();
        assert!(!graph.has_manifest(single.file_id()));
        for manifest in manifests {
            graph
                .insert(CanonicalObject::FileManifest(manifest))
                .unwrap();
        }
        assert!(graph.has_manifest(single.file_id()));
        assert!(graph.file_bytes(single.file_id()).is_err());
        graph
            .insert(CanonicalObject::ChunkRaw(chunk.clone()))
            .unwrap();
        assert_eq!(graph.file_bytes(single.file_id()).unwrap(), chunk.bytes());
    }
}
