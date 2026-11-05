-- Scoped reviews reference immutable candidate provenance, never StepId alone.
CREATE TABLE layrs_collaboration.scoped_reviews (
    review_id TEXT PRIMARY KEY CHECK (review_id ~ '^scoped-review:[0-9a-f]{64}$'),
    space_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    author_principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    request_digest TEXT NOT NULL,
    title TEXT NOT NULL CHECK (octet_length(title) BETWEEN 1 AND 1024),
    author_content_restricted BOOLEAN NOT NULL,
    author_visibility_fingerprint TEXT,
    lifecycle TEXT NOT NULL DEFAULT 'open' CHECK (lifecycle IN ('open','closed','integrated')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (space_id,candidate_id),
    FOREIGN KEY (space_id,candidate_id) REFERENCES scoped_patch_candidates(space_id,candidate_id)
);
CREATE INDEX scoped_reviews_space_created ON layrs_collaboration.scoped_reviews(space_id,created_at DESC,review_id DESC);

CREATE TABLE layrs_collaboration.scoped_review_actions (
    action_id TEXT PRIMARY KEY CHECK (action_id ~ '^scoped-review-action:[0-9a-f]{64}$'),
    sequence BIGSERIAL UNIQUE NOT NULL,
    review_id TEXT NOT NULL REFERENCES layrs_collaboration.scoped_reviews(review_id),
    actor_principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    action_kind TEXT NOT NULL CHECK (action_kind IN ('comment','approve','request_changes','close','reopen')),
    body TEXT NOT NULL CHECK (octet_length(body) <= 65536),
    anchor_path TEXT,
    reply_to TEXT REFERENCES layrs_collaboration.scoped_review_actions(action_id),
    request_digest TEXT NOT NULL,
    writer_content_restricted BOOLEAN NOT NULL,
    writer_visibility_fingerprint TEXT,
    writer_read_paths_jcs TEXT NOT NULL CHECK (octet_length(writer_read_paths_jcs) <= 4194304),
    writer_policy_set_id TEXT NOT NULL REFERENCES policy_sets(policy_set_id),
    writer_authorization_epoch BIGINT NOT NULL CHECK (writer_authorization_epoch > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp()
);
CREATE INDEX scoped_review_actions_review_sequence ON layrs_collaboration.scoped_review_actions(review_id,sequence);

-- Only the trusted integration path may write an exact candidate receipt binding.
CREATE TABLE layrs_collaboration.scoped_review_integrations (
    review_id TEXT PRIMARY KEY REFERENCES layrs_collaboration.scoped_reviews(review_id),
    space_id TEXT NOT NULL REFERENCES spaces(space_id),
    candidate_id TEXT NOT NULL,
    transaction_id TEXT NOT NULL,
    integrated_by TEXT NOT NULL REFERENCES principals(principal_id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    FOREIGN KEY (space_id,candidate_id) REFERENCES scoped_patch_candidates(space_id,candidate_id),
    FOREIGN KEY (space_id,transaction_id) REFERENCES sync_transactions(space_id,transaction_id)
);

DO $scoped_review_grants$
DECLARE writer TEXT;
BEGIN
    SELECT writer_role_name INTO STRICT writer FROM schema_metadata WHERE singleton_id=1;
    EXECUTE format('GRANT SELECT,INSERT,UPDATE ON layrs_collaboration.scoped_reviews TO %I',writer);
    EXECUTE format('GRANT SELECT,INSERT ON layrs_collaboration.scoped_review_actions,layrs_collaboration.scoped_review_integrations TO %I',writer);
    EXECUTE format('GRANT USAGE,SELECT ON SEQUENCE layrs_collaboration.scoped_review_actions_sequence_seq TO %I',writer);
END
$scoped_review_grants$;
