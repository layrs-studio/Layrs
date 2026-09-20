-- Layrs PostgreSQL fresh baseline for the native SCM and layrs.sync.v3.
--
-- This is intentionally not an upgrade migration. It rejects every existing
-- user relation in the target schema before creating anything. Canonical JSON columns ending
-- in `_jcs` contain RFC 8785 UTF-8 text; JSONB is deliberately not used for
-- identity bytes because PostgreSQL may normalize their representation.
-- Canonical objects and chunks live in the configured CAS, never in BYTEA.

BEGIN;

SELECT pg_advisory_xact_lock(1818323315, 3);

DO $fresh_baseline$
DECLARE
    existing_object TEXT;
BEGIN
    WITH user_objects AS (
        SELECT
            'relation'::TEXT AS object_kind,
            namespace.nspname AS schema_name,
            value.relname AS object_name,
            value.relkind::TEXT AS object_detail
        FROM pg_catalog.pg_class AS value
        JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = value.relnamespace
        WHERE namespace.nspname = pg_catalog.current_schema()
          AND value.relkind IN ('r', 'p', 'v', 'm', 'S', 'f', 'c')

        UNION ALL

        SELECT
            'routine', namespace.nspname, value.proname,
            format('%s:%s', value.prokind, pg_catalog.pg_get_function_identity_arguments(value.oid))
        FROM pg_catalog.pg_proc AS value
        JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = value.pronamespace
        WHERE namespace.nspname = pg_catalog.current_schema()

        UNION ALL

        SELECT 'type', namespace.nspname, value.typname, value.typtype::TEXT
        FROM pg_catalog.pg_type AS value
        JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = value.typnamespace
        WHERE namespace.nspname = pg_catalog.current_schema()
          AND value.typrelid = 0
          AND NOT EXISTS (
              SELECT 1 FROM pg_catalog.pg_type AS element
              WHERE element.typarray = value.oid
          )
          AND NOT EXISTS (
              SELECT 1 FROM pg_catalog.pg_range AS range_value
              WHERE range_value.rngmultitypid = value.oid
          )

        UNION ALL

        SELECT 'collation', namespace.nspname, value.collname, value.collprovider::TEXT
        FROM pg_catalog.pg_collation AS value
        JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = value.collnamespace
        WHERE namespace.nspname = pg_catalog.current_schema()

        UNION ALL

        SELECT 'conversion', namespace.nspname, value.conname, value.condefault::TEXT
        FROM pg_catalog.pg_conversion AS value
        JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = value.connamespace
        WHERE namespace.nspname = pg_catalog.current_schema()

        UNION ALL

        SELECT 'operator', namespace.nspname, value.oprname, value.oid::TEXT
        FROM pg_catalog.pg_operator AS value
        JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = value.oprnamespace
        WHERE namespace.nspname = pg_catalog.current_schema()

        UNION ALL

        SELECT 'operator_class', namespace.nspname, value.opcname, value.opcmethod::TEXT
        FROM pg_catalog.pg_opclass AS value
        JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = value.opcnamespace
        WHERE namespace.nspname = pg_catalog.current_schema()

        UNION ALL

        SELECT 'operator_family', namespace.nspname, value.opfname, value.opfmethod::TEXT
        FROM pg_catalog.pg_opfamily AS value
        JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = value.opfnamespace
        WHERE namespace.nspname = pg_catalog.current_schema()

        UNION ALL

        SELECT 'text_search_configuration', namespace.nspname, value.cfgname, value.cfgparser::TEXT
        FROM pg_catalog.pg_ts_config AS value
        JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = value.cfgnamespace
        WHERE namespace.nspname = pg_catalog.current_schema()

        UNION ALL

        SELECT 'text_search_dictionary', namespace.nspname, value.dictname, value.dicttemplate::TEXT
        FROM pg_catalog.pg_ts_dict AS value
        JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = value.dictnamespace
        WHERE namespace.nspname = pg_catalog.current_schema()

        UNION ALL

        SELECT 'text_search_parser', namespace.nspname, value.prsname, value.prsstart::TEXT
        FROM pg_catalog.pg_ts_parser AS value
        JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = value.prsnamespace
        WHERE namespace.nspname = pg_catalog.current_schema()

        UNION ALL

        SELECT 'text_search_template', namespace.nspname, value.tmplname, value.tmplinit::TEXT
        FROM pg_catalog.pg_ts_template AS value
        JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = value.tmplnamespace
        WHERE namespace.nspname = pg_catalog.current_schema()
    )
    SELECT format('%I.%I (%s:%s)', schema_name, object_name, object_kind, object_detail)
    INTO existing_object
    FROM user_objects
    ORDER BY
        object_kind COLLATE "C",
        object_name COLLATE "C",
        object_detail COLLATE "C"
    LIMIT 1;

    IF existing_object IS NOT NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = '55000',
            MESSAGE = format(
                'target schema is not empty (%s); the Sync V3 baseline is fresh-only and refuses legacy or partial stores',
                existing_object
            );
    END IF;

    IF EXISTS (
        SELECT 1
        FROM pg_catalog.pg_roles
        WHERE rolname = 'layrs_v3_w_' || pg_catalog.substr(
            pg_catalog.md5(
                pg_catalog.current_database() || '/' || pg_catalog.current_schema()
            ),
            1,
            24
        )
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = '55000',
            MESSAGE = 'unsafe pre-existing Layrs Sync V3 writer role; baseline refuses to reuse cluster roles';
    END IF;
END
$fresh_baseline$;

-- Capture a safe, deterministic schema-first path in every SECURITY DEFINER
-- routine created by this migration. The server login must be a member of this
-- NOLOGIN group role; it must never connect as the migration owner.
SELECT pg_catalog.set_config(
    'search_path',
    pg_catalog.quote_ident(pg_catalog.current_schema()) || ',pg_catalog,pg_temp',
    true
);

CREATE TABLE schema_metadata (
    singleton_id SMALLINT PRIMARY KEY CHECK (singleton_id = 1),
    schema_version TEXT NOT NULL UNIQUE
        CHECK (schema_version = 'layrs.server-store.v3-baseline.1'),
    protocol_version TEXT NOT NULL
        CHECK (protocol_version = 'layrs.sync.v3'),
    baseline_migration TEXT NOT NULL
        CHECK (baseline_migration = '0001_sync_v3_baseline.sql'),
    schema_contract_checksum TEXT NOT NULL,
    catalog_fingerprint TEXT NOT NULL CHECK (catalog_fingerprint ~ '^md5:[0-9a-f]{32}$'),
    installed_schema TEXT NOT NULL,
    writer_role_name TEXT NOT NULL UNIQUE CHECK (writer_role_name ~ '^layrs_v3_w_[0-9a-f]{24}$'),
    installed_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp()
);

INSERT INTO schema_metadata (
    singleton_id,
    schema_version,
    protocol_version,
    baseline_migration,
    schema_contract_checksum,
    catalog_fingerprint,
    installed_schema,
    writer_role_name
) VALUES (
    1,
    'layrs.server-store.v3-baseline.1',
    'layrs.sync.v3',
    '0001_sync_v3_baseline.sql',
    'fnv1a64:03d739a95324da99',
    'md5:00000000000000000000000000000000',
    pg_catalog.current_schema(),
    'layrs_v3_w_' || pg_catalog.substr(
        pg_catalog.md5(pg_catalog.current_database() || '/' || pg_catalog.current_schema()),
        1,
        24
    )
);

DO $writer_role$
DECLARE
    role_name TEXT;
BEGIN
    SELECT writer_role_name INTO STRICT role_name
    FROM schema_metadata WHERE singleton_id = 1;
    EXECUTE format(
        'CREATE ROLE %I NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS',
        role_name
    );
END
$writer_role$;

DO $trust_install_schema$
DECLARE
    schema_name TEXT;
BEGIN
    SELECT installed_schema INTO STRICT schema_name
    FROM schema_metadata WHERE singleton_id = 1;
    EXECUTE format('REVOKE CREATE ON SCHEMA %I FROM PUBLIC', schema_name);
END
$trust_install_schema$;

CREATE TABLE stores (
    store_id TEXT PRIMARY KEY CHECK (store_id LIKE 'store:%'),
    backend_kind TEXT NOT NULL CHECK (backend_kind IN ('filesystem', 's3')),
    backend_config_jcs TEXT NOT NULL CHECK (octet_length(backend_config_jcs) > 1),
    created_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp()
);

CREATE TABLE deployments (
    deployment_id TEXT PRIMARY KEY CHECK (deployment_id LIKE 'deployment:%'),
    store_id TEXT NOT NULL REFERENCES stores(store_id) ON DELETE RESTRICT,
    display_name TEXT NOT NULL CHECK (length(btrim(display_name)) > 0),
    status TEXT NOT NULL DEFAULT 'ready' CHECK (status IN ('provisioning', 'ready', 'suspended')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (deployment_id, store_id)
);

CREATE TABLE principals (
    principal_id TEXT PRIMARY KEY CHECK (principal_id LIKE 'principal:%'),
    actor_id TEXT NOT NULL UNIQUE
        CHECK (actor_id ~ '^actor:[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$'),
    deployment_id TEXT NOT NULL REFERENCES deployments(deployment_id) ON DELETE RESTRICT,
    principal_kind TEXT NOT NULL CHECK (principal_kind IN ('account', 'service', 'bootstrap_admin')),
    subject_digest TEXT NOT NULL,
    display_name TEXT NOT NULL CHECK (length(btrim(display_name)) > 0),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended', 'revoked')),
    authorization_generation BIGINT NOT NULL DEFAULT 1 CHECK (authorization_generation > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    revoked_at TIMESTAMPTZ,
    UNIQUE (deployment_id, subject_digest),
    CHECK ((status = 'revoked') = (revoked_at IS NOT NULL))
);

CREATE TABLE workspaces (
    workspace_id TEXT PRIMARY KEY CHECK (workspace_id LIKE 'workspace:%'),
    deployment_id TEXT NOT NULL REFERENCES deployments(deployment_id) ON DELETE RESTRICT,
    slug TEXT NOT NULL CHECK (slug ~ '^[a-z0-9][a-z0-9-]{0,62}$'),
    display_name TEXT NOT NULL CHECK (length(btrim(display_name)) > 0),
    created_by_principal_id TEXT NOT NULL REFERENCES principals(principal_id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (deployment_id, slug)
);

CREATE TABLE workspace_memberships (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    principal_id TEXT NOT NULL REFERENCES principals(principal_id) ON DELETE RESTRICT,
    role TEXT NOT NULL CHECK (role IN ('owner', 'administrator', 'member', 'auditor')),
    state TEXT NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'suspended', 'revoked')),
    membership_generation BIGINT NOT NULL DEFAULT 1 CHECK (membership_generation > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    PRIMARY KEY (workspace_id, principal_id)
);

-- This row is the only mutable repository/policy/event authority on the server.
-- Pointer foreign keys are added after immutable Views and PolicySets exist.
CREATE TABLE spaces (
    space_id TEXT PRIMARY KEY CHECK (space_id LIKE 'space:%'),
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE RESTRICT,
    slug TEXT NOT NULL CHECK (slug ~ '^[a-z0-9][a-z0-9-]{0,62}$'),
    display_name TEXT NOT NULL CHECK (length(btrim(display_name)) > 0),
    lifecycle_state TEXT NOT NULL DEFAULT 'provisioning'
        CHECK (lifecycle_state IN ('provisioning', 'ready', 'suspended')),
    current_repository_view_id TEXT NOT NULL,
    current_repository_view_generation BIGINT NOT NULL DEFAULT 0
        CHECK (current_repository_view_generation >= 0),
    current_policy_set_id TEXT NOT NULL,
    authorization_epoch BIGINT NOT NULL DEFAULT 1 CHECK (authorization_epoch > 0),
    next_event_sequence BIGINT NOT NULL DEFAULT 1 CHECK (next_event_sequence > 0),
    created_by_principal_id TEXT NOT NULL REFERENCES principals(principal_id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (workspace_id, slug),
    UNIQUE (space_id, current_repository_view_id),
    UNIQUE (space_id, current_policy_set_id)
);

CREATE TABLE canonical_objects (
    object_id TEXT PRIMARY KEY CHECK (object_id ~ '^blake3:[0-9a-f]{64}$'),
    store_id TEXT NOT NULL REFERENCES stores(store_id) ON DELETE RESTRICT,
    object_kind TEXT NOT NULL CHECK (object_kind IN (
        'step', 'repository_view', 'chunk', 'file_manifest', 'tree',
        'conflict', 'symlink', 'tombstone', 'evidence', 'product',
        'policy_set', 'authorization_snapshot'
    )),
    canonical_byte_length BIGINT NOT NULL CHECK (canonical_byte_length >= 0),
    encoded_byte_length BIGINT NOT NULL CHECK (encoded_byte_length >= 0),
    compression TEXT NOT NULL CHECK (compression IN ('identity', 'zstd')),
    storage_key TEXT NOT NULL CHECK (length(storage_key) > 0),
    verification_state TEXT NOT NULL DEFAULT 'verified'
        CHECK (verification_state IN ('verified', 'quarantined')),
    verified_at TIMESTAMPTZ,
    installed_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (store_id, storage_key),
    CHECK ((verification_state = 'verified') = (verified_at IS NOT NULL))
);

CREATE TABLE chunks (
    chunk_id TEXT PRIMARY KEY REFERENCES canonical_objects(object_id) ON DELETE RESTRICT,
    raw_byte_length BIGINT NOT NULL CHECK (raw_byte_length >= 0),
    backend_etag TEXT,
    CHECK (chunk_id ~ '^blake3:[0-9a-f]{64}$')
);

CREATE TABLE file_manifests (
    file_manifest_id TEXT PRIMARY KEY REFERENCES canonical_objects(object_id) ON DELETE RESTRICT,
    file_id TEXT NOT NULL CHECK (file_id ~ '^blake3:[0-9a-f]{64}$'),
    file_byte_length BIGINT NOT NULL CHECK (file_byte_length >= 0),
    chunker_name TEXT NOT NULL CHECK (length(chunker_name) > 0),
    chunker_version TEXT NOT NULL CHECK (length(chunker_version) > 0),
    chunker_parameters_jcs TEXT NOT NULL CHECK (octet_length(chunker_parameters_jcs) > 1),
    reconstruction_verified_at TIMESTAMPTZ NOT NULL,
    UNIQUE (file_manifest_id, file_id)
);

CREATE TABLE file_manifest_chunks (
    file_manifest_id TEXT NOT NULL REFERENCES file_manifests(file_manifest_id) ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    chunk_id TEXT NOT NULL REFERENCES chunks(chunk_id) ON DELETE RESTRICT,
    byte_offset BIGINT NOT NULL CHECK (byte_offset >= 0),
    byte_length BIGINT NOT NULL CHECK (byte_length >= 0),
    PRIMARY KEY (file_manifest_id, ordinal),
    UNIQUE (file_manifest_id, byte_offset),
    CHECK (byte_offset <= 9223372036854775807 - byte_length)
);

CREATE INDEX file_manifest_chunks_by_chunk
    ON file_manifest_chunks (chunk_id, file_manifest_id);
CREATE INDEX file_manifests_by_file
    ON file_manifests (file_id, reconstruction_verified_at DESC);

CREATE TABLE canonical_object_edges (
    source_object_id TEXT NOT NULL REFERENCES canonical_objects(object_id) ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    edge_kind TEXT NOT NULL CHECK (edge_kind IN (
        'tree_entry', 'conflict_side', 'manifest_chunk', 'product_dependency',
        'policy_dependency', 'evidence', 'authorization_evidence'
    )),
    target_object_id TEXT REFERENCES canonical_objects(object_id) ON DELETE RESTRICT,
    target_file_id TEXT CHECK (target_file_id IS NULL OR target_file_id ~ '^blake3:[0-9a-f]{64}$'),
    repo_path TEXT,
    PRIMARY KEY (source_object_id, ordinal),
    CHECK ((target_object_id IS NOT NULL)::INTEGER + (target_file_id IS NOT NULL)::INTEGER = 1),
    CHECK (repo_path IS NULL OR (repo_path <> '' AND repo_path !~ '(^|/)\.\.(/|$)'))
);

CREATE INDEX canonical_object_edges_by_object_target
    ON canonical_object_edges (target_object_id) WHERE target_object_id IS NOT NULL;
CREATE INDEX canonical_object_edges_by_file_target
    ON canonical_object_edges (target_file_id) WHERE target_file_id IS NOT NULL;

CREATE TABLE space_object_availability (
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    object_id TEXT NOT NULL REFERENCES canonical_objects(object_id) ON DELETE RESTRICT,
    availability_state TEXT NOT NULL DEFAULT 'available'
        CHECK (availability_state IN ('available', 'leased_orphan', 'quarantined')),
    admitted_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    PRIMARY KEY (space_id, object_id)
);

CREATE INDEX space_object_availability_by_state
    ON space_object_availability (space_id, availability_state, object_id);

CREATE TABLE steps (
    step_id TEXT PRIMARY KEY REFERENCES canonical_objects(object_id) ON DELETE RESTRICT,
    root_tree_id TEXT NOT NULL REFERENCES canonical_objects(object_id) ON DELETE RESTRICT,
    canonical_record_jcs TEXT NOT NULL CHECK (octet_length(canonical_record_jcs) > 1),
    admitted_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp()
);

CREATE TABLE step_parents (
    step_id TEXT NOT NULL REFERENCES steps(step_id) ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    parent_step_id TEXT NOT NULL REFERENCES steps(step_id) ON DELETE RESTRICT,
    PRIMARY KEY (step_id, ordinal),
    UNIQUE (step_id, parent_step_id),
    CHECK (step_id <> parent_step_id)
);

CREATE INDEX step_parents_by_parent ON step_parents (parent_step_id, step_id);

CREATE TABLE space_steps (
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    step_id TEXT NOT NULL REFERENCES steps(step_id) ON DELETE RESTRICT,
    admitted_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    PRIMARY KEY (space_id, step_id)
);

CREATE TABLE step_observations (
    observation_id TEXT PRIMARY KEY CHECK (observation_id LIKE 'observation:%'),
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    step_id TEXT NOT NULL,
    canonical_record_jcs TEXT NOT NULL CHECK (octet_length(canonical_record_jcs) > 1),
    observed_at TIMESTAMPTZ NOT NULL,
    accepted_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (space_id, observation_id),
    FOREIGN KEY (space_id, step_id)
        REFERENCES space_steps(space_id, step_id) ON DELETE RESTRICT
);

CREATE INDEX step_observations_by_step ON step_observations (space_id, step_id, observed_at);

CREATE TABLE products (
    product_object_id TEXT PRIMARY KEY REFERENCES canonical_objects(object_id) ON DELETE RESTRICT,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    product_kind TEXT NOT NULL CHECK (product_kind IN (
        'layer', 'marker', 'release', 'proof', 'gateDefinition',
        'gateEvaluation', 'policy', 'weave', 'decision'
    )),
    logical_id TEXT NOT NULL,
    previous_product_object_id TEXT,
    immutable_subject_jcs TEXT,
    canonical_envelope_jcs TEXT NOT NULL CHECK (octet_length(canonical_envelope_jcs) > 1),
    admitted_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (space_id, product_object_id),
    UNIQUE (space_id, product_kind, logical_id, product_object_id),
    FOREIGN KEY (space_id, product_kind, logical_id, previous_product_object_id)
        REFERENCES products(space_id, product_kind, logical_id, product_object_id)
        ON DELETE RESTRICT,
    CHECK (product_kind = 'layer' OR previous_product_object_id IS NULL),
    CHECK (
        (product_kind = 'layer' AND logical_id LIKE 'layer:%')
        OR (product_kind = 'marker' AND logical_id LIKE 'marker:%')
        OR (product_kind = 'release' AND logical_id LIKE 'release:%')
        OR (product_kind = 'proof' AND logical_id LIKE 'proof:%')
        OR (product_kind = 'gateDefinition' AND logical_id LIKE 'gate-definition:%')
        OR (product_kind = 'gateEvaluation' AND logical_id LIKE 'gate-evaluation:%')
        OR (product_kind = 'policy' AND logical_id LIKE 'policy:%')
        OR (product_kind = 'weave' AND logical_id LIKE 'weave:%')
        OR (product_kind = 'decision' AND logical_id LIKE 'decision:%')
    )
);

CREATE INDEX products_by_logical_id
    ON products (space_id, product_kind, logical_id, admitted_at DESC);
CREATE UNIQUE INDEX products_singleton_logical_identity
    ON products (space_id, product_kind, logical_id)
    WHERE product_kind IN (
        'marker', 'release', 'proof', 'gateEvaluation', 'weave', 'decision'
    );

CREATE TABLE product_step_dependencies (
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    product_object_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    step_id TEXT NOT NULL,
    PRIMARY KEY (product_object_id, ordinal),
    UNIQUE (product_object_id, step_id),
    FOREIGN KEY (space_id, product_object_id)
        REFERENCES products(space_id, product_object_id) ON DELETE RESTRICT,
    FOREIGN KEY (space_id, step_id)
        REFERENCES space_steps(space_id, step_id) ON DELETE RESTRICT
);

CREATE TABLE product_dependencies (
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    product_object_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    dependency_product_object_id TEXT NOT NULL,
    PRIMARY KEY (product_object_id, ordinal),
    UNIQUE (product_object_id, dependency_product_object_id),
    FOREIGN KEY (space_id, product_object_id)
        REFERENCES products(space_id, product_object_id) ON DELETE RESTRICT,
    FOREIGN KEY (space_id, dependency_product_object_id)
        REFERENCES products(space_id, product_object_id) ON DELETE RESTRICT,
    CHECK (product_object_id <> dependency_product_object_id)
);

CREATE TABLE evidence_descriptors (
    evidence_object_id TEXT PRIMARY KEY REFERENCES canonical_objects(object_id) ON DELETE RESTRICT,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    evidence_kind TEXT NOT NULL CHECK (length(evidence_kind) > 0),
    immutable_subject_jcs TEXT NOT NULL CHECK (octet_length(immutable_subject_jcs) > 1),
    media_type TEXT,
    descriptor_jcs TEXT NOT NULL CHECK (octet_length(descriptor_jcs) > 1),
    admitted_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp()
);

CREATE TABLE policy_sets (
    policy_set_id TEXT PRIMARY KEY REFERENCES canonical_objects(object_id) ON DELETE RESTRICT,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    canonical_record_jcs TEXT NOT NULL CHECK (octet_length(canonical_record_jcs) > 1),
    admitted_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (space_id, policy_set_id)
);

CREATE TABLE policy_set_entries (
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    policy_set_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    policy_id TEXT NOT NULL CHECK (policy_id LIKE 'policy:%'),
    policy_version_id TEXT NOT NULL CHECK (policy_version_id LIKE 'policy-version:%'),
    policy_product_object_id TEXT NOT NULL,
    dependency_object_ids_jcs TEXT NOT NULL CHECK (octet_length(dependency_object_ids_jcs) > 1),
    PRIMARY KEY (policy_set_id, ordinal),
    UNIQUE (policy_set_id, policy_id),
    UNIQUE (policy_set_id, policy_version_id),
    UNIQUE (policy_set_id, policy_product_object_id),
    FOREIGN KEY (space_id, policy_set_id)
        REFERENCES policy_sets(space_id, policy_set_id) ON DELETE RESTRICT,
    FOREIGN KEY (space_id, policy_product_object_id)
        REFERENCES products(space_id, product_object_id) ON DELETE RESTRICT
);

CREATE TABLE repository_views (
    repository_view_id TEXT PRIMARY KEY REFERENCES canonical_objects(object_id) ON DELETE RESTRICT,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    canonical_record_jcs TEXT NOT NULL CHECK (octet_length(canonical_record_jcs) > 1),
    admitted_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (space_id, repository_view_id)
);

-- Immutable projection of each View. It is rebuildable from the canonical View
-- and is never a mutable Layer-head authority.
CREATE TABLE repository_view_layer_heads (
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    repository_view_id TEXT NOT NULL,
    layer_id TEXT NOT NULL CHECK (layer_id LIKE 'layer:%'),
    step_id TEXT NOT NULL,
    PRIMARY KEY (repository_view_id, layer_id, step_id),
    UNIQUE (space_id, repository_view_id, layer_id, step_id),
    FOREIGN KEY (space_id, repository_view_id)
        REFERENCES repository_views(space_id, repository_view_id) ON DELETE RESTRICT,
    FOREIGN KEY (space_id, step_id)
        REFERENCES space_steps(space_id, step_id) ON DELETE RESTRICT
);

CREATE TABLE repository_view_markers (
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    repository_view_id TEXT NOT NULL,
    product_kind TEXT GENERATED ALWAYS AS ('marker'::TEXT) STORED,
    marker_id TEXT NOT NULL CHECK (marker_id LIKE 'marker:%'),
    marker_product_object_id TEXT NOT NULL,
    PRIMARY KEY (repository_view_id, marker_id),
    FOREIGN KEY (space_id, repository_view_id)
        REFERENCES repository_views(space_id, repository_view_id) ON DELETE RESTRICT,
    FOREIGN KEY (space_id, product_kind, marker_id, marker_product_object_id)
        REFERENCES products(space_id, product_kind, logical_id, product_object_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
);

CREATE TABLE repository_view_releases (
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    repository_view_id TEXT NOT NULL,
    product_kind TEXT GENERATED ALWAYS AS ('release'::TEXT) STORED,
    release_id TEXT NOT NULL CHECK (release_id LIKE 'release:%'),
    release_product_object_id TEXT NOT NULL,
    PRIMARY KEY (repository_view_id, release_id),
    FOREIGN KEY (space_id, repository_view_id)
        REFERENCES repository_views(space_id, repository_view_id) ON DELETE RESTRICT,
    FOREIGN KEY (space_id, product_kind, release_id, release_product_object_id)
        REFERENCES products(space_id, product_kind, logical_id, product_object_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
);

ALTER TABLE spaces
    ADD CONSTRAINT spaces_current_view_same_space_fk
        FOREIGN KEY (space_id, current_repository_view_id)
        REFERENCES repository_views(space_id, repository_view_id)
        DEFERRABLE INITIALLY DEFERRED,
    ADD CONSTRAINT spaces_current_policy_same_space_fk
        FOREIGN KEY (space_id, current_policy_set_id)
        REFERENCES policy_sets(space_id, policy_set_id)
        DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE current_layer_heads_projection (
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    repository_view_id TEXT NOT NULL REFERENCES repository_views(repository_view_id) ON DELETE RESTRICT,
    repository_view_generation BIGINT NOT NULL CHECK (repository_view_generation >= 0),
    layer_id TEXT NOT NULL CHECK (layer_id LIKE 'layer:%'),
    step_id TEXT NOT NULL REFERENCES steps(step_id) ON DELETE RESTRICT,
    PRIMARY KEY (space_id, layer_id, step_id),
    FOREIGN KEY (space_id, repository_view_id, layer_id, step_id)
        REFERENCES repository_view_layer_heads(space_id, repository_view_id, layer_id, step_id)
        ON DELETE RESTRICT
);

COMMENT ON TABLE current_layer_heads_projection IS
    'Disposable projection rebuilt only from spaces.current_repository_view_id; never an authority.';

CREATE TABLE authorization_snapshots (
    authorization_snapshot_id TEXT PRIMARY KEY REFERENCES canonical_objects(object_id) ON DELETE RESTRICT,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    principal_id TEXT REFERENCES principals(principal_id) ON DELETE RESTRICT,
    subject_digest TEXT,
    policy_set_id TEXT NOT NULL REFERENCES policy_sets(policy_set_id) ON DELETE RESTRICT,
    authorization_epoch BIGINT NOT NULL CHECK (authorization_epoch > 0),
    request_binding_jcs TEXT NOT NULL CHECK (octet_length(request_binding_jcs) > 1),
    decisions_jcs TEXT NOT NULL CHECK (octet_length(decisions_jcs) > 1),
    canonical_record_jcs TEXT NOT NULL CHECK (octet_length(canonical_record_jcs) > 1),
    evaluated_at TIMESTAMPTZ NOT NULL,
    installed_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (space_id, authorization_snapshot_id),
    FOREIGN KEY (space_id, policy_set_id)
        REFERENCES policy_sets(space_id, policy_set_id) ON DELETE RESTRICT,
    CHECK ((principal_id IS NOT NULL)::INTEGER + (subject_digest IS NOT NULL)::INTEGER = 1)
);

CREATE TABLE sync_transactions (
    transaction_id TEXT NOT NULL CHECK (transaction_id LIKE 'transaction:%'),
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    client_id TEXT NOT NULL CHECK (client_id LIKE 'client:%'),
    idempotency_key TEXT NOT NULL CHECK (length(idempotency_key) BETWEEN 16 AND 128),
    request_digest TEXT NOT NULL CHECK (request_digest ~ '^blake3:[0-9a-f]{64}$'),
    principal_id TEXT NOT NULL REFERENCES principals(principal_id) ON DELETE RESTRICT,
    authorization_snapshot_id TEXT NOT NULL,
    authorization_epoch BIGINT NOT NULL CHECK (authorization_epoch > 0),
    resulting_authorization_epoch BIGINT NOT NULL CHECK (resulting_authorization_epoch > 0),
    receipt_canonical_jcs TEXT NOT NULL CHECK (octet_length(receipt_canonical_jcs) > 1),
    receipt_digest TEXT NOT NULL CHECK (receipt_digest ~ '^blake3:[0-9a-f]{64}$'),
    first_event_sequence BIGINT,
    last_event_sequence BIGINT,
    committed_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    PRIMARY KEY (space_id, client_id, idempotency_key),
    UNIQUE (space_id, transaction_id),
    FOREIGN KEY (space_id, authorization_snapshot_id)
        REFERENCES authorization_snapshots(space_id, authorization_snapshot_id) ON DELETE RESTRICT,
    CHECK (
        (first_event_sequence IS NULL AND last_event_sequence IS NULL)
        OR
        (first_event_sequence IS NOT NULL AND last_event_sequence IS NOT NULL
            AND first_event_sequence > 0 AND last_event_sequence >= first_event_sequence)
    )
);

CREATE INDEX sync_transactions_by_commit
    ON sync_transactions (space_id, committed_at, transaction_id);

-- Product admission is immutable; this is the sole mutable authority selecting
-- the current Layer definition. Generation and previous product are exact CAS
-- preconditions, never timestamp/last-write-wins signals.
CREATE TABLE current_layer_definitions (
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    layer_id TEXT NOT NULL CHECK (layer_id LIKE 'layer:%'),
    product_kind TEXT GENERATED ALWAYS AS ('layer'::TEXT) STORED,
    current_product_object_id TEXT NOT NULL,
    generation BIGINT NOT NULL CHECK (generation >= 0),
    updated_transaction_id TEXT,
    PRIMARY KEY (space_id, layer_id),
    UNIQUE (space_id, layer_id, generation, current_product_object_id),
    FOREIGN KEY (space_id, product_kind, layer_id, current_product_object_id)
        REFERENCES products(space_id, product_kind, logical_id, product_object_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (space_id, updated_transaction_id)
        REFERENCES sync_transactions(space_id, transaction_id) ON DELETE RESTRICT
);

CREATE TABLE layer_definition_history (
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    layer_id TEXT NOT NULL CHECK (layer_id LIKE 'layer:%'),
    product_kind TEXT GENERATED ALWAYS AS ('layer'::TEXT) STORED,
    generation BIGINT NOT NULL CHECK (generation >= 0),
    product_object_id TEXT NOT NULL,
    previous_product_object_id TEXT,
    transaction_id TEXT,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    PRIMARY KEY (space_id, layer_id, generation),
    UNIQUE (space_id, layer_id, generation, product_object_id),
    FOREIGN KEY (space_id, product_kind, layer_id, product_object_id)
        REFERENCES products(space_id, product_kind, logical_id, product_object_id)
        ON DELETE RESTRICT,
    FOREIGN KEY (space_id, product_kind, layer_id, previous_product_object_id)
        REFERENCES products(space_id, product_kind, logical_id, product_object_id)
        ON DELETE RESTRICT,
    FOREIGN KEY (space_id, transaction_id)
        REFERENCES sync_transactions(space_id, transaction_id) ON DELETE RESTRICT,
    CHECK (
        (generation = 0 AND previous_product_object_id IS NULL)
        OR (generation > 0 AND previous_product_object_id IS NOT NULL)
    )
);

CREATE FUNCTION layrs_reject_layer_definition_history_mutation()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $layer_history_guard$
BEGIN
    RAISE EXCEPTION USING
        ERRCODE = '55000',
        MESSAGE = 'Layer definition history is append-only';
END
$layer_history_guard$;

CREATE TRIGGER layer_definition_history_append_only
    BEFORE UPDATE OR DELETE ON layer_definition_history
    FOR EACH ROW EXECUTE FUNCTION layrs_reject_layer_definition_history_mutation();

CREATE FUNCTION layrs_cas_layer_definition(
    target_space_id TEXT,
    target_layer_id TEXT,
    expected_product_object_id TEXT,
    expected_generation BIGINT,
    proposed_product_object_id TEXT,
    source_transaction_id TEXT DEFAULT NULL
)
RETURNS BIGINT
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path FROM CURRENT
AS $layer_definition_cas$
DECLARE
    proposed_previous_product_object_id TEXT;
    resulting_generation BIGINT;
    affected_rows BIGINT;
BEGIN
    IF expected_generation < 0 OR expected_generation = 9223372036854775807 THEN
        RAISE EXCEPTION USING ERRCODE = '22003', MESSAGE = 'invalid Layer definition generation';
    END IF;

    SELECT previous_product_object_id
    INTO proposed_previous_product_object_id
    FROM products
    WHERE space_id = target_space_id
      AND product_kind = 'layer'
      AND logical_id = target_layer_id
      AND product_object_id = proposed_product_object_id;

    IF NOT FOUND THEN
        RAISE EXCEPTION USING
            ERRCODE = '23503',
            MESSAGE = 'proposed Layer definition Product is missing, wrong-kind, wrong-Layer, or cross-Space';
    END IF;

    IF proposed_previous_product_object_id IS DISTINCT FROM expected_product_object_id THEN
        RAISE EXCEPTION USING
            ERRCODE = '40001',
            MESSAGE = 'Layer definition previous Product CAS mismatch';
    END IF;

    IF expected_product_object_id IS NULL THEN
        IF expected_generation <> 0 THEN
            RAISE EXCEPTION USING
                ERRCODE = '40001',
                MESSAGE = 'new Layer definition must start at generation zero';
        END IF;
        INSERT INTO current_layer_definitions (
            space_id, layer_id, current_product_object_id, generation, updated_transaction_id
        ) VALUES (
            target_space_id, target_layer_id, proposed_product_object_id, 0, source_transaction_id
        ) ON CONFLICT DO NOTHING;
        GET DIAGNOSTICS affected_rows = ROW_COUNT;
        IF affected_rows <> 1 THEN
            RAISE EXCEPTION USING ERRCODE = '40001', MESSAGE = 'Layer definition create CAS mismatch';
        END IF;
        resulting_generation := 0;
    ELSE
        resulting_generation := expected_generation + 1;
        UPDATE current_layer_definitions
        SET current_product_object_id = proposed_product_object_id,
            generation = resulting_generation,
            updated_transaction_id = source_transaction_id
        WHERE space_id = target_space_id
          AND layer_id = target_layer_id
          AND generation = expected_generation
          AND current_product_object_id = expected_product_object_id;
        GET DIAGNOSTICS affected_rows = ROW_COUNT;
        IF affected_rows <> 1 THEN
            RAISE EXCEPTION USING ERRCODE = '40001', MESSAGE = 'Layer definition CAS mismatch';
        END IF;
    END IF;

    INSERT INTO layer_definition_history (
        space_id, layer_id, generation, product_object_id,
        previous_product_object_id, transaction_id
    ) VALUES (
        target_space_id, target_layer_id, resulting_generation, proposed_product_object_id,
        expected_product_object_id, source_transaction_id
    );

    RETURN resulting_generation;
END
$layer_definition_cas$;

REVOKE ALL ON FUNCTION layrs_cas_layer_definition(
    TEXT, TEXT, TEXT, BIGINT, TEXT, TEXT
) FROM PUBLIC;
REVOKE INSERT, UPDATE, DELETE, TRUNCATE
    ON current_layer_definitions, layer_definition_history
    FROM PUBLIC;
DO $writer_privileges$
DECLARE
    role_name TEXT;
    schema_name TEXT;
BEGIN
    SELECT writer_role_name, installed_schema
    INTO STRICT role_name, schema_name
    FROM schema_metadata WHERE singleton_id = 1;
    EXECUTE format(
        'GRANT EXECUTE ON FUNCTION %I.layrs_cas_layer_definition(TEXT, TEXT, TEXT, BIGINT, TEXT, TEXT) TO %I',
        schema_name,
        role_name
    );
    EXECUTE format(
        'REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON %I.current_layer_definitions, %I.layer_definition_history FROM %I',
        schema_name,
        schema_name,
        role_name
    );
    EXECUTE format(
        'GRANT SELECT ON %I.current_layer_definitions, %I.layer_definition_history TO %I',
        schema_name,
        schema_name,
        role_name
    );
    EXECUTE format(
        'GRANT USAGE ON SCHEMA %I TO %I',
        schema_name,
        role_name
    );
END
$writer_privileges$;

CREATE TABLE repository_view_history (
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    generation BIGINT NOT NULL CHECK (generation >= 0),
    repository_view_id TEXT NOT NULL REFERENCES repository_views(repository_view_id) ON DELETE RESTRICT,
    transaction_id TEXT,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    PRIMARY KEY (space_id, generation),
    UNIQUE (space_id, generation, repository_view_id),
    FOREIGN KEY (space_id, repository_view_id)
        REFERENCES repository_views(space_id, repository_view_id) ON DELETE RESTRICT,
    FOREIGN KEY (space_id, transaction_id)
        REFERENCES sync_transactions(space_id, transaction_id) ON DELETE RESTRICT
);

CREATE TABLE policy_root_history (
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    authorization_epoch BIGINT NOT NULL CHECK (authorization_epoch > 0),
    policy_set_id TEXT NOT NULL REFERENCES policy_sets(policy_set_id) ON DELETE RESTRICT,
    transaction_id TEXT,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    PRIMARY KEY (space_id, authorization_epoch),
    UNIQUE (space_id, authorization_epoch, policy_set_id),
    FOREIGN KEY (space_id, policy_set_id)
        REFERENCES policy_sets(space_id, policy_set_id) ON DELETE RESTRICT,
    FOREIGN KEY (space_id, transaction_id)
        REFERENCES sync_transactions(space_id, transaction_id) ON DELETE RESTRICT
);

CREATE TABLE space_events (
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    sequence BIGINT NOT NULL CHECK (sequence > 0),
    event_id TEXT NOT NULL UNIQUE CHECK (event_id LIKE 'event:%'),
    transaction_id TEXT NOT NULL,
    event_kind TEXT NOT NULL CHECK (event_kind IN (
        'steps.accepted', 'products.accepted', 'layer.definition_changed',
        'repository_view.changed', 'policy.root_changed'
    )),
    actor_kind TEXT NOT NULL CHECK (actor_kind IN ('account', 'device', 'automation', 'server')),
    actor_id TEXT NOT NULL REFERENCES principals(actor_id) ON DELETE RESTRICT
        CHECK (actor_id ~ '^actor:[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$'),
    device_id TEXT
        CHECK (device_id IS NULL OR device_id ~ '^device:[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$'),
    payload_canonical_jcs TEXT NOT NULL CHECK (octet_length(payload_canonical_jcs) > 1),
    created_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    PRIMARY KEY (space_id, sequence),
    FOREIGN KEY (space_id, transaction_id)
        REFERENCES sync_transactions(space_id, transaction_id) ON DELETE RESTRICT,
    UNIQUE (space_id, event_id)
);

CREATE INDEX space_events_by_transaction
    ON space_events (space_id, transaction_id, sequence);

CREATE TABLE object_plans (
    plan_id TEXT PRIMARY KEY CHECK (plan_id LIKE 'plan:%'),
    request_id TEXT NOT NULL CHECK (request_id LIKE 'request:%'),
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    client_id TEXT NOT NULL CHECK (client_id LIKE 'client:%'),
    principal_id TEXT NOT NULL REFERENCES principals(principal_id) ON DELETE RESTRICT,
    authorization_epoch BIGINT NOT NULL CHECK (authorization_epoch > 0),
    request_descriptors_digest TEXT NOT NULL CHECK (request_descriptors_digest ~ '^blake3:[0-9a-f]{64}$'),
    plan_state TEXT NOT NULL DEFAULT 'open' CHECK (plan_state IN ('open', 'finalized')),
    expires_at TIMESTAMPTZ NOT NULL,
    finalized_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (space_id, client_id, request_id),
    UNIQUE (plan_id, principal_id, authorization_epoch, space_id, client_id, request_id),
    CHECK ((plan_state = 'finalized') = (finalized_at IS NOT NULL))
);

CREATE TABLE object_plan_items (
    plan_id TEXT NOT NULL REFERENCES object_plans(plan_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    object_id TEXT NOT NULL CHECK (object_id ~ '^blake3:[0-9a-f]{64}$'),
    object_kind TEXT NOT NULL CHECK (object_kind IN (
        'step', 'repository_view', 'chunk', 'file_manifest', 'tree',
        'conflict', 'symlink', 'tombstone', 'evidence', 'product', 'policy_set'
    )),
    canonical_byte_length BIGINT NOT NULL CHECK (canonical_byte_length >= 0),
    encoded_byte_length BIGINT NOT NULL CHECK (encoded_byte_length >= 0),
    compression TEXT NOT NULL CHECK (compression IN ('identity', 'zstd')),
    descriptor_digest TEXT NOT NULL CHECK (descriptor_digest ~ '^blake3:[0-9a-f]{64}$'),
    disposition TEXT NOT NULL CHECK (disposition IN ('present', 'missing')),
    PRIMARY KEY (plan_id, ordinal),
    UNIQUE (plan_id, object_id),
    UNIQUE (plan_id, ordinal, disposition, object_id, descriptor_digest)
);

CREATE TABLE upload_capabilities (
    capability_id TEXT PRIMARY KEY CHECK (capability_id LIKE 'capability:%'),
    plan_id TEXT NOT NULL,
    item_ordinal INTEGER NOT NULL,
    item_disposition TEXT GENERATED ALWAYS AS ('missing'::TEXT) STORED,
    object_id TEXT NOT NULL CHECK (object_id ~ '^blake3:[0-9a-f]{64}$'),
    descriptor_digest TEXT NOT NULL CHECK (descriptor_digest ~ '^blake3:[0-9a-f]{64}$'),
    space_id TEXT NOT NULL,
    client_id TEXT NOT NULL CHECK (client_id LIKE 'client:%'),
    request_id TEXT NOT NULL CHECK (request_id LIKE 'request:%'),
    principal_id TEXT NOT NULL REFERENCES principals(principal_id) ON DELETE RESTRICT,
    authorization_epoch BIGINT NOT NULL CHECK (authorization_epoch > 0),
    token_digest TEXT NOT NULL UNIQUE CHECK (length(token_digest) > 0),
    method TEXT NOT NULL CHECK (method = 'PUT'),
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    FOREIGN KEY (plan_id, principal_id, authorization_epoch, space_id, client_id, request_id)
        REFERENCES object_plans(
            plan_id, principal_id, authorization_epoch, space_id, client_id, request_id
        ) ON DELETE CASCADE,
    FOREIGN KEY (plan_id, item_ordinal, item_disposition, object_id, descriptor_digest)
        REFERENCES object_plan_items(
            plan_id, ordinal, disposition, object_id, descriptor_digest
        ) ON DELETE CASCADE,
    UNIQUE (plan_id, item_ordinal)
);

CREATE TABLE fetch_plans (
    plan_id TEXT PRIMARY KEY CHECK (plan_id LIKE 'plan:%'),
    request_id TEXT NOT NULL CHECK (request_id LIKE 'request:%'),
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    client_id TEXT NOT NULL CHECK (client_id LIKE 'client:%'),
    principal_id TEXT NOT NULL REFERENCES principals(principal_id) ON DELETE RESTRICT,
    authorization_epoch BIGINT NOT NULL CHECK (authorization_epoch > 0),
    request_claims_digest TEXT NOT NULL CHECK (request_claims_digest ~ '^blake3:[0-9a-f]{64}$'),
    last_scanned_claim INTEGER NOT NULL DEFAULT -1 CHECK (last_scanned_claim >= -1),
    plan_state TEXT NOT NULL DEFAULT 'open' CHECK (plan_state IN ('open', 'finalized')),
    expires_at TIMESTAMPTZ NOT NULL,
    finalized_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (space_id, client_id, request_id),
    UNIQUE (plan_id, principal_id, authorization_epoch, space_id, client_id, request_id),
    CHECK ((plan_state = 'finalized') = (finalized_at IS NOT NULL))
);

CREATE TABLE fetch_plan_claims (
    plan_id TEXT NOT NULL REFERENCES fetch_plans(plan_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    claim_kind TEXT NOT NULL CHECK (claim_kind IN ('object', 'file', 'authorization_snapshot')),
    claim_canonical_jcs TEXT NOT NULL CHECK (octet_length(claim_canonical_jcs) > 1),
    decision TEXT NOT NULL CHECK (decision IN ('allowed', 'denied')),
    denied_code TEXT CHECK (denied_code IN (
        'not_reachable', 'path_denied', 'audit_denied', 'not_available',
        'invalid_root', 'invalid_path', 'no_verified_manifest'
    )),
    selected_file_manifest_id TEXT REFERENCES file_manifests(file_manifest_id) ON DELETE RESTRICT,
    PRIMARY KEY (plan_id, ordinal),
    UNIQUE (plan_id, ordinal, decision),
    CHECK ((decision = 'denied') = (denied_code IS NOT NULL)),
    CHECK (decision = 'allowed' OR selected_file_manifest_id IS NULL)
);

-- One allowed wire claim may expand to multiple exact descriptors. For a file
-- this is the selected FileManifest plus every required Chunk.
CREATE TABLE fetch_claim_objects (
    plan_id TEXT NOT NULL,
    claim_ordinal INTEGER NOT NULL,
    claim_decision TEXT GENERATED ALWAYS AS ('allowed'::TEXT) STORED,
    object_ordinal INTEGER NOT NULL CHECK (object_ordinal >= 0),
    object_id TEXT NOT NULL REFERENCES canonical_objects(object_id) ON DELETE RESTRICT,
    descriptor_digest TEXT NOT NULL CHECK (descriptor_digest ~ '^blake3:[0-9a-f]{64}$'),
    PRIMARY KEY (plan_id, claim_ordinal, object_ordinal),
    UNIQUE (plan_id, claim_ordinal, object_id),
    UNIQUE (plan_id, claim_ordinal, claim_decision, object_id, descriptor_digest),
    FOREIGN KEY (plan_id, claim_ordinal, claim_decision)
        REFERENCES fetch_plan_claims(plan_id, ordinal, decision) ON DELETE CASCADE
);

CREATE TABLE fetch_capabilities (
    capability_id TEXT PRIMARY KEY CHECK (capability_id LIKE 'capability:%'),
    plan_id TEXT NOT NULL,
    claim_ordinal INTEGER NOT NULL,
    claim_decision TEXT GENERATED ALWAYS AS ('allowed'::TEXT) STORED,
    object_id TEXT NOT NULL REFERENCES canonical_objects(object_id) ON DELETE RESTRICT,
    descriptor_digest TEXT NOT NULL CHECK (descriptor_digest ~ '^blake3:[0-9a-f]{64}$'),
    space_id TEXT NOT NULL,
    client_id TEXT NOT NULL CHECK (client_id LIKE 'client:%'),
    request_id TEXT NOT NULL CHECK (request_id LIKE 'request:%'),
    principal_id TEXT NOT NULL REFERENCES principals(principal_id) ON DELETE RESTRICT,
    authorization_epoch BIGINT NOT NULL CHECK (authorization_epoch > 0),
    token_digest TEXT NOT NULL UNIQUE CHECK (length(token_digest) > 0),
    resume_token_digest TEXT NOT NULL CHECK (length(resume_token_digest) > 0),
    method TEXT NOT NULL CHECK (method = 'GET'),
    expires_at TIMESTAMPTZ NOT NULL,
    FOREIGN KEY (plan_id, principal_id, authorization_epoch, space_id, client_id, request_id)
        REFERENCES fetch_plans(
            plan_id, principal_id, authorization_epoch, space_id, client_id, request_id
        ) ON DELETE CASCADE,
    FOREIGN KEY (plan_id, claim_ordinal, claim_decision, object_id, descriptor_digest)
        REFERENCES fetch_claim_objects(
            plan_id, claim_ordinal, claim_decision, object_id, descriptor_digest
        ) ON DELETE CASCADE,
    UNIQUE (plan_id, claim_ordinal, object_id)
);

CREATE FUNCTION layrs_reject_finalized_plan_mutation()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $plan_guard$
DECLARE
    guarded_plan_id TEXT;
    guarded_state TEXT;
BEGIN
    guarded_plan_id := CASE WHEN TG_OP = 'DELETE' THEN OLD.plan_id ELSE NEW.plan_id END;

    IF TG_TABLE_NAME = 'object_plans' OR TG_TABLE_NAME = 'fetch_plans' THEN
        guarded_state := OLD.plan_state;
    ELSIF TG_TABLE_NAME = 'object_plan_items' OR TG_TABLE_NAME = 'upload_capabilities' THEN
        SELECT plan_state INTO guarded_state
        FROM object_plans
        WHERE plan_id = guarded_plan_id;
    ELSE
        SELECT plan_state INTO guarded_state
        FROM fetch_plans
        WHERE plan_id = guarded_plan_id;
    END IF;

    IF guarded_state = 'finalized' THEN
        RAISE EXCEPTION USING
            ERRCODE = '55000',
            MESSAGE = format('finalized plan %s is immutable', guarded_plan_id);
    END IF;

    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END
$plan_guard$;

CREATE FUNCTION layrs_validate_object_plan_bijection()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $object_plan_bijection$
DECLARE
    checked_plan_id TEXT;
    checked_state TEXT;
BEGIN
    checked_plan_id := CASE WHEN TG_OP = 'DELETE' THEN OLD.plan_id ELSE NEW.plan_id END;
    SELECT plan_state INTO checked_state FROM object_plans WHERE plan_id = checked_plan_id;

    IF checked_state = 'finalized' AND EXISTS (
        SELECT 1
        FROM object_plan_items AS item
        LEFT JOIN upload_capabilities AS capability
          ON capability.plan_id = item.plan_id
         AND capability.item_ordinal = item.ordinal
        WHERE item.plan_id = checked_plan_id
          AND item.disposition = 'missing'
        GROUP BY item.ordinal
        HAVING count(capability.capability_id) <> 1
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = format(
                'finalized object plan %s requires exactly one capability for every missing item',
                checked_plan_id
            );
    END IF;

    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END
$object_plan_bijection$;

CREATE FUNCTION layrs_validate_fetch_plan_bijection()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $fetch_plan_bijection$
DECLARE
    checked_plan_id TEXT;
    checked_state TEXT;
BEGIN
    checked_plan_id := CASE WHEN TG_OP = 'DELETE' THEN OLD.plan_id ELSE NEW.plan_id END;
    SELECT plan_state INTO checked_state FROM fetch_plans WHERE plan_id = checked_plan_id;

    IF checked_state = 'finalized' AND EXISTS (
        SELECT 1
        FROM fetch_plan_claims AS claim
        LEFT JOIN fetch_claim_objects AS expected
          ON expected.plan_id = claim.plan_id
         AND expected.claim_ordinal = claim.ordinal
        WHERE claim.plan_id = checked_plan_id
          AND claim.decision = 'allowed'
        GROUP BY claim.ordinal
        HAVING count(expected.object_id) = 0
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = format(
                'finalized fetch plan %s requires at least one expected object for every allowed claim',
                checked_plan_id
            );
    END IF;

    IF checked_state = 'finalized' AND EXISTS (
        SELECT 1
        FROM fetch_claim_objects AS expected
        LEFT JOIN fetch_capabilities AS capability
          ON capability.plan_id = expected.plan_id
         AND capability.claim_ordinal = expected.claim_ordinal
         AND capability.object_id = expected.object_id
        WHERE expected.plan_id = checked_plan_id
        GROUP BY expected.claim_ordinal, expected.object_id
        HAVING count(capability.capability_id) <> 1
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = format(
                'finalized fetch plan %s requires exactly one capability for every expected object',
                checked_plan_id
            );
    END IF;

    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END
$fetch_plan_bijection$;

CREATE TRIGGER object_plans_immutable_after_finalization
    BEFORE UPDATE OR DELETE ON object_plans
    FOR EACH ROW EXECUTE FUNCTION layrs_reject_finalized_plan_mutation();
CREATE TRIGGER object_plan_items_immutable_after_finalization
    BEFORE INSERT OR UPDATE OR DELETE ON object_plan_items
    FOR EACH ROW EXECUTE FUNCTION layrs_reject_finalized_plan_mutation();
CREATE TRIGGER upload_capabilities_immutable_after_finalization
    BEFORE INSERT OR UPDATE OR DELETE ON upload_capabilities
    FOR EACH ROW EXECUTE FUNCTION layrs_reject_finalized_plan_mutation();

CREATE CONSTRAINT TRIGGER object_plans_exact_capability_bijection
    AFTER INSERT OR UPDATE ON object_plans
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION layrs_validate_object_plan_bijection();
CREATE CONSTRAINT TRIGGER object_plan_items_exact_capability_bijection
    AFTER INSERT OR UPDATE OR DELETE ON object_plan_items
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION layrs_validate_object_plan_bijection();
CREATE CONSTRAINT TRIGGER upload_capabilities_exact_bijection
    AFTER INSERT OR UPDATE OR DELETE ON upload_capabilities
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION layrs_validate_object_plan_bijection();

CREATE TRIGGER fetch_plans_immutable_after_finalization
    BEFORE UPDATE OR DELETE ON fetch_plans
    FOR EACH ROW EXECUTE FUNCTION layrs_reject_finalized_plan_mutation();
CREATE TRIGGER fetch_plan_claims_immutable_after_finalization
    BEFORE INSERT OR UPDATE OR DELETE ON fetch_plan_claims
    FOR EACH ROW EXECUTE FUNCTION layrs_reject_finalized_plan_mutation();
CREATE TRIGGER fetch_claim_objects_immutable_after_finalization
    BEFORE INSERT OR UPDATE OR DELETE ON fetch_claim_objects
    FOR EACH ROW EXECUTE FUNCTION layrs_reject_finalized_plan_mutation();
CREATE TRIGGER fetch_capabilities_immutable_after_finalization
    BEFORE INSERT OR UPDATE OR DELETE ON fetch_capabilities
    FOR EACH ROW EXECUTE FUNCTION layrs_reject_finalized_plan_mutation();

CREATE CONSTRAINT TRIGGER fetch_plans_exact_capability_bijection
    AFTER INSERT OR UPDATE ON fetch_plans
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION layrs_validate_fetch_plan_bijection();
CREATE CONSTRAINT TRIGGER fetch_plan_claims_exact_capability_bijection
    AFTER INSERT OR UPDATE OR DELETE ON fetch_plan_claims
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION layrs_validate_fetch_plan_bijection();
CREATE CONSTRAINT TRIGGER fetch_claim_objects_exact_capability_bijection
    AFTER INSERT OR UPDATE OR DELETE ON fetch_claim_objects
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION layrs_validate_fetch_plan_bijection();
CREATE CONSTRAINT TRIGGER fetch_capabilities_exact_bijection
    AFTER INSERT OR UPDATE OR DELETE ON fetch_capabilities
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION layrs_validate_fetch_plan_bijection();

CREATE TABLE object_leases (
    lease_id TEXT PRIMARY KEY CHECK (lease_id LIKE 'lease:%'),
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    object_id TEXT NOT NULL CHECK (object_id ~ '^blake3:[0-9a-f]{64}$'),
    lease_purpose TEXT NOT NULL CHECK (lease_purpose IN ('upload', 'fetch', 'transaction', 'backup')),
    owner_principal_id TEXT REFERENCES principals(principal_id) ON DELETE RESTRICT,
    owner_client_id TEXT,
    fencing_generation BIGINT NOT NULL CHECK (fencing_generation > 0),
    expires_at TIMESTAMPTZ NOT NULL,
    heartbeat_at TIMESTAMPTZ NOT NULL,
    released_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    CHECK ((owner_principal_id IS NOT NULL)::INTEGER + (owner_client_id IS NOT NULL)::INTEGER >= 1)
);

CREATE UNIQUE INDEX object_leases_one_live_owner
    ON object_leases (space_id, object_id, lease_purpose)
    WHERE released_at IS NULL;
CREATE INDEX object_leases_expiry
    ON object_leases (expires_at) WHERE released_at IS NULL;

-- Runtime schema integrity is derived from PostgreSQL's normalized catalogs,
-- not from a hand-maintained table list. Every user routine body is included
-- exactly once, independently from the triggers that reference it.
CREATE FUNCTION layrs_schema_catalog_fingerprint()
RETURNS TEXT
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path FROM CURRENT
AS $catalog_fingerprint$
WITH target_namespace AS (
    SELECT namespace.oid
    FROM pg_catalog.pg_namespace AS namespace
    JOIN schema_metadata AS metadata
      ON metadata.singleton_id = 1
     AND namespace.nspname = metadata.installed_schema
),
catalog_items AS (
    SELECT
        'relation'::TEXT AS object_kind,
        value.relname::TEXT AS object_identity,
        format(
            'relkind=%s;persistence=%s;view=%s',
            value.relkind,
            value.relpersistence,
            CASE
                WHEN value.relkind IN ('v', 'm')
                    THEN pg_catalog.pg_get_viewdef(value.oid, true)
                ELSE ''
            END
        )::TEXT AS definition
    FROM pg_catalog.pg_class AS value
    JOIN target_namespace AS namespace ON namespace.oid = value.relnamespace
    WHERE value.relkind IN ('r', 'p', 'v', 'm', 'S', 'f', 'c')

    UNION ALL

    SELECT
        'column',
        format('%s.%s.%s', relation.relname, attribute.attnum, attribute.attname),
        format(
            'type=%s;notnull=%s;identity=%s;generated=%s;default=%s',
            pg_catalog.format_type(attribute.atttypid, attribute.atttypmod),
            attribute.attnotnull,
            attribute.attidentity,
            attribute.attgenerated,
            COALESCE(pg_catalog.pg_get_expr(default_value.adbin, default_value.adrelid, true), '')
        )
    FROM pg_catalog.pg_attribute AS attribute
    JOIN pg_catalog.pg_class AS relation ON relation.oid = attribute.attrelid
    JOIN target_namespace AS namespace ON namespace.oid = relation.relnamespace
    LEFT JOIN pg_catalog.pg_attrdef AS default_value
        ON default_value.adrelid = attribute.attrelid
       AND default_value.adnum = attribute.attnum
    WHERE relation.relkind IN ('r', 'p', 'v', 'm', 'S', 'f', 'c')
      AND attribute.attnum > 0
      AND NOT attribute.attisdropped

    UNION ALL

    SELECT
        'constraint',
        format('%s.%s', relation.relname, constraint_value.conname),
        format(
            'type=%s;deferrable=%s;deferred=%s;validated=%s;definition=%s',
            constraint_value.contype,
            constraint_value.condeferrable,
            constraint_value.condeferred,
            constraint_value.convalidated,
            pg_catalog.pg_get_constraintdef(constraint_value.oid, true)
        )
    FROM pg_catalog.pg_constraint AS constraint_value
    JOIN pg_catalog.pg_class AS relation ON relation.oid = constraint_value.conrelid
    JOIN target_namespace AS namespace ON namespace.oid = relation.relnamespace
    WHERE constraint_value.contype IN ('p', 'u', 'f', 'c', 'x')

    UNION ALL

    SELECT
        'index',
        format('%s.%s', relation.relname, index_relation.relname),
        format(
            'unique=%s;primary=%s;valid=%s;ready=%s;definition=%s',
            index_value.indisunique,
            index_value.indisprimary,
            index_value.indisvalid,
            index_value.indisready,
            pg_catalog.pg_get_indexdef(index_value.indexrelid, 0, true)
        )
    FROM pg_catalog.pg_index AS index_value
    JOIN pg_catalog.pg_class AS relation ON relation.oid = index_value.indrelid
    JOIN pg_catalog.pg_class AS index_relation ON index_relation.oid = index_value.indexrelid
    JOIN target_namespace AS namespace ON namespace.oid = relation.relnamespace
    WHERE relation.relkind IN ('r', 'p', 'm')

    UNION ALL

    SELECT
        'trigger',
        format('%s.%s', relation.relname, trigger_value.tgname),
        format(
            'enabled=%s;deferrable=%s;deferred=%s;definition=%s',
            trigger_value.tgenabled,
            trigger_value.tgdeferrable,
            trigger_value.tginitdeferred,
            pg_catalog.pg_get_triggerdef(trigger_value.oid, true)
        )
    FROM pg_catalog.pg_trigger AS trigger_value
    JOIN pg_catalog.pg_class AS relation ON relation.oid = trigger_value.tgrelid
    JOIN target_namespace AS namespace ON namespace.oid = relation.relnamespace
    WHERE NOT trigger_value.tgisinternal

    UNION ALL

    SELECT
        'routine',
        format(
            '%s(%s)',
            routine.proname,
            pg_catalog.pg_get_function_identity_arguments(routine.oid)
        ),
        CASE
            WHEN routine.prokind IN ('f', 'p', 'w')
                THEN pg_catalog.pg_get_functiondef(routine.oid)
            ELSE format(
                'kind=%s;language=%s;source=%s;binary=%s',
                routine.prokind,
                language.lanname,
                routine.prosrc,
                COALESCE(routine.probin, '')
            )
        END
    FROM pg_catalog.pg_proc AS routine
    JOIN pg_catalog.pg_language AS language ON language.oid = routine.prolang
    JOIN target_namespace AS namespace ON namespace.oid = routine.pronamespace

    UNION ALL

    SELECT
        'type',
        type_value.typname,
        format(
            'kind=%s;category=%s;notnull=%s;base=%s;typmod=%s;collation=%s;default=%s;enum=%s;constraints=%s;range=%s',
            type_value.typtype,
            type_value.typcategory,
            type_value.typnotnull,
            type_value.typbasetype,
            type_value.typtypmod,
            type_value.typcollation,
            COALESCE(type_value.typdefault, ''),
            COALESCE((
                SELECT pg_catalog.string_agg(
                    enum_value.enumlabel,
                    E'\x1f' ORDER BY enum_value.enumsortorder
                )
                FROM pg_catalog.pg_enum AS enum_value
                WHERE enum_value.enumtypid = type_value.oid
            ), ''),
            COALESCE((
                SELECT pg_catalog.string_agg(
                    pg_catalog.pg_get_constraintdef(constraint_value.oid, true),
                    E'\x1f' ORDER BY constraint_value.conname COLLATE "C"
                )
                FROM pg_catalog.pg_constraint AS constraint_value
                WHERE constraint_value.contypid = type_value.oid
            ), ''),
            COALESCE((
                SELECT format(
                    'subtype=%s;canonical=%s;subdiff=%s',
                    pg_catalog.format_type(range_value.rngsubtype, NULL),
                    range_value.rngcanonical,
                    range_value.rngsubdiff
                )
                FROM pg_catalog.pg_range AS range_value
                WHERE range_value.rngtypid = type_value.oid
            ), '')
        )
    FROM pg_catalog.pg_type AS type_value
    JOIN target_namespace AS namespace ON namespace.oid = type_value.typnamespace
    WHERE type_value.typrelid = 0
      AND NOT EXISTS (
          SELECT 1 FROM pg_catalog.pg_type AS element
          WHERE element.typarray = type_value.oid
      )
      AND NOT EXISTS (
          SELECT 1 FROM pg_catalog.pg_range AS range_value
          WHERE range_value.rngmultitypid = type_value.oid
      )

    UNION ALL

    SELECT
        'collation',
        collation_value.collname,
        format(
            'provider=%s;deterministic=%s;encoding=%s;collate=%s;ctype=%s;version=%s',
            collation_value.collprovider,
            collation_value.collisdeterministic,
            collation_value.collencoding,
            COALESCE(collation_value.collcollate, ''),
            COALESCE(collation_value.collctype, ''),
            COALESCE(collation_value.collversion, '')
        )
    FROM pg_catalog.pg_collation AS collation_value
    JOIN target_namespace AS namespace ON namespace.oid = collation_value.collnamespace

    UNION ALL

    SELECT
        'sequence',
        relation.relname,
        format(
            'type=%s;start=%s;increment=%s;max=%s;min=%s;cache=%s;cycle=%s',
            pg_catalog.format_type(sequence_value.seqtypid, NULL),
            sequence_value.seqstart,
            sequence_value.seqincrement,
            sequence_value.seqmax,
            sequence_value.seqmin,
            sequence_value.seqcache,
            sequence_value.seqcycle
        )
    FROM pg_catalog.pg_sequence AS sequence_value
    JOIN pg_catalog.pg_class AS relation ON relation.oid = sequence_value.seqrelid
    JOIN target_namespace AS namespace ON namespace.oid = relation.relnamespace

    UNION ALL

    SELECT
        'foreign_table',
        relation.relname,
        format(
            'server=%s;options=%s',
            server.srvname,
            COALESCE(pg_catalog.array_to_string(foreign_value.ftoptions, E'\x1f'), '')
        )
    FROM pg_catalog.pg_foreign_table AS foreign_value
    JOIN pg_catalog.pg_class AS relation ON relation.oid = foreign_value.ftrelid
    JOIN pg_catalog.pg_foreign_server AS server ON server.oid = foreign_value.ftserver
    JOIN target_namespace AS namespace ON namespace.oid = relation.relnamespace

    UNION ALL

    SELECT
        'conversion',
        conversion_value.conname,
        format(
            'source=%s;target=%s;procedure=%s;default=%s',
            pg_catalog.pg_encoding_to_char(conversion_value.conforencoding),
            pg_catalog.pg_encoding_to_char(conversion_value.contoencoding),
            conversion_value.conproc::REGPROCEDURE,
            conversion_value.condefault
        )
    FROM pg_catalog.pg_conversion AS conversion_value
    JOIN target_namespace AS namespace ON namespace.oid = conversion_value.connamespace

    UNION ALL

    SELECT
        'operator',
        format(
            '%s(%s,%s)',
            operator_value.oprname,
            pg_catalog.format_type(operator_value.oprleft, NULL),
            pg_catalog.format_type(operator_value.oprright, NULL)
        ),
        format(
            'kind=%s;result=%s;code=%s;merge=%s;hash=%s;restrict=%s;join=%s',
            operator_value.oprkind,
            pg_catalog.format_type(operator_value.oprresult, NULL),
            operator_value.oprcode::REGPROCEDURE,
            operator_value.oprcanmerge,
            operator_value.oprcanhash,
            operator_value.oprrest::REGPROCEDURE,
            operator_value.oprjoin::REGPROCEDURE
        )
    FROM pg_catalog.pg_operator AS operator_value
    JOIN target_namespace AS namespace ON namespace.oid = operator_value.oprnamespace

    UNION ALL

    SELECT
        'operator_class',
        operator_class.opcname,
        format(
            'method=%s;family=%s;input=%s;key=%s;default=%s',
            access_method.amname,
            operator_family.opfname,
            pg_catalog.format_type(operator_class.opcintype, NULL),
            pg_catalog.format_type(operator_class.opckeytype, NULL),
            operator_class.opcdefault
        )
    FROM pg_catalog.pg_opclass AS operator_class
    JOIN pg_catalog.pg_am AS access_method ON access_method.oid = operator_class.opcmethod
    JOIN pg_catalog.pg_opfamily AS operator_family ON operator_family.oid = operator_class.opcfamily
    JOIN target_namespace AS namespace ON namespace.oid = operator_class.opcnamespace

    UNION ALL

    SELECT
        'operator_family',
        operator_family.opfname,
        format('method=%s', access_method.amname)
    FROM pg_catalog.pg_opfamily AS operator_family
    JOIN pg_catalog.pg_am AS access_method ON access_method.oid = operator_family.opfmethod
    JOIN target_namespace AS namespace ON namespace.oid = operator_family.opfnamespace

    UNION ALL

    SELECT
        'text_search_configuration',
        configuration.cfgname,
        format(
            'parser=%s;mappings=%s',
            parser.prsname,
            COALESCE((
                SELECT pg_catalog.string_agg(
                    format('%s:%s:%s', mapping.maptokentype, mapping.mapseqno, dictionary.dictname),
                    E'\x1f' ORDER BY mapping.maptokentype, mapping.mapseqno
                )
                FROM pg_catalog.pg_ts_config_map AS mapping
                JOIN pg_catalog.pg_ts_dict AS dictionary ON dictionary.oid = mapping.mapdict
                WHERE mapping.mapcfg = configuration.oid
            ), '')
        )
    FROM pg_catalog.pg_ts_config AS configuration
    JOIN pg_catalog.pg_ts_parser AS parser ON parser.oid = configuration.cfgparser
    JOIN target_namespace AS namespace ON namespace.oid = configuration.cfgnamespace

    UNION ALL

    SELECT
        'text_search_dictionary',
        dictionary.dictname,
        format(
            'template=%s;options=%s',
            template.tmplname,
            COALESCE(dictionary.dictinitoption, '')
        )
    FROM pg_catalog.pg_ts_dict AS dictionary
    JOIN pg_catalog.pg_ts_template AS template ON template.oid = dictionary.dicttemplate
    JOIN target_namespace AS namespace ON namespace.oid = dictionary.dictnamespace

    UNION ALL

    SELECT
        'text_search_parser',
        parser.prsname,
        format(
            'start=%s;next=%s;end=%s;headline=%s;lextype=%s',
            parser.prsstart::REGPROCEDURE,
            parser.prstoken::REGPROCEDURE,
            parser.prsend::REGPROCEDURE,
            parser.prsheadline::REGPROCEDURE,
            parser.prslextype::REGPROCEDURE
        )
    FROM pg_catalog.pg_ts_parser AS parser
    JOIN target_namespace AS namespace ON namespace.oid = parser.prsnamespace

    UNION ALL

    SELECT
        'text_search_template',
        template.tmplname,
        format(
            'init=%s;lexize=%s',
            template.tmplinit::REGPROCEDURE,
            template.tmpllexize::REGPROCEDURE
        )
    FROM pg_catalog.pg_ts_template AS template
    JOIN target_namespace AS namespace ON namespace.oid = template.tmplnamespace

    UNION ALL

    SELECT
        'statistics',
        statistics.stxname,
        pg_catalog.pg_get_statisticsobjdef(statistics.oid)
    FROM pg_catalog.pg_statistic_ext AS statistics
    JOIN target_namespace AS namespace ON namespace.oid = statistics.stxnamespace
)
SELECT 'md5:' || pg_catalog.md5(
    COALESCE(
        pg_catalog.string_agg(
            object_kind || E'\x1f' || object_identity || E'\x1f' || definition,
            E'\x1e'
            ORDER BY
                object_kind COLLATE "C",
                object_identity COLLATE "C",
                definition COLLATE "C"
        ),
        ''
    )
)
FROM catalog_items
$catalog_fingerprint$;

UPDATE schema_metadata
SET catalog_fingerprint = layrs_schema_catalog_fingerprint()
WHERE singleton_id = 1;

COMMENT ON COLUMN stores.backend_config_jcs IS 'RFC 8785 JCS; secrets are referenced, never embedded.';
COMMENT ON COLUMN canonical_objects.storage_key IS 'Opaque CAS locator; PostgreSQL stores descriptors, never object bytes.';
COMMENT ON COLUMN file_manifests.chunker_parameters_jcs IS 'RFC 8785 JCS identity/projection bytes.';
COMMENT ON COLUMN steps.canonical_record_jcs IS 'RFC 8785 JCS representation of the admitted canonical Step record.';
COMMENT ON COLUMN products.canonical_envelope_jcs IS 'RFC 8785 JCS product envelope, byte-for-byte identity evidence.';
COMMENT ON COLUMN repository_views.canonical_record_jcs IS 'RFC 8785 JCS immutable View record; spaces owns the only mutable pointer.';
COMMENT ON COLUMN policy_sets.canonical_record_jcs IS 'RFC 8785 JCS layrs.policy-set.v1 record.';
COMMENT ON COLUMN authorization_snapshots.canonical_record_jcs IS 'RFC 8785 JCS server-generated authorization evidence.';
COMMENT ON COLUMN sync_transactions.receipt_canonical_jcs IS 'Stored canonical receipt returned byte-equivalently on idempotent replay.';
COMMENT ON COLUMN space_events.payload_canonical_jcs IS 'RFC 8785 JCS payload from the closed layrs.sync.v3 event union.';

COMMIT;
