fn capture_working_state(
    root: &Path,
    layer_id: &str,
    write_objects: bool,
) -> Result<WorkingStateFile, String> {
    let layrs_dir = root.join(LAYRS_DIR);
    if write_objects {
        create_local_space_directories(&layrs_dir)?;
    }
    let ignore_rules = read_ignore_rules(root)?;
    let mut next_cache = BTreeMap::new();
    let mut files = Vec::new();
    collect_files(
        root,
        root,
        &layrs_dir,
        &ignore_rules,
        &mut next_cache,
        &mut files,
        write_objects,
    )?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let root_tree_id = if write_objects {
        Some(write_tree_object(&layrs_dir, &files)?)
    } else {
        Some(tree_id_for_files(&files))
    };
    if write_objects {
        write_scan_cache(&layrs_dir, next_cache)?;
    }

    Ok(WorkingStateFile {
        schema: WORKING_STATE_SCHEMA.to_string(),
        layer_id: layer_id.to_string(),
        captured_at_unix: unix_now(),
        root_tree_id,
        files,
    })
}

fn collect_files(
    root: &Path,
    current: &Path,
    layrs_dir: &Path,
    ignore_rules: &IgnoreRules,
    next_cache: &mut BTreeMap<String, ScanCacheEntry>,
    files: &mut Vec<FileSnapshotEntry>,
    write_objects: bool,
) -> Result<(), String> {
    let mut entries = fs::read_dir(current)
        .map_err(|error| {
            format!(
                "Layrs Desktop could not scan working tree {}: {error}",
                current.display()
            )
        })?
        .collect::<Result<Vec<_>, io::Error>>()
        .map_err(|error| format!("Layrs Desktop could not read working tree entry: {error}"))?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        if path.file_name().and_then(|name| name.to_str()) == Some(LAYRS_DIR)
            || path.file_name().and_then(|name| name.to_str()) == Some(".git")
        {
            continue;
        }
        let key = relative_key(root, &path)?;
        if ignore_rules.ignores(&key) {
            continue;
        }

        let file_type = entry.file_type().map_err(|error| {
            format!(
                "Layrs Desktop could not inspect working tree path {}: {error}",
                path.display()
            )
        })?;

        if file_type.is_dir() {
            collect_files(
                root,
                &path,
                layrs_dir,
                ignore_rules,
                next_cache,
                files,
                write_objects,
            )?;
        } else if file_type.is_file() {
            let metadata = entry.metadata().map_err(|error| {
                format!(
                    "Layrs Desktop could not inspect working tree file {}: {error}",
                    path.display()
                )
            })?;
            let size = metadata.len();
            let modified_at = metadata
                .modified()
                .map(system_time_marker)
                .unwrap_or_else(|_| "unknown".to_string());

            let bytes = fs::read(&path).map_err(|error| {
                format!(
                    "Layrs Desktop could not read working tree file {}: {error}",
                    path.display()
                )
            })?;
            let snapshot = write_file_object(layrs_dir, &key, &bytes, write_objects)?;
            next_cache.insert(
                key,
                ScanCacheEntry {
                    path: snapshot.path.clone(),
                    size,
                    modified_at,
                    snapshot: snapshot.clone(),
                },
            );
            files.push(snapshot);
        }
    }

    Ok(())
}

#[derive(Debug, Default)]
struct IgnoreRules {
    rules: Vec<IgnoreRule>,
}

#[derive(Debug)]
struct IgnoreRule {
    pattern: String,
    directory_only: bool,
}

impl IgnoreRules {
    fn ignores(&self, path_key: &str) -> bool {
        self.rules.iter().any(|rule| rule.matches(path_key))
    }
}

impl IgnoreRule {
    fn matches(&self, path_key: &str) -> bool {
        let normalized = path_key.replace('\\', "/");
        let pattern = self.pattern.as_str();

        if self.directory_only {
            return normalized == pattern || normalized.starts_with(&format!("{pattern}/"));
        }

        if let Some(suffix) = pattern.strip_prefix('*') {
            return normalized.ends_with(suffix);
        }

        if pattern.contains('/') {
            return normalized == pattern || normalized.starts_with(&format!("{pattern}/"));
        }

        normalized
            .split('/')
            .any(|segment| segment == pattern || wildcard_segment_matches(pattern, segment))
    }
}

