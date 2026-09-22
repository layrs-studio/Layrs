use super::*;
use layrs_core::scm::{ObjectId, StepId};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::{error::Error, fmt};
use url::Url;

pub type V3Result<T> = Result<T, V3ValidationError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V3ValidationError {
    pub code: &'static str,
    pub field: &'static str,
    pub message: String,
}
impl V3ValidationError {
    pub(super) fn new(code: &'static str, field: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            field,
            message: message.into(),
        }
    }
}
impl fmt::Display for V3ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}: {}", self.code, self.field, self.message)
    }
}
impl Error for V3ValidationError {}

pub trait ValidateV3 {
    fn validate_v3(&self) -> V3Result<()>;
}

impl ValidateV3 for ProductEnvelope {
    fn validate_v3(&self) -> V3Result<()> {
        self.verify_identity().map_err(|error| {
            V3ValidationError::new(
                "immutable_object_mismatch",
                "productObjectId",
                error.to_string(),
            )
        })?;
        if !self.typed_identity_matches() {
            return Err(V3ValidationError::new(
                "immutable_object_mismatch",
                "productId",
                "kind/logical ID does not match the typed record identity",
            ));
        }
        let (steps, products) = self.dependencies();
        sorted_unique(steps, "dependsOnStepIds")?;
        sorted_unique(products, "dependsOnProductIds")?;
        let (projected_steps, intrinsic_products, complete_products) =
            self.intrinsic_dependency_projection().map_err(|error| {
                V3ValidationError::new(
                    "immutable_object_mismatch",
                    "dependsOnStepIds",
                    error.to_string(),
                )
            })?;
        if steps != projected_steps {
            return Err(V3ValidationError::new(
                "immutable_object_mismatch",
                "dependsOnStepIds",
                "Step dependency projection differs from the typed record",
            ));
        }
        if intrinsic_products
            .iter()
            .any(|dependency| products.binary_search(dependency).is_err())
            || (complete_products && products != intrinsic_products)
        {
            return Err(V3ValidationError::new(
                "immutable_object_mismatch",
                "dependsOnProductIds",
                "Product dependency projection differs from the typed record",
            ));
        }
        if let Some(weave) = self.weave_record() {
            validate_weave(weave)?;
        }
        if let Some(decision) = self.decision_record() {
            sorted_unique(
                &decision.discarded_head_step_ids,
                "record.discardedHeadStepIds",
            )?;
            sorted_unique(&decision.chosen_head_step_ids, "record.chosenHeadStepIds")?;
            if decision
                .discarded_head_step_ids
                .iter()
                .any(|step| decision.chosen_head_step_ids.binary_search(step).is_ok())
            {
                return Err(V3ValidationError::new(
                    "invalid_decision",
                    "record.discardedHeadStepIds",
                    "discarded and chosen head sets must be disjoint",
                ));
            }
        }
        Ok(())
    }
}

impl ValidateV3 for ObjectDescriptor {
    fn validate_v3(&self) -> V3Result<()> {
        if self.kind == ObjectKind::Evidence && self.canonical_byte_len > MAX_EVIDENCE_BYTE_LEN {
            return Err(V3ValidationError::new(
                "limit_exceeded",
                "canonicalByteLen",
                format!("Evidence maximum is {MAX_EVIDENCE_BYTE_LEN}"),
            ));
        }
        Ok(())
    }
}

fn sorted_unique<T: Ord>(items: &[T], field: &'static str) -> V3Result<()> {
    if items.windows(2).any(|w| w[0] >= w[1]) {
        return Err(V3ValidationError::new(
            "non_canonical_set",
            field,
            "must be strictly sorted and duplicate-free",
        ));
    }
    Ok(())
}
fn max_len<T>(items: &[T], max: usize, field: &'static str) -> V3Result<()> {
    if items.len() > max {
        Err(V3ValidationError::new(
            "limit_exceeded",
            field,
            format!("maximum is {max}"),
        ))
    } else {
        Ok(())
    }
}
fn absolute_transfer_url(
    value: &str,
    required_path: &'static str,
    field: &'static str,
) -> V3Result<()> {
    let valid = Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.as_str() == value
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == required_path
            && url.query().is_none()
            && url.fragment().is_none()
    });
    if !valid {
        return Err(V3ValidationError::new(
            "invalid_uri",
            field,
            format!("transfer URL must be an HTTPS origin plus the generic path {required_path}"),
        ));
    }
    Ok(())
}

