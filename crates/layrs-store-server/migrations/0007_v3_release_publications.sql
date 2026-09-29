-- Append-only release lifecycle projection; immutable Evidence is the authority.
CREATE TABLE layrs_collaboration.release_events (
    space_id TEXT NOT NULL REFERENCES spaces(space_id),
    release_id TEXT NOT NULL CHECK (release_id LIKE 'release:%'),
    sequence BIGINT NOT NULL CHECK (sequence > 0),
    event_object_id TEXT NOT NULL UNIQUE REFERENCES canonical_objects(object_id),
    previous_event_object_id TEXT REFERENCES layrs_collaboration.release_events(event_object_id),
    release_product_object_id TEXT NOT NULL,
    product_kind TEXT NOT NULL DEFAULT 'release' CHECK (product_kind='release'),
    step_id TEXT NOT NULL,
    publication_id TEXT NOT NULL CHECK (publication_id LIKE 'release-publication:%'),
    event_kind TEXT NOT NULL CHECK (event_kind IN ('published','revoked')),
    actor_principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    request_id TEXT NOT NULL CHECK (octet_length(request_id) BETWEEN 16 AND 128),
    request_digest TEXT NOT NULL,
    canonical_event_jcs TEXT NOT NULL CHECK (octet_length(canonical_event_jcs) BETWEEN 2 AND 16384),
    PRIMARY KEY(space_id,release_id,sequence),
    UNIQUE(space_id,release_id,actor_principal_id,request_id),
    FOREIGN KEY(space_id,product_kind,release_id,release_product_object_id)
        REFERENCES products(space_id,product_kind,logical_id,product_object_id),
    FOREIGN KEY(space_id,step_id) REFERENCES space_steps(space_id,step_id)
);
DO $release_grants$
DECLARE writer TEXT;
BEGIN
    SELECT writer_role_name INTO STRICT writer FROM schema_metadata WHERE singleton_id=1;
    EXECUTE format('GRANT SELECT,INSERT ON layrs_collaboration.release_events TO %I',writer);
END
$release_grants$;
