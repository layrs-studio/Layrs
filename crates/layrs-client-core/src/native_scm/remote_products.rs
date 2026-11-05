use super::{NativeRemoteError, NativeWorkspace, WorkspaceState};
use layrs_core::scm::*;
use layrs_store::scm::ScmReader;
use layrs_store_local::scm_v1::NativeScmReader;
use layrs_sync::v3::*;
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, NativeRemoteError>;

pub(super) fn prepare(
    workspace: &NativeWorkspace,
    state: &WorkspaceState,
    remote: &SpaceBootstrap,
) -> Result<Vec<ProductEnvelope>> {
    let placeholder = ObjectId::parse(format!("blake3:{}", "0".repeat(64)))?;
    let mut candidates = BTreeMap::new();
    for layer in &state.layers {
        if let Some(existing) = remote.layers.iter().find(|p| p.record.id() == layer.id()) {
            if existing.record != *layer {
                return Err(NativeRemoteError::Invalid(
                    "remote Layer definition differs; revision sync is not implemented",
                ));
            }
            continue;
        }
        let product = seal(ProductEnvelope::Layer {
            product_id: layer.id().clone(),
            product_object_id: placeholder.clone(),
            depends_on_step_ids: vec![],
            depends_on_product_ids: layer
                .parent_layer_ids()
                .iter()
                .map(|id| ProductLogicalId::parse(id.as_str()).expect("typed Layer ID"))
                .collect(),
            previous_product_object_id: Nullable::null(),
            record: layer.clone(),
        })?;
        candidates.insert(product.logical_id().to_owned(), product);
    }
    let store = workspace.open_store()?;
    let reader = NativeScmReader::new(&store)?;
    for id in reader.products_in_space(state.space.id())? {
        let product = reader
            .get_product(&id)?
            .ok_or(NativeRemoteError::Invalid("missing local Product"))?;
        if matches!(product.envelope(), ProductEnvelope::Weave { .. })
            && !remote
                .product_roots
                .iter()
                .any(|root| root.object_id() == &id)
        {
            candidates.insert(
                product.envelope().logical_id().into(),
                product.envelope().clone(),
            );
        }
    }
    for id in state.view.marker_ids() {
        let record = reader
            .get_marker(id)?
            .ok_or(NativeRemoteError::Invalid("missing local Marker"))?;
        if let Some(existing) = remote.markers.iter().find(|p| p.record.id() == id) {
            if existing.record != record {
                return Err(NativeRemoteError::Invalid(
                    "Marker immutable identity differs remotely",
                ));
            }
            continue;
        }
        let product = seal(ProductEnvelope::Marker {
            product_id: record.id().clone(),
            product_object_id: placeholder.clone(),
            depends_on_step_ids: vec![record.target_step_id().clone()],
            depends_on_product_ids: record
                .proof_ids()
                .iter()
                .map(|id| ProductLogicalId::parse(id.as_str()).expect("typed Proof ID"))
                .collect(),
            record,
        })?;
        candidates.insert(product.logical_id().into(), product);
    }
    for id in state.view.release_ids() {
        let record = reader
            .get_release(id)?
            .ok_or(NativeRemoteError::Invalid("missing local Release"))?;
        if let Some(existing) = remote.releases.iter().find(|p| p.record.id() == id) {
            if existing.record != record {
                return Err(NativeRemoteError::Invalid(
                    "Release immutable identity differs remotely",
                ));
            }
            continue;
        }
        let product = seal(ProductEnvelope::Release {
            product_id: record.id().clone(),
            product_object_id: placeholder.clone(),
            depends_on_step_ids: vec![record.target_step_id().clone()],
            depends_on_product_ids: record
                .proof_ids()
                .iter()
                .map(|id| ProductLogicalId::parse(id.as_str()).expect("typed Proof ID"))
                .collect(),
            record,
        })?;
        candidates.insert(product.logical_id().into(), product);
    }
    let mut ordered = Vec::new();
    while !candidates.is_empty() {
        let ready = candidates
            .iter()
            .find(|(_, p)| {
                p.dependencies()
                    .1
                    .iter()
                    .all(|id| !candidates.contains_key(id.as_str()))
            })
            .map(|(id, _)| id.clone())
            .ok_or(NativeRemoteError::Invalid("cyclic Product dependencies"))?;
        ordered.push(candidates.remove(&ready).expect("selected Product"));
    }
    Ok(ordered)
}
fn seal(mut product: ProductEnvelope) -> Result<ProductEnvelope> {
    let id = product
        .computed_object_id()
        .map_err(|_| NativeRemoteError::Invalid("invalid Product"))?;
    match &mut product {
        ProductEnvelope::Layer {
            product_object_id, ..
        }
        | ProductEnvelope::Marker {
            product_object_id, ..
        }
        | ProductEnvelope::Release {
            product_object_id, ..
        } => *product_object_id = id,
        _ => unreachable!(),
    }
    Ok(product)
}
pub(super) fn apply_projections(observed: &mut SpaceBootstrap, products: &[ProductEnvelope]) {
    for product in products {
        match product {
            ProductEnvelope::Layer {
                record,
                product_object_id,
                ..
            } => {
                if !observed.layers.iter().any(|p| p.record.id() == record.id()) {
                    observed.layers.push(LayerProjection {
                        product_object_id: product_object_id.clone(),
                        record: record.clone(),
                        head_step_ids: vec![],
                    });
                }
            }
            ProductEnvelope::Marker {
                record,
                product_object_id,
                ..
            } => {
                if !observed
                    .markers
                    .iter()
                    .any(|p| p.record.id() == record.id())
                {
                    observed.markers.push(MarkerProjection {
                        product_object_id: product_object_id.clone(),
                        record: record.clone(),
                    });
                }
            }
            ProductEnvelope::Release {
                record,
                product_object_id,
                ..
            } => {
                if !observed
                    .releases
                    .iter()
                    .any(|p| p.record.id() == record.id())
                {
                    observed.releases.push(ReleaseProjection {
                        product_object_id: product_object_id.clone(),
                        record: record.clone(),
                    });
                }
            }
            _ => (),
        }
    }
    observed
        .layers
        .sort_by(|a, b| a.record.id().cmp(b.record.id()));
    observed
        .markers
        .sort_by(|a, b| a.record.id().cmp(b.record.id()));
    observed
        .releases
        .sort_by(|a, b| a.record.id().cmp(b.record.id()));
}
