//! Normalized transition observation DATA, not an access capability. No SQL,
//! filesystem, network, native registration or bootstrap codec is used here.
//!
//! A client must independently prove authority and complete enumeration of the
//! ENTIRE expanded scope before and after acquisition. Neither the mode enum
//! nor declared coverage below supplies that proof. This normalizer checks
//! structural coverage of known old paths and requested selection anchors.
use super::{
    cancellation,
    records::{
        codec,
        import::{ImportSource, ProviderImportPrefixRecord},
        DescriptorRecord, ProviderSourceDescriptor, Record,
    },
    recovery::error::{invalid, Result},
};
use crate::projection::{database, format, selection, ProjectionIdentity};
use layrs_core::{
    projection::{
        EntryState, PathSemantics, ProjectionSelection, ProjectionSnapshot, ReservedPaths,
    },
    scm::{ObjectId, RepoPath, TreeEntryTarget},
};
use layrs_vfs::Cancellation;
use std::collections::{BTreeMap, BTreeSet};

mod bounds;
use bounds::PathBudget;

/// Exact borrowed source DATA supplied by the separately validated history
/// adapter. This is not evidence of a retained transaction or source ownership.
pub(super) struct SourceObservationData<'a> {
    pub prefix: &'a ProviderImportPrefixRecord,
    pub identity: &'a ProjectionIdentity,
    pub snapshot: &'a ProjectionSnapshot,
    pub snapshot_parent_digest: Option<&'a str>,
    pub selection: &'a ProjectionSelection,
    /// The current selection may have been last stored BEFORE the source head.
    pub selection_generation: u64,
    pub selection_snapshot_digest: &'a str,
}

/// Declared authority strategy DATA only. A matching epoch is not a current
/// server check; reobservation is not proven by selecting the second variant.
#[derive(serde::Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(super) enum AuthorityObservationData {
    PreservedEpoch {
        policy_set_id: ObjectId,
        authorization_epoch: u64,
    },
    ReobservedExpandedScope {
        policy_set_id: ObjectId,
        authorization_epoch: u64,
    },
}

/// Explicit denied exact-path DATA, NOT an absence observation or subtree
/// enumeration. This keeps a formerly observed absence representable when its
/// new access is denied, without inventing a target or an absence/write grant.
/// A future durable transition contract must persist these rows separately;
/// they cannot be injected into the strict old bootstrap snapshot/codec.
pub(super) struct DeniedPathData {
    pub path: RepoPath,
}

/// Declared structural coverage only. The absence of unknown/new names cannot
/// be established from these DATA; full remote enumeration remains external.
#[derive(Default)]
pub(super) struct ObservationCoverageData {
    pub denied_paths: Vec<DeniedPathData>,
}

pub(super) struct ObservationBindingData {
    pub source_prefix_id: String,
    pub source_identity: ProjectionIdentity,
    pub destination_identity: ProjectionIdentity,
    pub source_generation: u64,
    pub source_snapshot_digest: String,
    pub source_selection_digest: String,
    pub source_capture_count: u64,
    pub source_capture_head: Option<String>,
    /// Structural ordering only. The later SQL append protocol must also
    /// enforce its own exact contiguous generation/parent relationships.
    pub generation: u64,
    pub snapshot_digest: String,
    pub selection_digest: String,
    pub descriptors_digest: String,
    pub descriptor_bytes: u64,
    pub denied_path_count: u64,
    pub denied_paths_digest: String,
    pub authority_data_digest: String,
}

