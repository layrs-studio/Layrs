fn diff_state(
    previous: Option<&WorkingStateFile>,
    current: &WorkingStateFile,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let previous_files = previous
        .map(|state| file_hashes(&state.files))
        .unwrap_or_default();
    let current_files = file_hashes(&current.files);

    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();

    for (path, hash) in &current_files {
        match previous_files.get(path) {
            None => added.push(path.clone()),
            Some(previous_hash) if previous_hash != hash => modified.push(path.clone()),
            _ => {}
        }
    }

    for path in previous_files.keys() {
        if !current_files.contains_key(path) {
            deleted.push(path.clone());
        }
    }

    (added, modified, deleted)
}

fn changed_file_count(previous: Option<&WorkingStateFile>, current: &WorkingStateFile) -> usize {
    let (added, modified, deleted) = diff_state(previous, current);
    added.len() + modified.len() + deleted.len()
}

fn lens_diff_entries(
    handle: &LocalSpaceHandle,
    source: &str,
    layer_id: &str,
    step_id: Option<&str>,
    previous: Option<&WorkingStateFile>,
    current: &WorkingStateFile,
    added: &[String],
    modified: &[String],
    deleted: &[String],
) -> Vec<LensDiffEntry> {
    let previous_files = previous
        .map(|state| file_entries(&state.files))
        .unwrap_or_default();
    let current_files = file_entries(&current.files);
    let mut diffs = Vec::with_capacity(added.len() + modified.len() + deleted.len());

    for path in added {
        if let Some(diff) = lens_diff_entry(
            handle,
            path,
            "added",
            "Added locally",
            None,
            current_files.get(path),
            DiffWindowOptions::scan(source, layer_id, step_id),
        ) {
            diffs.push(diff);
        }
    }

    for path in modified {
        if let Some(diff) = lens_diff_entry(
            handle,
            path,
            "modified",
            "Modified locally",
            previous_files.get(path),
            current_files.get(path),
            DiffWindowOptions::scan(source, layer_id, step_id),
        ) {
            diffs.push(diff);
        }
    }

    for path in deleted {
        if let Some(diff) = lens_diff_entry(
            handle,
            path,
            "deleted",
            "Deleted locally",
            previous_files.get(path),
            None,
            DiffWindowOptions::scan(source, layer_id, step_id),
        ) {
            diffs.push(diff);
        }
    }

    diffs
}

fn load_step_diff_window(
    handle: &LocalSpaceHandle,
    path: &str,
    step_id: &str,
    start: usize,
    limit: usize,
) -> Result<LensDiffEntry, String> {
    let layer_id = handle.active.layer_id.clone();
    let mut steps = read_step_files(&handle.layrs_dir, &layer_id)?;
    steps.sort_by(compare_steps_by_timeline);

    let mut previous_state: Option<WorkingStateFile> = None;
    for step in steps {
        let current_state = state_from_step(&handle.layrs_dir, &step)?;
        let base_state = previous_state
            .clone()
            .or_else(|| recorded_base_state_for_step(&handle.layrs_dir, &step))
            .or_else(|| initial_step_base_state(handle, &layer_id));
        if step.step_id == step_id {
            return diff_window_for_path(
                handle,
                "localStep",
                &step.layer_id,
                Some(&step.step_id),
                base_state.as_ref(),
                &current_state,
                path,
                start,
                limit,
            );
        }
        previous_state = Some(current_state);
    }

    Err(format!(
        "Layrs Desktop could not find Local Step {step_id} on the active Layer."
    ))
}

fn diff_window_for_path(
    handle: &LocalSpaceHandle,
    source: &str,
    layer_id: &str,
    step_id: Option<&str>,
    previous: Option<&WorkingStateFile>,
    current: &WorkingStateFile,
    path: &str,
    start: usize,
    limit: usize,
) -> Result<LensDiffEntry, String> {
    let previous_files = previous
        .map(|state| file_entries(&state.files))
        .unwrap_or_default();
    let current_files = file_entries(&current.files);
    let old_file = previous_files.get(path);
    let new_file = current_files.get(path);
    let (state, title) = match (old_file, new_file) {
        (None, Some(_)) => ("added", "Added locally"),
        (Some(_), None) => ("deleted", "Deleted locally"),
        (Some(old_file), Some(new_file)) if old_file.hash != new_file.hash => {
            ("modified", "Modified locally")
        }
        (Some(_), Some(_)) => {
            return Err(format!(
                "Layrs Desktop did not find text changes for {path} in source {source}."
            ));
        }
        (None, None) => {
            return Err(format!(
                "Layrs Desktop could not find {path} in source {source}."
            ));
        }
    };

    lens_diff_entry(
        handle,
        path,
        state,
        title,
        old_file,
        new_file,
        DiffWindowOptions::requested(source, layer_id, step_id, start, limit),
    )
    .ok_or_else(|| format!("Layrs Desktop could not build a diff for {path}."))
}

