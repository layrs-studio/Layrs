//! HTTPS metadata observations for a future Provider transition caller.
//! No Store gate, transaction, remount, byte hydration or retained server lease.
use super::*;
use layrs_core::projection::{
    EntryState, ProjectionEntry as LocalEntry, ProjectionSelection, ProjectionSnapshot,
};

const PATH_BYTES: usize = 8 * 1024 * 1024;
const METADATA_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderTransitionObservationStrategy {
    PreservedEpoch,
    ReobservedExpandedScope,
}

/// Caller-supplied DATA. None of these fields proves a live source matches it.
/// The host must authenticate the exact prefix and recheck every field under
/// its local source gate AFTER all HTTP and before any transition effects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderTransitionSourceAnchor {
    pub store_id: StoreId,
    pub worktree_id: WorktreeId,
    pub source_prefix_id: ObjectId,
    pub generation: u64,
    pub snapshot_digest: String,
    pub snapshot_parent_digest: Option<String>,
    pub selection_generation: u64,
    pub selection_snapshot_digest: String,
    pub selection_digest: String,
    pub capture_count: u64,
    pub capture_head: Option<ObjectId>,
    pub effective_image_digest: String,
}

/// Canonical metadata DATA, never file-byte availability or a network grant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderTransitionDescriptor {
    pub path: RepoPath,
    pub manifest: FileManifest,
}

pub struct ProviderTransitionObservationRequest<'a> {
    pub source: &'a ProviderTransitionSourceAnchor,
    pub origin: &'a str,
    pub space_id: &'a SpaceId,
    pub principal_id: &'a PrincipalId,
    pub original: &'a ProjectionSnapshot,
    pub original_selection: &'a ProjectionSelection,
    pub expanded_selection: &'a ProjectionSelection,
    /// Sorted unique selected exact paths asserted absent in the immutable root.
    pub explicit_absence_paths: &'a [RepoPath],
    /// Optional old metadata, including metadata needed for revoked work.
    /// Reuse requires fresh exact rights AND an identical selected manifest.
    pub historical_descriptors: &'a [ProviderTransitionDescriptor],
    pub strategy: ProviderTransitionObservationStrategy,
    pub control: Option<&'a NativeTransferControl>,
}

/// Only this module constructs a scope. Getters expose immutable owned DATA.
/// No credentials/capabilities, Debug or Serialize implementation is retained.
/// A final fence is a network observation, not authority held through SQL.
pub struct ObservedProviderTransitionScope {
    source: ProviderTransitionSourceAnchor,
    origin: String,
    space_id: SpaceId,
    principal_id: PrincipalId,
    strategy: ProviderTransitionObservationStrategy,
    original: ProjectionSnapshot,
    original_selection: ProjectionSelection,
    selection: ProjectionSelection,
    snapshot: ProjectionSnapshot,
    namespace: Vec<ProjectionEntry>,
    access: Vec<ProjectionAccessEntry>,
    denied_absence_paths: Vec<RepoPath>,
    protected_frontiers: Vec<RepoPath>,
    descriptors: Vec<ProviderTransitionDescriptor>,
    historical_descriptors: Vec<ProviderTransitionDescriptor>,
    explicit_absence_paths: Vec<RepoPath>,
    control: Option<NativeTransferControl>,
}

