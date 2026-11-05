-- Forward migration; keep identity outside the fingerprinted SCM namespace.
CREATE SCHEMA layrs_identity;
REVOKE ALL ON SCHEMA layrs_identity FROM PUBLIC;

CREATE TABLE layrs_identity.bootstrap (
    deployment_id TEXT PRIMARY KEY REFERENCES deployments(deployment_id),
    secret_digest TEXT NOT NULL CHECK (secret_digest ~ '^[0-9a-f]{64}$'),
    consumed_at TIMESTAMPTZ,
    administrator_id TEXT REFERENCES principals(principal_id),
    CHECK ((consumed_at IS NULL) = (administrator_id IS NULL))
);
CREATE TABLE layrs_identity.accounts (
    principal_id TEXT PRIMARY KEY REFERENCES principals(principal_id),
    deployment_id TEXT NOT NULL REFERENCES deployments(deployment_id),
    login TEXT NOT NULL CHECK (length(login) BETWEEN 3 AND 254),
    password_hash TEXT NOT NULL CHECK (password_hash LIKE '$argon2id$%'),
    failed_attempts INTEGER NOT NULL DEFAULT 0 CHECK (failed_attempts >= 0),
    attempt_window TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (deployment_id, login)
);
CREATE TABLE layrs_identity.tokens (
    token_digest TEXT PRIMARY KEY CHECK (token_digest ~ '^[0-9a-f]{64}$'),
    principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    authorization_generation BIGINT NOT NULL CHECK (authorization_generation > 0),
    token_kind TEXT NOT NULL CHECK (token_kind IN ('session', 'device', 'service')),
    label TEXT NOT NULL CHECK (length(label) BETWEEN 1 AND 128),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    CHECK (expires_at > created_at)
);
CREATE INDEX tokens_principal ON layrs_identity.tokens(principal_id);
DO $identity_grants$
DECLARE writer TEXT;
BEGIN
    SELECT writer_role_name INTO STRICT writer FROM schema_metadata WHERE singleton_id=1;
    EXECUTE format('GRANT USAGE ON SCHEMA layrs_identity TO %I', writer);
    EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA layrs_identity TO %I', writer);
END
$identity_grants$;