fn local_step_summaries(
    handle: &LocalSpaceHandle,
    layer_id: &str,
) -> Result<Vec<LocalStepSummary>, String> {
    let mut steps = read_step_files(&handle.layrs_dir, layer_id)?;
    steps.sort_by(compare_steps_by_timeline);

    let mut summaries = Vec::with_capacity(steps.len());
    let mut previous_state: Option<WorkingStateFile> = None;

    for step in steps {
        let current_state = state_from_step(&handle.layrs_dir, &step)?;
        let base_state = previous_state
            .clone()
            .or_else(|| recorded_base_state_for_step(&handle.layrs_dir, &step))
            .or_else(|| initial_step_base_state(handle, layer_id));
        let (added, modified, deleted) = diff_state(base_state.as_ref(), &current_state);
        let diffs = lens_diff_entries(
            handle,
            "localStep",
            &step.layer_id,
            Some(&step.step_id),
            base_state.as_ref(),
            &current_state,
            &added,
            &modified,
            &deleted,
        );
        let changed_files = added.len() + modified.len() + deleted.len();
        let diff_stats = diff_stats(&diffs);

        summaries.push(LocalStepSummary {
            step_id: step.step_id.clone(),
            layer_id: step.layer_id.clone(),
            captured_at: step.captured_at_unix,
            timeline_position: step.timeline_position,
            origin_layer_id: step.origin_layer_id.clone(),
            origin_layer_name: step.origin_layer_name.clone(),
            origin_step_id: step.origin_step_id.clone(),
            step_kind: step.step_kind.clone(),
            changed_files,
            diff_stats,
            diffs,
        });
        previous_state = Some(current_state);
    }

    Ok(summaries)
}

fn layer_step_activities(
    layrs_dir: &Path,
    layers: &[LocalLayerMetadata],
) -> Result<Vec<LayerStepActivity>, String> {
    let mut activities = Vec::with_capacity(layers.len());

    for layer in layers {
        let steps = read_step_files(layrs_dir, &layer.layer_id)?;
        let latest_step_at = steps
            .iter()
            .map(|step| step.captured_at_unix)
            .max()
            .unwrap_or(0);

        activities.push(LayerStepActivity {
            layer_id: layer.layer_id.clone(),
            latest_step_at,
            step_count: steps.len(),
        });
    }

    Ok(activities)
}

fn initial_step_base_state(handle: &LocalSpaceHandle, layer_id: &str) -> Option<WorkingStateFile> {
    let parent_layer_id = handle
        .meta
        .layers
        .iter()
        .find(|layer| layer.layer_id == layer_id)
        .and_then(|layer| layer.parent_layer_id.as_deref());

    if let Some(parent_layer_id) = parent_layer_id {
        read_layer_state(&handle.layrs_dir, parent_layer_id)
            .or_else(|_| read_layer_index(&handle.layrs_dir, parent_layer_id))
            .ok()
    } else {
        read_layer_index(&handle.layrs_dir, layer_id).ok()
    }
}

fn read_step_files(layrs_dir: &Path, layer_id: &str) -> Result<Vec<StepFile>, String> {
    let steps_dir = layer_dir(layrs_dir, layer_id).join("steps");
    if !steps_dir.exists() {
        return Ok(Vec::new());
    }

    let mut steps = Vec::new();
    let entries = fs::read_dir(&steps_dir).map_err(|error| {
        format!(
            "Layrs Desktop could not read Layer steps {}: {error}",
            steps_dir.display()
        )
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "Layrs Desktop could not read Layer step entry {}: {error}",
                steps_dir.display()
            )
        })?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            steps.push(read_json::<StepFile>(&path)?);
        }
    }

    Ok(steps)
}

