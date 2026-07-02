-- File and tree objects are content-addressed Merkle objects. The object bytes
-- and manifests are global by digest, while this table records which Spaces
-- are allowed to reference those objects.

CREATE TABLE IF NOT EXISTS space_file_objects (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    file_object_id TEXT NOT NULL REFERENCES file_objects(file_object_id) ON DELETE CASCADE,
    created_by_account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, space_id, file_object_id)
);

INSERT INTO space_file_objects (workspace_id, space_id, file_object_id, created_by_account_id, created_at)
SELECT workspace_id, space_id, file_object_id, created_by_account_id, created_at
FROM file_objects
ON CONFLICT (workspace_id, space_id, file_object_id) DO NOTHING;

CREATE INDEX IF NOT EXISTS space_file_objects_file_idx
    ON space_file_objects (file_object_id);

CREATE TABLE IF NOT EXISTS space_tree_objects (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    space_id TEXT NOT NULL REFERENCES spaces(space_id) ON DELETE CASCADE,
    tree_id TEXT NOT NULL REFERENCES tree_objects(tree_id) ON DELETE CASCADE,
    created_by_account_id TEXT REFERENCES accounts(account_id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, space_id, tree_id)
);

INSERT INTO space_tree_objects (workspace_id, space_id, tree_id, created_by_account_id, created_at)
SELECT workspace_id, space_id, tree_id, created_by_account_id, created_at
FROM tree_objects
ON CONFLICT (workspace_id, space_id, tree_id) DO NOTHING;

CREATE INDEX IF NOT EXISTS space_tree_objects_tree_idx
    ON space_tree_objects (tree_id);
