//! Bounded immutable Layer/Marker/Release admission. Layer revisions, Proofs and
//! policy changes have separate protocols and cannot be smuggled through here.
mod weave;
use super::transaction_plan::reject;
use super::*;
use bytes::Bytes;
use futures_util::stream;
use layrs_core::scm::{ObjectId, SpaceId, Step, StepId};
use layrs_graph::scm::StepDag;
use layrs_sync::v3::*;
use sqlx::{Postgres, Row, Transaction};
use std::collections::BTreeMap;

pub(crate) struct PreparedProduct {
    pub envelope: ProductEnvelope,
    pub object: PreparedCasObject,
    pub root: ProductRoot,
    pub dependencies: Vec<ObjectId>,
    pub availability: Vec<ObjectId>,
    pub existing: bool,
}
struct NoObjectReferences;
impl ProductReferenceResolver for NoObjectReferences {
    fn classify(&self, _: &ObjectId) -> ProductReferenceClassification {
        ProductReferenceClassification::Missing
    }
}
pub(crate) fn product_root(product: &ProductEnvelope) -> Option<ProductRoot> {
    let object_id = product.object_id().clone();
    Some(match product {
        ProductEnvelope::Layer { product_id, .. } => ProductRoot::Layer {
            logical_id: product_id.clone(),
            object_id,
        },
        ProductEnvelope::Marker { product_id, .. } => ProductRoot::Marker {
            logical_id: product_id.clone(),
            object_id,
        },
        ProductEnvelope::Release { product_id, .. } => ProductRoot::Release {
            logical_id: product_id.clone(),
            object_id,
        },
        ProductEnvelope::Weave { product_id, .. } => ProductRoot::Weave {
            logical_id: product_id.clone(),
            object_id,
        },
        ProductEnvelope::Decision { product_id, .. } => ProductRoot::Decision {
            logical_id: product_id.clone(),
            object_id,
        },
        _ => return None,
    })
}
fn kind_and_space(product: &ProductEnvelope) -> Option<(&'static str, &SpaceId)> {
    match product {
        ProductEnvelope::Layer {
            record,
            previous_product_object_id,
            ..
        } if previous_product_object_id.is_none() => Some(("layer", record.space_id())),
        ProductEnvelope::Marker { record, .. } => Some(("marker", record.space_id())),
        ProductEnvelope::Release { record, .. } => Some(("release", record.space_id())),
        ProductEnvelope::Decision { record, .. } => Some(("decision", &record.space_id)),
        ProductEnvelope::Weave { record, .. }
            if record.gate_snapshot_id.is_none()
                && matches!(
                    record.outcome,
                    WeaveOutcome::Clean | WeaveOutcome::Conflicted
                ) =>
        {
            Some(("weave", &record.space_id))
        }
        _ => None,
    }
}
pub(crate) async fn prepare_products(
    repository: &PgV3Repository,
    space: &SpaceId,
    products: &[ProductEnvelope],
    graph: &StepDag,
    new_steps: &std::collections::BTreeSet<StepId>,
    steps: &BTreeMap<StepId, Step>,
) -> V3StoreResult<Result<Vec<PreparedProduct>, Rejection>> {
    let mut prepared = Vec::new();
    let mut overlay = BTreeMap::<String, ObjectId>::new();
    for product in products {
        let Some((kind, record_space)) = kind_and_space(product) else {
            return Ok(Err(reject(
                RejectionCode::IncompatibleVersion,
                "only initial Layer definitions and immutable Markers/Releases are admitted by this transaction endpoint",
            )));
        };
        if record_space != space {
            return Ok(Err(reject(
                RejectionCode::PermissionDenied,
                "Product belongs to another Space",
            )));
        }
        if product.verify_identity().is_err()
            || product
                .verify_dependency_projection(&NoObjectReferences)
                .is_err()
        {
            return Ok(Err(reject(
                RejectionCode::ImmutableObjectMismatch,
                "Product identity or exact dependency projection is invalid",
            )));
        }
        if product
            .dependencies()
            .0
            .iter()
            .any(|step| !graph.contains(step))
        {
            return Ok(Err(reject(
                RejectionCode::MissingParent,
                "Product Step dependency is unavailable in this Space",
            )));
        }
        let mut closure_availability = Vec::new();
        if let ProductEnvelope::Weave { record, .. } = product {
            let result = steps
                .get(&record.result_step_id)
                .ok_or(V3StoreError::NotFound)?;
            let closure = super::closure::verify_tree_closure(
                repository,
                space,
                vec![ObjectId::parse(result.root_tree_id().as_str()).expect("typed Tree ID")],
                steps,
                new_steps,
            )
            .await?;
            if closure.conflicts.iter().cloned().collect::<Vec<_>>() != record.conflict_ids
                || (closure.conflicts.is_empty() != (record.outcome == WeaveOutcome::Clean))
            {
                return Ok(Err(reject(
                    RejectionCode::ImmutableObjectMismatch,
                    "Weave outcome and conflicts differ from exact result closure",
                )));
            }
            match weave::validate(repository, record, &closure, steps).await {
                Ok(()) => {}
                Err(
                    V3StoreError::NotFound
                    | V3StoreError::IdentityMismatch
                    | V3StoreError::CorruptObject(_)
                    | V3StoreError::InvalidDescriptor(_),
                ) => {
                    return Ok(Err(reject(
                        RejectionCode::ImmutableObjectMismatch,
                        "Weave host, Lens executions or origin contract failed verification",
                    )));
                }
                Err(error) => return Err(error),
            }
            closure_availability.extend(closure.objects.into_keys());
            for heads in [&record.source_heads, &record.target_heads_before] {
                for (i, head) in heads.iter().enumerate() {
                    for other in heads.iter().skip(i + 1) {
                        if graph.is_ancestor(head, other).unwrap_or(true)
                            || graph.is_ancestor(other, head).unwrap_or(true)
                        {
                            return Ok(Err(reject(
                                RejectionCode::ImmutableObjectMismatch,
                                "Weave head set is not an antichain",
                            )));
                        }
                    }
                }
            }
            let parents: std::collections::BTreeSet<_> = record
                .source_heads
                .iter()
                .chain(&record.target_heads_before)
                .cloned()
                .collect();
            let parents: Vec<_> = parents.into_iter().collect();
            let bases: Option<Vec<_>> = record
                .merge_bases
                .iter()
                .map(|base| match base {
                    MergeBase::Step { step_id, .. } => Some(step_id.clone()),
                    _ => None,
                })
                .collect();
            if layrs_store::scm::CanonicalProduct::from_envelope(product.clone()).is_err()
                || graph.parents(&record.result_step_id).ok() != Some(parents.clone())
                || graph.merge_bases(&parents).ok() != bases
            {
                return Ok(Err(reject(
                    RejectionCode::ImmutableObjectMismatch,
                    "Weave receipt identity, result parents or merge bases disagree with the admitted DAG",
                )));
            }
        }
        let rows = sqlx::query(
            "SELECT space_id,product_object_id FROM products WHERE logical_id=$1 LIMIT 2",
        )
        .bind(product.logical_id())
        .fetch_all(repository.pool())
        .await
        .map_err(pg_error)?;
        if rows.iter().any(|r| {
            r.get::<String, _>("space_id") != space.as_str()
                || r.get::<String, _>("product_object_id") != product.object_id().as_str()
        }) {
            return Ok(Err(reject(
                RejectionCode::DuplicateIdentity,
                "logical Product ID is already bound to another immutable record",
            )));
        }
        let existing = !rows.is_empty();
        let mut availability = closure_availability;
        if existing {
            let descriptor =
                load_space_descriptor(repository.pool(), space, product.object_id()).await?;
            repository.cas().verify_existing(&descriptor).await?;
            availability.push(product.object_id().clone());
        }
        for step in product.dependencies().0 {
            if !new_steps.contains(step) {
                let id = ObjectId::parse(step.as_str()).expect("typed Step ID");
                let descriptor = load_space_descriptor(repository.pool(), space, &id).await?;
                let bytes =
                    read_canonical_object(repository.cas().as_ref(), &descriptor, 64 * 1024 * 1024)
                        .await?;
                let record: Step = serde_json::from_slice(&bytes)
                    .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
                if descriptor.kind != ObjectKind::Step || record.id() != step {
                    return Err(V3StoreError::IdentityMismatch);
                }
                availability.push(id);
            }
        }
        let mut dependencies = Vec::new();
        for logical in product.dependencies().1 {
            let id = if let Some(id) = overlay.get(logical.as_str()) {
                Some(id.clone())
            } else {
                let found:Option<String>=sqlx::query_scalar("SELECT p.product_object_id FROM products p LEFT JOIN current_layer_definitions d ON d.space_id=p.space_id AND d.layer_id=p.logical_id WHERE p.space_id=$1 AND p.logical_id=$2 AND (p.product_kind='proof' OR (p.product_kind='layer' AND d.current_product_object_id=p.product_object_id))")
                    .bind(space.as_str()).bind(logical.as_str()).fetch_optional(repository.pool()).await.map_err(pg_error)?;
                found
                    .map(ObjectId::parse)
                    .transpose()
                    .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?
            };
            let Some(id) = id else {
                return Ok(Err(reject(
                    RejectionCode::MissingObject,
                    "Product dependency is not admitted before its dependent",
                )));
            };
            if !overlay.values().any(|v| v == &id) {
                let descriptor = load_space_descriptor(repository.pool(), space, &id).await?;
                let bytes =
                    read_canonical_object(repository.cas().as_ref(), &descriptor, 64 * 1024 * 1024)
                        .await?;
                let dependency: ProductEnvelope = serde_json::from_slice(&bytes)
                    .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
                dependency
                    .verify_descriptor(&descriptor)
                    .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
                dependency
                    .verify_canonical_wire(&bytes)
                    .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
                if dependency.logical_id() != logical.as_str() {
                    return Err(V3StoreError::CorruptObject(
                        "Product dependency identity disagrees with catalog".into(),
                    ));
                }
                availability.push(id.clone());
            }
            dependencies.push(id);
        }
        dependencies.sort();
        dependencies.dedup();
        let bytes = serde_jcs::to_vec(product).map_err(|e| V3StoreError::Backend(e.to_string()))?;
        let descriptor = ObjectDescriptor {
            kind: ObjectKind::Product,
            object_id: product.object_id().clone(),
            canonical_byte_len: bytes.len() as u64,
            encoded_byte_len: bytes.len() as u64,
            compression: Compression::Identity,
        };
        repository
            .cas()
            .put_if_absent(
                &descriptor,
                Box::pin(stream::once(async move { Ok(Bytes::from(bytes)) })),
            )
            .await?;
        let object = repository.cas().verify_existing(&descriptor).await?;
        if object.descriptor.object_id != *product.object_id() {
            return Err(V3StoreError::IdentityMismatch);
        }
        overlay.insert(product.logical_id().into(), product.object_id().clone());
        prepared.push(PreparedProduct {
            envelope: product.clone(),
            object,
            root: product_root(product).expect("supported kind"),
            dependencies,
            availability,
            existing,
        });
        let _ = kind;
    }
    Ok(Ok(prepared))
}

