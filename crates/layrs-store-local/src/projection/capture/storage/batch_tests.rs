//! Origin-only DATA interpretation, never an admission or native byte proof.
use super::*;
use crate::projection::provider::owned_test as child;
use layrs_core::{
    projection::{PathSemantics, ProjectionEntry, ProjectionSelection},
    scm::{ObjectId, SpaceId, StepId, StoreId, TreeId, WorktreeId},
};

fn fixture() -> (Connection, ProjectionIdentity, VersionedProjection) {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(database::SQL).unwrap();
    c.execute_batch(selection::SQL).unwrap();
    let id = format!("blake3:{}", "1".repeat(64));
    let snapshot = ProjectionSnapshot {
        schema_version: "layrs.projection-snapshot.v1".into(),
        base_step_id: StepId::parse(&id).unwrap(),
        base_root_tree_id: TreeId::parse(&id).unwrap(),
        observed_policy_set_id: ObjectId::parse(&id).unwrap(),
        observed_authorization_epoch: 1,
        path_semantics: PathSemantics::PortableWindows,
        entries: vec![
            ProjectionEntry {
                path: RepoPath::parse("allowed.txt").unwrap(),
                target: None,
                executable: false,
                writable: true,
                state: EntryState::Absent,
            },
            ProjectionEntry {
                path: RepoPath::parse("blocked.txt").unwrap(),
                target: Some(TreeEntryTarget::file(FileId::from_bytes(b"denied"))),
                executable: false,
                writable: false,
                state: EntryState::Reserved,
            },
        ],
    };
    let payload = snapshot.canonical_bytes().unwrap();
    let digest = database::digest(0, None, &payload);
    c.execute(
        "INSERT INTO projection_snapshots VALUES(0,?1,NULL,?2)",
        params![digest, payload],
    )
    .unwrap();
    selection::insert(&c, 0, &ProjectionSelection::All).unwrap();
    let identity = ProjectionIdentity {
        store_id: StoreId::parse("store:batch-data").unwrap(),
        worktree_id: WorktreeId::parse("worktree:batch-data").unwrap(),
        space_id: SpaceId::parse("space:batch-data").unwrap(),
        origin: "https://example.test".into(),
        principal_id: "principal:batch-data".into(),
    };
    c.execute_batch("BEGIN IMMEDIATE").unwrap();
    (
        c,
        identity,
        VersionedProjection {
            generation: 0,
            digest,
            snapshot,
        },
    )
}

fn changes(path: &str, bytes: &[u8]) -> Vec<CapturedChange> {
    vec![CapturedChange {
        path: RepoPath::parse(path).unwrap(),
        expected: None,
        replacement: Some(CapturedFile {
            file_id: FileId::from_bytes(bytes),
            executable: false,
            bytes: bytes.len() as u64,
        }),
    }]
}

fn append(
    c: &Connection,
    identity: &ProjectionIdentity,
    state: &VersionedProjection,
    changes: &[CapturedChange],
) -> CapturedPatch {
    let payload = local_capture_append_payload(c, state, identity, changes)
        .unwrap()
        .unwrap();
    let patch: CapturedPatch = serde_json::from_slice(&payload).unwrap();
    c.execute(
        "INSERT INTO projection_captures VALUES(?1,?2,?3,?4)",
        params![
            patch.sequence,
            patch.digest,
            patch.projection_generation,
            payload
        ],
    )
    .unwrap();
    patch
}

