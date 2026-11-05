//! Typed Product edges. Embedded receipt/profile/execution identities are not
//! independent objects and free-form names/reasons never create CAS edges.
use layrs_sync::v3::*;
pub(super) type References = Vec<(String, Option<ObjectKind>)>;

pub(super) fn product_references(product: &ProductEnvelope) -> References {
    let mut out = product
        .dependencies()
        .0
        .iter()
        .map(|id| (id.as_str().to_owned(), Some(ObjectKind::Step)))
        .collect::<References>();
    match product {
        ProductEnvelope::Layer {
            previous_product_object_id,
            ..
        }
        | ProductEnvelope::GateDefinition {
            previous_product_object_id,
            ..
        }
        | ProductEnvelope::Policy {
            previous_product_object_id,
            ..
        } => {
            if let Some(id) = previous_product_object_id.as_ref() {
                out.push((id.as_str().into(), Some(ObjectKind::Product)));
            }
        }
        _ => {}
    }
    match product {
        ProductEnvelope::Proof { record, .. } => {
            subject(&record.subject, &mut out);
            out.push((
                record.recipe_object_id.as_str().into(),
                Some(ObjectKind::Evidence),
            ));
            out.extend(
                record
                    .evidence_object_ids
                    .iter()
                    .map(|id| (id.as_str().into(), Some(ObjectKind::Evidence))),
            );
            out.extend(
                record
                    .policy_version_object_ids
                    .iter()
                    .map(|id| (id.as_str().into(), Some(ObjectKind::Product))),
            );
        }
        ProductEnvelope::GateDefinition { record, .. } => {
            out.extend(
                record
                    .recipe_object_ids
                    .iter()
                    .map(|id| (id.as_str().into(), Some(ObjectKind::Evidence))),
            );
            out.extend(
                record
                    .policy_version_object_ids
                    .iter()
                    .map(|id| (id.as_str().into(), Some(ObjectKind::Product))),
            );
        }
        ProductEnvelope::GateEvaluation { record, .. } => {
            subject(&record.subject, &mut out);
            out.push((
                record.gate_definition_object_id.as_str().into(),
                Some(ObjectKind::Product),
            ));
            out.extend(
                record
                    .proof_object_ids
                    .iter()
                    .chain(&record.policy_version_object_ids)
                    .map(|id| (id.as_str().into(), Some(ObjectKind::Product))),
            );
        }
        ProductEnvelope::Policy { record, .. } => {
            out.push((
                record.rules_object_id.as_str().into(),
                Some(ObjectKind::Evidence),
            ));
        }
        ProductEnvelope::Weave { record, .. } => {
            out.extend(
                record
                    .conflict_ids
                    .iter()
                    .map(|id| (id.as_str().into(), Some(ObjectKind::Conflict))),
            );
            for base in &record.merge_bases {
                if let MergeBase::Virtual { result, .. } = base {
                    out.push((
                        result.object_id.as_str().into(),
                        match result.kind {
                            VirtualBaseResultKind::Tree => Some(ObjectKind::Tree),
                            VirtualBaseResultKind::Object => None,
                        },
                    ));
                }
            }
            if let Some(id) = record.gate_snapshot_id.as_ref() {
                out.push((id.as_str().into(), None));
            }
            for execution in &record.lens_executions {
                if let Some(id) = execution.conflict_id.as_ref() {
                    out.push((id.as_str().into(), Some(ObjectKind::Conflict)));
                }
            }
        }
        ProductEnvelope::Layer { .. }
        | ProductEnvelope::Marker { .. }
        | ProductEnvelope::Release { .. }
        | ProductEnvelope::Decision { .. } => {}
    }
    out
}

fn subject(subject: &ImmutableSubject, out: &mut References) {
    let (id, kind) = match subject {
        ImmutableSubject::Step { object_id } => (object_id, ObjectKind::Step),
        ImmutableSubject::Tree { object_id } => (object_id, ObjectKind::Tree),
        ImmutableSubject::Conflict { object_id } => (object_id, ObjectKind::Conflict),
        ImmutableSubject::Evidence { object_id } => (object_id, ObjectKind::Evidence),
        ImmutableSubject::Product { object_id } => (object_id, ObjectKind::Product),
        ImmutableSubject::ProductRoot { product } => (product.object_id(), ObjectKind::Product),
    };
    out.push((id.as_str().into(), Some(kind)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use layrs_core::scm::*;

    #[test]
    fn marker_text_that_looks_like_a_digest_is_not_a_cas_edge() {
        let target = StepId::parse(format!("blake3:{}", "1".repeat(64))).unwrap();
        let text = format!("blake3:{}", "2".repeat(64));
        let marker = Marker::new(
            MarkerId::parse("marker:00000000-0000-4000-8000-000000000001").unwrap(),
            SpaceId::parse("space:00000000-0000-4000-8000-000000000001").unwrap(),
            target.clone(),
            MarkerKind::Checkpoint,
            &text,
            &text,
            &text,
            None,
            Timestamp::parse("2026-09-20T00:00:00Z").unwrap(),
            vec![],
            Metadata::default(),
        )
        .unwrap();
        let product = ProductEnvelope::Marker {
            product_id: marker.id().clone(),
            product_object_id: ObjectId::parse(text).unwrap(),
            depends_on_step_ids: vec![target.clone()],
            depends_on_product_ids: vec![],
            record: marker,
        };
        assert_eq!(
            product_references(&product),
            vec![(target.as_str().into(), Some(ObjectKind::Step))]
        );
    }
}
