//! Real PostgreSQL provenance, pinned pagination and tamper detection.
use super::*;
use layrs_store_server::v3::{
    PublishedObservationFilter, PublishedObservationProvenance, StepHistoryRequest,
};

pub async fn verify(
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    space: &SpaceId,
    anchor: &StepId,
    first: &StepId,
    maintenance: &sqlx::PgPool,
) {
    let query =
        |filter: PublishedObservationFilter, cursor: Option<String>, limit| StepHistoryRequest {
            anchor: anchor.clone(),
            cursor,
            limit,
            path: None,
            filter,
        };
    let page = facade
        .step_history(principal, space, query(Default::default(), None, 1))
        .await
        .unwrap();
    let cursor = page.next.clone().unwrap();
    assert_eq!(page.observations.len(), 1);
    let observed = &page.observations[anchor][0];
    assert_eq!(
        observed.provenance,
        PublishedObservationProvenance::ServerAdmission
    );
    assert_eq!(observed.principal_id, principal.principal_id);
    assert_eq!(
        observed.observation.capture_reason(),
        CaptureReason::Received
    );
    assert!(observed.observation.operation_id().is_none());
    assert!(observed.observation.device_id().is_none());
    assert!(observed.observation.worktree_id().is_none());
    assert_eq!(observed.observation.observed_at(), &observed.received_at);
    let actor = observed.observation.actor_id().unwrap().clone();
    let actual_actor: String =
        sqlx::query_scalar("SELECT actor_id FROM principals WHERE principal_id=$1")
            .bind(principal.principal_id.as_str())
            .fetch_one(maintenance)
            .await
            .unwrap();
    assert_eq!(actor.as_str(), actual_actor);
    let exact = PublishedObservationFilter {
        actor_id: Some(actor.clone()),
        since: Some(observed.received_at.clone()),
        until: Some(observed.received_at.clone()),
    };
    let matching = facade
        .step_history(principal, space, query(exact.clone(), None, 100))
        .await
        .unwrap();
    assert!(matching.steps.iter().any(|step| step.id() == anchor));
    let mut wrong = exact.clone();
    wrong.actor_id = Some(ActorId::parse("actor:not-the-publisher").unwrap());
    assert!(
        facade
            .step_history(principal, space, query(wrong, None, 100))
            .await
            .unwrap()
            .steps
            .is_empty()
    );
    let distant = PublishedObservationFilter {
        actor_id: Some(actor),
        since: Some(Timestamp::parse("1900-01-01T00:00:00Z").unwrap()),
        until: Some(Timestamp::parse("1901-01-01T00:00:00Z").unwrap()),
    };
    assert!(
        facade
            .step_history(principal, space, query(distant, None, 100))
            .await
            .unwrap()
            .steps
            .is_empty()
    );
    assert!(
        facade
            .step_history(principal, space, query(exact, Some(cursor.clone()), 1))
            .await
            .is_err(),
        "cursor binds exact filter"
    );

    // A newly admitted child appends one observation, without changing the old
    // anchor or the observation snapshot embedded in its continuation cursor.
    let child = Step::new(vec![anchor.clone()], page.steps[0].root_tree_id().clone()).unwrap();
    let bootstrap = facade.bootstrap(principal, space).await.unwrap();
    let request = request(
        &bootstrap,
        &child,
        "observation-append",
        LayerMutationMode::Integrate,
    );
    let appended = facade.transact(principal, request.clone()).await.unwrap();
    assert_eq!(appended.receipt().status, ReceiptStatus::Applied);
    assert!(
        appended.receipt().accepted_observation_ids.is_empty(),
        "server side effects do not acknowledge client-supplied observations"
    );
    assert_eq!(
        facade
            .transact(principal, request.clone())
            .await
            .unwrap()
            .into_canonical_jcs(),
        appended.into_canonical_jcs()
    );
    let next = facade
        .step_history(
            principal,
            space,
            query(Default::default(), Some(cursor.clone()), 1),
        )
        .await
        .unwrap();
    assert_eq!(next.steps[0].id(), first);
    assert_eq!(next.observation_snapshot, page.observation_snapshot);
    let fresh = facade
        .step_history(principal, space, query(Default::default(), None, 100))
        .await
        .unwrap();
    assert_eq!(
        fresh.observation_snapshot.count,
        page.observation_snapshot.count + 1
    );
    assert_eq!(fresh.observations.len(), fresh.steps.len());
    let mut repeat = request.clone();
    repeat.view_mutation = None;
    repeat.idempotency_key = IdempotencyKey::parse("observation-republish-old-step").unwrap();
    repeat.request_digest = repeat.computed_request_digest().unwrap();
    let no_new = facade.transact(principal, repeat).await.unwrap();
    assert_eq!(no_new.receipt().status, ReceiptStatus::Applied);
    assert!(
        no_new.receipt().accepted_observation_ids.is_empty(),
        "republishing an old Step is not a fresh authorship claim"
    );

    // Declarative local capture identity/date cannot impersonate authenticated
    // publication provenance, including metadata claiming server admission.
    let mut forged = request;
    forged.steps.clear();
    forged.view_mutation = None;
    forged.idempotency_key = IdempotencyKey::parse("observation-forged-local-capture").unwrap();
    forged.observations = vec![StepObservation::new(
        ObservationId::parse("observation:forged-local").unwrap(),
        first.clone(),
        Some(space.clone()),
        None,
        None,
        Some(ActorId::parse("actor:forged-publisher").unwrap()),
        None,
        Timestamp::parse("1900-01-01T00:00:00Z").unwrap(),
        CaptureReason::Received,
        Metadata::new(vec![
            MetadataEntry::new("provenance", "server_admission").unwrap(),
        ])
        .unwrap(),
    )];
    forged.request_digest = forged.computed_request_digest().unwrap();
    let denied = facade.transact(principal, forged).await.unwrap();
    assert_eq!(denied.receipt().status, ReceiptStatus::Rejected);
    assert!(denied.receipt().accepted_observation_ids.is_empty());

    // Candidate-only transactions can race without moving a View. Only the
    // transaction that first installs space_steps may record first admission.
    let candidate = Step::new(vec![child.id().clone()], child.root_tree_id().clone()).unwrap();
    let current = facade.bootstrap(principal, space).await.unwrap();
    let mut one = super::request(
        &current,
        &candidate,
        "observation-race-one",
        LayerMutationMode::Advance,
    );
    one.view_mutation = None;
    one.request_digest = one.computed_request_digest().unwrap();
    let mut two = one.clone();
    two.idempotency_key = IdempotencyKey::parse("observation-race-two").unwrap();
    two.request_digest = two.computed_request_digest().unwrap();
    let (one, two) = tokio::join!(
        facade.transact(principal, one),
        facade.transact(principal, two)
    );
    assert_eq!(one.unwrap().receipt().status, ReceiptStatus::Applied);
    assert_eq!(two.unwrap().receipt().status, ReceiptStatus::Applied);
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM layrs_collaboration.published_step_observations WHERE space_id=$1 AND step_id=$2").bind(space.as_str()).bind(candidate.id().as_str()).fetch_one(maintenance).await.unwrap();
    assert_eq!(
        count, 1,
        "first admission is recorded once under concurrent candidate-only requests"
    );

    let id = observed.observation.id().as_str();
    let saved: String = sqlx::query_scalar(
        "SELECT canonical_record_jcs FROM step_observations WHERE observation_id=$1",
    )
    .bind(id)
    .fetch_one(maintenance)
    .await
    .unwrap();
    sqlx::query("UPDATE step_observations SET canonical_record_jcs='{}' WHERE observation_id=$1")
        .bind(id)
        .execute(maintenance)
        .await
        .unwrap();
    assert!(
        facade
            .step_history(principal, space, query(Default::default(), None, 100))
            .await
            .is_err(),
        "corrupt canonical record is never displayed"
    );
    sqlx::query("UPDATE step_observations SET canonical_record_jcs=$2 WHERE observation_id=$1")
        .bind(id)
        .bind(saved)
        .execute(maintenance)
        .await
        .unwrap();
    let provenance:String=sqlx::query_scalar("SELECT row_to_json(p)::text FROM layrs_collaboration.published_step_observations p WHERE observation_id=$1").bind(id).fetch_one(maintenance).await.unwrap();
    sqlx::query(
        "DELETE FROM layrs_collaboration.published_step_observations WHERE observation_id=$1",
    )
    .bind(id)
    .execute(maintenance)
    .await
    .unwrap();
    assert!(
        facade
            .step_history(
                principal,
                space,
                query(Default::default(), Some(cursor.clone()), 1)
            )
            .await
            .is_err(),
        "disappeared observation invalidates a pinned snapshot"
    );
    sqlx::query("INSERT INTO layrs_collaboration.published_step_observations SELECT (json_populate_record(NULL::layrs_collaboration.published_step_observations,$1::json)).*").bind(provenance).execute(maintenance).await.unwrap();
    assert!(
        facade
            .step_history(principal, space, query(Default::default(), Some(cursor), 1))
            .await
            .is_ok()
    );
    eprintln!(
        "Published observations: authenticated first-admission provenance, inclusive filters, append-pinned cursor, replay, local forgery refusal and corrupt/missing-row rejection passed"
    );
}
