-- Durable Weave requests/sessions. A Weave may pause in conflicted state
-- without advancing the target Layer head.

CREATE TABLE IF NOT EXISTS weave_requests (
    weave_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    source_layer_id TEXT NOT NULL REFERENCES layers(layer_id) ON DELETE CASCADE,
    target_layer_id TEXT NOT NULL REFERENCES layers(layer_id) ON DELETE CASCADE,
    title TEXT NOT NULL CHECK (length(trim(title)) >= 2),
    body TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'open'
        CHECK (status IN ('open', 'preview', 'applying', 'conflicted', 'resolved', 'applied', 'aborted', 'closed')),
    pre_weave_target_tree_id TEXT REFERENCES tree_objects(tree_id) ON DELETE SET NULL,
    pre_weave_target_step_id TEXT REFERENCES layer_steps(step_id) ON DELETE SET NULL,
    planned_steps TEXT[] NOT NULL DEFAULT '{}',
    applied_steps TEXT[] NOT NULL DEFAULT '{}',
    requested_by_account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (source_layer_id <> target_layer_id)
);

CREATE INDEX IF NOT EXISTS weave_requests_space_idx
    ON weave_requests (workspace_id, space_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS weave_sessions (
    weave_id TEXT PRIMARY KEY REFERENCES weave_requests(weave_id) ON DELETE CASCADE,
    status TEXT NOT NULL
        CHECK (status IN ('preview', 'applying', 'conflicted', 'resolved', 'applied', 'aborted')),
    session_payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS weave_conflicts (
    conflict_id TEXT PRIMARY KEY,
    weave_id TEXT NOT NULL REFERENCES weave_requests(weave_id) ON DELETE CASCADE,
    logical_path TEXT NOT NULL CHECK (length(trim(logical_path)) > 0),
    lens_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'open'
        CHECK (status IN ('open', 'resolved')),
    message TEXT NOT NULL DEFAULT '',
    base_file_object_id TEXT REFERENCES file_objects(file_object_id) ON DELETE SET NULL,
    ours_file_object_id TEXT REFERENCES file_objects(file_object_id) ON DELETE SET NULL,
    theirs_file_object_id TEXT REFERENCES file_objects(file_object_id) ON DELETE SET NULL,
    resolved_file_object_id TEXT REFERENCES file_objects(file_object_id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (weave_id, logical_path)
);

CREATE TABLE IF NOT EXISTS weave_resolutions (
    resolution_id TEXT PRIMARY KEY,
    conflict_id TEXT NOT NULL REFERENCES weave_conflicts(conflict_id) ON DELETE CASCADE,
    resolution_kind TEXT NOT NULL CHECK (resolution_kind IN ('ours', 'theirs', 'base', 'file', 'manual')),
    resolved_file_object_id TEXT REFERENCES file_objects(file_object_id) ON DELETE SET NULL,
    resolved_by_account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
