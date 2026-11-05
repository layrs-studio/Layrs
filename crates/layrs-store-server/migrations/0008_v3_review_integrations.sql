-- Immutable evidence binding a reviewed version to an actual applied native
-- transaction. Reviews.lifecycle is only the associated mutable projection.
CREATE TABLE layrs_collaboration.review_integrations (
    review_id TEXT PRIMARY KEY,
    version INTEGER NOT NULL CHECK(version>0),
    space_id TEXT NOT NULL REFERENCES spaces(space_id),
    transaction_id TEXT NOT NULL,
    evidence_object_id TEXT UNIQUE NOT NULL REFERENCES canonical_objects(object_id),
    canonical_document_jcs TEXT NOT NULL CHECK(octet_length(canonical_document_jcs) BETWEEN 2 AND 16384),
    FOREIGN KEY(review_id,version) REFERENCES layrs_collaboration.review_versions(review_id,version),
    FOREIGN KEY(space_id,transaction_id) REFERENCES sync_transactions(space_id,transaction_id)
);
DO $review_integration_grants$
DECLARE writer TEXT;
BEGIN
    SELECT writer_role_name INTO STRICT writer FROM schema_metadata WHERE singleton_id=1;
    EXECUTE format('GRANT SELECT,INSERT ON layrs_collaboration.review_integrations TO %I',writer);
END
$review_integration_grants$;