#[test]
#[ignore = "UNQUALIFIED origin Capture batch SQL DATA; ROOT review/release child120 required"]
fn fixed_origin_batch_preserves_chain_changes_and_exact_dedup_planning() {
    child::run(
        concat!(
            module_path!(),
            "::fixed_origin_batch_preserves_chain_changes_and_exact_dedup_planning"
        ),
        120,
        || {
            child::declare();
            let (c, identity, state) = fixture();
            let first_changes = changes("allowed.txt", b"first");
            let first = append(&c, &identity, &state, &first_changes);
            assert!(
                local_capture_append_payload(&c, &state, &identity, &first_changes)
                    .unwrap()
                    .is_none()
            );
            let second = append(&c, &identity, &state, &changes("allowed.txt", b"second"));
            assert_eq!(second.previous_patch_digest.as_ref(), Some(&first.digest));
            let mut observed = Vec::new();
            visit_metadata_range_for_origin(
                &c,
                1,
                2,
                &identity,
                0,
                &state.digest,
                || Ok(()),
                |patch| {
                    observed.push(patch.clone());
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(observed, vec![first, second]);
            assert!(!c.is_autocommit());
            c.execute_batch("ROLLBACK").unwrap();
        },
    );
}

#[test]
#[ignore = "UNQUALIFIED origin Capture batch SQL refusals; ROOT review/release child120 required"]
fn fixed_origin_batch_refuses_chain_identity_generation_and_reserved_changes() {
    child::run(
        concat!(
            module_path!(),
            "::fixed_origin_batch_refuses_chain_identity_generation_and_reserved_changes"
        ),
        120,
        || {
            child::declare();
            let (c, identity, state) = fixture();
            append(&c, &identity, &state, &changes("allowed.txt", b"first"));
            let mut second = append(&c, &identity, &state, &changes("allowed.txt", b"second"));
            let mut foreign = identity.clone();
            foreign.worktree_id = WorktreeId::parse("worktree:other").unwrap();
            let next_payload = state.snapshot.canonical_bytes().unwrap();
            let next_digest = database::digest(1, Some(&state.digest), &next_payload);
            c.execute(
                "INSERT INTO projection_snapshots VALUES(1,?1,?2,?3)",
                params![next_digest, state.digest, next_payload],
            )
            .unwrap();
            selection::insert(&c, 1, &ProjectionSelection::All).unwrap();
            for (owner, generation, digest) in
                [(&foreign, 0, &state.digest), (&identity, 1, &next_digest)]
            {
                let error = visit_metadata_range_for_origin(
                    &c,
                    1,
                    2,
                    owner,
                    generation,
                    digest,
                    || Ok(()),
                    |_| Ok(()),
                )
                .unwrap_err();
                assert!(
                    error.to_string().contains("verified origin range"),
                    "{error}"
                );
            }
            second.previous_patch_digest = Some(format!("blake3:{}", "2".repeat(64)));
            second.digest = second.computed_digest().unwrap();
            c.execute(
                "UPDATE projection_captures SET digest=?1,payload=?2 WHERE sequence=2",
                params![second.digest, canonical_json(&second).unwrap()],
            )
            .unwrap();
            let error = visit_metadata_range_for_origin(
                &c,
                1,
                2,
                &identity,
                0,
                &state.digest,
                || Ok(()),
                |_| Ok(()),
            )
            .unwrap_err();
            assert!(error.to_string().contains("verified origin range"));
            c.execute("DELETE FROM projection_captures", []).unwrap();
            append(
                &c,
                &identity,
                &state,
                &changes("blocked.txt", b"not authorized"),
            );
            let error = visit_metadata_range_for_origin(
                &c,
                1,
                1,
                &identity,
                0,
                &state.digest,
                || Ok(()),
                |_| Ok(()),
            )
            .unwrap_err();
            assert!(error.to_string().contains("reserved"));
            assert!(!c.is_autocommit());
            c.execute_batch("ROLLBACK").unwrap();
        },
    );
}

#[test]
#[ignore = "UNQUALIFIED forced Restore append planning SQL DATA; ROOT review/release child120 required"]
fn forced_restore_append_never_deduplicates_and_chains_exact_payload() {
    child::run(
        concat!(
            module_path!(),
            "::forced_restore_append_never_deduplicates_and_chains_exact_payload"
        ),
        120,
        || {
            child::declare();
            let (c, identity, state) = fixture();
            let first_changes = changes("allowed.txt", b"first");
            let first = append(&c, &identity, &state, &first_changes);
            // Capture deduplicates the same image; Restore must still append.
            assert!(
                local_capture_append_payload(&c, &state, &identity, &first_changes)
                    .unwrap()
                    .is_none()
            );
            let payload =
                local_restore_append_payload(&c, &state, &identity, &first_changes).unwrap();
            let forced: CapturedPatch = serde_json::from_slice(&payload).unwrap();
            assert_eq!(forced.sequence, first.sequence + 1);
            assert_eq!(forced.previous_patch_digest.as_ref(), Some(&first.digest));
            assert_eq!(forced.changes, first.changes);
            assert_eq!(forced.identity, identity);
            assert_eq!(forced.projection_digest, state.digest);
            assert_eq!(forced.digest, forced.computed_digest().unwrap());
            assert_eq!(canonical_json(&forced).unwrap(), payload);
            // A different image plans the identical bytes either way.
            let second_changes = changes("allowed.txt", b"second");
            assert_eq!(
                local_capture_append_payload(&c, &state, &identity, &second_changes)
                    .unwrap()
                    .unwrap(),
                local_restore_append_payload(&c, &state, &identity, &second_changes).unwrap()
            );
            c.execute_batch("ROLLBACK").unwrap();
        },
    );
}