impl ObservedProviderTransitionScope {
    pub fn source_anchor(&self) -> &ProviderTransitionSourceAnchor {
        &self.source
    }
    pub fn origin(&self) -> &str {
        &self.origin
    }
    pub fn space_id(&self) -> &SpaceId {
        &self.space_id
    }
    pub fn principal_id(&self) -> &PrincipalId {
        &self.principal_id
    }
    pub fn strategy(&self) -> ProviderTransitionObservationStrategy {
        self.strategy
    }
    pub fn original_snapshot(&self) -> &ProjectionSnapshot {
        &self.original
    }
    pub fn original_selection(&self) -> &ProjectionSelection {
        &self.original_selection
    }
    pub fn selection(&self) -> &ProjectionSelection {
        &self.selection
    }
    pub fn snapshot(&self) -> &ProjectionSnapshot {
        &self.snapshot
    }
    pub fn step_id(&self) -> &StepId {
        &self.snapshot.base_step_id
    }
    pub fn root_tree_id(&self) -> &TreeId {
        &self.snapshot.base_root_tree_id
    }
    pub fn policy_set_id(&self) -> &ObjectId {
        &self.snapshot.observed_policy_set_id
    }
    pub fn authorization_epoch(&self) -> u64 {
        self.snapshot.observed_authorization_epoch
    }
    /// Complete namespace before selection filtering, sorted by exact path.
    pub fn namespace(&self) -> &[ProjectionEntry] {
        &self.namespace
    }
    /// Exact refreshed rows, including old/excluded paths and denied targets.
    /// A directory row never grants access to any descendant.
    pub fn access(&self) -> &[ProjectionAccessEntry] {
        &self.access
    }
    /// Only unreadable target=None rows; these MUST NOT become Absent rows.
    pub fn denied_absence_paths(&self) -> &[RepoPath] {
        &self.denied_absence_paths
    }
    /// Original explicit protected Tree boundaries retained conservatively.
    pub fn protected_frontiers(&self) -> &[RepoPath] {
        &self.protected_frontiers
    }
    pub fn descriptors(&self) -> &[ProviderTransitionDescriptor] {
        &self.descriptors
    }
    pub fn historical_descriptors(&self) -> &[ProviderTransitionDescriptor] {
        &self.historical_descriptors
    }
    pub fn explicit_absence_paths(&self) -> &[RepoPath] {
        &self.explicit_absence_paths
    }
}

fn invalid() -> NativeRemoteError {
    NativeRemoteError::Invalid("provider transition observation binding, coverage or bounds differ")
}
fn core_error(_: impl std::fmt::Display) -> NativeRemoteError {
    invalid()
}

fn raw_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// Charge the prefix-index footprint too, before constructing core path indexes.
#[derive(Default)]
struct PathBudget {
    count: usize,
    bytes: usize,
    index_bytes: usize,
}
impl PathBudget {
    fn add(&mut self, path: &RepoPath) -> RemoteResult<()> {
        let text = path.as_str();
        if text.len() > 4096 || text.split('/').count() > 128 {
            return Err(invalid());
        }
        self.count = self.count.checked_add(1).ok_or_else(invalid)?;
        self.bytes = self.bytes.checked_add(text.len()).ok_or_else(invalid)?;
        for (at, _) in text
            .match_indices('/')
            .chain(std::iter::once((text.len(), "/")))
        {
            self.index_bytes = self
                .index_bytes
                .checked_add(at.saturating_mul(8).saturating_add(256))
                .ok_or_else(invalid)?;
        }
        if self.count > MAX_OBJECTS || self.bytes > PATH_BYTES || self.index_bytes > METADATA_BYTES
        {
            return Err(invalid());
        }
        Ok(())
    }
    fn selection(selection: &ProjectionSelection) -> RemoteResult<()> {
        let mut budget = Self::default();
        if let ProjectionSelection::Selected { paths, subtrees } = selection {
            for path in paths.iter().chain(subtrees) {
                budget.add(path)?;
            }
        }
        Ok(())
    }
}

