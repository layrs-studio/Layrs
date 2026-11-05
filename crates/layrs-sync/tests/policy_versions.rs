use layrs_core::scm::{ObjectId, SpaceId, Timestamp};
use layrs_sync::v3::*;

fn product(name: &str, dependency: &str, previous: Option<ObjectId>) -> ProductEnvelope {
    let mut product = ProductEnvelope::Policy {
        product_id: PolicyId::parse(name).unwrap(),
        product_object_id: id('a'),
        depends_on_step_ids: vec![],
        depends_on_product_ids: vec![ProductLogicalId::parse(dependency).unwrap()],
        previous_product_object_id: Nullable(previous),
        record: PolicyRecord {
            schema_version: PolicySchemaVersion::V1,
            policy_id: PolicyId::parse(name).unwrap(),
            version_id: PolicyVersionId::parse("policy-version:new").unwrap(),
            space_id: SpaceId::parse("space:versions").unwrap(),
            rules_object_id: id('b'),
            created_at: Timestamp::parse("2026-09-21T00:00:00Z").unwrap(),
        },
    };
    let computed = product.computed_object_id().unwrap();
    if let ProductEnvelope::Policy {
        product_object_id, ..
    } = &mut product
    {
        *product_object_id = computed;
    }
    product
}
fn id(ch: char) -> ObjectId {
    ObjectId::parse(format!("blake3:{}", ch.to_string().repeat(64))).unwrap()
}
fn request(products: Vec<ProductEnvelope>) -> SyncTransactionRequest {
    let mut request = SyncTransactionRequest {
        protocol: SyncV3Protocol::V3,
        message_type: TransactionRequestType::Request,
        space_id: SpaceId::parse("space:versions").unwrap(),
        client_id: ClientId::parse("client:versions").unwrap(),
        idempotency_key: IdempotencyKey::parse("version-contract-00001").unwrap(),
        request_digest: id('a'),
        known_event_cursor: None,
        steps: vec![],
        observations: vec![],
        products,
        view_mutation: None,
        policy_root_mutation: None,
    };
    request.request_digest = request.computed_request_digest().unwrap();
    request
}

#[test]
fn prior_policy_version_is_not_an_in_batch_logical_self_cycle() {
    request(vec![product("policy:one", "policy:one", Some(id('c')))])
        .validate_v3()
        .unwrap();
    assert!(
        request(vec![product("policy:one", "policy:one", None)])
            .validate_v3()
            .is_err()
    );
    let mut self_object = product("policy:one", "policy:one", Some(id('c')));
    let object = self_object.object_id().clone();
    if let ProductEnvelope::Policy {
        previous_product_object_id,
        ..
    } = &mut self_object
    {
        *previous_product_object_id = Nullable::some(object);
    }
    assert_eq!(
        request(vec![self_object]).validate_v3().unwrap_err().code,
        "immutable_object_mismatch"
    );
}

#[test]
fn prior_version_exception_does_not_allow_other_in_batch_cycles_or_duplicate_versions() {
    let one = product("policy:one", "policy:two", Some(id('c')));
    let two = product("policy:two", "policy:one", Some(id('d')));
    assert_eq!(
        request(vec![one, two]).validate_v3().unwrap_err().code,
        "non_topological_products"
    );
    let one = product("policy:one", "policy:one", Some(id('c')));
    assert_eq!(
        request(vec![one.clone(), one])
            .validate_v3()
            .unwrap_err()
            .code,
        "duplicate_identity"
    );
}
