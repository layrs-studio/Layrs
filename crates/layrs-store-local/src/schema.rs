//! Ordered, checksummed STRICT schema migrations.

pub const CURRENT_SCHEMA_VERSION: u32 = 2;

pub(crate) struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationInfo {
    pub version: u32,
    pub name: &'static str,
    pub checksum: String,
}

impl Migration {
    pub fn checksum(&self) -> String {
        blake3::hash(self.sql.as_bytes()).to_hex().to_string()
    }

    pub fn info(&self) -> MigrationInfo {
        MigrationInfo {
            version: self.version,
            name: self.name,
            checksum: self.checksum(),
        }
    }
}

pub fn migration_info() -> Vec<MigrationInfo> {
    MIGRATIONS.iter().map(Migration::info).collect()
}

pub(crate) const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "transactional_metadata_foundation",
        sql: MIGRATION_001,
    },
    Migration {
        version: 2,
        name: "weave_metadata",
        sql: MIGRATION_002,
    },
];

const MIGRATION_001: &str = r#"
CREATE TABLE store_meta (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    format_version INTEGER NOT NULL CHECK (format_version = 1),
    schema_version INTEGER NOT NULL CHECK (schema_version >= 0),
    quarantine_generation INTEGER NOT NULL DEFAULT 0 CHECK (quarantine_generation >= 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms)
) STRICT;

INSERT INTO store_meta (
    singleton, format_version, schema_version, quarantine_generation,
    created_at_ms, updated_at_ms
) VALUES (
    1, 1, 0, 0,
    CAST(unixepoch('subsec') * 1000 AS INTEGER),
    CAST(unixepoch('subsec') * 1000 AS INTEGER)
);

CREATE TABLE local_spaces (
    space_id TEXT PRIMARY KEY CHECK (length(trim(space_id)) > 0),
    canonical_root TEXT NOT NULL UNIQUE CHECK (length(canonical_root) > 0),
    display_name TEXT,
    case_sensitive INTEGER NOT NULL CHECK (case_sensitive IN (0, 1)),
    identity_token TEXT,
    state TEXT NOT NULL DEFAULT 'active'
        CHECK (state IN ('active', 'quarantined', 'deleted')),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms)
) STRICT;

CREATE TABLE object_index (
    object_id TEXT PRIMARY KEY CHECK (length(trim(object_id)) > 0),
    algorithm TEXT NOT NULL CHECK (algorithm IN ('blake3', 'placeholder')),
    byte_len INTEGER NOT NULL CHECK (byte_len >= 0),
    object_kind TEXT NOT NULL
        CHECK (object_kind IN ('chunk', 'file', 'tree', 'step', 'opaque')),
    relative_path TEXT NOT NULL UNIQUE CHECK (length(relative_path) > 0),
    state TEXT NOT NULL DEFAULT 'available'
        CHECK (state IN ('available', 'quarantined', 'missing')),
    verified_at_ms INTEGER CHECK (verified_at_ms IS NULL OR verified_at_ms >= 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0)
) STRICT;

