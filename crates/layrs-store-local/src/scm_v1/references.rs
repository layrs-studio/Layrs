//! Format 8 keeps no projection rows, hence none of the foreign keys through
//! which formats 5 to 7 refuse a dangling reference (council s26 r1). Every
//! reference those foreign keys guarded is listed here, typed, once: the
//! writer checks a batch's (`check_batch`), fsck the whole Store's.
use layrs_store::scm::{
    CanonicalObject, EntityRecord, Result, StoreError, StoreMutation, StoreObjectId, WriteBatch,
};
use rusqlite::{Connection, OptionalExtension};

/// What a record names and a foreign key of formats 5 to 7 required to
/// exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reference {
    /// A canonical record of this kind, admitted or not.
    Record(StoreObjectId),
    /// A metadata row: (table, key column, key).
    Row(&'static str, &'static str, String),
    /// Format 8: a history record (Operation), by its id (council s27).
    History(u8, String),
}

impl Reference {
    fn row(table: &'static str, column: &'static str, id: impl ToString) -> Self {
        Self::Row(table, column, id.to_string())
    }
}

/// The references of a canonical record (its `object_edges`, `steps`,
/// `step_parents` and `repository_view_*` rows in formats 5 to 7).
pub(crate) fn object_references(object: &CanonicalObject) -> Vec<Reference> {
    let mut references = Vec::new();
    match object {
        CanonicalObject::Tree(tree) => {
            for entry in tree.entries() {
                let target = entry.target();
                if let Some(id) = target.tree_id() {
                    references.push(Reference::Record(StoreObjectId::Tree(id.clone())));
                }
                if let Some(id) = target.conflict_id() {
                    references.push(Reference::Record(StoreObjectId::Conflict(id.clone())));
                }
                if let Some(id) = target.symlink_id() {
                    references.push(Reference::Record(StoreObjectId::Symlink(id.clone())));
                }
                if let Some(id) = target.tombstone_id() {
                    references.push(Reference::Record(StoreObjectId::Tombstone(id.clone())));
                }
            }
        }
        CanonicalObject::Step(step) => {
            references.push(Reference::Record(StoreObjectId::Tree(
                step.root_tree_id().clone(),
            )));
            references.extend(
                step.parent_step_ids()
                    .iter()
                    .map(|parent| Reference::Record(StoreObjectId::Step(parent.clone()))),
            );
        }
        CanonicalObject::RepositoryView(view) => {
            references.push(Reference::row("spaces", "space_id", view.space_id()));
            for heads in view.layer_heads() {
                references.push(Reference::row("layers", "layer_id", heads.layer_id()));
                references.extend(
                    heads
                        .step_ids()
                        .iter()
                        .map(|step| Reference::Record(StoreObjectId::Step(step.clone()))),
                );
            }
            references.extend(
                view.marker_ids()
                    .iter()
                    .map(|marker| Reference::row("markers", "marker_id", marker)),
            );
            references.extend(
                view.release_ids()
                    .iter()
                    .map(|release| Reference::row("releases", "release_id", release)),
            );
            for state in view.worktrees() {
                references.push(Reference::row(
                    "worktrees",
                    "worktree_id",
                    state.worktree_id(),
                ));
                if let Some(layer) = state.active_layer_id() {
                    references.push(Reference::row("layers", "layer_id", layer));
                }
                if let Some(step) = state.base_step_id() {
                    references.push(Reference::Record(StoreObjectId::Step(step.clone())));
                }
                if let Some(tree) = state.materialized_tree_id() {
                    references.push(Reference::Record(StoreObjectId::Tree(tree.clone())));
                }
            }
        }
        CanonicalObject::Product(product) => {
            references.extend(
                product
                    .direct_dependencies()
                    .into_iter()
                    .map(Reference::Record),
            );
        }
        CanonicalObject::ChunkRaw(_)
        | CanonicalObject::FileManifest(_)
        | CanonicalObject::Symlink(_)
        | CanonicalObject::Tombstone(_)
        | CanonicalObject::Conflict(_) => {}
    }
    references
}

/// The records an entity names (`operations.resulting_view_id`, rewrites,
/// observations, Markers and Releases in formats 5 to 7), and in format 8
/// (`history`) what the history tables' keys guarded: an Operation's Space
/// and parents, an observation's Space, Worktree and Operation, a
/// Worktree's and a publication's Operation.
pub(crate) fn entity_references(record: &EntityRecord, history: bool) -> Vec<Reference> {
    let operation = |id: &layrs_core::scm::OperationId| {
        Reference::History(crate::scm_v1::cas::OPERATION_KIND, id.as_str().to_owned())
    };
    let mut references = match record {
        EntityRecord::Operation(value) if history => {
            let mut references = vec![Reference::row("spaces", "space_id", value.space_id())];
            references.extend(value.parent_operation_ids().iter().map(operation));
            references
        }
        EntityRecord::StepObservation(value) if history => {
            let mut references = Vec::new();
            if let Some(space) = value.space_id() {
                references.push(Reference::row("spaces", "space_id", space));
            }
            if let Some(worktree) = value.worktree_id() {
                references.push(Reference::row("worktrees", "worktree_id", worktree));
            }
            references.extend(value.operation_id().map(operation));
            references
        }
        EntityRecord::Worktree(value) if history => vec![operation(value.active_operation_id())],
        EntityRecord::ReleasePublication(value) if history => {
            value.operation_id().map(operation).into_iter().collect()
        }
        _ => Vec::new(),
    };
    references.extend(record_references(record));
    references
}

