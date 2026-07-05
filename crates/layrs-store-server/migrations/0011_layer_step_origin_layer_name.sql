-- Keep a human-readable source Layer label on each Step.
-- Layer ids are still used for provenance/deduplication, but names must
-- survive Layer rename/delete so timelines can show where a Step came from.

ALTER TABLE layer_steps
    ADD COLUMN IF NOT EXISTS origin_layer_name TEXT NOT NULL DEFAULT '';

UPDATE layer_steps AS step
SET origin_layer_name = COALESCE(NULLIF(layer_row.name, ''), step.layer_id)
FROM layers AS layer_row
WHERE step.origin_layer_name = ''
  AND layer_row.workspace_id = step.workspace_id
  AND layer_row.space_id = step.space_id
  AND layer_row.layer_id = COALESCE(step.origin_layer_id, step.layer_id);

UPDATE layer_steps
SET origin_layer_name = layer_id
WHERE origin_layer_name = '';