fn validate_request(
    remote: &NativeRemote,
    request: &ProviderTransitionObservationRequest<'_>,
) -> RemoteResult<()> {
    let anchor = request.source;
    if request.origin.len() > 4096
        || request.original.entries.len() > MAX_OBJECTS
        || request.explicit_absence_paths.len() > MAX_OBJECTS
        || request.historical_descriptors.len() > MAX_OBJECTS
    {
        return Err(invalid());
    }
    let origin = Url::parse(request.origin).map_err(core_error)?;
    if origin != remote.origin
        || request.origin.trim_end_matches('/') != remote.origin().trim_end_matches('/')
        || anchor.generation >= i64::MAX as u64
        || anchor.selection_generation > anchor.generation
        || (anchor.generation == 0) != anchor.snapshot_parent_digest.is_none()
        || anchor.capture_count > MAX_OBJECTS as u64
        || (anchor.capture_count == 0) != anchor.capture_head.is_none()
        || !raw_digest(&anchor.snapshot_digest)
        || !raw_digest(&anchor.selection_digest)
        || !raw_digest(&anchor.selection_snapshot_digest)
        || !raw_digest(&anchor.effective_image_digest)
        || anchor
            .snapshot_parent_digest
            .as_deref()
            .is_some_and(|d| !raw_digest(d))
        || (anchor.selection_generation == anchor.generation
            && anchor.selection_snapshot_digest != anchor.snapshot_digest)
        || request.original.observed_authorization_epoch == 0
        || request
            .explicit_absence_paths
            .windows(2)
            .any(|p| p[0] >= p[1])
        || request
            .historical_descriptors
            .windows(2)
            .any(|p| p[0].path >= p[1].path)
    {
        return Err(invalid());
    }
    let mut budget = PathBudget::default();
    for entry in &request.original.entries {
        remote.transfer_check()?;
        budget.add(&entry.path)?;
    }
    PathBudget::selection(request.original_selection)?;
    PathBudget::selection(request.expanded_selection)?;
    let mut budget = PathBudget::default();
    for path in request.explicit_absence_paths {
        budget.add(path)?;
        if !request.expanded_selection.includes(path) {
            return Err(invalid());
        }
    }
    request
        .original_selection
        .validate_snapshot(request.original)
        .map_err(core_error)?;
    if request
        .expanded_selection
        .normalized(request.original.path_semantics)
        .map_err(core_error)?
        != *request.expanded_selection
        || !request
            .expanded_selection
            .is_expansion_of(request.original_selection)
    {
        return Err(invalid());
    }
    let mut metadata = 0usize;
    let mut budget = PathBudget::default();
    for descriptor in request.historical_descriptors {
        remote.transfer_check()?;
        budget.add(&descriptor.path)?;
        let entry = request
            .original
            .entries
            .binary_search_by(|e| e.path.cmp(&descriptor.path))
            .ok()
            .map(|i| &request.original.entries[i])
            .ok_or_else(invalid)?;
        if entry.target.as_ref()
            != Some(&TreeEntryTarget::file(
                descriptor.manifest.file_id().clone(),
            ))
        {
            return Err(invalid());
        }
        charge_manifest(&descriptor.manifest, &mut metadata)?;
    }
    Ok(())
}

fn charge_manifest(manifest: &FileManifest, total: &mut usize) -> RemoteResult<()> {
    // Bound before canonical encoding; FileManifest already validates coverage/id.
    if manifest.chunks().len() > 10_000 {
        return Err(invalid());
    }
    let bytes = canonical_json(manifest)?;
    if bytes.len() > MAX_OBJECT as usize {
        return Err(invalid());
    }
    *total = total
        .checked_add(bytes.len())
        .filter(|n| *n <= METADATA_BYTES)
        .ok_or_else(invalid)?;
    Ok(())
}

fn same_fence(base: &SpaceBootstrap, current: &SpaceBootstrap) -> RemoteResult<()> {
    if base.space_id != current.space_id
        || base.principal_id != current.principal_id
        || base.current_policy_set_id != current.current_policy_set_id
        || base.authorization_epoch != current.authorization_epoch
    {
        return Err(invalid());
    }
    Ok(())
}

