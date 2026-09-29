use super::*;

pub(super) async fn list_accounts(
    State(identity): State<PgIdentity>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>> {
    let mut tx = identity.pool.begin().await.map_err(db)?;
    authenticated_in_tx(&identity, &headers, &mut tx, true).await?;
    let rows=sqlx::query("SELECT principal_id,display_name,principal_kind,status FROM principals WHERE deployment_id=$1 ORDER BY display_name,principal_id LIMIT 257").bind(&identity.deployment_id).fetch_all(&mut *tx).await.map_err(db)?;
    let has_more = rows.len() > 256;
    Ok(Json(
        serde_json::json!({"accounts":rows.iter().take(256).map(|r|serde_json::json!({"principalId":r.get::<String,_>("principal_id"),"displayName":r.get::<String,_>("display_name"),"kind":r.get::<String,_>("principal_kind"),"status":r.get::<String,_>("status")})).collect::<Vec<_>>(),"hasMore":has_more}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CreateAccount {
    login: String,
    password: String,
    display_name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PrincipalRequest {
    principal_id: PrincipalId,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ServiceRequest {
    display_name: String,
    label: String,
    expires_in: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RotateService {
    principal_id: PrincipalId,
    label: String,
    expires_in: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RevokeToken {
    token_id: String,
}

/// Locks the current authenticated Principal and rechecks the token inside the transaction.
pub(super) async fn authenticated_in_tx(
    identity: &PgIdentity,
    headers: &HeaderMap,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    administrator: bool,
) -> Result<(String, i64)> {
    identity.check_origin(headers)?;
    let row=sqlx::query("SELECT p.principal_id,p.authorization_generation,p.principal_kind FROM principals p JOIN layrs_identity.tokens t USING(principal_id) JOIN deployments d USING(deployment_id) WHERE t.token_digest=$1 AND p.deployment_id=$2 AND t.revoked_at IS NULL AND t.expires_at>clock_timestamp() AND p.status='active' AND d.status='ready' AND t.authorization_generation=p.authorization_generation FOR SHARE OF p")
        .bind(token_digest(bearer(headers)?)).bind(&identity.deployment_id).fetch_optional(&mut **tx).await.map_err(db)?.ok_or(IdentityError::Unauthorized)?;
    if administrator && !is_instance_administrator(&row.get::<String, _>("principal_kind")) {
        return Err(IdentityError::Unauthorized);
    }
    Ok((row.get("principal_id"), row.get("authorization_generation")))
}

pub(super) async fn create_account(
    State(identity): State<PgIdentity>,
    headers: HeaderMap,
    Json(request): Json<CreateAccount>,
) -> Result<impl IntoResponse> {
    identity.check_origin(&headers)?;
    // Authenticate before expensive password work; revalidate under lock afterward.
    identity
        .verify_bearer(bearer(&headers)?)
        .await
        .map_err(|_| IdentityError::Unauthorized)?;
    let login = normalize_login(&request.login)?;
    validate_label(&request.display_name)?;
    let hash = identity.password_hash(request.password).await?;
    let mut tx = identity.pool.begin().await.map_err(db)?;
    authenticated_in_tx(&identity, &headers, &mut tx, true).await?;
    let principal = format!("principal:{}", uuid::Uuid::new_v4());
    sqlx::query("INSERT INTO principals(principal_id,actor_id,deployment_id,principal_kind,subject_digest,display_name) VALUES($1,$2,$3,'account',$4,$5)")
        .bind(&principal).bind(format!("actor:{}",uuid::Uuid::new_v4())).bind(&identity.deployment_id).bind(token_digest(&format!("account:{login}"))).bind(request.display_name).execute(&mut *tx).await.map_err(db)?;
    sqlx::query("INSERT INTO layrs_identity.accounts(principal_id,deployment_id,login,password_hash) VALUES($1,$2,$3,$4)")
        .bind(&principal).bind(&identity.deployment_id).bind(login).bind(hash).execute(&mut *tx).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"principalId":principal})),
    ))
}

pub(super) async fn revoke_account(
    State(identity): State<PgIdentity>,
    headers: HeaderMap,
    Json(request): Json<PrincipalRequest>,
) -> Result<StatusCode> {
    let mut tx = identity.pool.begin().await.map_err(db)?;
    let (administrator, _) = authenticated_in_tx(&identity, &headers, &mut tx, true).await?;
    // The bootstrap administrator is the offline recovery authority; do not orphan an installation.
    if administrator == request.principal_id.as_str() {
        return Err(IdentityError::Invalid);
    }
    let changed=sqlx::query("UPDATE principals SET status='revoked',revoked_at=clock_timestamp(),authorization_generation=authorization_generation+1 WHERE principal_id=$1 AND deployment_id=$2 AND principal_kind<>'bootstrap_admin' AND status<>'revoked'")
        .bind(request.principal_id.as_str()).bind(&identity.deployment_id).execute(&mut *tx).await.map_err(db)?;
    if changed.rows_affected() != 1 {
        return Err(IdentityError::Invalid);
    }
    tx.commit().await.map_err(db)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn create_service(
    State(identity): State<PgIdentity>,
    headers: HeaderMap,
    Json(request): Json<ServiceRequest>,
) -> Result<impl IntoResponse> {
    validate_label(&request.display_name)?;
    validate_token_request(&request.label, request.expires_in)?;
    let mut tx = identity.pool.begin().await.map_err(db)?;
    authenticated_in_tx(&identity, &headers, &mut tx, true).await?;
    let principal = format!("principal:{}", uuid::Uuid::new_v4());
    sqlx::query("INSERT INTO principals(principal_id,actor_id,deployment_id,principal_kind,subject_digest,display_name) VALUES($1,$2,$3,'service',$4,$5)")
        .bind(&principal).bind(format!("actor:{}",uuid::Uuid::new_v4())).bind(&identity.deployment_id).bind(token_digest(&format!("service:{principal}"))).bind(request.display_name).execute(&mut *tx).await.map_err(db)?;
    let session = issue_token(
        &mut tx,
        &principal,
        1,
        "service",
        &request.label,
        request.expires_in,
    )
    .await?;
    tx.commit().await.map_err(db)?;
    // No workspace membership or Policy grant is inherited from the administrator.
    Ok((StatusCode::CREATED, Json(session)))
}

pub(super) async fn rotate_service(
    State(identity): State<PgIdentity>,
    headers: HeaderMap,
    Json(request): Json<RotateService>,
) -> Result<Json<Session>> {
    validate_token_request(&request.label, request.expires_in)?;
    let mut tx = identity.pool.begin().await.map_err(db)?;
    authenticated_in_tx(&identity, &headers, &mut tx, true).await?;
    let generation:Option<i64>=sqlx::query_scalar("UPDATE principals SET authorization_generation=authorization_generation+1 WHERE principal_id=$1 AND deployment_id=$2 AND principal_kind='service' AND status='active' RETURNING authorization_generation")
        .bind(request.principal_id.as_str()).bind(&identity.deployment_id).fetch_optional(&mut *tx).await.map_err(db)?;
    let session = issue_token(
        &mut tx,
        request.principal_id.as_str(),
        generation.ok_or(IdentityError::Invalid)?,
        "service",
        &request.label,
        request.expires_in,
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Ok(Json(session))
}

pub(super) async fn list_tokens(
    State(identity): State<PgIdentity>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>> {
    identity.check_origin(&headers)?;
    let principal = identity
        .verify_bearer(bearer(&headers)?)
        .await
        .map_err(|_| IdentityError::Unauthorized)?;
    let rows=sqlx::query("SELECT token_digest,token_kind,label,expires_at::text AS expires_at,revoked_at IS NOT NULL AS revoked FROM layrs_identity.tokens WHERE principal_id=$1 ORDER BY created_at DESC LIMIT 100")
        .bind(principal.as_str()).fetch_all(&identity.pool).await.map_err(db)?;
    let values:Vec<_>=rows.iter().map(|row|serde_json::json!({"tokenId":row.get::<String,_>("token_digest"),"kind":row.get::<String,_>("token_kind"),"label":row.get::<String,_>("label"),"expiresAt":row.get::<String,_>("expires_at"),"revoked":row.get::<bool,_>("revoked")})).collect();
    Ok(Json(serde_json::json!({"tokens":values})))
}

pub(super) async fn revoke_token(
    State(identity): State<PgIdentity>,
    headers: HeaderMap,
    Json(request): Json<RevokeToken>,
) -> Result<StatusCode> {
    if request.token_id.len() != 64 || !request.token_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(IdentityError::Invalid);
    }
    let mut tx = identity.pool.begin().await.map_err(db)?;
    let (principal, _) = authenticated_in_tx(&identity, &headers, &mut tx, false).await?;
    sqlx::query("UPDATE layrs_identity.tokens SET revoked_at=clock_timestamp() WHERE token_digest=$1 AND principal_id=$2 AND revoked_at IS NULL")
        .bind(request.token_id).bind(principal).execute(&mut *tx).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) fn validate_label(label: &str) -> Result<()> {
    if label.trim().is_empty() || label.len() > 128 || label.chars().any(char::is_control) {
        Err(IdentityError::Invalid)
    } else {
        Ok(())
    }
}
fn validate_token_request(label: &str, expires: u64) -> Result<()> {
    validate_label(label)?;
    if !(3600..=90 * 24 * 3600).contains(&expires) {
        Err(IdentityError::Invalid)
    } else {
        Ok(())
    }
}
pub(super) async fn issue_token(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    principal: &str,
    generation: i64,
    kind: &str,
    label: &str,
    expires: u64,
) -> Result<Session> {
    let token = random_secret();
    sqlx::query("INSERT INTO layrs_identity.tokens(token_digest,principal_id,authorization_generation,token_kind,label,expires_at) VALUES($1,$2,$3,$4,$5,clock_timestamp()+($6::bigint * interval '1 second'))")
        .bind(token_digest(&token)).bind(principal).bind(generation).bind(kind).bind(label).bind(expires as i64).execute(&mut **tx).await.map_err(db)?;
    Ok(Session {
        principal_id: PrincipalId::parse(principal).map_err(|_| IdentityError::Unavailable)?,
        access_token: token,
        token_type: "Bearer",
        expires_in: expires,
    })
}
