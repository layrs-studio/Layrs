use super::*;
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    app: &Router,
    base: &str,
    admin: &str,
    runner: &str,
    repository: &PgV3Repository,
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    bootstrap: &SpaceBootstrap,
    candidate: &Step,
    other_step: &Step,
    evaluation_path: &str,
    run_detail: &Value,
    cas_root: &Path,
) {
    let product: ProductEnvelope = serde_json::from_value(run_detail["product"].clone()).unwrap();
    let ProductEnvelope::Proof { record, .. } = &product else {
        panic!("real proof required")
    };
    let roots = [
        ("product", product.object_id().as_str()),
        ("evidence", record.evidence_object_ids[0].as_str()),
        ("evidence", record.recipe_object_id.as_str()),
    ];
    for (index, (kind, id)) in roots.into_iter().enumerate() {
        let hex = &id[7..];
        let path = cas_root
            .join("objects")
            .join(kind)
            .join(&hex[..2])
            .join(hex);
        let original = std::fs::read(&path).unwrap();
        for missing in [false, true] {
            let saved = path.with_extension("integrity-fixture-original");
            if missing {
                std::fs::rename(&path, &saved).unwrap();
            } else {
                let mut damaged = original.clone();
                damaged[0] ^= 1;
                std::fs::write(&path, damaged).unwrap();
            }
            let verdict = ok(
                app,
                "POST",
                evaluation_path,
                admin,
                json!({"stepId":candidate.id()}),
            )
            .await;
            assert_eq!(
                verdict["outcome"], "failed",
                "missing/corrupt {kind} must block Gate"
            );
            let response = facade
                .transact(
                    principal,
                    request(
                        bootstrap,
                        candidate,
                        &format!("proof-integrity-{index}-{missing}"),
                        LayerMutationMode::Advance,
                    ),
                )
                .await
                .unwrap();
            assert_eq!(response.receipt().status, ReceiptStatus::Rejected);
            assert_eq!(
                response.receipt().rejections[0].code,
                RejectionCode::GateRejected
            );
            let current: String = sqlx::query_scalar(
                "SELECT current_repository_view_id FROM spaces WHERE space_id=$1",
            )
            .bind(bootstrap.space_id.as_str())
            .fetch_one(repository.pool())
            .await
            .unwrap();
            assert_eq!(
                current,
                bootstrap
                    .current_repository_view
                    .repository_view_id
                    .as_str(),
                "failed Proof cannot advance heads"
            );
            if missing {
                std::fs::rename(&saved, &path).unwrap();
            } else {
                std::fs::write(&path, &original).unwrap();
            }
            assert_eq!(
                ok(
                    app,
                    "POST",
                    evaluation_path,
                    admin,
                    json!({"stepId":candidate.id()})
                )
                .await["outcome"],
                "passed"
            );
        }
    }
    // A real Proof for another immutable subject cannot replace the SQL projection.
    let other = ok(app, "POST", &format!("{base}/runs"), runner, json!({"idempotencyKey":"proof-integrity-other-subject", "stepId":other_step.id(),"recipeObjectId":record.recipe_object_id})).await;
    let other = ok(
        app,
        "POST",
        &format!("{base}/runs/{}/complete", other["runId"].as_str().unwrap()),
        runner,
        json!({"outcome":"passed","log":"different exact subject"}),
    )
    .await;
    sqlx::query("UPDATE layrs_collaboration.runs SET proof_object_id=$2 WHERE run_id=$1")
        .bind(run_detail["runId"].as_str().unwrap())
        .bind(other["proofObjectId"].as_str().unwrap())
        .execute(repository.pool())
        .await
        .unwrap();
    assert_eq!(
        ok(
            app,
            "POST",
            evaluation_path,
            admin,
            json!({"stepId":candidate.id()})
        )
        .await["outcome"],
        "failed"
    );
    sqlx::query("UPDATE layrs_collaboration.runs SET proof_object_id=$2 WHERE run_id=$1")
        .bind(run_detail["runId"].as_str().unwrap())
        .bind(product.object_id().as_str())
        .execute(repository.pool())
        .await
        .unwrap();
    // Even an identity-valid Proof envelope is forbidden on the general Sync endpoint.
    let mut forged = product;
    if let ProductEnvelope::Proof {
        record,
        depends_on_step_ids,
        ..
    } = &mut forged
    {
        record.subject = ImmutableSubject::Step {
            object_id: object_id(other_step.id().as_str()),
        };
        *depends_on_step_ids = vec![other_step.id().clone()];
    }
    let forged_id = forged.computed_object_id().unwrap();
    if let ProductEnvelope::Proof {
        product_object_id, ..
    } = &mut forged
    {
        *product_object_id = forged_id;
    }
    forged.verify_identity().unwrap();
    let mut attempt = request(
        bootstrap,
        candidate,
        "proof-forged-via-sync",
        LayerMutationMode::Advance,
    );
    attempt.products = vec![forged];
    attempt.request_digest = attempt.computed_request_digest().unwrap();
    let response = facade.transact(principal, attempt).await.unwrap();
    assert_eq!(response.receipt().status, ReceiptStatus::Rejected);
    assert!(matches!(
        response.receipt().rejections[0].code,
        RejectionCode::InvalidRequest | RejectionCode::IncompatibleVersion
    ));
}
