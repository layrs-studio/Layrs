-- Layrs V1 server schema: auth, Workspaces, Teams, Spaces, Layers,
-- artifacts, object references, audit, and per-Layer access registries.

CREATE TABLE IF NOT EXISTS accounts (
    account_id TEXT PRIMARY KEY,
    email TEXT NOT NULL UNIQUE CHECK (position('@' in email) > 1),
    display_name TEXT NOT NULL CHECK (length(trim(display_name)) >= 2),
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'suspended', 'deleted')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS account_passwords (
    account_id TEXT PRIMARY KEY REFERENCES accounts(account_id) ON DELETE CASCADE,
    password_hash TEXT NOT NULL,
    password_algorithm TEXT NOT NULL DEFAULT 'argon2id',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS web_sessions (
    session_id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE,
    session_token_digest TEXT NOT NULL UNIQUE,
    user_agent TEXT,
    ip_hash TEXT,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS desktop_devices (
    device_id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE,
    display_name TEXT NOT NULL,
    public_key_thumbprint TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'trusted'
        CHECK (status IN ('trusted', 'pending', 'revoked')),
    last_seen_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS desktop_device_tokens (
    token_id TEXT PRIMARY KEY,
    device_id TEXT NOT NULL REFERENCES desktop_devices(device_id) ON DELETE CASCADE,
    account_id TEXT NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE,
    access_token_digest TEXT NOT NULL UNIQUE,
    refresh_token_digest TEXT NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS device_authorization_flows (
    device_flow_id TEXT PRIMARY KEY,
    device_code_digest TEXT NOT NULL UNIQUE,
    user_code_digest TEXT NOT NULL UNIQUE,
    account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL,
    requested_workspace_id TEXT,
    client_name TEXT NOT NULL,
    public_key_thumbprint TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'approved', 'denied', 'expired')),
    interval_seconds INTEGER NOT NULL DEFAULT 5 CHECK (interval_seconds > 0),
    poll_count INTEGER NOT NULL DEFAULT 0 CHECK (poll_count >= 0),
    expires_at TIMESTAMPTZ NOT NULL,
    approved_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS workspaces (
    workspace_id TEXT PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL CHECK (length(trim(name)) >= 2),
    created_by_account_id TEXT NOT NULL REFERENCES accounts(account_id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS workspace_memberships (
    membership_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    account_id TEXT NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member', 'viewer')),
    state TEXT NOT NULL DEFAULT 'active'
        CHECK (state IN ('invited', 'active', 'suspended', 'removed')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, account_id)
);

CREATE TABLE IF NOT EXISTS teams (
    team_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (length(trim(name)) >= 2),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, name)
);

CREATE TABLE IF NOT EXISTS team_memberships (
    team_id TEXT NOT NULL REFERENCES teams(team_id) ON DELETE CASCADE,
    account_id TEXT NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT 'member' CHECK (role IN ('maintainer', 'member')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (team_id, account_id)
);

CREATE TABLE IF NOT EXISTS spaces (
    space_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    slug TEXT NOT NULL,
    name TEXT NOT NULL CHECK (length(trim(name)) >= 2),
    description TEXT NOT NULL DEFAULT '',
    created_by_account_id TEXT NOT NULL REFERENCES accounts(account_id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, slug)
);

CREATE TABLE IF NOT EXISTS space_memberships (
    membership_id TEXT PRIMARY KEY,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    account_id TEXT REFERENCES accounts(account_id) ON DELETE CASCADE,
    team_id TEXT REFERENCES teams(team_id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('admin', 'writer', 'reader')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (
        (account_id IS NOT NULL AND team_id IS NULL)
        OR (account_id IS NULL AND team_id IS NOT NULL)
    )
);

CREATE TABLE IF NOT EXISTS layers (
    layer_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    parent_layer_id TEXT REFERENCES layers(layer_id) ON DELETE SET NULL,
    name TEXT NOT NULL CHECK (length(trim(name)) >= 2),
    registry_inheritance TEXT NOT NULL DEFAULT 'inherited'
        CHECK (registry_inheritance IN ('inherited', 'overridden')),
    created_by_account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS layer_memberships (
    membership_id TEXT PRIMARY KEY,
    layer_id TEXT NOT NULL REFERENCES layers(layer_id) ON DELETE CASCADE,
    account_id TEXT REFERENCES accounts(account_id) ON DELETE CASCADE,
    team_id TEXT REFERENCES teams(team_id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('admin', 'writer', 'reader')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (
        (account_id IS NOT NULL AND team_id IS NULL)
        OR (account_id IS NULL AND team_id IS NOT NULL)
    )
);

CREATE TABLE IF NOT EXISTS artifacts (
    artifact_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    layer_id TEXT NOT NULL REFERENCES layers(layer_id) ON DELETE CASCADE,
    logical_path TEXT NOT NULL CHECK (length(trim(logical_path)) > 0),
    artifact_kind TEXT NOT NULL DEFAULT 'file'
        CHECK (artifact_kind IN ('file', 'image', 'texture', 'binary', 'note', 'proof')),
    state TEXT NOT NULL DEFAULT 'active'
        CHECK (state IN ('active', 'redacted', 'deleted')),
    created_by_account_id TEXT NOT NULL REFERENCES accounts(account_id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (layer_id, logical_path)
);

CREATE TABLE IF NOT EXISTS artifact_content_objects (
    content_object_id TEXT PRIMARY KEY,
    artifact_id TEXT NOT NULL REFERENCES artifacts(artifact_id) ON DELETE CASCADE,
    object_key TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 0),
    media_type TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (artifact_id, content_hash)
);

CREATE TABLE IF NOT EXISTS layer_access_policies (
    policy_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    layer_id TEXT NOT NULL REFERENCES layers(layer_id) ON DELETE CASCADE,
    schema_name TEXT NOT NULL DEFAULT 'layrs.layer_access.v1',
    registry_path TEXT NOT NULL,
    policy_epoch BIGINT NOT NULL CHECK (policy_epoch > 0),
    generated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    signature_key_id TEXT,
    signature_value TEXT,
    updated_by_account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (layer_id),
    UNIQUE (layer_id, policy_epoch)
);

CREATE TABLE IF NOT EXISTS layer_access_policy_rules (
    rule_id TEXT PRIMARY KEY,
    policy_id TEXT NOT NULL REFERENCES layer_access_policies(policy_id) ON DELETE CASCADE,
    path TEXT NOT NULL CHECK (length(trim(path)) > 0),
    artifact_id TEXT REFERENCES artifacts(artifact_id) ON DELETE SET NULL,
    mode TEXT NOT NULL CHECK (mode IN ('restricted', 'reserved_redacted')),
    visibility TEXT NOT NULL DEFAULT 'stub' CHECK (visibility IN ('full', 'stub')),
    read_account_ids TEXT[] NOT NULL DEFAULT '{}',
    read_team_ids TEXT[] NOT NULL DEFAULT '{}',
    write_account_ids TEXT[] NOT NULL DEFAULT '{}',
    write_team_ids TEXT[] NOT NULL DEFAULT '{}',
    admin_account_ids TEXT[] NOT NULL DEFAULT '{}',
    admin_team_ids TEXT[] NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (policy_id, path)
);

CREATE TABLE IF NOT EXISTS chunks (
    chunk_id TEXT PRIMARY KEY,
    content_hash TEXT NOT NULL UNIQUE,
    object_key TEXT NOT NULL UNIQUE,
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS sync_manifests (
    manifest_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    space_id TEXT REFERENCES spaces(space_id) ON DELETE CASCADE,
    source_client_id TEXT NOT NULL,
    base_cursor TEXT,
    server_cursor TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS sync_idempotency (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    idempotency_key TEXT NOT NULL,
    manifest_id TEXT REFERENCES sync_manifests(manifest_id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, idempotency_key)
);

CREATE TABLE IF NOT EXISTS weaves (
    weave_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    source_layer_id TEXT NOT NULL REFERENCES layers(layer_id) ON DELETE CASCADE,
    target_layer_id TEXT NOT NULL REFERENCES layers(layer_id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'applied', 'policy_conflict', 'rejected')),
    created_by_account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS proofs (
    proof_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    layer_id TEXT REFERENCES layers(layer_id) ON DELETE CASCADE,
    artifact_id TEXT REFERENCES artifacts(artifact_id) ON DELETE CASCADE,
    proof_kind TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'accepted', 'failed', 'stale')),
    summary TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS policies (
    policy_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    policy_kind TEXT NOT NULL,
    body_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS timeline_events (
    event_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    space_id TEXT REFERENCES spaces(space_id) ON DELETE CASCADE,
    layer_id TEXT REFERENCES layers(layer_id) ON DELETE CASCADE,
    event_kind TEXT NOT NULL,
    title TEXT NOT NULL,
    body_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS invitations (
    invitation_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    email TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member', 'viewer')),
    invited_by_account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    accepted_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS audit_events (
    audit_event_id TEXT PRIMARY KEY,
    workspace_id TEXT REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    actor_account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL,
    action TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_id TEXT,
    metadata_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS workspace_memberships_account_idx
    ON workspace_memberships (account_id, state);

CREATE INDEX IF NOT EXISTS team_memberships_account_idx
    ON team_memberships (account_id);

CREATE INDEX IF NOT EXISTS space_memberships_account_idx
    ON space_memberships (account_id)
    WHERE account_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS layer_memberships_account_idx
    ON layer_memberships (account_id)
    WHERE account_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS artifacts_layer_path_idx
    ON artifacts (layer_id, logical_path);

CREATE INDEX IF NOT EXISTS layer_access_policy_rules_artifact_idx
    ON layer_access_policy_rules (artifact_id)
    WHERE artifact_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS audit_events_workspace_created_idx
    ON audit_events (workspace_id, created_at DESC);
