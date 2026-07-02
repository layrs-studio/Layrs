-- Store chunks as encoded bytes while keeping object IDs based on raw bytes.
-- Existing chunks remain identity-compressed and are backfilled from content_bytes.

ALTER TABLE object_chunks
    ADD COLUMN IF NOT EXISTS stored_size_bytes BIGINT CHECK (stored_size_bytes >= 0);

UPDATE object_chunks
SET stored_size_bytes = COALESCE(stored_size_bytes, octet_length(content_bytes), size_bytes),
    compression = COALESCE(NULLIF(compression, ''), 'identity')
WHERE stored_size_bytes IS NULL
   OR compression IS NULL
   OR compression = '';