/// Owned normalized DATA. Heavy values are private and never cloned wholesale.
/// In particular this is neither VerifiedAuthority nor ProviderBootstrapSource.
pub(super) struct TransitionObservationData {
    binding: ObservationBindingData,
    snapshot: ProjectionSnapshot,
    selection: ProjectionSelection,
    descriptors: Vec<DescriptorRecord>,
    coverage: ObservationCoverageData,
    authority: AuthorityObservationData,
}
impl TransitionObservationData {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn normalize(
        source: SourceObservationData<'_>,
        destination_identity: ProjectionIdentity,
        generation: u64,
        mut snapshot: ProjectionSnapshot,
        selected: ProjectionSelection,
        mut descriptors: Vec<ProviderSourceDescriptor>,
        mut coverage: ObservationCoverageData,
        authority: AuthorityObservationData,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        cancellation::check(cancel)?;
        // Count, bytes and conservative path-index footprint precede core's
        // maps, selection normalization, sorting, encodings and descriptor Vec.
        let mut budget = PathBudget::default();
        budget.snapshot(source.snapshot, cancel)?;
        budget.selection(source.selection, cancel)?;
        budget.snapshot(&snapshot, cancel)?;
        budget.selection(&selected, cancel)?;
        budget.descriptors(&descriptors, cancel)?;
        budget.denied(&coverage.denied_paths, cancel)?;
        let prefix_bytes = source.prefix.encode()?;
        let (identity, version) = match &source.prefix.source {
            ImportSource::Ordinary {
                identity, version, ..
            }
            | ImportSource::Provider {
                identity, version, ..
            } => (identity, version),
        };
        format::validate_identity(source.identity)?;
        format::validate_identity(&destination_identity)?;
        if source.identity != identity
            || destination_identity != source.prefix.destination_identity
            || destination_identity.worktree_id == identity.worktree_id
            || destination_identity.store_id != identity.store_id
            || destination_identity.space_id != identity.space_id
            || destination_identity.origin != identity.origin
            || destination_identity.principal_id != identity.principal_id
            || generation <= version.generation
            || generation >= i64::MAX as u64
            || source.snapshot.path_semantics != snapshot.path_semantics
            || source.snapshot.base_step_id != snapshot.base_step_id
            || source.snapshot.base_root_tree_id != snapshot.base_root_tree_id
            || source.snapshot.observed_authorization_epoch == 0
            || snapshot.observed_authorization_epoch == 0
            || snapshot
                .entries
                .iter()
                .any(|e| e.state == EntryState::Materialized)
        {
            return Err(invalid(
                "transition observation identity, version, Step/root or placement differs",
            ));
        }
        if (version.generation == 0) != source.snapshot_parent_digest.is_none()
            || source.selection_generation > version.generation
            || source.selection_generation == version.generation
                && source.selection_snapshot_digest != version.snapshot_digest
        {
            return Err(invalid(
                "transition source snapshot/selection anchor differs",
            ));
        }
        if let Some(parent) = source.snapshot_parent_digest {
            codec::raw_digest(parent)?;
        }
        codec::raw_digest(source.selection_snapshot_digest)?;
        source
            .selection
            .validate_snapshot(source.snapshot)
            .map_err(|error| invalid(error.to_string()))?;
        let source_bytes = codec::encode(source.snapshot, codec::SNAPSHOT_MAX)?;
        let old_selection = selection::encode(source.selection)?;
        if database::digest(
            version.generation,
            source.snapshot_parent_digest,
            &source_bytes,
        ) != version.snapshot_digest
            || database::digest(
                source.selection_generation,
                Some(source.selection_snapshot_digest),
                &old_selection,
            ) != version.selection_digest
        {
            return Err(invalid(
                "transition source snapshot/selection commitment differs",
            ));
        }
        let selected = selected
            .normalized(snapshot.path_semantics)
            .map_err(|error| invalid(error.to_string()))?;
        if !selected.is_expansion_of(source.selection) {
            return Err(invalid("transition selection shrink is unsupported"));
        }
        snapshot.entries.sort_by(|a, b| a.path.cmp(&b.path));
        selected
            .validate_snapshot(&snapshot)
            .map_err(|error| invalid(error.to_string()))?;
        let snapshot_bytes = codec::encode(&snapshot, codec::SNAPSHOT_MAX)?;
        let selection_bytes = selection::encode(&selected)?;
        coverage.denied_paths.sort_by(|a, b| a.path.cmp(&b.path));
        if coverage
            .denied_paths
            .windows(2)
            .any(|p| p[0].path == p[1].path)
        {
            return Err(invalid("transition denied paths repeat"));
        }
        // Reuse the core path validator for denied paths, including reserved
        // names, portable aliases, component/stream and traversal constraints.
        ReservedPaths::new(
            coverage.denied_paths.iter().map(|p| p.path.as_str()),
            snapshot.path_semantics,
        )
        .map_err(|error| invalid(error.to_string()))?;
        bounds::require_cross_view_aliases(
            source.snapshot,
            source.selection,
            &snapshot,
            &selected,
            &coverage.denied_paths,
            &descriptors,
            cancel,
        )?;
        let traversal = selected
            .traversal(snapshot.path_semantics)
            .map_err(|error| invalid(error.to_string()))?;
        let entries: BTreeMap<_, _> = snapshot
            .entries
            .iter()
            .map(|e| (e.path.as_str(), e))
            .collect();
        let denied: BTreeSet<_> = coverage
            .denied_paths
            .iter()
            .map(|p| p.path.as_str())
            .collect();
        let protected: BTreeSet<_> = snapshot
            .entries
            .iter()
            .filter(|e| {
                e.state == EntryState::Reserved
                    && matches!(e.target, Some(TreeEntryTarget::Tree { .. }))
            })
            .map(|e| e.path.as_str())
            .collect();
        for entry in &snapshot.entries {
            cancellation::check(cancel)?;
            if !traversal
                .intersects(entry.path.as_str())
                .map_err(|error| invalid(error.to_string()))?
                || protected_ancestor(entry.path.as_str(), &protected)
            {
                return Err(invalid(
                    "transition observation is outside scope or below a protected frontier",
                ));
            }
            // A denial describes one unknown exact path, never an invisible
            // directory through which a visible child can be exposed.
            if denied.contains(entry.path.as_str()) || has_ancestor(entry.path.as_str(), &denied) {
                return Err(invalid("transition denied and visible paths overlap"));
            }
        }
        for path in &coverage.denied_paths {
            cancellation::check(cancel)?;
            let below_visible_leaf = path.path.as_str().match_indices('/').any(|(at, _)| {
                entries.get(&path.path.as_str()[..at]).is_some_and(|entry| {
                    !matches!(entry.target, Some(TreeEntryTarget::Tree { .. }))
                })
            });
            if !traversal
                .intersects(path.path.as_str())
                .map_err(|error| invalid(error.to_string()))?
                || has_ancestor(path.path.as_str(), &denied)
                || protected_ancestor(path.path.as_str(), &protected)
                || below_visible_leaf
            {
                return Err(invalid("transition denied path scope is ambiguous"));
            }
        }
        let covered = |path: &str| {
            entries.contains_key(path)
                || denied.contains(path)
                || protected_ancestor(path, &protected)
        };
        let old_traversal = source
            .selection
            .traversal(source.snapshot.path_semantics)
            .map_err(|error| invalid(error.to_string()))?;
        for old in &source.snapshot.entries {
            cancellation::check(cancel)?;
            if old_traversal
                .intersects(old.path.as_str())
                .map_err(|error| invalid(error.to_string()))?
                && !covered(old.path.as_str())
            {
                return Err(invalid(
                    "transition observation omitted old selected namespace",
                ));
            }
            if let Some(fresh) = entries.get(old.path.as_str()) {
                // One immutable Step/root cannot change an already known
                // target/mode. Read/write revocation is a separate fact.
                if fresh.target != old.target || fresh.executable != old.executable {
                    return Err(invalid(
                        "transition observation changed an immutable source target",
                    ));
                }
            }
        }
        if let ProjectionSelection::Selected { paths, subtrees } = &selected {
            if paths
                .iter()
                .chain(subtrees)
                .any(|path| !covered(path.as_str()))
            {
                return Err(invalid(
                    "transition observation omitted a selected scope anchor",
                ));
            }
        }
        require_authority_data(
            &authority,
            source.snapshot,
            &snapshot,
            source.selection,
            &entries,
            &denied,
        )?;
        codec::preflight(&authority, 8192)?;
        let authority_bytes = layrs_core::scm::canonical_json(&authority)
            .map_err(|error| invalid(error.to_string()))?;
        let authority_data_digest = codec::hash(
            "layrs-provider-transition-authority-data-v1",
            &authority_bytes,
        );
        let mut denied_hash = blake3::Hasher::new();
        denied_hash.update(b"layrs-provider-transition-denied-paths-data-v1\0");
        denied_hash.update(&(coverage.denied_paths.len() as u64).to_be_bytes());
        for path in &coverage.denied_paths {
            cancellation::check(cancel)?;
            let bytes = layrs_core::scm::canonical_json(&path.path)
                .map_err(|error| invalid(error.to_string()))?;
            denied_hash.update(&(bytes.len() as u64).to_be_bytes());
            denied_hash.update(&bytes);
        }
        descriptors.sort_by(|a, b| a.path.cmp(&b.path));
        if descriptors.windows(2).any(|p| p[0].path == p[1].path) {
            return Err(invalid("transition descriptors repeat a path"));
        }
        let mut expected = snapshot.entries.iter().filter(|e| {
            selected.includes(&e.path)
                && e.state == EntryState::Unavailable
                && matches!(e.target, Some(TreeEntryTarget::File { .. }))
        });
        let mut rows = Vec::with_capacity(descriptors.len());
        let mut descriptor_bytes = 0u64;
        let mut descriptor_hash = blake3::Hasher::new();
        descriptor_hash.update(b"layrs-provider-transition-observation-descriptors-v1\0");
        descriptor_hash.update(&(descriptors.len() as u64).to_be_bytes());
        for input in descriptors {
            cancellation::check(cancel)?;
            let entry = expected
                .next()
                .ok_or_else(|| invalid("transition has an extra or denied descriptor"))?;
            let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
                unreachable!()
            };
            if input.path != entry.path || input.manifest.file_id() != file_id {
                return Err(invalid(
                    "transition descriptor differs from exact observed target",
                ));
            }
            let row = DescriptorRecord {
                schema_version: "layrs.projection-provider-descriptor.v1".into(),
                path: input.path,
                file_id: file_id.clone(),
                bytes: input.manifest.byte_length(),
                executable: entry.executable,
                manifest_id: input.manifest.id().clone(),
                manifest: input.manifest,
            };
            let payload = row.encode()?;
            descriptor_bytes = descriptor_bytes
                .checked_add(payload.len() as u64)
                .filter(|n| *n <= super::PROVIDER_CATALOG_AGGREGATE_MAX_BYTES)
                .ok_or_else(|| invalid("transition descriptor aggregate exceeds bound"))?;
            descriptor_hash.update(&(payload.len() as u64).to_be_bytes());
            descriptor_hash.update(&payload);
            rows.push(row);
        }
        if expected.next().is_some() {
            return Err(invalid("transition readable file descriptor is missing"));
        }
        let snapshot_digest =
            database::digest(generation, Some(&version.snapshot_digest), &snapshot_bytes);
        let selection_digest =
            database::digest(generation, Some(&snapshot_digest), &selection_bytes);
        cancellation::check(cancel)?;
        drop(entries);
        drop(protected);
        drop(denied);
        Ok(Self {
            binding: ObservationBindingData {
                source_prefix_id: codec::hash(
                    "layrs-projection-provider-import-prefix-v1",
                    &prefix_bytes,
                ),
                source_identity: identity.clone(),
                destination_identity,
                source_generation: version.generation,
                source_snapshot_digest: version.snapshot_digest.clone(),
                source_selection_digest: version.selection_digest.clone(),
                source_capture_count: version.capture_count,
                source_capture_head: version.capture_head.clone(),
                generation,
                snapshot_digest,
                selection_digest,
                descriptors_digest: format!("blake3:{}", descriptor_hash.finalize()),
                descriptor_bytes,
                denied_path_count: coverage.denied_paths.len() as u64,
                denied_paths_digest: format!("blake3:{}", denied_hash.finalize()),
                authority_data_digest,
            },
            snapshot,
            selection: selected,
            descriptors: rows,
            coverage,
            authority,
        })
    }

    pub(super) fn binding(&self) -> &ObservationBindingData {
        &self.binding
    }
    pub(super) fn snapshot(&self) -> &ProjectionSnapshot {
        &self.snapshot
    }
    pub(super) fn selection(&self) -> &ProjectionSelection {
        &self.selection
    }
    pub(super) fn descriptors(&self) -> &[DescriptorRecord] {
        &self.descriptors
    }
    pub(super) fn denied_paths(&self) -> &[DeniedPathData] {
        &self.coverage.denied_paths
    }
    pub(super) fn authority_data(&self) -> &AuthorityObservationData {
        &self.authority
    }
}