impl NativeRemote {
    /// Complete bounded metadata collection. Call before acquiring Store SQL or
    /// operation gates. No local source acceptance or transition is performed.
    pub fn observe_provider_transition(
        &self,
        request: &ProviderTransitionObservationRequest<'_>,
    ) -> RemoteResult<ObservedProviderTransitionScope> {
        let remote = self.provider_observation_remote(request.control);
        remote.transfer_check()?;
        validate_request(&remote, request)?;
        let base = remote.bootstrap(request.space_id)?;
        if &base.principal_id != request.principal_id
            || base.authorization_epoch < request.original.observed_authorization_epoch
            || (base.authorization_epoch == request.original.observed_authorization_epoch
                && base.current_policy_set_id != request.original.observed_policy_set_id)
            || (request.strategy == ProviderTransitionObservationStrategy::PreservedEpoch
                && (base.current_policy_set_id != request.original.observed_policy_set_id
                    || base.authorization_epoch != request.original.observed_authorization_epoch))
        {
            return Err(invalid());
        }
        let namespace = remote.provider_namespace(&base, request.original)?;
        let access = remote.provider_access(&base, request, &namespace)?;
        let (snapshot, denied_absence_paths, protected_frontiers) =
            provider_snapshot(request, &base, &access)?;
        let binding = Binding {
            origin: remote.origin().into(),
            space_id: base.space_id.clone(),
            client_id: ClientId::parse(format!("client:{}", uuid::Uuid::new_v4()))
                .map_err(core_error)?,
            clone_pending: false,
            observed: Some(base.clone()),
            pending: None,
            last_receipt: None,
        };
        let mut descriptors = Vec::new();
        let mut metadata = 0usize;
        for entry in &snapshot.entries {
            remote.transfer_check()?;
            let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
                continue;
            };
            if entry.state != EntryState::Unavailable
                || !request.expanded_selection.includes(&entry.path)
            {
                continue;
            }
            let cached = request
                .historical_descriptors
                .binary_search_by(|d| d.path.cmp(&entry.path))
                .ok()
                .map(|i| &request.historical_descriptors[i].manifest);
            let manifest = remote.provider_manifest(
                &binding,
                &entry.path,
                file_id,
                &snapshot.base_step_id,
                cached,
            )?;
            charge_manifest(&manifest, &mut metadata)?;
            descriptors.push(ProviderTransitionDescriptor {
                path: entry.path.clone(),
                manifest,
            });
        }
        same_fence(&base, &remote.bootstrap(request.space_id)?)?;
        remote.transfer_check()?;
        Ok(ObservedProviderTransitionScope {
            source: request.source.clone(),
            origin: remote.origin().trim_end_matches('/').into(),
            space_id: base.space_id,
            principal_id: base.principal_id,
            strategy: request.strategy,
            original: request.original.clone(),
            original_selection: request.original_selection.clone(),
            selection: request.expanded_selection.clone(),
            snapshot,
            namespace,
            access,
            denied_absence_paths,
            protected_frontiers,
            descriptors,
            historical_descriptors: request.historical_descriptors.to_vec(),
            explicit_absence_paths: request.explicit_absence_paths.to_vec(),
            control: remote.transfer.clone(),
        })
    }

    /// Fresh before/after bootstrap fence plus a fixed-Step/root projection
    /// probe. Call after any additional HTTP, before reacquiring local gates.
    /// It cannot validate caller-supplied source anchors or hold server authority.
    pub fn revalidate_provider_transition_scope(
        &self,
        scope: &ObservedProviderTransitionScope,
    ) -> RemoteResult<()> {
        let remote = self.provider_observation_remote(scope.control.as_ref());
        remote.transfer_check()?;
        if remote.origin().trim_end_matches('/') != scope.origin {
            return Err(invalid());
        }
        let base = remote.bootstrap(&scope.space_id)?;
        if base.principal_id != scope.principal_id
            || base.current_policy_set_id != scope.snapshot.observed_policy_set_id
            || base.authorization_epoch != scope.snapshot.observed_authorization_epoch
        {
            return Err(invalid());
        }
        let page = remote.projection_page(
            &scope.space_id,
            &ProjectionPageRequest {
                step_id: scope.snapshot.base_step_id.clone(),
                expected_policy_set_id: base.current_policy_set_id.clone(),
                expected_authorization_epoch: base.authorization_epoch,
                cursor: None,
                limit: 1,
            },
        )?;
        if page.principal_id != scope.principal_id
            || page.root_tree_id != scope.snapshot.base_root_tree_id
        {
            return Err(invalid());
        }
        same_fence(&base, &remote.bootstrap(&scope.space_id)?)?;
        remote.transfer_check()
    }

    fn provider_observation_remote(&self, control: Option<&NativeTransferControl>) -> Self {
        Self {
            http: self.http.clone(),
            origin: self.origin.clone(),
            token: self.token.clone(),
            transfer: control.cloned().or_else(|| self.transfer.clone()),
            download_cache: self.download_cache.clone(),
        }
    }

    fn provider_namespace(
        &self,
        base: &SpaceBootstrap,
        original: &ProjectionSnapshot,
    ) -> RemoteResult<Vec<ProjectionEntry>> {
        let mut request = ProjectionPageRequest {
            step_id: original.base_step_id.clone(),
            expected_policy_set_id: base.current_policy_set_id.clone(),
            expected_authorization_epoch: base.authorization_epoch,
            cursor: None,
            limit: 200,
        };
        let mut offset = 0u32;
        let mut cursors = BTreeSet::new();
        let mut cursor_bytes = 0usize;
        let mut entries = BTreeMap::new();
        let mut budget = PathBudget::default();
        loop {
            self.transfer_check()?;
            let page = self.projection_page(&base.space_id, &request)?;
            if page.principal_id != base.principal_id
                || page.root_tree_id != original.base_root_tree_id
                || page.offset != offset
            {
                return Err(invalid());
            }
            offset = offset
                .checked_add(page.entries.len() as u32)
                .filter(|n| *n <= MAX_OBJECTS as u32)
                .ok_or_else(invalid)?;
            for entry in page.entries {
                self.transfer_check()?;
                budget.add(&entry.path)?;
                if entries.insert(entry.path.clone(), entry).is_some() {
                    return Err(invalid());
                }
            }
            let Some(cursor) = page.next_cursor else {
                break;
            };
            cursor_bytes = cursor_bytes
                .checked_add(cursor.len())
                .filter(|n| *n <= PATH_BYTES)
                .ok_or_else(invalid)?;
            if cursor.is_empty() || !cursors.insert(cursor.clone()) {
                return Err(invalid());
            }
            request.cursor = Some(cursor);
        }
        // A child cannot appear without every explicit structural Tree parent.
        for entry in entries.values() {
            for (at, _) in entry.path.as_str().match_indices('/') {
                let parent = RepoPath::parse(&entry.path.as_str()[..at])?;
                if !entries
                    .get(&parent)
                    .is_some_and(|e| matches!(e.target, TreeEntryTarget::Tree { .. }))
                {
                    return Err(invalid());
                }
            }
        }
        // Use core's complete path/alias validator before selection filtering.
        let snapshot = ProjectionSnapshot {
            schema_version: "layrs.projection-snapshot.v1".into(),
            base_step_id: original.base_step_id.clone(),
            base_root_tree_id: original.base_root_tree_id.clone(),
            observed_policy_set_id: base.current_policy_set_id.clone(),
            observed_authorization_epoch: base.authorization_epoch,
            path_semantics: original.path_semantics,
            entries: entries
                .values()
                .map(|e| LocalEntry {
                    path: e.path.clone(),
                    target: Some(e.target.clone()),
                    executable: e.executable,
                    writable: e.writable,
                    state: EntryState::Unavailable,
                })
                .collect(),
        };
        snapshot.validate().map_err(core_error)?;
        Ok(entries.into_values().collect())
    }

    fn provider_access(
        &self,
        base: &SpaceBootstrap,
        request: &ProviderTransitionObservationRequest<'_>,
        namespace: &[ProjectionEntry],
    ) -> RemoteResult<Vec<ProjectionAccessEntry>> {
        // Refresh every enumerated path, every original path and all selection /
        // absence anchors. No cached manifest can bypass this exact-path phase.
        let mut paths = BTreeSet::new();
        let mut budget = PathBudget::default();
        let anchors = |selection: &ProjectionSelection| -> Vec<RepoPath> {
            match selection {
                ProjectionSelection::All => vec![],
                ProjectionSelection::Selected { paths, subtrees } => {
                    paths.iter().chain(subtrees).cloned().collect()
                }
            }
        };
        for path in namespace
            .iter()
            .map(|e| &e.path)
            .chain(request.original.entries.iter().map(|e| &e.path))
            .chain(request.explicit_absence_paths)
            .cloned()
            .chain(anchors(request.original_selection))
            .chain(anchors(request.expanded_selection))
        {
            self.transfer_check()?;
            if paths.insert(path.clone()) {
                budget.add(&path)?;
            }
        }
        // Cross-view alias validation includes unknown/denied absence anchors.
        ProjectionSelection::selected(
            paths.iter().cloned().collect(),
            vec![],
            request.original.path_semantics,
        )
        .map_err(core_error)?;
        let paths: Vec<_> = paths.into_iter().collect();
        let mut access = Vec::with_capacity(paths.len());
        for batch in paths.chunks(200) {
            let response = self.projection_access(
                &base.space_id,
                &ProjectionAccessRequest {
                    step_id: request.original.base_step_id.clone(),
                    expected_policy_set_id: base.current_policy_set_id.clone(),
                    expected_authorization_epoch: base.authorization_epoch,
                    paths: batch.to_vec(),
                },
            )?;
            if response.principal_id != base.principal_id
                || response.root_tree_id != request.original.base_root_tree_id
            {
                return Err(invalid());
            }
            for row in response.entries {
                self.transfer_check()?;
                let enumerated = namespace
                    .binary_search_by(|e| e.path.cmp(&row.path))
                    .ok()
                    .map(|i| &namespace[i]);
                if let Some(entry) = enumerated {
                    if row.target.as_ref() != Some(&entry.target)
                        || row.executable != entry.executable
                        || row.content_readable != entry.content_readable
                        || row.writable != entry.writable
                    {
                        return Err(invalid());
                    }
                } else if row.target.is_some() {
                    return Err(invalid());
                }
                if let Ok(i) = request
                    .original
                    .entries
                    .binary_search_by(|e| e.path.cmp(&row.path))
                {
                    let old = &request.original.entries[i];
                    if row.target.as_ref() != old.target.as_ref()
                        || row.executable != old.executable
                    {
                        return Err(invalid());
                    }
                    if request.strategy == ProviderTransitionObservationStrategy::PreservedEpoch
                        && !matches!(old.target, Some(TreeEntryTarget::Tree { .. }))
                        && (row.writable != old.writable
                            || row.content_readable == (old.state == EntryState::Reserved))
                    {
                        return Err(invalid());
                    }
                }
                if request
                    .explicit_absence_paths
                    .binary_search(&row.path)
                    .is_ok()
                    && row.target.is_some()
                {
                    return Err(invalid());
                }
                access.push(row);
            }
        }
        Ok(access)
    }

    fn provider_manifest(
        &self,
        binding: &Binding,
        path: &RepoPath,
        file: &FileId,
        step: &StepId,
        cached: Option<&FileManifest>,
    ) -> RemoteResult<FileManifest> {
        let claim = FetchClaim::File {
            file_id: file.clone(),
            preferred_manifest_ids: cached.map(|m| vec![m.id().clone()]).unwrap_or_default(),
            root: AuthorizationRoot::Step {
                step_id: step.clone(),
            },
            repo_path: path.clone(),
        };
        let plan = self.fetch_plan(binding, &claim, self.transfer_timeout())?;
        validate_metadata_plan(binding, &claim, &plan)?;
        let selection = &plan.file_manifest_selections[0];
        if let Some(manifest) = cached.filter(|m| m.id() == &selection.file_manifest_id) {
            validate_manifest_selection(manifest, selection, file)?;
            return Ok(manifest.clone());
        }
        let cap = plan
            .download_capabilities
            .iter()
            .find(|c| c.descriptor == selection.manifest_descriptor)
            .ok_or_else(invalid)?;
        // Deliberately download ONE selected manifest, never fetch_visit's whole
        // file closure. Chunk capabilities remain unused and never escape here.
        let object = self.download_object_from_store(cap, None, |remaining| {
            let renewed = self.fetch_plan(binding, &claim, remaining)?;
            validate_metadata_plan(binding, &claim, &renewed)?;
            if plan.file_manifest_selections != renewed.file_manifest_selections {
                return Err(invalid());
            }
            // Renewal must preserve the full descriptor/claim observation.
            let data = |p: &FetchPlanResponse| -> RemoteResult<BTreeSet<Vec<u8>>> {
                p.download_capabilities
                    .iter()
                    .map(|c| {
                        Ok(canonical_json(&(
                            &c.claim,
                            &c.claim_digest,
                            &c.descriptor,
                            c.accept_ranges,
                        ))?)
                    })
                    .collect()
            };
            if data(&plan)? != data(&renewed)? {
                return Err(invalid());
            }
            renewed
                .download_capabilities
                .into_iter()
                .find(|c| c.descriptor == selection.manifest_descriptor)
                .ok_or_else(invalid)
        })?;
        let CanonicalObject::FileManifest(manifest) = object else {
            return Err(invalid());
        };
        validate_manifest_selection(&manifest, selection, file)?;
        Ok(manifest)
    }
}

