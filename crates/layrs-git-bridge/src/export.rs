use super::*;
use crate::Result;
use base64::{Engine, engine::general_purpose::STANDARD};
use layrs_client_core::native_scm::NativeWorkspace;
use layrs_graph::scm::StepDag;
use layrs_store::scm::*;
use layrs_store_local::scm_v1::{NativeScmReader, open};
use std::collections::BTreeSet;

#[path = "export_selection.rs"]
mod selection;

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ExportMode {
    /// All visible Layers (every head) and Markers, preserving parent closure.
    History,
    /// Exact exported branch/tag names, preserving every selected head's parents.
    /// Multi-head Layers use their explicit generated /head-<Step hash> names.
    SelectedHistory { references: Vec<String> },
    /// One clean snapshot, explicitly without its native parent history.
    Snapshot { step_id: StepId, reference: String },
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    pub commits: BTreeMap<StepId, String>,
    pub references: BTreeMap<String, String>,
    pub warnings: Vec<String>,
}
struct Reference {
    name: String,
    step: StepId,
    annotation: Option<String>,
}

pub fn export_workspace(
    workspace: &NativeWorkspace,
    destination: impl AsRef<Path>,
    mode: ExportMode,
    limits: GitBridgeLimits,
) -> Result<ExportReport> {
    export_workspace_with_progress(workspace, destination, mode, limits, &mut |_| true)
}
pub fn export_workspace_with_progress(
    workspace: &NativeWorkspace,
    destination: impl AsRef<Path>,
    mode: ExportMode,
    limits: GitBridgeLimits,
    callback: &mut ProgressCallback<'_>,
) -> Result<ExportReport> {
    export_impl(workspace, destination, mode, limits, callback, false)
}
/// Explicitly resumes only an incomplete export for the exact same native
/// Operation/View/mode. Existing unexpected refs or reparse entries refuse.
pub fn resume_export_workspace(
    workspace: &NativeWorkspace,
    destination: impl AsRef<Path>,
    mode: ExportMode,
    limits: GitBridgeLimits,
    callback: &mut ProgressCallback<'_>,
) -> Result<ExportReport> {
    export_impl(workspace, destination, mode, limits, callback, true)
}
fn export_impl(
    workspace: &NativeWorkspace,
    destination: impl AsRef<Path>,
    mode: ExportMode,
    limits: GitBridgeLimits,
    callback: &mut ProgressCallback<'_>,
    resume: bool,
) -> Result<ExportReport> {
    let started = std::time::Instant::now();
    progress(callback, started, BridgePhase::ExportPlan, 0, None, 0, 0, 0)?;
    let mode = selection::normalize(mode, limits)?;
    let destination = std::path::absolute(destination)?;
    if destination.try_exists()? && !resume {
        return Err(invalid("Git export destination must not exist"));
    }
    let state = workspace.state().map_err(invalid)?;
    let checkpoint = serde_json::json!({"schemaVersion":"layrs.git-export-checkpoint.v1","spaceId":state.space.id(),"operationId":state.operation.id(),"viewId":state.view.id(),"mode":&mode});
    let checkpoint_bytes = canonical_json(&checkpoint).map_err(invalid)?;
    if checkpoint_bytes.len() > 16_384 {
        return Err(BridgeError::Limit("export checkpoint bytes"));
    }
    if resume {
        validate_resume_directory(&destination, limits)?;
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(destination.join("layrs-export.incomplete"))?
            .take(16385)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 16384 || bytes != checkpoint_bytes {
            return Err(invalid(
                "export checkpoint does not match exact native snapshot/mode",
            ));
        }
    }
    let store = open(workspace.store_root()).map_err(invalid)?;
    let reader = NativeScmReader::new(&store).map_err(invalid)?;
    let mut provenance: BTreeMap<StepId, Vec<GitCommitMapping>> = BTreeMap::new();
    let mut reports = BTreeMap::new();
    let mut references = vec![];
    let snapshot = matches!(mode, ExportMode::Snapshot { .. });
    let selected = match &mode {
        ExportMode::SelectedHistory { references } => {
            Some(references.iter().cloned().collect::<BTreeSet<_>>())
        }
        _ => None,
    };
    match mode {
        ExportMode::Snapshot { step_id, reference } => references.push(Reference {
            name: reference,
            step: step_id,
            annotation: None,
        }),
        ExportMode::History | ExportMode::SelectedHistory { .. } => {
            for layer in &state.layers {
                let base = metadata_value(layer.metadata(), "git.ref")
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("refs/heads/layrs/{}", escape_ref(layer.name())));
                let heads = state
                    .view
                    .layer_heads()
                    .iter()
                    .find(|h| h.layer_id() == layer.id())
                    .ok_or_else(|| invalid("Layer heads missing"))?;
                for step in heads.step_ids() {
                    let name = if heads.step_ids().len() == 1 {
                        base.clone()
                    } else {
                        format!("{base}/head-{}", &step.as_str()[7..])
                    };
                    if selected
                        .as_ref()
                        .is_some_and(|names| !names.contains(&name))
                    {
                        continue;
                    }
                    if references.len() >= limits.max_objects {
                        return Err(BridgeError::Limit("export references"));
                    }
                    load_provenance(&reader, state.space.id(), layer.metadata(), &mut reports)?;
                    references.push(Reference {
                        name,
                        step: step.clone(),
                        annotation: None,
                    });
                }
            }
            for id in state.view.marker_ids() {
                let marker = reader
                    .get_marker(id)
                    .map_err(invalid)?
                    .ok_or_else(|| invalid("Marker missing"))?;
                let name = metadata_value(marker.metadata(), "git.ref")
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("refs/tags/layrs/{}", escape_ref(marker.name())));
                if selected
                    .as_ref()
                    .is_some_and(|names| !names.contains(&name))
                {
                    continue;
                }
                if references.len() >= limits.max_objects {
                    return Err(BridgeError::Limit("export references"));
                }
                load_provenance(&reader, state.space.id(), marker.metadata(), &mut reports)?;
                let annotation = metadata_value(marker.metadata(), "git.importId")
                    .and_then(|id| reports.get(id))
                    .and_then(|report| report.references.iter().find(|r| r.git_ref == name))
                    .and_then(|r| r.annotated_tag_base64.clone());
                references.push(Reference {
                    name,
                    step: marker.target_step_id().clone(),
                    annotation,
                });
            }
        }
    }
    if selected.as_ref().is_some_and(|names| {
        names
            .iter()
            .any(|name| !references.iter().any(|reference| &reference.name == name))
    }) {
        return Err(invalid(
            "selected export reference missing or ambiguous Layer base; choose exact exported head names",
        ));
    }
    for report in reports.values() {
        for commit in &report.commits {
            provenance
                .entry(commit.step_id.clone())
                .or_default()
                .push(commit.clone());
        }
    }
    if references.is_empty() {
        return Err(invalid("no native references to export"));
    }
    references.sort_by(|a, b| a.name.cmp(&b.name));
    for pair in references.windows(2) {
        if pair[0].name == pair[1].name || pair[1].name.starts_with(&(pair[0].name.clone() + "/")) {
            return Err(invalid(
                "Git reference namespace collision; select an unambiguous snapshot",
            ));
        }
    }
    // Git's loose refs use filesystem paths. Windows cannot faithfully publish
    // case aliases or a file/directory alias; reject before creating the export.
    // Tree object names themselves remain canonical and may retain both cases.
    #[cfg(windows)]
    {
        let folded = references
            .iter()
            .map(|reference| reference.name.to_uppercase())
            .collect::<BTreeSet<_>>();
        if folded.len() != references.len()
            || folded.iter().any(|name| {
                name.match_indices('/')
                    .any(|(index, _)| folded.contains(&name[..index]))
            })
        {
            return Err(invalid(
                "Git reference case collision on Windows; select unambiguous refs",
            ));
        }
    }
    let mut loader = Loader {
        reader: &reader,
        space: state.space.id(),
        limits,
        steps: BTreeMap::new(),
        trees: BTreeMap::new(),
        files: BTreeMap::new(),
        links: BTreeMap::new(),
        bytes: 0,
        paths: 0,
        conflicted: BTreeSet::new(),
        admitted_objects: BTreeSet::new(),
        tombstones: BTreeSet::new(),
    };
    let mut pending = references
        .iter()
        .map(|r| r.step.clone())
        .collect::<Vec<_>>();
    while let Some(id) = pending.pop() {
        if loader.steps.contains_key(&id) {
            continue;
        }
        if loader.steps.len() >= limits.max_commits {
            return Err(BridgeError::Limit("export Steps"));
        }
        loader.admitted(StoreObjectId::Step(id.clone()))?;
        let step = reader
            .get_step(&id)
            .map_err(invalid)?
            .ok_or_else(|| invalid("Step missing"))?;
        loader.charge(step.canonical_payload().len())?;
        if !snapshot {
            pending.extend_from_slice(step.parent_step_ids());
        }
        if loader.tree(step.root_tree_id(), 0)? {
            loader.conflicted.insert(id.clone());
        }
        loader.steps.insert(id, step);
        progress(
            callback,
            started,
            BridgePhase::ExportPlan,
            loader.steps.len(),
            None,
            loader.admitted_objects.len(),
            loader.bytes,
            0,
        )?;
    }
    if !loader.conflicted.is_empty() {
        return Err(BridgeError::Unsupported(format!(
            "history contains conflicted Steps: {:?}; select a clean snapshot explicitly",
            loader.conflicted
        )));
    }
    let order = if snapshot {
        loader.steps.keys().cloned().collect()
    } else {
        StepDag::from_steps(loader.steps.values())
            .map_err(invalid)?
            .topological()
            .map_err(invalid)?
    };
    // No source repository is mutated. A failed new export keeps its explicit
    // incomplete marker so it cannot be mistaken for a validated destination.
    if !resume {
        std::fs::create_dir(&destination)?;
    }
    let incomplete = destination.join("layrs-export.incomplete");
    if !resume {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&incomplete)?;
        file.write_all(&checkpoint_bytes)?;
        file.sync_all()?;
    }
    let git = process::Git::new(&destination, limits)?;
    git.run(
        &["init", "--bare", "--template=", "--object-format=sha1", "."],
        None,
        4096,
    )?;
    let mut existing = BTreeMap::new();
    let output = git.run(
        &["for-each-ref", "--format=%(refname) %(objectname)"],
        None,
        limits.max_total_bytes,
    )?;
    for line in utf8(&output)?.lines() {
        let (name, id) = line
            .split_once(' ')
            .ok_or_else(|| invalid("export ref framing"))?;
        if !references.iter().any(|r| r.name == name) {
            return Err(invalid(
                "incomplete export contains an unexpected reference",
            ));
        }
        existing.insert(name.to_owned(), hex_id(id)?.to_owned());
    }
    for reference in &references {
        if !reference.name.starts_with("refs/heads/") && !reference.name.starts_with("refs/tags/") {
            return Err(invalid("export reference must be a branch or tag"));
        }
        git.run(&["check-ref-format", &reference.name], None, 256)?;
    }
    let mut writer = Writer {
        pack: pack::Pack::new(limits)?,
        objects: &loader,
        trees: BTreeMap::new(),
        blobs: BTreeMap::new(),
    };
    let mut result = ExportReport {
        commits: BTreeMap::new(),
        references: BTreeMap::new(),
        warnings: vec![],
    };
    if selected.is_some() {
        result.warnings.push("Explicit reference selection; every selected head includes its complete ancestor closure, other references are omitted".into());
    }
    if !loader.tombstones.is_empty() {
        result.warnings.push(format!(
            "{} native Tombstones exported as absent paths; deletion provenance remains native",
            loader.tombstones.len()
        ));
    }
    let total = order.len();
    progress(
        callback,
        started,
        BridgePhase::ExportCommits,
        0,
        Some(total),
        loader.admitted_objects.len(),
        loader.bytes,
        git.processes(),
    )?;
    for (index, id) in order.into_iter().enumerate() {
        let step = &loader.steps[&id];
        let tree = writer.tree(step.root_tree_id())?;
        let metadata = provenance
            .get(&id)
            .filter(|all| all.len() == 1)
            .map(|all| &all[0]);
        let mut parents = if snapshot {
            vec![]
        } else {
            step.parent_step_ids().to_vec()
        };
        if let Some(meta) = metadata {
            let source_report = reports
                .values()
                .find(|report| report.commits.iter().any(|c| c.git_id == meta.git_id));
            if let Some(report) = source_report {
                let original = meta
                    .parent_git_ids
                    .iter()
                    .filter_map(|git| {
                        report
                            .commits
                            .iter()
                            .find(|c| &c.git_id == git)
                            .map(|c| c.step_id.clone())
                    })
                    .collect::<Vec<_>>();
                if !snapshot
                    && original.iter().cloned().collect::<BTreeSet<_>>()
                        == parents.iter().cloned().collect()
                    && original.len() == parents.len()
                {
                    parents = original;
                }
            }
        }
        let mut raw =
            pack::ObjectBuffer::new(limits.max_object_bytes, format!("tree {tree}\n").as_bytes())?;
        for parent in parents {
            raw.extend_from_slice(
                format!(
                    "parent {}\n",
                    result
                        .commits
                        .get(&parent)
                        .ok_or_else(|| invalid("export parent missing"))?
                )
                .as_bytes(),
            )?;
        }
        let message = if let Some(meta) = metadata {
            let original = decode_provenance(&meta.raw_commit_base64, limits)?;
            let headers = import::parse_headers(&original)?;
            import::header(&headers, "author")?;
            import::header(&headers, "committer")?;
            for (key, value) in headers {
                if ["tree", "parent"].contains(&key.as_str()) {
                    continue;
                }
                if ["gpgsig", "gpgsig-sha256", "mergetag"].contains(&key.as_str()) {
                    result.warnings.push(format!("Signature header {key} removed from rebuilt Step {id}; raw provenance remains native"));
                    continue;
                }
                raw.extend_from_slice(key.as_bytes())?;
                raw.push(b' ')?;
                raw.extend_from_slice(&value)?;
                raw.push(b'\n')?;
            }
            let split = original.windows(2).position(|w| w == b"\n\n").unwrap();
            original[split + 2..].to_vec()
        } else {
            raw.extend_from_slice(b"author Layrs Export <layrs-export@localhost> 0 +0000\ncommitter Layrs Export <layrs-export@localhost> 0 +0000\n")?;
            result.warnings.push(format!(
                "Synthetic attribution for Step {id}: no unique Git provenance"
            ));
            format!("Layrs Step {id}\n").into_bytes()
        };
        raw.push(b'\n')?;
        raw.extend_from_slice(&message)?;
        let oid = writer.pack.insert("commit", &raw)?;
        result.commits.insert(id, oid);
        progress(
            callback,
            started,
            BridgePhase::ExportCommits,
            index + 1,
            Some(total),
            loader.admitted_objects.len(),
            loader.bytes,
            git.processes(),
        )?;
    }
    let total = references.len();
    for (index, reference) in references.into_iter().enumerate() {
        let mut oid = result.commits[&reference.step].clone();
        if let Some(annotation) = reference.annotation {
            let original = decode_provenance(&annotation, limits)?;
            let headers = import::parse_headers(&original)?;
            let mut raw = pack::ObjectBuffer::new(
                limits.max_object_bytes,
                format!("object {oid}\n").as_bytes(),
            )?;
            for (key, value) in headers {
                if key != "object" {
                    raw.extend_from_slice(key.as_bytes())?;
                    raw.push(b' ')?;
                    raw.extend_from_slice(&value)?;
                    raw.push(b'\n')?;
                }
            }
            raw.push(b'\n')?;
            let start = original.windows(2).position(|w| w == b"\n\n").unwrap() + 2;
            let mut message = &original[start..];
            if let Some(signature) = signature_start(message) {
                message = &message[..signature];
                result.warnings.push(format!(
                    "Tag signature removed from {}; original retained natively",
                    reference.name
                ));
            }
            raw.extend_from_slice(message)?;
            oid = writer.pack.insert("tag", &raw)?;
        }
        if existing.get(&reference.name).is_some_and(|old| old != &oid) {
            return Err(invalid(
                "incomplete export reference differs from expected immutable result",
            ));
        }
        result.references.insert(reference.name, oid);
        progress(
            callback,
            started,
            BridgePhase::ExportReferences,
            index + 1,
            Some(total),
            loader.admitted_objects.len(),
            loader.bytes,
            git.processes(),
        )?;
    }
    // Git validates and installs one complete self-contained pack. No object
    // database files are written directly by this bridge, including on resume.
    let packed = writer.pack.finish()?;
    let checksum = pack::hex_bytes(&packed[packed.len() - 20..]);
    let indexed = git.run_owned(&["index-pack", "--stdin", "--strict"], Some(packed), 128)?;
    if utf8(&indexed)?.trim() != format!("pack\t{checksum}") {
        return Err(invalid(
            "Git index-pack checksum differs from prepared pack",
        ));
    }
    let mut updates = String::from("start\n");
    for (name, id) in &result.references {
        updates.push_str(&format!(
            "update {name} {id} {}\n",
            existing
                .get(name)
                .map(String::as_str)
                .unwrap_or("0000000000000000000000000000000000000000")
        ));
    }
    updates.push_str("prepare\ncommit\n");
    git.run(&["update-ref", "--stdin"], Some(updates.as_bytes()), 4096)?;
    if let Some(branch) = result
        .references
        .keys()
        .find(|name| name.starts_with("refs/heads/"))
    {
        git.run(&["symbolic-ref", "HEAD", branch], None, 256)?;
    }
    git.run(
        &["fsck", "--strict", "--full"],
        None,
        limits.max_total_bytes,
    )?;
    std::fs::write(
        destination.join("layrs-export-report.json"),
        canonical_json(&result).map_err(invalid)?,
    )?;
    std::fs::remove_file(incomplete)?;
    // Completion is a notification, not a cancellable pre-commit boundary.
    let _ = progress(
        callback,
        started,
        BridgePhase::ExportComplete,
        result.commits.len(),
        Some(result.commits.len()),
        loader.admitted_objects.len(),
        loader.bytes,
        git.processes(),
    );
    Ok(result)
}

