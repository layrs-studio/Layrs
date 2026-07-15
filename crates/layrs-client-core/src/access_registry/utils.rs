fn media_type_for_path(path: &str) -> &'static str {
    layrs_lens_sdk::infer_media_type_from_path(Some(Path::new(path)))
        .unwrap_or("application/octet-stream")
}

fn is_text_path(path: &str) -> bool {
    lens_id_for_path(path) == layrs_lens_text::TEXT_LENS_ID
}

fn is_code_path(path: &str) -> bool {
    lens_id_for_path(path) == layrs_lens_code::CODE_LENS_ID
}

fn lens_id_for_path(path: &str) -> &'static str {
    select_builtin_lens(Some(path), None)
        .map(|(manifest, _)| manifest.id.as_str())
        .unwrap_or(layrs_lens_raw::RAW_LENS_ID)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LensMatchReason {
    MediaType,
    Extension,
    RawFallback,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LensSelectionError {
    code: &'static str,
    message: String,
}

impl std::fmt::Display for LensSelectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

fn select_builtin_lens(
    path: Option<&str>,
    media_type: Option<&str>,
) -> Result<(&'static layrs_lens_sdk::LensManifest, LensMatchReason), LensSelectionError> {
    select_lens_from_manifests(canonical_builtin_lens_manifests(), path, media_type)
}

fn built_in_lens_manifest(lens_id: &str) -> Option<&'static layrs_lens_sdk::LensManifest> {
    canonical_builtin_lens_manifests()
        .iter()
        .find(|manifest| manifest.id == lens_id)
}

fn canonical_builtin_lens_manifests() -> &'static [layrs_lens_sdk::LensManifest; 4] {
    static MANIFESTS: std::sync::OnceLock<[layrs_lens_sdk::LensManifest; 4]> =
        std::sync::OnceLock::new();
    MANIFESTS.get_or_init(|| {
        [
            layrs_lens_code::manifest(),
            layrs_lens_image::manifest(),
            layrs_lens_text::manifest(),
            layrs_lens_raw::manifest(),
        ]
    })
}

fn select_lens_from_manifests<'a>(
    manifests: &'a [layrs_lens_sdk::LensManifest],
    path: Option<&str>,
    media_type: Option<&str>,
) -> Result<(&'a layrs_lens_sdk::LensManifest, LensMatchReason), LensSelectionError> {
    if let Some(media_type) = normalize_lens_media_type(media_type) {
        if let Some(manifest) =
            unique_lens_match(manifests, "media type", &media_type, |manifest| {
                manifest
                    .analyzer
                    .supported_media_types
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(&media_type))
            })?
        {
            return Ok((manifest, LensMatchReason::MediaType));
        }
    }

    if let Some(extension) = lens_extension_from_path(path) {
        if let Some(manifest) = unique_lens_match(manifests, "extension", &extension, |manifest| {
            manifest
                .analyzer
                .file_extensions
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(&extension))
        })? {
            return Ok((manifest, LensMatchReason::Extension));
        }
    }

    manifests
        .iter()
        .find(|manifest| manifest.id == layrs_lens_raw::RAW_LENS_ID)
        .map(|manifest| (manifest, LensMatchReason::RawFallback))
        .ok_or_else(|| LensSelectionError {
            code: "missing_raw_lens",
            message: "canonical built-in Lens registry has no Raw fallback".to_string(),
        })
}

fn unique_lens_match<'a>(
    manifests: &'a [layrs_lens_sdk::LensManifest],
    criterion: &str,
    value: &str,
    predicate: impl Fn(&layrs_lens_sdk::LensManifest) -> bool,
) -> Result<Option<&'a layrs_lens_sdk::LensManifest>, LensSelectionError> {
    let matches = manifests
        .iter()
        .filter(|manifest| predicate(manifest))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [manifest] => Ok(Some(*manifest)),
        _ => {
            let mut lens_ids = matches
                .iter()
                .map(|manifest| manifest.id.as_str())
                .collect::<Vec<_>>();
            lens_ids.sort_unstable();
            Err(LensSelectionError {
                code: "ambiguous_lens_match",
                message: format!(
                    "{criterion} `{value}` matches multiple Lenses: {}",
                    lens_ids.join(", ")
                ),
            })
        }
    }
}

fn normalize_lens_media_type(media_type: Option<&str>) -> Option<String> {
    media_type
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn lens_extension_from_path(path: Option<&str>) -> Option<String> {
    Path::new(path?)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::trim)
        .filter(|extension| !extension.is_empty())
        .map(str::to_ascii_lowercase)
}

