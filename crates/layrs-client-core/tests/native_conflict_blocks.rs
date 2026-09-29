#![cfg(any(windows, all(target_os = "linux", target_arch = "x86_64")))]
use layrs_client_core::native_scm::*;
use layrs_core::scm::*;
use layrs_store::scm::ScmReader;
use layrs_store_local::scm_v1::{open, NativeScmReader};
use layrs_weave::{
    graph_merge::{ConflictBlockResolution, GraphMergeLimits},
    reconciliation::*,
    wasm_host::*,
};
use std::{collections::BTreeMap, fs, path::PathBuf};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn fixture() -> (Fixture, NativeWorkspace, LayerId, serde_json::Value) {
    let root = std::env::temp_dir().join(format!("layrs-blocks-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    fs::write(root.join("code.ts"), b"zero\r\nsame\r\nzero\r\n").unwrap();
    let w = NativeWorkspace::initialize(&root, "blocks").unwrap();
    w.create_layer("topic").unwrap();
    w.switch_layer("topic").unwrap();
    fs::write(root.join("code.ts"), "é\r\nsame\r\né\r\n").unwrap();
    w.capture().unwrap();
    let source = w
        .state()
        .unwrap()
        .view
        .worktrees()
        .iter()
        .find(|s| s.worktree_id() == w.worktree_id())
        .unwrap()
        .base_step_id()
        .unwrap()
        .clone();
    w.switch_layer("main").unwrap();
    fs::write(root.join("code.ts"), b"B\r\nsame\r\nB\r\n").unwrap();
    w.capture().unwrap();
    let target = w
        .state()
        .unwrap()
        .layers
        .iter()
        .find(|l| l.name() == "main")
        .unwrap()
        .id()
        .clone();
    let executable = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(if cfg!(windows) {
            "layrs-lens-worker.exe"
        } else {
            "layrs-lens-worker"
        });
    let digest = layrs_lens_sdk::sha256_digest(
        &fs::read(&executable).expect("build layrs-lens-worker first"),
    );
    let host = WasmHost::open(&executable, &digest, SandboxLimits::default()).unwrap();
    let policy = FileMergePolicy {
        unsupported: FailureAction::StructuralFallback,
        quota: FailureAction::Reject,
        limits: FileMergeLimits::default(),
    };
    let preview = w
        .preview_sandboxed_merge(
            &[source],
            &target,
            policy,
            GraphMergeLimits::default(),
            &host,
        )
        .unwrap();
    let document = preview
        .plan()
        .conflicts()
        .values()
        .next()
        .unwrap()
        .document()
        .clone();
    assert_eq!(document["blocks"].as_array().unwrap().len(), 2);
    let accepted = w.accept_merge(&preview).unwrap();
    assert!(!accepted.materialized);
    (Fixture(root), w, target, document)
}
fn select(
    document: &serde_json::Value,
    index: usize,
) -> BTreeMap<ConflictId, BTreeMap<ObjectId, ConflictBlockResolution>> {
    let block = &document["blocks"][index];
    BTreeMap::from([(
        ConflictId::parse(document["conflictId"].as_str().unwrap()).unwrap(),
        BTreeMap::from([(
            ObjectId::parse(block["blockId"].as_str().unwrap()).unwrap(),
            ConflictBlockResolution::SelectTerm(
                ObjectId::parse(block["baseValues"][0]["termId"].as_str().unwrap()).unwrap(),
            ),
        )]),
    )])
}

#[test]
fn canonical_blocks_survive_store_reopen_and_final_resolution_is_exact_and_idempotent() {
    let (f, w, target, original) = fixture();
    let preview = w
        .preview_conflict_blocks(&target, &select(&original, 0), GraphMergeLimits::default())
        .unwrap();
    let preview_bytes = preview.bundle().canonical_bytes().unwrap();
    let partial = w.accept_conflict_resolution(&preview).unwrap();
    assert!(!partial.materialized);
    assert_eq!(
        fs::read(f.0.join("code.ts")).unwrap(),
        b"B\r\nsame\r\nB\r\n"
    );
    assert_eq!(preview.bundle().canonical_bytes().unwrap(), preview_bytes);
    assert!(
        w.accept_conflict_resolution(&preview)
            .unwrap()
            .already_accepted
    );
    let w = NativeWorkspace::open(&f.0).unwrap();
    let documents = w.layer_conflicts(&target).unwrap();
    let doc = &documents[0]["document"];
    assert_eq!(doc["resolutionState"], "partial");
    assert_eq!(doc["bases"], original["bases"]);
    assert_eq!(doc["sides"], original["sides"]);
    let preview = w
        .preview_conflict_blocks(&target, &select(doc, 1), GraphMergeLimits::default())
        .unwrap();
    let resolved = w.accept_conflict_resolution(&preview).unwrap();
    assert!(resolved.materialized);
    assert!(resolved.conflict_ids.is_empty());
    assert_eq!(
        fs::read(f.0.join("code.ts")).unwrap(),
        b"zero\r\nsame\r\nzero\r\n"
    );
    let store = open(w.store_root()).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert_eq!(
        reader
            .get_step(&resolved.step_id)
            .unwrap()
            .unwrap()
            .parent_step_ids(),
        [partial.step_id]
    );
    assert!(reader
        .get_conflict(&ConflictId::parse(original["conflictId"].as_str().unwrap()).unwrap())
        .unwrap()
        .is_some());
    assert!(
        w.accept_conflict_resolution(&preview)
            .unwrap()
            .already_accepted
    );
}

#[test]
fn block_preview_rejects_dirty_projection_and_stale_operation_without_capture() {
    let (f, w, target, original) = fixture();
    let first = w
        .preview_conflict_blocks(&target, &select(&original, 0), GraphMergeLimits::default())
        .unwrap();
    let stale = w
        .preview_conflict_blocks(&target, &select(&original, 1), GraphMergeLimits::default())
        .unwrap();
    let before = w.state().unwrap().operation.id().clone();
    fs::write(f.0.join("code.ts"), b"unrecorded exact bytes\0\xff").unwrap();
    assert!(matches!(
        w.accept_conflict_resolution(&first),
        Err(NativeClientError::StaleCapture)
    ));
    assert_eq!(w.state().unwrap().operation.id(), &before);
    assert_eq!(
        fs::read(f.0.join("code.ts")).unwrap(),
        b"unrecorded exact bytes\0\xff"
    );
    fs::write(f.0.join("code.ts"), b"B\r\nsame\r\nB\r\n").unwrap();
    w.accept_conflict_resolution(&first).unwrap();
    assert!(matches!(
        w.accept_conflict_resolution(&stale),
        Err(NativeClientError::StaleCapture)
    ));
}

#[test]
fn manual_block_fragment_is_persisted_before_reopen_and_whole_term_escape_still_works() {
    let (f, w, target, original) = fixture();
    let mut choices = select(&original, 0);
    let choice = choices
        .values_mut()
        .next()
        .unwrap()
        .values_mut()
        .next()
        .unwrap();
    let source = match choice {
        ConflictBlockResolution::SelectTerm(id) => id.clone(),
        _ => unreachable!(),
    };
    *choice = ConflictBlockResolution::Manual {
        bytes: "manuel 🦀\r\n".as_bytes().to_vec(),
        source_term_ids: vec![source],
    };
    let preview = w
        .preview_conflict_blocks(&target, &choices, GraphMergeLimits::default())
        .unwrap();
    w.accept_conflict_resolution(&preview).unwrap();
    let w = NativeWorkspace::open(&f.0).unwrap();
    let docs = w.layer_conflicts(&target).unwrap();
    let doc = &docs[0]["document"];
    let id = FileId::parse(
        doc["blocks"][0]["resolution"]["result"]["objectId"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(id, FileId::from_bytes("manuel 🦀\r\n".as_bytes()));
    // Loading and resolving the remaining block validates the admitted manual
    // fragment closure, even if the caller chooses the whole original instead.
    w.preview_conflict_blocks(&target, &select(doc, 1), GraphMergeLimits::default())
        .unwrap();
    let terms = BTreeMap::from([(
        ConflictId::parse(doc["conflictId"].as_str().unwrap()).unwrap(),
        ObjectId::parse(doc["bases"][0]["termId"].as_str().unwrap()).unwrap(),
    )]);
    let preview = w
        .preview_conflict_resolution(&target, &terms, GraphMergeLimits::default())
        .unwrap();
    w.accept_conflict_resolution(&preview).unwrap();
    assert_eq!(
        fs::read(f.0.join("code.ts")).unwrap(),
        b"zero\r\nsame\r\nzero\r\n"
    );
}

#[test]
fn serialized_block_actions_bind_fragment_term_and_exact_preview_output() {
    let (f, w, _target, doc) = fixture();
    let block = &doc["blocks"][0];
    let input = serde_json::json!({"action":"conflictBlockVariant","layer":"main",
        "conflict":doc["conflictId"],"block":block["blockId"],"term":block["baseValues"][0]["termId"]});
    let before = w.state().unwrap().operation.id().clone();
    let read = execute_workspace(&f.0, serde_json::from_value(input.clone()).unwrap()).unwrap();
    assert_eq!(read["text"], "zero\r\n");
    assert_eq!(read["byteLength"], 6);
    let mut bad = input;
    bad["term"] = block["blockId"].clone();
    assert!(execute_workspace(&f.0, serde_json::from_value(bad).unwrap()).is_err());
    let mut action = serde_json::json!({"action":"previewBlockResolution","layer":"main",
        "conflict":doc["conflictId"],"block":block["blockId"],
        "choice":{"method":"manual","text":"original preview\r\n","sourceTermIds":[block["baseValues"][0]["termId"]]}});
    let preview = execute_workspace(&f.0, serde_json::from_value(action.clone()).unwrap()).unwrap();
    assert_eq!(w.state().unwrap().operation.id(), &before);
    action["action"] = serde_json::json!("applyBlockResolution");
    assert!(
        serde_json::from_value::<WorkspaceAction>(action.clone()).is_err(),
        "apply guards are required"
    );
    action["expectedOperation"] = preview["expectedOperation"].clone();
    action["expectedStep"] = preview["expectedStep"].clone();
    action["choice"]["text"] = serde_json::json!("changed after preview");
    assert!(matches!(
        execute_workspace(&f.0, serde_json::from_value(action.clone()).unwrap()),
        Err(NativeClientError::StaleCapture)
    ));
    assert_eq!(w.state().unwrap().operation.id(), &before);
    action["choice"]["text"] = serde_json::json!("original preview\r\n");
    execute_workspace(&f.0, serde_json::from_value(action.clone()).unwrap()).unwrap();
    assert!(matches!(
        execute_workspace(&f.0, serde_json::from_value(action).unwrap()),
        Err(NativeClientError::StaleCapture)
    ));
}
