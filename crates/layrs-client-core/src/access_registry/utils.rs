fn media_type_for_path(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".svg") {
        "image/svg+xml"
    } else if lower.ends_with(".json") {
        "application/json"
    } else {
        "text/plain"
    }
}

fn is_text_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".txt")
        || lower.ends_with(".md")
        || lower.ends_with(".log")
        || lower.ends_with(".ini")
        || lower.ends_with(".env")
}

fn is_code_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".json")
        || lower.ends_with(".toml")
        || lower.ends_with(".yaml")
        || lower.ends_with(".yml")
        || lower.ends_with(".rs")
        || lower.ends_with(".ts")
        || lower.ends_with(".tsx")
        || lower.ends_with(".js")
        || lower.ends_with(".jsx")
        || lower.ends_with(".css")
        || lower.ends_with(".html")
        || lower.ends_with(".csv")
}

fn lens_id_for_path(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".webp")
        || lower.ends_with(".gif")
        || lower.ends_with(".svg")
    {
        "layrs.image"
    } else if is_code_path(path) {
        "layrs.code"
    } else if is_text_path(path) {
        "layrs.text"
    } else {
        "layrs.raw"
    }
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
