-- Text corrections append Evidence; the original review action remains intact.
ALTER TABLE layrs_collaboration.review_actions
    ADD CONSTRAINT review_action_scope UNIQUE(action_id,review_id,version);
CREATE TABLE layrs_collaboration.comment_corrections (
    correction_id TEXT PRIMARY KEY CHECK(correction_id LIKE 'review-comment-correction:%'),
    action_id TEXT NOT NULL,
    review_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK(version>0),
    revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 100),
    actor_principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    request_digest TEXT NOT NULL,
    evidence_object_id TEXT UNIQUE NOT NULL REFERENCES canonical_objects(object_id),
    canonical_document_jcs TEXT NOT NULL CHECK(octet_length(canonical_document_jcs) BETWEEN 2 AND 98304),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(action_id,revision),
    FOREIGN KEY(action_id,review_id,version) REFERENCES layrs_collaboration.review_actions(action_id,review_id,version)
);
CREATE INDEX comment_corrections_review ON layrs_collaboration.comment_corrections(review_id,version,action_id,revision);
DO $comment_correction_grants$
DECLARE writer TEXT;
BEGIN
    SELECT writer_role_name INTO STRICT writer FROM schema_metadata WHERE singleton_id=1;
    EXECUTE format('GRANT SELECT,INSERT ON layrs_collaboration.comment_corrections TO %I',writer);
END
$comment_correction_grants$;
