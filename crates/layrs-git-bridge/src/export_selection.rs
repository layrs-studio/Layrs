use super::*;

/// Canonicalize the scope before constructing the existing v1 checkpoint.
/// Order is immaterial; duplicates are an input error, never silently dropped.
pub(super) fn normalize(mode: ExportMode, limits: GitBridgeLimits) -> Result<ExportMode> {
    let mut mode = mode;
    let references = match &mut mode {
        ExportMode::History => return Ok(mode),
        ExportMode::Snapshot { reference, .. } => std::slice::from_mut(reference),
        ExportMode::SelectedHistory { references } => references.as_mut_slice(),
    };
    if references.is_empty() {
        return Err(invalid("empty export reference selection"));
    }
    if references.len() > limits.max_objects {
        return Err(BridgeError::Limit("export selected references"));
    }
    let mut bytes = 0usize;
    for reference in references.iter() {
        bytes = bytes
            .checked_add(reference.len())
            .ok_or(BridgeError::Limit("export selection bytes"))?;
        if bytes > limits.max_total_bytes.min(16_384) {
            return Err(BridgeError::Limit("export selection checkpoint bytes"));
        }
        if (!reference.starts_with("refs/heads/") && !reference.starts_with("refs/tags/"))
            || reference.bytes().any(|b| b < 32 || b == 127)
        {
            return Err(invalid("select exact exported branch or tag names"));
        }
    }
    references.sort();
    if references.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(invalid("duplicate export reference selection"));
    }
    let git = process::Git::new(Path::new("."), limits)?;
    for reference in references.iter() {
        git.run(&["check-ref-format", reference], None, 256)?;
    }
    if canonical_json(&mode).map_err(invalid)?.len() > limits.max_total_bytes.min(16_384) {
        return Err(BridgeError::Limit("export selection checkpoint bytes"));
    }
    Ok(mode)
}
