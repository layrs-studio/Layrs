use super::*;

pub(super) struct Plan {
    pub target: ProjectionSnapshot,
    pub old: Vec<Record>,
    pub install: Vec<String>,
}
pub(super) fn normalize(
    before: &ProjectionSnapshot,
    target: &ProjectionSnapshot,
) -> Result<ProjectionSnapshot> {
    before.validate()?;
    target.validate()?;
    if before.path_semantics != target.path_semantics
        || target.observed_authorization_epoch < before.observed_authorization_epoch
        || (before.base_step_id == target.base_step_id
            && before.base_root_tree_id != target.base_root_tree_id)
        || (before.observed_authorization_epoch == target.observed_authorization_epoch
            && before.observed_policy_set_id != target.observed_policy_set_id)
    {
        return Err(invalid(
            "refresh base, path semantics or authority mismatch",
        ));
    }
    let same_observation = before.base_step_id == target.base_step_id
        && before.base_root_tree_id == target.base_root_tree_id
        && before.observed_policy_set_id == target.observed_policy_set_id
        && before.observed_authorization_epoch == target.observed_authorization_epoch;
    let old: BTreeMap<_, _> = before
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e))
        .collect();
    let mut next = target.clone();
    // An incoming snapshot cannot manufacture a grant for an unobserved absence.
    next.entries.retain(|e| e.state != EntryState::Absent);
    if same_observation {
        for entry in &before.entries {
            if entry.state == EntryState::Absent
                && !next.entries.iter().any(|e| e.path == entry.path)
            {
                next.entries.push(entry.clone());
            }
        }
    }
    next.entries.sort_by(|a, b| a.path.cmp(&b.path));
    next.validate()?;
    for entry in &target.entries {
        if entry.state == EntryState::Absent
            && same_observation
            && old.get(entry.path.as_str()).copied() != Some(entry)
        {
            return Err(invalid(
                "refresh cannot create an unobserved absent-path grant",
            ));
        }
        if entry.state == EntryState::Materialized
            && !matches!(
                entry.target,
                Some(TreeEntryTarget::File { .. } | TreeEntryTarget::Tree { .. })
            )
        {
            return Err(invalid("refresh materializes regular-file leaves only"));
        }
        if cfg!(windows) && entry.state == EntryState::Materialized && entry.executable {
            return Err(invalid("Windows cannot materialize executable mode"));
        }
    }
    Ok(next)
}
pub(super) fn operations(
    before: &ProjectionSnapshot,
    after: &ProjectionSnapshot,
) -> (BTreeSet<String>, Vec<String>) {
    let old: BTreeMap<_, _> = before
        .entries
        .iter()
        .filter(|e| materialized_file(e))
        .map(|e| (e.path.as_str(), e))
        .collect();
    let new: BTreeMap<_, _> = after
        .entries
        .iter()
        .filter(|e| materialized_file(e))
        .map(|e| (e.path.as_str(), e))
        .collect();
    let same = |a: &&ProjectionEntry, b: &&ProjectionEntry| {
        a.target == b.target && a.executable == b.executable
    };
    let remove = old
        .iter()
        .filter(|(path, entry)| !new.get(**path).is_some_and(|other| same(entry, other)))
        .map(|(path, _)| (*path).to_owned())
        .collect();
    let install = new
        .iter()
        .filter(|(path, entry)| !old.get(**path).is_some_and(|other| same(entry, other)))
        .map(|(path, _)| (*path).to_owned())
        .collect();
    (remove, install)
}
pub(super) fn materialized_file(e: &ProjectionEntry) -> bool {
    e.state == EntryState::Materialized && matches!(e.target, Some(TreeEntryTarget::File { .. }))
}
pub(super) fn prepare(
    store: &ProjectionStore,
    before: &ProjectionSnapshot,
    target: &ProjectionSnapshot,
) -> Result<Plan> {
    let target = normalize(before, target)?;
    let (remove, install) = operations(before, &target);
    let root = PinnedDirectory::open(&store.root)?;
    let mut old = Vec::new();
    let mut total = 0u64;
    for entry in &before.entries {
        if materialized_file(entry) {
            let record = files::inspect(
                &root,
                entry.path.as_str(),
                entry,
                format!("old-{:08}", old.len()),
            )?;
            total = total
                .checked_add(record.bytes)
                .filter(|n| *n <= MAX_TOTAL)
                .ok_or_else(|| invalid("refresh old files exceed4GiB"))?;
            old.push(record);
        } else if entry.state == EntryState::Absent
            && files::present(&store.root.join(entry.path.as_str()))?
        {
            return Err(invalid(
                "local creation must be published before projection refresh",
            ));
        }
    }
    let removed = remove.iter().map(String::as_str).collect();
    for path in &install {
        files::destination(store, path, &removed)?;
    }
    root.verify_path_identity()?;
    Ok(Plan {
        target,
        old,
        install,
    })
}