fn url_path_segment(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

fn remember_local_space(
    meta: &LocalSpaceFile,
    active_layer_id: Option<String>,
) -> Result<(), String> {
    let mut config = DesktopConfig::load_or_create()?;
    config.remember_local_space(LocalSpaceConfigEntry {
        local_space_id: meta.local_space_id.clone(),
        space_id: meta.space_id.clone(),
        root_path: meta.root_path.clone(),
        active_layer_id,
        updated_at_unix: unix_now(),
    });
    config.save()
}

fn remove_local_space_config_entry(local_space_id: &str, root: &Path) -> Result<(), String> {
    let mut config = DesktopConfig::load_or_create()?;
    let root_key = path_compare_key(root);
    config.local_spaces.retain(|entry| {
        entry.local_space_id != local_space_id
            && path_compare_key(&PathBuf::from(&entry.root_path)) != root_key
    });
    config.save()
}

fn layer_dir(layrs_dir: &Path, layer_id: &str) -> PathBuf {
    layrs_dir.join("layers").join(safe_id_fragment(layer_id))
}

fn step_layer_display_name(layrs_dir: &Path, layer_id: &str) -> Option<String> {
    read_json::<LocalSpaceFile>(&layrs_dir.join("local-space.json"))
        .ok()
        .and_then(|meta| {
            meta.layers
                .into_iter()
                .find(|layer| layer.layer_id == layer_id)
                .map(|layer| layer.display_name)
        })
        .filter(|name| !name.trim().is_empty())
}

fn pending_layer_deletions_path(layrs_dir: &Path) -> PathBuf {
    layrs_dir.join("sync").join("pending-layer-deletions.json")
}

fn read_pending_layer_deletions(layrs_dir: &Path) -> Result<PendingLayerDeletionsFile, String> {
    let path = pending_layer_deletions_path(layrs_dir);
    if !path.exists() {
        return Ok(PendingLayerDeletionsFile {
            schema: PENDING_LAYER_DELETIONS_SCHEMA.to_string(),
            deleted_layers: Vec::new(),
        });
    }

    let mut pending: PendingLayerDeletionsFile = read_json(&path)?;
    pending.schema = PENDING_LAYER_DELETIONS_SCHEMA.to_string();
    Ok(pending)
}

fn write_pending_layer_deletions(
    layrs_dir: &Path,
    pending: &PendingLayerDeletionsFile,
) -> Result<(), String> {
    let path = pending_layer_deletions_path(layrs_dir);
    if pending.deleted_layers.is_empty() {
        if path.exists() {
            fs::remove_file(&path).map_err(|error| {
                format!(
                    "Layrs Desktop could not clear pending Layer deletions {}: {error}",
                    path.display()
                )
            })?;
        }
        return Ok(());
    }

    write_json(&path, pending)
}

fn enqueue_pending_layer_deletion(
    layrs_dir: &Path,
    layer_id: &str,
    display_name: &str,
) -> Result<(), String> {
    let mut pending = read_pending_layer_deletions(layrs_dir)?;
    if let Some(existing) = pending
        .deleted_layers
        .iter_mut()
        .find(|deleted| deleted.layer_id == layer_id)
    {
        existing.display_name = display_name.to_string();
        existing.deleted_at_unix = unix_now();
    } else {
        pending.deleted_layers.push(PendingLayerDeletion {
            layer_id: layer_id.to_string(),
            display_name: display_name.to_string(),
            deleted_at_unix: unix_now(),
        });
    }
    write_pending_layer_deletions(layrs_dir, &pending)
}

fn delete_server_layer(
    handle: &LocalSpaceHandle,
    config: &DesktopConfig,
    layer_id: &str,
) -> Result<(), String> {
    ensure_linked_space_ready(handle)?;
    let token = desktop_token(config)?;
    let delete_path = format!(
        "/v1/workspaces/{}/spaces/{}/layers/{}",
        url_path_segment(&handle.meta.workspace_id),
        url_path_segment(&handle.meta.space_id),
        url_path_segment(layer_id)
    );
    let _: Value = delete_json(&config.server_endpoint, &delete_path, Some(&token))?;
    Ok(())
}

fn flush_pending_layer_deletions(
    handle: &LocalSpaceHandle,
    config: &DesktopConfig,
) -> Result<usize, String> {
    let pending = read_pending_layer_deletions(&handle.layrs_dir)?;
    if pending.deleted_layers.is_empty() {
        return Ok(0);
    }

    ensure_linked_space_ready(handle)?;
    let mut synced = 0usize;
    let mut remaining = Vec::new();
    let mut first_error = None;

    for deletion in pending.deleted_layers {
        if !is_probably_server_layer_id(&deletion.layer_id) {
            synced += 1;
            continue;
        }

        if first_error.is_some() {
            remaining.push(deletion);
            continue;
        }

        match delete_server_layer(handle, config, &deletion.layer_id) {
            Ok(()) => synced += 1,
            Err(error) if is_layer_not_found_error(&error) => synced += 1,
            Err(error) => {
                first_error = Some(error);
                remaining.push(deletion);
            }
        }
    }

    write_pending_layer_deletions(
        &handle.layrs_dir,
        &PendingLayerDeletionsFile {
            schema: PENDING_LAYER_DELETIONS_SCHEMA.to_string(),
            deleted_layers: remaining,
        },
    )?;

    if let Some(error) = first_error {
        return Err(format!(
            "Layrs Desktop could not sync pending Layer deletion to Studio: {error}"
        ));
    }

    Ok(synced)
}

fn unique_layer_id(handle: &LocalSpaceHandle, name: &str) -> String {
    let base = safe_id_fragment(name);
    let existing = handle
        .meta
        .layers
        .iter()
        .map(|layer| layer.layer_id.clone())
        .collect::<BTreeSet<_>>();
    let mut candidate = format!("{base}-{}", unix_now());
    let mut index = 2;
    while existing.contains(&candidate) {
        candidate = format!("{base}-{}-{index}", unix_now());
        index += 1;
    }
    candidate
}

fn safe_layer_path_key(layer_id: &str) -> Option<String> {
    if layer_id.is_empty()
        || layer_id == "."
        || layer_id == ".."
        || layer_id.contains('/')
        || layer_id.contains('\\')
        || layer_id.contains(':')
    {
        return None;
    }

    let safe = safe_id_fragment(layer_id);
    if safe.is_empty() || safe == "." || safe == ".." {
        None
    } else {
        Some(safe)
    }
}

fn safe_id_fragment(value: &str) -> String {
    let mut safe = String::with_capacity(value.len().max(4));
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            safe.push(ch.to_ascii_lowercase());
        } else if ch.is_whitespace() || matches!(ch, '/' | '\\' | ':') {
            safe.push('-');
        } else {
            safe.push('_');
        }
    }

    let safe = safe.trim_matches('-').to_string();
    if safe.is_empty() {
        "layer".to_string()
    } else {
        safe
    }
}

