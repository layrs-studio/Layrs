use super::*;
use layrs_core::scm::{
    Chunk, ConflictId, FileId, FileManifestId, Metadata, Operation, OperationId, OperationKind,
    RepositoryViewId, Space, SpaceId, Timestamp,
};

fn oid(seed: u64) -> String {
    format!("blake3:{seed:064x}")
}

fn space_id(name: &str) -> SpaceId {
    SpaceId::parse(format!("space:{name}")).unwrap()
}

fn view_id(seed: u64) -> RepositoryViewId {
    RepositoryViewId::parse(oid(seed)).unwrap()
}

fn operation_id(name: &str) -> OperationId {
    OperationId::parse(format!("operation:{name}")).unwrap()
}

fn operation(
    name: &str,
    target_space: &SpaceId,
    parents: Vec<OperationId>,
    kind: OperationKind,
    seed: u64,
) -> Operation {
    Operation::new(
        operation_id(name),
        target_space.clone(),
        parents,
        view_id(seed),
        kind,
        name,
        vec![],
        None,
        None,
        Timestamp::parse("2026-07-19T00:00:00Z").unwrap(),
        Metadata::empty(),
    )
    .unwrap()
}

fn sorted_ids(ids: impl IntoIterator<Item = OperationId>) -> Vec<OperationId> {
    let mut ids: Vec<_> = ids.into_iter().collect();
    ids.sort();
    ids
}

fn transaction(name: &str) -> TransactionId {
    TransactionId::new(format!("transaction:{name}")).unwrap()
}

fn conflict() -> CanonicalConflict {
    let payload = serde_json::json!({
        "schemaVersion": "layrs.conflict.v1",
        "sides": [
            {"label": "left", "treeId": oid(501)},
            {"label": "right", "treeId": oid(502)}
        ]
    });
    let payload_bytes = layrs_core::scm::canonical_json(&payload).unwrap();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"layrs-conflict-v1");
    hasher.update(&[0]);
    hasher.update(&payload_bytes);
    let id = ConflictId::parse(format!("blake3:{}", hasher.finalize().to_hex())).unwrap();
    let mut full = payload;
    full.as_object_mut()
        .unwrap()
        .insert("conflictId".into(), id.to_string().into());
    let full = layrs_core::scm::canonical_json(&full).unwrap();
    CanonicalConflict::new(id, full).unwrap()
}

#[test]
fn error_contract_is_stable_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<StoreError>();
    assert_send_sync::<WriteBatch>();
    assert_send_sync::<Box<dyn ScmReader>>();

    assert_eq!(StoreError::EmptyBatch.code(), "empty_batch");
    assert_eq!(
        StoreError::CasMismatch {
            target: "space:test".into()
        }
        .code(),
        "cas_mismatch"
    );
    assert_eq!(
        StoreError::TransactionReplayMismatch {
            transaction_id: transaction("replay")
        }
        .code(),
        "transaction_replay_mismatch"
    );
}

#[test]
fn transaction_ids_and_empty_batches_are_rejected() {
    assert_eq!(
        TransactionId::new("").unwrap_err().code(),
        "invalid_transaction_id"
    );
    assert_eq!(
        TransactionId::new("contains whitespace")
            .unwrap_err()
            .code(),
        "invalid_transaction_id"
    );
    assert_eq!(
        WriteBatch::new(transaction("empty"), vec![], vec![], vec![], vec![])
            .unwrap_err()
            .code(),
        "empty_batch"
    );
}

