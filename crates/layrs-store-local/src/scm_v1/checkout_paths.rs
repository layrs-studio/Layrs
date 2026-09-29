//! Checkout restrictions are platform properties, not canonical Tree identity.
use super::checkout::CheckoutEntry;
use std::{collections::BTreeMap, io};
pub(super) fn validate(entries: &[CheckoutEntry]) -> io::Result<()> {
    validate_for(entries, cfg!(windows))
}
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "Tree paths cannot be represented faithfully on this checkout platform",
    )
}
fn validate_for(entries: &[CheckoutEntry], windows: bool) -> io::Result<()> {
    validate_paths(entries.iter().map(|e| e.path.as_str()), windows)
}
pub(super) fn validate_paths<'a>(
    paths: impl Iterator<Item = &'a str> + Clone,
    windows: bool,
) -> io::Result<()> {
    let mut prefixes = BTreeMap::new();
    let mut files = std::collections::BTreeSet::new();
    for path in paths.clone() {
        layrs_core::scm::RepoPath::parse(path).map_err(|_| invalid())?;
        if !files.insert(path) {
            return Err(invalid());
        }
        let mut prefix = String::new();
        for component in path.split('/') {
            if component.eq_ignore_ascii_case(".git") || component.eq_ignore_ascii_case(".layrs") {
                return Err(invalid());
            }
            if windows {
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
                if component.ends_with(['.', ' '])
                    || component.chars().any(|c| c < ' ' || "<>:\"|?*".contains(c))
                    || matches!(
                        stem.as_str(),
                        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
                    )
                    || numbered
                    || component.encode_utf16().count() > 255
                {
                    return Err(invalid());
                }
            }
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            let key = if windows {
                prefix.to_uppercase()
            } else {
                prefix.clone()
            };
            if prefixes
                .insert(key, prefix.clone())
                .is_some_and(|old| old != prefix)
            {
                return Err(invalid());
            }
        }
    }
    for path in paths {
        for (index, _) in path.match_indices('/') {
            if files.contains(&path[..index]) {
                return Err(invalid());
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn entries(paths: &[&str]) -> Vec<CheckoutEntry> {
        paths
            .iter()
            .map(|p| CheckoutEntry {
                path: (*p).into(),
                content: super::super::checkout::CheckoutContent::File {
                    bytes: vec![],
                    executable: false,
                },
            })
            .collect()
    }
    #[test]
    fn windows_rejects_aliases_streams_devices_and_type_collisions() {
        for paths in [
            vec!["dir/name:stream"],
            vec!["NUL.txt"],
            vec!["COM¹.log"],
            vec!["CONIN$"],
            vec!["foo."],
            vec!["foo "],
            vec!["a", "A"],
            vec!["D/a", "d/b"],
            vec!["a", "a/b"],
            vec![".git/config"],
            vec!["x/.layrs/store"],
            vec!["a", "a"],
        ] {
            assert!(validate_for(&entries(&paths), true).is_err(), "{paths:?}");
        }
        assert!(validate_for(&entries(&["café/a.txt", "café/b.txt", "COM10.log"]), true).is_ok());
    }
    #[test]
    fn unix_preserves_distinct_names_and_rejects_file_parent() {
        assert!(validate_for(&entries(&["a", "A", "NUL.txt", "dir/file:stream"]), false).is_ok());
        assert!(validate_for(&entries(&["a", "a/b"]), false).is_err());
    }
}
