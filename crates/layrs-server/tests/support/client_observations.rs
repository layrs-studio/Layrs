//! Authenticated assertions through the real transaction service and PostgreSQL.
use super::*;
use layrs_store_server::v3::{
    PublishedObservationFilter, PublishedObservationProvenance, StepHistoryRequest,
};
use serde_json::json;

fn seal(mut request: SyncTransactionRequest, key: &str) -> SyncTransactionRequest {
    request.idempotency_key = IdempotencyKey::parse(format!("assertion-{key}-00000000")).unwrap();
    request.request_digest = request.computed_request_digest().unwrap();
    request
}
fn query(anchor: &StepId, cursor: Option<String>, limit: u16) -> StepHistoryRequest {
    StepHistoryRequest {
        anchor: anchor.clone(),
        cursor,
        limit,
        path: None,
        filter: Default::default(),
    }
}
pub async fn verify(
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    space: &SpaceId,
    anchor: &Step,
    maintenance: &sqlx::PgPool,
) {
    let actor: String = sqlx::query_scalar("SELECT actor_id FROM principals WHERE principal_id=$1")
        .bind(principal.principal_id.as_str())
        .fetch_one(maintenance)
        .await
        .unwrap();
    let actor = ActorId::parse(actor).unwrap();
    let before = facade
        .step_history(principal, space, query(anchor.id(), None, 1))
        .await
        .unwrap();
    let old_cursor = before.next.clone().unwrap();
    let bootstrap = facade.bootstrap(principal, space).await.unwrap();
    let mut template = request(&bootstrap, anchor, "assertions", LayerMutationMode::Advance);
    template.steps.clear();
    template.view_mutation = None;
    let observation = client_observation(
        space,
        &principal.principal_id,
        &actor,
        anchor.id(),
        &Timestamp::parse("2001-02-03T04:05:06.123456789Z").unwrap(),
        CaptureReason::ManualSnapshot,
    )
    .unwrap();
    template.observations = vec![observation.clone()];
    let first = seal(template.clone(), "first");
    let accepted = facade.transact(principal, first.clone()).await.unwrap();
    assert_eq!(
        accepted.receipt().status,
        ReceiptStatus::Applied,
        "{:?}",
        accepted.receipt().rejections
    );
    assert_eq!(
        accepted.receipt().accepted_observation_ids,
        [observation.id().clone()]
    );
    assert!(accepted.receipt().accepted_step_ids.is_empty());
    let replay = facade.transact(principal, first).await.unwrap();
    assert_eq!(replay.into_canonical_jcs(), accepted.into_canonical_jcs());
    let event = facade
        .events(
            principal,
            SpaceEventPageRequest {
                protocol: SyncV3Protocol::V3,
                message_type: EventPageRequestType::Request,
                space_id: space.clone(),
                client_id: ClientId::parse("client:assertions").unwrap(),
                cursor: bootstrap.watermark_cursor.clone(),
                max_events: 100,
            },
        )
        .await
        .unwrap();
    assert!(event.events.iter().any(|e|matches!(e,SpaceEvent::StepsAccepted{payload,..} if payload.step_ids.is_empty() && payload.observation_ids==[observation.id().clone()])));
    let fresh = facade
        .step_history(principal, space, query(anchor.id(), None, 100))
        .await
        .unwrap();
    assert_eq!(
        fresh.observation_snapshot.count,
        before.observation_snapshot.count + 1
    );
    assert_eq!(fresh.observation_snapshot.version, 2);
    let asserted = fresh.observations[anchor.id()]
        .iter()
        .find(|o| o.observation.id() == observation.id())
        .unwrap();
    assert_eq!(
        asserted.provenance,
        PublishedObservationProvenance::ClientAssertion
    );
    assert_eq!(asserted.observation, observation);
    assert_ne!(asserted.received_at, observation.observed_at().clone());
    let mut dated = query(anchor.id(), None, 100);
    dated.filter = PublishedObservationFilter {
        actor_id: Some(actor.clone()),
        since: Some(Timestamp::parse("2001-01-01T00:00:00Z").unwrap()),
        until: Some(Timestamp::parse("2002-01-01T00:00:00Z").unwrap()),
    };
    assert!(
        facade
            .step_history(principal, space, dated)
            .await
            .unwrap()
            .steps
            .is_empty(),
        "claimed date is never server receipt filter"
    );
    let continued = facade
        .step_history(principal, space, query(anchor.id(), Some(old_cursor), 1))
        .await
        .unwrap();
    assert_eq!(continued.observation_snapshot, before.observation_snapshot);
    let once = serde_json::to_vec(&fresh.observation_snapshot).unwrap();
    let (one, two) = tokio::join!(
        facade.transact(principal, seal(template.clone(), "repeat-one")),
        facade.transact(principal, seal(template.clone(), "repeat-two"))
    );
    for result in [one, two] {
        let receipt = result.unwrap();
        assert_eq!(receipt.receipt().status, ReceiptStatus::Applied);
        assert!(receipt.receipt().event_ids.is_empty());
        assert_eq!(
            receipt.receipt().accepted_observation_ids,
            [observation.id().clone()]
        );
    }
    assert_eq!(
        once,
        serde_json::to_vec(
            &facade
                .step_history(principal, space, query(anchor.id(), None, 100))
                .await
                .unwrap()
                .observation_snapshot
        )
        .unwrap()
    );

    for (key, value) in [
        ("actorId", json!("actor:forged")),
        ("spaceId", json!("space:other")),
        ("operationId", json!("operation:forged")),
        ("worktreeId", json!("worktree:forged")),
        ("deviceId", json!("device:forged")),
        (
            "metadata",
            json!([{"key":"provenance","value":"server_admission"}]),
        ),
        ("observationId", json!("observation:wrong")),
        ("observedAt", json!("2022-01-01T00:00:00Z")),
    ] {
        let mut value_json = serde_json::to_value(&observation).unwrap();
        value_json[key] = value;
        let mut forged = template.clone();
        forged.observations = vec![serde_json::from_value(value_json).unwrap()];
        let receipt = facade.transact(principal, seal(forged, key)).await.unwrap();
        assert_eq!(receipt.receipt().status, ReceiptStatus::Rejected, "{key}");
        assert!(receipt.receipt().accepted_observation_ids.is_empty());
    }
    // Invalid assertion rejects a valid new Step in the same transaction.
    let candidate = Step::new(
        vec![anchor.id().clone(), anchor.parent_step_ids()[0].clone()],
        anchor.root_tree_id().clone(),
    )
    .unwrap();
    let absent = StepId::parse(format!("blake3:{}", "9".repeat(64))).unwrap();
    let mut invalid = template.clone();
    invalid.steps = vec![candidate.clone()];
    invalid.observations = vec![
        client_observation(
            space,
            &principal.principal_id,
            &actor,
            &absent,
            observation.observed_at(),
            CaptureReason::Received,
        )
        .unwrap(),
    ];
    let receipt = facade
        .transact(principal, seal(invalid, "missing-step"))
        .await
        .unwrap();
    assert_eq!(receipt.receipt().status, ReceiptStatus::Rejected);
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM space_steps WHERE space_id=$1 AND step_id=$2)",
    )
    .bind(space.as_str())
    .bind(candidate.id().as_str())
    .fetch_one(maintenance)
    .await
    .unwrap();
    assert!(
        !exists,
        "invalid observation cannot partially admit its valid Step"
    );
    let distinct = candidate;
    let mut batch = template.clone();
    batch.steps = vec![distinct.clone()];
    let distinct_observation = client_observation(
        space,
        &principal.principal_id,
        &actor,
        distinct.id(),
        observation.observed_at(),
        CaptureReason::Rewrite,
    )
    .unwrap();
    batch.observations = vec![distinct_observation.clone()];
    let result = facade
        .transact(principal, seal(batch, "same-batch"))
        .await
        .unwrap();
    assert_eq!(result.receipt().status, ReceiptStatus::Applied);
    assert_eq!(
        result.receipt().accepted_observation_ids,
        [distinct_observation.id().clone()]
    );
    tamper(
        facade,
        principal,
        space,
        anchor.id(),
        &observation,
        maintenance,
    )
    .await;
    cross_space(facade, principal, space, &actor, &template).await;
    quotas(facade, principal, space, &actor, &template, maintenance).await;
    eprintln!(
        "Client observations: authenticated assertion, separate receive clock, exact replay/concurrency, no local authority, atomic Step batch, quotas and immutable snapshot checks passed"
    );
}

