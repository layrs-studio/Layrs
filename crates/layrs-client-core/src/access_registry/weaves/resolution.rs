use super::{WeaveConflictBlockFile, WeaveConflictFile};
use layrs_lens_sdk::{
    LensBlockResolutionInput, LensResolutionSegment, ResolutionMethod, ResolutionMethods,
    ResolutionScope,
};

fn lens_resolution_methods(lens_id: &str) -> ResolutionMethods {
    match lens_id {
        "layrs.text" => layrs_lens_text::resolution_methods(),
        "layrs.raw" | "layrs.image" | "layrs.code" => layrs_lens_raw::resolution_methods(),
        _ => layrs_lens_raw::resolution_methods(),
    }
}

pub(super) fn resolution_method_labels_for_storage(methods: &[ResolutionMethod]) -> Vec<String> {
    layrs_lens_sdk::resolution_method_labels(methods)
}

pub(super) fn labels_to_methods(labels: &[String]) -> Vec<ResolutionMethod> {
    let mut methods = Vec::new();
    for label in labels {
        if let Some(method) = ResolutionMethod::from_label(label) {
            if !methods.contains(&method) {
                methods.push(method);
            }
        }
    }
    methods
}

fn conflict_file_methods(conflict: &WeaveConflictFile) -> Vec<ResolutionMethod> {
    let methods = labels_to_methods(&conflict.methods);
    if methods.is_empty() {
        lens_resolution_methods(&conflict.lens_id).file
    } else {
        methods
    }
}

fn conflict_block_methods(
    conflict: &WeaveConflictFile,
    block: &WeaveConflictBlockFile,
) -> Vec<ResolutionMethod> {
    let methods = labels_to_methods(&block.methods);
    if methods.is_empty() {
        lens_resolution_methods(&conflict.lens_id).block
    } else {
        methods
    }
}

pub(super) fn conflict_file_method_labels(conflict: &WeaveConflictFile) -> Vec<String> {
    resolution_method_labels_for_storage(&conflict_file_methods(conflict))
}

pub(super) fn conflict_block_method_labels(
    conflict: &WeaveConflictFile,
    block: &WeaveConflictBlockFile,
) -> Vec<String> {
    resolution_method_labels_for_storage(&conflict_block_methods(conflict, block))
}

pub(super) fn validate_file_resolution_method(
    conflict: &WeaveConflictFile,
    resolution: &str,
) -> Result<ResolutionMethod, String> {
    let methods = conflict_file_methods(conflict);
    let Some(method) = ResolutionMethod::from_label(resolution) else {
        return Err(unsupported_resolution_message(
            &conflict.lens_id,
            ResolutionScope::File,
            resolution,
            &methods,
            &conflict.path,
        ));
    };
    if methods.contains(&method) {
        Ok(method)
    } else {
        Err(unsupported_resolution_message(
            &conflict.lens_id,
            ResolutionScope::File,
            resolution,
            &methods,
            &conflict.path,
        ))
    }
}

pub(super) fn validate_block_resolution_method(
    conflict: &WeaveConflictFile,
    block_id: &str,
    resolution: &str,
) -> Result<ResolutionMethod, String> {
    let normalized_id = normalize_block_id(block_id);
    let block = conflict
        .blocks
        .iter()
        .find(|block| block.block_id == normalized_id)
        .ok_or_else(|| {
            format!(
                "No text conflict block {block_id} exists for {}.",
                conflict.path
            )
        })?;
    let methods = conflict_block_methods(conflict, block);
    let Some(method) = ResolutionMethod::from_label(resolution) else {
        return Err(unsupported_resolution_message(
            &conflict.lens_id,
            ResolutionScope::Block,
            resolution,
            &methods,
            &format!("{} block {}", conflict.path, block.block_id),
        ));
    };
    if methods.contains(&method) {
        Ok(method)
    } else {
        Err(unsupported_resolution_message(
            &conflict.lens_id,
            ResolutionScope::Block,
            resolution,
            &methods,
            &format!("{} block {}", conflict.path, block.block_id),
        ))
    }
}

fn unsupported_resolution_message(
    lens_id: &str,
    scope: ResolutionScope,
    requested: &str,
    methods: &[ResolutionMethod],
    target: &str,
) -> String {
    let available = resolution_method_labels_for_storage(methods);
    let available = if available.is_empty() {
        "none".to_string()
    } else {
        available.join(", ")
    };
    format!(
        "Lens {lens_id} does not declare {}-level resolution method `{requested}` for {target}. Available methods: {available}.",
        scope.as_str()
    )
}

fn normalize_block_id(block_id: &str) -> String {
    if block_id.starts_with("block-") {
        block_id.to_string()
    } else {
        format!("block-{block_id}")
    }
}

