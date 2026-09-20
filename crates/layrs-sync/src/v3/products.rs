use super::{
    DecisionId, GateDefinitionId, GateEvaluationId, Nullable, ObjectDescriptor, ObjectKind,
    PolicyId, PolicyVersionId, ProductLogicalId, ProductRoot, WeaveLogicalId,
};
use layrs_core::scm::{
    ActorId, ConflictId, Layer, LayerId, Marker, MarkerId, ObjectId, OperationId, ProofId, Release,
    ReleaseId, SpaceId, StepId, Timestamp,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, error::Error, fmt};

pub const PRODUCT_IDENTITY_DOMAIN: &str = "layrs-product-v1";

#[derive(Debug)]
pub enum ProductIdentityError {
    CanonicalJson(serde_json::Error),
    ClaimMismatch {
        claimed: ObjectId,
        computed: ObjectId,
    },
    NonCanonicalWire,
    WrongDescriptorKind(ObjectKind),
}
impl fmt::Display for ProductIdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CanonicalJson(error) => write!(f, "cannot encode canonical Product: {error}"),
            Self::ClaimMismatch { claimed, computed } => {
                write!(
                    f,
                    "Product claim {claimed} does not match computed ID {computed}"
                )
            }
            Self::NonCanonicalWire => f.write_str("Product wire is not exact RFC 8785 JCS"),
            Self::WrongDescriptorKind(kind) => {
                write!(f, "Product bytes cannot satisfy descriptor kind {kind:?}")
            }
        }
    }
}
impl Error for ProductIdentityError {}
impl From<serde_json::Error> for ProductIdentityError {
    fn from(value: serde_json::Error) -> Self {
        Self::CanonicalJson(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductReferenceClassification {
    Product(ProductLogicalId),
    NonProduct,
    Missing,
}

/// Resolves object-ID references against the transaction/store overlay. This
/// is required because only verified Product objects contribute logical product
/// dependencies; opaque Evidence/recipe/rule bytes never do.
pub trait ProductReferenceResolver {
    fn classify(&self, object_id: &ObjectId) -> ProductReferenceClassification;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductDependencies {
    pub step_ids: Vec<StepId>,
    pub product_ids: Vec<ProductLogicalId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductProjectionError {
    InvalidStepSubject(ObjectId),
    MissingObject(ObjectId),
    ExpectedProduct(ObjectId),
    StepProjectionMismatch,
    ProductProjectionMismatch,
}
impl fmt::Display for ProductProjectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStepSubject(id) => {
                write!(f, "immutable Step subject has invalid Step ID {id}")
            }
            Self::MissingObject(id) => write!(f, "cannot resolve referenced object {id}"),
            Self::ExpectedProduct(id) => write!(f, "referenced object {id} must be a Product"),
            Self::StepProjectionMismatch => {
                f.write_str("dependsOnStepIds is not the exact record projection")
            }
            Self::ProductProjectionMismatch => {
                f.write_str("dependsOnProductIds is not the exact record projection")
            }
        }
    }
}
impl Error for ProductProjectionError {}

mod records;
pub use records::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum ProductEnvelope {
    #[serde(rename = "layer")]
    Layer {
        #[serde(rename = "productId")]
        product_id: LayerId,
        #[serde(rename = "productObjectId")]
        product_object_id: ObjectId,
        #[serde(rename = "dependsOnStepIds")]
        depends_on_step_ids: Vec<StepId>,
        #[serde(rename = "dependsOnProductIds")]
        depends_on_product_ids: Vec<ProductLogicalId>,
        #[serde(rename = "previousProductObjectId")]
        previous_product_object_id: Nullable<ObjectId>,
        record: Layer,
    },
    #[serde(rename = "marker")]
    Marker {
        #[serde(rename = "productId")]
        product_id: MarkerId,
        #[serde(rename = "productObjectId")]
        product_object_id: ObjectId,
        #[serde(rename = "dependsOnStepIds")]
        depends_on_step_ids: Vec<StepId>,
        #[serde(rename = "dependsOnProductIds")]
        depends_on_product_ids: Vec<ProductLogicalId>,
        record: Marker,
    },
    #[serde(rename = "release")]
    Release {
        #[serde(rename = "productId")]
        product_id: ReleaseId,
        #[serde(rename = "productObjectId")]
        product_object_id: ObjectId,
        #[serde(rename = "dependsOnStepIds")]
        depends_on_step_ids: Vec<StepId>,
        #[serde(rename = "dependsOnProductIds")]
        depends_on_product_ids: Vec<ProductLogicalId>,
        record: Release,
    },
    #[serde(rename = "proof")]
    Proof {
        #[serde(rename = "productId")]
        product_id: ProofId,
        #[serde(rename = "productObjectId")]
        product_object_id: ObjectId,
        #[serde(rename = "dependsOnStepIds")]
        depends_on_step_ids: Vec<StepId>,
        #[serde(rename = "dependsOnProductIds")]
        depends_on_product_ids: Vec<ProductLogicalId>,
        record: ProofRecord,
    },
    #[serde(rename = "gateDefinition")]
    GateDefinition {
        #[serde(rename = "productId")]
        product_id: GateDefinitionId,
        #[serde(rename = "productObjectId")]
        product_object_id: ObjectId,
        #[serde(rename = "dependsOnStepIds")]
        depends_on_step_ids: Vec<StepId>,
        #[serde(rename = "dependsOnProductIds")]
        depends_on_product_ids: Vec<ProductLogicalId>,
        #[serde(rename = "previousProductObjectId")]
        previous_product_object_id: Nullable<ObjectId>,
        record: GateDefinitionRecord,
    },
    #[serde(rename = "gateEvaluation")]
    GateEvaluation {
        #[serde(rename = "productId")]
        product_id: GateEvaluationId,
        #[serde(rename = "productObjectId")]
        product_object_id: ObjectId,
        #[serde(rename = "dependsOnStepIds")]
        depends_on_step_ids: Vec<StepId>,
        #[serde(rename = "dependsOnProductIds")]
        depends_on_product_ids: Vec<ProductLogicalId>,
        record: GateEvaluationRecord,
    },
    #[serde(rename = "policy")]
    Policy {
        #[serde(rename = "productId")]
        product_id: PolicyId,
        #[serde(rename = "productObjectId")]
        product_object_id: ObjectId,
        #[serde(rename = "dependsOnStepIds")]
        depends_on_step_ids: Vec<StepId>,
        #[serde(rename = "dependsOnProductIds")]
        depends_on_product_ids: Vec<ProductLogicalId>,
        #[serde(rename = "previousProductObjectId")]
        previous_product_object_id: Nullable<ObjectId>,
        record: PolicyRecord,
    },
    #[serde(rename = "weave")]
    Weave {
        #[serde(rename = "productId")]
        product_id: WeaveLogicalId,
        #[serde(rename = "productObjectId")]
        product_object_id: ObjectId,
        #[serde(rename = "dependsOnStepIds")]
        depends_on_step_ids: Vec<StepId>,
        #[serde(rename = "dependsOnProductIds")]
        depends_on_product_ids: Vec<ProductLogicalId>,
        record: Box<WeaveReceiptRecord>,
    },
    #[serde(rename = "decision")]
    Decision {
        #[serde(rename = "productId")]
        product_id: DecisionId,
        #[serde(rename = "productObjectId")]
        product_object_id: ObjectId,
        #[serde(rename = "dependsOnStepIds")]
        depends_on_step_ids: Vec<StepId>,
        #[serde(rename = "dependsOnProductIds")]
        depends_on_product_ids: Vec<ProductLogicalId>,
        record: DecisionRecord,
    },
}

#[derive(Serialize)]
#[serde(tag = "kind")]
enum ProductIdentity<'a> {
    #[serde(rename = "layer")]
    Layer {
        #[serde(rename = "productId")]
        product_id: &'a LayerId,
        #[serde(rename = "dependsOnStepIds")]
        depends_on_step_ids: &'a [StepId],
        #[serde(rename = "dependsOnProductIds")]
        depends_on_product_ids: &'a [ProductLogicalId],
        #[serde(rename = "previousProductObjectId")]
        previous_product_object_id: &'a Nullable<ObjectId>,
        record: &'a Layer,
    },
    #[serde(rename = "marker")]
    Marker {
        #[serde(rename = "productId")]
        product_id: &'a MarkerId,
        #[serde(rename = "dependsOnStepIds")]
        depends_on_step_ids: &'a [StepId],
        #[serde(rename = "dependsOnProductIds")]
        depends_on_product_ids: &'a [ProductLogicalId],
        record: &'a Marker,
    },
    #[serde(rename = "release")]
    Release {
        #[serde(rename = "productId")]
        product_id: &'a ReleaseId,
        #[serde(rename = "dependsOnStepIds")]
        depends_on_step_ids: &'a [StepId],
        #[serde(rename = "dependsOnProductIds")]
        depends_on_product_ids: &'a [ProductLogicalId],
        record: &'a Release,
    },
    #[serde(rename = "proof")]
    Proof {
        #[serde(rename = "productId")]
        product_id: &'a ProofId,
        #[serde(rename = "dependsOnStepIds")]
        depends_on_step_ids: &'a [StepId],
        #[serde(rename = "dependsOnProductIds")]
        depends_on_product_ids: &'a [ProductLogicalId],
        record: &'a ProofRecord,
    },
    #[serde(rename = "gateDefinition")]
    GateDefinition {
        #[serde(rename = "productId")]
        product_id: &'a GateDefinitionId,
        #[serde(rename = "dependsOnStepIds")]
        depends_on_step_ids: &'a [StepId],
        #[serde(rename = "dependsOnProductIds")]
        depends_on_product_ids: &'a [ProductLogicalId],
        #[serde(rename = "previousProductObjectId")]
        previous_product_object_id: &'a Nullable<ObjectId>,
        record: &'a GateDefinitionRecord,
    },
    #[serde(rename = "gateEvaluation")]
    GateEvaluation {
        #[serde(rename = "productId")]
        product_id: &'a GateEvaluationId,
        #[serde(rename = "dependsOnStepIds")]
        depends_on_step_ids: &'a [StepId],
        #[serde(rename = "dependsOnProductIds")]
        depends_on_product_ids: &'a [ProductLogicalId],
        record: &'a GateEvaluationRecord,
    },
    #[serde(rename = "policy")]
    Policy {
        #[serde(rename = "productId")]
        product_id: &'a PolicyId,
        #[serde(rename = "dependsOnStepIds")]
        depends_on_step_ids: &'a [StepId],
        #[serde(rename = "dependsOnProductIds")]
        depends_on_product_ids: &'a [ProductLogicalId],
        #[serde(rename = "previousProductObjectId")]
        previous_product_object_id: &'a Nullable<ObjectId>,
        record: &'a PolicyRecord,
    },
    #[serde(rename = "weave")]
    Weave {
        #[serde(rename = "productId")]
        product_id: &'a WeaveLogicalId,
        #[serde(rename = "dependsOnStepIds")]
        depends_on_step_ids: &'a [StepId],
        #[serde(rename = "dependsOnProductIds")]
        depends_on_product_ids: &'a [ProductLogicalId],
        record: &'a WeaveReceiptRecord,
    },
    #[serde(rename = "decision")]
    Decision {
        #[serde(rename = "productId")]
        product_id: &'a DecisionId,
        #[serde(rename = "dependsOnStepIds")]
        depends_on_step_ids: &'a [StepId],
        #[serde(rename = "dependsOnProductIds")]
        depends_on_product_ids: &'a [ProductLogicalId],
        record: &'a DecisionRecord,
    },
}

impl ProductEnvelope {
    fn identity(&self) -> ProductIdentity<'_> {
        match self {
            Self::Layer {
                product_id,
                depends_on_step_ids,
                depends_on_product_ids,
                previous_product_object_id,
                record,
                ..
            } => ProductIdentity::Layer {
                product_id,
                depends_on_step_ids,
                depends_on_product_ids,
                previous_product_object_id,
                record,
            },
            Self::Marker {
                product_id,
                depends_on_step_ids,
                depends_on_product_ids,
                record,
                ..
            } => ProductIdentity::Marker {
                product_id,
                depends_on_step_ids,
                depends_on_product_ids,
                record,
            },
            Self::Release {
                product_id,
                depends_on_step_ids,
                depends_on_product_ids,
                record,
                ..
            } => ProductIdentity::Release {
                product_id,
                depends_on_step_ids,
                depends_on_product_ids,
                record,
            },
            Self::Proof {
                product_id,
                depends_on_step_ids,
                depends_on_product_ids,
                record,
                ..
            } => ProductIdentity::Proof {
                product_id,
                depends_on_step_ids,
                depends_on_product_ids,
                record,
            },
            Self::GateDefinition {
                product_id,
                depends_on_step_ids,
                depends_on_product_ids,
                previous_product_object_id,
                record,
                ..
            } => ProductIdentity::GateDefinition {
                product_id,
                depends_on_step_ids,
                depends_on_product_ids,
                previous_product_object_id,
                record,
            },
            Self::GateEvaluation {
                product_id,
                depends_on_step_ids,
                depends_on_product_ids,
                record,
                ..
            } => ProductIdentity::GateEvaluation {
                product_id,
                depends_on_step_ids,
                depends_on_product_ids,
                record,
            },
            Self::Policy {
                product_id,
                depends_on_step_ids,
                depends_on_product_ids,
                previous_product_object_id,
                record,
                ..
            } => ProductIdentity::Policy {
                product_id,
                depends_on_step_ids,
                depends_on_product_ids,
                previous_product_object_id,
                record,
            },
            Self::Weave {
                product_id,
                depends_on_step_ids,
                depends_on_product_ids,
                record,
                ..
            } => ProductIdentity::Weave {
                product_id,
                depends_on_step_ids,
                depends_on_product_ids,
                record,
            },
            Self::Decision {
                product_id,
                depends_on_step_ids,
                depends_on_product_ids,
                record,
                ..
            } => ProductIdentity::Decision {
                product_id,
                depends_on_step_ids,
                depends_on_product_ids,
                record,
            },
        }
    }

