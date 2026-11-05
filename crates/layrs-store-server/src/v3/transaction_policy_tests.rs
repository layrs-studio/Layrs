use super::*;
use layrs_core::scm::Timestamp;

fn initial() -> ProductEnvelope {
    let id = ObjectId::parse(format!("blake3:{}", "a".repeat(64))).unwrap();
    let mut value = ProductEnvelope::Policy {
        product_id: PolicyId::parse("policy:versioned").unwrap(),
        product_object_id: id.clone(),
        depends_on_step_ids: vec![],
        depends_on_product_ids: vec![],
        previous_product_object_id: Nullable::null(),
        record: PolicyRecord {
            schema_version: PolicySchemaVersion::V1,
            policy_id: PolicyId::parse("policy:versioned").unwrap(),
            version_id: PolicyVersionId::parse("policy-version:first").unwrap(),
            space_id: SpaceId::parse("space:versioned").unwrap(),
            rules_object_id: id,
            created_at: Timestamp::parse("2026-09-21T00:00:00Z").unwrap(),
        },
    };
    rehash(&mut value);
    value
}
fn rehash(p: &mut ProductEnvelope) {
    let id = p.computed_object_id().unwrap();
    if let ProductEnvelope::Policy {
        product_object_id, ..
    } = p
    {
        *product_object_id = id;
    }
}
fn revision(parent: &ProductEnvelope) -> ProductEnvelope {
    let mut value = parent.clone();
    if let ProductEnvelope::Policy {
        previous_product_object_id,
        depends_on_product_ids,
        record,
        ..
    } = &mut value
    {
        *previous_product_object_id = Nullable::some(parent.object_id().clone());
        *depends_on_product_ids = vec![ProductLogicalId::parse(parent.logical_id()).unwrap()];
        record.version_id = PolicyVersionId::parse("policy-version:second").unwrap();
    }
    rehash(&mut value);
    value
}

#[test]
fn policy_predecessor_requires_exact_identity_space_logical_id_and_distinct_version() {
    let parent = initial();
    let revision = revision(&parent);
    validate_predecessor(&revision, &parent).unwrap();
    for case in 0..5 {
        let mut bad = revision.clone();
        if let ProductEnvelope::Policy {
            previous_product_object_id,
            depends_on_product_ids,
            record,
            product_id,
            ..
        } = &mut bad
        {
            match case {
                0 => *previous_product_object_id = Nullable::null(),
                1 => depends_on_product_ids.clear(),
                2 => record.space_id = SpaceId::parse("space:foreign").unwrap(),
                3 => {
                    record.policy_id = PolicyId::parse("policy:foreign").unwrap();
                    *product_id = record.policy_id.clone();
                }
                4 => record.version_id = PolicyVersionId::parse("policy-version:first").unwrap(),
                _ => unreachable!(),
            }
        }
        rehash(&mut bad);
        assert!(validate_predecessor(&bad, &parent).is_err());
    }
    let mut corrupt = parent.clone();
    if let ProductEnvelope::Policy { record, .. } = &mut corrupt {
        record.created_at = Timestamp::parse("2026-09-21T01:00:00Z").unwrap();
    }
    assert!(validate_predecessor(&revision, &corrupt).is_err());
}