/// Called while the Space is locked. Revalidation handles concurrent publication
/// of an immutable logical ID even when that transaction changed no View.
pub(crate) async fn recheck_products(
    tx: &mut Transaction<'_, Postgres>,
    space: &SpaceId,
    products: &mut [PreparedProduct],
) -> V3StoreResult<Option<Rejection>> {
    let logical_ids: std::collections::BTreeSet<_> = products
        .iter()
        .map(|p| p.envelope.logical_id().to_owned())
        .collect();
    for id in logical_ids {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,5))")
            .bind(format!("product:{id}"))
            .execute(&mut **tx)
            .await
            .map_err(pg_error)?;
    }
    for product in products {
        let rows = sqlx::query(
            "SELECT space_id,product_object_id FROM products WHERE logical_id=$1 LIMIT 2",
        )
        .bind(product.envelope.logical_id())
        .fetch_all(&mut **tx)
        .await
        .map_err(pg_error)?;
        if rows.iter().any(|r| {
            r.get::<String, _>("space_id") != space.as_str()
                || r.get::<String, _>("product_object_id") != product.envelope.object_id().as_str()
        }) {
            return Ok(Some(reject(
                RejectionCode::DuplicateIdentity,
                "concurrent Product publication bound this logical ID differently",
            )));
        }
        product.existing = !rows.is_empty();
        if product.existing && !product.availability.contains(product.envelope.object_id()) {
            product
                .availability
                .push(product.envelope.object_id().clone());
        }
        for id in &product.availability {
            let exists:Option<String>=sqlx::query_scalar("SELECT a.object_id FROM space_object_availability a JOIN canonical_objects o ON o.object_id=a.object_id WHERE a.space_id=$1 AND a.object_id=$2 AND a.availability_state='available' AND o.verification_state='verified' FOR SHARE OF a,o")
                .bind(space.as_str()).bind(id.as_str()).fetch_optional(&mut **tx).await.map_err(pg_error)?;
            if exists.is_none() {
                return Ok(Some(reject(
                    RejectionCode::MissingObject,
                    "Product dependency was quarantined before admission",
                )));
            }
        }
    }
    Ok(None)
}