#[test]
fn canonical_order_and_request_digest_ignore_input_and_transaction_ordering() {
    let first = Chunk::new(b"first".to_vec()).unwrap();
    let second = Chunk::new(b"second".to_vec()).unwrap();
    let alpha = SpaceObjectAdmission::new(
        space_id("alpha"),
        StoreObjectId::ChunkRaw(first.id().clone()),
    );
    let beta = SpaceObjectAdmission::new(
        space_id("beta"),
        StoreObjectId::ChunkRaw(second.id().clone()),
    );

    let forward = WriteBatch::new(
        transaction("forward"),
        vec![
            CanonicalObject::ChunkRaw(first.clone()),
            CanonicalObject::ChunkRaw(second.clone()),
        ],
        vec![],
        vec![alpha.clone(), beta.clone()],
        vec![],
    )
    .unwrap();
    let reverse = WriteBatch::new(
        transaction("reverse"),
        vec![
            CanonicalObject::ChunkRaw(second),
            CanonicalObject::ChunkRaw(first),
        ],
        vec![],
        vec![beta, alpha],
        vec![],
    )
    .unwrap();

    assert_eq!(forward.request_digest(), reverse.request_digest());
    assert_ne!(forward.transaction_id(), reverse.transaction_id());
    let ids: Vec<_> = reverse
        .canonical_objects()
        .iter()
        .map(CanonicalObject::object_id)
        .collect();
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn duplicate_objects_entities_and_admissions_are_rejected() {
    let chunk = Chunk::new(b"duplicate".to_vec()).unwrap();
    let duplicate_object = WriteBatch::new(
        transaction("duplicate-object"),
        vec![
            CanonicalObject::ChunkRaw(chunk.clone()),
            CanonicalObject::ChunkRaw(chunk.clone()),
        ],
        vec![],
        vec![],
        vec![],
    )
    .unwrap_err();
    assert_eq!(duplicate_object.code(), "duplicate_object");

    let target_space = Space::new(
        space_id("duplicate"),
        "Duplicate",
        Timestamp::parse("2026-07-19T00:00:00Z").unwrap(),
        Metadata::empty(),
    )
    .unwrap();
    let duplicate_entity = WriteBatch::new(
        transaction("duplicate-entity"),
        vec![],
        vec![
            EntityRecord::Space(target_space.clone()),
            EntityRecord::Space(target_space),
        ],
        vec![],
        vec![],
    )
    .unwrap_err();
    assert_eq!(duplicate_entity.code(), "duplicate_entity");

    let admission = SpaceObjectAdmission::new(
        space_id("duplicate"),
        StoreObjectId::ChunkRaw(chunk.id().clone()),
    );
    let duplicate_admission = WriteBatch::new(
        transaction("duplicate-admission"),
        vec![],
        vec![],
        vec![admission.clone(), admission],
        vec![],
    )
    .unwrap_err();
    assert_eq!(duplicate_admission.code(), "duplicate_admission");
}

#[test]
fn opaque_conflict_requires_exact_jcs_and_content_identity() {
    let valid = conflict();
    assert_eq!(
        valid.id().as_str(),
        StoreObjectId::Conflict(valid.id().clone()).as_str()
    );
    assert_eq!(StoreObjectKind::Conflict.to_string(), "conflict");
    assert_eq!(
        CanonicalConflict::new(valid.id().clone(), b"{")
            .unwrap_err()
            .code(),
        "invalid_conflict_document"
    );

    let document: serde_json::Value = serde_json::from_slice(valid.canonical_json()).unwrap();
    let pretty = serde_json::to_vec_pretty(&document).unwrap();
    assert_eq!(
        CanonicalConflict::new(valid.id().clone(), pretty)
            .unwrap_err()
            .code(),
        "non_canonical_conflict"
    );

    let wrong = ConflictId::parse(oid(999)).unwrap();
    assert_eq!(
        CanonicalConflict::new(wrong, valid.canonical_json())
            .unwrap_err()
            .code(),
        "conflict_identity_mismatch"
    );

    let mut altered: serde_json::Value = serde_json::from_slice(valid.canonical_json()).unwrap();
    altered["sides"][0]["label"] = "altered".into();
    let altered = layrs_core::scm::canonical_json(&altered).unwrap();
    assert_eq!(
        CanonicalConflict::new(valid.id().clone(), altered)
            .unwrap_err()
            .code(),
        "conflict_identity_mismatch"
    );
}

#[test]
fn opaque_conflict_participates_in_canonical_batch_identity() {
    let value = conflict();
    let first = WriteBatch::new(
        transaction("conflict-first"),
        vec![CanonicalObject::Conflict(value.clone())],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    let second = WriteBatch::new(
        transaction("conflict-second"),
        vec![CanonicalObject::Conflict(value.clone())],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    assert_eq!(first.request_digest(), second.request_digest());
    assert_eq!(
        first.canonical_objects()[0].object_id(),
        StoreObjectId::Conflict(value.id().clone())
    );
}

#[test]
fn cas_sets_are_exact_canonical_and_mutable_targets_are_unique() {
    let target_space = space_id("cas");
    let a = operation_id("a");
    let b = operation_id("b");
    assert_eq!(
        OperationHeadsCas::new(
            target_space.clone(),
            vec![b.clone(), a.clone()],
            operation_id("new-a"),
            vec![b.clone()],
        )
        .unwrap_err()
        .code(),
        "non_canonical_set"
    );
    assert_eq!(
        OperationHeadsCas::new(
            target_space.clone(),
            vec![a.clone(), a],
            operation_id("new-b"),
            vec![b],
        )
        .unwrap_err()
        .code(),
        "non_canonical_set"
    );
    assert_eq!(
        RepositoryViewCas::new(target_space.clone(), u64::MAX, None, view_id(1))
            .unwrap_err()
            .code(),
        "invalid_cas"
    );

    let first = StoreMutation::RepositoryView(
        RepositoryViewCas::new(target_space.clone(), 0, None, view_id(1)).unwrap(),
    );
    let second = StoreMutation::RepositoryView(
        RepositoryViewCas::new(target_space, 1, Some(view_id(1)), view_id(2)).unwrap(),
    );
    assert_eq!(
        WriteBatch::new(
            transaction("duplicate-target"),
            vec![],
            vec![],
            vec![],
            vec![first, second],
        )
        .unwrap_err()
        .code(),
        "duplicate_mutable_target"
    );
}

#[test]
fn operation_heads_cas_derives_initialize_normal_and_integration_transitions() {
    let target_space = space_id("head-transitions");
    let initialize = operation(
        "initialize",
        &target_space,
        vec![],
        OperationKind::Initialize,
        80,
    );
    let initialize_cas = OperationHeadsCas::new(
        target_space.clone(),
        vec![],
        initialize.id().clone(),
        vec![initialize.id().clone()],
    )
    .unwrap();
    WriteBatch::new(
        transaction("initialize-heads"),
        vec![],
        vec![EntityRecord::Operation(initialize.clone())],
        vec![],
        vec![
            StoreMutation::OperationHeads(initialize_cas),
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(
                    target_space.clone(),
                    0,
                    None,
                    initialize.resulting_view_id().clone(),
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();

    let left = operation_id("left");
    let right = operation_id("right");
    let expected = sorted_ids([left.clone(), right.clone()]);
    let advance = operation(
        "advance-left",
        &target_space,
        vec![left],
        OperationKind::CaptureWorktree,
        81,
    );
    let new_heads = sorted_ids([right.clone(), advance.id().clone()]);
    let advance_cas = OperationHeadsCas::new(
        target_space.clone(),
        expected.clone(),
        advance.id().clone(),
        new_heads,
    )
    .unwrap();
    WriteBatch::new(
        transaction("advance-heads"),
        vec![],
        vec![EntityRecord::Operation(advance.clone())],
        vec![],
        vec![
            StoreMutation::OperationHeads(advance_cas),
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(
                    target_space.clone(),
                    1,
                    Some(view_id(80)),
                    advance.resulting_view_id().clone(),
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();

    let integrate = operation(
        "integrate-heads",
        &target_space,
        expected.clone(),
        OperationKind::IntegrateOperations,
        82,
    );
    let integrate_cas = OperationHeadsCas::new(
        target_space.clone(),
        expected,
        integrate.id().clone(),
        vec![integrate.id().clone()],
    )
    .unwrap();
    WriteBatch::new(
        transaction("integrate-heads"),
        vec![],
        vec![EntityRecord::Operation(integrate.clone())],
        vec![],
        vec![
            StoreMutation::OperationHeads(integrate_cas),
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(
                    target_space,
                    2,
                    Some(view_id(81)),
                    integrate.resulting_view_id().clone(),
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
}

#[test]
fn operation_import_without_heads_cas_allows_multiple_historical_records() {
    let target_space = space_id("historical-import");
    let root = operation(
        "historical-root",
        &target_space,
        vec![],
        OperationKind::Initialize,
        90,
    );
    let child = operation(
        "historical-child",
        &target_space,
        vec![root.id().clone()],
        OperationKind::CaptureWorktree,
        91,
    );
    let batch = WriteBatch::new(
        transaction("historical-import"),
        vec![],
        vec![
            EntityRecord::Operation(child),
            EntityRecord::Operation(root),
        ],
        vec![],
        vec![],
    )
    .unwrap();
    assert_eq!(batch.entity_records().len(), 2);
}

#[test]
fn operation_heads_cas_requires_exactly_its_named_operation_in_the_space() {
    let target_space = space_id("head-binding");
    let missing_id = operation_id("missing");
    let missing_cas = OperationHeadsCas::new(
        target_space.clone(),
        vec![],
        missing_id.clone(),
        vec![missing_id],
    )
    .unwrap();
    assert_eq!(
        WriteBatch::new(
            transaction("missing-head-operation"),
            vec![],
            vec![],
            vec![],
            vec![StoreMutation::OperationHeads(missing_cas)],
        )
        .unwrap_err()
        .code(),
        "operation_heads_cas_missing_operation"
    );

    let root = operation_id("binding-root");
    let selected = operation(
        "binding-selected",
        &target_space,
        vec![root.clone()],
        OperationKind::CaptureWorktree,
        92,
    );
    let extra = operation(
        "binding-extra",
        &target_space,
        vec![root.clone()],
        OperationKind::SwitchLayer,
        93,
    );
    let selected_cas = OperationHeadsCas::new(
        target_space.clone(),
        vec![root],
        selected.id().clone(),
        vec![selected.id().clone()],
    )
    .unwrap();
    let selected_view = selected.resulting_view_id().clone();
    WriteBatch::new(
        transaction("selected-plus-passive-operation"),
        vec![],
        vec![
            EntityRecord::Operation(selected),
            EntityRecord::Operation(extra),
        ],
        vec![],
        vec![
            StoreMutation::OperationHeads(selected_cas),
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(target_space.clone(), 1, Some(view_id(90)), selected_view)
                    .unwrap(),
            ),
        ],
    )
    .unwrap();

    let other_space = space_id("other-head-binding");
    let wrong_space = operation(
        "wrong-space-operation",
        &other_space,
        vec![],
        OperationKind::Initialize,
        94,
    );
    let wrong_space_cas = OperationHeadsCas::new(
        target_space,
        vec![],
        wrong_space.id().clone(),
        vec![wrong_space.id().clone()],
    )
    .unwrap();
    assert_eq!(
        WriteBatch::new(
            transaction("wrong-space-head-operation"),
            vec![],
            vec![EntityRecord::Operation(wrong_space)],
            vec![],
            vec![StoreMutation::OperationHeads(wrong_space_cas)],
        )
        .unwrap_err()
        .code(),
        "operation_heads_cas_space_mismatch"
    );
}

#[test]
fn operation_publication_and_repository_view_cas_are_exactly_coupled() {
    let target_space = space_id("publication-view-coupling");
    let root = operation_id("coupling-root");
    let published = operation(
        "coupling-published",
        &target_space,
        vec![root.clone()],
        OperationKind::CaptureWorktree,
        120,
    );
    let heads = StoreMutation::OperationHeads(
        OperationHeadsCas::new(
            target_space.clone(),
            vec![root],
            published.id().clone(),
            vec![published.id().clone()],
        )
        .unwrap(),
    );
    assert_eq!(
        WriteBatch::new(
            transaction("publication-without-view"),
            vec![],
            vec![EntityRecord::Operation(published.clone())],
            vec![],
            vec![heads.clone()],
        )
        .unwrap_err()
        .code(),
        "operation_publication_missing_view_cas"
    );

    let mismatched_view = StoreMutation::RepositoryView(
        RepositoryViewCas::new(target_space.clone(), 1, Some(view_id(119)), view_id(121)).unwrap(),
    );
    assert_eq!(
        WriteBatch::new(
            transaction("publication-view-mismatch"),
            vec![],
            vec![EntityRecord::Operation(published.clone())],
            vec![],
            vec![heads.clone(), mismatched_view],
        )
        .unwrap_err()
        .code(),
        "operation_publication_view_mismatch"
    );

    let matching_view = StoreMutation::RepositoryView(
        RepositoryViewCas::new(
            target_space.clone(),
            1,
            Some(view_id(119)),
            published.resulting_view_id().clone(),
        )
        .unwrap(),
    );
    WriteBatch::new(
        transaction("publication-view-exact"),
        vec![],
        vec![EntityRecord::Operation(published)],
        vec![],
        vec![heads, matching_view],
    )
    .unwrap();

    let orphan_space = space_id("orphan-view-cas");
    let orphan_view = StoreMutation::RepositoryView(
        RepositoryViewCas::new(orphan_space, 0, None, view_id(122)).unwrap(),
    );
    assert_eq!(
        WriteBatch::new(
            transaction("orphan-view-cas"),
            vec![],
            vec![],
            vec![],
            vec![orphan_view],
        )
        .unwrap_err()
        .code(),
        "repository_view_cas_missing_operation_publication"
    );
}

#[test]
fn operation_heads_cas_rejects_reductions_arbitrary_heads_and_incomplete_integrations() {
    let target_space = space_id("invalid-head-transitions");
    let left = operation_id("invalid-left");
    let right = operation_id("invalid-right");
    let third = operation_id("invalid-third");
    let expected_two = sorted_ids([left.clone(), right.clone()]);
    let advance = operation(
        "invalid-advance",
        &target_space,
        vec![left.clone()],
        OperationKind::CaptureWorktree,
        95,
    );
    let reduction = OperationHeadsCas::new(
        target_space.clone(),
        expected_two.clone(),
        advance.id().clone(),
        vec![left.clone()],
    )
    .unwrap();
    assert_eq!(
        WriteBatch::new(
            transaction("head-reduction"),
            vec![],
            vec![EntityRecord::Operation(advance.clone())],
            vec![],
            vec![
                StoreMutation::OperationHeads(reduction),
                StoreMutation::RepositoryView(
                    RepositoryViewCas::new(
                        target_space.clone(),
                        1,
                        Some(view_id(94)),
                        advance.resulting_view_id().clone(),
                    )
                    .unwrap(),
                ),
            ],
        )
        .unwrap_err()
        .code(),
        "invalid_operation_heads_transition"
    );

    let historical = operation_id("arbitrary-historical-head");
    let arbitrary = OperationHeadsCas::new(
        target_space.clone(),
        expected_two.clone(),
        advance.id().clone(),
        sorted_ids([right.clone(), historical]),
    )
    .unwrap();
    assert_eq!(
        WriteBatch::new(
            transaction("arbitrary-historical-head"),
            vec![],
            vec![EntityRecord::Operation(advance.clone())],
            vec![],
            vec![
                StoreMutation::OperationHeads(arbitrary),
                StoreMutation::RepositoryView(
                    RepositoryViewCas::new(
                        target_space.clone(),
                        1,
                        Some(view_id(94)),
                        advance.resulting_view_id().clone(),
                    )
                    .unwrap(),
                ),
            ],
        )
        .unwrap_err()
        .code(),
        "invalid_operation_heads_transition"
    );

    let expected_three = sorted_ids([left.clone(), right.clone(), third]);
    let incomplete_integration = operation(
        "incomplete-integration",
        &target_space,
        vec![left, right],
        OperationKind::IntegrateOperations,
        96,
    );
    let incomplete_cas = OperationHeadsCas::new(
        target_space.clone(),
        expected_three,
        incomplete_integration.id().clone(),
        vec![incomplete_integration.id().clone()],
    )
    .unwrap();
    assert_eq!(
        WriteBatch::new(
            transaction("incomplete-integration"),
            vec![],
            vec![EntityRecord::Operation(incomplete_integration.clone())],
            vec![],
            vec![
                StoreMutation::OperationHeads(incomplete_cas),
                StoreMutation::RepositoryView(
                    RepositoryViewCas::new(
                        target_space,
                        1,
                        Some(view_id(94)),
                        incomplete_integration.resulting_view_id().clone(),
                    )
                    .unwrap(),
                ),
            ],
        )
        .unwrap_err()
        .code(),
        "invalid_operation_heads_transition"
    );
}

#[test]
fn absent_file_is_distinct_from_a_present_empty_file() {
    let empty_file_id = FileId::from_bytes(&[]);
    let manifest_id = FileManifestId::parse(oid(70)).unwrap();
    let present = FileAvailability::present(empty_file_id.clone(), vec![manifest_id]).unwrap();

    assert_ne!(present, FileAvailability::Absent);
    assert!(matches!(
        present,
        FileAvailability::Present(file) if file.file_id() == &empty_file_id
    ));
    assert_eq!(
        FileAvailability::present(empty_file_id, vec![])
            .unwrap_err()
            .code(),
        "admission_rejected"
    );
}

#[test]
fn maintenance_reports_are_typed_and_read_only_values() {
    let clean_recovery = RecoveryReport::default();
    let clean_fsck = FsckReport::default();
    assert!(clean_recovery.is_clean());
    assert!(clean_fsck.is_clean());

    let recovery = RecoveryReport::new(vec![RecoveryFinding::new(
        RecoveryFindingCode::IncompleteTechnicalTransaction,
        ReportSeverity::Warning,
        Some(transaction("interrupted")),
        "technical journal has an uncommitted entry",
    )]);
    assert!(!recovery.is_clean());
    assert_eq!(
        recovery.findings()[0].code(),
        RecoveryFindingCode::IncompleteTechnicalTransaction
    );
}

#[test]
fn public_repository_traits_are_object_safe_and_batch_oriented() {
    fn accepts_reader(_: &dyn ScmReader) {}
    fn accepts_maintenance(_: &dyn Maintenance) {}
    fn apply<R: ScmRepository>(repository: &mut R, batch: &WriteBatch) -> Result<()> {
        repository.apply_batch(batch).map(|_| ())
    }

    let _reader_signature: fn(&dyn ScmReader) = accepts_reader;
    let _maintenance_signature: fn(&dyn Maintenance) = accepts_maintenance;
    let _apply_is_generic = apply::<CompileOnlyRepository>;
}

struct CompileOnlyRepository;

impl ScmReader for CompileOnlyRepository {
    fn get_chunk(&self, _: &layrs_core::scm::ChunkId) -> Result<Option<Chunk>> {
        unimplemented!()
    }

    fn get_file_manifest(
        &self,
        _: &FileManifestId,
    ) -> Result<Option<layrs_core::scm::FileManifest>> {
        unimplemented!()
    }

    fn get_tree(&self, _: &layrs_core::scm::TreeId) -> Result<Option<layrs_core::scm::Tree>> {
        unimplemented!()
    }

    fn get_symlink(
        &self,
        _: &layrs_core::scm::SymlinkId,
    ) -> Result<Option<layrs_core::scm::Symlink>> {
        unimplemented!()
    }

    fn get_tombstone(
        &self,
        _: &layrs_core::scm::TombstoneId,
    ) -> Result<Option<layrs_core::scm::Tombstone>> {
        unimplemented!()
    }

    fn get_step(&self, _: &layrs_core::scm::StepId) -> Result<Option<layrs_core::scm::Step>> {
        unimplemented!()
    }

    fn get_repository_view(
        &self,
        _: &RepositoryViewId,
    ) -> Result<Option<layrs_core::scm::RepositoryView>> {
        unimplemented!()
    }

    fn get_conflict(&self, _: &ConflictId) -> Result<Option<CanonicalConflict>> {
        unimplemented!()
    }

    fn get_space(&self, _: &SpaceId) -> Result<Option<Space>> {
        unimplemented!()
    }

    fn get_layer(&self, _: &layrs_core::scm::LayerId) -> Result<Option<layrs_core::scm::Layer>> {
        unimplemented!()
    }

    fn get_step_observation(
        &self,
        _: &layrs_core::scm::ObservationId,
    ) -> Result<Option<layrs_core::scm::StepObservation>> {
        unimplemented!()
    }

    fn get_operation(&self, _: &OperationId) -> Result<Option<layrs_core::scm::Operation>> {
        unimplemented!()
    }

    fn get_worktree(
        &self,
        _: &layrs_core::scm::WorktreeId,
    ) -> Result<Option<layrs_core::scm::Worktree>> {
        unimplemented!()
    }

    fn get_marker(&self, _: &layrs_core::scm::MarkerId) -> Result<Option<layrs_core::scm::Marker>> {
        unimplemented!()
    }

    fn get_release(
        &self,
        _: &layrs_core::scm::ReleaseId,
    ) -> Result<Option<layrs_core::scm::Release>> {
        unimplemented!()
    }

    fn get_release_publication(
        &self,
        _: &layrs_core::scm::ReleasePublicationId,
    ) -> Result<Option<layrs_core::scm::ReleasePublication>> {
        unimplemented!()
    }

    fn file_availability(&self, _: &FileId) -> Result<FileAvailability> {
        unimplemented!()
    }

    fn object_admitted_to_space(&self, _: &StoreObjectId, _: &SpaceId) -> Result<bool> {
        unimplemented!()
    }

    fn is_step_ancestor(
        &self,
        _: &layrs_core::scm::StepId,
        _: &layrs_core::scm::StepId,
    ) -> Result<bool> {
        unimplemented!()
    }

    fn operation_reaches_step(&self, _: &OperationId, _: &layrs_core::scm::StepId) -> Result<bool> {
        unimplemented!()
    }

    fn store_exists(&self, _: &layrs_core::scm::StoreId) -> Result<bool> {
        unimplemented!()
    }

    fn conflict_exists(&self, _: &layrs_core::scm::ConflictId) -> Result<bool> {
        unimplemented!()
    }

    fn proof_in_space(&self, _: &layrs_core::scm::ProofId, _: &SpaceId) -> Result<bool> {
        unimplemented!()
    }

    fn actor_exists(&self, _: &layrs_core::scm::ActorId) -> Result<bool> {
        unimplemented!()
    }

    fn device_exists(&self, _: &layrs_core::scm::DeviceId) -> Result<bool> {
        unimplemented!()
    }

    fn current_repository_view(&self, _: &SpaceId) -> Result<Option<CurrentRepositoryView>> {
        unimplemented!()
    }

    fn operation_heads(&self, _: &SpaceId) -> Result<Vec<OperationId>> {
        unimplemented!()
    }

    fn initializer_in_space(&self, _: &SpaceId) -> Result<Option<layrs_core::scm::Operation>> {
        unimplemented!()
    }

    fn worktree_active_operation(
        &self,
        _: &layrs_core::scm::WorktreeId,
    ) -> Result<Option<OperationId>> {
        unimplemented!()
    }
}

impl ScmRepository for CompileOnlyRepository {
    fn apply_batch(&mut self, _: &WriteBatch) -> Result<ApplyBatchOutcome> {
        unimplemented!()
    }
}