impl ValidateV3 for SyncTransactionRequest {
    fn validate_v3(&self) -> V3Result<()> {
        max_len(&self.steps, 10_000, "steps")?;
        max_len(&self.observations, 10_000, "observations")?;
        max_len(&self.products, 10_000, "products")?;
        if self.steps.is_empty()
            && self.observations.is_empty()
            && self.products.is_empty()
            && self.view_mutation.is_none()
            && self.policy_root_mutation.is_none()
        {
            return Err(V3ValidationError::new(
                "invalid_request",
                "transaction",
                "semantic transaction is empty",
            ));
        }
        let computed = self.computed_request_digest().map_err(|e| {
            V3ValidationError::new("canonicalization_failed", "requestDigest", e.to_string())
        })?;
        if computed != self.request_digest {
            return Err(V3ValidationError::new(
                "request_digest_mismatch",
                "requestDigest",
                format!("computed {computed}"),
            ));
        }

        let positions: BTreeMap<&StepId, usize> = self
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id(), i))
            .collect();
        if positions.len() != self.steps.len() {
            return Err(V3ValidationError::new(
                "duplicate_identity",
                "steps",
                "duplicate Step ID",
            ));
        }
        for (index, step) in self.steps.iter().enumerate() {
            for parent in step.parent_step_ids() {
                if positions.get(parent).is_some_and(|p| *p >= index) {
                    return Err(V3ValidationError::new(
                        "non_topological_steps",
                        "steps",
                        "parent must precede child",
                    ));
                }
            }
        }
        let mut observation_ids = BTreeSet::new();
        for observation in &self.observations {
            if !observation_ids.insert(observation.id()) {
                return Err(V3ValidationError::new(
                    "duplicate_identity",
                    "observations",
                    "duplicate Observation ID",
                ));
            }
        }
        let product_positions: BTreeMap<&str, usize> = self
            .products
            .iter()
            .enumerate()
            .map(|(i, p)| (p.logical_id(), i))
            .collect();
        if product_positions.len() != self.products.len() {
            return Err(V3ValidationError::new(
                "duplicate_identity",
                "products",
                "duplicate product logical identity",
            ));
        }
        let mut product_objects = BTreeSet::new();
        for (index, product) in self.products.iter().enumerate() {
            if !product_objects.insert(product.object_id()) {
                return Err(V3ValidationError::new(
                    "duplicate_identity",
                    "products",
                    "duplicate product object identity",
                ));
            }
            let (steps, products) = product.dependencies();
            product.validate_v3()?;
            if product.record_space_id() != &self.space_id {
                return Err(V3ValidationError::new(
                    "cross_space_reference",
                    "products.record.spaceId",
                    "product record belongs to another Space",
                ));
            }
            sorted_unique(steps, "products.dependsOnStepIds")?;
            sorted_unique(products, "products.dependsOnProductIds")?;
            for dependency in products {
                // A Policy revision references its previously admitted version by
                // exact object ID, while the projected logical ID stays the same.
                // This is not an in-batch self dependency. Admission must verify
                // the predecessor outside the batch, including its immutable version.
                let previous_policy_version = matches!(product,
                    ProductEnvelope::Policy { previous_product_object_id, record, .. }
                    if previous_product_object_id.as_ref().is_some_and(|id| id != product.object_id())
                        && dependency.as_str() == record.policy_id.as_str());
                if previous_policy_version {
                    continue;
                }
                if product_positions
                    .get(dependency.as_str())
                    .is_some_and(|p| *p >= index)
                {
                    return Err(V3ValidationError::new(
                        "non_topological_products",
                        "products",
                        "product dependency must precede dependent",
                    ));
                }
            }
        }
        if let Some(mutation) = &self.view_mutation {
            mutation.validate_v3()?;
            for intent in &mutation.layer_mutations {
                if let Some(decision_object_id) = &intent.decision_product_object_id {
                    if let Some(product) = self
                        .products
                        .iter()
                        .find(|product| product.object_id() == decision_object_id)
                    {
                        if product.decision_layer_id() != Some(&intent.layer_id) {
                            return Err(V3ValidationError::new(
                                "invalid_view_mutation",
                                "decisionProductObjectId",
                                "referenced object must be the exact Decision Product for this Layer",
                            ));
                        }
                        if let (Some(decision), ProposedRepositoryView::Inline { view }) =
                            (product.decision_record(), &mutation.proposed_view)
                        {
                            let proposed_heads = view
                                .layer_heads()
                                .iter()
                                .find(|heads| heads.layer_id() == &intent.layer_id)
                                .map_or(&[][..], |heads| heads.step_ids());
                            if decision.chosen_head_step_ids != proposed_heads {
                                return Err(V3ValidationError::new(
                                    "invalid_view_mutation",
                                    "decisionProductObjectId",
                                    "Decision chosen heads must equal the proposed View heads for the Layer",
                                ));
                            }
                        }
                    }
                }
            }
        }
        if let Some(mutation) = &self.policy_root_mutation {
            mutation.validate_v3()?;
        }
        Ok(())
    }
}

