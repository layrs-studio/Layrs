//! Bounded root .layrsignore: ordered globs, ** directories and ! exceptions.
use crate::secure_fs::PinnedDirectory;
use std::io::{self, Read};
#[derive(Default)]
pub(super) struct IgnoreRules {
    rules: Vec<Rule>,
    pub has_exceptions: bool,
}
struct Rule {
    parts: Vec<String>,
    anchored: bool,
    directory: bool,
    include: bool,
}
impl IgnoreRules {
    pub fn load(root: &PinnedDirectory) -> io::Result<Self> {
        let file = match root.open_file(".layrsignore".as_ref()) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e),
        };
        let mut bytes = vec![];
        file.file().take(65537).read_to_end(&mut bytes)?;
        if bytes.len() > 65536 {
            return Err(invalid(".layrsignore exceeds 64 KiB"));
        }
        Self::parse(std::str::from_utf8(&bytes).map_err(|_| invalid(".layrsignore must be UTF-8"))?)
    }
    fn parse(text: &str) -> io::Result<Self> {
        let mut result = Self::default();
        for line in text.lines() {
            let mut pattern = line.trim();
            if pattern.is_empty() || pattern.starts_with('#') {
                continue;
            }
            let include = pattern.starts_with('!');
            if include {
                pattern = &pattern[1..];
                result.has_exceptions = true;
            }
            let anchored = pattern.starts_with('/');
            let directory = pattern.ends_with('/');
            pattern = pattern.trim_matches('/');
            if pattern.is_empty()
                || pattern.len() > 4096
                || result.rules.len() >= 512
                || pattern.contains(['\\', '[', ']', '\0'])
            {
                return Err(invalid(
                    "unsupported or excessive .layrsignore pattern (use *, ?, ** and ! rules)",
                ));
            }
            let parts: Vec<_> = pattern.split('/').map(str::to_owned).collect();
            if parts.len() > 128
                || parts
                    .iter()
                    .any(|p| p.is_empty() || p == "." || p == ".." || p.contains("**") && p != "**")
            {
                return Err(invalid("invalid .layrsignore path pattern"));
            }
            result.rules.push(Rule {
                parts,
                anchored,
                directory,
                include,
            });
        }
        Ok(result)
    }
    pub fn ignores(&self, path: &str, is_directory: bool) -> bool {
        if path == ".layrsignore" {
            return false;
        }
        let parts: Vec<_> = path.split('/').collect();
        let mut ignored = false;
        for rule in &self.rules {
            let matches = if rule.parts.len() == 1 && !rule.anchored {
                parts.iter().enumerate().any(|(i, p)| {
                    (!rule.directory || i + 1 < parts.len() || is_directory)
                        && glob(&rule.parts[0], p)
                })
            } else {
                (1..=parts.len()).any(|n| {
                    (!rule.directory || n < parts.len() || is_directory)
                        && path_glob(&rule.parts, &parts[..n])
                })
            };
            if matches {
                ignored = !rule.include;
            }
        }
        ignored
    }
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn path_glob(pattern: &[String], path: &[&str]) -> bool {
    let mut previous = vec![false; path.len() + 1];
    previous[0] = true;
    for part in pattern {
        let mut next = vec![false; path.len() + 1];
        if part == "**" {
            next[0] = previous[0];
            for i in 1..=path.len() {
                next[i] = previous[i] || next[i - 1];
            }
        } else {
            for i in 1..=path.len() {
                next[i] = previous[i - 1] && glob(part, path[i - 1]);
            }
        }
        previous = next;
    }
    previous[path.len()]
}
fn glob(pattern: &str, value: &str) -> bool {
    let p: Vec<_> = pattern.chars().collect();
    let v: Vec<_> = value.chars().collect();
    let (mut i, mut j, mut star, mut retry) = (0, 0, None, 0);
    while j < v.len() {
        if i < p.len() && (p[i] == '?' || p[i] == v[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some(i);
            i += 1;
            retry = j;
        } else if let Some(s) = star {
            retry += 1;
            j = retry;
            i = s + 1;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == '*' {
        i += 1;
    }
    i == p.len()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordered_patterns_and_exceptions() {
        let r = IgnoreRules::parse(
            "# comment\nnode_modules/\n*.tmp\n/root-only\nlogs/**/cache?.bin\n!important.tmp\n",
        )
        .unwrap();
        for p in [
            "a/node_modules/x",
            "x.tmp",
            "root-only/a",
            "logs/cache1.bin",
            "logs/a/b/cache2.bin",
        ] {
            assert!(r.ignores(p, false), "{p}");
        }
        for p in [
            "important.tmp",
            "a/root-only",
            "node_modules.txt",
            "logs/cache22.bin",
            ".layrsignore",
        ] {
            assert!(!r.ignores(p, false), "{p}");
        }
    }
    #[test]
    fn unsupported_patterns_fail_explicitly() {
        for p in ["!", "[abc]", "a/**b", "../x", "a//b"] {
            assert!(IgnoreRules::parse(p).is_err(), "{p}");
        }
    }
}