pub(super) fn parse_block_resolution(resolution: &str) -> Option<(String, String)> {
    let rest = resolution.strip_prefix("block:")?;
    let (block_id, choice) = rest.rsplit_once(':')?;
    Some((block_id.to_string(), choice.to_string()))
}

pub(super) fn resolve_text_conflict_block(
    conflict: &mut WeaveConflictFile,
    block_id: &str,
    method: ResolutionMethod,
    manual_text: Option<&str>,
) -> Result<(), String> {
    if conflict.blocks.is_empty() {
        return Err(format!(
            "Weave conflict {} does not expose text blocks.",
            conflict.path
        ));
    }
    let normalized_id = normalize_block_id(block_id);
    let block = conflict
        .blocks
        .iter_mut()
        .find(|block| block.block_id == normalized_id)
        .ok_or_else(|| {
            format!(
                "No text conflict block {block_id} exists for {}.",
                conflict.path
            )
        })?;
    let resolved_text = block_resolution_text(block, method, manual_text)?;
    block.status = "resolved".to_string();
    block.resolution = Some(method.as_str().to_string());
    block.resolved_text = Some(resolved_text);
    Ok(())
}

pub(super) fn assemble_text_conflict_resolution(
    conflict: &WeaveConflictFile,
) -> Result<Vec<u8>, String> {
    let blocks = conflict
        .blocks
        .iter()
        .map(|block| {
            let method = block
                .resolution
                .as_deref()
                .and_then(ResolutionMethod::from_label)
                .ok_or_else(|| {
                    format!("Text conflict block {} is not resolved.", block.block_id)
                })?;
            Ok(LensBlockResolutionInput {
                block_id: block.block_id.as_str(),
                base: block.base.as_str(),
                existing: block.ours.as_str(),
                incoming: block.theirs.as_str(),
                method,
                manual_text: None,
                resolved_text: block.resolved_text.as_deref(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let segments = conflict
        .segments
        .iter()
        .map(|segment| {
            let kind = match segment.kind.as_str() {
                "text" => layrs_lens_sdk::LensConflictSegmentKind::Text,
                "block" => layrs_lens_sdk::LensConflictSegmentKind::Block,
                other => {
                    return Err(format!(
                        "Unsupported text conflict segment kind `{other}` for {}.",
                        conflict.path
                    ));
                }
            };
            Ok(LensResolutionSegment {
                kind,
                text: segment.text.as_deref(),
                block_id: segment.block_id.as_deref(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let content = layrs_lens_text::resolve_text_conflict(&blocks, &segments)
        .map_err(|error| error.to_string())?;
    Ok(content.bytes)
}

fn block_resolution_text(
    block: &WeaveConflictBlockFile,
    method: ResolutionMethod,
    manual_text: Option<&str>,
) -> Result<String, String> {
    if let Some(resolved_text) = block.resolved_text.as_ref() {
        return Ok(resolved_text.clone());
    }
    layrs_lens_text::resolve_text_block_choice_by_method(
        &block.ours,
        &block.theirs,
        method,
        manual_text,
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_file_resolution_accepts_legacy_aliases_only_when_declared() {
        let conflict = conflict_file("layrs.raw", vec!["existing", "incoming"], Vec::new());

        assert_eq!(
            validate_file_resolution_method(&conflict, "theirs").expect("incoming"),
            ResolutionMethod::Incoming
        );
        assert!(validate_file_resolution_method(&conflict, "manual").is_err());
        assert!(validate_file_resolution_method(&conflict, "base").is_err());
    }

    #[test]
    fn text_block_resolution_rejects_file_scope_and_base_method() {
        let conflict = conflict_file(
            "layrs.text",
            Vec::new(),
            vec!["existing", "incoming", "both", "manual"],
        );

        assert!(validate_file_resolution_method(&conflict, "existing").is_err());
        assert_eq!(
            validate_block_resolution_method(&conflict, "1", "ours").expect("existing"),
            ResolutionMethod::Existing
        );
        assert!(validate_block_resolution_method(&conflict, "1", "base").is_err());
    }

    fn conflict_file(
        lens_id: &str,
        methods: Vec<&str>,
        block_methods: Vec<&str>,
    ) -> WeaveConflictFile {
        WeaveConflictFile {
            conflict_id: "conflict-note".to_string(),
            path: "note.txt".to_string(),
            lens_id: lens_id.to_string(),
            status: "open".to_string(),
            message: "conflict".to_string(),
            methods: methods.into_iter().map(ToString::to_string).collect(),
            resolution: None,
            blocks: vec![WeaveConflictBlockFile {
                block_id: "block-1".to_string(),
                status: "open".to_string(),
                base: "base\n".to_string(),
                ours: "existing\n".to_string(),
                theirs: "incoming\n".to_string(),
                methods: block_methods.into_iter().map(ToString::to_string).collect(),
                resolution: None,
                resolved_text: None,
            }],
            segments: Vec::new(),
        }
    }
}
