async fn workspace_values(pool: &PgPool, account_id: &str) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT w.workspace_id, w.name, w.slug,
               to_char(w.updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS updated_at
        FROM workspaces w
        JOIN workspace_memberships m ON m.workspace_id = w.workspace_id
        WHERE m.account_id = $1 AND m.state = 'active'
        ORDER BY w.created_at ASC
        "#,
    )
    .bind(account_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            workspace_value(
                row.get("workspace_id"),
                row.get("name"),
                row.get("slug"),
                "",
                row.get("updated_at"),
            )
        })
        .collect())
}

fn workspace_value(id: &str, name: &str, slug: &str, description: &str, updated_at: &str) -> Value {
    json!({
        "id": id,
        "name": name,
        "slug": slug,
        "description": description,
        "health": "pending",
        "updatedAt": updated_at
    })
}

async fn team_values(pool: &PgPool, workspace_id: &str) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT t.team_id, t.name, t.purpose, count(tm.account_id)::bigint AS members
        FROM teams t
        LEFT JOIN team_memberships tm ON tm.team_id = t.team_id
        WHERE t.workspace_id = $1
        GROUP BY t.team_id, t.name, t.purpose, t.created_at
        ORDER BY t.created_at ASC
        "#,
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>("team_id"),
                "workspaceId": workspace_id,
                "name": row.get::<String, _>("name"),
                "purpose": row.get::<String, _>("purpose"),
                "members": row.get::<i64, _>("members"),
                "gateResponsibility": "workspace"
            })
        })
        .collect())
}

async fn team_value(pool: &PgPool, workspace_id: &str, team_id: &str) -> Result<Value, ApiError> {
    team_values(pool, workspace_id)
        .await?
        .into_iter()
        .find(|team| team.get("id").and_then(Value::as_str) == Some(team_id))
        .ok_or_else(|| ApiError::not_found("team not found"))
}

async fn workspace_member_values(
    pool: &PgPool,
    workspace_id: &str,
) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT a.account_id, a.email, a.display_name, m.role, m.state,
               to_char(m.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at
        FROM workspace_memberships m
        JOIN accounts a ON a.account_id = m.account_id
        WHERE m.workspace_id = $1 AND m.state = 'active'
        ORDER BY a.display_name ASC, a.email ASC
        "#,
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "accountId": row.get::<String, _>("account_id"),
                "email": row.get::<String, _>("email"),
                "displayName": row.get::<String, _>("display_name"),
                "role": row.get::<String, _>("role"),
                "state": row.get::<String, _>("state"),
                "createdAt": row.get::<String, _>("created_at")
            })
        })
        .collect())
}

async fn team_member_values(
    pool: &PgPool,
    workspace_id: &str,
    team_id: &str,
) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT a.account_id, a.email, a.display_name, tm.role,
               to_char(tm.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at
        FROM team_memberships tm
        JOIN teams t ON t.team_id = tm.team_id
        JOIN accounts a ON a.account_id = tm.account_id
        JOIN workspace_memberships wm ON wm.workspace_id = t.workspace_id
            AND wm.account_id = a.account_id
            AND wm.state = 'active'
        WHERE t.workspace_id = $1 AND tm.team_id = $2
        ORDER BY a.display_name ASC, a.email ASC
        "#,
    )
    .bind(workspace_id)
    .bind(team_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "teamId": team_id,
                "accountId": row.get::<String, _>("account_id"),
                "email": row.get::<String, _>("email"),
                "displayName": row.get::<String, _>("display_name"),
                "role": row.get::<String, _>("role"),
                "createdAt": row.get::<String, _>("created_at")
            })
        })
        .collect())
}

async fn team_member_value(
    pool: &PgPool,
    workspace_id: &str,
    team_id: &str,
    account_id: &str,
) -> Result<Value, ApiError> {
    team_member_values(pool, workspace_id, team_id)
        .await?
        .into_iter()
        .find(|member| member.get("accountId").and_then(Value::as_str) == Some(account_id))
        .ok_or_else(|| ApiError::not_found("team member not found"))
}

async fn upsert_team_member(
    pool: &PgPool,
    workspace_id: &str,
    team_id: &str,
    account_id: &str,
    role: &str,
) -> Result<(), ApiError> {
    ensure_team_in_workspace(pool, workspace_id, team_id).await?;
    sqlx::query(
        r#"
        INSERT INTO team_memberships (team_id, account_id, role)
        VALUES ($1, $2, $3)
        ON CONFLICT (team_id, account_id) DO UPDATE
        SET role = excluded.role
        "#,
    )
    .bind(team_id)
    .bind(account_id)
    .bind(role)
    .execute(pool)
    .await?;
    Ok(())
}