async fn cross_space(
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    original: &SpaceId,
    actor: &ActorId,
    template: &SyncTransactionRequest,
) {
    let other = facade
        .provision_space(
            principal,
            ProvisionSpaceRequest {
                workspace_slug: "team".into(),
                space_slug: "assertions-other".into(),
                name: "Other assertions".into(),
                initial_layer: None,
            },
        )
        .await
        .unwrap();
    assert_ne!(&other.space_id, original);
    let bootstrap = facade.bootstrap(principal, &other.space_id).await.unwrap();
    let mut request = template.clone();
    request.space_id = other.space_id.clone();
    request.known_event_cursor = Some(bootstrap.watermark_cursor);
    request.observations = vec![
        client_observation(
            &other.space_id,
            &principal.principal_id,
            actor,
            template.observations[0].step_id(),
            template.observations[0].observed_at(),
            CaptureReason::ManualSnapshot,
        )
        .unwrap(),
    ];
    let receipt = facade
        .transact(principal, seal(request, "cross-space"))
        .await
        .unwrap();
    assert_eq!(receipt.receipt().status, ReceiptStatus::Rejected);
    assert_eq!(
        receipt.receipt().rejections[0].code,
        RejectionCode::MissingObject
    );
}

/// Run last: maintenance deliberately changes epoch under the real row lock
/// to inject a policy race; no later successful business action uses this Space.
pub async fn epoch_race(
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    space: &SpaceId,
    anchor: &Step,
    pool: &sqlx::PgPool,
) {
    let bootstrap = facade.bootstrap(principal, space).await.unwrap();
    let actor: String = sqlx::query_scalar("SELECT actor_id FROM principals WHERE principal_id=$1")
        .bind(principal.principal_id.as_str())
        .fetch_one(pool)
        .await
        .unwrap();
    let mut request = super::request(&bootstrap, anchor, "epoch-race", LayerMutationMode::Advance);
    request.steps.clear();
    request.view_mutation = None;
    request.observations = vec![
        client_observation(
            space,
            &principal.principal_id,
            &ActorId::parse(actor).unwrap(),
            anchor.id(),
            &Timestamp::parse("2001-02-03T04:05:06.123456789Z").unwrap(),
            CaptureReason::ManualSnapshot,
        )
        .unwrap(),
    ];
    request.known_event_cursor = Some(bootstrap.watermark_cursor);
    request.request_digest = request.computed_request_digest().unwrap();
    let mut blocked = pool.begin().await.unwrap();
    sqlx::query("SELECT space_id FROM spaces WHERE space_id=$1 FOR UPDATE")
        .bind(space.as_str())
        .fetch_one(&mut *blocked)
        .await
        .unwrap();
    let epoch = async {
        let mut waiting = false;
        for _ in 0..100 {
            waiting=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT current_repository_view_id,current_repository_view_generation,%FOR UPDATE')").fetch_one(pool).await.unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            waiting,
            "transaction must reach final Space lock before epoch mutation"
        );
        sqlx::query(
            "UPDATE spaces SET authorization_epoch=authorization_epoch+1 WHERE space_id=$1",
        )
        .bind(space.as_str())
        .execute(&mut *blocked)
        .await
        .unwrap();
        blocked.commit().await.unwrap();
    };
    let (result, ()) = tokio::join!(facade.transact(principal, request), epoch);
    let result = result.unwrap();
    assert_eq!(result.receipt().status, ReceiptStatus::Rejected);
    assert_eq!(
        result.receipt().rejections[0].code,
        RejectionCode::AuthorizationEpochChanged
    );
    assert!(result.receipt().accepted_observation_ids.is_empty());
}

