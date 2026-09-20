//! Transactionally coherent Sync V3 bootstrap projection.

use layrs_core::scm::{ObjectId, RepositoryView, RepositoryViewId, SpaceId, StepId};
use layrs_sync::v3::{
    BootstrapType, DecisionId, GateDefinitionId, GateEvaluationId, LayerProjection,
    MarkerProjection, PolicyId, PolicySet, PolicySetProjection, ProductEnvelope, ProductRoot,
    ReleaseProjection, RepositoryViewProjection, SpaceBootstrap, SyncV3Protocol, WeaveLogicalId,
};
use sqlx::Row;

use super::{
    AuthorizationCheck, AuthorizationTarget, BootstrapCursorIssuer, PgV3Repository,
    PrincipalContext, V3Action, V3StoreError, V3StoreResult, pg_error, require_allowed,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceAuthority {
    pub space_id: SpaceId,
    pub repository_view_id: RepositoryViewId,
    pub repository_view_generation: u64,
    pub policy_set_id: ObjectId,
    pub authorization_epoch: u64,
    pub watermark_sequence: u64,
}

#[derive(Clone)]
pub struct PgBootstrapStore<C> {
    repository: PgV3Repository,
    cursors: C,
}

impl<C> PgBootstrapStore<C>
where
    C: BootstrapCursorIssuer,
{
    pub fn new(repository: PgV3Repository, cursors: C) -> Self {
        Self {
            repository,
            cursors,
        }
    }

    pub async fn load(
        &self,
        principal: &PrincipalContext,
        space_id: &SpaceId,
    ) -> V3StoreResult<SpaceBootstrap> {
        let initial = self.load_authority(space_id).await?;
        let bootstrap_decision = self
            .repository
            .authorizer()
            .authorize(&AuthorizationCheck {
                principal: principal.clone(),
                action: V3Action::Bootstrap,
                target: AuthorizationTarget::Space(space_id.clone()),
                policy_set_id: initial.policy_set_id.clone(),
                authorization_epoch: initial.authorization_epoch,
            })
            .await?;
        let content_access =
            bootstrap_decision
                .content_restricted
                .then_some(layrs_sync::v3::ContentAccess {
                    schema_version: layrs_sync::v3::ContentAccessSchema::V1,
                    mode: layrs_sync::v3::ContentAccessMode::Restricted,
                });
        require_allowed(bootstrap_decision)?;

        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(pg_error)?;
        let authority = load_authority_in(&mut tx, space_id).await?;
        if authority.authorization_epoch != initial.authorization_epoch
            || authority.policy_set_id != initial.policy_set_id
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }

        let view_jcs: String = sqlx::query_scalar(
            r#"
            SELECT canonical_record_jcs
            FROM repository_views
            WHERE space_id = $1 AND repository_view_id = $2
            "#,
        )
        .bind(space_id.as_str())
        .bind(authority.repository_view_id.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(pg_error)?;
        let view: RepositoryView = parse_jcs(&view_jcs, "RepositoryView")?;
        if view.id() != &authority.repository_view_id || view.space_id() != space_id {
            return Err(V3StoreError::CorruptObject(
                "RepositoryView projection does not match Space authority".into(),
            ));
        }

        let policy_jcs: String = sqlx::query_scalar(
            r#"
            SELECT canonical_record_jcs
            FROM policy_sets
            WHERE space_id = $1 AND policy_set_id = $2
            "#,
        )
        .bind(space_id.as_str())
        .bind(authority.policy_set_id.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(pg_error)?;
        let policy_set: PolicySet = parse_jcs(&policy_jcs, "PolicySet")?;
        if policy_set.policy_set_id != authority.policy_set_id
            || policy_set.space_id != *space_id
            || policy_set.computed_id().map_err(|error| {
                V3StoreError::CorruptObject(format!("invalid PolicySet identity: {error}"))
            })? != authority.policy_set_id
        {
            return Err(V3StoreError::CorruptObject(
                "PolicySet projection does not match Space authority".into(),
            ));
        }

        let layers = load_layers(&mut tx, &authority).await?;
        let markers = load_markers(&mut tx, &authority).await?;
        let releases = load_releases(&mut tx, &authority).await?;
        let policy_roots = load_policy_roots(&mut tx, &authority).await?;
        let mut graph_root_step_ids = view
            .layer_heads()
            .iter()
            .flat_map(|heads| heads.step_ids().iter().cloned())
            .collect::<Vec<_>>();
        graph_root_step_ids.sort();
        graph_root_step_ids.dedup();

        let mut product_roots = load_product_roots(&mut tx, &authority).await?;
        product_roots.sort();

        let watermark_cursor = self.cursors.issue_bootstrap_cursor(
            space_id,
            &principal.principal_id,
            authority.authorization_epoch,
            authority.watermark_sequence,
        )?;
        tx.commit().await.map_err(pg_error)?;
        Ok(SpaceBootstrap {
            protocol: SyncV3Protocol::V3,
            message_type: BootstrapType::Bootstrap,
            principal_id: principal.principal_id.clone(),
            content_access,
            authorization_epoch: authority.authorization_epoch,
            space_id: space_id.clone(),
            watermark_cursor,
            watermark_sequence: authority.watermark_sequence,
            current_repository_view_generation: authority.repository_view_generation,
            current_repository_view: RepositoryViewProjection {
                repository_view_id: authority.repository_view_id,
                record: view,
            },
            current_policy_set_id: authority.policy_set_id.clone(),
            current_policy_set: PolicySetProjection {
                policy_set_id: authority.policy_set_id,
                record: policy_set,
            },
            layers,
            markers,
            releases,
            policy_roots,
            graph_root_step_ids,
            product_roots,
        })
    }

    pub async fn load_authority(&self, space_id: &SpaceId) -> V3StoreResult<SpaceAuthority> {
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        let authority = load_authority_in(&mut tx, space_id).await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(authority)
    }
}

async fn load_authority_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    space_id: &SpaceId,
) -> V3StoreResult<SpaceAuthority> {
    let row = sqlx::query(
        r#"
        SELECT current_repository_view_id, current_repository_view_generation,
               current_policy_set_id, authorization_epoch, next_event_sequence - 1 AS watermark
        FROM spaces
        WHERE space_id = $1 AND lifecycle_state = 'ready'
        "#,
    )
    .bind(space_id.as_str())
    .fetch_optional(&mut **tx)
    .await
    .map_err(pg_error)?
    .ok_or(V3StoreError::InvalidCapability)?;
    Ok(SpaceAuthority {
        space_id: space_id.clone(),
        repository_view_id: RepositoryViewId::parse(
            row.get::<String, _>("current_repository_view_id"),
        )
        .map_err(|_| V3StoreError::CorruptObject("invalid RepositoryViewId".into()))?,
        repository_view_generation: from_i64(
            row.get("current_repository_view_generation"),
            "view generation",
        )?,
        policy_set_id: ObjectId::parse(row.get::<String, _>("current_policy_set_id"))
            .map_err(|_| V3StoreError::CorruptObject("invalid PolicySet ID".into()))?,
        authorization_epoch: from_i64(row.get("authorization_epoch"), "epoch")?,
        watermark_sequence: from_i64(row.get("watermark"), "watermark")?,
    })
}

async fn load_layers(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    authority: &SpaceAuthority,
) -> V3StoreResult<Vec<LayerProjection>> {
    let rows = sqlx::query(
        r#"
        SELECT p.product_object_id, p.canonical_envelope_jcs
        FROM current_layer_definitions d
        JOIN products p
          ON p.space_id = d.space_id
         AND p.product_object_id = d.current_product_object_id
        WHERE d.space_id = $1
        ORDER BY d.layer_id COLLATE "C"
        "#,
    )
    .bind(authority.space_id.as_str())
    .fetch_all(&mut **tx)
    .await
    .map_err(pg_error)?;
    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let object_id = parse_object_id(&row, "product_object_id")?;
        let envelope: ProductEnvelope = parse_jcs(
            &row.get::<String, _>("canonical_envelope_jcs"),
            "Layer Product",
        )?;
        let (claimed, record) = match envelope {
            ProductEnvelope::Layer {
                product_object_id,
                record,
                ..
            } => (product_object_id, record),
            _ => return Err(V3StoreError::CorruptObject("non-Layer definition".into())),
        };
        if claimed != object_id || record.space_id() != &authority.space_id {
            return Err(V3StoreError::CorruptObject("Layer Product mismatch".into()));
        }
        let head_rows = sqlx::query_scalar::<_, String>(
            r#"
            SELECT step_id FROM repository_view_layer_heads
            WHERE space_id = $1 AND repository_view_id = $2 AND layer_id = $3
            ORDER BY step_id COLLATE "C"
            "#,
        )
        .bind(authority.space_id.as_str())
        .bind(authority.repository_view_id.as_str())
        .bind(record.id().as_str())
        .fetch_all(&mut **tx)
        .await
        .map_err(pg_error)?;
        let head_step_ids = head_rows
            .into_iter()
            .map(|value| {
                StepId::parse(value)
                    .map_err(|_| V3StoreError::CorruptObject("invalid StepId".into()))
            })
            .collect::<V3StoreResult<Vec<_>>>()?;
        result.push(LayerProjection {
            product_object_id: object_id,
            record,
            head_step_ids,
        });
    }
    Ok(result)
}

