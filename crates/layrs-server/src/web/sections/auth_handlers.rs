async fn server_page(State(state): State<AppState>) -> Html<String> {
    Html(format!(
        r#"<!doctype html>
<html lang="en">
  <head><meta charset="utf-8"><title>Layrs Server</title></head>
  <body>
    <main>
      <h1>Layrs Server</h1>
      <p>Postgres/sqlx runtime is active.</p>
      <p><a href="/healthz">/healthz</a> · <a href="/v1/routes">/v1/routes</a> · <a href="{studio}">Studio Web</a></p>
    </main>
  </body>
</html>"#,
        studio = escape_html(&state.config.studio_url)
    ))
}

async fn healthz(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    sqlx::query("SELECT 1").execute(&state.pool).await?;

    Ok(Json(json!({
        "service": "layrs-server",
        "status": "ok",
        "runtime": "axum",
        "store": "postgres",
        "deployment_id": state.config.deployment_id
    })))
}

async fn routes() -> Json<Value> {
    Json(Value::Array(ROUTES.iter().map(route_json).collect()))
}

async fn list_lenses() -> Json<Value> {
    Json(crate::lenses::registry_response_value_from_env())
}

async fn signup(
    State(state): State<AppState>,
    Json(body): Json<SignupRequest>,
) -> Result<Response, ApiError> {
    let email = normalize_email(&body.email)?;
    validate_password(&body.password)?;
    let display_name = body
        .display_name
        .or(body.name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| email.clone());
    let account_id = prefixed_id("account");
    let password_hash = hash_password(&body.password)?;

    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO accounts (account_id, email, display_name) VALUES ($1, $2, $3)")
        .bind(&account_id)
        .bind(&email)
        .bind(&display_name)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO account_passwords (account_id, password_hash) VALUES ($1, $2)")
        .bind(&account_id)
        .bind(&password_hash)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let user = UserPrincipal {
        id: account_id,
        email,
        display_name,
    };
    write_audit(
        &state.pool,
        None,
        Some(&user.id),
        "auth.signup",
        "account",
        Some(&user.id),
        json!({}),
    )
    .await?;
    session_response(&state, &user, StatusCode::CREATED).await
}

async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let email = normalize_email(&body.email)?;
    let row = sqlx::query(
        r#"
        SELECT a.account_id, a.email, a.display_name, p.password_hash
        FROM accounts a
        JOIN account_passwords p ON p.account_id = a.account_id
        WHERE a.email = $1 AND a.status = 'active'
        "#,
    )
    .bind(&email)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::unauthorized("email or password is incorrect"))?;
    let password_hash: String = row.get("password_hash");
    verify_password(&body.password, &password_hash)?;
    let user = row_user(&row);

    write_audit(
        &state.pool,
        None,
        Some(&user.id),
        "auth.login",
        "studio",
        None,
        json!({}),
    )
    .await?;
    session_response(&state, &user, StatusCode::OK).await
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    let Some(session_token) = session_cookie(&headers) else {
        return Err(ApiError::unauthorized("authentication is required"));
    };
    let digest = digest_secret(&session_token);
    sqlx::query("UPDATE web_sessions SET revoked_at = now() WHERE session_token_digest = $1")
        .bind(digest)
        .execute(&state.pool)
        .await?;

    let mut response = Json(json!({ "ok": true })).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&expired_session_cookie(&state.config))
            .map_err(|error| ApiError::bad_request(error.to_string()))?,
    );
    Ok(response)
}

async fn auth_session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    Ok(Json(auth_session_value(&state.pool, &user).await?))
}

async fn me(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    Ok(Json(json!({ "user": user_wire_json(&user) })))
}

