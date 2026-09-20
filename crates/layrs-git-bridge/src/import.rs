use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::collections::{BTreeSet, HashSet};

pub fn scan_git(source: impl AsRef<Path>, limits: GitBridgeLimits) -> Result<ImportPlan> {
    scan_git_with_progress(source, limits, &mut |_| true)
}
pub fn scan_git_with_progress(
    source: impl AsRef<Path>,
    limits: GitBridgeLimits,
    callback: &mut ProgressCallback<'_>,
) -> Result<ImportPlan> {
    scan_git_selected_with_progress(
        source,
        &ImportSelection::AllLocalReferences,
        limits,
        callback,
    )
}

pub fn scan_git_selected(
    source: impl AsRef<Path>,
    selection: &ImportSelection,
    limits: GitBridgeLimits,
) -> Result<ImportPlan> {
    scan_git_selected_with_progress(source, selection, limits, &mut |_| true)
}

/// Select exact branch/tag names while retaining every ancestor of their tips.
/// Neither revision expressions nor path filters can silently truncate history.
pub fn scan_git_selected_with_progress(
    source: impl AsRef<Path>,
    selection: &ImportSelection,
    limits: GitBridgeLimits,
    callback: &mut ProgressCallback<'_>,
) -> Result<ImportPlan> {
    let acquired = acquire(source.as_ref(), selection, limits, callback)?;
    convert(acquired, limits, callback)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrozenMetadata {
    pub(crate) format: String,
    pub(crate) references: Vec<(String, String, Option<String>)>,
    pub(crate) commits: Vec<String>,
    pub(crate) tag_bytes: usize,
    pub(crate) warnings: Vec<String>,
}
pub(crate) struct AcquiredScan {
    pub(crate) metadata: FrozenMetadata,
    pub(crate) objects: BTreeMap<String, (String, Vec<u8>)>,
    pub(crate) processes: usize,
    pub(crate) started: std::time::Instant,
}

pub(crate) fn acquire(
    source: &Path,
    selection: &ImportSelection,
    limits: GitBridgeLimits,
    callback: &mut ProgressCallback<'_>,
) -> Result<AcquiredScan> {
    let requested = match selection {
        ImportSelection::AllLocalReferences => None,
        ImportSelection::References(names) => {
            if names.is_empty() {
                return Err(invalid("empty Git reference selection"));
            }
            if names.len() > limits.max_objects {
                return Err(BridgeError::Limit("Git selected references"));
            }
            let mut total = 0usize;
            let mut unique = BTreeSet::new();
            for name in names {
                total = total
                    .checked_add(name.len())
                    .ok_or(BridgeError::Limit("Git selection bytes"))?;
                if total > limits.max_total_bytes {
                    return Err(BridgeError::Limit("Git selection bytes"));
                }
                if (!name.starts_with("refs/heads/") && !name.starts_with("refs/tags/"))
                    || name.bytes().any(|b| b < 32 || b == 127)
                    || !unique.insert(name.as_str())
                {
                    return Err(invalid("select unique full local branch or tag names"));
                }
            }
            Some(unique)
        }
    };
    let started = std::time::Instant::now();
    progress(
        callback,
        started,
        BridgePhase::ScanReferences,
        0,
        None,
        0,
        0,
        0,
    )?;
    let git = process::Git::new(source, limits)?;
    if utf8(&git.run(&["rev-parse", "--is-shallow-repository"], None, 16)?)?.trim() != "false" {
        return Err(BridgeError::Unsupported("shallow Git repository".into()));
    }
    let config = git.run(
        &["config", "--local", "--includes", "--null", "--list"],
        None,
        limits.max_object_bytes,
    )?;
    for record in config.split(|b| *b == 0) {
        let text = String::from_utf8_lossy(record).to_ascii_lowercase();
        if text.starts_with("extensions.partialclone\n")
            || (text
                .split('\n')
                .next()
                .is_some_and(|key| key.ends_with(".promisor"))
                && !text.ends_with("\nfalse"))
        {
            return Err(BridgeError::Unsupported(
                "partial/promisor Git repository; fetch it explicitly before import".into(),
            ));
        }
    }
    let format = utf8(&git.run(&["rev-parse", "--show-object-format"], None, 16)?)?
        .trim()
        .to_owned();
    if !["sha1", "sha256"].contains(&format.as_str()) {
        return Err(BridgeError::Unsupported("Git object format".into()));
    }
    let refs_args = [
        "for-each-ref",
        "--format=%(refname)%00%(objectname)%00%(objecttype)",
    ];
    let refs_before = git.run(&refs_args, None, limits.max_total_bytes)?;
    let mut references = vec![];
    let mut selected = BTreeSet::new();
    let mut warnings = vec!["Selection includes local branches and commit tags only; detached-only commits, reflogs and remote-tracking references are not imported".into()];
    let mut tag_bytes = 0usize;
    for row in refs_before
        .split(|b| *b == b'\n')
        .filter(|row| !row.is_empty())
    {
        let fields = row.split(|b| *b == 0).collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err(invalid("Git ref framing"));
        }
        let name = utf8(fields[0])?.to_owned();
        if requested
            .as_ref()
            .is_some_and(|names| !names.contains(name.as_str()))
        {
            continue;
        }
        if !name.starts_with("refs/heads/") && !name.starts_with("refs/tags/") {
            warnings.push(format!("Reference not imported: {name}"));
            continue;
        }
        if references.len() >= limits.max_objects {
            return Err(BridgeError::Limit("Git references"));
        }
        let oid = hex_id(utf8(fields[1])?)?.to_owned();
        let (commit, tag) = match fields[2] {
            b"commit" => (oid.clone(), None),
            b"tag" => {
                let raw = git.object("tag", &oid, limits)?;
                tag_bytes = tag_bytes
                    .checked_add(raw.len())
                    .ok_or(BridgeError::Limit("tag bytes"))?;
                if tag_bytes > limits.max_total_bytes {
                    return Err(BridgeError::Limit("tag bytes"));
                }
                let headers = parse_headers(&raw)?;
                if header(&headers, "type")? != b"commit" {
                    return Err(BridgeError::Unsupported(
                        "tag that does not directly target a commit".into(),
                    ));
                }
                if signature_start(&raw).is_some() {
                    warnings.push(format!("Tag signature kept only as provenance: {name}"));
                }
                (
                    hex_id(utf8(header(&headers, "object")?)?)?.to_owned(),
                    Some(STANDARD.encode(raw)),
                )
            }
            _ => {
                return Err(BridgeError::Unsupported(format!(
                    "non-commit reference: {name}"
                )));
            }
        };
        selected.insert(commit.clone());
        references.push((name, commit, tag));
    }
    if requested
        .as_ref()
        .is_some_and(|names| names.len() != references.len())
    {
        return Err(invalid(
            "selected Git reference missing; no import prepared",
        ));
    }
    if requested.is_some() {
        warnings.push("Explicit reference selection; each selected tip includes its complete ancestor closure, other references are omitted".into());
    }
    if selected.is_empty() {
        return Err(BridgeError::Unsupported(
            "no local branches or commit tags to import".into(),
        ));
    }
    let mut input = String::new();
    for id in &selected {
        input.push_str(id);
        input.push('\n');
    }
    let order = git.run(
        &["rev-list", "--topo-order", "--reverse", "--stdin"],
        Some(input.as_bytes()),
        limits.max_commits.saturating_mul(65),
    )?;
    let commits = utf8(&order)?.lines().collect::<Vec<_>>();
    if commits.len() > limits.max_commits {
        return Err(BridgeError::Limit("Git commits"));
    }
    let all_objects = git.run(
        &["rev-list", "--objects", "--no-object-names", "--stdin"],
        Some(input.as_bytes()),
        limits.max_objects.saturating_mul(65),
    )?;
    let object_ids = utf8(&all_objects)?.lines().collect::<Vec<_>>();
    git.prefetch(&object_ids, limits)?;
    if git.run(&refs_args, None, limits.max_total_bytes)? != refs_before {
        return Err(invalid("Git references changed during acquisition; rescan"));
    }
    let processes = git.processes();
    Ok(AcquiredScan {
        metadata: FrozenMetadata {
            format,
            references,
            commits: commits.into_iter().map(str::to_owned).collect(),
            tag_bytes,
            warnings,
        },
        objects: git.into_cache(),
        processes,
        started,
    })
}

