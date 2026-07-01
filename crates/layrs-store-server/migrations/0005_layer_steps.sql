-- Layer steps are synchronizable anonymous snapshots for a Layer. They are
-- distinct from layer_states: states are the published head, steps preserve
-- the reviewable snapshot history that clients can download and inspect.

CREATE TABLE IF NOT EXISTS layer_steps (
    step_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    layer_id TEXT NOT NULL REFERENCES layers(layer_id) ON DELETE CASCADE,
    parent_step_id TEXT REFERENCES layer_steps(step_id) ON DELETE SET NULL,
    base_layer_id TEXT REFERENCES layers(layer_id) ON DELETE SET NULL,
    base_tree_id TEXT REFERENCES tree_objects(tree_id) ON DELETE SET NULL,
    root_tree_id TEXT REFERENCES tree_objects(tree_id) ON DELETE SET NULL,
    changed_paths TEXT[] NOT NULL DEFAULT '{}',
    source_client_id TEXT,
    sync_batch_id TEXT REFERENCES sync_batches(sync_batch_id) ON DELETE SET NULL,
    created_by_account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL,
    captured_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS layer_steps_layer_captured_idx
    ON layer_steps (workspace_id, space_id, layer_id, captured_at ASC, created_at ASC);

CREATE INDEX IF NOT EXISTS layer_steps_root_tree_idx
    ON layer_steps (root_tree_id)
    WHERE root_tree_id IS NOT NULL;

ALTER TABLE sync_batch_changes
    DROP CONSTRAINT IF EXISTS sync_batch_changes_change_kind_check;

ALTER TABLE sync_batch_changes
    ADD CONSTRAINT sync_batch_changes_change_kind_check
    CHECK (change_kind IN ('upsert_file', 'delete_path', 'advance_head', 'record_step'));