fn validate_metadata_plan(
    binding: &Binding,
    claim: &FetchClaim,
    plan: &FetchPlanResponse,
) -> RemoteResult<()> {
    let base = binding.observed.as_ref().ok_or_else(invalid)?;
    if plan.principal_id != base.principal_id
        || plan.authorization_epoch != base.authorization_epoch
        || plan.space_id != binding.space_id
        || plan.client_id != binding.client_id
    {
        return Err(invalid());
    }
    validate_fetch_descriptors(plan, claim)?;
    // claim_digest is the server's opaque internal claim encoding digest (not
    // the canonical wire FetchClaim hash). Bind the echoed exact claim itself,
    // require one digest across its closure, and retain it across renewal.
    let digest = &plan
        .download_capabilities
        .first()
        .ok_or_else(invalid)?
        .claim_digest;
    let mut seen = BTreeSet::new();
    for cap in &plan.download_capabilities {
        if cap.download_url.len() > 4096 {
            return Err(invalid());
        }
        let url = Url::parse(&cap.download_url).map_err(core_error)?;
        let origin = Url::parse(&binding.origin).map_err(core_error)?;
        if cap.principal_id != base.principal_id
            || cap.authorization_epoch != base.authorization_epoch
            || cap.space_id != binding.space_id
            || cap.client_id != binding.client_id
            || cap.request_id != plan.request_id
            || cap.plan_id != plan.plan_id
            || &cap.claim != claim
            || &cap.claim_digest != digest
            || url.origin() != origin.origin()
            || !seen.insert(cap.descriptor.clone())
        {
            return Err(invalid());
        }
    }
    Ok(())
}