fn decode_provenance(encoded: &str, limits: GitBridgeLimits) -> Result<Vec<u8>> {
    let max_encoded = limits
        .max_object_bytes
        .checked_add(2)
        .and_then(|n| n.checked_div(3))
        .and_then(|n| n.checked_mul(4))
        .ok_or(BridgeError::Limit("Git provenance bytes"))?;
    if encoded.len() > max_encoded {
        return Err(BridgeError::Limit("Git provenance bytes"));
    }
    let bytes = STANDARD.decode(encoded).map_err(invalid)?;
    if bytes.len() > limits.max_object_bytes {
        return Err(BridgeError::Limit("Git provenance bytes"));
    }
    Ok(bytes)
}

fn validate_resume_directory(root: &Path, limits: GitBridgeLimits) -> Result<()> {
    let mut pending = vec![root.to_owned()];
    let mut visited = 0usize;
    while let Some(path) = pending.pop() {
        visited += 1;
        if visited > limits.max_objects.saturating_mul(2).saturating_add(1024) {
            return Err(BridgeError::Limit("export resume entries"));
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(invalid("export resume rejects symbolic links"));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(invalid("export resume rejects reparse entries"));
            }
        }
        if metadata.is_dir() {
            for entry in std::fs::read_dir(&path)? {
                let entry = entry?;
                if path == root
                    && ![
                        "HEAD",
                        "config",
                        "description",
                        "hooks",
                        "info",
                        "objects",
                        "refs",
                        "packed-refs",
                        "layrs-export.incomplete",
                        "layrs-export-report.json",
                    ]
                    .contains(
                        &entry
                            .file_name()
                            .to_str()
                            .ok_or_else(|| invalid("export entry name"))?,
                    )
                {
                    return Err(invalid("unexpected file in incomplete export"));
                }
                pending.push(entry.path());
            }
        } else if !metadata.is_file() {
            return Err(invalid("export resume rejects special entries"));
        }
    }
    Ok(())
}

