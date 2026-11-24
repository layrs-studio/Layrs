//! Store format 8, draft 12 (council s36, B): a history record's index key
//! keeps 16 bytes of its derivation. Ids whose keys collide (forced here by
//! keeping one byte) are never merged and never served for one another: a
//! batch carrying an id whose key a stored record already has is refused,
//! two colliding ids in one batch are refused, and a read of a refused id
//! never answers the stored record. Its own test binary: the hook holds for
//! the whole process.
use std::path::{Path, PathBuf};

use layrs_core::scm::{
    CaptureReason, Layer, LayerHeadSet, LayerId, LayerKind, LayerSettings, LayerStatus, Metadata,
    ObservationId, Operation, OperationId, OperationKind, RepositoryView, RepositoryViewId, Space,
    SpaceId, Step, StepId, StepObservation, StoreId, Timestamp, Tree,
};
use layrs_store::scm::{
    CanonicalObject, EntityRecord, OperationHeadsCas, RepositoryViewCas, ScmReader, ScmRepository,
    SpaceObjectAdmission, StoreMutation, TransactionId, WriteBatch,
};
use layrs_store_local::scm_v1::{
    FsckOptions, MaintenanceOutcome, NativeScmReader, NativeScmRepository, fsck_existing, init,
    maintain_explicit, open, set_history_key_bytes,
};

struct TestRoot(PathBuf);

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn timestamp() -> Timestamp {
    Timestamp::parse("2026-11-24T00:00:00Z").unwrap()
}