fn validate_manifest_selection(
    manifest: &FileManifest,
    selection: &FileManifestSelection,
    file: &FileId,
) -> RemoteResult<()> {
    if manifest.file_id() != file
        || &selection.file_id != file
        || manifest.id() != &selection.file_manifest_id
        || selection.manifest_descriptor.object_id.as_str() != manifest.id().as_str()
        || selection.manifest_descriptor.kind != ObjectKind::FileManifest
        || canonical_json(manifest)?.len() as u64
            != selection.manifest_descriptor.canonical_byte_len
    {
        return Err(invalid());
    }
    let chunks: BTreeMap<_, _> = selection
        .chunk_descriptors
        .iter()
        .map(|d| (d.object_id.as_str(), d))
        .collect();
    let expected: BTreeSet<_> = manifest
        .chunks()
        .iter()
        .map(|c| c.chunk_id().as_str())
        .collect();
    if chunks.len() != selection.chunk_descriptors.len()
        || chunks.keys().copied().collect::<BTreeSet<_>>() != expected
        || manifest.chunks().iter().any(|c| {
            chunks.get(c.chunk_id().as_str()).is_none_or(|d| {
                d.kind != ObjectKind::Chunk || d.canonical_byte_len != c.byte_length()
            })
        })
    {
        return Err(invalid());
    }
    Ok(())
}

