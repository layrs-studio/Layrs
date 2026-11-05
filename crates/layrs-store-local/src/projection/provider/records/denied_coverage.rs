//! Immutable negative-observation DATA, intended for projection_observations.
//! No SQL writer, remote capability, positive absence or acceptance grant is
//! created here. Required versioned plan/receipt links and scoped SQL readers
//! must authenticate this entire body before a live transition can use it.
use super::{
    codec,
    import::{ImportSource, ImportVersion, ProviderImportPrefixRecord},
    invalid, Record, Result,
};
use crate::projection::{
    format,
    provider::{
        cancellation,
        transition_observation::{AuthorityObservationData, TransitionObservationData},
    },
    ProjectionIdentity,
};
use layrs_core::{
    projection::{PathSemantics, ProjectionSnapshot, ReservedPaths},
    scm::{ObjectId, RepoPath, StepId, TreeId},
};
use layrs_vfs::Cancellation;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const SCHEMA: &str = "layrs.projection-transition-denied-coverage.v1";
const DOMAIN: &str = "layrs-projection-transition-denied-coverage-v1";
const ROWS_DOMAIN: &[u8] = b"layrs-provider-transition-denied-paths-data-v1\0";
const MAX_PATHS: usize = 100_000;
const MAX_PATH_BYTES: u64 = 8 * 1024 * 1024;
const MAX_INDEX_UPPER: u64 = 256 * 1024 * 1024;

#[path = "denied_coverage/binding.rs"]
mod binding;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::projection) enum DeniedAuthorityData {
    PreservedEpoch {
        policy_set_id: ObjectId,
        authorization_epoch: u64,
    },
    ReobservedExpandedScope {
        policy_set_id: ObjectId,
        authorization_epoch: u64,
    },
}
impl DeniedAuthorityData {
    fn from_observation(value: &AuthorityObservationData) -> Self {
        match value {
            AuthorityObservationData::PreservedEpoch {
                policy_set_id,
                authorization_epoch,
            } => Self::PreservedEpoch {
                policy_set_id: policy_set_id.clone(),
                authorization_epoch: *authorization_epoch,
            },
            AuthorityObservationData::ReobservedExpandedScope {
                policy_set_id,
                authorization_epoch,
            } => Self::ReobservedExpandedScope {
                policy_set_id: policy_set_id.clone(),
                authorization_epoch: *authorization_epoch,
            },
        }
    }
    fn epoch(&self) -> u64 {
        match self {
            Self::PreservedEpoch {
                authorization_epoch,
                ..
            }
            | Self::ReobservedExpandedScope {
                authorization_epoch,
                ..
            } => *authorization_epoch,
        }
    }
    fn digest(&self) -> Result<String> {
        Ok(codec::hash(
            "layrs-provider-transition-authority-data-v1",
            &codec::encode(self, 8192)?,
        ))
    }
}

/// A compact required link for FUTURE versioned plans and receipts. A link
/// without a locally authenticated matching body never proves coverage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct DeniedCoverageRef {
    pub id: String,
    pub count: u64,
    pub path_bytes: u64,
    pub rows_digest: String,
}
impl DeniedCoverageRef {
    pub fn validate(&self) -> Result<()> {
        codec::digest_token(&self.id)?;
        codec::digest_token(&self.rows_digest)?;
        if self.count > MAX_PATHS as u64
            || self.path_bytes > MAX_PATH_BYTES
            || self.count == 0 && self.path_bytes != 0
            || self.count > self.path_bytes
        {
            return Err(invalid("denied coverage reference bounds differ"));
        }
        Ok(())
    }
}

/// The final plan digest is deliberately absent: document -> plan -> receipt
/// is acyclic. All paths are exact negative observations, not subtree ACLs.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct TransitionDeniedCoverageRecord {
    pub schema_version: String,
    pub transaction_id: String,
    pub source_prefix_id: String,
    pub source_identity: ProjectionIdentity,
    pub destination_identity: ProjectionIdentity,
    pub source_version: ImportVersion,
    pub step_id: StepId,
    pub root_tree_id: TreeId,
    pub path_semantics: PathSemantics,
    pub authority: DeniedAuthorityData,
    pub authority_data_digest: String,
    pub observation_generation: u64,
    pub observation_snapshot_digest: String,
    pub observation_selection_digest: String,
    pub denied_path_count: u64,
    pub path_bytes: u64,
    pub rows_digest: String,
    #[serde(deserialize_with = "bounded_paths")]
    pub denied_paths: Vec<RepoPath>,
}

