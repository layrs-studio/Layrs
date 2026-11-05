//! Selection adapters retain the existing authenticated remote mutation engine.
use super::{NativeRemote, remote_error};
use layrs_core::projection::{PathSemantics, ProjectionSelection};
use layrs_store_local::projection::ProjectionStore;
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn describe(store: &ProjectionStore, status: &str) -> Result<Value, String> {
    let (projection, selection) = store.state_with_selection().map_err(|e| e.to_string())?;
    Ok(
        json!({"status":status,"identity":store.identity(),"projection":projection,"selection":selection}),
    )
}

pub(super) fn expand(
    remote: &NativeRemote,
    root: &Path,
    expected_generation: u64,
    additions: &ProjectionSelection,
) -> Result<Value, String> {
    let store = ProjectionStore::open(root).map_err(|e| format!("projection_store_error: {e}"))?;
    let (before, selection) = store.state_with_selection().map_err(|e| e.to_string())?;
    if before.generation != expected_generation {
        return Err("selection expansion generation changed".into());
    }
    let selection = union(&selection, additions, before.snapshot.path_semantics)?;
    let projection = remote
        .expand_projection_selection(&store, expected_generation, &selection)
        .map_err(remote_error)?;
    Ok(
        json!({"status":"projectionSelectionExpanded","identity":store.identity(),"projection":projection,"selection":selection}),
    )
}

fn union(
    existing: &ProjectionSelection,
    additions: &ProjectionSelection,
    semantics: PathSemantics,
) -> Result<ProjectionSelection, String> {
    let additions = additions.normalized(semantics).map_err(|e| e.to_string())?;
    match (existing, additions) {
        (ProjectionSelection::All, _) | (_, ProjectionSelection::All) => {
            Ok(ProjectionSelection::All)
        }
        (
            ProjectionSelection::Selected { paths, subtrees },
            ProjectionSelection::Selected {
                paths: new_paths,
                subtrees: new_subtrees,
            },
        ) => ProjectionSelection::selected(
            paths.iter().cloned().chain(new_paths).collect(),
            subtrees.iter().cloned().chain(new_subtrees).collect(),
            semantics,
        )
        .map_err(|e| e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_scm::NativeRemoteAction;
    use layrs_core::scm::RepoPath;

    fn selected(paths: &[&str], subtrees: &[&str]) -> ProjectionSelection {
        ProjectionSelection::selected(
            paths
                .iter()
                .map(|path| RepoPath::parse(*path).unwrap())
                .collect(),
            subtrees
                .iter()
                .map(|path| RepoPath::parse(*path).unwrap())
                .collect(),
            PathSemantics::PortableWindows,
        )
        .unwrap()
    }

    #[test]
    fn expansion_retains_old_intent_and_normalizes_added_subtrees() {
        let before = selected(&["README.md", "src/main.rs"], &["assets"]);
        let additions = selected(&["docs/help.md"], &["src"]);
        let expanded = union(&before, &additions, PathSemantics::PortableWindows).unwrap();
        assert!(expanded.is_expansion_of(&before));
        assert_eq!(
            expanded,
            selected(&["README.md", "docs/help.md"], &["assets", "src"])
        );
        assert_eq!(
            union(
                &expanded,
                &selected(&[], &[]),
                PathSemantics::PortableWindows
            )
            .unwrap(),
            expanded
        );
        assert_eq!(
            union(
                &ProjectionSelection::All,
                &additions,
                PathSemantics::PortableWindows
            )
            .unwrap(),
            ProjectionSelection::All
        );
        assert_eq!(
            union(
                &before,
                &ProjectionSelection::All,
                PathSemantics::PortableWindows
            )
            .unwrap(),
            ProjectionSelection::All
        );
        assert!(
            union(
                &before,
                &selected(&["readme.md"], &[]),
                PathSemantics::PortableWindows
            )
            .is_err()
        );
    }

    #[test]
    fn remote_contract_is_additive_and_rejects_unrecognized_or_incomplete_requests() {
        let step = format!("blake3:{}", "1".repeat(64));
        let legacy: NativeRemoteAction = serde_json::from_value(
            json!({"action":"cloneProjection","space":"space:test","step":step}),
        )
        .unwrap();
        assert!(matches!(legacy, NativeRemoteAction::CloneProjection { .. }));
        let selected: NativeRemoteAction = serde_json::from_value(json!({"action":"cloneProjectionSelected","space":"space:test","step":step,"selection":{"kind":"selected","paths":["src/main.rs"],"subtrees":[]}})).unwrap();
        assert!(matches!(
            selected,
            NativeRemoteAction::CloneProjectionSelected { .. }
        ));
        let expansion: NativeRemoteAction = serde_json::from_value(json!({"action":"expandProjectionSelection","expectedGeneration":5,"selection":{"kind":"all"}})).unwrap();
        assert!(matches!(
            expansion,
            NativeRemoteAction::ExpandProjectionSelection {
                expected_generation: 5,
                ..
            }
        ));
        for request in [
            json!({"action":"expandProjectionSelection","selection":{"kind":"all"}}),
            json!({"action":"expandProjectionSelection","expectedGeneration":5}),
            json!({"action":"expandProjectionSelection","expectedGeneration":5,"selection":{"kind":"selected","paths":["../private"],"subtrees":[]}}),
            json!({"action":"expandProjectionSelection","expectedGeneration":5,"selection":{"kind":"all"},"evict":true}),
            json!({"action":"cloneProjection","space":"space:test","step":step,"selection":{"kind":"all"}}),
        ] {
            assert!(
                serde_json::from_value::<NativeRemoteAction>(request.clone()).is_err(),
                "accepted {request}"
            );
        }
    }
}