fn provider_snapshot(
    request: &ProviderTransitionObservationRequest<'_>,
    base: &SpaceBootstrap,
    access: &[ProjectionAccessEntry],
) -> RemoteResult<(ProjectionSnapshot, Vec<RepoPath>, Vec<RepoPath>)> {
    let traversal = request
        .expanded_selection
        .traversal(request.original.path_semantics)
        .map_err(core_error)?;
    let frontiers: BTreeSet<_> = request
        .original
        .entries
        .iter()
        .filter(|e| {
            e.state == EntryState::Reserved
                && matches!(e.target, Some(TreeEntryTarget::Tree { .. }))
        })
        .map(|e| e.path.as_str())
        .collect();
    let below_frontier = |path: &RepoPath| {
        path.as_str()
            .match_indices('/')
            .any(|(at, _)| frontiers.contains(&path.as_str()[..at]))
    };
    let mut snapshot = ProjectionSnapshot {
        schema_version: "layrs.projection-snapshot.v1".into(),
        base_step_id: request.original.base_step_id.clone(),
        base_root_tree_id: request.original.base_root_tree_id.clone(),
        observed_policy_set_id: base.current_policy_set_id.clone(),
        observed_authorization_epoch: base.authorization_epoch,
        path_semantics: request.original.path_semantics,
        entries: Vec::new(),
    };
    let mut denied = Vec::new();
    for row in access {
        if let Some(control) = request.control {
            control.check()?;
        }
        if !traversal
            .intersects(row.path.as_str())
            .map_err(core_error)?
            || below_frontier(&row.path)
        {
            continue;
        }
        if row.target.is_none() && !row.content_readable {
            denied.push(row.path.clone());
            continue;
        }
        // Ancestors absent from the immutable tree are exact access DATA, not
        // invented absent directories owning a selected descendant.
        if row.target.is_none() && !request.expanded_selection.includes(&row.path) {
            continue;
        }
        let frontier = frontiers.contains(row.path.as_str());
        let tree = matches!(row.target.as_ref(), Some(TreeEntryTarget::Tree { .. }));
        snapshot.entries.push(LocalEntry {
            path: row.path.clone(),
            target: row.target.0.clone(),
            executable: row.executable,
            writable: row.content_readable && row.writable && !frontier,
            state: if frontier {
                EntryState::Reserved
            } else if row.target.is_none() {
                EntryState::Absent
            } else if tree || row.content_readable {
                EntryState::Unavailable
            } else {
                EntryState::Reserved
            },
        });
    }
    request
        .expanded_selection
        .validate_snapshot(&snapshot)
        .map_err(core_error)?;
    let denied_set: BTreeSet<_> = denied.iter().map(|p| p.as_str()).collect();
    for path in snapshot
        .entries
        .iter()
        .map(|e| e.path.as_str())
        .chain(denied.iter().map(|p| p.as_str()))
    {
        if path
            .match_indices('/')
            .any(|(at, _)| denied_set.contains(&path[..at]))
        {
            return Err(invalid());
        }
    }
    let frontiers = frontiers
        .into_iter()
        .filter(|p| {
            !p.match_indices('/')
                .any(|(at, _)| frontiers_contains(request.original, &p[..at]))
        })
        .map(RepoPath::parse)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok((snapshot, denied, frontiers))
}

fn frontiers_contains(original: &ProjectionSnapshot, path: &str) -> bool {
    original
        .entries
        .binary_search_by(|e| e.path.as_str().cmp(path))
        .ok()
        .is_some_and(|i| {
            let entry = &original.entries[i];
            entry.state == EntryState::Reserved
                && matches!(entry.target, Some(TreeEntryTarget::Tree { .. }))
        })
}

#[cfg(test)]
#[path = "provider_observation_tests.rs"]
mod tests;