fn has_ancestor(path: &str, paths: &BTreeSet<&str>) -> bool {
    path.match_indices('/')
        .any(|(at, _)| paths.contains(&path[..at]))
}
fn protected_ancestor(path: &str, frontiers: &BTreeSet<&str>) -> bool {
    has_ancestor(path, frontiers)
}

fn require_authority_data(
    mode: &AuthorityObservationData,
    old: &ProjectionSnapshot,
    fresh: &ProjectionSnapshot,
    old_selection: &ProjectionSelection,
    entries: &BTreeMap<&str, &layrs_core::projection::ProjectionEntry>,
    denied: &BTreeSet<&str>,
) -> Result<()> {
    let (policy, epoch) = match mode {
        AuthorityObservationData::PreservedEpoch {
            policy_set_id,
            authorization_epoch,
        }
        | AuthorityObservationData::ReobservedExpandedScope {
            policy_set_id,
            authorization_epoch,
        } => (policy_set_id, *authorization_epoch),
    };
    if policy != &fresh.observed_policy_set_id
        || epoch == 0
        || epoch != fresh.observed_authorization_epoch
    {
        return Err(invalid(
            "transition declared authority does not match observation DATA",
        ));
    }
    if matches!(mode, AuthorityObservationData::PreservedEpoch { .. }) {
        if policy != &old.observed_policy_set_id || epoch != old.observed_authorization_epoch {
            return Err(invalid("transition preserved epoch DATA changed"));
        }
        let traversal = old_selection
            .traversal(old.path_semantics)
            .map_err(|error| invalid(error.to_string()))?;
        for prior in &old.entries {
            if !traversal
                .intersects(prior.path.as_str())
                .map_err(|error| invalid(error.to_string()))?
            {
                continue;
            }
            let next = entries.get(prior.path.as_str()).ok_or_else(|| {
                invalid("transition preserved epoch omitted original access DATA")
            })?;
            if denied.contains(prior.path.as_str())
                || prior.writable != next.writable
                || (prior.state == EntryState::Reserved) != (next.state == EntryState::Reserved)
            {
                return Err(invalid(
                    "transition preserved epoch changed original access DATA",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
