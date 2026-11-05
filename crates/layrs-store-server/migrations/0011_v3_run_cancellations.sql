-- Cancellation is immutable evidence, never a synthetic failed verdict.
ALTER TABLE layrs_collaboration.runs ADD CONSTRAINT runs_space_identity UNIQUE(space_id,run_id);
CREATE TABLE layrs_collaboration.run_cancellations (
    run_id TEXT PRIMARY KEY,
    space_id TEXT NOT NULL,
    cancelled_by TEXT NOT NULL REFERENCES principals(principal_id),
    request_digest TEXT NOT NULL,
    evidence_object_id TEXT UNIQUE NOT NULL REFERENCES canonical_objects(object_id),
    canonical_document_jcs TEXT NOT NULL CHECK(octet_length(canonical_document_jcs) BETWEEN 2 AND 16384),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY(space_id,run_id) REFERENCES layrs_collaboration.runs(space_id,run_id)
);
DO $run_cancellation_grants$
DECLARE writer TEXT;
BEGIN
    SELECT writer_role_name INTO STRICT writer FROM schema_metadata WHERE singleton_id=1;
    EXECUTE format('GRANT SELECT,INSERT ON layrs_collaboration.run_cancellations TO %I',writer);
END
$run_cancellation_grants$;