fn wildcard_segment_matches(pattern: &str, segment: &str) -> bool {
    if let Some((prefix, suffix)) = pattern.split_once('*') {
        segment.starts_with(prefix) && segment.ends_with(suffix)
    } else {
        false
    }
}

fn read_ignore_rules(root: &Path) -> Result<IgnoreRules, String> {
    let path = root.join(".layrsignore");
    if !path.exists() {
        return Ok(IgnoreRules::default());
    }

    let body = fs::read_to_string(&path).map_err(|error| {
        format!(
            "Layrs Desktop could not read ignore file {}: {error}",
            path.display()
        )
    })?;
    let rules = body
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            let directory_only = trimmed.ends_with('/');
            let pattern = trimmed
                .trim_matches('/')
                .replace('\\', "/")
                .trim()
                .to_string();
            if pattern.is_empty() {
                None
            } else {
                Some(IgnoreRule {
                    pattern,
                    directory_only,
                })
            }
        })
        .collect();
    Ok(IgnoreRules { rules })
}

fn materialize_state(root: &Path, state: &WorkingStateFile) -> Result<(), String> {
    let current = capture_working_state(root, &state.layer_id, false)?;
    let (added, modified, deleted) = diff_state(Some(&current), state);
    let target_files = file_entries(&state.files);
    let mut prepared = Vec::new();

    for path_key in added.iter().chain(modified.iter()) {
        let file = target_files.get(path_key).ok_or_else(|| {
            format!("Layrs Desktop cannot restore missing tree entry {path_key}.")
        })?;
        let bytes = read_snapshot_object_bytes(&root.join(LAYRS_DIR), file)?;
        prepared.push((path_key.clone(), bytes));
    }

    for (path_key, bytes) in prepared {
        let target = path_from_key(root, &path_key)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "Layrs Desktop could not create directory while switching Layer {}: {error}",
                    parent.display()
                )
            })?;
        }
        fs::write(&target, bytes).map_err(|error| {
            format!(
                "Layrs Desktop could not restore file {}: {error}",
                target.display()
            )
        })?;
    }

    for path_key in deleted {
        let path = path_from_key(root, &path_key)?;
        if path.exists() {
            fs::remove_file(&path).map_err(|error| {
                format!(
                    "Layrs Desktop could not remove file while switching Layer {}: {error}",
                    path.display()
                )
            })?;
        }
    }

    Ok(())
}

fn read_layer_state(layrs_dir: &Path, layer_id: &str) -> Result<WorkingStateFile, String> {
    read_state_file(
        layrs_dir,
        &layer_dir(layrs_dir, layer_id).join("working-state.json"),
    )
}

fn read_layer_index(layrs_dir: &Path, layer_id: &str) -> Result<WorkingStateFile, String> {
    read_state_file(
        layrs_dir,
        &layer_dir(layrs_dir, layer_id).join("index.json"),
    )
}

fn read_state_file(layrs_dir: &Path, path: &Path) -> Result<WorkingStateFile, String> {
    let mut state = read_json::<WorkingStateFile>(path)?;
    hydrate_state_files(layrs_dir, &mut state)?;
    Ok(state)
}

fn hydrate_state_files(layrs_dir: &Path, state: &mut WorkingStateFile) -> Result<(), String> {
    if state.files.is_empty() {
        if let Some(root_tree_id) = state.root_tree_id.clone() {
            state.files = read_tree_object(layrs_dir, &root_tree_id)?.files;
        }
    } else if state.root_tree_id.is_none() {
        state.root_tree_id = Some(write_tree_object(layrs_dir, &state.files)?);
    }
    Ok(())
}

fn storage_state(layrs_dir: &Path, state: &WorkingStateFile) -> Result<WorkingStateFile, String> {
    let root_tree_id = state
        .root_tree_id
        .clone()
        .map(Ok)
        .unwrap_or_else(|| write_tree_object(layrs_dir, &state.files))?;
    Ok(WorkingStateFile {
        schema: WORKING_STATE_SCHEMA.to_string(),
        layer_id: state.layer_id.clone(),
        captured_at_unix: state.captured_at_unix,
        root_tree_id: Some(root_tree_id),
        files: Vec::new(),
    })
}