impl ValidateV3 for SyncTransactionError {
    fn validate_v3(&self) -> V3Result<()> {
        if self.http_status != 409 || self.receipt_created {
            return Err(V3ValidationError::new(
                "invalid_transaction_error",
                "httpStatus",
                "pre-semantic transaction errors are HTTP 409 and never create receipts",
            ));
        }
        if self.request_digest == self.expected_request_digest {
            return Err(V3ValidationError::new(
                "invalid_transaction_error",
                "expectedRequestDigest",
                "a digest mismatch requires two different digests",
            ));
        }
        match (self.code, self.existing_receipt_unchanged) {
            (TransactionErrorCode::IdempotencyDigestMismatch, Some(true))
            | (TransactionErrorCode::RequestDigestMismatch, None) => Ok(()),
            (TransactionErrorCode::IdempotencyDigestMismatch, _) => Err(V3ValidationError::new(
                "invalid_transaction_error",
                "existingReceiptUnchanged",
                "idempotency mismatch must affirm that the stored receipt remains unchanged",
            )),
            (TransactionErrorCode::RequestDigestMismatch, Some(_)) => Err(V3ValidationError::new(
                "invalid_transaction_error",
                "existingReceiptUnchanged",
                "request digest mismatch occurs before any receipt lookup or creation",
            )),
        }
    }
}

mod weave;
use weave::validate_weave;

impl ValidateV3 for RepositoryViewMutation {
    fn validate_v3(&self) -> V3Result<()> {
        if let ProposedRepositoryView::Inline { view } = &self.proposed_view {
            reject_wire_worktrees(view.worktrees(), "proposedView.view.worktrees")?;
        }
        if self.layer_mutations.len() > 1_000 {
            return Err(V3ValidationError::new(
                "invalid_view_mutation",
                "layerMutations",
                "requires at most 1000 intents",
            ));
        }
        if self.layer_mutations.is_empty()
            && !matches!(&self.proposed_view,
            ProposedRepositoryView::Inline { view } if view.id() != &self.expected_current_view_id)
        {
            return Err(V3ValidationError::new(
                "invalid_view_mutation",
                "proposedView",
                "reference-only mutation requires a changed inline View and exact View/generation CAS",
            ));
        }
        let mut layers = BTreeSet::new();
        for intent in &self.layer_mutations {
            if !layers.insert(&intent.layer_id) {
                return Err(V3ValidationError::new(
                    "duplicate_identity",
                    "layerMutations",
                    "duplicate Layer mutation",
                ));
            }
            match (intent.mode, &intent.decision_product_object_id) {
                (LayerMutationMode::Choose, None) => {
                    return Err(V3ValidationError::new(
                        "invalid_view_mutation",
                        "decisionProductObjectId",
                        "choose requires an exact Decision Product object",
                    ));
                }
                (LayerMutationMode::Advance | LayerMutationMode::Integrate, Some(_)) => {
                    return Err(V3ValidationError::new(
                        "invalid_view_mutation",
                        "decisionProductObjectId",
                        "only choose may name a Decision Product object",
                    ));
                }
                _ => {}
            }
        }
        sorted_unique(
            &self.expected_policy_version_object_ids,
            "expectedPolicyVersionObjectIds",
        )
    }
}
impl ValidateV3 for PolicyRootMutation {
    fn validate_v3(&self) -> V3Result<()> {
        if self.expected_authorization_epoch == 0 {
            return Err(V3ValidationError::new(
                "invalid_epoch",
                "expectedAuthorizationEpoch",
                "PolicyRoot compare-and-swap requires a positive expected epoch",
            ));
        }
        if let ProposedPolicySet::Inline { policy_set } = &self.proposed_policy_set {
            policy_set.validate_v3()?;
        }
        Ok(())
    }
}

