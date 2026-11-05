//! Recheck the namespace of a historical observation against its stored rows.
//! This establishes structural DATA only, never current remote authority.
use super::*;

pub(in crate::projection::provider) fn require_stored_namespace(
    snapshot: &ProjectionSnapshot,
    selected: &ProjectionSelection,
    denied_paths: &[DeniedPathData],
    cancel: Option<&Cancellation>,
) -> Result<()> {
    let mut budget = PathBudget::default();
    budget.snapshot_with_denied(snapshot, denied_paths, cancel)?;
    budget.selection(selected, cancel)?;
    if snapshot
        .entries
        .iter()
        .any(|e| e.state == EntryState::Materialized)
    {
        return Err(invalid("stored observation contains placement state"));
    }
    selected
        .validate_snapshot(snapshot)
        .map_err(|e| invalid(e.to_string()))?;
    ReservedPaths::new(
        denied_paths.iter().map(|p| p.path.as_str()),
        snapshot.path_semantics,
    )
    .map_err(|e| invalid(e.to_string()))?;
    bounds::require_cross_view_aliases(
        snapshot,
        selected,
        snapshot,
        selected,
        denied_paths,
        &[],
        cancel,
    )?;
    let traversal = selected
        .traversal(snapshot.path_semantics)
        .map_err(|e| invalid(e.to_string()))?;
    let entries: BTreeMap<_, _> = snapshot
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e))
        .collect();
    let denied: BTreeSet<_> = denied_paths.iter().map(|p| p.path.as_str()).collect();
    let protected: BTreeSet<_> = snapshot
        .entries
        .iter()
        .filter(|e| {
            e.state == EntryState::Reserved
                && matches!(e.target, Some(TreeEntryTarget::Tree { .. }))
        })
        .map(|e| e.path.as_str())
        .collect();
    for entry in &snapshot.entries {
        cancellation::check(cancel)?;
        if !traversal
            .intersects(entry.path.as_str())
            .map_err(|e| invalid(e.to_string()))?
            || protected_ancestor(entry.path.as_str(), &protected)
            || denied.contains(entry.path.as_str())
            || has_ancestor(entry.path.as_str(), &denied)
        {
            return Err(invalid("stored observation visible namespace differs"));
        }
    }
    for path in denied_paths {
        cancellation::check(cancel)?;
        let path = path.path.as_str();
        let below_leaf = path.match_indices('/').any(|(at, _)| {
            entries
                .get(&path[..at])
                .is_some_and(|entry| !matches!(entry.target, Some(TreeEntryTarget::Tree { .. })))
        });
        if !traversal
            .intersects(path)
            .map_err(|e| invalid(e.to_string()))?
            || has_ancestor(path, &denied)
            || protected_ancestor(path, &protected)
            || below_leaf
        {
            return Err(invalid("stored observation denied namespace differs"));
        }
    }
    if let ProjectionSelection::Selected { paths, subtrees } = selected {
        for path in paths.iter().chain(subtrees) {
            cancellation::check(cancel)?;
            let path = path.as_str();
            if !entries.contains_key(path)
                && !denied.contains(path)
                && !protected_ancestor(path, &protected)
            {
                return Err(invalid("stored observation omitted selected scope anchor"));
            }
        }
    }
    Ok(cancellation::check(cancel)?)
}