    /// RFC 8785 JCS bytes of the complete Product envelope, including its claim.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ProductIdentityError> {
        Ok(serde_jcs::to_vec(self)?)
    }

    /// Domain-separated identity preimage, omitting only `productObjectId`.
    pub fn identity_bytes(&self) -> Result<Vec<u8>, ProductIdentityError> {
        let canonical = serde_jcs::to_vec(&self.identity())?;
        let mut bytes = Vec::with_capacity(PRODUCT_IDENTITY_DOMAIN.len() + 1 + canonical.len());
        bytes.extend_from_slice(PRODUCT_IDENTITY_DOMAIN.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&canonical);
        Ok(bytes)
    }

    pub fn computed_object_id(&self) -> Result<ObjectId, ProductIdentityError> {
        let digest = blake3::hash(&self.identity_bytes()?);
        Ok(ObjectId::parse(format!("blake3:{digest}")).expect("BLAKE3 is a valid ObjectId"))
    }

    pub fn verify_identity(&self) -> Result<(), ProductIdentityError> {
        let computed = self.computed_object_id()?;
        if &computed != self.object_id() {
            return Err(ProductIdentityError::ClaimMismatch {
                claimed: self.object_id().clone(),
                computed,
            });
        }
        Ok(())
    }

    /// Rejects semantically equivalent but non-JCS Product upload bytes.
    pub fn verify_canonical_wire(&self, wire: &[u8]) -> Result<(), ProductIdentityError> {
        self.verify_identity()?;
        if self.canonical_bytes()?.as_slice() != wire {
            return Err(ProductIdentityError::NonCanonicalWire);
        }
        Ok(())
    }

    pub fn verify_descriptor(
        &self,
        descriptor: &ObjectDescriptor,
    ) -> Result<(), ProductIdentityError> {
        if descriptor.kind != ObjectKind::Product {
            return Err(ProductIdentityError::WrongDescriptorKind(descriptor.kind));
        }
        self.verify_identity()?;
        if descriptor.object_id != *self.object_id() {
            return Err(ProductIdentityError::ClaimMismatch {
                claimed: descriptor.object_id.clone(),
                computed: self.object_id().clone(),
            });
        }
        Ok(())
    }

    /// Recomputes exact direct dependency projections from the typed record and
    /// a kind-aware transaction/store overlay.
    pub fn projected_dependencies<R: ProductReferenceResolver>(
        &self,
        resolver: &R,
    ) -> Result<ProductDependencies, ProductProjectionError> {
        let mut steps = BTreeSet::new();
        let mut products = BTreeSet::new();
        match self {
            Self::Layer {
                previous_product_object_id,
                record,
                ..
            } => {
                for parent in record.parent_layer_ids() {
                    products.insert(
                        ProductLogicalId::parse(parent.as_str())
                            .expect("LayerId is a ProductLogicalId"),
                    );
                }
                if let Some(previous) = previous_product_object_id.as_ref() {
                    products.insert(require_product(resolver, previous)?);
                }
            }
            Self::Marker { record, .. } => {
                steps.insert(record.target_step_id().clone());
                for proof in record.proof_ids() {
                    products.insert(
                        ProductLogicalId::parse(proof.as_str())
                            .expect("ProofId is a ProductLogicalId"),
                    );
                }
            }
            Self::Release { record, .. } => {
                steps.insert(record.target_step_id().clone());
                for proof in record.proof_ids() {
                    products.insert(
                        ProductLogicalId::parse(proof.as_str())
                            .expect("ProofId is a ProductLogicalId"),
                    );
                }
            }
            Self::Proof { record, .. } => {
                project_subject(&record.subject, resolver, &mut steps, &mut products)?;
                for object in &record.policy_version_object_ids {
                    products.insert(require_product(resolver, object)?);
                }
            }
            Self::GateDefinition {
                previous_product_object_id,
                record,
                ..
            } => {
                if let Some(previous) = previous_product_object_id.as_ref() {
                    products.insert(require_product(resolver, previous)?);
                }
                for object in &record.policy_version_object_ids {
                    products.insert(require_product(resolver, object)?);
                }
            }
            Self::GateEvaluation { record, .. } => {
                project_subject(&record.subject, resolver, &mut steps, &mut products)?;
                products.insert(require_product(
                    resolver,
                    &record.gate_definition_object_id,
                )?);
                for object in record
                    .proof_object_ids
                    .iter()
                    .chain(&record.policy_version_object_ids)
                {
                    products.insert(require_product(resolver, object)?);
                }
            }
            Self::Policy {
                previous_product_object_id,
                ..
            } => {
                if let Some(previous) = previous_product_object_id.as_ref() {
                    products.insert(require_product(resolver, previous)?);
                }
            }
            Self::Weave { record, .. } => {
                steps.extend(record.source_heads.iter().cloned());
                steps.extend(record.target_heads_before.iter().cloned());
                steps.insert(record.result_step_id.clone());
                for base in &record.merge_bases {
                    match base {
                        MergeBase::Step { step_id, .. } => {
                            steps.insert(step_id.clone());
                        }
                        MergeBase::Virtual {
                            source_step_ids, ..
                        } => steps.extend(source_step_ids.iter().cloned()),
                    }
                }
                if let Some(gate) = record.gate_snapshot_id.as_ref() {
                    if let ProductReferenceClassification::Product(product) =
                        classify_required(resolver, gate)?
                    {
                        products.insert(product);
                    }
                }
            }
            Self::Decision { record, .. } => {
                steps.extend(record.discarded_head_step_ids.iter().cloned());
                steps.extend(record.chosen_head_step_ids.iter().cloned());
                products.insert(
                    ProductLogicalId::parse(record.layer_id.as_str())
                        .expect("LayerId is a ProductLogicalId"),
                );
            }
        }
        Ok(ProductDependencies {
            step_ids: steps.into_iter().collect(),
            product_ids: products.into_iter().collect(),
        })
    }

    pub fn verify_dependency_projection<R: ProductReferenceResolver>(
        &self,
        resolver: &R,
    ) -> Result<(), ProductProjectionError> {
        let projected = self.projected_dependencies(resolver)?;
        let (claimed_steps, claimed_products) = self.dependencies();
        if claimed_steps != projected.step_ids {
            return Err(ProductProjectionError::StepProjectionMismatch);
        }
        if claimed_products != projected.product_ids {
            return Err(ProductProjectionError::ProductProjectionMismatch);
        }
        Ok(())
    }

    pub(crate) fn intrinsic_dependency_projection(
        &self,
    ) -> Result<(Vec<StepId>, Vec<ProductLogicalId>, bool), ProductProjectionError> {
        let mut steps = BTreeSet::new();
        let mut products = BTreeSet::new();
        let complete = match self {
            Self::Layer {
                previous_product_object_id,
                record,
                ..
            } => {
                products.extend(record.parent_layer_ids().iter().map(|id| {
                    ProductLogicalId::parse(id.as_str()).expect("LayerId is a ProductLogicalId")
                }));
                previous_product_object_id.is_none()
            }
            Self::Marker { record, .. } => {
                steps.insert(record.target_step_id().clone());
                products.extend(record.proof_ids().iter().map(|id| {
                    ProductLogicalId::parse(id.as_str()).expect("ProofId is a ProductLogicalId")
                }));
                true
            }
            Self::Release { record, .. } => {
                steps.insert(record.target_step_id().clone());
                products.extend(record.proof_ids().iter().map(|id| {
                    ProductLogicalId::parse(id.as_str()).expect("ProofId is a ProductLogicalId")
                }));
                true
            }
            Self::Proof { record, .. } => {
                let subject_complete =
                    project_intrinsic_subject(&record.subject, &mut steps, &mut products)?;
                subject_complete && record.policy_version_object_ids.is_empty()
            }
            Self::GateDefinition {
                previous_product_object_id,
                record,
                ..
            } => {
                previous_product_object_id.is_none() && record.policy_version_object_ids.is_empty()
            }
            Self::GateEvaluation { record, .. } => {
                let subject_complete =
                    project_intrinsic_subject(&record.subject, &mut steps, &mut products)?;
                subject_complete
                    && record.proof_object_ids.is_empty()
                    && record.policy_version_object_ids.is_empty()
            }
            Self::Policy {
                previous_product_object_id,
                ..
            } => previous_product_object_id.is_none(),
            Self::Weave { record, .. } => {
                steps.extend(record.source_heads.iter().cloned());
                steps.extend(record.target_heads_before.iter().cloned());
                steps.insert(record.result_step_id.clone());
                for base in &record.merge_bases {
                    match base {
                        MergeBase::Step { step_id, .. } => {
                            steps.insert(step_id.clone());
                        }
                        MergeBase::Virtual {
                            source_step_ids, ..
                        } => steps.extend(source_step_ids.iter().cloned()),
                    }
                }
                record.gate_snapshot_id.is_none()
            }
            Self::Decision { record, .. } => {
                steps.extend(record.discarded_head_step_ids.iter().cloned());
                steps.extend(record.chosen_head_step_ids.iter().cloned());
                products.insert(
                    ProductLogicalId::parse(record.layer_id.as_str())
                        .expect("LayerId is a ProductLogicalId"),
                );
                true
            }
        };
        Ok((
            steps.into_iter().collect(),
            products.into_iter().collect(),
            complete,
        ))
    }
    pub fn object_id(&self) -> &ObjectId {
        match self {
            Self::Layer {
                product_object_id, ..
            }
            | Self::Marker {
                product_object_id, ..
            }
            | Self::Release {
                product_object_id, ..
            }
            | Self::Proof {
                product_object_id, ..
            }
            | Self::GateDefinition {
                product_object_id, ..
            }
            | Self::GateEvaluation {
                product_object_id, ..
            }
            | Self::Policy {
                product_object_id, ..
            }
            | Self::Weave {
                product_object_id, ..
            }
            | Self::Decision {
                product_object_id, ..
            } => product_object_id,
        }
    }
    pub fn logical_id(&self) -> &str {
        match self {
            Self::Layer { product_id, .. } => product_id.as_str(),
            Self::Marker { product_id, .. } => product_id.as_str(),
            Self::Release { product_id, .. } => product_id.as_str(),
            Self::Proof { product_id, .. } => product_id.as_str(),
            Self::GateDefinition { product_id, .. } => product_id.as_str(),
            Self::GateEvaluation { product_id, .. } => product_id.as_str(),
            Self::Policy { product_id, .. } => product_id.as_str(),
            Self::Weave { product_id, .. } => product_id.as_str(),
            Self::Decision { product_id, .. } => product_id.as_str(),
        }
    }
    pub fn dependencies(&self) -> (&[StepId], &[ProductLogicalId]) {
        match self {
            Self::Layer {
                depends_on_step_ids: s,
                depends_on_product_ids: p,
                ..
            }
            | Self::Marker {
                depends_on_step_ids: s,
                depends_on_product_ids: p,
                ..
            }
            | Self::Release {
                depends_on_step_ids: s,
                depends_on_product_ids: p,
                ..
            }
            | Self::Proof {
                depends_on_step_ids: s,
                depends_on_product_ids: p,
                ..
            }
            | Self::GateDefinition {
                depends_on_step_ids: s,
                depends_on_product_ids: p,
                ..
            }
            | Self::GateEvaluation {
                depends_on_step_ids: s,
                depends_on_product_ids: p,
                ..
            }
            | Self::Policy {
                depends_on_step_ids: s,
                depends_on_product_ids: p,
                ..
            }
            | Self::Weave {
                depends_on_step_ids: s,
                depends_on_product_ids: p,
                ..
            }
            | Self::Decision {
                depends_on_step_ids: s,
                depends_on_product_ids: p,
                ..
            } => (s, p),
        }
    }

    pub(crate) fn record_space_id(&self) -> &SpaceId {
        match self {
            Self::Layer { record, .. } => record.space_id(),
            Self::Marker { record, .. } => record.space_id(),
            Self::Release { record, .. } => record.space_id(),
            Self::Proof { record, .. } => &record.space_id,
            Self::GateDefinition { record, .. } => &record.space_id,
            Self::GateEvaluation { record, .. } => &record.space_id,
            Self::Policy { record, .. } => &record.space_id,
            Self::Weave { record, .. } => &record.space_id,
            Self::Decision { record, .. } => &record.space_id,
        }
    }

    pub(crate) fn typed_identity_matches(&self) -> bool {
        match self {
            Self::Layer {
                product_id, record, ..
            } => product_id == record.id(),
            Self::Marker {
                product_id, record, ..
            } => product_id == record.id(),
            Self::Release {
                product_id, record, ..
            } => product_id == record.id(),
            Self::Proof {
                product_id, record, ..
            } => product_id == &record.proof_id,
            Self::GateDefinition {
                product_id, record, ..
            } => product_id == &record.gate_definition_id,
            Self::GateEvaluation {
                product_id, record, ..
            } => product_id == &record.gate_evaluation_id,
            Self::Policy {
                product_id, record, ..
            } => product_id == &record.policy_id,
            Self::Weave {
                product_id, record, ..
            } => product_id.as_str() == format!("weave:{}", record.weave_id.as_str()),
            Self::Decision {
                product_id, record, ..
            } => product_id == &record.decision_id,
        }
    }

    pub(crate) fn weave_record(&self) -> Option<&WeaveReceiptRecord> {
        match self {
            Self::Weave { record, .. } => Some(record),
            _ => None,
        }
    }

    pub fn decision_layer_id(&self) -> Option<&LayerId> {
        match self {
            Self::Decision { record, .. } => Some(&record.layer_id),
            _ => None,
        }
    }

    pub fn decision_record(&self) -> Option<&DecisionRecord> {
        match self {
            Self::Decision { record, .. } => Some(record),
            _ => None,
        }
    }
}

