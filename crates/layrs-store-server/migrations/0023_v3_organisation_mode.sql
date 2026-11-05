-- Administration-owned organisation mode. Workspaces and memberships stay intact
-- when a non-primary organisation is temporarily hidden.
ALTER TABLE layrs_collaboration.deployment_project_contexts
    ADD COLUMN primary_workspace_id TEXT REFERENCES workspaces(workspace_id) ON DELETE SET NULL,
    ADD COLUMN mode_revision BIGINT NOT NULL DEFAULT 0 CHECK (mode_revision >= 0),
    ADD COLUMN multi_organisation_managed BOOLEAN NOT NULL DEFAULT FALSE;

-- Existing single-company installations must retain direct sync access before
-- any caller opens the context catalog after an upgrade.
UPDATE layrs_collaboration.deployment_project_contexts dc
SET primary_workspace_id=c.workspace_id
FROM layrs_collaboration.workspace_contexts c
WHERE c.deployment_id=dc.deployment_id AND c.kind='company'
  AND dc.primary_workspace_id IS NULL;

DO $organisation_mode_grants$
DECLARE writer TEXT;
BEGIN
    SELECT writer_role_name INTO STRICT writer FROM schema_metadata WHERE singleton_id=1;
    EXECUTE format('GRANT UPDATE (multi_organisation_enabled,primary_workspace_id,mode_revision,multi_organisation_managed) ON layrs_collaboration.deployment_project_contexts TO %I',writer);
END
$organisation_mode_grants$;
