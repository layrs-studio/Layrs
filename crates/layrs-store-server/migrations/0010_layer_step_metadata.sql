-- Preserve client Step timeline/provenance metadata across publish/receive.
-- Steps are anonymous snapshots, but their order and origin are product data.

ALTER TABLE layer_steps
    ADD COLUMN IF NOT EXISTS timeline_position BIGINT;

ALTER TABLE layer_steps
    ADD COLUMN IF NOT EXISTS origin_layer_id TEXT REFERENCES layers(layer_id) ON DELETE SET NULL;

ALTER TABLE layer_steps
    ADD COLUMN IF NOT EXISTS origin_step_id TEXT;

ALTER TABLE layer_steps
    ADD COLUMN IF NOT EXISTS step_kind TEXT NOT NULL DEFAULT 'native'
        CHECK (step_kind IN ('native', 'inherited', 'woven'));

CREATE INDEX IF NOT EXISTS layer_steps_layer_timeline_idx
    ON layer_steps (workspace_id, space_id, layer_id, timeline_position ASC, captured_at ASC, created_at ASC);