fn classify_required<R: ProductReferenceResolver>(
    resolver: &R,
    object_id: &ObjectId,
) -> Result<ProductReferenceClassification, ProductProjectionError> {
    match resolver.classify(object_id) {
        ProductReferenceClassification::Missing => {
            Err(ProductProjectionError::MissingObject(object_id.clone()))
        }
        classification => Ok(classification),
    }
}

fn require_product<R: ProductReferenceResolver>(
    resolver: &R,
    object_id: &ObjectId,
) -> Result<ProductLogicalId, ProductProjectionError> {
    match classify_required(resolver, object_id)? {
        ProductReferenceClassification::Product(product) => Ok(product),
        ProductReferenceClassification::NonProduct => {
            Err(ProductProjectionError::ExpectedProduct(object_id.clone()))
        }
        ProductReferenceClassification::Missing => unreachable!("handled by classify_required"),
    }
}

fn project_subject<R: ProductReferenceResolver>(
    subject: &ImmutableSubject,
    resolver: &R,
    steps: &mut BTreeSet<StepId>,
    products: &mut BTreeSet<ProductLogicalId>,
) -> Result<(), ProductProjectionError> {
    match subject {
        ImmutableSubject::Step { object_id } => {
            steps.insert(
                StepId::parse(object_id.as_str())
                    .map_err(|_| ProductProjectionError::InvalidStepSubject(object_id.clone()))?,
            );
        }
        ImmutableSubject::Product { object_id } => {
            products.insert(require_product(resolver, object_id)?);
        }
        ImmutableSubject::ProductRoot { product } => {
            products.insert(product.logical_id());
        }
        ImmutableSubject::Tree { .. }
        | ImmutableSubject::Conflict { .. }
        | ImmutableSubject::Evidence { .. } => {}
    }
    Ok(())
}

fn project_intrinsic_subject(
    subject: &ImmutableSubject,
    steps: &mut BTreeSet<StepId>,
    products: &mut BTreeSet<ProductLogicalId>,
) -> Result<bool, ProductProjectionError> {
    match subject {
        ImmutableSubject::Step { object_id } => {
            steps.insert(
                StepId::parse(object_id.as_str())
                    .map_err(|_| ProductProjectionError::InvalidStepSubject(object_id.clone()))?,
            );
            Ok(true)
        }
        ImmutableSubject::ProductRoot { product } => {
            products.insert(product.logical_id());
            Ok(true)
        }
        ImmutableSubject::Product { .. } => Ok(false),
        ImmutableSubject::Tree { .. }
        | ImmutableSubject::Conflict { .. }
        | ImmutableSubject::Evidence { .. } => Ok(true),
    }
}
