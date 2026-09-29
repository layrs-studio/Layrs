//! Authorized Space catalog: the Spaces the caller may open, by name.
//!
//! Candidates are the ready Spaces of Workspaces where the caller holds an
//! active membership; each candidate is then admitted only by the same
//! `space.bootstrap` decision that guards `GET /v3/spaces/:space/bootstrap`.
//! Nothing is cached, so a revocation hides a Space from the next page.
//! Responses never reveal how many candidates were hidden, and the resume
//! position is sealed because it may name a hidden Space.
use super::*;
use crate::v3::sealed_cursor::SealedCursorCodec;
use serde::Deserialize;

pub const SPACE_CATALOG_DEFAULT_LIMIT: u16 = 50;
pub const SPACE_CATALOG_MAX_LIMIT: u16 = 100;
/// Bootstrap decisions evaluated per request. A page may therefore hold fewer
/// than `limit` entries (possibly none) while `hasMore` is true.
const SPACE_CATALOG_MAX_EVALUATIONS: usize = 256;
const CANDIDATE_BATCH: i64 = 64;
const CURSOR_DOMAIN: &str = "layrs.space-catalog.v1/name-then-id";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceCatalogQuery {
    pub limit: Option<u16>,
    pub cursor: Option<String>,
}

/// `limited` means content-restricted: the Space opens but paths are filtered
/// and whole-Space collaboration endpoints refuse the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpaceAccess {
    Full,
    Limited,
}
impl SpaceAccess {
    pub(super) fn from_open_decision(decision: &AuthorizationDecision) -> Self {
        if decision.content_restricted {
            Self::Limited
        } else {
            Self::Full
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceCatalogWorkspace {
    pub workspace_id: String,
    pub slug: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceCatalogEntry {
    pub space_id: SpaceId,
    pub slug: String,
    pub name: String,
    pub workspace: SpaceCatalogWorkspace,
    /// Stored Workspace membership role; per-Space rights come from policy.
    pub workspace_role: String,
    pub access: SpaceAccess,
    /// Time of the latest accepted repository event; `None` without event or
    /// for limited access. Review/comment activity is not included.
    pub last_activity_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceCatalogPage {
    pub schema_version: &'static str,
    pub spaces: Vec<SpaceCatalogEntry>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Position {
    sort_name: String,
    space_id: String,
}

impl PgCollaborationStore {
    pub async fn space_catalog(
        &self,
        principal: &PrincipalContext,
        query: SpaceCatalogQuery,
    ) -> V3StoreResult<SpaceCatalogPage> {
        let limit = usize::from(query.limit.unwrap_or(SPACE_CATALOG_DEFAULT_LIMIT));
        if !(1..=usize::from(SPACE_CATALOG_MAX_LIMIT)).contains(&limit) {
            return Err(V3StoreError::LimitExceeded {
                resource: "space_catalog_limit",
                limit: u64::from(SPACE_CATALOG_MAX_LIMIT),
            });
        }
        let cursors = SealedCursorCodec::new(self.repository.capabilities(), CURSOR_DOMAIN);
        let mut position = query
            .cursor
            .map(|token| {
                let bytes = cursors.open(&principal.principal_id, &token)?;
                serde_json::from_slice::<Position>(&bytes).map_err(|_| V3StoreError::InvalidCursor)
            })
            .transpose()?;
        let mut spaces = Vec::with_capacity(limit);
        let mut evaluated = 0;
        let mut has_more = false;
        'scan: loop {
            let rows = self
                .catalog_candidates(principal, position.as_ref())
                .await?;
            let exhausted = rows.len() < CANDIDATE_BATCH as usize;
            for row in rows {
                if evaluated == SPACE_CATALOG_MAX_EVALUATIONS {
                    has_more = true;
                    break 'scan;
                }
                evaluated += 1;
                let space: SpaceId = parse(row.get("space_id"))?;
                let Some(decision) = self.open_decision(principal, &space, &row).await? else {
                    position = Some(row_position(&row));
                    continue;
                };
                if decision.allowed {
                    if spaces.len() == limit {
                        // Resume before this visible Space; it opens the next page.
                        has_more = true;
                        break 'scan;
                    }
                    spaces.push(entry(&row, space, &decision));
                }
                position = Some(row_position(&row));
            }
            if exhausted {
                break;
            }
        }
        let next_cursor = match (&position, has_more) {
            (Some(position), true) => Some(cursors.seal(
                &principal.principal_id,
                &serde_json::to_vec(position).map_err(bad)?,
            )?),
            _ => None,
        };
        Ok(SpaceCatalogPage {
            schema_version: "layrs.space-catalog.v1",
            spaces,
            next_cursor,
            has_more,
        })
    }

    async fn catalog_candidates(
        &self,
        principal: &PrincipalContext,
        after: Option<&Position>,
    ) -> V3StoreResult<Vec<sqlx::postgres::PgRow>> {
        sqlx::query(
            r#"
            SELECT s.space_id, s.slug, s.display_name, lower(s.display_name) AS sort_name,
                   s.current_repository_view_id, s.current_repository_view_generation,
                   s.current_policy_set_id, s.authorization_epoch, s.next_event_sequence,
                   w.workspace_id, w.slug AS workspace_slug, w.display_name AS workspace_name,
                   m.role,
                   to_char(e.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"')
                       AS last_activity_at
            FROM workspace_memberships m
            JOIN workspaces w ON w.workspace_id = m.workspace_id AND w.deployment_id = $2
            JOIN spaces s ON s.workspace_id = w.workspace_id AND s.lifecycle_state = 'ready'
            LEFT JOIN space_events e
              ON e.space_id = s.space_id AND e.sequence = s.next_event_sequence - 1
            WHERE m.principal_id = $1 AND m.state = 'active'
              AND ($3::text IS NULL OR (lower(s.display_name), s.space_id) > ($3::text, $4::text))
            ORDER BY lower(s.display_name), s.space_id
            LIMIT $5
            "#,
        )
        .bind(principal.principal_id.as_str())
        .bind(&principal.deployment_id)
        .bind(after.map(|p| p.sort_name.as_str()))
        .bind(after.map(|p| p.space_id.as_str()))
        .bind(CANDIDATE_BATCH)
        .fetch_all(self.repository.pool())
        .await
        .map_err(pg_error)
    }

    /// The `space.bootstrap` decision for one candidate. `None` when the Space
    /// stopped being ready meanwhile. A concurrent policy change is retried
    /// once against the new authority, then reported to the caller.
    async fn open_decision(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        row: &sqlx::postgres::PgRow,
    ) -> V3StoreResult<Option<AuthorizationDecision>> {
        let authority = SpaceAuthority {
            space_id: space.clone(),
            repository_view_id: parse(row.get("current_repository_view_id"))?,
            repository_view_generation: row.get::<i64, _>("current_repository_view_generation")
                as u64,
            policy_set_id: parse(row.get("current_policy_set_id"))?,
            authorization_epoch: row.get::<i64, _>("authorization_epoch") as u64,
            watermark_sequence: row.get::<i64, _>("next_event_sequence") as u64 - 1,
        };
        match self
            .decide(principal, &authority, V3Action::Bootstrap)
            .await
        {
            Err(V3StoreError::AuthorizationEpochChanged) => {}
            other => return other.map(Some),
        }
        let authority = match self.current_authority(space).await {
            Err(V3StoreError::NotFound) => return Ok(None),
            other => other?,
        };
        self.decide(principal, &authority, V3Action::Bootstrap)
            .await
            .map(Some)
    }
}

fn row_position(row: &sqlx::postgres::PgRow) -> Position {
    Position {
        sort_name: row.get("sort_name"),
        space_id: row.get("space_id"),
    }
}

fn entry(
    row: &sqlx::postgres::PgRow,
    space_id: SpaceId,
    decision: &AuthorizationDecision,
) -> SpaceCatalogEntry {
    let access = SpaceAccess::from_open_decision(decision);
    SpaceCatalogEntry {
        space_id,
        slug: row.get("slug"),
        name: row.get("display_name"),
        workspace: SpaceCatalogWorkspace {
            workspace_id: row.get("workspace_id"),
            slug: row.get("workspace_slug"),
            name: row.get("workspace_name"),
        },
        workspace_role: row.get("role"),
        access,
        last_activity_at: match access {
            SpaceAccess::Full => row.get("last_activity_at"),
            SpaceAccess::Limited => None,
        },
    }
}
