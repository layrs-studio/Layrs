async fn device_values(pool: &PgPool, account_id: &str) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT device_id, display_name, status,
               to_char(coalesce(last_seen_at, created_at) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS last_seen_at
        FROM desktop_devices
        WHERE account_id = $1
        ORDER BY created_at DESC
        "#,
    )
    .bind(account_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>("device_id"),
                "accountId": account_id,
                "name": row.get::<String, _>("display_name"),
                "kind": "desktop",
                "status": row.get::<String, _>("status"),
                "lastSeenAt": row.get::<String, _>("last_seen_at")
            })
        })
        .collect())
}

async fn audit_event_values(pool: &PgPool, workspace_id: &str) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT audit_event_id, actor_account_id, action, target_kind, target_id,
               to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at
        FROM audit_events
        WHERE workspace_id = $1
        ORDER BY created_at DESC
        LIMIT 50
        "#,
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            let action = row.get::<String, _>("action");
            let target_kind = row.get::<String, _>("target_kind");
            let target_id = row.try_get::<String, _>("target_id").unwrap_or_default();
            json!({
                "id": row.get::<String, _>("audit_event_id"),
                "workspaceId": workspace_id,
                "actorAccountId": row.try_get::<String, _>("actor_account_id").unwrap_or_default(),
                "action": action,
                "target": if target_id.is_empty() { target_kind.clone() } else { format!("{target_kind}:{target_id}") },
                "summary": format!("{action} on {target_kind}"),
                "at": row.get::<String, _>("created_at")
            })
        })
        .collect())
}

async fn space_summaries_for_workspaces(
    pool: &PgPool,
    workspace_ids: &[String],
) -> Result<Vec<Value>, ApiError> {
    let mut values = Vec::new();
    for workspace_id in workspace_ids {
        for value in space_values(pool, workspace_id).await? {
            values.push(json!({
                "id": value["id"],
                "workspaceId": value["workspaceId"],
                "name": value["name"],
                "currentLayerId": value["currentLayerId"]
            }));
        }
    }
    Ok(values)
}

async fn layer_summaries_for_workspaces(
    pool: &PgPool,
    workspace_ids: &[String],
) -> Result<Vec<Value>, ApiError> {
    let mut values = Vec::new();
    for workspace_id in workspace_ids {
        let layers = layer_values(pool, workspace_id).await?;
        for layer in layers {
            let space_id = layer
                .get("spaceId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            values.push(json!({
                "id": layer["id"],
                "workspaceId": workspace_id,
                "spaceId": space_id,
                "name": layer["name"],
                "kind": layer["kind"],
                "parentLayerId": layer.get("parentId").cloned().unwrap_or(Value::Null),
                "access": "open"
            }));
        }
    }
    Ok(values)
}

async fn layer_access_policy_value(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
) -> Result<Value, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT policy_id, policy_epoch,
               to_char(generated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS generated_at,
               coalesce(signature_key_id, 'server_key_local') AS signature_key_id,
               coalesce(signature_value, 'unsigned-dev') AS signature_value
        FROM layer_access_policies
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("layer access policy not found"))?;
    let policy_id = row.get::<String, _>("policy_id");
    let rule_rows = sqlx::query(
        r#"
        SELECT rule_id, path, artifact_id, mode, visibility,
               read_account_ids, read_team_ids, write_account_ids, write_team_ids, admin_account_ids, admin_team_ids
        FROM layer_access_policy_rules
        WHERE policy_id = $1
        ORDER BY created_at ASC
        "#,
    )
    .bind(&policy_id)
    .fetch_all(pool)
    .await?;
    let rules = rule_rows
        .iter()
        .map(|rule| {
            json!({
                "id": rule.get::<String, _>("rule_id"),
                "path": rule.get::<String, _>("path"),
                "artifact_id": rule.try_get::<String, _>("artifact_id").ok(),
                "mode": rule.get::<String, _>("mode"),
                "visibility": rule.get::<String, _>("visibility"),
                "permissions": {
                    "read": {
                        "accounts": rule.try_get::<Vec<String>, _>("read_account_ids").unwrap_or_default(),
                        "teams": rule.try_get::<Vec<String>, _>("read_team_ids").unwrap_or_default()
                    },
                    "write": {
                        "accounts": rule.try_get::<Vec<String>, _>("write_account_ids").unwrap_or_default(),
                        "teams": rule.try_get::<Vec<String>, _>("write_team_ids").unwrap_or_default()
                    },
                    "admin": {
                        "accounts": rule.try_get::<Vec<String>, _>("admin_account_ids").unwrap_or_default(),
                        "teams": rule.try_get::<Vec<String>, _>("admin_team_ids").unwrap_or_default()
                    }
                }
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "schema": "layrs.layer_access.v1",
        "workspace_id": workspace_id,
        "space_id": space_id,
        "layer_id": layer_id,
        "policy_epoch": row.get::<i64, _>("policy_epoch"),
        "generated_at": row.get::<String, _>("generated_at"),
        "rules": rules,
        "signature": {
            "key_id": row.get::<String, _>("signature_key_id"),
            "value": row.get::<String, _>("signature_value")
        }
    }))
}