async fn create_pending_invitation(
    pool: &PgPool,
    workspace_id: &str,
    email: &str,
    workspace_role: &str,
    invited_by_account_id: &str,
    team_ids: &[String],
    team_role: &str,
) -> Result<String, ApiError> {
    let invitation_id = prefixed_id("invitation");
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"
        INSERT INTO invitations
            (invitation_id, workspace_id, email, role, invited_by_account_id, expires_at, status)
        VALUES
            ($1, $2, $3, $4, $5, now() + interval '14 days', 'pending')
        "#,
    )
    .bind(&invitation_id)
    .bind(workspace_id)
    .bind(email)
    .bind(workspace_role)
    .bind(invited_by_account_id)
    .execute(&mut *tx)
    .await?;

    for team_id in unique_strings(team_ids) {
        sqlx::query(
            "INSERT INTO invitation_team_assignments (invitation_id, team_id, role) VALUES ($1, $2, $3)",
        )
        .bind(&invitation_id)
        .bind(&team_id)
        .bind(team_role)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(invitation_id)
}

async fn invitation_value(pool: &PgPool, invitation_id: &str) -> Result<Value, ApiError> {
    invitation_values(pool, Some(invitation_id), None, None)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::not_found("invitation not found"))
}

async fn invitation_values_for_workspace(
    pool: &PgPool,
    workspace_id: &str,
) -> Result<Vec<Value>, ApiError> {
    invitation_values(pool, None, Some(workspace_id), None).await
}

async fn invitation_values_for_email(pool: &PgPool, email: &str) -> Result<Vec<Value>, ApiError> {
    invitation_values(pool, None, None, Some(email)).await
}

async fn invitation_values(
    pool: &PgPool,
    invitation_id: Option<&str>,
    workspace_id: Option<&str>,
    email: Option<&str>,
) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT i.invitation_id, i.workspace_id, w.name AS workspace_name, i.email, i.role, i.status,
               to_char(i.expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS expires_at,
               to_char(i.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at,
               coalesce(array_remove(array_agg(ita.team_id ORDER BY t.name), NULL), '{}') AS team_ids
        FROM invitations i
        JOIN workspaces w ON w.workspace_id = i.workspace_id
        LEFT JOIN invitation_team_assignments ita ON ita.invitation_id = i.invitation_id
        LEFT JOIN teams t ON t.team_id = ita.team_id
        WHERE ($1::text IS NULL OR i.invitation_id = $1)
          AND ($2::text IS NULL OR i.workspace_id = $2)
          AND ($3::text IS NULL OR lower(i.email) = lower($3))
        GROUP BY i.invitation_id, i.workspace_id, w.name, i.email, i.role, i.status, i.expires_at, i.created_at
        ORDER BY i.created_at DESC
        "#,
    )
    .bind(invitation_id)
    .bind(workspace_id)
    .bind(email)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>("invitation_id"),
                "workspaceId": row.get::<String, _>("workspace_id"),
                "workspaceName": row.get::<String, _>("workspace_name"),
                "email": row.get::<String, _>("email"),
                "role": row.get::<String, _>("role"),
                "status": row.get::<String, _>("status"),
                "teamIds": row.try_get::<Vec<String>, _>("team_ids").unwrap_or_default(),
                "expiresAt": row.get::<String, _>("expires_at"),
                "createdAt": row.get::<String, _>("created_at")
            })
        })
        .collect())
}

async fn accept_or_decline_invitation(
    pool: &PgPool,
    invitation_id: &str,
    user: &UserPrincipal,
    accept: bool,
) -> Result<(), ApiError> {
    let row = sqlx::query(
        r#"
        SELECT invitation_id, workspace_id, email, role, status, expires_at < now() AS expired
        FROM invitations
        WHERE invitation_id = $1
        "#,
    )
    .bind(invitation_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("invitation not found"))?;

    let email: String = row.get("email");
    if normalize_email(&email)? != user.email {
        return Err(ApiError::forbidden("invitation belongs to another email"));
    }
    let status: String = row.get("status");
    let expired: bool = row.get("expired");
    if status != "pending" || expired {
        return Err(ApiError::bad_request("invitation is not pending"));
    }

    let workspace_id: String = row.get("workspace_id");
    let workspace_role: String = row.get("role");
    let mut tx = pool.begin().await?;
    if accept {
        sqlx::query(
            "UPDATE invitations SET status = 'accepted', accepted_at = now() WHERE invitation_id = $1",
        )
        .bind(invitation_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO workspace_memberships (membership_id, workspace_id, account_id, role, state)
            VALUES ($1, $2, $3, $4, 'active')
            ON CONFLICT (workspace_id, account_id) DO UPDATE
            SET role = excluded.role, state = 'active', updated_at = now()
            "#,
        )
        .bind(prefixed_id("membership"))
        .bind(&workspace_id)
        .bind(&user.id)
        .bind(&workspace_role)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO team_memberships (team_id, account_id, role)
            SELECT team_id, $2, role
            FROM invitation_team_assignments
            WHERE invitation_id = $1
            ON CONFLICT (team_id, account_id) DO UPDATE
            SET role = excluded.role
            "#,
        )
        .bind(invitation_id)
        .bind(&user.id)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query(
            "UPDATE invitations SET status = 'declined', declined_at = now() WHERE invitation_id = $1",
        )
        .bind(invitation_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn space_values(pool: &PgPool, workspace_id: &str) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT s.space_id, s.name, s.description,
               to_char(s.updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS updated_at,
               (
                 SELECT l.layer_id
                 FROM layers l
                 WHERE l.space_id = s.space_id
                 ORDER BY l.created_at ASC
                 LIMIT 1
               ) AS current_layer_id
        FROM spaces s
        WHERE s.workspace_id = $1
        ORDER BY s.created_at ASC
        "#,
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>("space_id"),
                "workspaceId": workspace_id,
                "teamId": "",
                "name": row.get::<String, _>("name"),
                "description": row.get::<String, _>("description"),
                "status": "pending",
                "currentLayerId": row.try_get::<String, _>("current_layer_id").unwrap_or_default(),
                "updatedAt": row.get::<String, _>("updated_at")
            })
        })
        .collect())
}

