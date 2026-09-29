-- Runtime grants only; no schema shape change and no direct Layer-definition authority.
DO $writer_access$
DECLARE
    target_schema TEXT;
    writer TEXT;
    table_name TEXT;
BEGIN
    SELECT installed_schema, writer_role_name INTO STRICT target_schema, writer
    FROM schema_metadata WHERE singleton_id=1;
    EXECUTE format('GRANT USAGE ON SCHEMA %I TO %I', target_schema, writer);
    EXECUTE format('GRANT SELECT ON ALL TABLES IN SCHEMA %I TO %I', target_schema, writer);
    FOREACH table_name IN ARRAY ARRAY[
        'workspaces','workspace_memberships','spaces',
        'canonical_objects','chunks','file_manifests','file_manifest_chunks','canonical_object_edges',
        'space_object_availability','steps','step_parents','space_steps','step_observations',
        'products','product_step_dependencies','product_dependencies','evidence_descriptors',
        'policy_sets','policy_set_entries','repository_views','repository_view_layer_heads',
        'repository_view_markers','repository_view_releases','authorization_snapshots',
        'sync_transactions','repository_view_history','policy_root_history','space_events',
        'object_plans','object_plan_items','upload_capabilities','fetch_plans','fetch_plan_claims',
        'fetch_claim_objects','fetch_capabilities','object_leases','current_layer_heads_projection'
    ] LOOP
        EXECUTE format('GRANT INSERT ON %I.%I TO %I',target_schema,table_name,writer);
    END LOOP;
    -- PostgreSQL SELECT FOR SHARE/UPDATE also requires UPDATE privilege.
    FOREACH table_name IN ARRAY ARRAY[
        'deployments','spaces','workspace_memberships','canonical_objects','space_object_availability',
        'object_plans','object_plan_items','upload_capabilities','fetch_plans','fetch_plan_claims',
        'fetch_claim_objects','fetch_capabilities','object_leases'
    ] LOOP
        EXECUTE format('GRANT UPDATE ON %I.%I TO %I',target_schema,table_name,writer);
    END LOOP;
    EXECUTE format('GRANT DELETE ON %I.current_layer_heads_projection TO %I',target_schema,writer);
END
$writer_access$;