async fn load_markers(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    authority: &SpaceAuthority,
) -> V3StoreResult<Vec<MarkerProjection>> {
    let rows = sqlx::query(
        r#"
        SELECT p.product_object_id, p.canonical_envelope_jcs
        FROM repository_view_markers m
        JOIN products p ON p.space_id=m.space_id AND p.product_object_id=m.marker_product_object_id
        WHERE m.space_id=$1 AND m.repository_view_id=$2
        ORDER BY m.marker_id COLLATE "C"
        "#,
    )
    .bind(authority.space_id.as_str())
    .bind(authority.repository_view_id.as_str())
    .fetch_all(&mut **tx)
    .await
    .map_err(pg_error)?;
    rows.into_iter()
        .map(|row| {
            let object_id = parse_object_id(&row, "product_object_id")?;
            let envelope: ProductEnvelope = parse_jcs(
                &row.get::<String, _>("canonical_envelope_jcs"),
                "Marker Product",
            )?;
            match envelope {
                ProductEnvelope::Marker {
                    product_object_id,
                    record,
                    ..
                } if product_object_id == object_id => Ok(MarkerProjection {
                    product_object_id: object_id,
                    record,
                }),
                _ => Err(V3StoreError::CorruptObject(
                    "Marker Product mismatch".into(),
                )),
            }
        })
        .collect()
}