impl ValidateV3 for PolicySet {
    fn validate_v3(&self) -> V3Result<()> {
        max_len(&self.policy_versions, 10_000, "policyVersions")?;
        let policy_ids: Vec<_> = self.policy_versions.iter().map(|p| &p.policy_id).collect();
        sorted_unique(&policy_ids, "policyVersions.policyId")?;
        for version in &self.policy_versions {
            sorted_unique(&version.dependency_object_ids, "dependencyObjectIds")?;
        }
        let computed = self.computed_id().map_err(|e| {
            V3ValidationError::new("canonicalization_failed", "policySetId", e.to_string())
        })?;
        if computed != self.policy_set_id {
            return Err(V3ValidationError::new(
                "identity_mismatch",
                "policySetId",
                format!("computed {computed}"),
            ));
        }
        Ok(())
    }
}
impl ValidateV3 for AuthorizationSnapshot {
    fn validate_v3(&self) -> V3Result<()> {
        if (self.schema_version == AuthorizationSnapshotSchemaVersion::V2)
            != self.request_binding.path_patch_request_digest.is_some()
        {
            return Err(V3ValidationError::new(
                "invalid_patch_binding",
                "requestBinding",
                "only snapshot v2 carries a mandatory path patch digest",
            ));
        }
        if self.schema_version == AuthorizationSnapshotSchemaVersion::V2 {
            let mut paths = std::collections::BTreeSet::new();
            let mut base = None;
            let mut valid = matches!(self.subject, AuthorizationSubject::Principal { .. })
                && !self.decisions.is_empty()
                && self.decisions.len() <= 1024;
            for decision in &self.decisions {
                valid &= decision.action == "transaction.apply";
                match &decision.scope {
                    AuthorizationScope::Path {
                        root: AuthorizationRoot::Step { step_id },
                        repo_path,
                    } => {
                        valid &= base.is_none_or(|previous| previous == step_id);
                        base = Some(step_id);
                        valid &= paths.insert(repo_path);
                        valid &= repo_path.as_str().len() <= 4096
                            && repo_path.as_str().split('/').count() <= 128;
                    }
                    _ => valid = false,
                }
            }
            if !valid {
                return Err(V3ValidationError::new(
                    "invalid_patch_decisions",
                    "decisions",
                    "patch snapshots require one principal and unique paths at one Step",
                ));
            }
        }
        if self.authorization_epoch == 0 {
            return Err(V3ValidationError::new(
                "invalid_epoch",
                "authorizationEpoch",
                "must be positive",
            ));
        }
        max_len(&self.decisions, 10_000, "decisions")?;
        let keys = self
            .decisions
            .iter()
            .map(|d| serde_jcs::to_string(d).unwrap_or_default())
            .collect::<Vec<_>>();
        sorted_unique(&keys, "decisions")?;
        for decision in &self.decisions {
            sorted_unique(&decision.policy_version_ids, "policyVersionIds")?;
        }
        let computed = self.computed_id().map_err(|e| {
            V3ValidationError::new(
                "canonicalization_failed",
                "authorizationSnapshotId",
                e.to_string(),
            )
        })?;
        if computed != self.authorization_snapshot_id {
            return Err(V3ValidationError::new(
                "identity_mismatch",
                "authorizationSnapshotId",
                format!("computed {computed}"),
            ));
        }
        Ok(())
    }
}

