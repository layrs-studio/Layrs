//! Checkout restrictions are platform properties, not canonical Tree identity.
use super::checkout::CheckoutEntry;
use std::io;
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
    // Each prefix (directory or full path), keyed as the platform compares
    // names, with its exact spelling: one key spelled two ways is a
    // collision ("D/a" and "d/b" on Windows). Keys borrow the paths.
    let mut prefixes: std::collections::HashMap<Folded<'a>, &'a str> =
        std::collections::HashMap::new();
    let mut files = std::collections::HashSet::new();
    // The directory part of the previous path, whose components and
    // prefixes were all checked: most paths share it.
    let mut checked_directory: Option<&str> = None;
    for path in paths.clone() {
        layrs_core::scm::RepoPath::check(path).map_err(|_| invalid())?;
        if !files.insert(path) {
            return Err(invalid());
        }
        let directory_end = path.rfind('/');
        let directory = directory_end.map(|end| &path[..end]);
        // Components already checked (same directory as the previous path)
        // are skipped: their checks and prefix inserts would repeat exactly.
        let first = match (directory, checked_directory) {
            (Some(directory), Some(checked)) if directory == checked => {
                directory_end.map_or(0, |end| end + 1)
            }
            _ => 0,
        };
        let mut start = 0;
        for component in path.split('/') {
            let end = start + component.len();
            if start >= first {
                check_component(component, windows)?;
                let prefix = &path[..end];
                let key = Folded {
                    text: prefix,
                    fold: windows,
                };
                if let Some(old) = prefixes.insert(key, prefix) {
                    if old != prefix {
                        return Err(invalid());
                    }
                }
            }
            start = end + 1;
        }
        checked_directory = directory;
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

/// One path component's own restrictions.
fn check_component(component: &str, windows: bool) -> io::Result<()> {
    if component.eq_ignore_ascii_case(".git") || component.eq_ignore_ascii_case(".layrs") {
        return Err(invalid());
    }
    if !windows {
        return Ok(());
    }
    let reserved = if component.is_ascii() {
        // ASCII: the same tests without allocating (uppercasing ASCII text
        // leaves it ASCII; the superscript digits are not ASCII).
        let stem = component
            .split('.')
            .next()
            .unwrap_or("")
            .trim_end_matches(' ');
        let numbered = stem.len() == 4
            && (stem[..3].eq_ignore_ascii_case("COM") || stem[..3].eq_ignore_ascii_case("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9');
        ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"]
            .iter()
            .any(|name| stem.eq_ignore_ascii_case(name))
            || numbered
            || component.len() > 255
    } else {
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
        matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || numbered
            || component.encode_utf16().count() > 255
    };
    if reserved
        || component.ends_with(['.', ' '])
        || component.chars().any(|c| c < ' ' || "<>:\"|?*".contains(c))
    {
        return Err(invalid());
    }
    Ok(())
}

/// A prefix compared as the platform compares names: exactly, or (Windows)
/// as `str::to_uppercase` would, char by char, without allocating.
#[derive(Clone, Copy)]
struct Folded<'a> {
    text: &'a str,
    fold: bool,
}

impl Folded<'_> {
    /// The folded text (`fold` only): what `str::to_uppercase` yields.
    fn chars(&self) -> impl Iterator<Item = char> + '_ {
        self.text.chars().flat_map(char::to_uppercase)
    }
}

impl PartialEq for Folded<'_> {
    fn eq(&self, other: &Self) -> bool {
        if !self.fold {
            return self.text == other.text;
        }
        if self.text.is_ascii() && other.text.is_ascii() {
            return self.text.eq_ignore_ascii_case(other.text);
        }
        self.chars().eq(other.chars())
    }
}

impl Eq for Folded<'_> {}

impl std::hash::Hash for Folded<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        if !self.fold {
            self.text.hash(state);
            return;
        }
        if self.text.is_ascii() {
            for byte in self.text.bytes() {
                state.write_u32(u32::from(byte.to_ascii_uppercase()));
            }
        } else {
            for c in self.chars() {
                state.write_u32(u32::from(c));
            }
        }
        state.write_u8(0xff);
    }
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