fn write_layer_state(
    layrs_dir: &Path,
    layer_id: &str,
    state: &WorkingStateFile,
) -> Result<(), String> {
    let dir = layer_dir(layrs_dir, layer_id);
    let state = storage_state(layrs_dir, state)?;
    write_json(&dir.join("working-state.json"), &state)?;
    write_json(&dir.join("index.json"), &state)
}

fn write_working_state(
    layrs_dir: &Path,
    layer_id: &str,
    state: &WorkingStateFile,
) -> Result<(), String> {
    let state = storage_state(layrs_dir, state)?;
    write_json(
        &layer_dir(layrs_dir, layer_id).join("working-state.json"),
        &state,
    )
}

fn write_step(
    layrs_dir: &Path,
    layer_id: &str,
    state: &WorkingStateFile,
) -> Result<String, String> {
    let step_id = unique_step_id(layrs_dir, layer_id);
    let (parent_step_id, base_layer_id, base_tree_id, base_state) = step_base(layrs_dir, layer_id)?;
    let (added, modified, deleted) = diff_state(base_state.as_ref(), state);
    let changed_paths = added
        .iter()
        .chain(modified.iter())
        .chain(deleted.iter())
        .cloned()
        .collect::<Vec<_>>();
    let root_tree_id = state
        .root_tree_id
        .clone()
        .map(Ok)
        .unwrap_or_else(|| write_tree_object(layrs_dir, &state.files))?;
    let step = StepFile {
        schema: STEP_SCHEMA.to_string(),
        step_id: step_id.clone(),
        layer_id: layer_id.to_string(),
        parent_step_id,
        base_layer_id,
        base_tree_id,
        root_tree_id: Some(root_tree_id),
        changed_paths,
        timeline_position: Some(next_timeline_position(layrs_dir, layer_id)?),
        origin_layer_id: Some(layer_id.to_string()),
        origin_layer_name: step_layer_display_name(layrs_dir, layer_id),
        origin_step_id: Some(step_id.clone()),
        step_kind: Some("native".to_string()),
        captured_at_unix: state.captured_at_unix,
        files: Vec::new(),
    };
    write_json(
        &layer_dir(layrs_dir, layer_id)
            .join("steps")
            .join(format!("{step_id}.json")),
        &step,
    )?;
    Ok(step_id)
}

fn write_step_and_pending_publish(
    layrs_dir: &Path,
    layer_id: &str,
    state: &WorkingStateFile,
) -> Result<String, String> {
    let step_id = write_step(layrs_dir, layer_id, state)?;
    let step = read_step_file(layrs_dir, layer_id, &step_id)?;
    write_pending_publish(layrs_dir, &step)?;
    Ok(step_id)
}

fn unique_step_id(layrs_dir: &Path, layer_id: &str) -> String {
    let layer_hash = fnv1a_hex(layer_id.as_bytes());
    for attempt in 0..1000u32 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let step_id = format!(
            "{}{:09}-{}-{attempt}",
            now.as_secs(),
            now.subsec_nanos(),
            layer_hash
        );
        if !layer_dir(layrs_dir, layer_id)
            .join("steps")
            .join(format!("{step_id}.json"))
            .exists()
        {
            return step_id;
        }
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{now}-{layer_hash}-fallback")
}

fn next_timeline_position(layrs_dir: &Path, layer_id: &str) -> Result<u64, String> {
    Ok(read_step_files(layrs_dir, layer_id)?
        .into_iter()
        .filter_map(|step| step.timeline_position)
        .max()
        .map_or(0, |position| position.saturating_add(1)))
}

fn read_step_file(layrs_dir: &Path, layer_id: &str, step_id: &str) -> Result<StepFile, String> {
    read_json(
        &layer_dir(layrs_dir, layer_id)
            .join("steps")
            .join(format!("{step_id}.json")),
    )
}

fn pending_publish_dir(layrs_dir: &Path, layer_id: &str) -> PathBuf {
    layer_dir(layrs_dir, layer_id).join("pending-publish")
}

