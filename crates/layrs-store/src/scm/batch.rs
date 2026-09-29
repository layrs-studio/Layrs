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
        for pair in canonical_objects.windows(2) {
            let first = pair[0].object_id();
            let second = pair[1].object_id();
            if first == second {
                return Err(StoreError::DuplicateObject {
                    kind: first.kind(),
                    id: first.to_string(),
                });
            }
        }

        entity_records.sort_by_key(|value| (value.kind(), value.id_str().to_owned()));
        for pair in entity_records.windows(2) {
            if pair[0].kind() == pair[1].kind() && pair[0].id_str() == pair[1].id_str() {
                return Err(StoreError::DuplicateEntity {
                    kind: pair[0].kind(),
                    id: pair[0].id_str().to_owned(),
                });
            }
        }

        admissions.sort_by_key(|value| {
            (
                value.space_id().as_str().to_owned(),
                value.object_id().kind(),
                value.object_id().as_str().to_owned(),
            )
        });
        for pair in admissions.windows(2) {
            if pair[0] == pair[1] {
                return Err(StoreError::DuplicateAdmission {
                    space_id: pair[0].space_id().to_string(),
                    object_id: pair[0].object_id().to_string(),
                });
            }
        }

        mutations.sort_by_key(StoreMutation::target_key);
        let mut targets = BTreeSet::new();
        for mutation in &mutations {
            let target = mutation.target_key();
            if !targets.insert(target.clone()) {
                return Err(StoreError::DuplicateMutableTarget { target });
            }
        }

        validate_operation_publications(&entity_records, &mutations)?;

        let payload = BatchPayload {
            schema_version: "layrs.store-write-batch.v1",
            canonical_objects: &canonical_objects,
            entity_records: &entity_records,
            admissions: &admissions,
            mutations: &mutations,
        };
        let canonical = canonical_json(&payload).map_err(|error| StoreError::Serialization {
            message: error.to_string(),
        })?;
        let request_digest = RequestDigest::compute(&canonical);

        Ok(Self {
            transaction_id,
            request_digest,
            canonical_objects,
            entity_records,
            admissions,
            mutations,
        })
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
        validate_operation_head_transition(cas, operation)?;
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
            if parents.is_empty() || !parents.is_subset(&expected) {
                return Err(invalid(
                    "non-initialize parents must be a non-empty subset of expected heads",
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
