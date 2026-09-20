CREATE TABLE layrs_collaboration.webhooks (
    webhook_id TEXT PRIMARY KEY CHECK(webhook_id LIKE 'webhook:%'),
    space_id TEXT NOT NULL REFERENCES spaces(space_id),
    url TEXT NOT NULL,
    event_kinds TEXT[] NOT NULL,
    secret_generation BIGINT NOT NULL DEFAULT 1 CHECK(secret_generation>0),
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    start_sequence BIGINT NOT NULL CHECK(start_sequence>=0),
    created_by TEXT NOT NULL REFERENCES principals(principal_id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE TABLE layrs_collaboration.webhook_deliveries (
    webhook_id TEXT NOT NULL REFERENCES layrs_collaboration.webhooks(webhook_id),
    notification_sequence BIGINT NOT NULL REFERENCES layrs_collaboration.notifications(sequence),
    delivery_id TEXT UNIQUE NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','sending','delivered','dead')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts>=0),
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    lease_until TIMESTAMPTZ,
    last_status INTEGER,
    last_error TEXT,
    delivered_at TIMESTAMPTZ,
    PRIMARY KEY(webhook_id,notification_sequence)
);
CREATE INDEX webhook_deliveries_due ON layrs_collaboration.webhook_deliveries(next_attempt_at) WHERE state IN ('pending','sending');
CREATE TABLE layrs_collaboration.webhook_audit (
    sequence BIGSERIAL PRIMARY KEY,
    webhook_id TEXT NOT NULL REFERENCES layrs_collaboration.webhooks(webhook_id),
    actor_principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    action TEXT NOT NULL CHECK(action IN ('create','rotate','disable','replay')),
    delivery_id TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
DO $webhook_grants$
DECLARE writer TEXT;
BEGIN
    SELECT writer_role_name INTO STRICT writer FROM schema_metadata WHERE singleton_id=1;
    EXECUTE format('GRANT SELECT,INSERT,UPDATE ON ALL TABLES IN SCHEMA layrs_collaboration TO %I',writer);
    EXECUTE format('GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA layrs_collaboration TO %I',writer);
END
$webhook_grants$;
