use super::*;
use layrs_sync::v3::ObjectKind;

/// External edges only. Placement, term and block IDs name records embedded in
/// the Conflict; they are never independent CAS objects. None denotes a FileId
/// (resolved through a verified manifest) or an untyped canonical object edge.
pub fn conflict_object_references(
    bytes: &[u8],
) -> V3StoreResult<Vec<(ObjectId, Option<ObjectKind>)>> {
    let conflict: ConflictObject = serde_json::from_slice(bytes)
        .map_err(|error| V3StoreError::CorruptObject(error.to_string()))?;
    conflict.validate().map_err(V3StoreError::CorruptObject)?;
    let mut out = Vec::new();
    for id in &conflict.origin_step_ids {
        out.push((
            ObjectId::parse(id.as_str()).expect("typed Step ID"),
            Some(ObjectKind::Step),
        ));
    }
    for base in &conflict.bases {
        if let BaseProvenance::Virtual { result, .. } = &base.provenance {
            out.push((
                result.object_id.clone(),
                match result.kind {
                    VirtualBaseResultKind::Tree => Some(ObjectKind::Tree),
                    VirtualBaseResultKind::Object => None,
                },
            ));
        }
        for placed in &base.values {
            value(&placed.value, &mut out);
        }
    }
    for side in &conflict.sides {
        for placed in &side.values {
            value(&placed.value, &mut out);
        }
    }
    for resolution in &conflict.placement_resolutions {
        value(&resolution.resolution.result, &mut out);
    }
    for block in &conflict.blocks {
        for term in block.base_values.iter().chain(&block.side_values) {
            value(&term.value, &mut out);
        }
        if let Some(resolution) = &block.resolution {
            value(&resolution.result, &mut out);
        }
    }
    for segment in &conflict.segments {
        if let Segment::Content { object_id } = segment {
            out.push((object_id.clone(), None));
        }
    }
    if let Some(id) = conflict.rendered_object_id {
        out.push((id, None));
    }
    Ok(out)
}

fn value(value: &ConflictValue, out: &mut Vec<(ObjectId, Option<ObjectKind>)>) {
    let (id, kind) = match value {
        ConflictValue::Absent => return,
        ConflictValue::File { object_id, .. } => (object_id, None),
        ConflictValue::Tree { object_id, .. } => (object_id, Some(ObjectKind::Tree)),
        ConflictValue::Symlink { object_id, .. } => (object_id, Some(ObjectKind::Symlink)),
    };
    out.push((id.clone(), kind));
}
