use super::{Result, invalid};
use crate::scm::RepoPath;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathSemantics {
    CaseSensitive,
    PortableWindows,
}

/// Namespace fences, independent of the host OS and content possession. A
/// mutation touching an ancestor, descendant or alias of a fence is forbidden.
#[derive(Debug, Clone)]
pub struct ReservedPaths {
    semantics: PathSemantics,
    paths: BTreeSet<String>,
}
impl ReservedPaths {
    pub fn new<'a>(
        paths: impl IntoIterator<Item = &'a str>,
        semantics: PathSemantics,
    ) -> Result<Self> {
        let mut result = Self {
            semantics,
            paths: BTreeSet::new(),
        };
        let mut total = 0usize;
        for (index, path) in paths.into_iter().enumerate() {
            if index >= 100_000 {
                return Err(invalid("reserved path count exceeds 100000"));
            }
            total = total
                .checked_add(path.len())
                .ok_or_else(|| invalid("reserved path size overflow"))?;
            if total > 64 * 1024 * 1024 {
                return Err(invalid("reserved paths exceed 64MiB"));
            }
            result.paths.insert(path_key(path, semantics)?);
        }
        Ok(result)
    }
    pub fn check_path(&self, path: &str) -> Result<()> {
        let key = path_key(path, self.semantics)?;
        if self.paths.contains(&key) {
            return Err(invalid("path is reserved"));
        }
        for (index, _) in key.match_indices('/') {
            if self.paths.contains(&key[..index]) {
                return Err(invalid("path descends from a reserved path"));
            }
        }
        let prefix = format!("{key}/");
        if self
            .paths
            .range(prefix.clone()..)
            .next()
            .is_some_and(|p| p.starts_with(&prefix))
        {
            return Err(invalid("path contains a reserved descendant"));
        }
        Ok(())
    }
    /// Both sides are checked: moving a parent must not relocate hidden files.
    pub fn check_move(&self, source: &str, destination: &str) -> Result<()> {
        self.check_path(source)?;
        self.check_path(destination)
    }
    pub fn semantics(&self) -> PathSemantics {
        self.semantics
    }
}

pub(super) fn path_key(path: &str, semantics: PathSemantics) -> Result<String> {
    if path.len() > 32768 || path.split('/').count() > 128 {
        return Err(invalid("projection path limit exceeded"));
    }
    RepoPath::parse(path).map_err(|e| invalid(e.to_string()))?;
    for component in path.split('/') {
        if component.eq_ignore_ascii_case(".layrs") || component.eq_ignore_ascii_case(".git") {
            return Err(invalid("reserved metadata directory"));
        }
        if semantics == PathSemantics::PortableWindows {
            let stem = component
                .split('.')
                .next()
                .unwrap_or("")
                .trim_end_matches(' ')
                .to_uppercase();
            let numbered = stem
                .strip_prefix("COM")
                .or_else(|| stem.strip_prefix("LPT"))
                .is_some_and(|suffix| {
                    matches!(
                        suffix,
                        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                    )
                });
            // Reject short-name spellings instead of guessing the filesystem's
            // 8.3 alias table. This is intentionally conservative and explicit.
            if component.ends_with(['.', ' '])
                || component.contains('~')
                || component.chars().any(|c| c < ' ' || "<>:\"|?*".contains(c))
                || matches!(
                    stem.as_str(),
                    "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
                )
                || numbered
                || component.encode_utf16().count() > 255
            {
                return Err(invalid(
                    "path cannot be represented safely by portable Windows semantics",
                ));
            }
        }
    }
    Ok(if semantics == PathSemantics::PortableWindows {
        path.to_uppercase()
    } else {
        path.to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reservation_covers_exact_ancestors_descendants_and_both_move_sides() {
        let r = ReservedPaths::new(
            ["private/secret.txt", "closed"],
            PathSemantics::CaseSensitive,
        )
        .unwrap();
        for p in [
            "private",
            "private/secret.txt",
            "private/secret.txt/child",
            "closed/a",
        ] {
            assert!(r.check_path(p).is_err(), "{p}");
        }
        for p in ["private/other", "private2", "closed2/a", "public"] {
            r.check_path(p).unwrap();
        }
        assert!(r.check_move("private", "public").is_err());
        assert!(r.check_move("public", "private/secret.txt").is_err());
        r.check_move("public", "private/other").unwrap();
    }
    #[test]
    fn windows_aliases_case_and_reserved_devices_fail_on_every_host() {
        let r = ReservedPaths::new(
            ["Private/Café.txt", "longfilename"],
            PathSemantics::PortableWindows,
        )
        .unwrap();
        for p in [
            "private",
            "PRIVATE/CAFÉ.TXT",
            "PRIVATE/café.txt/child",
            "private/cafe\u{301}.txt",
            "Private/Café.txt.",
            "Private/Café.txt ",
            "Private/Café.txt:stream",
            "LONGFI~1",
            "aux.txt",
            "CLOCK$.txt",
            "CON .txt",
            "COM¹.txt",
            "a/CONIN$",
            "LPT9",
            "a\\b",
            "../x",
            "/x",
            "x//y",
            ".LaYrS/data",
            "a/.GIT/x",
            "C:/x",
        ] {
            assert!(r.check_path(p).is_err(), "{p}");
        }
        r.check_path("private/other.txt").unwrap();
        r.check_path("COM10.txt").unwrap();
        ReservedPaths::new(["A"], PathSemantics::CaseSensitive)
            .unwrap()
            .check_path("a")
            .unwrap();
    }
}
