use super::{EntryState, PathSemantics, ProjectionSnapshot, Result, invalid, paths::path_key};
use crate::scm::RepoPath;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Local checkout intent, independent of authorization and content residency.
/// Exact paths select one leaf; subtrees include their root and descendants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectionSelection {
    #[default]
    All,
    Selected {
        paths: Vec<RepoPath>,
        subtrees: Vec<RepoPath>,
    },
}

fn covered(roots: &[RepoPath], path: &str) -> bool {
    path.match_indices('/')
        .map(|(i, _)| &path[..i])
        .chain(std::iter::once(path))
        .any(|prefix| roots.binary_search_by(|r| r.as_str().cmp(prefix)).is_ok())
}

/// Platform-key index for bounded namespace walking, including aliases.
pub struct SelectionTraversal {
    all: bool,
    paths: BTreeSet<String>,
    subtrees: BTreeSet<String>,
    semantics: PathSemantics,
}
impl SelectionTraversal {
    pub fn intersects(&self, path: &str) -> Result<bool> {
        if self.all {
            return Ok(true);
        }
        let key = path_key(path, self.semantics)?;
        if self.paths.contains(&key) || self.subtrees.contains(&key) {
            return Ok(true);
        }
        if key
            .match_indices('/')
            .any(|(i, _)| self.subtrees.contains(&key[..i]))
        {
            return Ok(true);
        }
        let prefix = format!("{key}/");
        Ok([&self.paths, &self.subtrees].iter().any(|set| {
            set.range(prefix.clone()..)
                .next()
                .is_some_and(|p| p.starts_with(&prefix))
        }))
    }
}

impl ProjectionSelection {
    pub fn selected(
        paths: Vec<RepoPath>,
        subtrees: Vec<RepoPath>,
        semantics: PathSemantics,
    ) -> Result<Self> {
        Self::Selected { paths, subtrees }.normalized(semantics)
    }

    pub fn normalized(&self, semantics: PathSemantics) -> Result<Self> {
        let Self::Selected { paths, subtrees } = self else {
            return Ok(Self::All);
        };
        if paths.len().saturating_add(subtrees.len()) > 100_000 {
            return Err(invalid("selection exceeds100000 rules"));
        }
        let mut prefixes = BTreeMap::new();
        let mut total = 0usize;
        for path in paths.iter().chain(subtrees) {
            total = total.saturating_add(path.as_str().len());
            if total > 8 * 1024 * 1024 {
                return Err(invalid("selection exceeds8MiB"));
            }
            Self::record_prefixes(path.as_str(), semantics, &mut prefixes)?;
        }
        let mut subtrees = subtrees.clone();
        subtrees.sort();
        subtrees.dedup();
        let mut roots: Vec<RepoPath> = Vec::new();
        for path in subtrees {
            if !covered(&roots, path.as_str()) {
                roots.push(path);
            }
        }
        let mut paths = paths.clone();
        paths.sort();
        paths.dedup();
        paths.retain(|p| !covered(&roots, p.as_str()));
        Ok(Self::Selected {
            paths,
            subtrees: roots,
        })
    }

