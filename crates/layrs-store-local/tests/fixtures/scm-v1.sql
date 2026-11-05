CREATE TABLE meta (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    magic TEXT NOT NULL CHECK (length(magic) > 0),
    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
    schema_checksum TEXT NOT NULL CHECK (length(schema_checksum) = 64),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0)
) STRICT;

CREATE TABLE migrations (
    version INTEGER PRIMARY KEY CHECK (version >= 1),
    name TEXT NOT NULL UNIQUE CHECK (length(name) > 0),
    checksum TEXT NOT NULL CHECK (length(checksum) = 64),
    applied_at_ms INTEGER NOT NULL CHECK (applied_at_ms >= 0)
) STRICT;

CREATE TABLE applied_transactions (
    transaction_id TEXT PRIMARY KEY CHECK (length(transaction_id) > 0),
    request_digest TEXT NOT NULL CHECK (length(request_digest) = 71),
    -- Complete ApplyBatch replay receipt encoded as RFC 8785 JCS. SQLite
    -- enforces valid JSON; the repository write boundary enforces canonical
    -- bytes before insertion and again before replay.
    receipt_json TEXT NOT NULL CHECK (json_valid(receipt_json)),
    applied_at_ms INTEGER NOT NULL CHECK (applied_at_ms >= 0)
) STRICT;

CREATE TABLE stores (
    store_id TEXT PRIMARY KEY CHECK (length(store_id) > 0 AND instr(store_id, char(0)) = 0),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0)
) STRICT;

CREATE TABLE spaces (
    space_id TEXT PRIMARY KEY CHECK (length(space_id) > 0 AND instr(space_id, char(0)) = 0),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0)
) STRICT;