impl ValidateV3 for ObjectPlanRequest {
    fn validate_v3(&self) -> V3Result<()> {
        max_len(&self.objects, 10_000, "objects")?;
        sorted_unique(&self.objects, "objects")?;
        if self
            .objects
            .iter()
            .any(|o| o.kind == ObjectKind::AuthorizationSnapshot)
        {
            return Err(V3ValidationError::new(
                "invalid_object_kind",
                "objects",
                "clients cannot upload AuthorizationSnapshots",
            ));
        }
        for descriptor in &self.objects {
            descriptor.validate_v3()?;
        }
        Ok(())
    }
}
impl ValidateV3 for ObjectPlanResponse {
    fn validate_v3(&self) -> V3Result<()> {
        sorted_unique(&self.present_objects, "presentObjects")?;
        sorted_unique(&self.missing_objects, "missingObjects")?;
        if self
            .present_objects
            .iter()
            .any(|p| self.missing_objects.binary_search(p).is_ok())
        {
            return Err(V3ValidationError::new(
                "invalid_partition",
                "presentObjects",
                "present and missing overlap",
            ));
        }
        if self.upload_capabilities.len() != self.missing_objects.len() {
            return Err(V3ValidationError::new(
                "invalid_partition",
                "uploadCapabilities",
                "requires exactly one capability per missing descriptor",
            ));
        }
        let mut covered = BTreeSet::new();
        for capability in &self.upload_capabilities {
            capability.descriptor.validate_v3()?;
            absolute_transfer_url(&capability.upload_url, "/v3/transfers/uploads", "uploadUrl")?;
            if capability.principal_id != self.principal_id
                || capability.authorization_epoch != self.authorization_epoch
                || capability.space_id != self.space_id
                || capability.client_id != self.client_id
                || capability.request_id != self.request_id
                || capability.plan_id != self.plan_id
            {
                return Err(V3ValidationError::new(
                    "capability_binding_mismatch",
                    "uploadCapabilities",
                    "capability is not fully response-bound",
                ));
            }
            if capability.descriptor.kind == ObjectKind::AuthorizationSnapshot
                || capability.compression != capability.descriptor.compression
                || capability.max_canonical_byte_len != capability.descriptor.canonical_byte_len
                || capability.max_encoded_byte_len != capability.descriptor.encoded_byte_len
            {
                return Err(V3ValidationError::new(
                    "capability_binding_mismatch",
                    "uploadCapabilities",
                    "descriptor bounds/compression mismatch",
                ));
            }
            if self
                .missing_objects
                .binary_search(&capability.descriptor)
                .is_err()
                || !covered.insert(&capability.descriptor)
            {
                return Err(V3ValidationError::new(
                    "invalid_partition",
                    "uploadCapabilities",
                    "capability is duplicate or not missing",
                ));
            }
        }
        Ok(())
    }
}

fn validate_claim(claim: &FetchClaim) -> V3Result<()> {
    match claim {
        FetchClaim::Object {
            object, repo_path, ..
        } => {
            let needs_path = matches!(
                object.kind,
                ObjectKind::Chunk
                    | ObjectKind::FileManifest
                    | ObjectKind::Conflict
                    | ObjectKind::Symlink
                    | ObjectKind::Tombstone
            );
            if needs_path && repo_path.is_none() {
                return Err(V3ValidationError::new(
                    "invalid_path",
                    "repoPath",
                    "content-adjacent objects require a path",
                ));
            }
            if object.kind == ObjectKind::AuthorizationSnapshot {
                return Err(V3ValidationError::new(
                    "invalid_claim",
                    "object",
                    "use the dedicated audit claim",
                ));
            }
        }
        FetchClaim::File {
            preferred_manifest_ids,
            ..
        } => {
            max_len(preferred_manifest_ids, 128, "preferredManifestIds")?;
            sorted_unique(preferred_manifest_ids, "preferredManifestIds")?;
        }
        FetchClaim::AuthorizationSnapshot { .. } => {}
    }
    Ok(())
}
impl ValidateV3 for FetchPlanRequest {
    fn validate_v3(&self) -> V3Result<()> {
        if self.claims.is_empty()
            || self.claims.len() > 10_000
            || self.max_objects == 0
            || self.max_objects > 10_000
        {
            return Err(V3ValidationError::new(
                "limit_exceeded",
                "claims",
                "invalid fetch page bounds",
            ));
        }
        let mut seen = HashSet::new();
        for claim in &self.claims {
            validate_claim(claim)?;
            let key = serde_jcs::to_string(claim).map_err(|e| {
                V3ValidationError::new("canonicalization_failed", "claims", e.to_string())
            })?;
            if !seen.insert(key) {
                return Err(V3ValidationError::new(
                    "duplicate_identity",
                    "claims",
                    "duplicate claim",
                ));
            }
        }
        Ok(())
    }
}
impl ValidateV3 for FetchPlanResponse {
    fn validate_v3(&self) -> V3Result<()> {
        max_len(&self.download_capabilities, 10_000, "downloadCapabilities")?;
        for capability in &self.download_capabilities {
            validate_claim(&capability.claim)?;
            absolute_transfer_url(
                &capability.download_url,
                "/v3/transfers/downloads",
                "downloadUrl",
            )?;
            if capability.principal_id != self.principal_id
                || capability.authorization_epoch != self.authorization_epoch
                || capability.space_id != self.space_id
                || capability.client_id != self.client_id
                || capability.request_id != self.request_id
                || capability.plan_id != self.plan_id
            {
                return Err(V3ValidationError::new(
                    "capability_binding_mismatch",
                    "downloadCapabilities",
                    "capability is not fully response-bound",
                ));
            }
            if capability.token.len() < 32 || capability.resume_token.len() < 16 {
                return Err(V3ValidationError::new(
                    "invalid_capability",
                    "downloadCapabilities",
                    "token too short",
                ));
            }
        }
        if self.has_more != self.next_page_cursor.is_some() {
            return Err(V3ValidationError::new(
                "invalid_pagination",
                "nextPageCursor",
                "hasMore and next cursor disagree",
            ));
        }
        Ok(())
    }
}

