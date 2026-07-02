-- Chunks are content-addressed globally by chunk_id, while availability is
-- scoped per Space. This lets multiple Spaces reuse identical chunk bytes
-- without making object_chunks.workspace_id/space_id the ownership boundary.

ALTER TABLE object_chunks
    DROP CONSTRAINT IF EXISTS object_chunks_workspace_id_fkey;

ALTER TABLE object_chunks
    DROP CONSTRAINT IF EXISTS object_chunks_space_id_fkey;

CREATE TABLE IF NOT EXISTS space_object_chunks (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    chunk_id TEXT NOT NULL REFERENCES object_chunks(chunk_id) ON DELETE CASCADE,
    created_by_account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, space_id, chunk_id)
);

INSERT INTO space_object_chunks (workspace_id, space_id, chunk_id, created_by_account_id, created_at)
SELECT workspace_id, space_id, chunk_id, created_by_account_id, created_at
FROM object_chunks
ON CONFLICT (workspace_id, space_id, chunk_id) DO NOTHING;

CREATE INDEX IF NOT EXISTS space_object_chunks_chunk_idx
    ON space_object_chunks (chunk_id);
