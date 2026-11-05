//! PostgreSQL admission of immutable policy revisions, including physical closure.
use super::*;

fn rehash(product: &mut ProductEnvelope) {
    let id = product.computed_object_id().unwrap();
    if let ProductEnvelope::Policy {
        product_object_id, ..
    } = product
    {
        *product_object_id = id;
    }
}
fn revision(previous: &ProductEnvelope, version: &str) -> ProductEnvelope {
    let mut product = previous.clone();
    if let ProductEnvelope::Policy {
        previous_product_object_id,
        depends_on_product_ids,
        record,
        ..
    } = &mut product
    {
        *previous_product_object_id = Nullable::some(previous.object_id().clone());
        *depends_on_product_ids = vec![ProductLogicalId::parse(previous.logical_id()).unwrap()];
        record.version_id = PolicyVersionId::parse(format!("policy-version:{version}")).unwrap();
        record.created_at = Timestamp::parse("2026-09-21T00:00:00Z").unwrap();
    }
    rehash(&mut product);
    product
}
fn inert(
    base: &SpaceBootstrap,
    seed: &Step,
    product: &ProductEnvelope,
    label: &str,
) -> SyncTransactionRequest {
    let mut value = mutation(base, seed, &base.current_policy_set.record, label);
    value.policy_root_mutation = None;
    value.products = vec![product.clone()];
    bind(&mut value, label);
    value
}

pub async fn run(
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    repository: &PgV3Repository,
    seed: &Step,
    old: &ProductEnvelope,
) {
    let ProductEnvelope::Policy {
        record: old_record, ..
    } = old
    else {
        unreachable!()
    };
    let base = facade
        .bootstrap(principal, &old_record.space_id)
        .await
        .unwrap();
    let good = revision(old, "revision-second");
    for (index, mut bad) in (0..3).map(|i| (i, good.clone())) {
        if let ProductEnvelope::Policy {
            record,
            product_id,
            depends_on_product_ids,
            ..
        } = &mut bad
        {
            match index {
                0 => {
                    let ProductEnvelope::Policy {
                        record: old_record, ..
                    } = old
                    else {
                        unreachable!()
                    };
                    record.version_id = old_record.version_id.clone();
                }
                1 => {
                    record.policy_id = PolicyId::parse("policy:foreign-revision").unwrap();
                    *product_id = record.policy_id.clone();
                }
                2 => depends_on_product_ids.clear(),
                _ => unreachable!(),
            }
        }
        rehash(&mut bad);
        denied(
            facade,
            principal,
            inert(&base, seed, &bad, &format!("revision-bad-{index}")),
        )
        .await;
    }
    // CAS bytes are not admission. A valid uploaded predecessor alone grants nothing.
    let mut uploaded = old.clone();
    if let ProductEnvelope::Policy {
        record, product_id, ..
    } = &mut uploaded
    {
        record.policy_id = PolicyId::parse("policy:uploaded-unadmitted").unwrap();
        *product_id = record.policy_id.clone();
    }
    rehash(&mut uploaded);
    upload(
        facade,
        principal,
        &base.space_id,
        ObjectKind::Product,
        uploaded.object_id().as_str(),
        uploaded.canonical_bytes().unwrap(),
        "revision-unadmitted-parent",
    )
    .await;
    let orphan = revision(&uploaded, "orphan-second");
    denied(
        facade,
        principal,
        inert(&base, seed, &orphan, "revision-parent-unadmitted"),
    )
    .await;

    let request = inert(&base, seed, &good, "revision-inert-accepted");
    let receipt = facade.transact(principal, request.clone()).await.unwrap();
    assert_eq!(receipt.receipt().status, ReceiptStatus::Applied);
    assert_eq!(
        facade.transact(principal, request).await.unwrap().receipt(),
        receipt.receipt()
    );
    let after = facade.bootstrap(principal, &base.space_id).await.unwrap();
    assert_eq!(after.current_policy_set_id, base.current_policy_set_id);
    assert_eq!(after.authorization_epoch, base.authorization_epoch);
    let dependencies: Vec<String> = sqlx::query_scalar("SELECT dependency_product_object_id FROM product_dependencies WHERE space_id=$1 AND product_object_id=$2 ORDER BY ordinal")
        .bind(base.space_id.as_str()).bind(good.object_id().as_str()).fetch_all(repository.pool()).await.unwrap();
    assert_eq!(dependencies, vec![old.object_id().as_str()]);

    let mut set = base.current_policy_set.record.clone();
    let ProductEnvelope::Policy { record, .. } = &good else {
        unreachable!()
    };
    let entry = set
        .policy_versions
        .iter_mut()
        .find(|e| e.policy_id == record.policy_id)
        .unwrap();
    entry.policy_version_id = record.version_id.clone();
    entry.policy_product_object_id = good.object_id().clone();
    entry.dependency_object_ids = vec![record.rules_object_id.clone()];
    set.policy_set_id = set.computed_id().unwrap();
    denied(
        facade,
        principal,
        mutation(&base, seed, &set, "revision-closure-incomplete"),
    )
    .await;
    let entry = set
        .policy_versions
        .iter_mut()
        .find(|e| e.policy_id == record.policy_id)
        .unwrap();
    entry.dependency_object_ids.push(old.object_id().clone());
    entry.dependency_object_ids.sort();
    set.policy_set_id = set.computed_id().unwrap();
    let request = mutation(&base, seed, &set, "revision-activate");
    let receipt = facade.transact(principal, request.clone()).await.unwrap();
    assert_eq!(receipt.receipt().status, ReceiptStatus::Applied);
    assert_eq!(
        facade.transact(principal, request).await.unwrap().receipt(),
        receipt.receipt()
    );
    let final_state = facade.bootstrap(principal, &base.space_id).await.unwrap();
    assert_eq!(final_state.current_policy_set_id, set.policy_set_id);
    assert_eq!(
        final_state.authorization_epoch,
        base.authorization_epoch + 1
    );
    let old_bytes: String = sqlx::query_scalar(
        "SELECT canonical_envelope_jcs FROM products WHERE product_object_id=$1",
    )
    .bind(old.object_id().as_str())
    .fetch_one(repository.pool())
    .await
    .unwrap();
    assert_eq!(old_bytes.as_bytes(), old.canonical_bytes().unwrap());
    eprintln!(
        "Policy revision: exact admitted predecessor, immutable version, inert admission, activation closure and replay PASS"
    );
}
