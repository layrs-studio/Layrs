-- Layer settings shared by Studio Web/Desktop/CLI sync.
-- Clear Steps is a soft-delete: history disappears from active review surfaces,
-- but rows stay available for audit and recovery.

ALTER TABLE layers
    ADD COLUMN IF NOT EXISTS lineage_status TEXT NOT NULL DEFAULT 'linked'
        CHECK (lineage_status IN ('linked', 'unlinked'));

ALTER TABLE layer_steps
    ADD COLUMN IF NOT EXISTS cleared_at TIMESTAMPTZ;

ALTER TABLE layer_steps
    ADD COLUMN IF NOT EXISTS cleared_by_account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS layer_steps_active_layer_timeline_idx
    ON layer_steps (workspace_id, space_id, layer_id, timeline_position ASC, captured_at ASC, created_at ASC)
    WHERE cleared_at IS NULL;
