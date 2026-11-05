-- Context metadata is outside the immutable native catalog. Legacy
-- workspaces without a row remain organisations with unchanged memberships.
CREATE TABLE layrs_collaboration.deployment_project_contexts (
    deployment_id TEXT PRIMARY KEY REFERENCES deployments(deployment_id),
    multi_organisation_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    personal_projects_enabled BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE TABLE layrs_collaboration.workspace_contexts (
    workspace_id TEXT PRIMARY KEY REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    deployment_id TEXT NOT NULL REFERENCES deployments(deployment_id),
    kind TEXT NOT NULL CHECK (kind IN ('company','organisation','personal')),
    owner_principal_id TEXT REFERENCES principals(principal_id),
    CHECK ((kind='personal') = (owner_principal_id IS NOT NULL))
);
CREATE UNIQUE INDEX workspace_contexts_one_company
    ON layrs_collaboration.workspace_contexts(deployment_id) WHERE kind='company';
CREATE UNIQUE INDEX workspace_contexts_one_personal_per_owner
    ON layrs_collaboration.workspace_contexts(deployment_id,owner_principal_id)
    WHERE kind='personal';

DO $context_grants$
DECLARE writer TEXT;
BEGIN
    SELECT writer_role_name INTO STRICT writer FROM schema_metadata WHERE singleton_id=1;
    EXECUTE format('GRANT SELECT ON layrs_collaboration.deployment_project_contexts TO %I',writer);
    EXECUTE format('GRANT SELECT,INSERT ON layrs_collaboration.workspace_contexts TO %I',writer);
END
$context_grants$;