/// Immutable DATA derived from an actual complete normalized observation.
/// Canonical decoding produces only a raw record, never this writer input.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::projection::provider) struct DerivedDeniedCoverage {
    record: TransitionDeniedCoverageRecord,
}

impl DerivedDeniedCoverage {
    pub(in crate::projection::provider) fn from_observation(
        prefix: &ProviderImportPrefixRecord,
        transaction_id: &str,
        observed: &TransitionObservationData,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        Ok(Self {
            record: TransitionDeniedCoverageRecord::from_observation(
                prefix,
                transaction_id,
                observed,
                cancel,
            )?,
        })
    }

    pub(in crate::projection::provider) fn record(&self) -> &TransitionDeniedCoverageRecord {
        &self.record
    }
}

fn bounded_paths<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<RepoPath>, D::Error> {
    struct Paths;
    impl<'de> serde::de::Visitor<'de> for Paths {
        type Value = Vec<RepoPath>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("at most 100000 bounded exact paths totaling at most 8MiB")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            use serde::de::Error;
            if sequence.size_hint().is_some_and(|count| count > MAX_PATHS) {
                return Err(A::Error::custom("denied path count exceeds bound"));
            }
            let mut result = Vec::new();
            let mut bytes = 0u64;
            while let Some(path) = sequence.next_element::<RepoPath>()? {
                let text = path.as_str();
                if result.len() == MAX_PATHS
                    || text.is_empty()
                    || text.len() > codec::PATH_MAX
                    || text.split('/').count() > 128
                {
                    return Err(A::Error::custom("denied path/count exceeds bound"));
                }
                bytes = bytes
                    .checked_add(text.len() as u64)
                    .filter(|n| *n <= MAX_PATH_BYTES)
                    .ok_or_else(|| A::Error::custom("denied raw path bytes exceed bound"))?;
                result.push(path);
            }
            Ok(result)
        }
    }
    deserializer.deserialize_seq(Paths)
}

impl Record for TransitionDeniedCoverageRecord {
    // Existing projection_observations payload bound; no schema change.
    const MAX_BYTES: usize = 64 * 1024 * 1024;

    fn validate_fields(&self) -> Result<()> {
        codec::schema(&self.schema_version, SCHEMA)?;
        codec::transaction(&self.transaction_id)?;
        codec::digest_token(&self.source_prefix_id)?;
        format::validate_identity(&self.source_identity)?;
        format::validate_identity(&self.destination_identity)?;
        let source = &self.source_identity;
        let destination = &self.destination_identity;
        if source.worktree_id == destination.worktree_id
            || source.store_id != destination.store_id
            || source.space_id != destination.space_id
            || source.origin != destination.origin
            || source.principal_id != destination.principal_id
        {
            return Err(invalid(
                "denied coverage source/destination identity differs",
            ));
        }
        let version = &self.source_version;
        for value in [
            &version.snapshot_digest,
            &version.selection_digest,
            &version.effective_image_digest,
            &self.observation_snapshot_digest,
            &self.observation_selection_digest,
        ] {
            codec::raw_digest(value)?;
        }
        if version.generation > i64::MAX as u64
            || version.capture_count > 1_000_000
            || version.capture_head.is_some() != (version.capture_count > 0)
            || self.observation_generation <= version.generation
            || self.observation_generation > i64::MAX as u64
            || self.authority.epoch() == 0
        {
            return Err(invalid(
                "denied coverage generation/capture/authority invalid",
            ));
        }
        if let Some(head) = &version.capture_head {
            codec::digest_token(head)?;
        }
        codec::digest_token(&self.authority_data_digest)?;
        if self.authority_data_digest != self.authority.digest()? {
            return Err(invalid("denied coverage authority digest differs"));
        }
        let (path_bytes, rows_digest) =
            validate_paths(&self.denied_paths, self.path_semantics, None)?;
        if self.denied_path_count != self.denied_paths.len() as u64
            || self.path_bytes != path_bytes
            || self.rows_digest != rows_digest
        {
            return Err(invalid("denied coverage path commitments differ"));
        }
        Ok(())
    }
}

