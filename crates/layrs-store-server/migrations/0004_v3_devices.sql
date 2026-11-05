CREATE TABLE layrs_identity.device_requests (
    device_digest TEXT PRIMARY KEY CHECK (device_digest ~ '^[0-9a-f]{64}$'),
    user_digest TEXT NOT NULL UNIQUE CHECK (user_digest ~ '^[0-9a-f]{64}$'),
    deployment_id TEXT NOT NULL REFERENCES deployments(deployment_id),
    label TEXT NOT NULL CHECK (length(label) BETWEEN 1 AND 128),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()+interval '10 minutes',
    next_poll_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    approved_principal_id TEXT REFERENCES principals(principal_id),
    approved_generation BIGINT,
    consumed_at TIMESTAMPTZ,
    CHECK ((approved_principal_id IS NULL) = (approved_generation IS NULL)),
    CHECK (consumed_at IS NULL OR approved_principal_id IS NOT NULL)
);
CREATE INDEX device_requests_expiry ON layrs_identity.device_requests(expires_at);
DO $device_grants$
DECLARE writer TEXT;
BEGIN
    SELECT writer_role_name INTO STRICT writer FROM schema_metadata WHERE singleton_id=1;
    EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON layrs_identity.device_requests TO %I',writer);
END
$device_grants$;
