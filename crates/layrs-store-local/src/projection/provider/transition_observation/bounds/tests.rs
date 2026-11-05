//! In-memory namespace fences; no SQL, native handles or filesystem fixtures.
use super::*;
use crate::projection::provider::recovery::error::ProviderBootstrapCause;
use layrs_core::{
    projection::ProjectionEntry,
    scm::{Chunk, ChunkingDescriptor, FileManifest, StepId, TreeId},
};

const TOKEN: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";

fn snapshot(paths: impl Iterator<Item = RepoPath>) -> ProjectionSnapshot {
    ProjectionSnapshot {
        schema_version: "layrs.projection-snapshot.v1".into(),
        base_step_id: StepId::parse(TOKEN).unwrap(),
        base_root_tree_id: TreeId::parse(TOKEN).unwrap(),
        observed_policy_set_id: ObjectId::parse(TOKEN).unwrap(),
        observed_authorization_epoch: 1,
        path_semantics: PathSemantics::PortableWindows,
        entries: paths
            .map(|path| ProjectionEntry {
                path,
                target: None,
                executable: false,
                writable: true,
                state: EntryState::Absent,
            })
            .collect(),
    }
}

fn full_namespace() -> (ProjectionSnapshot, Vec<DeniedPathData>) {
    let width = MAX_PATH_BYTES / MAX_ROWS;
    let remainder = MAX_PATH_BYTES % MAX_ROWS;
    let mut paths = (0..MAX_ROWS).map(|index| {
        let length = width + usize::from(index < remainder);
        RepoPath::parse(format!("{index:06}-{}", "x".repeat(length - 7))).unwrap()
    });
    let visible = snapshot(paths.by_ref().take(MAX_ROWS / 2));
    let denied = paths.map(|path| DeniedPathData { path }).collect();
    (visible, denied)
}

#[test]
fn visible_and_denied_share_exact_count_and_byte_boundaries() {
    let (visible, mut denied) = full_namespace();
    assert_eq!(visible.entries.len() + denied.len(), MAX_ROWS);
    assert_eq!(
        visible
            .entries
            .iter()
            .map(|entry| entry.path.as_str().len())
            .chain(denied.iter().map(|row| row.path.as_str().len()))
            .sum::<usize>(),
        MAX_PATH_BYTES
    );
    PathBudget::default()
        .snapshot_with_denied(&visible, &denied, None)
        .unwrap();

    denied.push(DeniedPathData {
        path: RepoPath::parse("zz-extra").unwrap(),
    });
    let error = PathBudget::default()
        .snapshot_with_denied(&visible, &denied, None)
        .unwrap_err();
    assert!(error.to_string().contains("path count exceeds bound"));

    denied.pop().unwrap();
    let last = denied.last_mut().unwrap();
    last.path = RepoPath::parse(format!("{}x", last.path.as_str())).unwrap();
    let error = PathBudget::default()
        .snapshot_with_denied(&visible, &denied, None)
        .unwrap_err();
    assert!(error.to_string().contains("path bytes exceed bound"));
}

#[test]
fn source_selection_and_descriptor_copies_keep_independent_namespace_allowances() {
    let (visible, denied) = full_namespace();
    let selection = ProjectionSelection::Selected {
        paths: visible
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect(),
        subtrees: vec![],
    };
    let descriptor = ProviderSourceDescriptor {
        path: visible.entries[0].path.clone(),
        manifest: FileManifest::from_chunks(
            ChunkingDescriptor::single(),
            &[Chunk::new(b"descriptor".to_vec()).unwrap()],
        )
        .unwrap(),
    };
    let mut budget = PathBudget::default();
    // Match the normalizer's call order. Copies across these categories must
    // not consume the fresh namespace's 100k/8MiB allowance a second time.
    budget.snapshot(&visible, None).unwrap();
    budget.selection(&selection, None).unwrap();
    budget
        .snapshot_with_denied(&visible, &denied, None)
        .unwrap();
    budget.selection(&selection, None).unwrap();
    budget.descriptors(&[descriptor], None).unwrap();
}

#[test]
fn shared_namespace_cancellation_precedes_count_and_index_work() {
    let visible = snapshot(std::iter::once(RepoPath::parse("visible").unwrap()));
    let denied: Vec<_> = (0..MAX_ROWS)
        .map(|_| DeniedPathData {
            path: RepoPath::parse("denied").unwrap(),
        })
        .collect();
    let cancel = Cancellation::default();
    cancel.cancel();
    let mut budget = PathBudget::default();
    assert!(matches!(
        budget.snapshot_with_denied(&visible, &denied, Some(&cancel)),
        Err(ProviderBootstrapCause::Vfs(layrs_vfs::Error::Cancelled))
    ));
    assert_eq!(budget.index_upper, 0);
}

#[test]
fn index_work_budget_still_accumulates_across_categories() {
    let deep = std::iter::repeat_n("x".repeat(31), 128)
        .collect::<Vec<_>>()
        .join("/");
    let visible = snapshot(std::iter::once(RepoPath::parse(deep).unwrap()));
    let mut budget = PathBudget::default();
    budget.snapshot(&visible, None).unwrap();
    let error = (0..100)
        .find_map(|_| budget.snapshot_with_denied(&visible, &[], None).err())
        .expect("repeated categories must retain the shared index-work fence");
    assert!(
        error
            .to_string()
            .contains("path index footprint exceeds bound")
    );
}
