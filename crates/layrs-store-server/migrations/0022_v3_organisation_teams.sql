-- Reusable organisation directory. Legacy space_teams and their policy history
-- are deliberately left intact; no existing grant is widened by this migration.
CREATE TABLE layrs_collaboration.organisation_teams (
    team_id TEXT PRIMARY KEY CHECK (team_id ~ '^org-team:[0-9a-f]{64}$'),
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE RESTRICT,
    current_version INTEGER NOT NULL CHECK (current_version BETWEEN 1 AND 1000),
    name TEXT NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 128),
    state TEXT NOT NULL CHECK (state IN ('active','archived')),
    CHECK (current_version < 1000 OR state = 'archived'),
    UNIQUE (workspace_id, team_id)
);
CREATE TABLE layrs_collaboration.organisation_team_members (
    team_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    principal_id TEXT NOT NULL,
    PRIMARY KEY (team_id, principal_id),
    FOREIGN KEY (workspace_id, team_id) REFERENCES layrs_collaboration.organisation_teams(workspace_id, team_id),
    FOREIGN KEY (workspace_id, principal_id) REFERENCES workspace_memberships(workspace_id, principal_id)
);
CREATE TABLE layrs_collaboration.organisation_team_versions (
    team_id TEXT NOT NULL REFERENCES layrs_collaboration.organisation_teams(team_id),
    version INTEGER NOT NULL CHECK (version BETWEEN 1 AND 1000),
    mutation_id TEXT NOT NULL UNIQUE,
    request_digest TEXT NOT NULL,
    actor_principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    canonical_document_jcs TEXT NOT NULL CHECK (octet_length(canonical_document_jcs) BETWEEN 2 AND 32768),
    PRIMARY KEY (team_id, version)
);
CREATE TABLE layrs_collaboration.organisation_team_assignments (
    team_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    space_id TEXT NOT NULL,
    current_version INTEGER NOT NULL CHECK (current_version BETWEEN 1 AND 1000),
    role TEXT CHECK (role IN ('member','auditor','release_manager')),
    CHECK (current_version < 1000 OR role IS NULL),
    PRIMARY KEY (team_id, space_id),
    FOREIGN KEY (workspace_id, team_id) REFERENCES layrs_collaboration.organisation_teams(workspace_id, team_id),
    FOREIGN KEY (space_id) REFERENCES spaces(space_id)
);
-- Keep the immutable core catalog unchanged. Validate the boundary on this
-- extension table instead of adding a constraint to core Spaces.
CREATE FUNCTION layrs_collaboration.check_organisation_team_assignment() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM spaces WHERE space_id=NEW.space_id AND workspace_id=NEW.workspace_id) THEN
        RAISE EXCEPTION 'organisation team and project must share a workspace' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER organisation_team_assignment_workspace
    BEFORE INSERT OR UPDATE ON layrs_collaboration.organisation_team_assignments
    FOR EACH ROW EXECUTE FUNCTION layrs_collaboration.check_organisation_team_assignment();
CREATE INDEX organisation_team_assignments_space ON layrs_collaboration.organisation_team_assignments(space_id);
CREATE TABLE layrs_collaboration.organisation_team_assignment_versions (
    team_id TEXT NOT NULL,
    space_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version BETWEEN 1 AND 1000),
    mutation_id TEXT NOT NULL UNIQUE,
    request_digest TEXT NOT NULL,
    actor_principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    canonical_document_jcs TEXT NOT NULL CHECK (octet_length(canonical_document_jcs) BETWEEN 2 AND 32768),
    PRIMARY KEY (team_id, space_id, version),
    FOREIGN KEY (team_id, space_id) REFERENCES layrs_collaboration.organisation_team_assignments(team_id, space_id)
);
DO $org_team_grants$
DECLARE writer TEXT;
BEGIN
    SELECT writer_role_name INTO STRICT writer FROM schema_metadata WHERE singleton_id=1;
    EXECUTE format('GRANT SELECT,INSERT,UPDATE ON layrs_collaboration.organisation_teams TO %I', writer);
    EXECUTE format('GRANT SELECT,INSERT,DELETE ON layrs_collaboration.organisation_team_members TO %I', writer);
    EXECUTE format('GRANT SELECT,INSERT ON layrs_collaboration.organisation_team_versions TO %I', writer);
    EXECUTE format('GRANT SELECT,INSERT,UPDATE ON layrs_collaboration.organisation_team_assignments TO %I', writer);
    EXECUTE format('GRANT SELECT,INSERT ON layrs_collaboration.organisation_team_assignment_versions TO %I', writer);
END
$org_team_grants$;