async fn tamper(
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    space: &SpaceId,
    anchor: &StepId,
    observation: &StepObservation,
    pool: &sqlx::PgPool,
) {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let before = facade
        .step_history(principal, space, query(anchor, None, 1))
        .await
        .unwrap();
    let cursor = before.next.clone().unwrap();
    let mut old: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&cursor).unwrap()).unwrap();
    old["observationSnapshot"]
        .as_object_mut()
        .unwrap()
        .remove("version");
    let old = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&old).unwrap());
    assert!(
        facade
            .step_history(principal, space, query(anchor, Some(old), 1))
            .await
            .is_err()
    );
    sqlx::query("UPDATE layrs_collaboration.published_step_observations SET received_at=received_at+interval '1 second' WHERE observation_id=$1").bind(observation.id().as_str()).execute(pool).await.unwrap();
    assert!(
        facade
            .step_history(principal, space, query(anchor, Some(cursor), 1))
            .await
            .is_err(),
        "snapshot binds receipt clock"
    );
    sqlx::query("UPDATE layrs_collaboration.published_step_observations SET received_at=received_at-interval '1 second' WHERE observation_id=$1").bind(observation.id().as_str()).execute(pool).await.unwrap();
    sqlx::query("UPDATE layrs_collaboration.published_step_observations SET record_digest=$2 WHERE observation_id=$1").bind(observation.id().as_str()).bind(format!("blake3:{}","0".repeat(64))).execute(pool).await.unwrap();
    assert!(
        facade
            .step_history(principal, space, query(anchor, None, 100))
            .await
            .is_err()
    );
    let bootstrap = facade.bootstrap(principal, space).await.unwrap();
    let mut duplicate = SyncTransactionRequest {
        protocol: SyncV3Protocol::V3,
        message_type: TransactionRequestType::Request,
        space_id: space.clone(),
        client_id: ClientId::parse("client:assertion-tamper").unwrap(),
        idempotency_key: IdempotencyKey::parse("assertion-tamper-00000000").unwrap(),
        request_digest: object_id(&format!("blake3:{}", "0".repeat(64))),
        known_event_cursor: Some(bootstrap.watermark_cursor),
        steps: vec![],
        observations: vec![observation.clone()],
        products: vec![],
        view_mutation: None,
        policy_root_mutation: None,
    };
    duplicate.request_digest = duplicate.computed_request_digest().unwrap();
    let refused = facade.transact(principal, duplicate).await.unwrap();
    assert_eq!(
        refused.receipt().status,
        ReceiptStatus::Rejected,
        "corrupt existing provenance must never be acknowledged"
    );
    let digest = format!(
        "blake3:{}",
        blake3::hash(&serde_jcs::to_vec(observation).unwrap())
    );
    sqlx::query("UPDATE layrs_collaboration.published_step_observations SET record_digest=$2 WHERE observation_id=$1").bind(observation.id().as_str()).bind(digest).execute(pool).await.unwrap();
}