async fn load_releases(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    authority: &SpaceAuthority,
) -> V3StoreResult<Vec<ReleaseProjection>> {
    let rows = sqlx::query(
        r#"
        SELECT p.product_object_id, p.canonical_envelope_jcs
        FROM repository_view_releases r
        JOIN products p ON p.space_id=r.space_id AND p.product_object_id=r.release_product_object_id
        WHERE r.space_id=$1 AND r.repository_view_id=$2
        ORDER BY r.release_id COLLATE "C"
        "#,
    )
    .bind(authority.space_id.as_str())
    .bind(authority.repository_view_id.as_str())
    .fetch_all(&mut **tx)
    .await
    .map_err(pg_error)?;
    rows.into_iter()
        .map(|row| {
            let object_id = parse_object_id(&row, "product_object_id")?;
            let envelope: ProductEnvelope = parse_jcs(
                &row.get::<String, _>("canonical_envelope_jcs"),
                "Release Product",
            )?;
            match envelope {
                ProductEnvelope::Release {
                    product_object_id,
                    record,
                    ..
                } if product_object_id == object_id => Ok(ReleaseProjection {
                    product_object_id: object_id,
                    record,
                }),
                _ => Err(V3StoreError::CorruptObject(
                    "Release Product mismatch".into(),
                )),
            }
        })
        .collect()
}

async fn load_policy_roots(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    authority: &SpaceAuthority,
) -> V3StoreResult<Vec<ProductRoot>> {
    let rows = sqlx::query(
        r#"
        SELECT policy_id, policy_product_object_id
        FROM policy_set_entries
        WHERE space_id=$1 AND policy_set_id=$2
        ORDER BY ordinal
        "#,
    )
    .bind(authority.space_id.as_str())
    .bind(authority.policy_set_id.as_str())
    .fetch_all(&mut **tx)
    .await
    .map_err(pg_error)?;
    rows.into_iter()
        .map(|row| {
            Ok(ProductRoot::Policy {
                logical_id: PolicyId::parse(row.get::<String, _>("policy_id"))
                    .map_err(|_| V3StoreError::CorruptObject("invalid PolicyId".into()))?,
                object_id: ObjectId::parse(row.get::<String, _>("policy_product_object_id"))
                    .map_err(|_| V3StoreError::CorruptObject("invalid Policy object ID".into()))?,
            })
        })
        .collect()
}