fn write_pending_publish(layrs_dir: &Path, step: &StepFile) -> Result<(), String> {
    let pending = PendingPublishFile {
        schema: PENDING_PUBLISH_SCHEMA.to_string(),
        step_id: step.step_id.clone(),
        layer_id: step.layer_id.clone(),
        root_tree_id: step.root_tree_id.clone(),
        changed_paths: step.changed_paths.clone(),
        created_at_unix: unix_now(),
    };
    write_json(
        &pending_publish_dir(layrs_dir, &step.layer_id).join(format!("{}.json", step.step_id)),
        &pending,
    )
}

fn read_pending_publish_files(
    layrs_dir: &Path,
    layer_id: &str,
) -> Result<Vec<PendingPublishFile>, String> {
    let dir = pending_publish_dir(layrs_dir, layer_id);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut pending = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|error| {
        format!(
            "Layrs Desktop could not read pending publish directory {}: {error}",
            dir.display()
        )
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "Layrs Desktop could not read pending publish entry {}: {error}",
                dir.display()
            )
        })?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            pending.push(read_json::<PendingPublishFile>(&path)?);
        }
    }

    pending.sort_by(|left, right| {
        left.created_at_unix
            .cmp(&right.created_at_unix)
            .then_with(|| left.step_id.cmp(&right.step_id))
    });
    Ok(pending)
}

fn latest_pending_publish_step(
    layrs_dir: &Path,
    layer_id: &str,
) -> Result<Option<StepFile>, String> {
    let Some(pending) = read_pending_publish_files(layrs_dir, layer_id)?
        .last()
        .cloned()
    else {
        return Ok(None);
    };
    Ok(Some(read_step_file(layrs_dir, layer_id, &pending.step_id)?))
}

fn pending_publish_steps(layrs_dir: &Path, layer_id: &str) -> Result<Vec<StepFile>, String> {
    read_pending_publish_files(layrs_dir, layer_id)?
        .into_iter()
        .map(|pending| read_step_file(layrs_dir, layer_id, &pending.step_id))
        .collect()
}

