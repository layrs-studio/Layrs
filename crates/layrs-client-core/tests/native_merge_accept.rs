use layrs_client_core::native_scm::*;
use layrs_core::scm::*;
use layrs_store::scm::*;
use layrs_store_local::scm_v1::{self, materialize::*, NativeScmReader};
use layrs_sync::v3::ProductEnvelope;
use layrs_weave::{graph_merge::GraphMergeLimits, reconciliation::*};
use std::{fs, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("layrs-accept-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&p).unwrap();
        fs::write(p.join("code.ts"), b"a\nb\nc\n").unwrap();
        fs::write(p.join("b"), b"old\n").unwrap();
        Self(p)
    }
    fn workspace(&self) -> NativeWorkspace {
        NativeWorkspace::initialize(&self.0, "merge accept").unwrap()
    }
    fn write(&self, path: &str, bytes: &[u8]) {
        fs::write(self.0.join(path), bytes).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.0.parent() == Some(std::env::temp_dir().as_path()) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn head(w: &NativeWorkspace) -> StepId {
    w.state()
        .unwrap()
        .view
        .worktrees()
        .iter()
        .find(|s| s.worktree_id() == w.worktree_id())
        .unwrap()
        .base_step_id()
        .unwrap()
        .clone()
}
fn layer(w: &NativeWorkspace) -> LayerId {
    w.state()
        .unwrap()
        .layers
        .iter()
        .find(|l| l.name() == "main")
        .unwrap()
        .id()
        .clone()
}
fn policy() -> FileMergePolicy {
    FileMergePolicy {
        unsupported: FailureAction::StructuralFallback,
        quota: FailureAction::Reject,
        limits: FileMergeLimits::default(),
    }
}

#[test]
fn variant_comparison_preserves_absence_empty_binary_newlines_and_read_only_history() {
    for (left, right, available) in [
        (Some(b"".as_slice()), Some("é\r\nfin".as_bytes()), true),
        (
            Some(b"\0\xff".as_slice()),
            Some(b"text\n".as_slice()),
            false,
        ),
        (None, Some(b"changed\n".as_slice()), true),
    ] {
        let f = Fixture::new();
        let w = f.workspace();
        w.create_layer("topic").unwrap();
        w.switch_layer("topic").unwrap();
        match left {
            Some(bytes) => f.write("code.ts", bytes),
            None => fs::remove_file(f.0.join("code.ts")).unwrap(),
        }
        w.capture().unwrap();
        let source = head(&w);
        w.switch_layer("main").unwrap();
        f.write("code.ts", right.unwrap());
        w.capture().unwrap();
        let preview = w
            .preview_structural_merge(
                std::slice::from_ref(&source),
                &layer(&w),
                policy(),
                GraphMergeLimits::default(),
            )
            .unwrap();
        w.accept_merge(&preview).unwrap();
        let documents = w.layer_conflicts(&layer(&w)).unwrap();
        let conflict = &documents[0];
        let id = ConflictId::parse(conflict["conflictId"].as_str().unwrap()).unwrap();
        let sides = conflict["document"]["sides"].as_array().unwrap();
        let before = ObjectId::parse(
            sides
                .iter()
                .find(|side| side["provenance"]["stepId"] == source.as_str())
                .unwrap()["termId"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let after = ObjectId::parse(
            sides
                .iter()
                .find(|side| side["termId"] != before.as_str())
                .unwrap()["termId"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let operation = w.state().unwrap().operation.id().clone();
        let compared = w
            .compare_conflict_variants(&layer(&w), &id, &before, &after)
            .unwrap();
        assert_eq!(compared["available"], available);
        assert_eq!(compared["beforeTerm"], before.as_str());
        assert_eq!(compared["afterTerm"], after.as_str());
        if available {
            assert_eq!(
                compared["changes"][0]["beforeBytes"],
                left.map(|bytes| bytes.len())
                    .map_or(serde_json::Value::Null, |length| serde_json::json!(length))
            );
            assert_eq!(compared["changes"][0]["afterBytes"], right.unwrap().len());
            assert!(compared["changes"][0]["textDiff"]
                .as_array()
                .unwrap()
                .iter()
                .any(|line| line["op"] == "insert"));
        } else {
            assert!(compared["changes"].is_null());
            assert_eq!(compared["before"]["byteLength"], 2);
        }
        assert_eq!(w.state().unwrap().operation.id(), &operation);
        assert_eq!(fs::read(f.0.join("code.ts")).unwrap(), right.unwrap());
        let unknown = ObjectId::parse(format!("blake3:{}", "0".repeat(64))).unwrap();
        assert!(w
            .compare_conflict_variants(&layer(&w), &id, &unknown, &after)
            .is_err());
    }
}
fn branch(f: &Fixture, w: &NativeWorkspace) -> StepId {
    w.create_layer("topic").unwrap();
    w.switch_layer("topic").unwrap();
    f.write("code.ts", b"A\nb\nc\n");
    w.capture().unwrap();
    let source = head(w);
    w.switch_layer("main").unwrap();
    source
}
fn clean(f: &Fixture, w: &NativeWorkspace) -> NativeMergePreview {
    let source = branch(f, w);
    f.write("b", b"new\n");
    w.capture().unwrap();
    w.preview_merge(&[source], &layer(w), policy(), GraphMergeLimits::default())
        .unwrap()
}

#[cfg(windows)]
fn wasm_host(limits: layrs_weave::wasm_host::SandboxLimits) -> layrs_weave::wasm_host::WasmHost {
    let executable = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("layrs-lens-worker.exe");
    let bytes = fs::read(&executable)
        .expect("build fixture first: cargo build -p layrs-weave --bin layrs-lens-worker");
    let digest = layrs_lens_sdk::sha256_digest(&bytes);
    layrs_weave::wasm_host::WasmHost::open(&executable, &digest, limits).unwrap()
}

#[cfg(windows)]
#[test]
fn wasm_semantic_acceptance_persists_exact_module_and_enforced_budgets() {
    use layrs_weave::wasm_host::{self, SandboxLimits};
    let f = Fixture::new();
    let w = f.workspace();
    let source = branch(&f, &w);
    f.write("code.ts", b"a\nb\nC\n");
    w.capture().unwrap();
    let host = wasm_host(SandboxLimits::default());
    let preview = w
        .preview_sandboxed_merge(
            &[source],
            &layer(&w),
            policy(),
            GraphMergeLimits::default(),
            &host,
        )
        .unwrap();
    let accepted = w.accept_merge(&preview).unwrap();
    assert!(accepted.materialized);
    assert_eq!(fs::read(f.0.join("code.ts")).unwrap(), b"A\nb\nC\n");
    let store = scm_v1::open(w.store_root()).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let product = reader
        .get_product(&accepted.product_object_id)
        .unwrap()
        .unwrap();
    let ProductEnvelope::Weave { record, .. } = product.envelope() else {
        panic!("Weave expected")
    };
    wasm_host::validate_receipt_host(record).unwrap();
    assert_eq!(record.lens_executions.len(), 1);
    assert_eq!(
        record.lens_executions[0].implementation_digest.as_str(),
        wasm_host::module_digest()
    );
    assert_eq!(
        record.determinism_profile.limits.max_memory_bytes,
        SandboxLimits::default().process.memory_bytes as u64
    );
    assert_eq!(
        record.determinism_profile.limits.max_fuel.as_ref(),
        Some(&SandboxLimits::default().fuel)
    );
    assert!(w.accept_merge(&preview).unwrap().already_accepted);
}

#[cfg(windows)]
#[test]
fn wasm_conflict_receipt_preserves_projection_and_failed_budget_changes_nothing() {
    use layrs_weave::wasm_host::SandboxLimits;
    let f = Fixture::new();
    let w = f.workspace();
    let source = branch(&f, &w);
    f.write("code.ts", b"OTHER\nb\nc\n");
    w.capture().unwrap();
    let before = w.state().unwrap();
    let low = wasm_host(SandboxLimits {
        fuel: 1,
        ..SandboxLimits::default()
    });
    assert!(w
        .preview_sandboxed_merge(
            std::slice::from_ref(&source),
            &layer(&w),
            policy(),
            GraphMergeLimits::default(),
            &low
        )
        .is_err());
    assert_eq!(w.state().unwrap().operation.id(), before.operation.id());
    let host = wasm_host(SandboxLimits::default());
    let preview = w
        .preview_sandboxed_merge(
            &[source],
            &layer(&w),
            policy(),
            GraphMergeLimits::default(),
            &host,
        )
        .unwrap();
    let accepted = w.accept_merge(&preview).unwrap();
    assert!(!accepted.materialized);
    assert_eq!(accepted.conflict_ids.len(), 1);
    assert_eq!(fs::read(f.0.join("code.ts")).unwrap(), b"OTHER\nb\nc\n");
    let store = scm_v1::open(w.store_root()).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let product = reader
        .get_product(&accepted.product_object_id)
        .unwrap()
        .unwrap();
    let ProductEnvelope::Weave { record, .. } = product.envelope() else {
        panic!("Weave expected")
    };
    assert_eq!(
        record.lens_executions[0].conflict_id.as_ref(),
        Some(&accepted.conflict_ids[0])
    );
}

#[test]
fn clean_acceptance_materializes_and_persists_canonical_receipt_with_replay() {
    let f = Fixture::new();
    let w = f.workspace();
    let preview = clean(&f, &w);
    let accepted = w.accept_merge(&preview).unwrap();
    assert!(accepted.materialized);
    assert!(!accepted.already_accepted);
    assert_eq!(fs::read(f.0.join("code.ts")).unwrap(), b"A\nb\nc\n");
    assert_eq!(fs::read(f.0.join("b")).unwrap(), b"new\n");
    let state = w.state().unwrap();
    assert_eq!(state.operation.kind(), OperationKind::Weave);
    assert_eq!(head(&w), accepted.step_id);
    let store = scm_v1::open(w.store_root()).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert_eq!(
        reader.products_in_space(state.space.id()).unwrap(),
        std::slice::from_ref(&accepted.product_object_id)
    );
    let product = reader
        .get_product(&accepted.product_object_id)
        .unwrap()
        .unwrap();
    let ProductEnvelope::Weave { record, .. } = product.envelope() else {
        panic!("Weave expected")
    };
    assert_eq!(record.result_step_id, accepted.step_id);
    assert_eq!(record.operation_id, accepted.operation_id);
    assert!(record.lens_executions.is_empty());
    assert_eq!(
        record.determinism_profile.host_policy.id,
        "layrs.no-lens-execution"
    );
    let mut forged = product.envelope().clone();
    if let ProductEnvelope::Weave { record, .. } = &mut forged {
        record.weave_id = ObjectId::parse(format!("blake3:{}", "0".repeat(64))).unwrap();
    }
    let forged_id = forged.computed_object_id().unwrap();
    if let ProductEnvelope::Weave {
        product_object_id, ..
    } = &mut forged
    {
        *product_object_id = forged_id;
    }
    assert!(CanonicalProduct::from_envelope(forged).is_err());
    let retry = w.accept_merge(&preview).unwrap();
    assert!(retry.already_accepted);
    assert_eq!(retry.operation_id, accepted.operation_id);
}

#[test]
fn structural_conflict_keeps_honest_old_projection_and_persists_variants() {
    let f = Fixture::new();
    let w = f.workspace();
    let source = branch(&f, &w);
    f.write("code.ts", b"OTHER\nb\nc\n");
    w.capture().unwrap();
    let before = w.state().unwrap();
    let old = before
        .view
        .worktrees()
        .iter()
        .find(|s| s.worktree_id() == w.worktree_id())
        .unwrap();
    let preview = w
        .preview_structural_merge(&[source], &layer(&w), policy(), GraphMergeLimits::default())
        .unwrap();
    let accepted = w.accept_merge(&preview).unwrap();
    assert!(!accepted.materialized);
    assert_eq!(accepted.conflict_ids.len(), 1);
    assert_eq!(fs::read(f.0.join("code.ts")).unwrap(), b"OTHER\nb\nc\n");
    let state = w.state().unwrap();
    let current = state
        .view
        .worktrees()
        .iter()
        .find(|s| s.worktree_id() == w.worktree_id())
        .unwrap();
    assert_eq!(current.status(), WorktreeStatus::Conflicted);
    assert_eq!(current.base_step_id(), old.base_step_id());
    assert_eq!(current.materialized_tree_id(), old.materialized_tree_id());
    assert_eq!(
        state
            .view
            .layer_heads()
            .iter()
            .find(|h| h.layer_id() == &layer(&w))
            .unwrap()
            .step_ids(),
        [accepted.step_id]
    );
    let store = scm_v1::open(w.store_root()).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert!(reader
        .get_conflict(&accepted.conflict_ids[0])
        .unwrap()
        .is_some());
    assert!(reader
        .get_product(&accepted.product_object_id)
        .unwrap()
        .is_some());
}

#[test]
fn actual_lens_execution_and_dirty_checkout_are_not_accepted() {
    let f = Fixture::new();
    let w = f.workspace();
    let source = branch(&f, &w);
    f.write("code.ts", b"a\nb\nC\n");
    w.capture().unwrap();
    let preview = w
        .preview_merge(&[source], &layer(&w), policy(), GraphMergeLimits::default())
        .unwrap();
    let old = w.state().unwrap().operation.id().clone();
    assert!(w.accept_merge(&preview).is_err());
    assert_eq!(w.state().unwrap().operation.id(), &old);
    let f = Fixture::new();
    let w = f.workspace();
    let preview = clean(&f, &w);
    let old = w.state().unwrap().operation.id().clone();
    f.write("code.ts", b"uncommitted\n");
    assert!(w.accept_merge(&preview).is_err());
    assert_eq!(w.state().unwrap().operation.id(), &old);
    assert_eq!(fs::read(f.0.join("code.ts")).unwrap(), b"uncommitted\n");
}

#[test]
fn graph_only_join_still_rejects_dirty_files() {
    let f = Fixture::new();
    let w = f.workspace();
    let source = branch(&f, &w);
    f.write("code.ts", b"intermediate\n");
    w.capture().unwrap();
    f.write("code.ts", b"A\nb\nc\n");
    w.capture().unwrap();
    let preview = w
        .preview_merge(&[source], &layer(&w), policy(), GraphMergeLimits::default())
        .unwrap();
    let old = w.state().unwrap().operation.id().clone();
    f.write("code.ts", b"uncommitted\n");
    assert!(w.accept_merge(&preview).is_err());
    assert_eq!(w.state().unwrap().operation.id(), &old);
}

struct Fail(MaterializationPoint);
impl MaterializationFaults for Fail {
    fn check(&self, p: MaterializationPoint) -> std::io::Result<()> {
        if p == self.0 {
            Err(std::io::Error::other("injected crash"))
        } else {
            Ok(())
        }
    }
}
#[test]
fn crash_around_atomic_metadata_recovers_and_retries_once() {
    for point in [
        MaterializationPoint::BeforeMetadata,
        MaterializationPoint::AfterMetadata,
    ] {
        let f = Fixture::new();
        let w = f.workspace();
        let preview = clean(&f, &w);
        let old = w.state().unwrap().operation.id().clone();
        assert!(w.accept_merge_with_faults(&preview, &Fail(point)).is_err());
        recover_checkout(w.store_root(), w.worktree_id()).unwrap();
        if point == MaterializationPoint::BeforeMetadata {
            assert_eq!(w.state().unwrap().operation.id(), &old);
        }
        let accepted = w.accept_merge(&preview).unwrap();
        assert_eq!(
            accepted.already_accepted,
            point == MaterializationPoint::AfterMetadata
        );
        assert_eq!(head(&w), accepted.step_id);
        assert_eq!(fs::read(f.0.join("code.ts")).unwrap(), b"A\nb\nc\n");
    }
}

#[test]
fn partial_then_complete_resolution_preserves_ancestry_and_old_conflict() {
    let f = Fixture::new();
    let w = f.workspace();
    w.create_layer("topic").unwrap();
    w.switch_layer("topic").unwrap();
    f.write("code.ts", b"source a\n");
    f.write("b", b"source b\n");
    w.capture().unwrap();
    let source = head(&w);
    w.switch_layer("main").unwrap();
    f.write("code.ts", b"target a\n");
    f.write("b", b"target b\n");
    w.capture().unwrap();
    let old = head(&w);
    let preview = w
        .preview_structural_merge(&[source], &layer(&w), policy(), GraphMergeLimits::default())
        .unwrap();
    let choices = preview
        .plan()
        .conflicts()
        .iter()
        .map(|(id, c)| {
            (
                id.clone(),
                ObjectId::parse(c.document()["bases"][0]["termId"].as_str().unwrap()).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(choices.len(), 2);
    let accepted = w.accept_merge(&preview).unwrap();
    let first = w
        .preview_conflict_resolution(
            &layer(&w),
            &[choices[0].clone()].into_iter().collect(),
            GraphMergeLimits::default(),
        )
        .unwrap();
    let partial = w.accept_conflict_resolution(&first).unwrap();
    assert_eq!(partial.conflict_ids.len(), 1);
    assert!(!partial.materialized);
    assert_eq!(head(&w), old);
    let stale = w
        .preview_conflict_resolution(
            &layer(&w),
            &[choices[1].clone()].into_iter().collect(),
            GraphMergeLimits::default(),
        )
        .unwrap();
    assert_eq!(fs::read(f.0.join("code.ts")).unwrap(), b"target a\n");
    assert_eq!(fs::read(f.0.join("b")).unwrap(), b"target b\n");
    let second = w
        .preview_conflict_resolution(
            &layer(&w),
            &[choices[1].clone()].into_iter().collect(),
            GraphMergeLimits::default(),
        )
        .unwrap();
    let resolved = w.accept_conflict_resolution(&second).unwrap();
    assert!(resolved.conflict_ids.is_empty());
    assert!(resolved.materialized);
    assert_eq!(head(&w), resolved.step_id);
    assert!(w.accept_conflict_resolution(&stale).is_err());
    assert_eq!(fs::read(f.0.join("code.ts")).unwrap(), b"a\nb\nc\n");
    assert_eq!(fs::read(f.0.join("b")).unwrap(), b"old\n");
    assert_eq!(
        w.state().unwrap().operation.kind(),
        OperationKind::ResolveConflict
    );
    assert!(
        w.accept_conflict_resolution(&second)
            .unwrap()
            .already_accepted
    );
    let store = scm_v1::open(w.store_root()).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert_eq!(
        reader
            .get_step(&resolved.step_id)
            .unwrap()
            .unwrap()
            .parent_step_ids(),
        std::slice::from_ref(&partial.step_id)
    );
    assert_eq!(
        reader
            .get_step(&partial.step_id)
            .unwrap()
            .unwrap()
            .parent_step_ids(),
        [accepted.step_id]
    );
    for (id, _) in choices {
        assert!(reader.get_conflict(&id).unwrap().is_some());
    }
    assert_eq!(
        reader
            .products_in_space(w.state().unwrap().space.id())
            .unwrap(),
        [accepted.product_object_id]
    );
    drop(reader);
    drop(store);
    assert!(
        scm_v1::fsck_existing(w.store_root(), scm_v1::FsckOptions::default())
            .unwrap()
            .is_clean()
    );
}

#[test]
fn store_refuses_a_local_weave_operation_without_canonical_receipt() {
    let f = Fixture::new();
    let w = f.workspace();
    let state = w.state().unwrap();
    let stamp = MutationStamp::fresh().unwrap();
    let operation = Operation::new(
        stamp.operation_id,
        state.space.id().clone(),
        vec![state.operation.id().clone()],
        state.view.id().clone(),
        OperationKind::Weave,
        "unreceipted Weave",
        vec![],
        None,
        None,
        stamp.timestamp,
        Metadata::empty(),
    )
    .unwrap();
    let batch = WriteBatch::new(
        stamp.transaction_id,
        vec![],
        vec![EntityRecord::Operation(operation)],
        vec![],
        vec![],
    )
    .unwrap();
    let mut repository = scm_v1::NativeScmRepository::new(w.store_root());
    assert!(repository.apply_batch(&batch).is_err());
    assert_eq!(w.state().unwrap().operation.id(), state.operation.id());
}

#[test]
fn undo_and_redo_conflicted_weave_preserve_projection_and_reject_local_edits() {
    let f = Fixture::new();
    let w = f.workspace();
    let source = branch(&f, &w);
    f.write("code.ts", b"target\n");
    w.capture().unwrap();
    let before = w.state().unwrap().view.clone();
    let preview = w
        .preview_structural_merge(&[source], &layer(&w), policy(), GraphMergeLimits::default())
        .unwrap();
    w.accept_merge(&preview).unwrap();
    let conflicted = w.state().unwrap().view.clone();
    f.write("code.ts", b"local edits\n");
    assert!(w.undo().is_err());
    assert_eq!(fs::read(f.0.join("code.ts")).unwrap(), b"local edits\n");
    f.write("code.ts", b"target\n");
    w.undo().unwrap();
    assert_eq!(w.state().unwrap().view, before);
    assert_eq!(fs::read(f.0.join("code.ts")).unwrap(), b"target\n");
    w.redo(None).unwrap();
    assert_eq!(w.state().unwrap().view, conflicted);
    assert_eq!(fs::read(f.0.join("code.ts")).unwrap(), b"target\n");
}
#[test]
fn manual_binary_then_empty_replacements_are_atomic_partial_stale_safe_and_replayable() {
    use layrs_weave::graph_merge::ConflictReplacement;
    use std::collections::BTreeMap;
    let f = Fixture::new();
    let w = f.workspace();
    w.create_layer("topic").unwrap();
    w.switch_layer("topic").unwrap();
    f.write("code.ts", b"source a\n");
    f.write("b", b"source b\n");
    w.capture().unwrap();
    let source = head(&w);
    w.switch_layer("main").unwrap();
    f.write("code.ts", b"target a\n");
    f.write("b", b"target b\n");
    w.capture().unwrap();
    let preview = w
        .preview_structural_merge(&[source], &layer(&w), policy(), GraphMergeLimits::default())
        .unwrap();
    let entries = preview
        .plan()
        .trees()
        .get(preview.plan().step().root_tree_id())
        .unwrap()
        .entries();
    let conflict = |name: &str| {
        entries
            .iter()
            .find(|entry| entry.name().as_str() == name)
            .unwrap()
            .target()
            .conflict_id()
            .unwrap()
            .clone()
    };
    let code = conflict("code.ts");
    let other = conflict("b");
    let accepted = w.accept_merge(&preview).unwrap();
    let before = w.state().unwrap();
    let binary = vec![0, 255, 13, 10, 1];
    let replacements = BTreeMap::from([(
        code.clone(),
        ConflictReplacement {
            bytes: binary.clone(),
            executable: cfg!(unix),
        },
    )]);
    assert!(w
        .preview_conflict_replacements(
            &layer(&w),
            &replacements,
            GraphMergeLimits {
                max_input_bytes: 1,
                ..GraphMergeLimits::default()
            }
        )
        .is_err());
    let unknown = ConflictId::parse(format!("blake3:{}", "0".repeat(64))).unwrap();
    assert!(w
        .preview_conflict_replacements(
            &layer(&w),
            &BTreeMap::from([(
                unknown,
                ConflictReplacement {
                    bytes: vec![],
                    executable: false
                }
            )]),
            GraphMergeLimits::default()
        )
        .is_err());
    assert_eq!(w.state().unwrap().operation.id(), before.operation.id());
    let empty = BTreeMap::from([(
        other.clone(),
        ConflictReplacement {
            bytes: vec![],
            executable: false,
        },
    )]);
    let stale = w
        .preview_conflict_replacements(&layer(&w), &empty, GraphMergeLimits::default())
        .unwrap();
    let first = w
        .preview_conflict_replacements(&layer(&w), &replacements, GraphMergeLimits::default())
        .unwrap();
    let partial = w.accept_conflict_resolution(&first).unwrap();
    assert_eq!(partial.conflict_ids, [other]);
    assert!(!partial.materialized);
    assert_eq!(fs::read(f.0.join("code.ts")).unwrap(), b"target a\n");
    assert!(w.accept_conflict_resolution(&stale).is_err());
    let second = w
        .preview_conflict_replacements(&layer(&w), &empty, GraphMergeLimits::default())
        .unwrap();
    let completed = w.accept_conflict_resolution(&second).unwrap();
    assert!(completed.materialized);
    assert!(completed.conflict_ids.is_empty());
    assert_eq!(fs::read(f.0.join("code.ts")).unwrap(), binary);
    assert_eq!(fs::read(f.0.join("b")).unwrap(), b"");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(
            fs::metadata(f.0.join("code.ts"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
    }
    assert!(
        w.accept_conflict_resolution(&second)
            .unwrap()
            .already_accepted
    );
    assert!(!w.is_dirty().unwrap());
    let store = scm_v1::open(w.store_root()).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert_eq!(
        reader
            .get_step(&partial.step_id)
            .unwrap()
            .unwrap()
            .parent_step_ids(),
        [accepted.step_id]
    );
    assert_eq!(
        reader
            .get_step(&completed.step_id)
            .unwrap()
            .unwrap()
            .parent_step_ids(),
        [partial.step_id]
    );
    assert!(reader.get_conflict(&code).unwrap().is_some());
    drop(reader);
    drop(store);
    assert!(
        scm_v1::fsck_existing(w.store_root(), scm_v1::FsckOptions::default())
            .unwrap()
            .is_clean()
    );
}
