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

/// `validate_paths(current)` when `valid` passed it: only what `current`
/// adds to `valid` (`added`, each a path of `current` absent from `valid`)
/// is checked, against all of `current`. `current` and `valid` are sorted by
/// bytes (`current` must be strictly: no path twice).
///
/// A violation needs a path that `valid` does not hold (`valid` has none):
/// - an added path's own syntax and components;
/// - a file used as a folder: an added path below a file of `current`, or
///   an added path that is a folder of `current`;
/// - one name spelled two ways (Windows): two prefixes folding alike fold
///   alike component by component (no character uppercases to `/`), so
///   either they are siblings, or their parents collide one level up. By
///   induction from the root, comparing the names in each folder that gains
///   a prefix `valid` does not hold finds every collision.
pub(super) fn validate_added_paths(
    current: &[&str],
    valid: &[&str],
    added: &[&str],
    windows: bool,
) -> io::Result<()> {
    if current.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(invalid());
    }
    // Folders gaining a name `valid` does not hold ("" is the root).
    let mut folders = std::collections::BTreeSet::new();
    for path in added {
        layrs_core::scm::RepoPath::check(path).map_err(|_| invalid())?;
        let mut start = 0;
        for component in path.split('/') {
            let end = start + component.len();
            check_component(component, windows)?;
            let prefix = &path[..end];
            if end < path.len() && holds_path(current, prefix) {
                return Err(invalid());
            }
            if windows && !holds_path(valid, prefix) && !holds_folder(valid, prefix) {
                folders.insert(&path[..start.saturating_sub(1)]);
            }
            start = end + 1;
        }
        if holds_folder(current, path) {
            return Err(invalid());
        }
    }
    for folder in folders {
        let below = if folder.is_empty() {
            String::new()
        } else {
            format!("{folder}/")
        };
        let first = current.partition_point(|path| *path < below.as_str());
        let mut names: std::collections::HashMap<Folded<'_>, &str> =
            std::collections::HashMap::new();
        let mut last = None;
        for path in current[first..]
            .iter()
            .take_while(|path| path.starts_with(below.as_str()))
        {
            let rest = &path[below.len()..];
            let name = &rest[..rest.find('/').unwrap_or(rest.len())];
            // The paths of one subfolder are mostly adjacent.
            if last == Some(name) {
                continue;
            }
            last = Some(name);
            if let Some(old) = names.insert(
                Folded {
                    text: name,
                    fold: true,
                },
                name,
            ) {
                if old != name {
                    return Err(invalid());
                }
            }
        }
    }
    Ok(())
}

/// Whether `paths` (sorted by bytes) holds `path` itself.
fn holds_path(paths: &[&str], path: &str) -> bool {
    paths.binary_search(&path).is_ok()
}

/// Whether some path of `paths` (sorted by bytes) is below folder `folder`.
fn holds_folder(paths: &[&str], folder: &str) -> bool {
    let below = format!("{folder}/");
    let first = paths.partition_point(|path| *path < below.as_str());
    paths
        .get(first)
        .is_some_and(|path| path.starts_with(below.as_str()))
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
    /// `validate_added_paths` decides as `validate_paths` of the whole set
    /// whenever the base set is valid, on both platforms.
    #[test]
    fn added_paths_decide_as_the_whole_set() {
        const NAMES: [&str; 9] = ["a", "A", "b", "\u{df}", "SS", "ss", "NUL", "x.txt", ".git"];
        fn next(seed: &mut u64, bound: usize) -> usize {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            (*seed % bound as u64) as usize
        }
        fn path(seed: &mut u64) -> String {
            let depth = 1 + next(seed, 3);
            (0..depth)
                .map(|_| NAMES[next(seed, NAMES.len())])
                .collect::<Vec<_>>()
                .join("/")
        }
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut checked = [0usize; 2];
        for round in 0..6_000 {
            let windows = round % 2 == 0;
            let count = next(&mut seed, 6);
            let mut valid: Vec<String> = (0..count).map(|_| path(&mut seed)).collect();
            valid.sort();
            valid.dedup();
            let valid_refs: Vec<&str> = valid.iter().map(String::as_str).collect();
            if validate_paths(valid_refs.iter().copied(), windows).is_err() {
                continue;
            }
            let mut current = valid.clone();
            current.retain(|_| next(&mut seed, 4) != 0);
            let count = 1 + next(&mut seed, 3);
            current.extend((0..count).map(|_| path(&mut seed)));
            current.sort();
            current.dedup();
            let current_refs: Vec<&str> = current.iter().map(String::as_str).collect();
            let added: Vec<&str> = current_refs
                .iter()
                .copied()
                .filter(|path| valid_refs.binary_search(path).is_err())
                .collect();
            let whole = validate_paths(current_refs.iter().copied(), windows).is_ok();
            let incremental =
                validate_added_paths(&current_refs, &valid_refs, &added, windows).is_ok();
            assert_eq!(
                whole, incremental,
                "windows {windows}: {valid_refs:?} -> {current_refs:?}"
            );
            checked[usize::from(whole)] += 1;
        }
        assert!(checked[0] > 100 && checked[1] > 100, "{checked:?}");
        let twice = ["a", "a"];
        assert!(validate_added_paths(&twice, &[], &twice, true).is_err());
    }

    #[test]
    fn unix_preserves_distinct_names_and_rejects_file_parent() {
        assert!(validate_for(&entries(&["a", "A", "NUL.txt", "dir/file:stream"]), false).is_ok());
        assert!(validate_for(&entries(&["a", "a/b"]), false).is_err());
    }
}