impl ValidateV3 for SpaceEventPageRequest {
    fn validate_v3(&self) -> V3Result<()> {
        if self.max_events == 0 || self.max_events > 1_000 {
            Err(V3ValidationError::new(
                "limit_exceeded",
                "maxEvents",
                "must be 1..=1000",
            ))
        } else {
            Ok(())
        }
    }
}
impl ValidateV3 for SpaceEventPage {
    fn validate_v3(&self) -> V3Result<()> {
        if self.authorization_epoch == 0 || self.events.len() > 1_000 {
            return Err(V3ValidationError::new(
                "limit_exceeded",
                "events",
                "invalid event-page bounds",
            ));
        }
        let mut previous = 0;
        for event in &self.events {
            let sequence = event.sequence();
            if sequence <= previous || sequence > self.scanned_through_sequence {
                return Err(V3ValidationError::new(
                    "invalid_event_sequence",
                    "events",
                    "events must be positive, increasing and within scanned watermark",
                ));
            }
            previous = sequence;
            if let SpaceEvent::RepositoryViewChanged { payload, .. } = event {
                if payload.outcome == ViewOutcome::NoOp {
                    return Err(V3ValidationError::new(
                        "invalid_event",
                        "events",
                        "no-op View cannot emit an event",
                    ));
                }
            }
        }
        Ok(())
    }
}

