//! Pure bounded comparison shared by local clients and authorized remote reads.
use std::collections::BTreeMap;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffContent<'a> {
    File { bytes: &'a [u8], executable: bool },
    Symlink { target: &'a str },
}
#[derive(Clone, Copy, Debug)]
pub struct DiffEntry<'a> {
    pub path: &'a str,
    pub content: DiffContent<'a>,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeFileChange {
    pub path: String,
    pub status: &'static str,
    pub before_bytes: Option<usize>,
    pub after_bytes: Option<usize>,
    pub before_digest: Option<String>,
    pub after_digest: Option<String>,
    pub executable_changed: bool,
    pub binary: bool,
    pub text_diff: Option<Vec<NativeDiffLine>>,
    pub text_omitted: bool,
}

#[derive(serde::Serialize)]
pub struct NativeDiffLine {
    pub op: &'static str,
    pub text: String,
}

pub fn diff_entries(before: &[DiffEntry], after: &[DiffEntry]) -> Vec<NativeFileChange> {
    let before: BTreeMap<_, _> = before.iter().map(|e| (&e.path, &e.content)).collect();
    let after: BTreeMap<_, _> = after.iter().map(|e| (&e.path, &e.content)).collect();
    let mut paths: Vec<_> = before.keys().chain(after.keys()).copied().collect();
    paths.sort();
    paths.dedup();
    let mut budget = 4 * 1024 * 1024usize;
    paths
        .into_iter()
        .filter_map(|path| {
            let previous = before.get(path).copied();
            let current = after.get(path).copied();
            if previous == current {
                return None;
            }
            let bytes = |c: Option<&DiffContent>| {
                c.map(|c| match c {
                    DiffContent::File { bytes, .. } => bytes.len(),
                    DiffContent::Symlink { target } => target.len(),
                })
            };
            let executable = |c: Option<&DiffContent>| {
                matches!(
                    c,
                    Some(DiffContent::File {
                        executable: true,
                        ..
                    })
                )
            };
            let old = text(previous);
            let new = text(current);
            let binary = old.is_none() || new.is_none();
            let text_diff = old.zip(new).and_then(|(old, new)| {
                let len = old.len().saturating_add(new.len());
                if len > budget
                    || len > 1024 * 1024
                    || old
                        .lines()
                        .count()
                        .saturating_add(1)
                        .saturating_mul(new.lines().count().saturating_add(1))
                        > 1_000_000
                {
                    return None;
                }
                budget -= len;
                Some(line_diff(old, new))
            });
            Some(NativeFileChange {
                path: (*path).into(),
                status: if previous.is_none() {
                    "added"
                } else if current.is_none() {
                    "deleted"
                } else {
                    "modified"
                },
                before_bytes: bytes(previous),
                after_bytes: bytes(current),
                before_digest: digest(previous),
                after_digest: digest(current),
                executable_changed: executable(previous) != executable(current),
                binary,
                text_omitted: !binary && text_diff.is_none(),
                text_diff,
            })
        })
        .collect()
}
fn text<'a>(content: Option<&DiffContent<'a>>) -> Option<&'a str> {
    match content {
        None => Some(""),
        Some(DiffContent::Symlink { target }) => Some(target),
        Some(DiffContent::File { bytes, .. }) => {
            if bytes.contains(&0) {
                None
            } else {
                std::str::from_utf8(bytes).ok()
            }
        }
    }
}
fn digest(content: Option<&DiffContent>) -> Option<String> {
    content.map(|c| match c {
        DiffContent::File { bytes, .. } => crate::scm::FileId::from_bytes(bytes).to_string(),
        DiffContent::Symlink { target } => {
            format!("symlink:{}", blake3::hash(target.as_bytes()).to_hex())
        }
    })
}

fn line_diff(old: &str, new: &str) -> Vec<NativeDiffLine> {
    let old: Vec<_> = old.split_inclusive('\n').collect();
    let new: Vec<_> = new.split_inclusive('\n').collect();
    let width = new.len() + 1;
    let mut lcs = vec![0u32; (old.len() + 1) * width];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            lcs[i * width + j] = if old[i] == new[j] {
                1 + lcs[(i + 1) * width + j + 1]
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut lines = Vec::new();
    while i < old.len() || j < new.len() {
        let (op, text) = if i < old.len() && j < new.len() && old[i] == new[j] {
            let text = old[i];
            i += 1;
            j += 1;
            ("equal", text)
        } else if i < old.len()
            && (j == new.len() || lcs[(i + 1) * width + j] >= lcs[i * width + j + 1])
        {
            let text = old[i];
            i += 1;
            ("delete", text)
        } else {
            let text = new[j];
            j += 1;
            ("insert", text)
        };
        lines.push(NativeDiffLine {
            op,
            text: text.into(),
        });
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file<'a>(path: &'a str, bytes: &'a [u8], executable: bool) -> DiffEntry<'a> {
        DiffEntry {
            path,
            content: DiffContent::File { bytes, executable },
        }
    }
    #[test]
    fn exact_newlines_binary_and_empty_presence_remain_distinct() {
        let old = [
            file("text", b"same\r\nold", false),
            file("binary", b"\0a", false),
            file("empty", b"", false),
        ];
        let new = [
            file("text", b"same\r\nnew\n", true),
            file("binary", b"\0b", false),
        ];
        let changes = diff_entries(&old, &new);
        assert_eq!(changes.len(), 3);
        assert!(changes[0].binary && changes[0].text_diff.is_none());
        assert_eq!(changes[1].status, "deleted");
        assert_eq!(changes[1].before_bytes, Some(0));
        assert_eq!(changes[1].after_bytes, None);
        let text = &changes[2];
        assert!(text.executable_changed);
        let lines = text.text_diff.as_ref().unwrap();
        assert_eq!(
            lines
                .iter()
                .map(|l| (l.op, l.text.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("equal", "same\r\n"),
                ("delete", "old"),
                ("insert", "new\n")
            ]
        );
        assert_eq!(
            text.before_digest.as_deref(),
            Some(crate::scm::FileId::from_bytes(b"same\r\nold").as_str())
        );
    }
    #[test]
    fn quadratic_work_and_text_bytes_are_bounded_without_losing_metadata() {
        let many = "a\n".repeat(1100);
        let changed = "b\n".repeat(1100);
        let result = diff_entries(
            &[file("large", many.as_bytes(), false)],
            &[file("large", changed.as_bytes(), false)],
        );
        assert!(result[0].text_omitted && !result[0].binary);
        assert!(result[0].before_digest.is_some() && result[0].after_digest.is_some());
        let huge = vec![b'a'; 1024 * 1024 + 1];
        let result = diff_entries(&[], &[file("huge", &huge, false)]);
        assert!(result[0].text_omitted && result[0].text_diff.is_none());
    }
}
