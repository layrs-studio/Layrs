use super::*;

fn repeated_file_tree(file: &layrs_core::scm::FileId, entries: u32) -> Tree {
    Tree::new(
        (0..entries)
            .map(|index| {
                TreeEntry::new(
                    RepoPathComponent::parse(format!("f{index:06}")).unwrap(),
                    TreeEntryTarget::file(file.clone()),
                    false,
                )
            })
            .collect(),
    )
    .unwrap()
}

fn inventory(root: &Path) -> std::collections::BTreeMap<PathBuf, String> {
    let mut result = std::collections::BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    blake3::hash(&fs::read(path).unwrap()).to_hex().to_string(),
                );
            }
        }
    }
    result
}

#[test]
fn wide_tree_admission_is_independent_from_history_dag_budget_and_oversize_is_read_only() {
    use layrs_store::scm::{ScmRepository, StoreError};
    use layrs_store_local::scm_v1::NativeScmRepository;
    let root = unique_root("wide-tree-budget");
    let fixture = fixture();
    seed(&root, &fixture);
    let file = chunk_file_id(&fixture).clone();
    let tree = repeated_file_tree(&file, 99_000);
    let id = tree.id().clone();
    let batch = WriteBatch::new(
        transaction("wide-tree"),
        vec![CanonicalObject::Tree(tree)],
        vec![],
        vec![SpaceObjectAdmission::new(
            fixture.space.id().clone(),
            StoreObjectId::Tree(id.clone()),
        )],
        vec![],
    )
    .unwrap();
    NativeScmRepository::new(&root).apply_batch(&batch).unwrap();
    drop(batch);
    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert_eq!(
        reader.get_tree(&id).unwrap().unwrap().entries().len(),
        99_000
    );
    drop(reader);
    drop(store);
    let before = inventory(&root);
    let rejected = WriteBatch::new(
        transaction("too-wide-tree"),
        vec![CanonicalObject::Tree(repeated_file_tree(&file, 262_145))],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    assert!(matches!(
        NativeScmRepository::new(&root).apply_batch(&rejected),
        Err(StoreError::AdmissionBudgetExceeded {
            resource: "object graph edges",
            limit: 262_144
        })
    ));
    assert_eq!(
        inventory(&root),
        before,
        "no CAS preparation, SQLite change or sidecar on budget refusal"
    );
    cleanup(&root);
}

#[test]
fn history_parent_edges_remain_limited_to_16384() {
    use layrs_store::scm::StoreError;
    let root = unique_root("history-edge-budget");
    let fixture = fixture();
    seed(&root, &fixture);
    let file = chunk_file_id(&fixture);
    let mut objects = Vec::new();
    let mut trees = Vec::new();
    let mut parents = Vec::new();
    for index in 0..130 {
        let tree = Tree::new(vec![TreeEntry::new(
            RepoPathComponent::parse(format!("root{index}")).unwrap(),
            TreeEntryTarget::file(file.clone()),
            false,
        )])
        .unwrap();
        let step = Step::new(vec![], tree.id().clone()).unwrap();
        parents.push(step.id().clone());
        trees.push(tree.id().clone());
        objects.extend([CanonicalObject::Tree(tree), CanonicalObject::Step(step)]);
    }
    for tree in trees {
        objects.push(CanonicalObject::Step(
            Step::new(parents.clone(), tree).unwrap(),
        ));
    }
    let batch = WriteBatch::new(
        transaction("too-many-parent-edges"),
        objects,
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert!(matches!(
        CompositeAdmissionResolver::new(&reader, &batch),
        Err(StoreError::AdmissionBudgetExceeded {
            resource: "Step graph edges",
            limit: 16_384
        })
    ));
    drop(reader);
    drop(store);
    cleanup(&root);
}

#[test]
fn admission_chain_is_linearized_and_oversized_batches_fail_with_a_stable_budget() {
    let root = unique_root("admission-budget");
    let fixture = fixture();
    seed(&root, &fixture);
    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let tree = Tree::new(vec![]).unwrap();
    let mut parent = None;
    let mut objects = vec![CanonicalObject::Tree(tree.clone())];
    for _ in 0..1_500 {
        let step = Step::new(parent.into_iter().collect(), tree.id().clone()).unwrap();
        parent = Some(step.id().clone());
        objects.push(CanonicalObject::Step(step));
    }
    let chain =
        WriteBatch::new(transaction("long-chain"), objects, vec![], vec![], vec![]).unwrap();
    CompositeAdmissionResolver::new(&reader, &chain)
        .unwrap()
        .validate_batch()
        .unwrap();

    let too_many: Vec<_> = (0_u32..16_385)
        .map(|value| CanonicalObject::ChunkRaw(Chunk::new(value.to_le_bytes().to_vec()).unwrap()))
        .collect();
    let oversized =
        WriteBatch::new(transaction("oversized"), too_many, vec![], vec![], vec![]).unwrap();
    assert_eq!(
        CompositeAdmissionResolver::new(&reader, &oversized)
            .err()
            .unwrap()
            .code(),
        "admission_budget_exceeded"
    );

    let file_id = fixture
        .objects
        .iter()
        .find_map(|object| match object {
            CanonicalObject::FileManifest(value) => Some(value.file_id().clone()),
            _ => None,
        })
        .unwrap();
    let entries = (0_u32..16_385)
        .map(|index| {
            TreeEntry::new(
                RepoPathComponent::parse(format!("f{index:05}")).unwrap(),
                TreeEntryTarget::file(file_id.clone()),
                false,
            )
        })
        .collect();
    let edge_heavy = WriteBatch::new(
        transaction("edge-heavy"),
        vec![CanonicalObject::Tree(Tree::new(entries).unwrap())],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    CompositeAdmissionResolver::new(&reader, &edge_heavy)
        .unwrap()
        .validate_batch()
        .unwrap();
    drop(reader);
    drop(store);
    cleanup(&root);
}

#[test]
fn hostile_file_id_manifest_scan_stops_at_the_explicit_admission_budget() {
    let root = unique_root("file-budget");
    let fixture = fixture();
    seed(&root, &fixture);
    let chunk = fixture
        .objects
        .iter()
        .find_map(|object| match object {
            CanonicalObject::ChunkRaw(value) => Some(value.clone()),
            _ => None,
        })
        .unwrap();
    let manifests: Vec<_> = (1_u64..=12)
        .map(|chunk_bytes| {
            FileManifest::from_chunks(
                ChunkingDescriptor::deterministic_fixed(chunk_bytes).unwrap(),
                std::slice::from_ref(&chunk),
            )
            .unwrap()
        })
        .collect();

    let exclusive = open_exclusive(&root).unwrap();
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let mut rows = Vec::new();
    for manifest in &manifests {
        let outcome = cas
            .put(&CanonicalObject::FileManifest(manifest.clone()))
            .unwrap();
        rows.push((
            manifest.clone(),
            outcome
                .path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/"),
            outcome.bytes_len,
        ));
    }
    drop(cas);
    drop(exclusive);
    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    for (manifest, path, bytes_len) in rows {
        connection
            .execute(
                "INSERT INTO objects VALUES (?1, 'file_manifest', ?2, ?3, 1, 0)",
                params![
                    manifest.id().as_str(),
                    path,
                    i64::try_from(bytes_len).unwrap()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO file_manifests VALUES (?1, ?2, ?3, ?4)",
                params![
                    manifest.id().as_str(),
                    manifest.file_id().as_str(),
                    i64::try_from(manifest.byte_length()).unwrap(),
                    json(manifest.chunking())
                ],
            )
            .unwrap();
        let entry = &manifest.chunks()[0];
        connection
            .execute(
                "INSERT INTO file_manifest_chunks VALUES (?1, 0, ?2, ?3, ?4)",
                params![
                    manifest.id().as_str(),
                    entry.chunk_id().as_str(),
                    i64::try_from(entry.offset()).unwrap(),
                    i64::try_from(entry.byte_length()).unwrap()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO object_edges VALUES (?1, ?2, 'manifest_chunk', 0)",
                params![manifest.id().as_str(), entry.chunk_id().as_str()],
            )
            .unwrap();
    }
    checkpoint(&connection);
    drop(connection);

    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let error = reader
        .file_availability_with_budget(chunk_file_id(&fixture), 100, 1024 * 1024, 4, 100)
        .unwrap_err();
    assert_eq!(error.code(), "admission_budget_exceeded");
    assert!(!matches!(
        error,
        layrs_store::scm::StoreError::Backend { .. }
            | layrs_store::scm::StoreError::Corruption { .. }
    ));
    drop(reader);
    drop(store);
    cleanup(&root);
}

#[test]
fn chunk_durable_size_is_budgeted_and_checked_before_any_chunk_cas_read() {
    let root = unique_root("chunk-preflight-budget");
    let fixture = fixture();
    seed(&root, &fixture);
    let large_chunk = Chunk::new(vec![b'z'; 64 * 1024]).unwrap();
    let tiny_chunk = Chunk::new(vec![b'z']).unwrap();
    let tiny_manifest =
        FileManifest::from_chunks(ChunkingDescriptor::single(), &[tiny_chunk]).unwrap();
    let forged_manifest = FileManifest::new(
        tiny_manifest.file_id().clone(),
        1,
        ChunkingDescriptor::single(),
        vec![FileChunk::new(large_chunk.id().clone(), 0, 1).unwrap()],
    )
    .unwrap();

    let exclusive = open_exclusive(&root).unwrap();
    let cas = NativeCas::open_exclusive(&exclusive).unwrap();
    let chunk_outcome = cas
        .put(&CanonicalObject::ChunkRaw(large_chunk.clone()))
        .unwrap();
    let manifest_outcome = cas
        .put(&CanonicalObject::FileManifest(forged_manifest.clone()))
        .unwrap();
    drop(cas);
    drop(exclusive);

    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    for (outcome, kind) in [
        (&chunk_outcome, "chunk_raw"),
        (&manifest_outcome, "file_manifest"),
    ] {
        connection
            .execute(
                "INSERT INTO objects VALUES (?1, ?2, ?3, ?4, 1, 0)",
                params![
                    outcome.object_id.as_str(),
                    kind,
                    outcome
                        .path
                        .strip_prefix(&root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                    i64::try_from(outcome.bytes_len).unwrap()
                ],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO file_availability VALUES (?1, 'present', 0)",
            [forged_manifest.file_id().as_str()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO file_manifests VALUES (?1, ?2, 1, ?3)",
            params![
                forged_manifest.id().as_str(),
                forged_manifest.file_id().as_str(),
                json(forged_manifest.chunking())
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO file_manifest_chunks VALUES (?1, 0, ?2, 0, 1)",
            params![forged_manifest.id().as_str(), large_chunk.id().as_str()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO object_edges VALUES (?1, ?2, 'manifest_chunk', 0)",
            params![forged_manifest.id().as_str(), large_chunk.id().as_str()],
        )
        .unwrap();
    checkpoint(&connection);
    drop(connection);

    // If either path attempted to read the Chunk CAS, this sabotage would
    // surface as corruption from `NativeCas::read` instead of the preflight
    // decision asserted below.
    fs::write(&chunk_outcome.path, b"sabotaged chunk CAS").unwrap();
    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let budget_error = reader
        .file_availability_with_budget(forged_manifest.file_id(), 8, 4 * 1024, 1, 1)
        .unwrap_err();
    assert_eq!(budget_error.code(), "admission_budget_exceeded");

    let mismatch = reader
        .file_availability_with_budget(forged_manifest.file_id(), 8, 1024 * 1024, 1, 1)
        .unwrap_err();
    assert_eq!(mismatch.code(), "corruption");
    assert!(
        mismatch
            .to_string()
            .contains("objects.byte_length for Chunk")
    );
    drop(reader);
    drop(store);
    cleanup(&root);
}