async fn quotas(
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    space: &SpaceId,
    actor: &ActorId,
    template: &SyncTransactionRequest,
    pool: &sqlx::PgPool,
) {
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM layrs_collaboration.published_step_observations WHERE space_id=$1 AND principal_id=$2 AND provenance='client_assertion'").bind(space.as_str()).bind(principal.principal_id.as_str()).fetch_one(pool).await.unwrap();
    let mut full = template.clone();
    let existing = facade
        .step_history(
            principal,
            space,
            query(template.observations[0].step_id(), None, 1),
        )
        .await
        .unwrap();
    let mut parent = template.observations[0].step_id().clone();
    for _ in 0..40 {
        let step = Step::new(vec![parent], existing.steps[0].root_tree_id().clone()).unwrap();
        parent = step.id().clone();
        full.steps.push(step);
    }
    let mut crowded = full.clone();
    crowded.steps.truncate(1);
    crowded.observations = (0..257)
        .map(|i| {
            client_observation(
                space,
                &principal.principal_id,
                actor,
                crowded.steps[0].id(),
                &Timestamp::parse(format!("1998-01-01T00:00:00.{i:06}Z")).unwrap(),
                CaptureReason::ManualSnapshot,
            )
            .unwrap()
        })
        .collect();
    let denied = facade
        .transact(principal, seal(crowded, "crowded-step"))
        .await
        .unwrap();
    assert_eq!(denied.receipt().status, ReceiptStatus::Rejected);
    assert!(denied.receipt().rejections[0].message.contains("per-Step"));
    full.observations = (0..10_000 - count)
        .map(|i| {
            client_observation(
                space,
                &principal.principal_id,
                actor,
                full.steps[i as usize / 250].id(),
                &Timestamp::parse(format!("2000-01-01T00:00:00.{i:06}Z")).unwrap(),
                CaptureReason::ManualSnapshot,
            )
            .unwrap()
        })
        .collect();
    let result = facade
        .transact(principal, seal(full, "fill-quota"))
        .await
        .unwrap();
    assert_eq!(
        result.receipt().status,
        ReceiptStatus::Applied,
        "{:?}",
        result.receipt().rejections
    );
    let mut extra = template.clone();
    extra.observations = vec![
        client_observation(
            space,
            &principal.principal_id,
            actor,
            template.observations[0].step_id(),
            &Timestamp::parse("1999-01-01T00:00:00Z").unwrap(),
            CaptureReason::ManualSnapshot,
        )
        .unwrap(),
    ];
    let (one, two) = tokio::join!(
        facade.transact(principal, seal(extra.clone(), "quota-one")),
        facade.transact(principal, seal(extra, "quota-two"))
    );
    for result in [one, two] {
        let result = result.unwrap();
        assert_eq!(result.receipt().status, ReceiptStatus::Rejected);
        assert!(result.receipt().accepted_observation_ids.is_empty());
    }
    let repeat = facade
        .transact(principal, seal(template.clone(), "retry-at-quota"))
        .await
        .unwrap();
    assert_eq!(repeat.receipt().status, ReceiptStatus::Applied);
    let mut cursor = None;
    let mut seen = std::collections::BTreeSet::new();
    let mut seen_observations = std::collections::BTreeSet::new();
    let mut pages = 0;
    loop {
        let page = facade
            .step_history(principal, space, query(&parent, cursor, 30))
            .await
            .unwrap();
        assert!(!page.steps.is_empty());
        assert!(serde_json::to_vec(&page).unwrap().len() <= 2 * 1024 * 1024);
        for step in &page.steps {
            assert!(
                seen.insert(step.id().clone()),
                "byte pagination cannot repeat Steps"
            );
        }
        for values in page.observations.values() {
            for item in values {
                if item.provenance == PublishedObservationProvenance::ClientAssertion {
                    assert!(seen_observations.insert(item.observation.id().clone()));
                }
            }
        }
        cursor = page.next;
        pages += 1;
        if cursor.is_none() {
            break;
        }
        assert!(pages < 100);
    }
    assert!(
        pages > 2,
        "dense observation pages must split on bytes, not only Step count"
    );
    assert_eq!(
        seen_observations.len(),
        9999,
        "chain plus original assertion; other assertion belongs to detached merge Step"
    );
}