CREATE TABLE layers (
    layer_id TEXT PRIMARY KEY CHECK (length(layer_id) > 0 AND instr(layer_id, char(0)) = 0),
    space_id TEXT NOT NULL,
    layer_kind TEXT NOT NULL CHECK (layer_kind IN (
        'base', 'exploration', 'proposal', 'automated_result',
        'release_candidate', 'published'
    )),
    layer_status TEXT NOT NULL CHECK (layer_status IN ('draft', 'active', 'archived', 'superseded')),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    UNIQUE (layer_id, space_id),
    FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX layers_space_idx ON layers(space_id, layer_id);

CREATE TABLE layer_parents (
    layer_id TEXT NOT NULL,
    parent_layer_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    PRIMARY KEY (layer_id, ordinal),
    UNIQUE (layer_id, parent_layer_id),
    CHECK (layer_id <> parent_layer_id),
    FOREIGN KEY (layer_id) REFERENCES layers(layer_id) ON DELETE CASCADE,
    FOREIGN KEY (parent_layer_id) REFERENCES layers(layer_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE INDEX layer_parents_parent_idx ON layer_parents(parent_layer_id);

CREATE TABLE objects (
    object_id TEXT PRIMARY KEY CHECK (length(object_id) > 0 AND instr(object_id, char(0)) = 0),
    object_kind TEXT NOT NULL CHECK (object_kind IN (
        'chunk_raw', 'file_manifest', 'tree', 'symlink', 'tombstone',
        'step', 'repository_view', 'conflict'
    )),
    storage_path TEXT NOT NULL UNIQUE CHECK (length(storage_path) > 0 AND instr(storage_path, char(0)) = 0),
    byte_length INTEGER NOT NULL CHECK (byte_length > 0),
    verified INTEGER NOT NULL CHECK (verified IN (0, 1)),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0)
) STRICT;

CREATE INDEX objects_kind_idx ON objects(object_kind, object_id);

CREATE TABLE object_edges (
    source_object_id TEXT NOT NULL,
    target_object_id TEXT NOT NULL,
    edge_kind TEXT NOT NULL CHECK (edge_kind IN (
        'manifest_chunk', 'tree_entry', 'step_parent', 'step_tree', 'view_step',
        'view_marker', 'view_release', 'view_worktree', 'conflict_side'
    )),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    PRIMARY KEY (source_object_id, edge_kind, ordinal),
    FOREIGN KEY (source_object_id) REFERENCES objects(object_id) ON DELETE CASCADE,
    FOREIGN KEY (target_object_id) REFERENCES objects(object_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE INDEX object_edges_target_idx ON object_edges(target_object_id, edge_kind);

CREATE TABLE file_availability (
    file_id TEXT PRIMARY KEY CHECK (length(file_id) > 0 AND instr(file_id, char(0)) = 0),
    availability TEXT NOT NULL DEFAULT 'present' CHECK (availability = 'present'),
    verified_at_ms INTEGER NOT NULL CHECK (verified_at_ms >= 0)
) STRICT;

CREATE TABLE file_manifests (
    manifest_id TEXT PRIMARY KEY,
    file_id TEXT NOT NULL,
    file_byte_length INTEGER NOT NULL CHECK (file_byte_length >= 0),
    chunking_json TEXT NOT NULL CHECK (json_valid(chunking_json)),
    FOREIGN KEY (manifest_id) REFERENCES objects(object_id) ON DELETE CASCADE,
    FOREIGN KEY (file_id) REFERENCES file_availability(file_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX file_manifests_file_idx ON file_manifests(file_id, manifest_id);

CREATE TABLE file_manifest_chunks (
    manifest_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    chunk_id TEXT NOT NULL,
    byte_offset INTEGER NOT NULL CHECK (byte_offset >= 0),
    byte_length INTEGER NOT NULL CHECK (byte_length > 0),
    PRIMARY KEY (manifest_id, ordinal),
    UNIQUE (manifest_id, byte_offset),
    FOREIGN KEY (manifest_id) REFERENCES file_manifests(manifest_id) ON DELETE CASCADE,
    FOREIGN KEY (chunk_id) REFERENCES objects(object_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX file_manifest_chunks_chunk_idx ON file_manifest_chunks(chunk_id);

CREATE TABLE space_object_admissions (
    space_id TEXT NOT NULL,
    object_id TEXT NOT NULL,
    admitted_by_operation_id TEXT,
    admitted_at_ms INTEGER NOT NULL CHECK (admitted_at_ms >= 0),
    PRIMARY KEY (space_id, object_id),
    FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE CASCADE,
    FOREIGN KEY (object_id) REFERENCES objects(object_id) ON DELETE RESTRICT,
    FOREIGN KEY (admitted_by_operation_id) REFERENCES operations(operation_id) ON DELETE SET NULL
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE INDEX admissions_object_idx ON space_object_admissions(object_id, space_id);

CREATE TABLE pending_objects (
    transaction_id TEXT NOT NULL,
    object_id TEXT NOT NULL,
    object_kind TEXT NOT NULL CHECK (object_kind IN (
        'chunk_raw', 'file_manifest', 'tree', 'symlink', 'tombstone',
        'step', 'repository_view', 'conflict'
    )),
    staging_path TEXT NOT NULL CHECK (length(staging_path) > 0),
    state TEXT NOT NULL CHECK (state IN ('staged', 'verified', 'installed', 'abandoned')),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    PRIMARY KEY (transaction_id, object_id)
) STRICT;

CREATE INDEX pending_objects_state_idx ON pending_objects(state, created_at_ms);

CREATE TABLE pending_admissions (
    transaction_id TEXT NOT NULL,
    space_id TEXT NOT NULL,
    object_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'validated', 'applied', 'rejected')),
    PRIMARY KEY (transaction_id, space_id, object_id),
    FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE steps (
    step_id TEXT PRIMARY KEY,
    root_tree_id TEXT NOT NULL,
    parent_count INTEGER NOT NULL CHECK (parent_count >= 0),
    FOREIGN KEY (step_id) REFERENCES objects(object_id) ON DELETE CASCADE,
    FOREIGN KEY (root_tree_id) REFERENCES objects(object_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX steps_root_tree_idx ON steps(root_tree_id);

CREATE TABLE step_parents (
    step_id TEXT NOT NULL,
    parent_step_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    PRIMARY KEY (step_id, ordinal),
    UNIQUE (step_id, parent_step_id),
    CHECK (step_id <> parent_step_id),
    FOREIGN KEY (step_id) REFERENCES steps(step_id) ON DELETE CASCADE,
    FOREIGN KEY (parent_step_id) REFERENCES steps(step_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE INDEX step_parents_parent_idx ON step_parents(parent_step_id);

CREATE TABLE repository_views (
    repository_view_id TEXT PRIMARY KEY,
    space_id TEXT NOT NULL,
    FOREIGN KEY (repository_view_id) REFERENCES objects(object_id) ON DELETE CASCADE,
    FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX repository_views_space_idx ON repository_views(space_id, repository_view_id);

CREATE TABLE repository_view_layer_heads (
    repository_view_id TEXT NOT NULL,
    layer_id TEXT NOT NULL,
    step_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    PRIMARY KEY (repository_view_id, layer_id, ordinal),
    UNIQUE (repository_view_id, layer_id, step_id),
    FOREIGN KEY (repository_view_id) REFERENCES repository_views(repository_view_id) ON DELETE CASCADE,
    FOREIGN KEY (layer_id) REFERENCES layers(layer_id) ON DELETE RESTRICT,
    FOREIGN KEY (step_id) REFERENCES steps(step_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX repository_view_heads_step_idx ON repository_view_layer_heads(step_id);

CREATE TABLE repository_view_markers (
    repository_view_id TEXT NOT NULL,
    marker_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    PRIMARY KEY (repository_view_id, ordinal),
    UNIQUE (repository_view_id, marker_id),
    FOREIGN KEY (repository_view_id) REFERENCES repository_views(repository_view_id) ON DELETE CASCADE,
    FOREIGN KEY (marker_id) REFERENCES markers(marker_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE INDEX repository_view_markers_marker_idx ON repository_view_markers(marker_id);

CREATE TABLE repository_view_releases (
    repository_view_id TEXT NOT NULL,
    release_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    PRIMARY KEY (repository_view_id, ordinal),
    UNIQUE (repository_view_id, release_id),
    FOREIGN KEY (repository_view_id) REFERENCES repository_views(repository_view_id) ON DELETE CASCADE,
    FOREIGN KEY (release_id) REFERENCES releases(release_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE INDEX repository_view_releases_release_idx ON repository_view_releases(release_id);

CREATE TABLE repository_view_worktrees (
    repository_view_id TEXT NOT NULL,
    worktree_id TEXT NOT NULL,
    active_layer_id TEXT,
    base_step_id TEXT,
    materialized_tree_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('ready', 'conflicted', 'needs_repair', 'stale')),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    PRIMARY KEY (repository_view_id, ordinal),
    UNIQUE (repository_view_id, worktree_id),
    FOREIGN KEY (repository_view_id) REFERENCES repository_views(repository_view_id) ON DELETE CASCADE,
    FOREIGN KEY (worktree_id) REFERENCES worktrees(worktree_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (active_layer_id) REFERENCES layers(layer_id) ON DELETE RESTRICT,
    FOREIGN KEY (base_step_id) REFERENCES steps(step_id) ON DELETE RESTRICT,
    FOREIGN KEY (materialized_tree_id) REFERENCES objects(object_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX repository_view_worktrees_worktree_idx ON repository_view_worktrees(worktree_id);

CREATE TABLE step_observations (
    observation_id TEXT PRIMARY KEY CHECK (length(observation_id) > 0),
    step_id TEXT NOT NULL,
    space_id TEXT,
    operation_id TEXT,
    worktree_id TEXT,
    actor_id TEXT,
    device_id TEXT,
    observed_at TEXT NOT NULL,
    capture_reason TEXT NOT NULL CHECK (capture_reason IN (
        'automatic_idle', 'before_layer_switch', 'before_worktree_switch',
        'before_operation', 'manual_snapshot', 'weave_result',
        'conflict_resolution', 'rewrite', 'received', 'automation'
    )),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    FOREIGN KEY (step_id) REFERENCES steps(step_id) ON DELETE RESTRICT,
    FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE CASCADE,
    FOREIGN KEY (operation_id) REFERENCES operations(operation_id) ON DELETE SET NULL
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (worktree_id) REFERENCES worktrees(worktree_id) ON DELETE SET NULL
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE INDEX step_observations_step_idx ON step_observations(step_id, observed_at);
CREATE INDEX step_observations_space_idx ON step_observations(space_id, observed_at);

CREATE TABLE operations (
    operation_id TEXT PRIMARY KEY CHECK (length(operation_id) > 0),
    space_id TEXT NOT NULL,
    resulting_view_id TEXT NOT NULL,
    operation_kind TEXT NOT NULL CHECK (operation_kind IN (
        'initialize', 'capture_worktree', 'switch_layer', 'move_layer_heads',
        'create_marker', 'hide_marker', 'create_release', 'weave',
        'resolve_conflict', 'rewrite_steps', 'receive_remote_state',
        'integrate_operations', 'undo', 'redo', 'restore', 'revert', 'prune'
    )),
    actor_id TEXT,
    device_id TEXT,
    created_at TEXT NOT NULL,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)) CHECK (
        json_type(payload_json, '$') = 'object'
        AND json_extract(payload_json, '$.schemaVersion') IS 'layrs.operation.v1'
        AND operation_id IS json_extract(payload_json, '$.operationId')
        AND space_id IS json_extract(payload_json, '$.spaceId')
        AND resulting_view_id IS json_extract(payload_json, '$.resultingViewId')
        AND operation_kind IS json_extract(payload_json, '$.kind')
        AND COALESCE(json_type(payload_json, '$.actorId') IN ('text', 'null'), 0)
        AND actor_id IS json_extract(payload_json, '$.actorId')
        AND COALESCE(json_type(payload_json, '$.deviceId') IN ('text', 'null'), 0)
        AND device_id IS json_extract(payload_json, '$.deviceId')
        AND created_at IS json_extract(payload_json, '$.createdAt')
    ),
    FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE CASCADE,
    FOREIGN KEY (resulting_view_id) REFERENCES repository_views(repository_view_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX operations_space_created_idx ON operations(space_id, created_at, operation_id);

CREATE INDEX operations_payload_space_kind_idx ON operations(
    json_extract(payload_json, '$.spaceId'),
    json_extract(payload_json, '$.kind'),
    operation_id
);

CREATE INDEX operations_projection_space_payload_kind_idx ON operations(
    space_id,
    json_extract(payload_json, '$.kind'),
    operation_id
);

CREATE INDEX operations_payload_space_projection_kind_idx ON operations(
    json_extract(payload_json, '$.spaceId'),
    operation_kind,
    operation_id
);

CREATE UNIQUE INDEX operations_single_initialize_per_space_idx
ON operations(space_id) WHERE operation_kind = 'initialize';

CREATE UNIQUE INDEX operations_single_initialize_payload_space_idx
ON operations(json_extract(payload_json, '$.spaceId'))
WHERE json_extract(payload_json, '$.kind') = 'initialize';

CREATE TABLE operation_parents (
    operation_id TEXT NOT NULL,
    parent_operation_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    PRIMARY KEY (operation_id, ordinal),
    UNIQUE (operation_id, parent_operation_id),
    CHECK (operation_id <> parent_operation_id),
    FOREIGN KEY (operation_id) REFERENCES operations(operation_id) ON DELETE CASCADE,
    FOREIGN KEY (parent_operation_id) REFERENCES operations(operation_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE INDEX operation_parents_parent_idx ON operation_parents(parent_operation_id);

CREATE TABLE operation_rewrites (
    operation_id TEXT NOT NULL,
    old_step_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    PRIMARY KEY (operation_id, ordinal),
    UNIQUE (operation_id, old_step_id),
    FOREIGN KEY (operation_id) REFERENCES operations(operation_id) ON DELETE CASCADE,
    FOREIGN KEY (old_step_id) REFERENCES steps(step_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE operation_rewrite_targets (
    operation_id TEXT NOT NULL,
    old_step_id TEXT NOT NULL,
    new_step_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    PRIMARY KEY (operation_id, old_step_id, ordinal),
    UNIQUE (operation_id, old_step_id, new_step_id),
    FOREIGN KEY (operation_id, old_step_id)
        REFERENCES operation_rewrites(operation_id, old_step_id) ON DELETE CASCADE,
    FOREIGN KEY (new_step_id) REFERENCES steps(step_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE worktrees (
    worktree_id TEXT PRIMARY KEY CHECK (length(worktree_id) > 0),
    store_id TEXT NOT NULL,
    space_id TEXT NOT NULL,
    active_operation_id TEXT NOT NULL,
    checkout_root TEXT NOT NULL CHECK (length(checkout_root) > 0),
    filesystem_identity TEXT NOT NULL CHECK (length(filesystem_identity) > 0),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    UNIQUE (worktree_id, space_id),
    FOREIGN KEY (store_id) REFERENCES stores(store_id) ON DELETE RESTRICT,
    FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE CASCADE,
    FOREIGN KEY (active_operation_id) REFERENCES operations(operation_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE INDEX worktrees_space_idx ON worktrees(space_id, worktree_id);

CREATE TABLE markers (
    marker_id TEXT PRIMARY KEY CHECK (length(marker_id) > 0),
    space_id TEXT NOT NULL,
    target_step_id TEXT NOT NULL,
    marker_kind TEXT NOT NULL CHECK (marker_kind IN (
        'checkpoint', 'milestone', 'decision', 'baseline', 'custom'
    )),
    created_at TEXT NOT NULL,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE CASCADE,
    FOREIGN KEY (target_step_id) REFERENCES steps(step_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX markers_space_target_idx ON markers(space_id, target_step_id);

CREATE TABLE marker_proofs (
    marker_id TEXT NOT NULL,
    proof_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    PRIMARY KEY (marker_id, ordinal),
    UNIQUE (marker_id, proof_id),
    FOREIGN KEY (marker_id) REFERENCES markers(marker_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE releases (
    release_id TEXT PRIMARY KEY CHECK (length(release_id) > 0),
    space_id TEXT NOT NULL,
    target_step_id TEXT NOT NULL,
    version TEXT NOT NULL CHECK (length(version) > 0),
    created_at TEXT NOT NULL,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE CASCADE,
    FOREIGN KEY (target_step_id) REFERENCES steps(step_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX releases_space_target_idx ON releases(space_id, target_step_id);

CREATE TABLE release_proofs (
    release_id TEXT NOT NULL,
    proof_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    PRIMARY KEY (release_id, ordinal),
    UNIQUE (release_id, proof_id),
    FOREIGN KEY (release_id) REFERENCES releases(release_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE release_publications (
    release_publication_id TEXT PRIMARY KEY CHECK (length(release_publication_id) > 0),
    release_id TEXT NOT NULL,
    space_id TEXT NOT NULL,
    operation_id TEXT,
    published_at TEXT NOT NULL,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    FOREIGN KEY (release_id) REFERENCES releases(release_id) ON DELETE CASCADE,
    FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE CASCADE,
    FOREIGN KEY (operation_id) REFERENCES operations(operation_id) ON DELETE SET NULL
) STRICT;

CREATE INDEX release_publications_release_idx
ON release_publications(release_id, published_at);

CREATE TABLE current_repository_views (
    space_id TEXT PRIMARY KEY,
    generation INTEGER NOT NULL CHECK (generation >= 1),
    repository_view_id TEXT NOT NULL,
    updated_by_transaction_id TEXT NOT NULL,
    FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE CASCADE,
    FOREIGN KEY (repository_view_id) REFERENCES repository_views(repository_view_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE operation_heads (
    space_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    updated_by_transaction_id TEXT NOT NULL,
    PRIMARY KEY (space_id, ordinal),
    UNIQUE (space_id, operation_id),
    FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE CASCADE,
    FOREIGN KEY (operation_id) REFERENCES operations(operation_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX operation_heads_operation_idx ON operation_heads(operation_id);

CREATE TABLE recovery_operations (
    recovery_operation_id TEXT PRIMARY KEY CHECK (length(recovery_operation_id) > 0),
    idempotency_key TEXT NOT NULL UNIQUE CHECK (
        length(idempotency_key) > 0 AND instr(idempotency_key, char(0)) = 0
    ),
    request_digest TEXT NOT NULL CHECK (length(request_digest) > 0),
    worktree_id TEXT NOT NULL,
    operation_id TEXT,
    recovery_type TEXT NOT NULL CHECK (recovery_type IN (
        'materialize', 'restore', 'quarantine', 'cas_staging_cleanup'
    )),
    state TEXT NOT NULL CHECK (state IN (
        'pending', 'prepared', 'applying', 'completed', 'failed'
    )),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    evidence_json TEXT NOT NULL CHECK (json_valid(evidence_json)),
    attempt_count INTEGER NOT NULL CHECK (attempt_count >= 0),
    owner_token TEXT,
    last_error TEXT,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    completed_at_ms INTEGER,
    CHECK ((state = 'completed') = (completed_at_ms IS NOT NULL)),
    FOREIGN KEY (worktree_id) REFERENCES worktrees(worktree_id) ON DELETE RESTRICT,
    FOREIGN KEY (operation_id) REFERENCES operations(operation_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX recovery_operations_state_idx ON recovery_operations(state, updated_at_ms);
CREATE INDEX recovery_operations_worktree_idx
ON recovery_operations(worktree_id, state, updated_at_ms);

CREATE TABLE recovery_paths (
    recovery_operation_id TEXT NOT NULL,
    path TEXT NOT NULL CHECK (length(path) > 0 AND instr(path, char(0)) = 0),
    path_role TEXT NOT NULL CHECK (path_role IN (
        'source', 'destination', 'temporary', 'quarantine', 'materialized'
    )),
    expected_object_id TEXT,
    evidence_json TEXT NOT NULL CHECK (json_valid(evidence_json)),
    state TEXT NOT NULL CHECK (state IN (
        'pending', 'verified', 'applied', 'rolled_back', 'quarantined'
    )),
    PRIMARY KEY (recovery_operation_id, path, path_role),
    FOREIGN KEY (recovery_operation_id)
        REFERENCES recovery_operations(recovery_operation_id) ON DELETE CASCADE,
    FOREIGN KEY (expected_object_id) REFERENCES objects(object_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE materialization_leases (
    worktree_id TEXT NOT NULL,
    resource_key TEXT NOT NULL CHECK (
        length(resource_key) > 0 AND instr(resource_key, char(0)) = 0
    ),
    recovery_operation_id TEXT NOT NULL,
    owner_token TEXT NOT NULL CHECK (length(owner_token) > 0),
    generation INTEGER NOT NULL CHECK (generation > 0),
    acquired_at_ms INTEGER NOT NULL CHECK (acquired_at_ms >= 0),
    heartbeat_at_ms INTEGER NOT NULL CHECK (heartbeat_at_ms >= acquired_at_ms),
    expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms > acquired_at_ms),
    released_at_ms INTEGER,
    PRIMARY KEY (worktree_id, resource_key),
    CHECK (expires_at_ms > heartbeat_at_ms),
    CHECK (released_at_ms IS NULL OR released_at_ms >= heartbeat_at_ms),
    FOREIGN KEY (worktree_id) REFERENCES worktrees(worktree_id) ON DELETE CASCADE,
    FOREIGN KEY (recovery_operation_id)
        REFERENCES recovery_operations(recovery_operation_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX materialization_leases_expiry_idx ON materialization_leases(expires_at_ms);
CREATE UNIQUE INDEX materialization_leases_owner_resource_idx
ON materialization_leases(recovery_operation_id, owner_token, worktree_id, resource_key);

CREATE TABLE pending_steps (
    transaction_id TEXT NOT NULL,
    step_id TEXT NOT NULL,
    space_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'validated', 'admitted', 'rejected')),
    PRIMARY KEY (transaction_id, step_id, space_id),
    FOREIGN KEY (step_id) REFERENCES steps(step_id) ON DELETE CASCADE,
    FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE pending_ref_updates (
    transaction_id TEXT NOT NULL,
    target_kind TEXT NOT NULL CHECK (target_kind IN (
        'repository_view', 'operation_heads', 'worktree_active_operation'
    )),
    target_id TEXT NOT NULL CHECK (length(target_id) > 0),
    expected_json TEXT NOT NULL CHECK (json_valid(expected_json)),
    new_json TEXT NOT NULL CHECK (json_valid(new_json)),
    state TEXT NOT NULL CHECK (state IN ('pending', 'validated', 'applied', 'rejected')),
    PRIMARY KEY (transaction_id, target_kind, target_id)
) STRICT;

CREATE TABLE sync_cursors (
    space_id TEXT NOT NULL,
    remote_id TEXT NOT NULL CHECK (length(remote_id) > 0),
    cursor TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0),
    PRIMARY KEY (space_id, remote_id),
    FOREIGN KEY (space_id) REFERENCES spaces(space_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE object_pins (
    pin_id TEXT PRIMARY KEY CHECK (length(pin_id) > 0),
    object_id TEXT NOT NULL,
    reason TEXT NOT NULL CHECK (length(reason) > 0),
    expires_at_ms INTEGER,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    CHECK (expires_at_ms IS NULL OR expires_at_ms > created_at_ms),
    FOREIGN KEY (object_id) REFERENCES objects(object_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX object_pins_object_idx ON object_pins(object_id, expires_at_ms);

CREATE TABLE gc_roots (
    root_kind TEXT NOT NULL CHECK (root_kind IN (
        'current_view', 'operation_head', 'retained_operation', 'worktree',
        'marker', 'release', 'observation', 'pin'
    )),
    root_id TEXT NOT NULL CHECK (length(root_id) > 0),
    object_id TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    PRIMARY KEY (root_kind, root_id, object_id),
    FOREIGN KEY (object_id) REFERENCES objects(object_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX gc_roots_object_idx ON gc_roots(object_id);