    fn record_prefixes<'a>(
        path: &'a str,
        semantics: PathSemantics,
        prefixes: &mut BTreeMap<String, &'a str>,
    ) -> Result<()> {
        for (index, _) in path
            .match_indices('/')
            .chain(std::iter::once((path.len(), "/")))
        {
            let prefix = &path[..index];
            let key = path_key(prefix, semantics)?;
            if prefixes
                .insert(key, prefix)
                .is_some_and(|old| old != prefix)
            {
                return Err(invalid("selection paths alias on the checkout platform"));
            }
        }
        Ok(())
    }

    pub fn includes(&self, path: &RepoPath) -> bool {
        match self {
            Self::All => true,
            Self::Selected { paths, subtrees } => {
                paths.binary_search(path).is_ok() || covered(subtrees, path.as_str())
            }
        }
    }
    pub fn includes_subtree(&self, path: &RepoPath) -> bool {
        match self {
            Self::All => true,
            Self::Selected { subtrees, .. } => covered(subtrees, path.as_str()),
        }
    }

    /// Includes ancestors needed for traversal. Platform aliases must be visited
    /// and rejected by the namespace verifier rather than silently skipped.
    pub fn intersects(&self, path: &str, semantics: PathSemantics) -> Result<bool> {
        self.traversal(semantics)?.intersects(path)
    }
    pub fn traversal(&self, semantics: PathSemantics) -> Result<SelectionTraversal> {
        let (paths, subtrees) = match self {
            Self::All => (BTreeSet::new(), BTreeSet::new()),
            Self::Selected { paths, subtrees } => (
                paths
                    .iter()
                    .map(|p| path_key(p.as_str(), semantics))
                    .collect::<Result<_>>()?,
                subtrees
                    .iter()
                    .map(|p| path_key(p.as_str(), semantics))
                    .collect::<Result<_>>()?,
            ),
        };
        Ok(SelectionTraversal {
            all: matches!(self, Self::All),
            paths,
            subtrees,
            semantics,
        })
    }

    pub fn is_expansion_of(&self, before: &Self) -> bool {
        match (self, before) {
            (Self::All, _) => true,
            (_, Self::All) => false,
            (
                Self::Selected { subtrees, .. },
                Self::Selected {
                    paths: old_paths,
                    subtrees: old_roots,
                },
            ) => {
                old_paths.iter().all(|p| self.includes(p))
                    && old_roots.iter().all(|p| covered(subtrees, p.as_str()))
            }
        }
    }

    /// Validate local intent against the complete bounded structural observation.
    /// Exclusion does not change the target, rights, or explicit absence semantics.
    pub fn validate_snapshot(&self, snapshot: &ProjectionSnapshot) -> Result<()> {
        snapshot.validate()?;
        if self.normalized(snapshot.path_semantics)? != *self {
            return Err(invalid("selection is not normalized"));
        }
        let mut prefixes = BTreeMap::new();
        if let Self::Selected { paths, subtrees } = self {
            for p in paths.iter().chain(subtrees) {
                Self::record_prefixes(p.as_str(), snapshot.path_semantics, &mut prefixes)?;
            }
        }
        for entry in &snapshot.entries {
            Self::record_prefixes(entry.path.as_str(), snapshot.path_semantics, &mut prefixes)?;
            if !self.includes(&entry.path)
                && matches!(entry.state, EntryState::Materialized | EntryState::Absent)
            {
                return Err(invalid(
                    "excluded paths cannot be materialized or observed absent",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(p: &str) -> RepoPath {
        RepoPath::parse(p).unwrap()
    }
    #[test]
    fn normalizes_exact_paths_and_subtrees_without_sibling_prefixes() {
        let selection = ProjectionSelection::selected(
            vec![p("a/x"), p("b"), p("b")],
            vec![p("a/z"), p("a")],
            PathSemantics::PortableWindows,
        )
        .unwrap();
        assert_eq!(
            selection,
            ProjectionSelection::Selected {
                paths: vec![p("b")],
                subtrees: vec![p("a")]
            }
        );
        assert!(selection.includes(&p("a/x")));
        assert!(!selection.includes(&p("ab/x")));
        assert!(!selection.includes(&p("b/x")));
        assert!(ProjectionSelection::All.is_expansion_of(&selection));
        assert!(!selection.is_expansion_of(&ProjectionSelection::All));
    }
    #[test]
    fn rejects_metadata_and_case_aliases() {
        for (a, b) in [
            ("src/a", "SRC/b"),
            (".git/a", "x"),
            (".layrs/a", "x"),
            ("aux.txt", "x"),
        ] {
            assert!(
                ProjectionSelection::selected(
                    vec![p(a), p(b)],
                    vec![],
                    PathSemantics::PortableWindows
                )
                .is_err()
            );
        }
    }
}