impl ValidateV3 for SyncTransactionReceipt {
    fn validate_v3(&self) -> V3Result<()> {
        if self.authorization_epoch == 0 || self.resulting_authorization_epoch == 0 {
            return Err(V3ValidationError::new(
                "invalid_epoch",
                "authorizationEpoch",
                "epochs must be positive",
            ));
        }
        match self.status {
            ReceiptStatus::Applied
                if !self.rejections.is_empty() || self.event_cursor.is_none() =>
            {
                return Err(V3ValidationError::new(
                    "invalid_receipt",
                    "status",
                    "applied receipt requires cursor and no rejections",
                ));
            }
            ReceiptStatus::Rejected
                if self.rejections.is_empty()
                    || !self.accepted_step_ids.is_empty()
                    || !self.accepted_observation_ids.is_empty()
                    || !self.accepted_repository_view_ids.is_empty()
                    || !self.accepted_policy_set_ids.is_empty()
                    || !self.accepted_products.is_empty()
                    || self.view_result.is_some()
                    || self.policy_root_result.is_some()
                    || !self.event_ids.is_empty()
                    || self.first_event_sequence.is_some()
                    || self.last_event_sequence.is_some()
                    || self.event_cursor.is_some() =>
            {
                return Err(V3ValidationError::new(
                    "invalid_receipt",
                    "status",
                    "rejected receipt cannot expose partial acceptance",
                ));
            }
            _ => {}
        }
        if self.first_event_sequence.is_none() != self.last_event_sequence.is_none() {
            return Err(V3ValidationError::new(
                "invalid_receipt",
                "eventSequence",
                "first/last must both be null or present",
            ));
        }
        if let Some(view_result) = self.view_result.as_ref() {
            for layer in &view_result.layer_results {
                sorted_unique(&layer.previous_heads, "layerResults.previousHeads")?;
                sorted_unique(&layer.proposed_heads, "layerResults.proposedHeads")?;
                sorted_unique(&layer.resulting_heads, "layerResults.resultingHeads")?;
                let divergence_created =
                    layer.previous_heads.len() <= 1 && layer.resulting_heads.len() > 1;
                if layer.divergence_created != divergence_created {
                    return Err(V3ValidationError::new(
                        "invalid_receipt",
                        "layerResults.divergenceCreated",
                        "must be true exactly when previousHeads <= 1 and resultingHeads > 1",
                    ));
                }
                match layer.mode {
                    LayerMutationMode::Choose
                        if layer.decision_product_object_id.is_none()
                            || layer.proposed_heads != layer.resulting_heads =>
                    {
                        return Err(V3ValidationError::new(
                            "invalid_receipt",
                            "layerResults",
                            "choose requires a Decision object and resultingHeads equal proposedHeads",
                        ));
                    }
                    LayerMutationMode::Advance | LayerMutationMode::Integrate
                        if layer.decision_product_object_id.is_some() =>
                    {
                        return Err(V3ValidationError::new(
                            "invalid_receipt",
                            "layerResults.decisionProductObjectId",
                            "only choose may reference a Decision object",
                        ));
                    }
                    _ => {}
                }
            }
        }
        if let (Some(first), Some(last)) = (
            self.first_event_sequence.as_ref(),
            self.last_event_sequence.as_ref(),
        ) {
            if *first == 0 || first > last {
                return Err(V3ValidationError::new(
                    "invalid_event_sequence",
                    "eventSequence",
                    "invalid receipt event range",
                ));
            }
        }
        sorted_unique(&self.accepted_step_ids, "acceptedStepIds")?;
        sorted_unique(&self.accepted_observation_ids, "acceptedObservationIds")?;
        sorted_unique(
            &self.accepted_repository_view_ids,
            "acceptedRepositoryViewIds",
        )?;
        sorted_unique(&self.accepted_policy_set_ids, "acceptedPolicySetIds")?;
        sorted_unique(&self.event_ids, "eventIds")?;
        Ok(())
    }
}

impl ValidateV3 for SpaceBootstrap {
    fn validate_v3(&self) -> V3Result<()> {
        if self.authorization_epoch == 0 {
            return Err(V3ValidationError::new(
                "invalid_epoch",
                "authorizationEpoch",
                "must be positive",
            ));
        }
        if self.current_repository_view.repository_view_id
            != *self.current_repository_view.record.id()
        {
            return Err(V3ValidationError::new(
                "identity_mismatch",
                "currentRepositoryView",
                "projection ID differs from record ID",
            ));
        }
        reject_wire_worktrees(
            self.current_repository_view.record.worktrees(),
            "currentRepositoryView.record.worktrees",
        )?;
        if self.current_policy_set_id != self.current_policy_set.policy_set_id
            || self.current_policy_set.policy_set_id != self.current_policy_set.record.policy_set_id
        {
            return Err(V3ValidationError::new(
                "identity_mismatch",
                "currentPolicySetId",
                "literal and embedded PolicySet IDs differ",
            ));
        }
        if self.current_repository_view.record.space_id() != &self.space_id
            || self.current_policy_set.record.space_id != self.space_id
        {
            return Err(V3ValidationError::new(
                "cross_space_reference",
                "spaceId",
                "bootstrap roots must belong to the Space",
            ));
        }
        sorted_unique(&self.graph_root_step_ids, "graphRootStepIds")?;
        Ok(())
    }
}

fn reject_wire_worktrees<T>(worktrees: &[T], field: &'static str) -> V3Result<()> {
    if worktrees.is_empty() {
        Ok(())
    } else {
        Err(V3ValidationError::new(
            "wire_worktree_forbidden",
            field,
            "Worktree state is local-only and must not cross a Sync V3 boundary",
        ))
    }
}

#[allow(dead_code)]
fn _typed_object_id(_: &ObjectId) {}
