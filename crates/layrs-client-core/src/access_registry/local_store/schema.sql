CREATE TABLE IF NOT EXISTS client_store_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    layout_version INTEGER NOT NULL CHECK (layout_version > 0),
    generation INTEGER NOT NULL CHECK (generation >= 0),
    legacy_backup_path TEXT,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms)
) STRICT;

CREATE TABLE IF NOT EXISTS client_documents (
    path TEXT PRIMARY KEY CHECK (length(path) > 0 AND instr(path, char(0)) = 0),
    document_kind TEXT NOT NULL CHECK (length(document_kind) > 0),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    generation INTEGER NOT NULL CHECK (generation >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
) STRICT;

CREATE INDEX IF NOT EXISTS client_documents_kind_idx
ON client_documents(document_kind, path);

CREATE TABLE IF NOT EXISTS client_space_details (
    space_id TEXT PRIMARY KEY,
    local_space_id TEXT NOT NULL UNIQUE,
    server_space_id TEXT,
    workspace_id TEXT NOT NULL,
    local_state TEXT NOT NULL,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    FOREIGN KEY (space_id) REFERENCES local_spaces(space_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE IF NOT EXISTS client_layer_details (
    layer_id TEXT PRIMARY KEY,
    parent_layer_id TEXT,
    lineage_status TEXT NOT NULL,
    access_kind TEXT NOT NULL,
    can_open INTEGER NOT NULL CHECK (can_open IN (0, 1)),
    policy_epoch INTEGER NOT NULL DEFAULT 0 CHECK (policy_epoch >= 0),
    access_payload_json TEXT CHECK (access_payload_json IS NULL OR json_valid(access_payload_json)),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    FOREIGN KEY (layer_id) REFERENCES layers(layer_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE IF NOT EXISTS client_step_details (
    step_id TEXT PRIMARY KEY,
    base_layer_id TEXT,
    base_tree_id TEXT,
    timeline_position INTEGER,
    origin_layer_id TEXT,
    origin_layer_name TEXT,
    origin_step_id TEXT,
    step_kind TEXT,
    changed_paths_json TEXT NOT NULL CHECK (json_valid(changed_paths_json)),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    FOREIGN KEY (step_id) REFERENCES steps(step_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE IF NOT EXISTS client_weave_details (
    weave_id TEXT PRIMARY KEY,
    source_layer_id TEXT NOT NULL,
    pre_target_tree_id TEXT,
    pre_target_step_id TEXT,
    planned_steps_json TEXT NOT NULL CHECK (json_valid(planned_steps_json)),
    applied_steps_json TEXT NOT NULL CHECK (json_valid(applied_steps_json)),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    FOREIGN KEY (weave_id) REFERENCES weave_sessions(weave_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE IF NOT EXISTS client_weave_conflict_details (
    conflict_id TEXT PRIMARY KEY,
    lens_id TEXT NOT NULL,
    message TEXT NOT NULL,
    methods_json TEXT NOT NULL CHECK (json_valid(methods_json)),
    segments_json TEXT NOT NULL CHECK (json_valid(segments_json)),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    FOREIGN KEY (conflict_id) REFERENCES weave_conflicts(conflict_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE IF NOT EXISTS client_projected_objects (
    object_id TEXT PRIMARY KEY,
    FOREIGN KEY (object_id) REFERENCES object_index(object_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE IF NOT EXISTS client_network_operations (
    operation_id TEXT PRIMARY KEY,
    space_id TEXT NOT NULL,
    operation_kind TEXT NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    expected_generation INTEGER NOT NULL CHECK (expected_generation >= 0),
    state TEXT NOT NULL CHECK (
        state IN ('external_pending', 'applying', 'finalized', 'stale', 'failed')
    ),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms)
) STRICT;

CREATE INDEX IF NOT EXISTS client_network_pending_idx
ON client_network_operations(space_id, state, created_at_ms);

CREATE TABLE IF NOT EXISTS client_materialization_operations (
    operation_id TEXT PRIMARY KEY,
    state TEXT NOT NULL CHECK (state IN ('applying', 'completed', 'rolled_back')),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms)
) STRICT;

CREATE INDEX IF NOT EXISTS client_materialization_state_idx
ON client_materialization_operations(state, created_at_ms);
