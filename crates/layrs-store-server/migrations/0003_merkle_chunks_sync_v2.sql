-- Layrs V2 server schema: Merkle chunks, file/tree objects, Layer heads,
-- and sync batches. This migration is additive and idempotent so V1 data keeps
-- working while Desktop clients move off inline artifact payloads.

CREATE TABLE IF NOT EXISTS object_chunks (
    chunk_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    sha256 TEXT NOT NULL,
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 0),
    object_key TEXT NOT NULL UNIQUE,
    media_type TEXT,
    compression TEXT NOT NULL DEFAULT 'identity',
    state TEXT NOT NULL DEFAULT 'available'
        CHECK (state IN ('reserved', 'available', 'deleted')),
    content_bytes BYTEA,
    created_by_account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, space_id, sha256)
);

CREATE TABLE IF NOT EXISTS file_objects (
    file_object_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    sha256 TEXT NOT NULL,
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 0),
    media_type TEXT,
    chunk_count INTEGER NOT NULL CHECK (chunk_count >= 0),
    created_by_account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, space_id, sha256)
);

CREATE TABLE IF NOT EXISTS file_object_chunks (
    file_object_id TEXT NOT NULL REFERENCES file_objects(file_object_id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    chunk_id TEXT NOT NULL REFERENCES object_chunks(chunk_id) ON DELETE RESTRICT,
    byte_offset BIGINT NOT NULL CHECK (byte_offset >= 0),
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (file_object_id, chunk_index)
);

CREATE TABLE IF NOT EXISTS tree_objects (
    tree_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    sha256 TEXT NOT NULL,
    entry_count INTEGER NOT NULL CHECK (entry_count >= 0),
    created_by_account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, space_id, sha256)
);

CREATE TABLE IF NOT EXISTS tree_entries (
    tree_id TEXT NOT NULL REFERENCES tree_objects(tree_id) ON DELETE CASCADE,
    logical_path TEXT NOT NULL CHECK (length(trim(logical_path)) > 0),
    entry_kind TEXT NOT NULL CHECK (entry_kind IN ('file', 'tree', 'tombstone')),
    file_object_id TEXT REFERENCES file_objects(file_object_id) ON DELETE RESTRICT,
    child_tree_id TEXT REFERENCES tree_objects(tree_id) ON DELETE RESTRICT,
    artifact_id TEXT REFERENCES artifacts(artifact_id) ON DELETE SET NULL,
    mode TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tree_id, logical_path),
    CHECK (
        (entry_kind = 'file' AND file_object_id IS NOT NULL AND child_tree_id IS NULL)
        OR (entry_kind = 'tree' AND file_object_id IS NULL AND child_tree_id IS NOT NULL)
        OR (entry_kind = 'tombstone' AND file_object_id IS NULL AND child_tree_id IS NULL)
    )
);

ALTER TABLE artifacts
    ADD COLUMN IF NOT EXISTS current_file_object_id TEXT REFERENCES file_objects(file_object_id) ON DELETE SET NULL;

ALTER TABLE artifacts
    ADD COLUMN IF NOT EXISTS current_tree_id TEXT REFERENCES tree_objects(tree_id) ON DELETE SET NULL;

CREATE TABLE IF NOT EXISTS layer_states (
    layer_state_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    layer_id TEXT NOT NULL REFERENCES layers(layer_id) ON DELETE CASCADE,
    root_tree_id TEXT REFERENCES tree_objects(tree_id) ON DELETE RESTRICT,
    policy_epoch BIGINT NOT NULL CHECK (policy_epoch > 0),
    parent_layer_state_id TEXT REFERENCES layer_states(layer_state_id) ON DELETE SET NULL,
    created_by_account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS layer_heads (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    layer_id TEXT NOT NULL REFERENCES layers(layer_id) ON DELETE CASCADE,
    layer_state_id TEXT REFERENCES layer_states(layer_state_id) ON DELETE SET NULL,
    root_tree_id TEXT REFERENCES tree_objects(tree_id) ON DELETE RESTRICT,
    policy_epoch BIGINT NOT NULL CHECK (policy_epoch > 0),
    server_cursor TEXT,
    updated_by_account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, space_id, layer_id)
);

CREATE TABLE IF NOT EXISTS sync_batches (
    sync_batch_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    layer_id TEXT REFERENCES layers(layer_id) ON DELETE CASCADE,
    idempotency_key TEXT NOT NULL,
    source_client_id TEXT,
    base_cursor TEXT,
    server_cursor TEXT,
    policy_epoch BIGINT,
    status TEXT NOT NULL DEFAULT 'applied'
        CHECK (status IN ('reserved', 'applied', 'rejected')),
    request_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    response_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_by_account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, space_id, idempotency_key)
);

CREATE TABLE IF NOT EXISTS sync_batch_changes (
    sync_batch_change_id TEXT PRIMARY KEY,
    sync_batch_id TEXT NOT NULL REFERENCES sync_batches(sync_batch_id) ON DELETE CASCADE,
    change_index INTEGER NOT NULL CHECK (change_index >= 0),
    change_kind TEXT NOT NULL CHECK (change_kind IN ('upsert_file', 'delete_path', 'advance_head')),
    artifact_id TEXT REFERENCES artifacts(artifact_id) ON DELETE SET NULL,
    logical_path TEXT,
    file_object_id TEXT REFERENCES file_objects(file_object_id) ON DELETE SET NULL,
    tree_id TEXT REFERENCES tree_objects(tree_id) ON DELETE SET NULL,
    body_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (sync_batch_id, change_index)
);

CREATE INDEX IF NOT EXISTS object_chunks_space_state_idx
    ON object_chunks (workspace_id, space_id, state);

CREATE INDEX IF NOT EXISTS file_object_chunks_chunk_idx
    ON file_object_chunks (chunk_id);

CREATE INDEX IF NOT EXISTS tree_entries_file_object_idx
    ON tree_entries (file_object_id)
    WHERE file_object_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS layer_states_layer_created_idx
    ON layer_states (workspace_id, space_id, layer_id, created_at DESC);

CREATE INDEX IF NOT EXISTS sync_batches_layer_created_idx
    ON sync_batches (workspace_id, space_id, layer_id, created_at DESC);
