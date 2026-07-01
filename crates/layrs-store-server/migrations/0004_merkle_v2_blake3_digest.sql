-- Align V2 object metadata with the canonical Layrs digest format:
-- blake3:<64 hex>. The original additive V2 migration used a sha256 column
-- name as a placeholder; keep this rename idempotent for existing databases.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'object_chunks' AND column_name = 'sha256'
    ) AND NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'object_chunks' AND column_name = 'digest'
    ) THEN
        ALTER TABLE object_chunks RENAME COLUMN sha256 TO digest;
    END IF;
END $$;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'file_objects' AND column_name = 'sha256'
    ) AND NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'file_objects' AND column_name = 'digest'
    ) THEN
        ALTER TABLE file_objects RENAME COLUMN sha256 TO digest;
    END IF;
END $$;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'tree_objects' AND column_name = 'sha256'
    ) AND NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'tree_objects' AND column_name = 'digest'
    ) THEN
        ALTER TABLE tree_objects RENAME COLUMN sha256 TO digest;
    END IF;
END $$;

ALTER TABLE object_chunks
    DROP CONSTRAINT IF EXISTS object_chunks_workspace_id_space_id_sha256_key;

ALTER TABLE file_objects
    DROP CONSTRAINT IF EXISTS file_objects_workspace_id_space_id_sha256_key;

ALTER TABLE tree_objects
    DROP CONSTRAINT IF EXISTS tree_objects_workspace_id_space_id_sha256_key;

CREATE UNIQUE INDEX IF NOT EXISTS object_chunks_workspace_space_digest_idx
    ON object_chunks (workspace_id, space_id, digest);

CREATE UNIQUE INDEX IF NOT EXISTS file_objects_workspace_space_digest_idx
    ON file_objects (workspace_id, space_id, digest);

CREATE UNIQUE INDEX IF NOT EXISTS tree_objects_workspace_space_digest_idx
    ON tree_objects (workspace_id, space_id, digest);