fn record_references(record: &EntityRecord) -> Vec<Reference> {
    match record {
        EntityRecord::Operation(operation) => {
            let mut references = vec![Reference::Record(StoreObjectId::RepositoryView(
                operation.resulting_view_id().clone(),
            ))];
            for rewrite in operation.rewrite_map() {
                references.push(Reference::Record(StoreObjectId::Step(
                    rewrite.old_step_id().clone(),
                )));
                references.extend(
                    rewrite
                        .new_step_ids()
                        .iter()
                        .map(|step| Reference::Record(StoreObjectId::Step(step.clone()))),
                );
            }
            references
        }
        EntityRecord::StepObservation(observation) => vec![Reference::Record(StoreObjectId::Step(
            observation.step_id().clone(),
        ))],
        EntityRecord::Marker(marker) => vec![Reference::Record(StoreObjectId::Step(
            marker.target_step_id().clone(),
        ))],
        EntityRecord::Release(release) => vec![Reference::Record(StoreObjectId::Step(
            release.target_step_id().clone(),
        ))],
        _ => Vec::new(),
    }
}

/// Whether metadata row `reference` exists.
pub(crate) fn row_exists(
    connection: &Connection,
    table: &'static str,
    column: &'static str,
    id: &str,
) -> rusqlite::Result<bool> {
    connection
        .prepare_cached(&format!("SELECT 1 FROM {table} WHERE {column}=?1"))?
        .query_row([id], |_| Ok(()))
        .optional()
        .map(|found| found.is_some())
}

fn label(reference: &Reference) -> &'static str {
    match reference {
        Reference::Record(id) => match id {
            StoreObjectId::ChunkRaw(_) => "Chunk",
            StoreObjectId::FileManifest(_) => "FileManifest",
            StoreObjectId::Tree(_) => "Tree",
            StoreObjectId::Symlink(_) => "Symlink",
            StoreObjectId::Tombstone(_) => "Tombstone",
            StoreObjectId::Step(_) => "Step",
            StoreObjectId::RepositoryView(_) => "RepositoryView",
            StoreObjectId::Conflict(_) => "Conflict",
            StoreObjectId::Product(_) => "Product",
        },
        Reference::Row("spaces", ..) => "Space",
        Reference::Row("layers", ..) => "Layer",
        Reference::Row("markers", ..) => "Marker",
        Reference::Row("releases", ..) => "Release",
        Reference::Row("worktrees", ..) => "Worktree",
        Reference::Row(..) => "metadata row",
        Reference::History(..) => "Operation",
    }
}

/// Every reference a batch's records, entities and current View make,
/// checked once its rows are written (where formats 5 to 7 check their
/// deferred foreign keys): a record registered in the Store (`registered`,
/// which also answers for the batch's own), a metadata row in SQLite.
pub(crate) fn check_batch(
    connection: &Connection,
    batch: &WriteBatch,
    registered: impl Fn(&StoreObjectId) -> Result<bool>,
    history: impl Fn(u8, &str) -> Result<bool>,
) -> Result<()> {
    let operation = |id: &layrs_core::scm::OperationId| {
        Reference::History(crate::scm_v1::cas::OPERATION_KIND, id.as_str().to_owned())
    };
    let mut references = Vec::new();
    for object in batch.canonical_objects() {
        references.extend(object_references(object));
    }
    for record in batch.entity_records() {
        references.extend(entity_references(record, true));
    }
    for mutation in batch.mutations() {
        match mutation {
            StoreMutation::RepositoryView(change) => {
                references.push(Reference::Record(StoreObjectId::RepositoryView(
                    change.new_view_id().clone(),
                )));
            }
            StoreMutation::OperationHeads(change) => {
                references.extend(change.new_head_ids().iter().map(operation));
            }
            StoreMutation::WorktreeActiveOperation(change) => {
                references.push(operation(change.new_operation_id()));
            }
        }
    }
    for reference in references {
        let present = match &reference {
            Reference::Record(id) => registered(id)?,
            Reference::Row(table, column, id) => row_exists(connection, table, column, id)
                .map_err(|error| StoreError::Backend {
                    message: error.to_string(),
                })?,
            Reference::History(code, id) => history(*code, id)?,
        };
        if !present {
            let id = match &reference {
                Reference::Record(id) => id.to_string(),
                Reference::Row(_, _, id) | Reference::History(_, id) => id.clone(),
            };
            return Err(StoreError::NotFound {
                kind: label(&reference),
                id,
            });
        }
    }
    Ok(())
}
