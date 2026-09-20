use super::*;

#[test]
fn seeded_reader_returns_all_typed_objects_entities_and_projections_without_mutation() {
    let root = unique_root("reader-all");
    let fixture = fixture();
    seed(&root, &fixture);
    let before = snapshot(&root);
    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();

    for object in &fixture.objects {
        match object {
            CanonicalObject::ChunkRaw(value) => {
                assert_eq!(reader.get_chunk(value.id()).unwrap(), Some(value.clone()))
            }
            CanonicalObject::FileManifest(value) => assert_eq!(
                reader.get_file_manifest(value.id()).unwrap(),
                Some(value.clone())
            ),
            CanonicalObject::Tree(value) => {
                assert_eq!(reader.get_tree(value.id()).unwrap(), Some(value.clone()))
            }
            CanonicalObject::Symlink(value) => {
                assert_eq!(reader.get_symlink(value.id()).unwrap(), Some(value.clone()))
            }
            CanonicalObject::Tombstone(value) => assert_eq!(
                reader.get_tombstone(value.id()).unwrap(),
                Some(value.clone())
            ),
            CanonicalObject::Step(value) => {
                assert_eq!(reader.get_step(value.id()).unwrap(), Some(value.clone()))
            }
            CanonicalObject::RepositoryView(value) => assert_eq!(
                reader.get_repository_view(value.id()).unwrap(),
                Some(value.clone())
            ),
            CanonicalObject::Conflict(value) => assert_eq!(
                reader.get_conflict(value.id()).unwrap(),
                Some(value.clone())
            ),
            CanonicalObject::Product(value) => {
                assert_eq!(reader.get_product(value.id()).unwrap(), Some(value.clone()))
            }
        }
    }

    assert_eq!(
        reader.get_space(fixture.space.id()).unwrap(),
        Some(fixture.space.clone())
    );
    assert_eq!(
        reader.get_layer(fixture.layer.id()).unwrap(),
        Some(fixture.layer.clone())
    );
    assert_eq!(
        reader
            .get_step_observation(fixture.observation.id())
            .unwrap(),
        Some(fixture.observation.clone())
    );
    assert_eq!(
        reader.get_operation(fixture.operation.id()).unwrap(),
        Some(fixture.operation.clone())
    );
    assert_eq!(
        reader.get_worktree(fixture.worktree.id()).unwrap(),
        Some(fixture.worktree.clone())
    );
    assert_eq!(
        reader.get_marker(fixture.marker.id()).unwrap(),
        Some(fixture.marker.clone())
    );
    assert_eq!(
        reader.get_release(fixture.release.id()).unwrap(),
        Some(fixture.release.clone())
    );
    assert_eq!(
        reader
            .get_release_publication(fixture.publication.id())
            .unwrap(),
        Some(fixture.publication.clone())
    );

    let manifest = fixture
        .objects
        .iter()
        .find_map(|object| match object {
            CanonicalObject::FileManifest(value) => Some(value),
            _ => None,
        })
        .unwrap();
    assert!(matches!(
        reader.file_availability(manifest.file_id()).unwrap(),
        FileAvailability::Present(_)
    ));
    let current = reader
        .current_repository_view(fixture.space.id())
        .unwrap()
        .unwrap();
    assert_eq!(current.generation(), 1);
    assert_eq!(
        current.view_id(),
        Some(fixture.operation.resulting_view_id())
    );
    assert_eq!(
        reader.operation_heads(fixture.space.id()).unwrap(),
        vec![fixture.operation.id().clone()]
    );
    assert_eq!(
        reader.initializer_in_space(fixture.space.id()).unwrap(),
        Some(fixture.operation.clone())
    );
    assert_eq!(
        reader
            .worktree_active_operation(fixture.worktree.id())
            .unwrap(),
        Some(fixture.operation.id().clone())
    );
    let step = fixture
        .objects
        .iter()
        .find_map(|object| match object {
            CanonicalObject::Step(value) => Some(value),
            _ => None,
        })
        .unwrap();
    assert!(
        reader
            .object_admitted_to_space(&StoreObjectId::Step(step.id().clone()), fixture.space.id())
            .unwrap()
    );
    assert_snapshot_eq(&snapshot(&root), &before);
    drop(reader);
    drop(store);
    cleanup(&root);
}

#[test]
fn typed_kind_and_payload_projection_mismatches_are_corruption() {
    let root = unique_root("reader-corruption");
    let fixture = fixture();
    seed(&root, &fixture);
    let chunk = fixture
        .objects
        .iter()
        .find_map(|object| match object {
            CanonicalObject::ChunkRaw(value) => Some(value),
            _ => None,
        })
        .unwrap();
    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    connection
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .unwrap();
    connection
        .execute(
            "UPDATE objects SET object_kind = 'tree' WHERE object_id = ?1",
            [chunk.id().as_str()],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE layers SET layer_kind = 'automated_result' WHERE layer_id = ?1",
            [fixture.layer.id().as_str()],
        )
        .unwrap();
    checkpoint(&connection);
    drop(connection);

    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert_eq!(
        reader.get_chunk(chunk.id()).unwrap_err().code(),
        "corruption"
    );
    assert_eq!(
        reader.get_layer(fixture.layer.id()).unwrap_err().code(),
        "corruption"
    );
    drop(reader);
    drop(store);
    cleanup(&root);
}