CREATE TABLE object_edges (
    parent_object_id TEXT NOT NULL,
    child_object_id TEXT NOT NULL,
    edge_kind TEXT NOT NULL
        CHECK (edge_kind IN ('chunk', 'entry', 'parent', 'dependency')),
    ordinal INTEGER NOT NULL DEFAULT 0 CHECK (ordinal >= 0),
    PRIMARY KEY (parent_object_id, edge_kind, ordinal),
    UNIQUE (parent_object_id, child_object_id, edge_kind),
    CHECK (parent_object_id <> child_object_id),
    FOREIGN KEY (parent_object_id) REFERENCES object_index(object_id) ON DELETE CASCADE,
    FOREIGN KEY (child_object_id) REFERENCES object_index(object_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX object_edges_child_idx ON object_edges(child_object_id);

CREATE TABLE layers (
    layer_id TEXT PRIMARY KEY CHECK (length(trim(layer_id)) > 0),
    space_id TEXT NOT NULL,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    layer_kind TEXT NOT NULL
        CHECK (layer_kind IN ('local', 'tracking', 'system')),
    state TEXT NOT NULL DEFAULT 'active'
        CHECK (state IN ('active', 'deleting', 'deleted', 'quarantined')),
    base_layer_id TEXT,
    timeline_active INTEGER NOT NULL DEFAULT 0 CHECK (timeline_active IN (0, 1)),
    generation INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    UNIQUE (space_id, name),
    UNIQUE (layer_id, space_id),
    FOREIGN KEY (space_id) REFERENCES local_spaces(space_id) ON DELETE CASCADE,
    FOREIGN KEY (base_layer_id, space_id) REFERENCES layers(layer_id, space_id)
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE UNIQUE INDEX layers_one_active_timeline_per_space
ON layers(space_id)
WHERE timeline_active = 1 AND state = 'active';

CREATE INDEX layers_space_state_idx ON layers(space_id, state);

CREATE TABLE layer_access_rules (
    rule_id TEXT PRIMARY KEY CHECK (length(trim(rule_id)) > 0),
    layer_id TEXT NOT NULL,
    subject_kind TEXT NOT NULL
        CHECK (subject_kind IN ('owner', 'account', 'team', 'device', 'public')),
    subject_id TEXT,
    effect TEXT NOT NULL CHECK (effect IN ('allow', 'deny')),
    capability TEXT NOT NULL
        CHECK (capability IN ('read', 'write', 'publish', 'admin')),
    priority INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    CHECK (
        (subject_kind IN ('owner', 'public') AND subject_id IS NULL) OR
        (subject_kind NOT IN ('owner', 'public') AND length(trim(subject_id)) > 0)
    ),
    UNIQUE (layer_id, subject_kind, subject_id, capability),
    FOREIGN KEY (layer_id) REFERENCES layers(layer_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX layer_access_rules_eval_idx
ON layer_access_rules(layer_id, capability, priority DESC);

CREATE TABLE steps (
    step_id TEXT PRIMARY KEY CHECK (length(trim(step_id)) > 0),
    space_id TEXT NOT NULL,
    layer_id TEXT NOT NULL,
    parent_step_id TEXT,
    root_object_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence >= 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    authored_at TEXT,
    UNIQUE (layer_id, sequence),
    UNIQUE (step_id, space_id, layer_id),
    FOREIGN KEY (layer_id, space_id) REFERENCES layers(layer_id, space_id) ON DELETE CASCADE,
    FOREIGN KEY (parent_step_id, space_id, layer_id)
        REFERENCES steps(step_id, space_id, layer_id)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (root_object_id) REFERENCES object_index(object_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX steps_space_layer_idx ON steps(space_id, layer_id, sequence DESC);

CREATE TABLE step_paths (
    step_id TEXT NOT NULL,
    path TEXT NOT NULL CHECK (
        length(path) > 0 AND path <> '.' AND path NOT LIKE '/%' AND instr(path, char(0)) = 0
    ),
    object_id TEXT NOT NULL,
    entry_kind TEXT NOT NULL CHECK (entry_kind IN ('file', 'directory', 'symlink')),
    executable INTEGER NOT NULL DEFAULT 0 CHECK (executable IN (0, 1)),
    PRIMARY KEY (step_id, path),
    FOREIGN KEY (step_id) REFERENCES steps(step_id) ON DELETE CASCADE,
    FOREIGN KEY (object_id) REFERENCES object_index(object_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX step_paths_object_idx ON step_paths(object_id);

CREATE TABLE refs (
    space_id TEXT NOT NULL,
    ref_name TEXT NOT NULL CHECK (length(trim(ref_name)) > 0),
    ref_kind TEXT NOT NULL CHECK (ref_kind IN ('layer_head', 'timeline', 'remote')),
    layer_id TEXT NOT NULL,
    step_id TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation > 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0),
    PRIMARY KEY (space_id, ref_name),
    FOREIGN KEY (layer_id, space_id) REFERENCES layers(layer_id, space_id) ON DELETE CASCADE,
    FOREIGN KEY (step_id, space_id, layer_id)
        REFERENCES steps(step_id, space_id, layer_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX refs_step_idx ON refs(step_id);

CREATE TABLE operation_journal (
    operation_id TEXT PRIMARY KEY CHECK (length(trim(operation_id)) > 0),
    space_id TEXT NOT NULL,
    operation_kind TEXT NOT NULL CHECK (
        operation_kind IN ('publish', 'layer_delete', 'scan', 'weave', 'gc', 'object_install')
    ),
    state TEXT NOT NULL CHECK (
        state IN (
            'prepared', 'external_pending', 'applying', 'metadata_committed',
            'finalized', 'rolling_back', 'rolled_back', 'needs_repair'
        )
    ),
    payload_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(payload_json)),
    attempt INTEGER NOT NULL DEFAULT 0 CHECK (attempt >= 0),
    last_error TEXT,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    FOREIGN KEY (space_id) REFERENCES local_spaces(space_id) ON DELETE RESTRICT
) STRICT;

CREATE UNIQUE INDEX operation_journal_one_non_terminal_per_space
ON operation_journal(space_id)
WHERE state IN (
    'prepared', 'external_pending', 'applying', 'metadata_committed', 'rolling_back'
);

CREATE INDEX operation_journal_recovery_idx
ON operation_journal(state, updated_at_ms);

CREATE TABLE operation_paths (
    operation_id TEXT NOT NULL,
    path TEXT NOT NULL CHECK (length(path) > 0 AND instr(path, char(0)) = 0),
    path_role TEXT NOT NULL
        CHECK (path_role IN ('source', 'destination', 'temporary', 'quarantine')),
    expected_object_id TEXT,
    path_state TEXT NOT NULL DEFAULT 'pending'
        CHECK (path_state IN ('pending', 'verified', 'applied', 'rolled_back', 'quarantined')),
    PRIMARY KEY (operation_id, path, path_role),
    FOREIGN KEY (operation_id) REFERENCES operation_journal(operation_id) ON DELETE CASCADE,
    FOREIGN KEY (expected_object_id) REFERENCES object_index(object_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE pending_publish (
    publish_id TEXT PRIMARY KEY CHECK (length(trim(publish_id)) > 0),
    space_id TEXT NOT NULL,
    layer_id TEXT NOT NULL,
    step_id TEXT NOT NULL,
    operation_id TEXT,
    idempotency_key TEXT NOT NULL UNIQUE CHECK (length(trim(idempotency_key)) > 0),
    expected_generation INTEGER NOT NULL CHECK (expected_generation >= 0),
    state TEXT NOT NULL
        CHECK (state IN ('prepared', 'uploading', 'committed', 'failed', 'needs_repair')),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    FOREIGN KEY (layer_id, space_id) REFERENCES layers(layer_id, space_id) ON DELETE CASCADE,
    FOREIGN KEY (step_id, space_id, layer_id)
        REFERENCES steps(step_id, space_id, layer_id) ON DELETE RESTRICT,
    FOREIGN KEY (operation_id) REFERENCES operation_journal(operation_id) ON DELETE SET NULL
) STRICT;

CREATE INDEX pending_publish_space_state_idx ON pending_publish(space_id, state);

CREATE TABLE pending_layer_deletions (
    layer_id TEXT PRIMARY KEY,
    space_id TEXT NOT NULL,
    operation_id TEXT NOT NULL UNIQUE,
    expected_generation INTEGER NOT NULL CHECK (expected_generation >= 0),
    state TEXT NOT NULL CHECK (
        state IN ('prepared', 'external_pending', 'metadata_committed', 'finalized', 'needs_repair')
    ),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    FOREIGN KEY (layer_id, space_id) REFERENCES layers(layer_id, space_id) ON DELETE CASCADE,
    FOREIGN KEY (operation_id) REFERENCES operation_journal(operation_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE sync_cursors (
    space_id TEXT NOT NULL,
    remote_id TEXT NOT NULL CHECK (length(trim(remote_id)) > 0),
    cursor TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation > 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0),
    PRIMARY KEY (space_id, remote_id),
    FOREIGN KEY (space_id) REFERENCES local_spaces(space_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE scan_cache (
    space_id TEXT NOT NULL,
    path_key TEXT NOT NULL CHECK (length(path_key) > 0 AND instr(path_key, char(0)) = 0),
    file_identity TEXT,
    byte_len INTEGER NOT NULL CHECK (byte_len >= 0),
    modified_at_ns INTEGER NOT NULL CHECK (modified_at_ns >= 0),
    mode INTEGER NOT NULL CHECK (mode >= 0),
    object_id TEXT,
    scanned_at_ms INTEGER NOT NULL CHECK (scanned_at_ms >= 0),
    PRIMARY KEY (space_id, path_key),
    FOREIGN KEY (space_id) REFERENCES local_spaces(space_id) ON DELETE CASCADE,
    FOREIGN KEY (object_id) REFERENCES object_index(object_id) ON DELETE SET NULL
) STRICT;

CREATE INDEX scan_cache_object_idx ON scan_cache(object_id);

CREATE TABLE gc_roots (
    root_id TEXT PRIMARY KEY CHECK (length(trim(root_id)) > 0),
    space_id TEXT NOT NULL,
    object_id TEXT NOT NULL,
    root_kind TEXT NOT NULL
        CHECK (root_kind IN ('ref', 'pending', 'journal', 'lease', 'manual')),
    expires_at_ms INTEGER CHECK (expires_at_ms IS NULL OR expires_at_ms >= 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    UNIQUE (space_id, object_id, root_kind),
    FOREIGN KEY (space_id) REFERENCES local_spaces(space_id) ON DELETE CASCADE,
    FOREIGN KEY (object_id) REFERENCES object_index(object_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX gc_roots_object_idx ON gc_roots(object_id);
"#;

const MIGRATION_002: &str = r#"
CREATE TABLE weave_sessions (
    weave_id TEXT PRIMARY KEY CHECK (length(trim(weave_id)) > 0),
    space_id TEXT NOT NULL,
    target_layer_id TEXT NOT NULL,
    base_step_id TEXT,
    target_step_id TEXT,
    result_step_id TEXT,
    state TEXT NOT NULL CHECK (
        state IN (
            'prepared', 'in_progress', 'conflicted', 'resolved', 'applying',
            'completed', 'aborted', 'needs_repair'
        )
    ),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    FOREIGN KEY (target_layer_id, space_id)
        REFERENCES layers(layer_id, space_id) ON DELETE CASCADE,
    FOREIGN KEY (base_step_id) REFERENCES steps(step_id) ON DELETE RESTRICT,
    FOREIGN KEY (target_step_id) REFERENCES steps(step_id) ON DELETE RESTRICT,
    FOREIGN KEY (result_step_id) REFERENCES steps(step_id) ON DELETE RESTRICT
) STRICT;

CREATE UNIQUE INDEX weave_sessions_one_active_per_space
ON weave_sessions(space_id)
WHERE state IN ('prepared', 'in_progress', 'conflicted', 'resolved', 'applying');

CREATE INDEX weave_sessions_recovery_idx ON weave_sessions(state, updated_at_ms);

CREATE TABLE weave_steps (
    weave_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    step_id TEXT NOT NULL,
    step_role TEXT NOT NULL CHECK (step_role IN ('base', 'source', 'target', 'result')),
    PRIMARY KEY (weave_id, ordinal),
    UNIQUE (weave_id, step_id, step_role),
    FOREIGN KEY (weave_id) REFERENCES weave_sessions(weave_id) ON DELETE CASCADE,
    FOREIGN KEY (step_id) REFERENCES steps(step_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE weave_conflicts (
    conflict_id TEXT PRIMARY KEY CHECK (length(trim(conflict_id)) > 0),
    weave_id TEXT NOT NULL,
    path TEXT NOT NULL CHECK (length(path) > 0 AND instr(path, char(0)) = 0),
    conflict_kind TEXT NOT NULL CHECK (
        conflict_kind IN ('content', 'delete_modify', 'rename', 'type', 'mode', 'binary')
    ),
    state TEXT NOT NULL CHECK (state IN ('open', 'resolved', 'quarantined')),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    UNIQUE (weave_id, path),
    FOREIGN KEY (weave_id) REFERENCES weave_sessions(weave_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX weave_conflicts_state_idx ON weave_conflicts(weave_id, state);

CREATE TABLE weave_blocks (
    block_id TEXT PRIMARY KEY CHECK (length(trim(block_id)) > 0),
    conflict_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    side TEXT NOT NULL CHECK (side IN ('base', 'source', 'target', 'merged')),
    object_id TEXT,
    content_hash TEXT,
    UNIQUE (conflict_id, side, ordinal),
    FOREIGN KEY (conflict_id) REFERENCES weave_conflicts(conflict_id) ON DELETE CASCADE,
    FOREIGN KEY (object_id) REFERENCES object_index(object_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE weave_segments (
    segment_id TEXT PRIMARY KEY CHECK (length(trim(segment_id)) > 0),
    block_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    start_byte INTEGER NOT NULL CHECK (start_byte >= 0),
    end_byte INTEGER NOT NULL CHECK (end_byte >= start_byte),
    digest TEXT,
    UNIQUE (block_id, ordinal),
    FOREIGN KEY (block_id) REFERENCES weave_blocks(block_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE weave_resolutions (
    resolution_id TEXT PRIMARY KEY CHECK (length(trim(resolution_id)) > 0),
    conflict_id TEXT NOT NULL,
    resolution_kind TEXT NOT NULL CHECK (
        resolution_kind IN ('source', 'target', 'manual', 'delete', 'rename')
    ),
    object_id TEXT,
    payload_json TEXT CHECK (payload_json IS NULL OR json_valid(payload_json)),
    selected INTEGER NOT NULL DEFAULT 0 CHECK (selected IN (0, 1)),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    FOREIGN KEY (conflict_id) REFERENCES weave_conflicts(conflict_id) ON DELETE CASCADE,
    FOREIGN KEY (object_id) REFERENCES object_index(object_id) ON DELETE RESTRICT
) STRICT;

CREATE UNIQUE INDEX weave_resolutions_one_selected
ON weave_resolutions(conflict_id)
WHERE selected = 1;
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_ordered_and_checksums_are_stable_length() {
        assert_eq!(MIGRATIONS.len(), CURRENT_SCHEMA_VERSION as usize);
        for (index, migration) in MIGRATIONS.iter().enumerate() {
            assert_eq!(migration.version as usize, index + 1);
            assert_eq!(migration.checksum().len(), 64);
        }
    }
}
