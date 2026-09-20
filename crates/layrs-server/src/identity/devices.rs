use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StartRequest {
    label: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Approval {
    user_code: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PollRequest {
    device_code: String,
}

pub(super) async fn inspect(
    State(identity): State<PgIdentity>,
    headers: HeaderMap,
    Json(request): Json<Approval>,
) -> Result<Json<serde_json::Value>> {
    identity.check_origin(&headers)?;
    identity
        .verify_bearer(bearer(&headers)?)
        .await
        .map_err(|_| IdentityError::Unauthorized)?;
    let code = normalize_user_code(&request.user_code)?;
    let row=sqlx::query("SELECT label,expires_at::text AS expires_at FROM layrs_identity.device_requests WHERE user_digest=$1 AND deployment_id=$2 AND expires_at>clock_timestamp() AND consumed_at IS NULL AND approved_principal_id IS NULL")
        .bind(token_digest(&code)).bind(&identity.deployment_id).fetch_optional(&identity.pool).await.map_err(db)?.ok_or(IdentityError::Invalid)?;
    Ok(Json(
        serde_json::json!({"label":row.get::<String,_>("label"),"expiresAt":row.get::<String,_>("expires_at")}),
    ))
}

pub(super) async fn page() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'self'; script-src 'self'; style-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        include_str!("device.html"),
    )
}
pub(super) async fn script() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        include_str!("device.js"),
    )
}

pub(super) async fn start(
    State(identity): State<PgIdentity>,
    headers: HeaderMap,
    Json(request): Json<StartRequest>,
) -> Result<Json<serde_json::Value>> {
    identity.check_origin(&headers)?;
    management::validate_label(&request.label)?;
    let device = random_secret();
    let user = random_secret()[..10].to_ascii_uppercase();
    let mut tx = identity.pool.begin().await.map_err(db)?;
    // Short, deployment-scoped lock bounds anonymous pending requests across server processes.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1),4)")
        .bind(&identity.deployment_id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    sqlx::query("DELETE FROM layrs_identity.device_requests WHERE deployment_id=$1 AND expires_at<clock_timestamp()-interval '1 day'").bind(&identity.deployment_id).execute(&mut *tx).await.map_err(db)?;
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM layrs_identity.device_requests WHERE deployment_id=$1 AND expires_at>clock_timestamp() AND consumed_at IS NULL")
        .bind(&identity.deployment_id).fetch_one(&mut *tx).await.map_err(db)?;
    if count >= 1000 {
        return Err(IdentityError::RateLimited);
    }
    sqlx::query("INSERT INTO layrs_identity.device_requests(device_digest,user_digest,deployment_id,label) VALUES($1,$2,$3,$4)")
        .bind(token_digest(&device)).bind(token_digest(&user)).bind(&identity.deployment_id).bind(request.label).execute(&mut *tx).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(Json(
        serde_json::json!({"deviceCode":device,"userCode":format!("{}-{}",&user[..5],&user[5..]),"verificationUri":format!("{}/auth/device",identity.origin),"expiresIn":600,"interval":5}),
    ))
}

pub(super) async fn approve(
    State(identity): State<PgIdentity>,
    headers: HeaderMap,
    Json(request): Json<Approval>,
) -> Result<StatusCode> {
    let code = normalize_user_code(&request.user_code)?;
    let mut tx = identity.pool.begin().await.map_err(db)?;
    let (principal, generation) =
        management::authenticated_in_tx(&identity, &headers, &mut tx, false).await?;
    let human:bool=sqlx::query_scalar("SELECT principal_kind IN ('account','bootstrap_admin') FROM principals WHERE principal_id=$1")
        .bind(&principal).fetch_one(&mut *tx).await.map_err(db)?;
    if !human {
        return Err(IdentityError::Unauthorized);
    }
    let result=sqlx::query("UPDATE layrs_identity.device_requests SET approved_principal_id=$3,approved_generation=$4 WHERE deployment_id=$1 AND user_digest=$2 AND expires_at>clock_timestamp() AND consumed_at IS NULL AND approved_principal_id IS NULL")
        .bind(&identity.deployment_id).bind(token_digest(&code)).bind(principal).bind(generation).execute(&mut *tx).await.map_err(db)?;
    if result.rows_affected() != 1 {
        return Err(IdentityError::Invalid);
    }
    tx.commit().await.map_err(db)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn poll(
    State(identity): State<PgIdentity>,
    headers: HeaderMap,
    Json(request): Json<PollRequest>,
) -> Result<impl IntoResponse> {
    identity.check_origin(&headers)?;
    if request.device_code.len() != 64
        || !request.device_code.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(IdentityError::Invalid);
    }
    let digest = token_digest(&request.device_code);
    let mut tx = identity.pool.begin().await.map_err(db)?;
    let row=sqlx::query("SELECT label,approved_principal_id,approved_generation,next_poll_at<=clock_timestamp() AS can_poll FROM layrs_identity.device_requests WHERE device_digest=$1 AND deployment_id=$2 AND expires_at>clock_timestamp() AND consumed_at IS NULL FOR UPDATE")
        .bind(&digest).bind(&identity.deployment_id).fetch_optional(&mut *tx).await.map_err(db)?.ok_or(IdentityError::Unauthorized)?;
    if !row.get::<bool, _>("can_poll") {
        return Err(IdentityError::RateLimited);
    }
    sqlx::query("UPDATE layrs_identity.device_requests SET next_poll_at=clock_timestamp()+interval '5 seconds' WHERE device_digest=$1").bind(&digest).execute(&mut *tx).await.map_err(db)?;
    let Some(principal) = row.get::<Option<String>, _>("approved_principal_id") else {
        tx.commit().await.map_err(db)?;
        return Ok((
            StatusCode::ACCEPTED,
            Json(serde_json::json!({"status":"authorization_pending","interval":5})),
        ));
    };
    let generation: i64 = row.get("approved_generation");
    let active:Option<String>=sqlx::query_scalar("SELECT principal_id FROM principals WHERE principal_id=$1 AND deployment_id=$2 AND status='active' AND authorization_generation=$3 AND principal_kind IN ('account','bootstrap_admin') FOR SHARE")
        .bind(&principal).bind(&identity.deployment_id).bind(generation).fetch_optional(&mut *tx).await.map_err(db)?;
    if active.is_none() {
        return Err(IdentityError::Unauthorized);
    }
    let session = management::issue_token(
        &mut tx,
        &principal,
        generation,
        "device",
        &row.get::<String, _>("label"),
        30 * 24 * 3600,
    )
    .await?;
    sqlx::query("UPDATE layrs_identity.device_requests SET consumed_at=clock_timestamp() WHERE device_digest=$1").bind(digest).execute(&mut *tx).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok((
        StatusCode::OK,
        Json(serde_json::to_value(session).map_err(|_| IdentityError::Unavailable)?),
    ))
}

fn normalize_user_code(code: &str) -> Result<String> {
    let code = code.trim().replace('-', "").to_ascii_uppercase();
    if code.len() != 10 || !code.bytes().all(|b| b.is_ascii_hexdigit()) {
        Err(IdentityError::Invalid)
    } else {
        Ok(code)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codes_allow_only_fixed_length_hex() {
        assert_eq!(normalize_user_code("abcde-12345").unwrap(), "ABCDE12345");
        for code in ["12345", "ABCDE12345<script>", "ABCDEZZZZZ"] {
            assert!(normalize_user_code(code).is_err());
        }
    }
}
