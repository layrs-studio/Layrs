-- Metadata-only path patch sessions and immutable server-owned candidate provenance.
CREATE TABLE scoped_patch_sessions (
    session_id TEXT PRIMARY KEY CHECK (session_id ~ '^scoped-patch:[0-9a-f]{64}$'),
    space_id TEXT NOT NULL REFERENCES spaces(space_id),
    principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    client_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_digest TEXT NOT NULL CHECK (request_digest ~ '^blake3:[0-9a-f]{64}$'),
    capture_digest TEXT NOT NULL CHECK (capture_digest ~ '^blake3:[0-9a-f]{64}$'),
    intent TEXT NOT NULL CHECK (intent IN ('advance','candidate')),
    request_jcs TEXT NOT NULL CHECK (octet_length(request_jcs) <= 4194304),
    base_step_id TEXT NOT NULL,
    base_root_tree_id TEXT NOT NULL,
    expected_view_id TEXT NOT NULL,
    expected_policy_set_id TEXT NOT NULL,
    expected_authorization_epoch BIGINT NOT NULL CHECK (expected_authorization_epoch > 0),
    finalized_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (space_id,client_id,idempotency_key),
    FOREIGN KEY (space_id,base_step_id) REFERENCES space_steps(space_id,step_id)
);

CREATE TABLE scoped_patch_chunks (
    session_id TEXT NOT NULL REFERENCES scoped_patch_sessions(session_id),
    repo_path TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    object_id TEXT NOT NULL CHECK (object_id ~ '^blake3:[0-9a-f]{64}$'),
    byte_length BIGINT NOT NULL CHECK (byte_length BETWEEN 1 AND 1048576),
    supplied_at TIMESTAMPTZ,
    PRIMARY KEY (session_id,repo_path,ordinal)
);
CREATE INDEX scoped_patch_chunks_object ON scoped_patch_chunks(object_id);

-- Rows survive expiry/abandonment until a verified CAS GC accounts for orphans.
CREATE TABLE scoped_patch_reserved_objects (
    space_id TEXT NOT NULL REFERENCES spaces(space_id),
    object_id TEXT NOT NULL CHECK (object_id ~ '^blake3:[0-9a-f]{64}$'),
    object_kind TEXT NOT NULL,
    canonical_byte_length BIGINT NOT NULL CHECK (canonical_byte_length >= 0),
    encoded_byte_length BIGINT NOT NULL CHECK (encoded_byte_length >= 0),
    compression TEXT NOT NULL,
    PRIMARY KEY (space_id,object_id)
);

CREATE TABLE scoped_patch_candidates (
    space_id TEXT NOT NULL REFERENCES spaces(space_id),
    candidate_id TEXT NOT NULL CHECK (candidate_id ~ '^candidate:[0-9a-f]{64}$'),
    candidate_step_id TEXT NOT NULL,
    candidate_root_tree_id TEXT NOT NULL,
    source_request_digest TEXT NOT NULL CHECK (source_request_digest ~ '^blake3:[0-9a-f]{64}$'),
    capture_digest TEXT NOT NULL CHECK (capture_digest ~ '^blake3:[0-9a-f]{64}$'),
    author_principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    layer_id TEXT NOT NULL,
    base_step_id TEXT NOT NULL,
    base_root_tree_id TEXT NOT NULL,
    expected_view_id TEXT NOT NULL,
    expected_policy_set_id TEXT NOT NULL,
    expected_authorization_epoch BIGINT NOT NULL CHECK (expected_authorization_epoch > 0),
    changed_paths_jcs TEXT NOT NULL,
    transaction_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    PRIMARY KEY (space_id,candidate_id),
    UNIQUE (space_id,source_request_digest),
    FOREIGN KEY (space_id,candidate_step_id) REFERENCES space_steps(space_id,step_id),
    FOREIGN KEY (space_id,base_step_id) REFERENCES space_steps(space_id,step_id),
    FOREIGN KEY (space_id,transaction_id) REFERENCES sync_transactions(space_id,transaction_id)
);
CREATE INDEX scoped_patch_candidate_step ON scoped_patch_candidates(space_id,candidate_step_id);

DO $scoped_patch_grants$
DECLARE writer TEXT;
BEGIN
    SELECT writer_role_name INTO STRICT writer FROM schema_metadata WHERE singleton_id=1;
    EXECUTE format('GRANT SELECT,INSERT ON scoped_patch_sessions TO %I',writer);
    EXECUTE format('GRANT SELECT,INSERT,UPDATE(supplied_at) ON scoped_patch_chunks TO %I',writer);
    EXECUTE format('GRANT SELECT,INSERT ON scoped_patch_reserved_objects,scoped_patch_candidates TO %I',writer);
END
$scoped_patch_grants$;
