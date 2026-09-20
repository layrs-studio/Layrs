//! Authorization evidence shared by ordinary and administrative transactions.
use super::*;
use bytes::Bytes;
use layrs_core::scm::Timestamp;
use layrs_sync::v3::*;

pub(crate) async fn snapshot(
    repository: &PgV3Repository,
    principal: &PrincipalContext,
    request: &SyncTransactionRequest,
    authority: &SpaceAuthority,
    policy: &PolicySet,
    timestamp: &Timestamp,
) -> V3StoreResult<(AuthorizationSnapshot, PreparedCasObject, bool)> {
    let decision = repository
        .authorizer()
        .authorize(&AuthorizationCheck {
            principal: principal.clone(),
            action: V3Action::ApplyTransaction,
            target: AuthorizationTarget::Space(request.space_id.clone()),
            policy_set_id: authority.policy_set_id.clone(),
            authorization_epoch: authority.authorization_epoch,
        })
        .await?;
    let mut versions: Vec<_> = policy
        .policy_versions
        .iter()
        .map(|v| v.policy_version_id.clone())
        .collect();
    versions.sort();
    let choose_authorization = if super::transaction_choose::required(request)
        || super::transaction_policy::required(request)
    {
        Some(super::transaction_choose::authorize(repository, principal, authority).await?)
    } else {
        None
    };
    let choose_allowed = choose_authorization.as_ref().is_none_or(|d| d.allowed);
    let mut snapshot = AuthorizationSnapshot {
        schema_version: AuthorizationSnapshotSchemaVersion::V1,
        authorization_snapshot_id: request.request_digest.clone(),
        space_id: request.space_id.clone(),
        subject: AuthorizationSubject::Principal {
            principal_id: principal.principal_id.clone(),
        },
        policy_set_id: authority.policy_set_id.clone(),
        authorization_epoch: authority.authorization_epoch,
        request_binding: AuthorizationRequestBinding {
            client_id: request.client_id.clone(),
            idempotency_key: request.idempotency_key.clone(),
            request_digest: request.request_digest.clone(),
        },
        decisions: vec![layrs_sync::v3::AuthorizationDecision {
            action: V3Action::ApplyTransaction.as_policy_action().into(),
            scope: AuthorizationScope::Space {
                space_id: request.space_id.clone(),
            },
            allowed: decision.allowed,
            policy_version_ids: versions.clone(),
            reason_code: decision.reason_code,
        }],
        evaluated_at: timestamp.clone(),
    };
    if let Some(decision) = choose_authorization {
        snapshot
            .decisions
            .push(layrs_sync::v3::AuthorizationDecision {
                action: V3Action::AdministerPolicy.as_policy_action().into(),
                scope: AuthorizationScope::Space {
                    space_id: request.space_id.clone(),
                },
                allowed: decision.allowed,
                policy_version_ids: versions,
                reason_code: decision.reason_code,
            });
        snapshot.decisions.sort_by(|a, b| a.action.cmp(&b.action));
    }
    snapshot.authorization_snapshot_id = snapshot.computed_id().map_err(json_error)?;
    let snapshot_object = repository
        .cas()
        .prepare_generated(
            ObjectKind::AuthorizationSnapshot,
            Bytes::from(serde_jcs::to_vec(&snapshot).map_err(json_error)?),
        )
        .await?;
    Ok((
        snapshot,
        snapshot_object,
        decision.allowed && choose_allowed,
    ))
}
fn json_error(e: impl std::fmt::Display) -> V3StoreError {
    V3StoreError::Backend(e.to_string())
}
