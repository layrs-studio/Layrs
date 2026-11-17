use super::types::ensure_strictly_sorted;
use super::{
    CanonicalObject, EntityRecord, RequestDigest, Result, SpaceObjectAdmission, StoreError,
    TransactionId,
};
use layrs_core::scm::{
    Operation, OperationId, OperationKind, RepositoryViewId, SpaceId, WorktreeId, canonical_json,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryViewCas {
    space_id: SpaceId,
    expected_generation: u64,
    expected_view_id: Option<RepositoryViewId>,
    new_view_id: RepositoryViewId,
}

impl RepositoryViewCas {
    pub fn new(
        space_id: SpaceId,
        expected_generation: u64,
        expected_view_id: Option<RepositoryViewId>,
        new_view_id: RepositoryViewId,
    ) -> Result<Self> {
        expected_generation
            .checked_add(1)
            .ok_or_else(|| StoreError::InvalidCas {
                target: format!("repository_view:{space_id}"),
                reason: "generation would overflow".into(),
            })?;
        Ok(Self {
            space_id,
            expected_generation,
            expected_view_id,
            new_view_id,
        })
    }

    pub fn space_id(&self) -> &SpaceId {
        &self.space_id
    }

    pub fn expected_generation(&self) -> u64 {
        self.expected_generation
    }

    pub fn expected_view_id(&self) -> Option<&RepositoryViewId> {
        self.expected_view_id.as_ref()
    }

    pub fn new_view_id(&self) -> &RepositoryViewId {
        &self.new_view_id
    }

    pub fn new_generation(&self) -> u64 {
        self.expected_generation + 1
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationHeadsCas {
    space_id: SpaceId,
    expected_head_ids: Vec<OperationId>,
    new_operation_id: OperationId,
    new_head_ids: Vec<OperationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree_registration_source: Option<WorktreeId>,
}

impl OperationHeadsCas {
    pub fn new(
        space_id: SpaceId,
        expected_head_ids: Vec<OperationId>,
        new_operation_id: OperationId,
        new_head_ids: Vec<OperationId>,
    ) -> Result<Self> {
        ensure_strictly_sorted("expectedOperationHeadIds", &expected_head_ids)?;
        ensure_strictly_sorted("newOperationHeadIds", &new_head_ids)?;
        if new_head_ids.is_empty() {
            return Err(StoreError::InvalidCas {
                target: format!("operation_heads:{space_id}"),
                reason: "the new Operation head set cannot be empty".into(),
            });
        }
        Ok(Self {
            space_id,
            expected_head_ids,
            new_operation_id,
            new_head_ids,
            worktree_registration_source: None,
        })
    }

    pub fn space_id(&self) -> &SpaceId {
        &self.space_id
    }

    pub fn expected_head_ids(&self) -> &[OperationId] {
        &self.expected_head_ids
    }

    pub fn new_head_ids(&self) -> &[OperationId] {
        &self.new_head_ids
    }

    pub fn new_operation_id(&self) -> &OperationId {
        &self.new_operation_id
    }

    /// Registration may fork a retained Operation of this durable source
    /// Worktree. The backend must validate its own ancestry and the new Stale
    /// Worktree record; this hint alone grants no publication authority.
    pub fn with_worktree_registration_source(mut self, source: WorktreeId) -> Self {
        self.worktree_registration_source = Some(source);
        self
    }

    pub fn worktree_registration_source(&self) -> Option<&WorktreeId> {
        self.worktree_registration_source.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeActiveOperationCas {
    worktree_id: WorktreeId,
    expected_operation_id: OperationId,
    new_operation_id: OperationId,
}

impl WorktreeActiveOperationCas {
    pub fn new(
        worktree_id: WorktreeId,
        expected_operation_id: OperationId,
        new_operation_id: OperationId,
    ) -> Result<Self> {
        if expected_operation_id == new_operation_id {
            return Err(StoreError::InvalidCas {
                target: format!("worktree_active_operation:{worktree_id}"),
                reason: "expected and new Operation IDs must differ".into(),
            });
        }
        Ok(Self {
            worktree_id,
            expected_operation_id,
            new_operation_id,
        })
    }

    pub fn worktree_id(&self) -> &WorktreeId {
        &self.worktree_id
    }

    pub fn expected_operation_id(&self) -> &OperationId {
        &self.expected_operation_id
    }

    pub fn new_operation_id(&self) -> &OperationId {
        &self.new_operation_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum StoreMutation {
    RepositoryView(RepositoryViewCas),
    OperationHeads(OperationHeadsCas),
    WorktreeActiveOperation(WorktreeActiveOperationCas),
}

impl StoreMutation {
    fn target_key(&self) -> String {
        match self {
            Self::RepositoryView(value) => format!("repository_view:{}", value.space_id()),
            Self::OperationHeads(value) => format!("operation_heads:{}", value.space_id()),
            Self::WorktreeActiveOperation(value) => {
                format!("worktree_active_operation:{}", value.worktree_id())
            }
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchPayload<'a> {
    schema_version: &'static str,
    canonical_objects: &'a [CanonicalObject],
    entity_records: &'a [EntityRecord],
    admissions: &'a [SpaceObjectAdmission],
    mutations: &'a [StoreMutation],
}

/// Immutable, locally validated unit of atomic repository work.
///
/// The constructor canonicalizes collection order. It validates only facts
/// available without repository state. Existence, graph closure, admission,
/// and CAS preconditions remain the backend's responsibility inside the same
/// atomic transaction as publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteBatch {
    transaction_id: TransactionId,
    request_digest: RequestDigest,
    canonical_objects: Vec<CanonicalObject>,
    entity_records: Vec<EntityRecord>,
    admissions: Vec<SpaceObjectAdmission>,
    mutations: Vec<StoreMutation>,
}

impl WriteBatch {
    pub fn new(
        transaction_id: TransactionId,
        mut canonical_objects: Vec<CanonicalObject>,
        mut entity_records: Vec<EntityRecord>,
        mut admissions: Vec<SpaceObjectAdmission>,
        mut mutations: Vec<StoreMutation>,
    ) -> Result<Self> {
        if canonical_objects.is_empty()
            && entity_records.is_empty()
            && admissions.is_empty()
            && mutations.is_empty()
        {
            return Err(StoreError::EmptyBatch);
        }

        canonical_objects.sort_by_key(|value| {
            let id = value.object_id();
            (id.kind(), id.as_str().to_owned())
        });
        entity_records.sort_by_key(|value| (value.kind(), value.id_str().to_owned()));
        admissions.sort_by_key(|value| {
            (
                value.space_id().as_str().to_owned(),
                value.object_id().kind(),
                value.object_id().as_str().to_owned(),
            )
        });
        mutations.sort_by_key(StoreMutation::target_key);
        let request_digest =
            checked_digest(&canonical_objects, &entity_records, &admissions, &mutations)?;

        Ok(Self {
            transaction_id,
            request_digest,
            canonical_objects,
            entity_records,
            admissions,
            mutations,
        })
    }

    /// Whether this batch is exactly what `new` builds from its own
    /// collections: each in canonical order without duplicates, Operation
    /// publications coupled, and its digest theirs. Checked in place, without
    /// copying the objects (a backend's preflight).
    pub fn verify(&self) -> Result<()> {
        if self.canonical_objects.is_empty()
            && self.entity_records.is_empty()
            && self.admissions.is_empty()
            && self.mutations.is_empty()
        {
            return Err(StoreError::EmptyBatch);
        }
        let digest = checked_digest(
            &self.canonical_objects,
            &self.entity_records,
            &self.admissions,
            &self.mutations,
        )?;
        if digest != self.request_digest {
            return Err(StoreError::Corruption {
                message: "WriteBatch digest changed after construction".into(),
            });
        }
        Ok(())
    }

    pub fn transaction_id(&self) -> &TransactionId {
        &self.transaction_id
    }

    pub fn request_digest(&self) -> RequestDigest {
        self.request_digest
    }

    pub fn canonical_objects(&self) -> &[CanonicalObject] {
        &self.canonical_objects
    }

    pub fn entity_records(&self) -> &[EntityRecord] {
        &self.entity_records
    }

    pub fn admissions(&self) -> &[SpaceObjectAdmission] {
        &self.admissions
    }

    pub fn mutations(&self) -> &[StoreMutation] {
        &self.mutations
    }
}

/// The checks `WriteBatch::new` makes of its sorted collections (each
/// strictly in canonical order: an equal neighbour is a duplicate), then
/// their digest.
fn checked_digest(
    canonical_objects: &[CanonicalObject],
    entity_records: &[EntityRecord],
    admissions: &[SpaceObjectAdmission],
    mutations: &[StoreMutation],
) -> Result<RequestDigest> {
    use std::cmp::Ordering;
    let out_of_order = || StoreError::Corruption {
        message: "WriteBatch collections are not in canonical order".into(),
    };
    for pair in canonical_objects.windows(2) {
        let first = pair[0].object_id();
        let second = pair[1].object_id();
        match (first.kind(), first.as_str()).cmp(&(second.kind(), second.as_str())) {
            Ordering::Less => {}
            Ordering::Equal => {
                return Err(StoreError::DuplicateObject {
                    kind: first.kind(),
                    id: first.to_string(),
                });
            }
            Ordering::Greater => return Err(out_of_order()),
        }
    }
    for pair in entity_records.windows(2) {
        match (pair[0].kind(), pair[0].id_str()).cmp(&(pair[1].kind(), pair[1].id_str())) {
            Ordering::Less => {}
            Ordering::Equal => {
                return Err(StoreError::DuplicateEntity {
                    kind: pair[0].kind(),
                    id: pair[0].id_str().to_owned(),
                });
            }
            Ordering::Greater => return Err(out_of_order()),
        }
    }
    let admission_key = |value: &SpaceObjectAdmission| {
        let object = value.object_id();
        (
            value.space_id().as_str().to_owned(),
            object.kind(),
            object.as_str().to_owned(),
        )
    };
    for pair in admissions.windows(2) {
        match admission_key(&pair[0]).cmp(&admission_key(&pair[1])) {
            Ordering::Less => {}
            Ordering::Equal => {
                return Err(StoreError::DuplicateAdmission {
                    space_id: pair[0].space_id().to_string(),
                    object_id: pair[0].object_id().to_string(),
                });
            }
            Ordering::Greater => return Err(out_of_order()),
        }
    }
    for pair in mutations.windows(2) {
        let target = pair[1].target_key();
        match pair[0].target_key().cmp(&target) {
            Ordering::Less => {}
            Ordering::Equal => return Err(StoreError::DuplicateMutableTarget { target }),
            Ordering::Greater => return Err(out_of_order()),
        }
    }

    validate_operation_publications(entity_records, mutations)?;

    let payload = BatchPayload {
        schema_version: "layrs.store-write-batch.v1",
        canonical_objects,
        entity_records,
        admissions,
        mutations,
    };
    Ok(RequestDigest::compute(&batch_canonical_json(&payload)?))
}

/// `canonical_json(payload)`, byte for byte (keys in UTF-16 order:
/// admissions, canonicalObjects, entityRecords, mutations, schemaVersion),
/// each Tree written by its own canonical encoder instead of through a JSON
/// value per entry: a capture of a folder of 10,000 files carries a Tree of
/// that many entries (`batch_json_matches_the_general_encoder`).
fn batch_canonical_json(payload: &BatchPayload<'_>) -> Result<Vec<u8>> {
    let serialization = |error: layrs_core::scm::ScmError| StoreError::Serialization {
        message: error.to_string(),
    };
    let mut out = Vec::new();
    out.extend_from_slice(b"{\"admissions\":");
    out.extend(canonical_json(&payload.admissions).map_err(serialization)?);
    out.extend_from_slice(b",\"canonicalObjects\":[");
    for (index, object) in payload.canonical_objects.iter().enumerate() {
        if index != 0 {
            out.push(b',');
        }
        match object {
            CanonicalObject::Tree(tree) => {
                out.extend_from_slice(b"{\"kind\":\"tree\",\"value\":");
                out.extend(tree.canonical_json().map_err(serialization)?);
                out.push(b'}');
            }
            other => out.extend(canonical_json(other).map_err(serialization)?),
        }
    }
    out.extend_from_slice(b"],\"entityRecords\":");
    out.extend(canonical_json(&payload.entity_records).map_err(serialization)?);
    out.extend_from_slice(b",\"mutations\":");
    out.extend(canonical_json(&payload.mutations).map_err(serialization)?);
    out.extend_from_slice(b",\"schemaVersion\":");
    out.extend(canonical_json(&payload.schema_version).map_err(serialization)?);
    out.push(b'}');
    Ok(out)
}

fn validate_operation_publications(
    entity_records: &[EntityRecord],
    mutations: &[StoreMutation],
) -> Result<()> {
    let operations: BTreeMap<_, _> = entity_records
        .iter()
        .filter_map(|record| match record {
            EntityRecord::Operation(operation) => Some((operation.id(), operation)),
            _ => None,
        })
        .collect();

    let view_cas_by_space: BTreeMap<_, _> = mutations
        .iter()
        .filter_map(|mutation| match mutation {
            StoreMutation::RepositoryView(cas) => Some((cas.space_id(), cas)),
            _ => None,
        })
        .collect();
    let heads_cas_by_space: BTreeMap<_, _> = mutations
        .iter()
        .filter_map(|mutation| match mutation {
            StoreMutation::OperationHeads(cas) => Some((cas.space_id(), cas)),
            _ => None,
        })
        .collect();

    for cas in heads_cas_by_space.values() {
        let operation = operations
            .get(cas.new_operation_id())
            .copied()
            .ok_or_else(|| StoreError::OperationHeadsCasMissingOperation {
                space_id: cas.space_id().clone(),
                operation_id: cas.new_operation_id().clone(),
            })?;
        if operation.space_id() != cas.space_id() {
            return Err(StoreError::OperationHeadsCasSpaceMismatch {
                operation_id: operation.id().clone(),
                expected_space_id: cas.space_id().clone(),
                actual_space_id: operation.space_id().clone(),
            });
        }
        let view_cas = view_cas_by_space.get(cas.space_id()).ok_or_else(|| {
            StoreError::OperationPublicationMissingViewCas {
                space_id: cas.space_id().clone(),
            }
        })?;
        if view_cas.new_view_id() != operation.resulting_view_id() {
            return Err(StoreError::OperationPublicationViewMismatch {
                operation_id: operation.id().clone(),
                published_view_id: operation.resulting_view_id().to_string(),
                cas_view_id: view_cas.new_view_id().to_string(),
            });
        }
        validate_operation_head_transition(cas, operation, entity_records, mutations)?;
    }
    for space_id in view_cas_by_space.keys() {
        if !heads_cas_by_space.contains_key(space_id) {
            return Err(StoreError::RepositoryViewCasMissingOperationPublication {
                space_id: (*space_id).clone(),
            });
        }
    }
    Ok(())
}

fn validate_operation_head_transition(
    cas: &OperationHeadsCas,
    operation: &Operation,
    records: &[EntityRecord],
    mutations: &[StoreMutation],
) -> Result<()> {
    let invalid = |reason: &str| StoreError::InvalidOperationHeadsTransition {
        operation_id: operation.id().clone(),
        reason: reason.into(),
    };
    if cas.expected_head_ids().contains(operation.id()) {
        return Err(invalid("the new Operation is already an expected head"));
    }

    let expected: BTreeSet<_> = cas.expected_head_ids().iter().cloned().collect();
    let parents: BTreeSet<_> = operation.parent_operation_ids().iter().cloned().collect();
    let worktree_changes = mutations
        .iter()
        .filter_map(|mutation| match mutation {
            StoreMutation::WorktreeActiveOperation(change)
                if change.new_operation_id() == operation.id() =>
            {
                Some(change)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if worktree_changes
        .iter()
        .any(|change| !parents.contains(change.expected_operation_id()))
    {
        return Err(invalid(
            "every Worktree CAS must advance from a parent of the published Operation",
        ));
    }
    if operation.kind() != OperationKind::IntegrateOperations
        && !worktree_changes.is_empty()
        && (parents.len() != 1 || worktree_changes.len() != 1)
    {
        return Err(invalid(
            "normal Worktree publication must advance exactly one own parent binding",
        ));
    }
    let registration = cas.worktree_registration_source().is_some();
    if registration
        && (operation.kind() != OperationKind::Restore || parents.len() != 1 || !worktree_changes.is_empty()
            || records.iter().filter(|record| matches!(record, EntityRecord::Worktree(w)
                if w.active_operation_id() == operation.id() && w.space_id() == operation.space_id())).count() != 1)
    {
        return Err(invalid("Worktree registration requires one parent and one new Worktree bound to the published Restore"));
    }
    let derived: BTreeSet<_> = match operation.kind() {
        OperationKind::Initialize => {
            if !expected.is_empty() || !parents.is_empty() {
                return Err(invalid(
                    "Initialize requires empty expected heads and no parents",
                ));
            }
            BTreeSet::from([operation.id().clone()])
        }
        OperationKind::IntegrateOperations => {
            if parents.len() < 2 || parents != expected {
                return Err(invalid(
                    "IntegrateOperations parents must equal the complete expected head set and contain at least two heads",
                ));
            }
            BTreeSet::from([operation.id().clone()])
        }
        _ => {
            let bound_active = parents.len() == 1 && mutations.iter().any(|mutation| matches!(mutation,
                StoreMutation::WorktreeActiveOperation(change)
                if parents.contains(change.expected_operation_id()) && change.new_operation_id() == operation.id()));
            if parents.is_empty()
                || (!parents.is_subset(&expected) && !bound_active && !registration)
            {
                return Err(invalid(
                    "parents outside expected heads require a single explicitly bound Worktree parent",
                ));
            }
            expected
                .difference(&parents)
                .cloned()
                .chain([operation.id().clone()])
                .collect()
        }
    };
    let actual: BTreeSet<_> = cas.new_head_ids().iter().cloned().collect();
    if actual != derived {
        return Err(invalid(
            "new heads must exactly equal the transition derived from expected heads, parents, and the new Operation",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use layrs_core::scm::{
        FileId, RepoPathComponent, Symlink, Tree, TreeEntry, TreeEntryTarget, TreeId,
    };

    fn id(seed: &str) -> String {
        format!("blake3:{}", blake3::hash(seed.as_bytes()).to_hex())
    }

    #[test]
    fn batch_json_matches_the_general_encoder() {
        let names = ["a", "B", "caf\u{e9}", "quo\"te", "tab\tname", "\u{1f600}x"];
        let tree = |seed: &str| {
            let entries = names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    let id = id(&format!("{seed}{name}"));
                    let target = if index % 2 == 0 {
                        TreeEntryTarget::file(FileId::parse(id).unwrap())
                    } else {
                        TreeEntryTarget::tree(TreeId::parse(id).unwrap())
                    };
                    TreeEntry::new(
                        RepoPathComponent::parse(*name).unwrap(),
                        target,
                        index % 3 == 0,
                    )
                })
                .collect();
            CanonicalObject::Tree(Tree::new(entries).unwrap())
        };
        let objects = vec![
            tree("one"),
            CanonicalObject::Symlink(Symlink::new("target \"quoted\"").unwrap()),
            tree("two"),
            CanonicalObject::Tree(Tree::new(Vec::new()).unwrap()),
        ];
        let payload = BatchPayload {
            schema_version: "layrs.store-write-batch.v1",
            canonical_objects: &objects,
            entity_records: &[],
            admissions: &[],
            mutations: &[],
        };
        assert_eq!(
            batch_canonical_json(&payload).unwrap(),
            canonical_json(&payload).unwrap()
        );
        let empty = BatchPayload {
            canonical_objects: &[],
            ..payload
        };
        assert_eq!(
            batch_canonical_json(&empty).unwrap(),
            canonical_json(&empty).unwrap()
        );
    }
}