async fn layer_values(pool: &PgPool, workspace_id: &str) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT l.layer_id, l.space_id, l.parent_layer_id, l.name,
               COALESCE(
                   (
                       SELECT array_agg(a.artifact_id ORDER BY a.created_at)
                       FROM artifacts a
                       WHERE a.workspace_id = l.workspace_id
                         AND a.layer_id = l.layer_id
                         AND a.state <> 'deleted'
                   ),
                   ARRAY[]::text[]
               ) AS artifact_ids,
               COALESCE(
                   (
                       SELECT array_agg(s.step_id ORDER BY s.captured_at, s.created_at)
                       FROM layer_steps s
                       WHERE s.workspace_id = l.workspace_id
                         AND s.space_id = l.space_id
                         AND s.layer_id = l.layer_id
                   ),
                   ARRAY[]::text[]
               ) AS step_ids
        FROM layers l
        WHERE l.workspace_id = $1
        ORDER BY l.created_at ASC
        "#,
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            let parent_id = row.try_get::<String, _>("parent_layer_id").ok();
            let artifact_ids = row
                .try_get::<Vec<String>, _>("artifact_ids")
                .unwrap_or_default();
            let step_ids = row
                .try_get::<Vec<String>, _>("step_ids")
                .unwrap_or_default();
            json!({
                "id": row.get::<String, _>("layer_id"),
                "spaceId": row.get::<String, _>("space_id"),
                "parentId": parent_id,
                "name": row.get::<String, _>("name"),
                "kind": if parent_id.is_some() { "proposal" } else { "base" },
                "status": "active",
                "summary": "Persisted Layer",
                "artifactIds": artifact_ids,
                "stepIds": step_ids,
                "gateIds": []
            })
        })
        .collect())
}

async fn artifact_values(pool: &PgPool, workspace_id: &str) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT artifact_id, space_id, layer_id, logical_path, artifact_kind, state,
               to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS updated_at
        FROM artifacts
        WHERE workspace_id = $1 AND state <> 'deleted'
        ORDER BY logical_path ASC
        "#,
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            let path = row.get::<String, _>("logical_path");
            let state = row.get::<String, _>("state");
            let redacted = state == "redacted";
            json!({
                "id": row.get::<String, _>("artifact_id"),
                "spaceId": row.get::<String, _>("space_id"),
                "layerId": row.get::<String, _>("layer_id"),
                "name": path.rsplit('/').next().unwrap_or(&path),
                "type": artifact_type(row.get::<String, _>("artifact_kind").as_str()),
                "summary": if redacted { "Restricted by Layer access policy" } else { "Persisted artifact" },
                "location": path,
                "updatedAt": row.get::<String, _>("updated_at"),
                "sizeLabel": if redacted { "redacted" } else { "stored" },
                "proofIds": [],
                "access": {
                    "mode": if redacted { "none" } else { "read" },
                    "canOpen": !redacted,
                    "isRedacted": redacted,
                    "reason": if redacted { "Restricted by Layer access policy" } else { "" }
                }
            })
        })
        .collect())
}

async fn artifact_values_for_layer(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    account_id: &str,
) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT artifact_id, logical_path, artifact_kind, state, current_file_object_id, current_tree_id,
               to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS updated_at
        FROM artifacts
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3 AND state <> 'deleted'
        ORDER BY logical_path ASC
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_all(pool)
    .await?;

    let mut values = Vec::new();
    for row in rows {
        let artifact_id = row.get::<String, _>("artifact_id");
        let path = row.get::<String, _>("logical_path");
        let state = row.get::<String, _>("state");
        let decision = access_decision_for_path(
            pool,
            workspace_id,
            space_id,
            layer_id,
            &path,
            Some(&artifact_id),
            account_id,
        )
        .await?;
        let redacted = state == "redacted" || !decision.can_read;
        let reason = if state == "redacted" {
            "Artifact state is redacted"
        } else {
            decision.reason.as_str()
        };
        values.push(artifact_metadata_value(
            workspace_id,
            space_id,
            layer_id,
            &artifact_id,
            &path,
            row.get::<String, _>("artifact_kind").as_str(),
            row.get::<String, _>("updated_at").as_str(),
            row.try_get::<String, _>("current_file_object_id").ok(),
            row.try_get::<String, _>("current_tree_id").ok(),
            redacted,
            reason,
        ));
    }
    Ok(values)
}