fn lens_diff_entry(
    handle: &LocalSpaceHandle,
    path: &str,
    state: &str,
    title: &str,
    old_file: Option<&FileSnapshotEntry>,
    new_file: Option<&FileSnapshotEntry>,
    options: DiffWindowOptions<'_>,
) -> Option<LensDiffEntry> {
    let lens_id = lens_id_for_path(path).to_string();
    let (diff, message) = if lens_id == "layrs.code" || lens_id == "layrs.text" {
        let old_text = old_file
            .map(|file| read_text_object_for_diff(handle, file))
            .unwrap_or_else(TextDiffSource::absent);
        let new_text = new_file
            .map(|file| read_text_object_for_diff(handle, file))
            .unwrap_or_else(TextDiffSource::absent);
        let message = text_diff_message(
            old_file,
            new_file,
            old_text.readable,
            new_text.readable,
            old_text.truncated || new_text.truncated,
        );
        (
            text_diff_model(
                path,
                state,
                &lens_id,
                old_text.text.as_deref().unwrap_or_default(),
                new_text.text.as_deref().unwrap_or_default(),
                old_file,
                new_file,
                old_text.truncated,
                new_text.truncated,
                &options,
            ),
            message,
        )
    } else if lens_id == "layrs.image" {
        (
            metadata_diff_model("imageMetadata", path, state, &lens_id, old_file, new_file),
            Some(
                "Image metadata diff is available; visual pixel diff is not available yet."
                    .to_string(),
            ),
        )
    } else {
        (
            metadata_diff_model("binary", path, state, &lens_id, old_file, new_file),
            Some(
                "Binary diff is represented as metadata because visual diff is not available."
                    .to_string(),
            ),
        )
    };

    Some(LensDiffEntry {
        path: path.to_string(),
        state: state.to_string(),
        lens_id,
        title: title.to_string(),
        diff,
        message,
    })
}

fn text_diff_model(
    path: &str,
    state: &str,
    lens_id: &str,
    old_text: &str,
    new_text: &str,
    old_file: Option<&FileSnapshotEntry>,
    new_file: Option<&FileSnapshotEntry>,
    old_truncated: bool,
    new_truncated: bool,
    options: &DiffWindowOptions<'_>,
) -> DiffModel {
    let old_lines = split_text_lines(old_text);
    let new_lines = split_text_lines(new_text);
    let mut all_lines = diff_text_lines(&old_lines, &new_lines);
    let old_hash = old_file.map(|file| file.hash.as_str());
    let new_hash = new_file.map(|file| file.hash.as_str());
    if line_stats(&all_lines).additions == 0
        && line_stats(&all_lines).deletions == 0
        && old_hash != new_hash
    {
        all_lines = synthetic_text_change_lines(old_file, new_file, old_truncated || new_truncated);
    }
    let stats = line_stats(&all_lines);
    let total_diff_lines = all_lines.len();
    let window_start = options.start.min(total_diff_lines);
    let window_limit = clamp_diff_window_limit(options.limit);
    let window_end = window_start
        .saturating_add(window_limit)
        .min(total_diff_lines);
    let lines = all_lines[window_start..window_end].to_vec();
    let has_more = window_end < total_diff_lines;
    let large_diff = old_truncated || new_truncated || has_more || total_diff_lines > window_limit;
    let mut fields = common_diff_fields(path, state, lens_id, old_file, new_file);
    fields.insert(
        "source".to_string(),
        Value::String(options.source.to_string()),
    );
    fields.insert(
        "layerId".to_string(),
        Value::String(options.layer_id.to_string()),
    );
    if let Some(step_id) = options.step_id {
        fields.insert("stepId".to_string(), Value::String(step_id.to_string()));
    }
    fields.insert("additions".to_string(), Value::from(stats.additions as u64));
    fields.insert("deletions".to_string(), Value::from(stats.deletions as u64));
    fields.insert("unchanged".to_string(), Value::from(stats.unchanged as u64));
    fields.insert(
        "totalOldLines".to_string(),
        Value::from(old_lines.len() as u64),
    );
    fields.insert(
        "totalNewLines".to_string(),
        Value::from(new_lines.len() as u64),
    );
    fields.insert(
        "totalDiffLines".to_string(),
        Value::from(total_diff_lines as u64),
    );
    fields.insert("windowStart".to_string(), Value::from(window_start as u64));
    fields.insert("windowLimit".to_string(), Value::from(window_limit as u64));
    fields.insert("windowEnd".to_string(), Value::from(window_end as u64));
    fields.insert("hasMore".to_string(), Value::Bool(has_more));
    fields.insert("largeDiff".to_string(), Value::Bool(large_diff));
    fields.insert("preview".to_string(), Value::Bool(options.preview));
    fields.insert(
        "windowUnit".to_string(),
        Value::String("diffLines".to_string()),
    );
    fields.insert("oldTruncated".to_string(), Value::Bool(old_truncated));
    fields.insert("newTruncated".to_string(), Value::Bool(new_truncated));
    if old_truncated || new_truncated {
        fields.insert("truncated".to_string(), Value::Bool(true));
    }

    DiffModel {
        kind: "textLines".to_string(),
        summary: summarize_line_diff(&stats, large_diff),
        hunks: vec![DiffHunk {
            old_start: first_old_line(&lines).unwrap_or(1),
            old_lines: lines.iter().filter(|line| line.old_line.is_some()).count(),
            new_start: first_new_line(&lines).unwrap_or(1),
            new_lines: lines.iter().filter(|line| line.new_line.is_some()).count(),
            lines,
        }],
        fields,
    }
}

