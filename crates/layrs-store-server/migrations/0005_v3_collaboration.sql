-- Business projections of immutable Evidence / native Proof and Gate Products.
CREATE SCHEMA layrs_collaboration;
REVOKE ALL ON SCHEMA layrs_collaboration FROM PUBLIC;

CREATE TABLE layrs_collaboration.reviews (
    review_id TEXT PRIMARY KEY CHECK (review_id LIKE 'review:%'),
    space_id TEXT NOT NULL REFERENCES spaces(space_id),
    layer_id TEXT NOT NULL,
    author_principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    latest_version INTEGER NOT NULL CHECK (latest_version>0),
    lifecycle TEXT NOT NULL DEFAULT 'open' CHECK (lifecycle IN ('open','closed','integrated')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY (space_id,layer_id) REFERENCES current_layer_definitions(space_id,layer_id)
);
CREATE TABLE layrs_collaboration.review_versions (
    review_id TEXT NOT NULL REFERENCES layrs_collaboration.reviews(review_id),
    version INTEGER NOT NULL CHECK (version>0),
    proposal_object_id TEXT NOT NULL REFERENCES canonical_objects(object_id),
    proposed_step_id TEXT NOT NULL REFERENCES steps(step_id),
    base_repository_view_id TEXT NOT NULL REFERENCES repository_views(repository_view_id),
    policy_set_id TEXT NOT NULL REFERENCES policy_sets(policy_set_id),
    authorization_epoch BIGINT NOT NULL CHECK (authorization_epoch>0),
    request_digest TEXT NOT NULL,
    canonical_document_jcs TEXT NOT NULL,
    PRIMARY KEY(review_id,version)
);
CREATE TABLE layrs_collaboration.review_actions (
    action_id TEXT PRIMARY KEY CHECK (action_id LIKE 'review-action:%'),
    sequence BIGSERIAL UNIQUE NOT NULL,
    review_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    actor_principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    action_kind TEXT NOT NULL CHECK (action_kind IN ('comment','approve','request_changes','close','reopen')),
    evidence_object_id TEXT NOT NULL REFERENCES canonical_objects(object_id),
    request_digest TEXT NOT NULL,
    canonical_document_jcs TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY(review_id,version) REFERENCES layrs_collaboration.review_versions(review_id,version)
);
CREATE TABLE layrs_collaboration.layer_protections (
    space_id TEXT NOT NULL REFERENCES spaces(space_id),
    layer_id TEXT NOT NULL,
    protection_object_id TEXT NOT NULL REFERENCES canonical_objects(object_id),
    gate_definition_object_id TEXT REFERENCES products(product_object_id),
    canonical_document_jcs TEXT NOT NULL,
    PRIMARY KEY(space_id,layer_id),
    FOREIGN KEY(space_id,layer_id) REFERENCES current_layer_definitions(space_id,layer_id)
);
CREATE TABLE layrs_collaboration.runs (
    run_id TEXT PRIMARY KEY CHECK (run_id LIKE 'run:%'),
    space_id TEXT NOT NULL REFERENCES spaces(space_id),
    step_id TEXT NOT NULL,
    recipe_object_id TEXT NOT NULL REFERENCES canonical_objects(object_id),
    runner_principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    policy_set_id TEXT NOT NULL REFERENCES policy_sets(policy_set_id),
    authorization_epoch BIGINT NOT NULL CHECK (authorization_epoch>0),
    start_digest TEXT NOT NULL,
    completion_digest TEXT,
    proof_object_id TEXT REFERENCES products(product_object_id),
    outcome TEXT CHECK (outcome IN ('passed','failed')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()+interval '1 hour',
    completed_at TIMESTAMPTZ,
    CHECK ((proof_object_id IS NULL) = (completion_digest IS NULL)),
    CHECK ((proof_object_id IS NULL) = (outcome IS NULL)),
    CHECK ((proof_object_id IS NULL) = (completed_at IS NULL)),
    FOREIGN KEY(space_id,step_id) REFERENCES space_steps(space_id,step_id)
);
CREATE TABLE layrs_collaboration.notifications (
    sequence BIGSERIAL PRIMARY KEY,
    space_id TEXT NOT NULL REFERENCES spaces(space_id),
    actor_principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    event_kind TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    evidence_object_id TEXT NOT NULL REFERENCES canonical_objects(object_id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(space_id,event_kind,evidence_object_id)
);
CREATE TABLE layrs_collaboration.notification_reads (
    space_id TEXT NOT NULL REFERENCES spaces(space_id),
    principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    last_sequence BIGINT NOT NULL CHECK (last_sequence>=0),
    PRIMARY KEY(space_id,principal_id)
);
DO $collaboration_grants$
DECLARE writer TEXT;
BEGIN
    SELECT writer_role_name INTO STRICT writer FROM schema_metadata WHERE singleton_id=1;
    EXECUTE format('GRANT USAGE ON SCHEMA layrs_collaboration TO %I',writer);
    EXECUTE format('GRANT SELECT,INSERT,UPDATE ON ALL TABLES IN SCHEMA layrs_collaboration TO %I',writer);
    EXECUTE format('GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA layrs_collaboration TO %I',writer);
END
$collaboration_grants$;
