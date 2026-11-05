-- Project-local teams. Policy Products remain the authorization authority.
CREATE TABLE layrs_collaboration.space_teams (
    team_id TEXT PRIMARY KEY CHECK(team_id ~ '^team:[0-9a-f]{64}$'),
    space_id TEXT NOT NULL REFERENCES spaces(space_id),
    current_version INTEGER NOT NULL CHECK(current_version BETWEEN 1 AND 1000),
    UNIQUE(space_id,team_id)
);
CREATE TABLE layrs_collaboration.team_versions (
    team_id TEXT NOT NULL,
    space_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK(version BETWEEN 1 AND 1000),
    mutation_id TEXT UNIQUE NOT NULL,
    actor_principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    request_digest TEXT NOT NULL,
    evidence_object_id TEXT UNIQUE NOT NULL REFERENCES canonical_objects(object_id),
    canonical_document_jcs TEXT NOT NULL CHECK(octet_length(canonical_document_jcs) BETWEEN 2 AND 32768),
    PRIMARY KEY(team_id,version),
    FOREIGN KEY(space_id,team_id) REFERENCES layrs_collaboration.space_teams(space_id,team_id)
);
CREATE TABLE layrs_collaboration.team_members (
    team_id TEXT NOT NULL,
    space_id TEXT NOT NULL,
    principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    PRIMARY KEY(team_id,principal_id),
    FOREIGN KEY(space_id,team_id) REFERENCES layrs_collaboration.space_teams(space_id,team_id)
);
DO $team_grants$
DECLARE writer TEXT;
BEGIN
    SELECT writer_role_name INTO STRICT writer FROM schema_metadata WHERE singleton_id=1;
    EXECUTE format('GRANT SELECT,INSERT,UPDATE ON layrs_collaboration.space_teams TO %I',writer);
    EXECUTE format('GRANT SELECT,INSERT ON layrs_collaboration.team_versions TO %I',writer);
    EXECUTE format('GRANT SELECT,INSERT,DELETE ON layrs_collaboration.team_members TO %I',writer);
END
$team_grants$;
