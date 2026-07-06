-- Durable server-side replay plan for Weave requests.
-- weave_requests.planned_steps remains the wire-friendly summary; this table
-- stores the ordered replay data the server needs to apply a Weave atomically.

CREATE TABLE IF NOT EXISTS weave_step_replays (
    replay_id TEXT PRIMARY KEY,
    weave_id TEXT NOT NULL REFERENCES weave_requests(weave_id) ON DELETE CASCADE,
    order_index INTEGER NOT NULL CHECK (order_index >= 0),
    source_step_id TEXT NOT NULL REFERENCES layer_steps(step_id) ON DELETE CASCADE,
    target_step_id TEXT REFERENCES layer_steps(step_id) ON DELETE SET NULL,
    origin_layer_id TEXT REFERENCES layers(layer_id) ON DELETE SET NULL,
    origin_layer_name TEXT NOT NULL DEFAULT '',
    origin_step_id TEXT NOT NULL DEFAULT '',
    target_before_tree_id TEXT REFERENCES tree_objects(tree_id) ON DELETE SET NULL,
    incoming_tree_id TEXT REFERENCES tree_objects(tree_id) ON DELETE SET NULL,
    target_after_tree_id TEXT REFERENCES tree_objects(tree_id) ON DELETE SET NULL,
    source_base_tree_id TEXT REFERENCES tree_objects(tree_id) ON DELETE SET NULL,
    source_root_tree_id TEXT REFERENCES tree_objects(tree_id) ON DELETE SET NULL,
    changed_paths TEXT[] NOT NULL DEFAULT '{}',
    path_replays JSONB NOT NULL DEFAULT '[]'::jsonb,
    status TEXT NOT NULL DEFAULT 'planned'
        CHECK (status IN ('planned', 'conflicted', 'resolved', 'applied', 'skipped')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (weave_id, order_index),
    UNIQUE (weave_id, source_step_id)
);

CREATE INDEX IF NOT EXISTS weave_step_replays_weave_order_idx
    ON weave_step_replays (weave_id, order_index ASC);

CREATE INDEX IF NOT EXISTS weave_step_replays_source_step_idx
    ON weave_step_replays (source_step_id);
