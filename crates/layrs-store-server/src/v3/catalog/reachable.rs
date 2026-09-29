use super::*;

/// Traverses typed repository roots as well as content edges. Path components
/// accumulate through hierarchical Trees; a basename elsewhere never matches.
pub(super) async fn rooted_reachable(
    pool: &sqlx::PgPool,
    space: &SpaceId,
    root: &ObjectId,
    target: &str,
    path: Option<&str>,
    file: bool,
) -> V3StoreResult<bool> {
    sqlx::query_scalar(r#"
        WITH RECURSIVE edges(source,target,component) AS (
            SELECT source_object_id,target_object_id,repo_path FROM canonical_object_edges WHERE target_object_id IS NOT NULL
            UNION ALL SELECT e.source_object_id,m.file_manifest_id,e.repo_path FROM canonical_object_edges e JOIN file_manifests m ON m.file_id=e.target_file_id
            UNION ALL SELECT step_id,root_tree_id,NULL::TEXT FROM steps
            UNION ALL SELECT step_id,parent_step_id,NULL::TEXT FROM step_parents WHERE $4::TEXT IS NULL
            UNION ALL SELECT repository_view_id,step_id,NULL::TEXT FROM repository_view_layer_heads WHERE space_id=$1
            UNION ALL SELECT repository_view_id,marker_product_object_id,NULL::TEXT FROM repository_view_markers WHERE space_id=$1
            UNION ALL SELECT repository_view_id,release_product_object_id,NULL::TEXT FROM repository_view_releases WHERE space_id=$1
            UNION ALL SELECT product_object_id,step_id,NULL::TEXT FROM product_step_dependencies WHERE space_id=$1
            UNION ALL SELECT product_object_id,dependency_product_object_id,NULL::TEXT FROM product_dependencies WHERE space_id=$1
            UNION ALL SELECT policy_set_id,policy_product_object_id,NULL::TEXT FROM policy_set_entries WHERE space_id=$1
        ), reachable(object_id,repo_path,depth) AS (
            SELECT $2::TEXT,''::TEXT,0
            UNION
            SELECT edge.target,
                CASE WHEN edge.component IS NULL THEN parent.repo_path
                     WHEN parent.repo_path='' THEN edge.component
                     ELSE parent.repo_path || '/' || edge.component END,
                parent.depth+1
            FROM reachable parent JOIN edges edge ON edge.source=parent.object_id
            JOIN space_object_availability a ON a.space_id=$1 AND a.object_id=edge.target AND a.availability_state='available'
            JOIN canonical_objects o ON o.object_id=edge.target AND o.verification_state='verified'
            WHERE parent.depth<256
        )
        SELECT CASE WHEN $5::BOOLEAN THEN EXISTS (
            SELECT 1 FROM reachable r JOIN canonical_object_edges e ON e.source_object_id=r.object_id
            WHERE e.target_file_id=$3 AND CASE WHEN e.repo_path IS NULL THEN r.repo_path WHEN r.repo_path='' THEN e.repo_path ELSE r.repo_path || '/' || e.repo_path END=$4
        ) ELSE EXISTS (SELECT 1 FROM reachable WHERE object_id=$3 AND ($4::TEXT IS NULL OR repo_path=$4)) END
    "#).bind(space.as_str()).bind(root.as_str()).bind(target).bind(path).bind(file).fetch_one(pool).await.map_err(pg_error)
}
