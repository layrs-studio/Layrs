use super::*;
fn object(n: usize) -> ObjectId {
    ObjectId::parse(format!("blake3:{n:064x}")).unwrap()
}
fn fixture(count: usize) -> (NativePolicyPreview, PolicySet, PolicyId) {
    let space = SpaceId::parse("space:replacement-test").unwrap();
    let mut base = PolicySet {
        schema_version: PolicySetSchemaVersion::V1,
        policy_set_id: object(1),
        space_id: space.clone(),
        policy_versions: (0..count)
            .map(|i| PolicyVersionEntry {
                policy_id: PolicyId::parse(format!("policy:test-{i:03}")).unwrap(),
                policy_version_id: PolicyVersionId::parse(format!("policy-version:old-{i}"))
                    .unwrap(),
                policy_product_object_id: object(i + 2),
                dependency_object_ids: vec![object(i + 200)],
            })
            .collect(),
    };
    base.policy_set_id = base.computed_id().unwrap();
    let mut request = SyncTransactionRequest {
        protocol: SyncV3Protocol::V3,
        message_type: TransactionRequestType::Request,
        space_id: space.clone(),
        client_id: ClientId::parse("client:replacement-test").unwrap(),
        idempotency_key: IdempotencyKey::parse("replacement-test-001").unwrap(),
        request_digest: object(1),
        known_event_cursor: None,
        steps: vec![],
        observations: vec![],
        products: vec![],
        view_mutation: None,
        policy_root_mutation: Some(PolicyRootMutation {
            mutation_id: MutationId::parse("mutation:replacement-test").unwrap(),
            expected_policy_set_id: base.policy_set_id.clone(),
            expected_authorization_epoch: 7,
            proposed_policy_set: ProposedPolicySet::Stored {
                policy_set_id: base.policy_set_id.clone(),
            },
        }),
    };
    request.request_digest = request.computed_request_digest().unwrap();
    let preview = NativePolicyPreview {
        schema_version: "layrs.policy-root-preview.v1".into(),
        origin: "https://example.test/".into(),
        principal_id: PrincipalId::parse("principal:admin").unwrap(),
        space_id: space,
        worktree_id: WorktreeId::parse("worktree:replacement-test").unwrap(),
        store_root: "test-store".into(),
        expected_policy_set_id: base.policy_set_id.clone(),
        expected_authorization_epoch: 7,
        proposed_policy_set_id: base.policy_set_id.clone(),
        request,
        inline_rules: None,
    };
    let target = base.policy_versions[0].policy_id.clone();
    (preview, base, target)
}
fn rules() -> NativePolicyRules {
    checked_rules(br#"{"schemaVersion":"layrs.space-policy-rules.v3","rules":[],"pathPolicies":[{"principalIds":["principal:member"],"read":{"default":"allow","rules":[]},"write":{"default":"deny","rules":[]}}]}"#).unwrap()
}
fn preview() -> NativePolicyPreview {
    let (p, b, id) = fixture(2);
    replace(p, b, &id, rules()).unwrap()
}
fn rehash_product(preview: &mut NativePolicyPreview) {
    let product = &mut preview.request.products[0];
    let object = product.computed_object_id().unwrap();
    if let ProductEnvelope::Policy {
        product_object_id, ..
    } = product
    {
        *product_object_id = object.clone();
    }
    if let ProposedPolicySet::Inline { policy_set } = &mut preview
        .request
        .policy_root_mutation
        .as_mut()
        .unwrap()
        .proposed_policy_set
    {
        policy_set.policy_versions[0].policy_product_object_id = object;
        policy_set.policy_set_id = policy_set.computed_id().unwrap();
        preview.proposed_policy_set_id = policy_set.policy_set_id.clone();
    }
    preview.request.request_digest = preview.request.computed_request_digest().unwrap();
}
#[test]
fn replaces_exactly_one_at_policy_limit_and_preserves_others() {
    let (p, b, id) = fixture(32);
    let result = replace(p, b.clone(), &id, rules()).unwrap();
    validate(&result).unwrap();
    let ProposedPolicySet::Inline { policy_set } = &result
        .request
        .policy_root_mutation
        .as_ref()
        .unwrap()
        .proposed_policy_set
    else {
        panic!()
    };
    assert_eq!(policy_set.policy_versions.len(), 32);
    assert_eq!(&policy_set.policy_versions[1..], &b.policy_versions[1..]);
    assert_eq!(policy_set.policy_versions[0].policy_id, id);
    assert_ne!(
        policy_set.policy_versions[0].policy_version_id,
        b.policy_versions[0].policy_version_id
    );
    assert_eq!(result.schema_version, REPLACE_SCHEMA);
    let ProductEnvelope::Policy {
        previous_product_object_id,
        depends_on_product_ids,
        ..
    } = &result.request.products[0]
    else {
        panic!()
    };
    assert_eq!(
        previous_product_object_id.as_ref(),
        Some(&b.policy_versions[0].policy_product_object_id)
    );
    assert_eq!(
        depends_on_product_ids,
        &[ProductLogicalId::parse(id.as_str()).unwrap()]
    );
}
#[test]
fn replacement_cannot_be_reinterpreted_as_old_additive_schema() {
    let mut p = preview();
    p.schema_version = SCHEMA.into();
    assert!(validate(&p).is_err());
    p.schema_version = REPLACE_SCHEMA.into();
    p.inline_rules.as_mut().unwrap().replacement = None;
    assert!(validate(&p).is_err());
}
#[test]
fn rehashed_forged_predecessor_and_same_version_are_rejected() {
    let mut p = preview();
    if let ProductEnvelope::Policy {
        previous_product_object_id,
        ..
    } = &mut p.request.products[0]
    {
        *previous_product_object_id = Nullable::some(object(999));
    }
    rehash_product(&mut p);
    assert!(validate(&p).is_err());
    let mut p = preview();
    let old = p
        .inline_rules
        .as_ref()
        .unwrap()
        .base_policy_set
        .policy_versions[0]
        .policy_version_id
        .clone();
    if let ProductEnvelope::Policy { record, .. } = &mut p.request.products[0] {
        record.version_id = old;
    }
    rehash_product(&mut p);
    assert!(validate(&p).is_err());
}
#[test]
fn missing_target_and_changed_unrelated_policy_are_rejected() {
    let (p, b, _) = fixture(2);
    assert!(replace(p, b, &PolicyId::parse("policy:missing").unwrap(), rules()).is_err());
    let mut p = preview();
    if let ProposedPolicySet::Inline { policy_set } = &mut p
        .request
        .policy_root_mutation
        .as_mut()
        .unwrap()
        .proposed_policy_set
    {
        policy_set.policy_versions.pop();
        policy_set.policy_set_id = policy_set.computed_id().unwrap();
        p.proposed_policy_set_id = policy_set.policy_set_id.clone();
    }
    p.request.request_digest = p.request.computed_request_digest().unwrap();
    assert!(validate(&p).is_err());
}
#[test]
fn replacement_rules_remain_closed_and_bounded() {
    assert!(checked_rules(br#"{"schemaVersion":"layrs.space-policy-rules.v3","rules":[],"pathPolicies":[],"request":{}}"#).is_err());
    assert!(checked_rules(&vec![b' '; MAX_RULES + 1]).is_err());
    let p = preview();
    let encoded = serde_json::to_vec(&p).unwrap();
    let roundtrip: NativePolicyPreview = serde_json::from_slice(&encoded).unwrap();
    validate(&roundtrip).unwrap();
    assert_eq!(roundtrip.request, p.request);
}