fn metadata_diff_model(
    kind: &str,
    path: &str,
    state: &str,
    lens_id: &str,
    old_file: Option<&FileSnapshotEntry>,
    new_file: Option<&FileSnapshotEntry>,
) -> DiffModel {
    DiffModel {
        kind: kind.to_string(),
        summary: metadata_diff_summary(kind, state),
        hunks: Vec::new(),
        fields: common_diff_fields(path, state, lens_id, old_file, new_file),
    }
}

fn metadata_diff_summary(kind: &str, state: &str) -> String {
    let label = if kind == "imageMetadata" {
        "image metadata"
    } else {
        "binary metadata"
    };

    match state {
        "added" => format!("Added {label}"),
        "deleted" => format!("Deleted {label}"),
        _ => format!("Changed {label}"),
    }
}

fn common_diff_fields(
    path: &str,
    state: &str,
    lens_id: &str,
    old_file: Option<&FileSnapshotEntry>,
    new_file: Option<&FileSnapshotEntry>,
) -> BTreeMap<String, Value> {
    let mut fields = BTreeMap::new();
    fields.insert("path".to_string(), Value::String(path.to_string()));
    fields.insert("state".to_string(), Value::String(state.to_string()));
    fields.insert("lensId".to_string(), Value::String(lens_id.to_string()));
    fields.insert(
        "mediaType".to_string(),
        Value::String(media_type_for_path(path).to_string()),
    );
    if let Some(file) = old_file {
        fields.insert("oldHash".to_string(), Value::String(file.hash.clone()));
        fields.insert("oldSize".to_string(), Value::from(file.size));
    }
    if let Some(file) = new_file {
        fields.insert("newHash".to_string(), Value::String(file.hash.clone()));
        fields.insert("newSize".to_string(), Value::from(file.size));
    }
    fields
}

fn split_text_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }

    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(ToString::to_string)
        .collect()
}

fn clamp_diff_window_limit(limit: usize) -> usize {
    if limit == 0 {
        TEXT_DIFF_DEFAULT_WINDOW_LIMIT
    } else {
        limit.min(TEXT_DIFF_MAX_WINDOW_LIMIT)
    }
}

fn first_old_line(lines: &[DiffLine]) -> Option<usize> {
    lines.iter().find_map(|line| line.old_line)
}

fn first_new_line(lines: &[DiffLine]) -> Option<usize> {
    lines.iter().find_map(|line| line.new_line)
}

fn diff_text_lines(old_lines: &[String], new_lines: &[String]) -> Vec<DiffLine> {
    let mut prefix = 0usize;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }

    let mut suffix = 0usize;
    while suffix + prefix < old_lines.len()
        && suffix + prefix < new_lines.len()
        && old_lines[old_lines.len() - 1 - suffix] == new_lines[new_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let mut lines = Vec::with_capacity(old_lines.len().max(new_lines.len()));

    for index in 0..prefix {
        lines.push(DiffLine {
            op: "equal".to_string(),
            old_line: Some(index + 1),
            new_line: Some(index + 1),
            text: old_lines[index].clone(),
        });
    }

    for old_index in prefix..old_lines.len().saturating_sub(suffix) {
        lines.push(DiffLine {
            op: "delete".to_string(),
            old_line: Some(old_index + 1),
            new_line: None,
            text: old_lines[old_index].clone(),
        });
    }

    for new_index in prefix..new_lines.len().saturating_sub(suffix) {
        lines.push(DiffLine {
            op: "insert".to_string(),
            old_line: None,
            new_line: Some(new_index + 1),
            text: new_lines[new_index].clone(),
        });
    }

    for offset in 0..suffix {
        let old_index = old_lines.len() - suffix + offset;
        let new_index = new_lines.len() - suffix + offset;
        lines.push(DiffLine {
            op: "equal".to_string(),
            old_line: Some(old_index + 1),
            new_line: Some(new_index + 1),
            text: old_lines[old_index].clone(),
        });
    }

    lines
}