async fn load_user_by_email(pool: &PgPool, email: &str) -> Result<Option<UserPrincipal>, ApiError> {
    let row = sqlx::query(
        "SELECT account_id, email, display_name FROM accounts WHERE email = $1 AND status = 'active'",
    )
    .bind(email)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| row_user(&row)))
}

async fn create_workspace_owner_only(
    pool: &PgPool,
    account_id: &str,
    workspace_id: &str,
    name: &str,
    slug: &str,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO workspaces (workspace_id, slug, name, created_by_account_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(workspace_id)
    .bind(slug)
    .bind(name)
    .bind(account_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO workspace_memberships (membership_id, workspace_id, account_id, role) VALUES ($1, $2, $3, 'owner')",
    )
    .bind(prefixed_id("membership"))
    .bind(workspace_id)
    .bind(account_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn insert_empty_layer_policy(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    account_id: Option<&str>,
) -> Result<String, ApiError> {
    let policy_id = prefixed_id("access_policy");
    sqlx::query(
        r#"
        INSERT INTO layer_access_policies
            (policy_id, workspace_id, space_id, layer_id, registry_path, policy_epoch, updated_by_account_id, signature_key_id, signature_value)
        VALUES
            ($1, $2, $3, $4, $5, 1, $6, 'server_key_local', 'unsigned-dev')
        "#,
    )
    .bind(&policy_id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(format!(".layrs/layers/{layer_id}/access.json"))
    .bind(account_id)
    .execute(&mut **tx)
    .await?;
    Ok(policy_id)
}

async fn inherit_layer_access_rules_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    parent_layer_id: &str,
    child_policy_id: &str,
) -> Result<(), ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT r.path, r.artifact_id, r.mode, r.visibility,
               r.read_account_ids, r.read_team_ids, r.write_account_ids, r.write_team_ids,
               r.admin_account_ids, r.admin_team_ids
        FROM layer_access_policy_rules r
        JOIN layer_access_policies p ON p.policy_id = r.policy_id
        WHERE p.workspace_id = $1 AND p.space_id = $2 AND p.layer_id = $3
        ORDER BY r.created_at ASC
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(parent_layer_id)
    .fetch_all(&mut **tx)
    .await?;

    for row in rows {
        sqlx::query(
            r#"
            INSERT INTO layer_access_policy_rules
                (rule_id, policy_id, path, artifact_id, mode, visibility,
                 read_account_ids, read_team_ids, write_account_ids, write_team_ids, admin_account_ids, admin_team_ids)
            VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            "#,
        )
        .bind(prefixed_id("access_rule"))
        .bind(child_policy_id)
        .bind(row.get::<String, _>("path"))
        .bind(row.try_get::<String, _>("artifact_id").ok())
        .bind(row.get::<String, _>("mode"))
        .bind(row.get::<String, _>("visibility"))
        .bind(row.try_get::<Vec<String>, _>("read_account_ids").unwrap_or_default())
        .bind(row.try_get::<Vec<String>, _>("read_team_ids").unwrap_or_default())
        .bind(row.try_get::<Vec<String>, _>("write_account_ids").unwrap_or_default())
        .bind(row.try_get::<Vec<String>, _>("write_team_ids").unwrap_or_default())
        .bind(row.try_get::<Vec<String>, _>("admin_account_ids").unwrap_or_default())
        .bind(row.try_get::<Vec<String>, _>("admin_team_ids").unwrap_or_default())
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn upsert_layer_policy(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    account_id: Option<&str>,
) -> Result<String, ApiError> {
    let row = sqlx::query(
        r#"
        INSERT INTO layer_access_policies
            (policy_id, workspace_id, space_id, layer_id, registry_path, policy_epoch, updated_by_account_id, signature_key_id, signature_value)
        VALUES
            ($1, $2, $3, $4, $5, 1, $6, 'server_key_local', 'unsigned-dev')
        ON CONFLICT (layer_id) DO UPDATE
        SET policy_epoch = layer_access_policies.policy_epoch + 1,
            updated_by_account_id = excluded.updated_by_account_id,
            updated_at = now()
        RETURNING policy_id
        "#,
    )
    .bind(prefixed_id("access_policy"))
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(format!(".layrs/layers/{layer_id}/access.json"))
    .bind(account_id)
    .fetch_one(pool)
    .await?;
    Ok(row.get("policy_id"))
}

async fn policy_id_for_layer(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
) -> Result<String, ApiError> {
    sqlx::query_scalar(
        "SELECT policy_id FROM layer_access_policies WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3",
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("layer access policy not found"))
}

async fn insert_layer_access_rule(
    pool: &PgPool,
    policy_id: &str,
    rule_id: &str,
    rule: &LayerAccessRuleBody,
    account_id: Option<&str>,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    insert_layer_access_rule_in_tx(&mut tx, policy_id, rule_id, rule).await?;
    bump_policy_epoch_in_tx(&mut tx, policy_id, account_id).await?;
    tx.commit().await?;
    Ok(())
}

async fn update_layer_access_rule_row(
    pool: &PgPool,
    policy_id: &str,
    rule_id: &str,
    rule: &LayerAccessRuleBody,
    account_id: Option<&str>,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    let result = sqlx::query(
        r#"
        UPDATE layer_access_policy_rules
        SET path = $1,
            artifact_id = $2,
            mode = $3,
            visibility = $4,
            read_account_ids = $5,
            read_team_ids = $6,
            write_account_ids = $7,
            write_team_ids = $8,
            admin_account_ids = $9,
            admin_team_ids = $10
        WHERE policy_id = $11 AND rule_id = $12
        "#,
    )
    .bind(&rule.path)
    .bind(rule.artifact_id.as_deref())
    .bind(&rule.mode)
    .bind(&rule.visibility)
    .bind(&rule.permissions.read.accounts)
    .bind(&rule.permissions.read.teams)
    .bind(&rule.permissions.write.accounts)
    .bind(&rule.permissions.write.teams)
    .bind(&rule.permissions.admin.accounts)
    .bind(&rule.permissions.admin.teams)
    .bind(policy_id)
    .bind(rule_id)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("access rule not found"));
    }
    bump_policy_epoch_in_tx(&mut tx, policy_id, account_id).await?;
    tx.commit().await?;
    Ok(())
}

async fn delete_layer_access_rule_row(
    pool: &PgPool,
    policy_id: &str,
    rule_id: &str,
    account_id: Option<&str>,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    let result =
        sqlx::query("DELETE FROM layer_access_policy_rules WHERE policy_id = $1 AND rule_id = $2")
            .bind(policy_id)
            .bind(rule_id)
            .execute(&mut *tx)
            .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("access rule not found"));
    }
    bump_policy_epoch_in_tx(&mut tx, policy_id, account_id).await?;
    tx.commit().await?;
    Ok(())
}