pub(crate) async fn insert_products(
    tx: &mut Transaction<'_, Postgres>,
    catalog: &PgCatalogStore,
    space: &SpaceId,
    products: &[PreparedProduct],
) -> V3StoreResult<Vec<ProductRoot>> {
    let mut accepted = Vec::new();
    for product in products {
        if product.existing {
            continue;
        }
        catalog.install_prepared(tx, &product.object).await?;
        catalog
            .make_available(tx, space, product.envelope.object_id())
            .await?;
        let (kind, _) = kind_and_space(&product.envelope).expect("prepared supported kind");
        sqlx::query("INSERT INTO products(product_object_id,space_id,product_kind,logical_id,canonical_envelope_jcs) VALUES($1,$2,$3,$4,$5)")
            .bind(product.envelope.object_id().as_str()).bind(space.as_str()).bind(kind).bind(product.envelope.logical_id()).bind(serde_jcs::to_string(&product.envelope).map_err(|e|V3StoreError::Backend(e.to_string()))?).execute(&mut **tx).await.map_err(pg_error)?;
        for (ordinal, step) in product.envelope.dependencies().0.iter().enumerate() {
            sqlx::query("INSERT INTO product_step_dependencies(space_id,product_object_id,ordinal,step_id) VALUES($1,$2,$3,$4)").bind(space.as_str()).bind(product.envelope.object_id().as_str()).bind(ordinal as i32).bind(step.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
        }
        for (ordinal, id) in product.dependencies.iter().enumerate() {
            sqlx::query("INSERT INTO product_dependencies(space_id,product_object_id,ordinal,dependency_product_object_id) VALUES($1,$2,$3,$4)").bind(space.as_str()).bind(product.envelope.object_id().as_str()).bind(ordinal as i32).bind(id.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
        }
        accepted.push(product.root.clone());
    }
    accepted.sort();
    Ok(accepted)
}
pub(crate) async fn install_layer_definitions(
    tx: &mut Transaction<'_, Postgres>,
    space: &SpaceId,
    products: &[PreparedProduct],
    transaction: &TransactionId,
) -> V3StoreResult<()> {
    for product in products {
        if !product.existing {
            if let ProductEnvelope::Layer { product_id, .. } = &product.envelope {
                sqlx::query("SELECT layrs_cas_layer_definition($1,$2,NULL,0,$3,$4)")
                    .bind(space.as_str())
                    .bind(product_id.as_str())
                    .bind(product.envelope.object_id().as_str())
                    .bind(transaction.as_str())
                    .execute(&mut **tx)
                    .await
                    .map_err(pg_error)?;
            }
        }
    }
    Ok(())
}