pub(crate) fn convert(
    acquired: AcquiredScan,
    limits: GitBridgeLimits,
    callback: &mut ProgressCallback<'_>,
) -> Result<ImportPlan> {
    let started = acquired.started;
    let processes = acquired.processes;
    let FrozenMetadata {
        format,
        references,
        commits,
        tag_bytes,
        warnings,
    } = acquired.metadata;
    let git = process::Git::cached(acquired.objects, limits)?;
    let mut reader = ImportReader {
        git: &git,
        limits,
        objects: BTreeMap::new(),
        trees: BTreeMap::new(),
        blobs: BTreeMap::new(),
        bytes: tag_bytes,
        paths: 0,
        warnings,
    };
    let mut mapping = BTreeMap::new();
    let mut provenance = vec![];
    let count = commits.len();
    progress(
        callback,
        started,
        BridgePhase::ImportCommits,
        0,
        Some(count),
        0,
        reader.bytes,
        processes,
    )?;
    for (index, oid) in commits.iter().map(String::as_str).enumerate() {
        hex_id(oid)?;
        let raw = git.object("commit", oid, limits)?;
        reader.charge(raw.len())?;
        let headers = parse_headers(&raw)?;
        if headers
            .iter()
            .any(|(key, _)| key == "gpgsig" || key == "gpgsig-sha256")
        {
            reader
                .warnings
                .push(format!("Commit signature kept only as provenance: {oid}"));
        }
        let root = reader.tree(hex_id(utf8(header(&headers, "tree")?)?)?, 0)?;
        let parents = headers
            .iter()
            .filter(|(key, _)| key == "parent")
            .map(|(_, bytes)| Ok(hex_id(utf8(bytes)?)?.to_owned()))
            .collect::<Result<Vec<_>>>()?;
        let mut native_parents = parents
            .iter()
            .map(|id| {
                mapping
                    .get(id)
                    .cloned()
                    .ok_or_else(|| invalid("Git topological parent missing"))
            })
            .collect::<Result<Vec<StepId>>>()?;
        native_parents.sort();
        native_parents.dedup();
        if native_parents.len() != parents.len() {
            reader.warnings.push(format!("Canonical parent identities coalesced for Git commit {oid}; original parent list retained"));
        }
        let step = Step::new(native_parents, root).map_err(invalid)?;
        if mapping.values().any(|id| id == step.id()) {
            reader.warnings.push(format!(
                "Git commit {oid} shares canonical Step {}; distinct provenance retained",
                step.id()
            ));
        }
        mapping.insert(oid.to_owned(), step.id().clone());
        provenance.push(GitCommitMapping {
            git_id: oid.to_owned(),
            step_id: step.id().clone(),
            parent_git_ids: parents,
            raw_commit_base64: STANDARD.encode(raw),
        });
        reader.insert(CanonicalObject::Step(step))?;
        progress(
            callback,
            started,
            BridgePhase::ImportCommits,
            index + 1,
            Some(count),
            reader.objects.len(),
            reader.bytes,
            processes,
        )?;
    }
    let references = references
        .into_iter()
        .map(|(git_ref, git_id, annotated_tag_base64)| {
            let step_id = mapping
                .get(&git_id)
                .ok_or_else(|| invalid("reference commit missing"))?
                .clone();
            Ok(GitRefMapping {
                git_ref,
                git_id,
                step_id,
                annotated_tag_base64,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut report = ImportReport {
        import_id: String::new(),
        object_format: format,
        commits: provenance,
        references,
        warnings: reader.warnings,
        source_bytes: reader.bytes,
    };
    let bytes = canonical_json(&report).map_err(invalid)?;
    if bytes.len() > limits.max_total_bytes {
        return Err(BridgeError::Limit("import provenance"));
    }
    report.import_id = blake3::hash(&bytes).to_hex().to_string();
    progress(
        callback,
        started,
        BridgePhase::ImportComplete,
        count,
        Some(count),
        reader.objects.len(),
        report.source_bytes,
        processes,
    )?;
    Ok(ImportPlan {
        report,
        objects: reader.objects.into_values().collect(),
    })
}

pub(crate) fn parse_headers(raw: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let split = raw
        .windows(2)
        .position(|w| w == b"\n\n")
        .ok_or_else(|| invalid("Git object header terminator missing"))?;
    let mut headers: Vec<(String, Vec<u8>)> = vec![];
    for line in raw[..split].split(|b| *b == b'\n') {
        if line.starts_with(b" ") {
            let (_, value) = headers
                .last_mut()
                .ok_or_else(|| invalid("Git header continuation"))?;
            value.push(b'\n');
            value.extend_from_slice(line);
            continue;
        }
        let space = line
            .iter()
            .position(|b| *b == b' ')
            .ok_or_else(|| invalid("Git header"))?;
        headers.push((utf8(&line[..space])?.to_owned(), line[space + 1..].to_vec()));
    }
    Ok(headers)
}

fn raw_tree_listing(mut raw: &[u8], hash_bytes: usize) -> Result<Vec<u8>> {
    use std::fmt::Write;
    if ![20, 32].contains(&hash_bytes) {
        return Err(invalid("Git tree object format"));
    }
    let mut listing = Vec::new();
    while !raw.is_empty() {
        let space = raw
            .iter()
            .position(|b| *b == b' ')
            .ok_or_else(|| invalid("Git tree mode"))?;
        let mode = utf8(&raw[..space])?;
        let tail = &raw[space + 1..];
        let nul = tail
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| invalid("Git tree name"))?;
        let hash = tail
            .get(nul + 1..nul + 1 + hash_bytes)
            .ok_or_else(|| invalid("Git tree hash framing"))?;
        let (mode, kind) = match mode {
            "40000" | "040000" => ("040000", "tree"),
            "100644" => ("100644", "blob"),
            "100755" => ("100755", "blob"),
            "120000" => ("120000", "blob"),
            "160000" => ("160000", "commit"),
            _ => return Err(BridgeError::Unsupported(format!("Git tree mode {mode}"))),
        };
        let mut id = String::with_capacity(hash_bytes * 2);
        for byte in hash {
            write!(&mut id, "{byte:02x}").expect("String write");
        }
        listing.extend_from_slice(format!("{mode} {kind} {id}\t").as_bytes());
        listing.extend_from_slice(&tail[..nul]);
        listing.push(0);
        raw = &tail[nul + 1 + hash_bytes..];
    }
    Ok(listing)
}
pub(crate) fn header<'a>(headers: &'a [(String, Vec<u8>)], name: &str) -> Result<&'a [u8]> {
    let values = headers
        .iter()
        .filter(|(key, _)| key == name)
        .collect::<Vec<_>>();
    if values.len() != 1 {
        return Err(invalid(format!("expected one Git {name} header")));
    }
    Ok(&values[0].1)
}

struct ImportReader<'a> {
    git: &'a process::Git,
    limits: GitBridgeLimits,
    objects: BTreeMap<String, CanonicalObject>,
    trees: BTreeMap<String, TreeId>,
    blobs: BTreeMap<String, Vec<u8>>,
    bytes: usize,
    paths: usize,
    warnings: Vec<String>,
}
impl ImportReader<'_> {
    fn charge(&mut self, size: usize) -> Result<()> {
        self.bytes = self
            .bytes
            .checked_add(size)
            .ok_or(BridgeError::Limit("import bytes"))?;
        if self.bytes > self.limits.max_total_bytes {
            return Err(BridgeError::Limit("import bytes"));
        }
        Ok(())
    }
    fn insert(&mut self, object: CanonicalObject) -> Result<()> {
        let id = object.object_id().as_str().to_owned();
        if !self.objects.contains_key(&id) && self.objects.len() >= self.limits.max_objects {
            return Err(BridgeError::Limit("native objects"));
        }
        self.objects.insert(id, object);
        Ok(())
    }
    fn blob(&mut self, id: &str) -> Result<Vec<u8>> {
        if let Some(bytes) = self.blobs.get(id) {
            return Ok(bytes.clone());
        }
        let bytes = self.git.object("blob", id, self.limits)?;
        self.charge(bytes.len())?;
        if bytes.starts_with(b"version https://git-lfs.github.com/spec/v1") {
            return Err(BridgeError::Unsupported(
                "Git LFS pointer: materialize LFS explicitly before import".into(),
            ));
        }
        if self.blobs.len() >= self.limits.max_objects {
            return Err(BridgeError::Limit("Git blobs"));
        }
        self.blobs.insert(id.to_owned(), bytes.clone());
        Ok(bytes)
    }
    fn tree(&mut self, id: &str, depth: usize) -> Result<TreeId> {
        if depth > self.limits.max_depth {
            return Err(BridgeError::Limit("Git tree depth"));
        }
        if let Some(tree) = self.trees.get(id) {
            return Ok(tree.clone());
        }
        let raw = self.git.object("tree", id, self.limits)?;
        self.charge(raw.len())?;
        let listing = raw_tree_listing(&raw, id.len() / 2)?;
        let mut entries = vec![];
        let mut folded = HashSet::new();
        for row in listing.split(|b| *b == 0).filter(|row| !row.is_empty()) {
            self.paths += 1;
            if self.paths > self.limits.max_paths {
                return Err(BridgeError::Limit("Git paths"));
            }
            let tab = row
                .iter()
                .position(|b| *b == b'\t')
                .ok_or_else(|| invalid("Git tree record"))?;
            let name = component(&row[tab + 1..])?;
            if !folded.insert(name.as_str().to_lowercase()) {
                self.warnings.push(format!(
                    "Case collision retained in tree {id}: {} (checkout may be unsupported)",
                    name
                ));
            }
            let fields = utf8(&row[..tab])?.split(' ').collect::<Vec<_>>();
            if fields.len() != 3 {
                return Err(invalid("Git tree header"));
            }
            let object = hex_id(fields[2])?;
            let (target, executable) = match fields[0] {
                "040000" => (TreeEntryTarget::tree(self.tree(object, depth + 1)?), false),
                "100644" | "100755" => {
                    let bytes = self.blob(object)?;
                    let chunks = if bytes.is_empty() {
                        vec![]
                    } else {
                        vec![Chunk::new(bytes).map_err(invalid)?]
                    };
                    let manifest = FileManifest::from_chunks(ChunkingDescriptor::single(), &chunks)
                        .map_err(invalid)?;
                    let target = TreeEntryTarget::file(manifest.file_id().clone());
                    for chunk in chunks {
                        self.insert(CanonicalObject::ChunkRaw(chunk))?;
                    }
                    self.insert(CanonicalObject::FileManifest(manifest))?;
                    (target, fields[0] == "100755")
                }
                "120000" => {
                    let bytes = self.blob(object)?;
                    let link = Symlink::new(utf8(&bytes)?).map_err(invalid)?;
                    let target = TreeEntryTarget::symlink(link.id().clone());
                    self.insert(CanonicalObject::Symlink(link))?;
                    (target, false)
                }
                "160000" => return Err(BridgeError::Unsupported("Git submodule/gitlink".into())),
                mode => return Err(BridgeError::Unsupported(format!("Git mode {mode}"))),
            };
            entries.push(TreeEntry::new(name, target, executable));
        }
        let tree = Tree::new(entries).map_err(invalid)?;
        let native = tree.id().clone();
        self.insert(CanonicalObject::Tree(tree))?;
        self.trees.insert(id.to_owned(), native.clone());
        Ok(native)
    }
}