async fn insert_layer_access_rule_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    policy_id: &str,
    rule_id: &str,
    rule: &LayerAccessRuleBody,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO layer_access_policy_rules
            (rule_id, policy_id, path, artifact_id, mode, visibility,
             read_account_ids, read_team_ids, write_account_ids, write_team_ids, admin_account_ids, admin_team_ids)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        "#,
    )
    .bind(rule_id)
    .bind(policy_id)
    .bind(&rule.path)
    .bind(rule.artifact_id.as_deref())
    .bind(&rule.mode)
    .bind(&rule.visibility)
    .bind(&rule.permissions.read.accounts)
    .bind(&rule.permissions.read.teams)
    .bind(&rule.permissions.write.accounts)
    .bind(&rule.permissions.write.teams)
    .bind(&rule.permissions.admin.accounts)
    .bind(&rule.permissions.admin.teams)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn bump_policy_epoch_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    policy_id: &str,
    account_id: Option<&str>,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        UPDATE layer_access_policies
        SET policy_epoch = policy_epoch + 1,
            updated_by_account_id = $1,
            updated_at = now()
        WHERE policy_id = $2
        "#,
    )
    .bind(account_id)
    .bind(policy_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn replace_layer_policy_rules(
    pool: &PgPool,
    workspace_id: &str,
    policy_id: &str,
    body: &LayerAccessPolicyBody,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;

    if let Some(signature) = &body.signature {
        sqlx::query(
            "UPDATE layer_access_policies SET signature_key_id = $1, signature_value = $2 WHERE policy_id = $3",
        )
        .bind(&signature.key_id)
        .bind(&signature.value)
        .bind(policy_id)
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query("DELETE FROM layer_access_policy_rules WHERE policy_id = $1")
        .bind(policy_id)
        .execute(&mut *tx)
        .await?;

    for rule in &body.rules {
        validate_layer_access_rule(pool, workspace_id, rule).await?;
        let rule_id = rule
            .id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| prefixed_id("access_rule"));
        sqlx::query(
            r#"
            INSERT INTO layer_access_policy_rules
                (rule_id, policy_id, path, artifact_id, mode, visibility,
                 read_account_ids, read_team_ids, write_account_ids, write_team_ids, admin_account_ids, admin_team_ids)
            VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            "#,
        )
        .bind(&rule_id)
        .bind(policy_id)
        .bind(&rule.path)
        .bind(rule.artifact_id.as_deref())
        .bind(&rule.mode)
        .bind(&rule.visibility)
        .bind(&rule.permissions.read.accounts)
        .bind(&rule.permissions.read.teams)
        .bind(&rule.permissions.write.accounts)
        .bind(&rule.permissions.write.teams)
        .bind(&rule.permissions.admin.accounts)
        .bind(&rule.permissions.admin.teams)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

async fn write_audit(
    pool: &PgPool,
    workspace_id: Option<&str>,
    actor_account_id: Option<&str>,
    action: &str,
    target_kind: &str,
    target_id: Option<&str>,
    metadata: Value,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO audit_events
            (audit_event_id, workspace_id, actor_account_id, action, target_kind, target_id, metadata_json)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(prefixed_id("audit"))
    .bind(workspace_id)
    .bind(actor_account_id)
    .bind(action)
    .bind(target_kind)
    .bind(target_id)
    .bind(metadata)
    .execute(pool)
    .await?;
    Ok(())
}