async fn load_product_roots(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    authority: &SpaceAuthority,
) -> V3StoreResult<Vec<ProductRoot>> {
    let rows = sqlx::query(
        r#"
        SELECT product_kind, logical_id, product_object_id
        FROM products
        WHERE space_id=$1
        ORDER BY product_kind COLLATE "C", logical_id COLLATE "C", product_object_id
        "#,
    )
    .bind(authority.space_id.as_str())
    .fetch_all(&mut **tx)
    .await
    .map_err(pg_error)?;
    rows.into_iter()
        .map(|row| {
            let kind: String = row.get("product_kind");
            let logical: String = row.get("logical_id");
            let object_id = parse_object_id(&row, "product_object_id")?;
            match kind.as_str() {
                "layer" => Ok(ProductRoot::Layer {
                    logical_id: layrs_core::scm::LayerId::parse(logical)
                        .map_err(|_| V3StoreError::CorruptObject("invalid LayerId".into()))?,
                    object_id,
                }),
                "marker" => Ok(ProductRoot::Marker {
                    logical_id: layrs_core::scm::MarkerId::parse(logical)
                        .map_err(|_| V3StoreError::CorruptObject("invalid MarkerId".into()))?,
                    object_id,
                }),
                "release" => Ok(ProductRoot::Release {
                    logical_id: layrs_core::scm::ReleaseId::parse(logical)
                        .map_err(|_| V3StoreError::CorruptObject("invalid ReleaseId".into()))?,
                    object_id,
                }),
                "proof" => Ok(ProductRoot::Proof {
                    logical_id: layrs_core::scm::ProofId::parse(logical)
                        .map_err(|_| V3StoreError::CorruptObject("invalid ProofId".into()))?,
                    object_id,
                }),
                "gateDefinition" => Ok(ProductRoot::GateDefinition {
                    logical_id: GateDefinitionId::parse(logical).map_err(|_| {
                        V3StoreError::CorruptObject("invalid GateDefinitionId".into())
                    })?,
                    object_id,
                }),
                "gateEvaluation" => Ok(ProductRoot::GateEvaluation {
                    logical_id: GateEvaluationId::parse(logical).map_err(|_| {
                        V3StoreError::CorruptObject("invalid GateEvaluationId".into())
                    })?,
                    object_id,
                }),
                "policy" => Ok(ProductRoot::Policy {
                    logical_id: PolicyId::parse(logical)
                        .map_err(|_| V3StoreError::CorruptObject("invalid PolicyId".into()))?,
                    object_id,
                }),
                "weave" => Ok(ProductRoot::Weave {
                    logical_id: WeaveLogicalId::parse(logical)
                        .map_err(|_| V3StoreError::CorruptObject("invalid Weave ID".into()))?,
                    object_id,
                }),
                "decision" => Ok(ProductRoot::Decision {
                    logical_id: DecisionId::parse(logical)
                        .map_err(|_| V3StoreError::CorruptObject("invalid DecisionId".into()))?,
                    object_id,
                }),
                _ => Err(V3StoreError::CorruptObject("invalid Product kind".into())),
            }
        })
        .collect()
}

fn parse_jcs<T: serde::de::DeserializeOwned>(value: &str, kind: &str) -> V3StoreResult<T> {
    serde_json::from_str(value)
        .map_err(|error| V3StoreError::CorruptObject(format!("invalid {kind}: {error}")))
}

fn parse_object_id(row: &sqlx::postgres::PgRow, column: &str) -> V3StoreResult<ObjectId> {
    ObjectId::parse(row.get::<String, _>(column))
        .map_err(|_| V3StoreError::CorruptObject(format!("invalid {column}")))
}

fn from_i64(value: i64, field: &str) -> V3StoreResult<u64> {
    u64::try_from(value).map_err(|_| V3StoreError::CorruptObject(format!("invalid {field}")))
}
