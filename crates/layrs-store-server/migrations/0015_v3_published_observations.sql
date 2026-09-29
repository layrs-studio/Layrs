-- Server observations describe first admission, never local capture authorship.
CREATE TABLE layrs_collaboration.published_step_observations (
    ordinal BIGINT NOT NULL CHECK(ordinal > 0),
    observation_id TEXT UNIQUE NOT NULL,
    space_id TEXT NOT NULL,
    step_id TEXT NOT NULL,
    principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    request_digest TEXT NOT NULL CHECK(request_digest ~ '^blake3:[0-9a-f]{64}$'),
    record_digest TEXT NOT NULL CHECK(record_digest ~ '^blake3:[0-9a-f]{64}$'),
    received_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY(space_id,ordinal),
    UNIQUE(space_id,step_id),
    FOREIGN KEY(space_id,observation_id) REFERENCES step_observations(space_id,observation_id),
    FOREIGN KEY(space_id,step_id) REFERENCES space_steps(space_id,step_id)
);
CREATE INDEX published_observations_snapshot ON layrs_collaboration.published_step_observations(space_id,ordinal);
DO $observation_grants$
DECLARE writer TEXT;
BEGIN
    SELECT writer_role_name INTO STRICT writer FROM schema_metadata WHERE singleton_id=1;
    EXECUTE format('GRANT SELECT,INSERT ON layrs_collaboration.published_step_observations TO %I',writer);
END
$observation_grants$;