/// A Space with one empty Step: its Initialize batch.
fn initialize(space: &SpaceId) -> (StepId, RepositoryViewId, WriteBatch) {
    let space_record = Space::new(space.clone(), "Space", timestamp(), Metadata::empty()).unwrap();
    let layer = Layer::new(
        LayerId::parse("layer:collision").unwrap(),
        space.clone(),
        "Main",
        LayerKind::Base,
        LayerStatus::Active,
        vec![],
        LayerSettings::new(true, 100).unwrap(),
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let tree = Tree::new(vec![]).unwrap();
    let step = Step::new(vec![], tree.id().clone()).unwrap();
    let view = RepositoryView::new(
        space.clone(),
        vec![LayerHeadSet::new(layer.id().clone(), vec![step.id().clone()]).unwrap()],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    let operation = Operation::new(
        OperationId::parse("operation:collision-0").unwrap(),
        space.clone(),
        vec![],
        view.id().clone(),
        OperationKind::Initialize,
        "Initialize",
        vec![],
        None,
        None,
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let objects = vec![
        CanonicalObject::Tree(tree),
        CanonicalObject::Step(step.clone()),
        CanonicalObject::RepositoryView(view.clone()),
    ];
    let admissions = objects
        .iter()
        .map(|object| SpaceObjectAdmission::new(space.clone(), object.object_id()))
        .collect();
    let batch = WriteBatch::new(
        TransactionId::new("transaction:collision-0").unwrap(),
        objects,
        vec![
            EntityRecord::Space(space_record),
            EntityRecord::Layer(layer),
            EntityRecord::Operation(operation.clone()),
        ],
        admissions,
        vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(space.clone(), 0, None, view.id().clone()).unwrap(),
            ),
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(
                    space.clone(),
                    vec![],
                    operation.id().clone(),
                    vec![operation.id().clone()],
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    (step.id().clone(), view.id().clone(), batch)
}

fn observation(id: &str, step: &StepId, space: &SpaceId) -> StepObservation {
    StepObservation::new(
        ObservationId::parse(id).unwrap(),
        step.clone(),
        Some(space.clone()),
        None,
        None,
        None,
        None,
        timestamp(),
        CaptureReason::Received,
        Metadata::empty(),
    )
}

fn observations(transaction: &str, records: Vec<StepObservation>) -> WriteBatch {
    WriteBatch::new(
        TransactionId::new(transaction).unwrap(),
        vec![],
        records
            .into_iter()
            .map(EntityRecord::StepObservation)
            .collect(),
        vec![],
        vec![],
    )
    .unwrap()
}

/// The first byte of a StepObservation's history key (kind 12).
fn first_key_byte(id: &str) -> u8 {
    key_byte(12, id)
}

/// The first byte of a history key of kind `code`.
fn key_byte(code: u8, id: &str) -> u8 {
    blake3::derive_key(
        "layrs history key v1",
        &[&[code][..], id.as_bytes()].concat(),
    )[0]
}

/// An Operation on `parents`, a Capture of the Space's first View.
fn operation(id: &str, space: &SpaceId, view: &RepositoryViewId, parents: &[&str]) -> Operation {
    Operation::new(
        OperationId::parse(id).unwrap(),
        space.clone(),
        parents
            .iter()
            .map(|parent| OperationId::parse(*parent).unwrap())
            .collect(),
        view.clone(),
        OperationKind::CaptureWorktree,
        "Capture",
        vec![],
        None,
        None,
        timestamp(),
        Metadata::empty(),
    )
    .unwrap()
}

/// Three ids whose keys share their first byte, the first one `base`'s.
fn colliding(base: &str) -> Vec<String> {
    let wanted = first_key_byte(base);
    let mut found = vec![base.to_owned()];
    for n in 0.. {
        let id = format!("observation:collide-{n}");
        if first_key_byte(&id) == wanted {
            found.push(id);
            if found.len() == 3 {
                return found;
            }
        }
    }
    unreachable!()
}

fn get(root: &Path, id: &str) -> Result<Option<StepObservation>, String> {
    let store = open(root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    reader
        .get_step_observation(&ObservationId::parse(id).unwrap())
        .map_err(|error| error.to_string())
}

#[test]
fn colliding_history_keys_are_never_merged_nor_served_for_one_another() {
    set_history_key_bytes(Some(1));
    let root = TestRoot(
        std::env::temp_dir().join(format!("layrs-history-collision-{}", std::process::id())),
    );
    let _ = std::fs::remove_dir_all(&root.0);
    // SAFETY: the only test of this binary.
    unsafe { std::env::set_var("LAYRS_STORE_FORMAT", "8") };
    init(&root.0, StoreId::parse("store:collision").unwrap()).unwrap();
    unsafe { std::env::remove_var("LAYRS_STORE_FORMAT") };
    let space = SpaceId::parse("space:collision").unwrap();
    let (step, view, batch) = initialize(&space);
    let mut repository = NativeScmRepository::new(&root.0);
    repository.apply_batch(&batch).unwrap();
    let ids = colliding("observation:first");
    let first = observation(&ids[0], &step, &space);
    repository
        .apply_batch(&observations(
            "transaction:collision-1",
            vec![first.clone()],
        ))
        .unwrap();
    // An id whose key the stored record has: refused, the record kept.
    let second = observation(&ids[1], &step, &space);
    let refused = repository
        .apply_batch(&observations("transaction:collision-2", vec![second]))
        .unwrap_err()
        .to_string();
    assert!(refused.contains("history key collision"), "{refused}");
    // Two colliding ids in one batch: refused.
    let other = |taken: &[u8]| {
        (0..)
            .map(|n| format!("observation:other-{n}"))
            .find(|id| !taken.contains(&first_key_byte(id)))
            .unwrap()
    };
    let pair = colliding(&other(&[first_key_byte(&ids[0])]));
    let refused = repository
        .apply_batch(&observations(
            "transaction:collision-3",
            vec![
                observation(&pair[0], &step, &space),
                observation(&pair[1], &step, &space),
            ],
        ))
        .unwrap_err()
        .to_string();
    assert!(refused.contains("history key collision"), "{refused}");
    // A reference to an id whose key another id's record holds is missing,
    // not found under that key (council s37): an Operation on the stored
    // Initialize's colliding twin is refused, on the Initialize itself it
    // lands.
    let initialize_id = "operation:collision-0";
    let twin = (0..)
        .map(|n| format!("operation:twin-{n}"))
        .find(|id| key_byte(11, id) == key_byte(11, initialize_id))
        .unwrap();
    let child = |n: usize| {
        (n..)
            .map(|n| format!("operation:child-{n}"))
            .find(|id| key_byte(11, id) != key_byte(11, initialize_id))
            .unwrap()
    };
    let on_twin = operation(&child(0), &space, &view, &[&twin]);
    let batch = |transaction: &str, operation: Operation| {
        WriteBatch::new(
            TransactionId::new(transaction).unwrap(),
            vec![],
            vec![EntityRecord::Operation(operation)],
            vec![],
            vec![],
        )
        .unwrap()
    };
    let refused = repository
        .apply_batch(&batch("transaction:collision-5", on_twin))
        .unwrap_err()
        .to_string();
    assert!(refused.contains(&twin), "{refused}");
    let first_child = child(0);
    let second_child = child(
        first_child
            .trim_start_matches("operation:child-")
            .parse::<usize>()
            .unwrap()
            + 1,
    );
    repository
        .apply_batch(&batch(
            "transaction:collision-6",
            operation(&second_child, &space, &view, &[initialize_id]),
        ))
        .unwrap();
    // An id of another key still lands.
    let unrelated = other(&[first_key_byte(&ids[0]), first_key_byte(&pair[0])]);
    let lands = observation(&unrelated, &step, &space);
    repository
        .apply_batch(&observations(
            "transaction:collision-4",
            vec![lands.clone()],
        ))
        .unwrap();
    drop(repository);
    assert_eq!(get(&root.0, &ids[0]).unwrap(), Some(first.clone()));
    assert_eq!(get(&root.0, &unrelated).unwrap(), Some(lands.clone()));
    // A read of a refused id never answers another record.
    for id in [&ids[1], &pair[0], &pair[1]] {
        let answer = get(&root.0, id);
        assert!(!matches!(&answer, Ok(Some(_))), "{id}: {answer:?}");
    }
    // Consolidated into one pack, every stored record keeps its own key.
    assert!(matches!(
        maintain_explicit(&root.0).unwrap(),
        MaintenanceOutcome::Consolidated { .. } | MaintenanceOutcome::NothingToDo
    ));
    assert_eq!(get(&root.0, &ids[0]).unwrap(), Some(first));
    assert_eq!(get(&root.0, &unrelated).unwrap(), Some(lands));
    let report = fsck_existing(&root.0, FsckOptions::default()).unwrap();
    assert!(report.is_clean(), "{:#?}", report.findings);
}
