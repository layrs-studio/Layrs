-- Content-addressed file and tree objects are global immutable values.
-- A Space owns only availability references in space_file_objects and
-- space_tree_objects. Keep the original creator columns as provenance, but do
-- not cascade global objects when that creator Space is deleted.

ALTER TABLE file_objects
    DROP CONSTRAINT IF EXISTS file_objects_workspace_id_fkey;

ALTER TABLE file_objects
    DROP CONSTRAINT IF EXISTS file_objects_space_id_fkey;

ALTER TABLE tree_objects
    DROP CONSTRAINT IF EXISTS tree_objects_workspace_id_fkey;

ALTER TABLE tree_objects
    DROP CONSTRAINT IF EXISTS tree_objects_space_id_fkey;

ALTER TABLE object_chunks
    ALTER COLUMN workspace_id DROP NOT NULL,
    ALTER COLUMN space_id DROP NOT NULL;

ALTER TABLE file_objects
    ALTER COLUMN workspace_id DROP NOT NULL,
    ALTER COLUMN space_id DROP NOT NULL;

ALTER TABLE tree_objects
    ALTER COLUMN workspace_id DROP NOT NULL,
    ALTER COLUMN space_id DROP NOT NULL;

-- Older schemas already allowed the creator Space to disappear while leaving
-- provenance ids behind. Normalize those legacy orphans before restoring
-- referential integrity with SET NULL semantics.
UPDATE object_chunks value
SET workspace_id = NULL
WHERE workspace_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM workspaces owner WHERE owner.workspace_id = value.workspace_id
  );

UPDATE object_chunks value
SET space_id = NULL
WHERE space_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM spaces owner WHERE owner.space_id = value.space_id
  );

UPDATE file_objects value
SET workspace_id = NULL
WHERE workspace_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM workspaces owner WHERE owner.workspace_id = value.workspace_id
  );

UPDATE file_objects value
SET space_id = NULL
WHERE space_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM spaces owner WHERE owner.space_id = value.space_id
  );

UPDATE tree_objects value
SET workspace_id = NULL
WHERE workspace_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM workspaces owner WHERE owner.workspace_id = value.workspace_id
  );

UPDATE tree_objects value
SET space_id = NULL
WHERE space_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM spaces owner WHERE owner.space_id = value.space_id
  );

ALTER TABLE object_chunks
    ADD CONSTRAINT object_chunks_workspace_id_fkey
        FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE SET NULL,
    ADD CONSTRAINT object_chunks_space_id_fkey
        FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE SET NULL;

ALTER TABLE file_objects
    ADD CONSTRAINT file_objects_workspace_id_fkey
        FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE SET NULL,
    ADD CONSTRAINT file_objects_space_id_fkey
        FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE SET NULL;

ALTER TABLE tree_objects
    ADD CONSTRAINT tree_objects_workspace_id_fkey
        FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE SET NULL,
    ADD CONSTRAINT tree_objects_space_id_fkey
        FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE SET NULL;

-- Artifact ids are Layer-scoped references, not part of a global Merkle tree.
-- Normalize legacy rows once, then prevent artifact deletion from mutating an
-- immutable tree entry through ON DELETE SET NULL.
UPDATE tree_entries SET artifact_id = NULL WHERE artifact_id IS NOT NULL;

ALTER TABLE tree_entries
    DROP CONSTRAINT IF EXISTS tree_entries_artifact_id_fkey;
