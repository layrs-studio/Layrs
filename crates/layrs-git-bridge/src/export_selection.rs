use super::*;

/// Canonicalize the scope before constructing the existing v1 checkpoint.
/// Order is immaterial; duplicates are an input error, never silently dropped.
pub(super) fn normalize(mode: ExportMode, limits: GitBridgeLimits) -> Result<ExportMode> {
    let ExportMode::SelectedHistory { mut references } = mode else {
        return Ok(mode);
    };
    if references.is_empty() {
        return Err(invalid("empty export reference selection"));
    }
    if references.len() > limits.max_objects {
        return Err(BridgeError::Limit("export selected references"));
    }
    let mut bytes = 0usize;
    for reference in &references {
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
    Ok(ExportMode::SelectedHistory { references })
}
