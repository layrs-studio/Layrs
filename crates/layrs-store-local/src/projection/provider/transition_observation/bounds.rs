//! Cheap fences before normalization's maps and canonical encodings.
use super::*;
use serde::Serialize;
use std::io::{self, Write};

const MAX_ROWS: usize = 100_000;
const MAX_PATH_BYTES: usize = 8 * 1024 * 1024;
const MAX_INDEX_UPPER: usize = 256 * 1024 * 1024;

#[derive(Default)]
pub(super) struct PathBudget {
    index_upper: usize,
}
impl PathBudget {
    fn paths<'a>(
        &mut self,
        count: usize,
        paths: impl Iterator<Item = &'a RepoPath>,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        if count > MAX_ROWS {
            return Err(invalid("transition observation path count exceeds bound"));
        }
        let mut total = 0usize;
        for path in paths {
            cancellation::check(cancel)?;
            let path = path.as_str();
            if path.is_empty() || path.len() > codec::PATH_MAX {
                return Err(invalid("transition observation path exceeds bound"));
            }
            let depth = path.split('/').count();
            if depth > 128 {
                return Err(invalid("transition observation path exceeds bound"));
            }
            total = total
                .checked_add(path.len())
                .filter(|n| *n <= MAX_PATH_BYTES)
                .ok_or_else(|| invalid("transition observation path bytes exceed bound"))?;
            // Logical prefix-expansion work budget before core's path maps:
            // original/uppercase string allowance plus per-prefix work. This
            // is not an allocator geometry or peak-RSS measurement.
            let upper = path
                .len()
                .checked_mul(6)
                .and_then(|n| n.checked_add(128))
                .and_then(|n| n.checked_mul(depth))
                .ok_or_else(|| invalid("transition path index overflow"))?;
            self.index_upper = self
                .index_upper
                .checked_add(upper)
                .filter(|n| *n <= MAX_INDEX_UPPER)
                .ok_or_else(|| invalid("transition path index footprint exceeds bound"))?;
        }
        Ok(())
    }
    pub(super) fn snapshot(
        &mut self,
        snapshot: &ProjectionSnapshot,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        self.snapshot_with_denied(snapshot, &[], cancel)
    }
    pub(super) fn snapshot_with_denied(
        &mut self,
        snapshot: &ProjectionSnapshot,
        denied: &[DeniedPathData],
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        cancellation::check(cancel)?;
        // The fresh visible and denied paths describe one namespace. Source,
        // selection and descriptor copies retain their own count/byte bounds;
        // all categories still contribute to the shared index-work budget.
        let count = snapshot
            .entries
            .len()
            .checked_add(denied.len())
            .ok_or_else(|| invalid("transition observation path count exceeds bound"))?;
        self.paths(
            count,
            snapshot
                .entries
                .iter()
                .map(|entry| &entry.path)
                .chain(denied.iter().map(|item| &item.path)),
            cancel,
        )?;
        codec::preflight(snapshot, codec::SNAPSHOT_MAX)?;
        Ok(())
    }
    pub(super) fn selection(
        &mut self,
        selection: &ProjectionSelection,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        if let ProjectionSelection::Selected { paths, subtrees } = selection {
            let count = paths
                .len()
                .checked_add(subtrees.len())
                .ok_or_else(|| invalid("transition selection count overflow"))?;
            self.paths(count, paths.iter().chain(subtrees), cancel)?;
        }
        // selection::encode adds a small fixed schema wrapper and clones its
        // input; settle the serialized budget BEFORE that clone/canonical Vec.
        codec::preflight(selection, codec::SELECTION_MAX - 128)?;
        Ok(())
    }
    pub(super) fn descriptors(
        &mut self,
        descriptors: &[ProviderSourceDescriptor],
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        self.paths(
            descriptors.len(),
            descriptors.iter().map(|item| &item.path),
            cancel,
        )?;
        let mut aggregate = 0u64;
        for input in descriptors {
            cancellation::check(cancel)?;
            let counted =
                serialized_bytes(&input.manifest, super::super::PROVIDER_DESCRIPTOR_MAX_BYTES)?;
            // This manifest's schema uses only fixed ASCII tags/IDs and u64
            // scalars. Double its JSON count conservatively covers canonical
            // numeric spelling; add worst-case escaped path plus row scalars.
            let row_upper = counted
                .checked_mul(2)
                .and_then(|n| n.checked_add(6 * input.path.as_str().len() as u64 + 1024))
                .ok_or_else(|| invalid("transition descriptor preflight overflow"))?;
            aggregate = aggregate
                .checked_add(row_upper)
                .filter(|n| *n <= super::super::PROVIDER_CATALOG_AGGREGATE_MAX_BYTES)
                .ok_or_else(|| {
                    invalid("transition descriptor preflight aggregate exceeds bound")
                })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

struct Count {
    bytes: u64,
    max: u64,
}
impl Write for Count {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len() as u64)
            .filter(|n| *n <= self.max)
            .ok_or_else(|| io::Error::other("transition metadata serialization exceeds bound"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn serialized_bytes(value: &impl Serialize, max: u64) -> Result<u64> {
    let mut count = Count { bytes: 0, max };
    serde_json::to_writer(&mut count, value).map_err(|error| invalid(error.to_string()))?;
    Ok(count.bytes)
}

fn add_aliases(
    paths: &mut BTreeMap<String, String>,
    path: &RepoPath,
    semantics: PathSemantics,
) -> Result<()> {
    for (at, _) in path
        .as_str()
        .match_indices('/')
        .chain(std::iter::once((path.as_str().len(), "/")))
    {
        let prefix = &path.as_str()[..at];
        let key = match semantics {
            PathSemantics::CaseSensitive => prefix.to_owned(),
            PathSemantics::PortableWindows => prefix.to_uppercase(),
        };
        if paths.get(&key).is_some_and(|prior| prior != prefix) {
            return Err(invalid(
                "transition paths alias across source and destination observations",
            ));
        }
        paths.entry(key).or_insert_with(|| prefix.to_owned());
    }
    Ok(())
}
fn selection_aliases(
    paths: &mut BTreeMap<String, String>,
    selected: &ProjectionSelection,
    semantics: PathSemantics,
) -> Result<()> {
    if let ProjectionSelection::Selected {
        paths: exact,
        subtrees,
    } = selected
    {
        for path in exact.iter().chain(subtrees) {
            add_aliases(paths, path, semantics)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn require_cross_view_aliases(
    source: &ProjectionSnapshot,
    source_selection: &ProjectionSelection,
    fresh: &ProjectionSnapshot,
    selected: &ProjectionSelection,
    denied: &[DeniedPathData],
    descriptors: &[ProviderSourceDescriptor],
    cancel: Option<&Cancellation>,
) -> Result<()> {
    let semantics = fresh.path_semantics;
    let mut paths = BTreeMap::new();
    for path in source
        .entries
        .iter()
        .map(|e| &e.path)
        .chain(fresh.entries.iter().map(|e| &e.path))
        .chain(denied.iter().map(|e| &e.path))
        .chain(descriptors.iter().map(|e| &e.path))
    {
        cancellation::check(cancel)?;
        add_aliases(&mut paths, path, semantics)?;
    }
    selection_aliases(&mut paths, source_selection, semantics)?;
    selection_aliases(&mut paths, selected, semantics)
}