fn clear_pending_publish(layrs_dir: &Path, layer_id: &str) -> Result<(), String> {
    let dir = pending_publish_dir(layrs_dir, layer_id);
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&dir).map_err(|error| {
        format!(
            "Layrs Desktop could not read pending publish directory {}: {error}",
            dir.display()
        )
    })? {
        let path = entry
            .map_err(|error| {
                format!(
                    "Layrs Desktop could not read pending publish entry {}: {error}",
                    dir.display()
                )
            })?
            .path();
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            fs::remove_file(&path).map_err(|error| {
                format!(
                    "Layrs Desktop could not remove pending publish file {}: {error}",
                    path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn step_base(
    layrs_dir: &Path,
    layer_id: &str,
) -> Result<
    (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<WorkingStateFile>,
    ),
    String,
> {
    let mut steps = read_step_files(layrs_dir, layer_id)?;
    steps.sort_by(compare_steps_by_timeline);
    if let Some(previous_step) = steps.last() {
        let state = state_from_step(layrs_dir, previous_step)?;
        return Ok((
            Some(previous_step.step_id.clone()),
            Some(layer_id.to_string()),
            state.root_tree_id.clone(),
            Some(state),
        ));
    }

    let parent_layer_id = read_json::<LocalSpaceFile>(&layrs_dir.join("local-space.json"))
        .ok()
        .and_then(|meta| {
            meta.layers
                .into_iter()
                .find(|layer| layer.layer_id == layer_id)
                .and_then(|layer| layer.parent_layer_id)
        });
    let base_layer_id = parent_layer_id.unwrap_or_else(|| layer_id.to_string());
    let base_state = read_layer_index(layrs_dir, &base_layer_id).ok();
    Ok((
        None,
        Some(base_layer_id),
        base_state
            .as_ref()
            .and_then(|state| state.root_tree_id.clone()),
        base_state,
    ))
}

fn state_from_step(layrs_dir: &Path, step: &StepFile) -> Result<WorkingStateFile, String> {
    let mut state = WorkingStateFile {
        schema: WORKING_STATE_SCHEMA.to_string(),
        layer_id: step.layer_id.clone(),
        captured_at_unix: step.captured_at_unix,
        root_tree_id: step.root_tree_id.clone(),
        files: step.files.clone(),
    };
    hydrate_state_files(layrs_dir, &mut state)?;
    Ok(state)
}

fn recorded_base_state_for_step(layrs_dir: &Path, step: &StepFile) -> Option<WorkingStateFile> {
    let root_tree_id = step.base_tree_id.clone()?;
    let mut state = WorkingStateFile {
        schema: WORKING_STATE_SCHEMA.to_string(),
        layer_id: step
            .base_layer_id
            .clone()
            .unwrap_or_else(|| step.layer_id.clone()),
        captured_at_unix: step.captured_at_unix,
        root_tree_id: Some(root_tree_id),
        files: Vec::new(),
    };
    hydrate_state_files(layrs_dir, &mut state).ok()?;
    Some(state)
}

fn tree_id_for_files(files: &[FileSnapshotEntry]) -> String {
    let mut material = String::new();
    for file in files {
        material.push_str(&file.path);
        material.push('\0');
        material.push_str(&file.hash);
        material.push('\0');
        material.push_str(&file.size.to_string());
        material.push('\n');
    }
    blake3_id(material.as_bytes())
}

fn write_tree_object(layrs_dir: &Path, files: &[FileSnapshotEntry]) -> Result<String, String> {
    let tree_id = tree_id_for_files(files);
    let path = tree_object_path(layrs_dir, &tree_id);
    if !path.exists() {
        let tree = TreeObjectFile {
            schema: TREE_OBJECT_SCHEMA.to_string(),
            tree_id: tree_id.clone(),
            files: files.to_vec(),
        };
        write_json(&path, &tree)?;
    }
    Ok(tree_id)
}

fn read_tree_object(layrs_dir: &Path, tree_id: &str) -> Result<TreeObjectFile, String> {
    read_json(&tree_object_path(layrs_dir, tree_id))
}

fn tree_object_path(layrs_dir: &Path, tree_id: &str) -> PathBuf {
    layrs_dir
        .join("objects")
        .join("trees")
        .join(format!("{}.json", object_file_stem(tree_id)))
}

fn write_file_object(
    layrs_dir: &Path,
    path: &str,
    bytes: &[u8],
    write_objects: bool,
) -> Result<FileSnapshotEntry, String> {
    validate_snapshot_key(path)?;
    let hash = blake3_id(bytes);
    let object = format!("objects/files/{}.json", object_file_stem(&hash));
    if write_objects {
        write_file_object_manifest(layrs_dir, &hash, bytes, media_type_for_path(path))?;
    }

    Ok(FileSnapshotEntry {
        path: path.to_string(),
        object,
        hash,
        size: bytes.len() as u64,
    })
}

fn read_snapshot_object_bytes(
    layrs_dir: &Path,
    file: &FileSnapshotEntry,
) -> Result<Vec<u8>, String> {
    let object_path = layrs_dir.join(&file.object);
    if file.object.starts_with("objects/files/") {
        let manifest = read_json::<FileObjectFile>(&object_path)?;
        let bytes = read_file_object_bytes(layrs_dir, &manifest)?;
        if blake3_id(&bytes) != file.hash {
            return Err(format!(
                "Layrs Desktop object hash mismatch while reading {}.",
                file.path
            ));
        }
        return Ok(bytes);
    }

    fs::read(&object_path).map_err(|error| {
        format!(
            "Layrs Desktop could not read snapshot object {}: {error}",
            object_path.display()
        )
    })
}

fn write_scan_cache(
    layrs_dir: &Path,
    entries: BTreeMap<String, ScanCacheEntry>,
) -> Result<(), String> {
    let cache = ScanCacheFile {
        schema: SCAN_CACHE_SCHEMA.to_string(),
        entries: entries.into_values().collect(),
    };
    write_json(&layrs_dir.join("scan-cache.json"), &cache)
}

fn system_time_marker(time: SystemTime) -> String {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| format!("{}.{}", duration.as_secs(), duration.subsec_nanos()))
        .unwrap_or_else(|_| "unknown".to_string())
}