impl TransitionDeniedCoverageRecord {
    /// Derive only from the bounded normalizer's complete negative inventory.
    /// This checks DATA correspondence; callers still owe actual remote/source
    /// authentication, durable storage and required versioned protocol links.
    pub(in crate::projection::provider) fn from_observation(
        prefix: &ProviderImportPrefixRecord,
        transaction_id: &str,
        observed: &TransitionObservationData,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        cancellation::check(cancel)?;
        codec::transaction(transaction_id)?;
        let (identity, version) = match &prefix.source {
            ImportSource::Ordinary {
                identity, version, ..
            }
            | ImportSource::Provider {
                identity, version, ..
            } => (identity, version),
        };
        let binding = observed.binding();
        if prefix.digest()? != binding.source_prefix_id
            || identity != &binding.source_identity
            || prefix.destination_identity != binding.destination_identity
            || version.generation != binding.source_generation
            || version.snapshot_digest != binding.source_snapshot_digest
            || version.selection_digest != binding.source_selection_digest
            || version.capture_count != binding.source_capture_count
            || version.capture_head != binding.source_capture_head
        {
            return Err(invalid("denied coverage normalized source binding differs"));
        }
        // A denial is not a second 100k/8MiB namespace allowance. This check
        // precedes cloning even though the normalizer already bounded inputs.
        require_shared_namespace(
            observed.snapshot(),
            observed.denied_paths().len(),
            observed.denied_paths().iter().map(|row| &row.path),
            cancel,
        )?;
        let denied_paths: Vec<_> = observed
            .denied_paths()
            .iter()
            .map(|row| row.path.clone())
            .collect();
        let (path_bytes, rows_digest) =
            validate_paths(&denied_paths, observed.snapshot().path_semantics, cancel)?;
        if binding.denied_path_count != denied_paths.len() as u64
            || binding.denied_paths_digest != rows_digest
        {
            return Err(invalid("denied coverage normalized rows differ"));
        }
        let result = Self {
            schema_version: SCHEMA.into(),
            transaction_id: transaction_id.into(),
            source_prefix_id: binding.source_prefix_id.clone(),
            source_identity: identity.clone(),
            destination_identity: prefix.destination_identity.clone(),
            source_version: version.clone(),
            step_id: observed.snapshot().base_step_id.clone(),
            root_tree_id: observed.snapshot().base_root_tree_id.clone(),
            path_semantics: observed.snapshot().path_semantics,
            authority: DeniedAuthorityData::from_observation(observed.authority_data()),
            authority_data_digest: binding.authority_data_digest.clone(),
            observation_generation: binding.generation,
            observation_snapshot_digest: binding.snapshot_digest.clone(),
            observation_selection_digest: binding.selection_digest.clone(),
            denied_path_count: denied_paths.len() as u64,
            path_bytes,
            rows_digest,
            denied_paths,
        };
        codec::preflight(&result, Self::MAX_BYTES)?;
        result.validate_fields()?;
        cancellation::check(cancel)?;
        Ok(result)
    }

    pub fn digest(&self) -> Result<String> {
        Ok(codec::hash(DOMAIN, &self.encode()?))
    }

    /// Exact logical SQL values for this one immutable document row: the
    /// canonical BLOB plus its 71-byte digest key. Path bytes and counts are
    /// not a substitute for the encoded body size. No page geometry is claimed.
    pub(in crate::projection) fn normalized_column_bytes(&self) -> Result<u64> {
        (self.encode()?.len() as u64)
            .checked_add(71)
            .ok_or_else(|| invalid("denied coverage SQL column bytes overflow"))
    }

    pub fn reference(&self) -> Result<DeniedCoverageRef> {
        let result = DeniedCoverageRef {
            id: self.digest()?,
            count: self.denied_path_count,
            path_bytes: self.path_bytes,
            rows_digest: self.rows_digest.clone(),
        };
        result.validate()?;
        Ok(result)
    }

    /// Validate actual supplied canonical bytes and their SQL key together.
    /// Does not authenticate a connection, a plan or a source owner.
    pub fn read_body(id: &str, payload: &[u8], expected: &DeniedCoverageRef) -> Result<Self> {
        expected.validate()?;
        if id != expected.id {
            return Err(invalid("denied coverage SQL key differs from reference"));
        }
        let value = Self::decode(payload)?;
        if value.reference()? != *expected {
            return Err(invalid(
                "denied coverage actual body differs from reference",
            ));
        }
        Ok(value)
    }
}