#[test]
fn verified_database_row_with_tampered_cas_bytes_is_corruption() {
    let root = unique_root("reader-cas-corruption");
    let fixture = fixture();
    seed(&root, &fixture);
    let chunk = fixture
        .objects
        .iter()
        .find_map(|object| match object {
            CanonicalObject::ChunkRaw(value) => Some(value),
            _ => None,
        })
        .unwrap();
    let manifest = fixture
        .objects
        .iter()
        .find_map(|object| match object {
            CanonicalObject::FileManifest(value) => Some(value),
            _ => None,
        })
        .unwrap();

    let store = open(&root).unwrap();
    let cas = NativeCas::open(&store).unwrap();
    let path = cas
        .object_path(&StoreObjectId::ChunkRaw(chunk.id().clone()))
        .unwrap();
    drop(cas);
    drop(store);
    let original = fs::read(&path).unwrap();
    fs::write(&path, vec![b'x'; original.len()]).unwrap();

    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert_eq!(
        reader
            .file_availability(manifest.file_id())
            .unwrap_err()
            .code(),
        "corruption"
    );
    drop(reader);
    drop(store);
    cleanup(&root);
}

#[test]
fn verified_database_row_with_missing_cas_object_is_corruption() {
    let root = unique_root("reader-cas-missing");
    let fixture = fixture();
    seed(&root, &fixture);
    let chunk = fixture
        .objects
        .iter()
        .find_map(|object| match object {
            CanonicalObject::ChunkRaw(value) => Some(value),
            _ => None,
        })
        .unwrap();
    let manifest = fixture
        .objects
        .iter()
        .find_map(|object| match object {
            CanonicalObject::FileManifest(value) => Some(value),
            _ => None,
        })
        .unwrap();

    let store = open(&root).unwrap();
    let cas = NativeCas::open(&store).unwrap();
    let path = cas
        .object_path(&StoreObjectId::ChunkRaw(chunk.id().clone()))
        .unwrap();
    drop(cas);
    drop(store);
    fs::remove_file(path).unwrap();

    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert_eq!(
        reader
            .file_availability(manifest.file_id())
            .unwrap_err()
            .code(),
        "corruption"
    );
    drop(reader);
    drop(store);
    cleanup(&root);
}

#[test]
fn ordered_entity_and_object_projection_divergence_is_corruption() {
    let root = unique_root("reader-ordered-projection-corruption");
    let fixture = fixture();
    seed(&root, &fixture);
    let step = fixture
        .objects
        .iter()
        .find_map(|object| match object {
            CanonicalObject::Step(value) => Some(value),
            _ => None,
        })
        .unwrap();
    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    connection
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .unwrap();
    connection
        .execute(
            "INSERT INTO operation_parents VALUES (?1, 'operation:ghost', 0)",
            [fixture.operation.id().as_str()],
        )
        .unwrap();
    connection
        .execute(
            "DELETE FROM object_edges WHERE source_object_id = ?1 AND edge_kind = 'step_tree'",
            [step.id().as_str()],
        )
        .unwrap();
    checkpoint(&connection);
    drop(connection);

    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert_eq!(
        reader
            .get_operation(fixture.operation.id())
            .unwrap_err()
            .code(),
        "corruption"
    );
    assert_eq!(reader.get_step(step.id()).unwrap_err().code(), "corruption");
    assert_eq!(
        reader
            .operation_reaches_step(fixture.operation.id(), step.id())
            .unwrap_err()
            .code(),
        "corruption"
    );
    let unrelated = StepId::parse(format!("blake3:{}", "d".repeat(64))).unwrap();
    assert_eq!(
        reader
            .is_step_ancestor(&unrelated, step.id())
            .unwrap_err()
            .code(),
        "corruption"
    );
    drop(reader);
    drop(store);
    cleanup(&root);
}

#[test]
fn authorization_decisions_revalidate_typed_cas() {
    let root = unique_root("reader-authorization-cas-corruption");
    let fixture = fixture();
    seed(&root, &fixture);
    let step = fixture
        .objects
        .iter()
        .find_map(|object| match object {
            CanonicalObject::Step(value) => Some(value),
            _ => None,
        })
        .unwrap();
    let conflict = fixture
        .objects
        .iter()
        .find_map(|object| match object {
            CanonicalObject::Conflict(value) => Some(value),
            _ => None,
        })
        .unwrap();
    let store = open(&root).unwrap();
    let cas = NativeCas::open(&store).unwrap();
    let step_path = cas
        .object_path(&StoreObjectId::Step(step.id().clone()))
        .unwrap();
    let conflict_path = cas
        .object_path(&StoreObjectId::Conflict(conflict.id().clone()))
        .unwrap();
    drop(cas);
    drop(store);
    fs::remove_file(step_path).unwrap();
    let bytes = fs::read(&conflict_path).unwrap();
    fs::write(conflict_path, vec![b'x'; bytes.len()]).unwrap();

    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert_eq!(
        reader
            .object_admitted_to_space(&StoreObjectId::Step(step.id().clone()), fixture.space.id(),)
            .unwrap_err()
            .code(),
        "corruption"
    );
    assert_eq!(
        reader.conflict_exists(conflict.id()).unwrap_err().code(),
        "corruption"
    );
    drop(reader);
    drop(store);
    cleanup(&root);
}