fn relative_key(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path.strip_prefix(root).map_err(|error| {
        format!(
            "Layrs Desktop could not create relative path for {}: {error}",
            path.display()
        )
    })?;
    Ok(relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/"))
}

fn path_from_key(root: &Path, key: &str) -> Result<PathBuf, String> {
    validate_snapshot_key(key)?;
    let mut path = root.to_path_buf();
    for segment in key.split('/') {
        path.push(segment);
    }
    Ok(path)
}

fn validate_snapshot_key(key: &str) -> Result<(), String> {
    if key.trim().is_empty() || key.starts_with('/') || key.starts_with('\\') {
        return Err("Layrs Desktop snapshot path is invalid.".to_string());
    }
    for segment in key.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." || segment.contains('\\') {
            return Err(format!("Layrs Desktop snapshot path {key} is invalid."));
        }
    }
    Ok(())
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if path.is_absolute() {
                Ok(path.to_path_buf())
            } else {
                env::current_dir()
                    .map(|cwd| cwd.join(path))
                    .map_err(|cwd_error| {
                        format!(
                            "Layrs Desktop could not resolve local path {}: {cwd_error}",
                            path.display()
                        )
                    })
            }
        }
        Err(error) => Err(format!(
            "Layrs Desktop could not resolve local path {}: {error}",
            path.display()
        )),
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let body = fs::read_to_string(path).map_err(|error| {
        format!(
            "Layrs Desktop could not read JSON file {}: {error}",
            path.display()
        )
    })?;
    serde_json::from_str(&body).map_err(|error| {
        format!(
            "Layrs Desktop JSON file {} is invalid: {error}",
            path.display()
        )
    })
}

fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Layrs Desktop could not create directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let body = serde_json::to_string_pretty(value)
        .map_err(|error| format!("Layrs Desktop could not encode JSON: {error}"))?;
    fs::write(path, body).map_err(|error| {
        format!(
            "Layrs Desktop could not write JSON file {}: {error}",
            path.display()
        )
    })
}

fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn blake3_id(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

fn object_file_stem(object_id: &str) -> &str {
    object_id.strip_prefix("blake3:").unwrap_or(object_id)
}

fn validate_blake3_id(object_id: &str) -> Result<(), String> {
    let Some(hex) = object_id.strip_prefix("blake3:") else {
        return Err(format!("Object id {object_id} is not a blake3 id."));
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "Object id {object_id} is not a canonical blake3 id."
        ));
    }
    Ok(())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn default_linked_state() -> String {
    LOCAL_SPACE_STATE_LINKED.to_string()
}

fn default_layer_lineage_status() -> String {
    "linked".to_string()
}

fn compare_steps_by_timeline(left: &StepFile, right: &StepFile) -> std::cmp::Ordering {
    match (left.timeline_position, right.timeline_position) {
        (Some(left_position), Some(right_position)) => left_position.cmp(&right_position),
        _ => left.captured_at_unix.cmp(&right.captured_at_unix),
    }
    .then_with(|| left.captured_at_unix.cmp(&right.captured_at_unix))
    .then_with(|| left.step_id.cmp(&right.step_id))
}

#[allow(dead_code)]
pub fn scaffold_access_registry(
    workspace_root_input: Option<String>,
    bootstrap: &BootstrapData,
) -> Result<AccessRegistryResult, String> {
    let root = workspace_root(workspace_root_input)?;
    let layers = access_views(&bootstrap.layers, Some(&root))?;
    let pointer_path = root.join(LAYRS_DIR).join("access.json");
    Ok(AccessRegistryResult {
        root: root.display().to_string(),
        pointer_path: pointer_path.display().to_string(),
        layers,
    })
}

#[cfg(test)]
mod lens_selection_tests {
    use super::*;

    #[test]
    fn canonical_extensions_select_each_builtin_and_raw_fallback() {
        let cases = [
            ("src/Main.RS", layrs_lens_code::CODE_LENS_ID),
            ("notes/readme.rst", layrs_lens_text::TEXT_LENS_ID),
            ("assets/photo.WEBP", layrs_lens_image::IMAGE_LENS_ID),
            ("assets/vector.svg", layrs_lens_raw::RAW_LENS_ID),
            ("data/table.csv", layrs_lens_raw::RAW_LENS_ID),
            ("opaque", layrs_lens_raw::RAW_LENS_ID),
        ];

        for (path, expected_lens_id) in cases {
            assert_eq!(lens_id_for_path(path), expected_lens_id, "{path}");
        }
    }

    #[test]
    fn media_type_precedes_extension_and_is_normalized() {
        let (manifest, reason) = select_builtin_lens(
            Some("notes/readme.md"),
            Some(" Application/JSON ; charset=utf-8"),
        )
        .expect("deterministic media match");

        assert_eq!(manifest.id, layrs_lens_code::CODE_LENS_ID);
        assert_eq!(reason, LensMatchReason::MediaType);

        let (manifest, reason) = select_builtin_lens(Some("src/main.rs"), Some("unknown/type"))
            .expect("extension fallback");
        assert_eq!(manifest.id, layrs_lens_code::CODE_LENS_ID);
        assert_eq!(reason, LensMatchReason::Extension);
    }

    #[test]
    fn ambiguous_matches_fail_closed_with_sorted_candidates() {
        let text = layrs_lens_text::manifest();
        let mut duplicate = text.clone();
        duplicate.id = "example.text".to_string();
        let manifests = [text, duplicate, layrs_lens_raw::manifest()];

        let error = select_lens_from_manifests(&manifests, Some("README.md"), None)
            .expect_err("duplicate extension must fail closed");

        assert_eq!(error.code, "ambiguous_lens_match");
        assert!(error.message.contains("example.text, layrs.text"));
    }

    #[test]
    fn media_type_inference_uses_sdk_contract_without_extra_extensions() {
        assert_eq!(media_type_for_path("src/main.rs"), "text/rust");
        assert_eq!(media_type_for_path("assets/photo.jpeg"), "image/jpeg");
        assert_eq!(
            media_type_for_path("assets/vector.svg"),
            "application/octet-stream"
        );
    }
}