fn require_shared_namespace<'a>(
    snapshot: &'a ProjectionSnapshot,
    denied_count: usize,
    denied: impl Iterator<Item = &'a RepoPath>,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    if snapshot
        .entries
        .len()
        .checked_add(denied_count)
        .filter(|n| *n <= MAX_PATHS)
        .is_none()
    {
        return Err(invalid(
            "denied coverage combined namespace count exceeds bound",
        ));
    }
    let mut total = 0u64;
    for path in snapshot
        .entries
        .iter()
        .map(|entry| &entry.path)
        .chain(denied)
    {
        cancellation::check(cancel)?;
        total = total
            .checked_add(path.as_str().len() as u64)
            .filter(|n| *n <= MAX_PATH_BYTES)
            .ok_or_else(|| invalid("denied coverage combined namespace bytes exceed bound"))?;
    }
    Ok(())
}

fn validate_paths(
    paths: &[RepoPath],
    semantics: PathSemantics,
    cancel: Option<&Cancellation>,
) -> Result<(u64, String)> {
    if paths.len() > MAX_PATHS {
        return Err(invalid("denied coverage path count exceeds bound"));
    }
    let mut bytes = 0u64;
    let mut index_upper = 0u64;
    let mut previous: Option<&str> = None;
    for path in paths {
        cancellation::check(cancel)?;
        let path = path.as_str();
        let depth = path.split('/').count() as u64;
        if path.is_empty()
            || path.len() > codec::PATH_MAX
            || depth > 128
            || previous.is_some_and(|old| old >= path)
        {
            return Err(invalid(
                "denied coverage paths are invalid or not strictly sorted",
            ));
        }
        previous = Some(path);
        bytes = bytes
            .checked_add(path.len() as u64)
            .filter(|n| *n <= MAX_PATH_BYTES)
            .ok_or_else(|| invalid("denied coverage raw path bytes exceed bound"))?;
        let upper = (path.len() as u64)
            .checked_mul(6)
            .and_then(|n| n.checked_add(128))
            .and_then(|n| n.checked_mul(depth))
            .ok_or_else(|| invalid("denied coverage index overflow"))?;
        index_upper = index_upper
            .checked_add(upper)
            .filter(|n| *n <= MAX_INDEX_UPPER)
            .ok_or_else(|| invalid("denied coverage path index exceeds bound"))?;
    }
    // All allocation bounds above precede canonical/alias maps.
    ReservedPaths::new(paths.iter().map(|p| p.as_str()), semantics).map_err(invalid)?;
    let key = |path: &str| match semantics {
        PathSemantics::CaseSensitive => path.to_owned(),
        PathSemantics::PortableWindows => path.to_uppercase(),
    };
    let mut aliases = BTreeMap::<String, &str>::new();
    let mut full = BTreeSet::new();
    let mut hash = blake3::Hasher::new();
    hash.update(ROWS_DOMAIN);
    hash.update(&(paths.len() as u64).to_be_bytes());
    for path in paths {
        cancellation::check(cancel)?;
        let text = path.as_str();
        for (at, _) in text
            .match_indices('/')
            .chain(std::iter::once((text.len(), "/")))
        {
            let prefix = &text[..at];
            if aliases
                .insert(key(prefix), prefix)
                .is_some_and(|old| old != prefix)
            {
                return Err(invalid("denied coverage paths contain a namespace alias"));
            }
        }
        if !full.insert(key(text)) {
            return Err(invalid("denied coverage exact paths alias"));
        }
        let row = layrs_core::scm::canonical_json(path).map_err(invalid)?;
        hash.update(&(row.len() as u64).to_be_bytes());
        hash.update(&row);
    }
    for path in &full {
        cancellation::check(cancel)?;
        if path
            .match_indices('/')
            .any(|(at, _)| full.contains(&path[..at]))
        {
            return Err(invalid(
                "denied coverage exact paths contain a denied ancestor",
            ));
        }
    }
    Ok((bytes, format!("blake3:{}", hash.finalize())))
}

#[cfg(test)]
#[path = "denied_coverage/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "denied_coverage/test_data.rs"]
pub(in crate::projection::provider) mod test_data;