fn synthetic_text_change_lines(
    old_file: Option<&FileSnapshotEntry>,
    new_file: Option<&FileSnapshotEntry>,
    truncated: bool,
) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    if let Some(file) = old_file {
        lines.push(DiffLine {
            op: "delete".to_string(),
            old_line: Some(1),
            new_line: None,
            text: synthetic_text_change_label("Old", file, truncated),
        });
    }
    if let Some(file) = new_file {
        lines.push(DiffLine {
            op: "insert".to_string(),
            old_line: None,
            new_line: Some(1),
            text: synthetic_text_change_label("New", file, truncated),
        });
    }
    lines
}

fn synthetic_text_change_label(side: &str, file: &FileSnapshotEntry, truncated: bool) -> String {
    let detail = if truncated {
        "content changed outside available text content"
    } else {
        "content changed but text preview is unavailable"
    };
    format!(
        "{side} {detail}; size: {}; object: {}",
        format_bytes(file.size),
        file.hash
    )
}

fn line_stats(lines: &[DiffLine]) -> TextLineStats {
    let mut stats = TextLineStats::default();
    for line in lines {
        match line.op.as_str() {
            "insert" => stats.additions += 1,
            "delete" => stats.deletions += 1,
            _ => stats.unchanged += 1,
        }
    }
    stats
}

fn diff_stats(diffs: &[LensDiffEntry]) -> DiffStats {
    let mut stats = DiffStats {
        files: diffs.len(),
        ..DiffStats::default()
    };

    for diff in diffs {
        for hunk in &diff.diff.hunks {
            for line in &hunk.lines {
                match line.op.as_str() {
                    "insert" => stats.additions += 1,
                    "delete" => stats.deletions += 1,
                    _ => {}
                }
            }
        }
    }

    stats
}

#[derive(Clone, Debug, Default)]
struct TextLineStats {
    additions: usize,
    deletions: usize,
    unchanged: usize,
}

fn summarize_line_diff(stats: &TextLineStats, truncated: bool) -> String {
    if stats.additions == 0 && stats.deletions == 0 {
        "No text changes".to_string()
    } else if truncated {
        format!(
            "Large text changed: {} additions, {} deletions",
            stats.additions, stats.deletions
        )
    } else {
        format!(
            "{} additions, {} deletions",
            stats.additions, stats.deletions
        )
    }
}

fn file_entries(files: &[FileSnapshotEntry]) -> BTreeMap<String, FileSnapshotEntry> {
    files
        .iter()
        .map(|file| (file.path.clone(), file.clone()))
        .collect()
}

fn read_text_object_for_diff(
    handle: &LocalSpaceHandle,
    file: &FileSnapshotEntry,
) -> TextDiffSource {
    if !(is_text_path(&file.path) || is_code_path(&file.path)) {
        return TextDiffSource {
            text: None,
            readable: false,
            truncated: false,
        };
    }

    match read_snapshot_object_bytes(&handle.layrs_dir, file)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
    {
        Some(text) => TextDiffSource {
            text: Some(text),
            readable: true,
            truncated: false,
        },
        None => TextDiffSource {
            text: None,
            readable: false,
            truncated: false,
        },
    }
}

fn text_diff_message(
    old_file: Option<&FileSnapshotEntry>,
    new_file: Option<&FileSnapshotEntry>,
    old_readable: bool,
    new_readable: bool,
    truncated: bool,
) -> Option<String> {
    if !old_readable || !new_readable {
        return Some(
            "Text diff preview is unavailable because this file could not be read as UTF-8."
                .to_string(),
        );
    }

    if truncated {
        let old_size = old_file.map(|file| format_bytes(file.size));
        let new_size = new_file.map(|file| format_bytes(file.size));
        return Some(format!(
            "Large text diff is available in line windows. Old size: {}; New size: {}.",
            old_size.as_deref().unwrap_or("none"),
            new_size.as_deref().unwrap_or("none")
        ));
    }

    None
}

fn format_bytes(size: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    if size >= 1024 * 1024 {
        format!("{:.1} MiB", size as f64 / MIB)
    } else if size >= 1024 {
        format!("{:.1} KiB", size as f64 / KIB)
    } else {
        format!("{size} B")
    }
}

fn file_hashes(files: &[FileSnapshotEntry]) -> BTreeMap<String, String> {
    files
        .iter()
        .map(|file| (file.path.clone(), file.hash.clone()))
        .collect()
}
