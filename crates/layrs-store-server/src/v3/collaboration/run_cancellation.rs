use super::*;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CancelRunRequest {
    pub idempotency_key: String,
    pub reason: String,
}
pub(super) fn cancellation_from_row(
    row: &sqlx::postgres::PgRow,
) -> V3StoreResult<Option<serde_json::Value>> {
    match (
        row.get::<Option<String>, _>("cancellation_object_id"),
        row.get::<Option<String>, _>("cancellation_document"),
    ) {
        (None, None) => Ok(None),
        (Some(object), Some(document)) => {
            let record: serde_json::Value = serde_json::from_str(&document).map_err(bad)?;
            Ok(Some(
                serde_json::json!({"evidenceObjectId":object,"record":record}),
            ))
        }
        _ => Err(types::invalid("invalid_cancellation_projection")),
    }
}

impl PgCollaborationStore {
    /// Stops admission of a future verdict. It cannot terminate an external
    /// worker process; that worker remains subject to its own execution bounds.
    pub async fn cancel_run(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        run: &str,
        request: CancelRunRequest,
    ) -> V3StoreResult<serde_json::Value> {
        types::validate_key(&request.idempotency_key)?;
        if request.reason.trim().is_empty()
            || request.reason.len() > 4096
            || request.reason.contains('\0')
        {
            return Err(types::invalid("invalid_cancellation_reason"));
        }
        // Existing capabilities only: policy administrators may stop any run,
        // while proof producers may stop only the run they actually created.
        let (authority, administrator) = match self
            .authority(principal, space, V3Action::AdministerPolicy)
            .await
        {
            Ok(authority) => (authority, true),
            Err(V3StoreError::InvalidCapability) => (
                self.authority(principal, space, V3Action::RunProof).await?,
                false,
            ),
            Err(error) => return Err(error),
        };
        let request_digest = digest(&(&principal.principal_id, &request))?;
        let mut tx = self.locked(principal, &authority).await?;
        let row=sqlx::query("SELECT step_id,recipe_object_id,runner_principal_id,policy_set_id,authorization_epoch,proof_object_id FROM layrs_collaboration.runs WHERE space_id=$1 AND run_id=$2 FOR UPDATE")
            .bind(space.as_str()).bind(run).fetch_optional(&mut *tx).await.map_err(pg_error)?.ok_or(V3StoreError::NotFound)?;
        let producer: String = row.get("runner_principal_id");
        if !administrator && producer != principal.principal_id.as_str() {
            return Err(V3StoreError::InvalidCapability);
        }
        if let Some(previous)=sqlx::query("SELECT request_digest,evidence_object_id,canonical_document_jcs FROM layrs_collaboration.run_cancellations WHERE space_id=$1 AND run_id=$2")
            .bind(space.as_str()).bind(run).fetch_optional(&mut *tx).await.map_err(pg_error)? {
            if previous.get::<String,_>("request_digest") != request_digest {
                return Err(types::invalid("run_already_cancelled"));
            }
            let record: serde_json::Value=serde_json::from_str(&previous.get::<String,_>("canonical_document_jcs")).map_err(bad)?;
            return Ok(serde_json::json!({"runId":run,"status":"cancelled","cancellation":{"evidenceObjectId":previous.get::<String,_>("evidence_object_id"),"record":record}}));
        }
        if row.get::<Option<String>, _>("proof_object_id").is_some() {
            return Err(types::invalid("run_already_completed"));
        }
        let step: String = row.get("step_id");
        let recipe: String = row.get("recipe_object_id");
        let started_policy: String = row.get("policy_set_id");
        let record = serde_json::json!({
            "schemaVersion":"layrs.run-cancellation.v1","runId":run,"spaceId":space,
            "stepId":step,"recipeObjectId":recipe,"runnerPrincipalId":producer,
            "startedUnderPolicySetId":started_policy,"startedAuthorizationEpoch":row.get::<i64,_>("authorization_epoch"),
            "cancelledBy":principal.principal_id,"reason":request.reason,"cancelledAt":now()?,
            "policySetId":authority.policy_set_id,"authorizationEpoch":authority.authorization_epoch,
        });
        let object = self.evidence(&record).await?;
        self.install(&mut tx, space, &object).await?;
        let mut references = vec![
            step,
            recipe,
            started_policy,
            authority.policy_set_id.as_str().to_owned(),
        ];
        references.sort();
        references.dedup();
        for (ordinal, reference) in references.iter().enumerate() {
            sqlx::query("INSERT INTO canonical_object_edges(source_object_id,ordinal,edge_kind,target_object_id) VALUES($1,$2,'evidence',$3)")
                .bind(object.descriptor.object_id.as_str()).bind(ordinal as i32).bind(reference).execute(&mut *tx).await.map_err(pg_error)?;
        }
        sqlx::query("INSERT INTO layrs_collaboration.run_cancellations(run_id,space_id,cancelled_by,request_digest,evidence_object_id,canonical_document_jcs) VALUES($1,$2,$3,$4,$5,$6)")
            .bind(run).bind(space.as_str()).bind(principal.principal_id.as_str()).bind(request_digest).bind(object.descriptor.object_id.as_str()).bind(jcs(&record)?).execute(&mut *tx).await.map_err(pg_error)?;
        self.notify(
            &mut tx,
            principal,
            space,
            "proof.cancelled",
            run,
            &object.descriptor.object_id,
        )
        .await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(
            serde_json::json!({"runId":run,"status":"cancelled","cancellation":{"evidenceObjectId":object.descriptor.object_id,"record":record}}),
        )
    }
}
