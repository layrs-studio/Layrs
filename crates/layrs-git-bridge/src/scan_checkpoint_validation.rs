use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::collections::BTreeSet;

/// Validate object identities, annotated-tag bindings and the complete selected
/// commit ancestry. The converter subsequently checks every Tree/blob edge and
/// constructs every native identity from raw bytes using its normal rules.
pub(super) fn validate(scan: &AcquiredScan, limits: GitBridgeLimits) -> Result<()> {
    let width = match scan.metadata.format.as_str() {
        "sha1" => 40,
        "sha256" => 64,
        _ => return Err(invalid("checkpoint Git format")),
    };
    if scan.objects.len() > limits.max_objects
        || scan.metadata.references.len() > limits.max_objects
        || scan.metadata.commits.len() > limits.max_commits
        || scan.metadata.references.is_empty()
        || scan.metadata.commits.is_empty()
    {
        return Err(BridgeError::Limit("scan checkpoint graph"));
    }
    let mut total = 0usize;
    let mut tags = BTreeSet::new();
    for (id, (kind, bytes)) in &scan.objects {
        if hex_id(id)?.len() != width || !["commit", "tree", "blob", "tag"].contains(&kind.as_str())
        {
            return Err(invalid("checkpoint object format/kind mismatch"));
        }
        total = total
            .checked_add(bytes.len())
            .ok_or(BridgeError::Limit("scan checkpoint bytes"))?;
        if bytes.len() > limits.max_object_bytes || total > limits.max_total_bytes {
            return Err(BridgeError::Limit("scan checkpoint bytes"));
        }
        process::verify_git_id(kind, id, bytes)?;
        if kind == "tag" {
            tags.insert(*blake3::hash(bytes).as_bytes());
        }
    }
    let mut parents = BTreeMap::<String, Vec<String>>::new();
    for id in &scan.metadata.commits {
        if parents.contains_key(id) {
            return Err(invalid("checkpoint duplicate commit"));
        }
        let (kind, bytes) = scan
            .objects
            .get(id)
            .ok_or_else(|| invalid("checkpoint commit object missing"))?;
        if kind != "commit" {
            return Err(invalid("checkpoint commit kind mismatch"));
        }
        let headers = import::parse_headers(bytes)?;
        let mut edges = Vec::new();
        for (_, bytes) in headers.iter().filter(|(key, _)| key == "parent") {
            let parent = hex_id(utf8(bytes)?)?;
            if !parents.contains_key(parent) {
                return Err(invalid("checkpoint topological parent missing"));
            }
            edges.push(parent.to_owned());
        }
        parents.insert(id.clone(), edges);
    }
    let mut pending = Vec::new();
    let mut prior = None;
    let mut tag_bytes = 0usize;
    for (name, commit, annotation) in &scan.metadata.references {
        validate_ref(name)?;
        if prior.is_some_and(|prior: &String| prior >= name) || !parents.contains_key(commit) {
            return Err(invalid("checkpoint reference order or target mismatch"));
        }
        prior = Some(name);
        pending.push(commit.as_str());
        if let Some(encoded) = annotation {
            if !name.starts_with("refs/tags/") {
                return Err(invalid("checkpoint branch cannot be an annotated tag"));
            }
            if encoded.len()
                > limits
                    .max_object_bytes
                    .saturating_add(2)
                    .saturating_div(3)
                    .saturating_mul(4)
            {
                return Err(BridgeError::Limit("checkpoint tag bytes"));
            }
            let raw = STANDARD.decode(encoded).map_err(invalid)?;
            tag_bytes = tag_bytes
                .checked_add(raw.len())
                .ok_or(BridgeError::Limit("checkpoint tag bytes"))?;
            if tag_bytes > limits.max_total_bytes || raw.len() > limits.max_object_bytes {
                return Err(BridgeError::Limit("checkpoint tag bytes"));
            }
            if !tags.contains(blake3::hash(&raw).as_bytes()) || STANDARD.encode(&raw) != *encoded {
                return Err(invalid(
                    "checkpoint annotation not bound to verified Git tag",
                ));
            }
            let headers = import::parse_headers(&raw)?;
            if import::header(&headers, "type")? != b"commit"
                || import::header(&headers, "object")? != commit.as_bytes()
            {
                return Err(invalid("checkpoint annotated tag target mismatch"));
            }
        }
    }
    if tag_bytes != scan.metadata.tag_bytes {
        return Err(invalid("checkpoint tag byte count mismatch"));
    }
    let mut reached = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if reached.insert(id) {
            pending.extend(parents[id].iter().map(String::as_str));
        }
    }
    if reached.len() != parents.len() {
        return Err(invalid("checkpoint commits outside selected ancestry"));
    }
    Ok(())
}

fn validate_ref(name: &str) -> Result<()> {
    if (!name.starts_with("refs/heads/") && !name.starts_with("refs/tags/"))
        || name.ends_with('.')
        || name.contains("..")
        || name.contains("@{")
        || name
            .bytes()
            .any(|b| b <= 32 || b == 127 || b"~^:?*[\\".contains(&b))
        || name
            .split('/')
            .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"))
    {
        return Err(invalid(
            "checkpoint reference is not a full valid local Git ref",
        ));
    }
    Ok(())
}