fn escape_ref(name: &str) -> String {
    name.as_bytes()
        .iter()
        .map(|b| {
            if b.is_ascii_alphanumeric() || [b'-', b'_'].contains(b) {
                (*b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn load_provenance(
    reader: &dyn ScmReader,
    space: &SpaceId,
    metadata: &Metadata,
    reports: &mut BTreeMap<String, ImportReport>,
) -> Result<()> {
    if let Some(id) = metadata_value(metadata, "git.importId") {
        if !reports.contains_key(id) {
            let operation = reader
                .get_operation(&install::import_operation_id(id, space)?)
                .map_err(invalid)?
                .ok_or_else(|| invalid("import provenance Operation missing"))?;
            reports.insert(id.to_owned(), install::read_report(operation.metadata())?);
        }
    }
    Ok(())
}

struct Loader<'a> {
    reader: &'a dyn ScmReader,
    space: &'a SpaceId,
    limits: GitBridgeLimits,
    steps: BTreeMap<StepId, Step>,
    trees: BTreeMap<TreeId, (Tree, bool)>,
    files: BTreeMap<FileId, Vec<u8>>,
    links: BTreeMap<SymlinkId, Symlink>,
    bytes: usize,
    paths: usize,
    conflicted: BTreeSet<StepId>,
    admitted_objects: BTreeSet<String>,
    tombstones: BTreeSet<TombstoneId>,
}
impl Loader<'_> {
    fn admitted(&mut self, id: StoreObjectId) -> Result<()> {
        self.admitted_objects.insert(id.as_str().to_owned());
        if self.admitted_objects.len() > self.limits.max_objects {
            return Err(BridgeError::Limit("export objects"));
        }
        if !self
            .reader
            .object_admitted_to_space(&id, self.space)
            .map_err(invalid)?
        {
            return Err(invalid("export object outside selected Space"));
        }
        Ok(())
    }
    fn charge(&mut self, n: usize) -> Result<()> {
        if n > self.limits.max_object_bytes {
            return Err(BridgeError::Limit("export object bytes"));
        }
        self.bytes = self
            .bytes
            .checked_add(n)
            .ok_or(BridgeError::Limit("export bytes"))?;
        if self.bytes > self.limits.max_total_bytes {
            return Err(BridgeError::Limit("export bytes"));
        }
        Ok(())
    }
    fn tree(&mut self, id: &TreeId, depth: usize) -> Result<bool> {
        if depth > self.limits.max_depth {
            return Err(BridgeError::Limit("export depth"));
        }
        if let Some((_, conflicted)) = self.trees.get(id) {
            return Ok(*conflicted);
        }
        self.admitted(StoreObjectId::Tree(id.clone()))?;
        let tree = self
            .reader
            .get_tree(id)
            .map_err(invalid)?
            .ok_or_else(|| invalid("export Tree missing"))?;
        self.charge(tree.canonical_payload().map_err(invalid)?.len())?;
        let mut conflicted = false;
        for entry in tree.entries() {
            self.paths += 1;
            if self.paths > self.limits.max_paths {
                return Err(BridgeError::Limit("export paths"));
            }
            match entry.target() {
                TreeEntryTarget::File { file_id } => self.file(file_id)?,
                TreeEntryTarget::Tree { tree_id } => conflicted |= self.tree(tree_id, depth + 1)?,
                TreeEntryTarget::Symlink { symlink_id } => {
                    self.admitted(StoreObjectId::Symlink(symlink_id.clone()))?;
                    let link = self
                        .reader
                        .get_symlink(symlink_id)
                        .map_err(invalid)?
                        .ok_or_else(|| invalid("export Symlink missing"))?;
                    self.charge(canonical_json(&link).map_err(invalid)?.len())?;
                    self.links.insert(symlink_id.clone(), link);
                }
                TreeEntryTarget::Conflict { .. } => conflicted = true,
                TreeEntryTarget::Tombstone { tombstone_id } => {
                    self.admitted(StoreObjectId::Tombstone(tombstone_id.clone()))?;
                    let value = self
                        .reader
                        .get_tombstone(tombstone_id)
                        .map_err(invalid)?
                        .ok_or_else(|| invalid("export Tombstone missing"))?;
                    self.charge(canonical_json(&value).map_err(invalid)?.len())?;
                    self.tombstones.insert(tombstone_id.clone());
                }
            }
        }
        self.trees.insert(id.clone(), (tree, conflicted));
        Ok(conflicted)
    }
    fn file(&mut self, id: &FileId) -> Result<()> {
        if self.files.contains_key(id) {
            return Ok(());
        }
        let FileAvailability::Present(file) = self.reader.file_availability(id).map_err(invalid)?
        else {
            return Err(invalid("export File missing"));
        };
        let mut selected = None;
        for id in file.verified_manifest_ids() {
            if self
                .reader
                .object_admitted_to_space(&StoreObjectId::FileManifest(id.clone()), self.space)
                .map_err(invalid)?
            {
                selected = self.reader.get_file_manifest(id).map_err(invalid)?;
                if selected.is_some() {
                    break;
                }
            }
        }
        let manifest = selected.ok_or_else(|| invalid("export manifest missing"))?;
        self.admitted(StoreObjectId::FileManifest(manifest.id().clone()))?;
        self.charge(manifest.canonical_payload().map_err(invalid)?.len())?;
        let size = usize::try_from(manifest.byte_length()).map_err(invalid)?;
        if size > self.limits.max_object_bytes {
            return Err(BridgeError::Limit("export file bytes"));
        }
        self.charge(size)?;
        let mut chunks = BTreeMap::new();
        for chunk in manifest.chunks() {
            self.admitted(StoreObjectId::ChunkRaw(chunk.chunk_id().clone()))?;
            chunks.insert(
                chunk.chunk_id().clone(),
                self.reader
                    .get_chunk(chunk.chunk_id())
                    .map_err(invalid)?
                    .ok_or_else(|| invalid("export Chunk missing"))?,
            );
        }
        let bytes = manifest
            .reconstruct(|id| chunks.get(id).map(|chunk| chunk.bytes().to_vec()))
            .map_err(invalid)?;
        self.files.insert(id.clone(), bytes);
        Ok(())
    }
}
struct Writer<'a, 'b> {
    pack: pack::Pack,
    objects: &'a Loader<'b>,
    trees: BTreeMap<TreeId, String>,
    blobs: BTreeMap<String, String>,
}
impl Writer<'_, '_> {
    fn tree(&mut self, id: &TreeId) -> Result<String> {
        if let Some(oid) = self.trees.get(id) {
            return Ok(oid.clone());
        }
        let tree = &self.objects.trees[id].0;
        let mut entries = tree.entries().iter().collect::<Vec<_>>();
        entries.sort_by(|a, b| {
            a.name()
                .as_str()
                .bytes()
                .chain(a.target().tree_id().map(|_| b'/'))
                .cmp(
                    b.name()
                        .as_str()
                        .bytes()
                        .chain(b.target().tree_id().map(|_| b'/')),
                )
        });
        let length = entries
            .iter()
            .filter(|e| !matches!(e.target(), TreeEntryTarget::Tombstone { .. }))
            .try_fold(0usize, |sum, e| {
                sum.checked_add(e.name().as_str().len())
                    .and_then(|n| n.checked_add(28))
            })
            .ok_or(BridgeError::Limit("Git tree bytes"))?;
        if length > self.objects.limits.max_object_bytes {
            return Err(BridgeError::Limit("Git tree bytes"));
        }
        let mut input = Vec::with_capacity(length);
        for entry in entries {
            let (mode, oid) = match entry.target() {
                TreeEntryTarget::Tree { tree_id } => ("40000", self.tree(tree_id)?),
                TreeEntryTarget::File { file_id } => (
                    if entry.executable() {
                        "100755"
                    } else {
                        "100644"
                    },
                    self.blob(file_id.as_str(), &self.objects.files[file_id])?,
                ),
                TreeEntryTarget::Symlink { symlink_id } => (
                    "120000",
                    self.blob(
                        symlink_id.as_str(),
                        self.objects.links[symlink_id].target().as_bytes(),
                    )?,
                ),
                TreeEntryTarget::Tombstone { .. } => continue,
                TreeEntryTarget::Conflict { .. } => {
                    return Err(invalid("conflict slipped through export validation"));
                }
            };
            input.extend_from_slice(mode.as_bytes());
            input.push(b' ');
            input.extend_from_slice(entry.name().as_str().as_bytes());
            input.push(0);
            input.extend_from_slice(&pack::oid_bytes(&oid)?);
        }
        let oid = self.pack.insert("tree", &input)?;
        self.trees.insert(id.clone(), oid.clone());
        Ok(oid)
    }
    fn blob(&mut self, key: &str, bytes: &[u8]) -> Result<String> {
        if let Some(id) = self.blobs.get(key) {
            return Ok(id.clone());
        }
        let id = self.pack.insert("blob", bytes)?;
        self.blobs.insert(key.to_owned(), id.clone());
        Ok(id)
    }
}
