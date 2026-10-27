//! Copy only the authenticated immutable SQL closure. The caller owns both
//! transactions and must roll back the destination transaction on any error.
use super::{
    rows::{Kind, Spec},
    scoped::{self, ScopeView},
    *,
};
use rusqlite::{
    params, params_from_iter,
    types::{Value, ValueRef},
    OptionalExtension,
};

struct Layout {
    insert: &'static str,
    select_key: &'static str,
    key_columns: usize,
}

fn layout(kind: Kind) -> Layout {
    match kind {
        Kind::Snapshots => Layout {
            insert: "INSERT INTO projection_snapshots(generation,digest,parent_digest,payload) VALUES(?1,?2,?3,?4) ON CONFLICT DO NOTHING",
            select_key: "SELECT generation,digest,parent_digest,payload FROM projection_snapshots WHERE generation=?1",
            key_columns: 1,
        },
        Kind::Selections => Layout {
            insert: "INSERT INTO projection_selections(generation,digest,payload) VALUES(?1,?2,?3) ON CONFLICT DO NOTHING",
            select_key: "SELECT generation,digest,payload FROM projection_selections WHERE generation=?1",
            key_columns: 1,
        },
        Kind::Captures => Layout {
            insert: "INSERT INTO projection_captures(sequence,digest,projection_generation,payload) VALUES(?1,?2,?3,?4) ON CONFLICT DO NOTHING",
            select_key: "SELECT sequence,digest,projection_generation,payload FROM projection_captures WHERE sequence=?1",
            key_columns: 1,
        },
        Kind::Publications => Layout {
            insert: "INSERT INTO projection_publications(sequence,request_digest,receipt_digest,receipt_bytes,next_snapshot,projection_generation,projection_digest,after_state) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT DO NOTHING",
            select_key: "SELECT sequence,request_digest,receipt_digest,receipt_bytes,next_snapshot,projection_generation,projection_digest,after_state FROM projection_publications WHERE sequence=?1",
            key_columns: 1,
        },
        Kind::Catalogs => Layout {
            insert: "INSERT INTO projection_provider_catalogs(id,source_generation,source_digest,selection_digest,payload) VALUES(?1,?2,?3,?4,?5) ON CONFLICT DO NOTHING",
            select_key: "SELECT id,source_generation,source_digest,selection_digest,payload FROM projection_provider_catalogs WHERE id=?1",
            key_columns: 1,
        },
        Kind::Descriptors => Layout {
            insert: "INSERT INTO projection_provider_descriptors(catalog_id,path,file_id,bytes,executable,manifest_id,manifest) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT DO NOTHING",
            select_key: "SELECT catalog_id,path,file_id,bytes,executable,manifest_id,manifest FROM projection_provider_descriptors WHERE catalog_id=?1 AND path=?2",
            key_columns: 2,
        },
        Kind::ActivationMaps => Layout {
            insert: "INSERT INTO projection_provider_activations(id,owner_identity_digest,mount_id,catalog_id,source_generation,source_digest,activated_generation,activated_digest,payload) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT DO NOTHING",
            select_key: "SELECT id,owner_identity_digest,mount_id,catalog_id,source_generation,source_digest,activated_generation,activated_digest,payload FROM projection_provider_activations WHERE id=?1",
            key_columns: 1,
        },
        Kind::TransitionReceipts => Layout {
            insert: "INSERT INTO projection_provider_transition_receipts(id,owner_identity_digest,transaction_id,companion_id,payload) VALUES(?1,?2,?3,?4,?5) ON CONFLICT DO NOTHING",
            select_key: "SELECT id,owner_identity_digest,transaction_id,companion_id,payload FROM projection_provider_transition_receipts WHERE id=?1",
            key_columns: 1,
        },
        Kind::Companions => Layout {
            insert: "INSERT INTO projection_provider_companions(id,file_count,deletion_count,files_digest,deletions_digest,payload) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT DO NOTHING",
            select_key: "SELECT id,file_count,deletion_count,files_digest,deletions_digest,payload FROM projection_provider_companions WHERE id=?1",
            key_columns: 1,
        },
        Kind::CompanionFiles => Layout {
            insert: "INSERT INTO projection_provider_companion_files(companion_id,path,file_id,bytes,executable,manifest_id,base_file_id,base_executable,reason,digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT DO NOTHING",
            select_key: "SELECT companion_id,path,file_id,bytes,executable,manifest_id,base_file_id,base_executable,reason,digest FROM projection_provider_companion_files WHERE companion_id=?1 AND path=?2",
            key_columns: 2,
        },
        Kind::CompanionDeletions => Layout {
            insert: "INSERT INTO projection_provider_companion_deletions(companion_id,path,base_file_id,base_executable,base_manifest_id,reason,digest) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT DO NOTHING",
            select_key: "SELECT companion_id,path,base_file_id,base_executable,base_manifest_id,reason,digest FROM projection_provider_companion_deletions WHERE companion_id=?1 AND path=?2",
            key_columns: 2,
        },
        Kind::ContentMappings => Layout {
            insert: "INSERT INTO projection_content(file_id,manifest_id,bytes) VALUES(?1,?2,?3) ON CONFLICT DO NOTHING",
            select_key: "SELECT file_id,manifest_id,bytes FROM projection_content WHERE file_id=?1",
            key_columns: 1,
        },
        Kind::RestoreHistory => Layout {
            insert: "INSERT INTO projection_restore_history(operation_id,capture_sequence,payload) VALUES(?1,?2,?3) ON CONFLICT DO NOTHING",
            select_key: "SELECT operation_id,capture_sequence,payload FROM projection_restore_history WHERE operation_id=?1",
            key_columns: 1,
        },
    }
}

fn same_value(actual: ValueRef<'_>, expected: &Value) -> bool {
    match (actual, expected) {
        (ValueRef::Null, Value::Null) => true,
        (ValueRef::Integer(a), Value::Integer(b)) => a == *b,
        (ValueRef::Text(a), Value::Text(b)) => a == b.as_bytes(),
        (ValueRef::Blob(a), Value::Blob(b)) => a == b,
        _ => false,
    }
}

fn exact_existing(
    destination: &Connection,
    layout: &Layout,
    values: &[Value],
) -> CauseResult<bool> {
    let mut statement = destination.prepare(layout.select_key)?;
    let mut rows = statement.query(params_from_iter(values.iter().take(layout.key_columns)))?;
    let Some(row) = rows.next()? else {
        return Ok(false);
    };
    if row.as_ref().column_count() != values.len() {
        return Err(invalid("import destination row shape differs"));
    }
    for (index, expected) in values.iter().enumerate() {
        if !same_value(row.get_ref(index)?, expected) {
            return Err(invalid("import destination collision differs from source"));
        }
    }
    if rows.next()?.is_some() {
        return Err(invalid("import destination key is not unique"));
    }
    Ok(true)
}

fn require_reference(
    destination: &Connection,
    query: &'static str,
    value: &Value,
) -> CauseResult<()> {
    if destination
        .query_row(query, params![value], |row| row.get::<_, u8>(0))
        .optional()?
        .is_none()
    {
        return Err(invalid(
            "import copied row has missing destination foreign key",
        ));
    }
    Ok(())
}

/// Exact schema8 declares this complete FK graph for the copied
/// rowsets. Each query is an indexed point lookup from one bounded source row.
fn require_copied_references(
    destination: &Connection,
    kind: Kind,
    row: &[Value],
) -> CauseResult<()> {
    const SNAPSHOT_GENERATION: &str = "SELECT 1 FROM projection_snapshots WHERE generation=?1";
    const SNAPSHOT_DIGEST: &str = "SELECT 1 FROM projection_snapshots WHERE digest=?1";
    const CAPTURE_SEQUENCE: &str = "SELECT 1 FROM projection_captures WHERE sequence=?1";
    const CATALOG_ID: &str = "SELECT 1 FROM projection_provider_catalogs WHERE id=?1";
    const COMPANION_ID: &str = "SELECT 1 FROM projection_provider_companions WHERE id=?1";
    match kind {
        Kind::Snapshots => {
            if row[2] != Value::Null {
                require_reference(destination, SNAPSHOT_DIGEST, &row[2])?;
            }
        }
        Kind::Selections => require_reference(destination, SNAPSHOT_GENERATION, &row[0])?,
        Kind::Captures => require_reference(destination, SNAPSHOT_GENERATION, &row[2])?,
        Kind::Publications => {
            require_reference(destination, CAPTURE_SEQUENCE, &row[0])?;
            require_reference(destination, SNAPSHOT_GENERATION, &row[5])?;
        }
        Kind::RestoreHistory => require_reference(destination, CAPTURE_SEQUENCE, &row[1])?,
        Kind::Catalogs => require_reference(destination, SNAPSHOT_GENERATION, &row[1])?,
        Kind::Descriptors => require_reference(destination, CATALOG_ID, &row[0])?,
        Kind::ActivationMaps => {
            require_reference(destination, CATALOG_ID, &row[3])?;
            require_reference(destination, SNAPSHOT_GENERATION, &row[4])?;
            require_reference(destination, SNAPSHOT_GENERATION, &row[6])?;
        }
        Kind::TransitionReceipts => {
            if row[3] != Value::Null {
                require_reference(destination, COMPANION_ID, &row[3])?;
            }
        }
        Kind::CompanionFiles | Kind::CompanionDeletions => {
            require_reference(destination, COMPANION_ID, &row[0])?;
        }
        Kind::Companions | Kind::ContentMappings => {}
    }
    Ok(())
}

fn visit(
    source: &Connection,
    destination: &Connection,
    spec: &Spec<'_>,
    insert: bool,
    references: bool,
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    let layout = layout(spec.kind);
    let mut statement = source.prepare(spec.rows_sql)?;
    let mut rows = statement.query(spec.params)?;
    while let Some(row) = rows.next()? {
        super::super::cancellation::check(cancel)?;
        let mut values = Vec::with_capacity(row.as_ref().column_count());
        for column in 0..row.as_ref().column_count() {
            let value = row.get_ref(column)?;
            if matches!(value, ValueRef::Real(_)) {
                return Err(invalid("import copy source REAL value differs"));
            }
            values.push(Value::from(value));
        }
        let exists = exact_existing(destination, &layout, &values)?;
        if insert && !exists {
            destination.execute(layout.insert, params_from_iter(&values))?;
            if !exact_existing(destination, &layout, &values)? {
                return Err(invalid("import destination insert/readback differs"));
            }
        }
        if references {
            require_copied_references(destination, spec.kind, &values)?;
        }
    }
    Ok(())
}

/// The proof must have been issued for `source` in the same retained SQL read
/// transaction. Both connections remain caller-owned. A failed copy may have
/// made destination writes: the caller must roll back its transaction.
pub(in crate::projection) fn copy_verified_prefix(
    source: &Connection,
    destination: &Connection,
    destination_marker: &format::Marker,
    prefix: &ProviderImportPrefixRecord,
    proof: &VerifiedImportClosure,
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    if !std::ptr::eq(source, proof.source_connection)
        || std::ptr::eq(source, destination)
        || source.is_autocommit()
        || destination.is_autocommit()
    {
        return Err(invalid(
            "import copy requires separate retained transactions",
        ));
    }
    let source_identity = match &prefix.source {
        ImportSource::Ordinary { identity, .. } | ImportSource::Provider { identity, .. } => {
            identity
        }
    };
    if destination_marker.identity != prefix.destination_identity
        || destination_marker.identity == *source_identity
        || proof.prefix_id() != prefix.digest()?
    {
        return Err(invalid("import copy source/destination identity differs"));
    }
    require_schema(source, &proof.marker, proof.metadata.kind, cancel)?;
    require_schema(
        destination,
        destination_marker,
        ImportDatabaseKind::Provider,
        cancel,
    )?;
    let scope = ScopeView::from_metadata(&proof.metadata);
    scoped::with_scope(
        source,
        proof.metadata.kind,
        prefix,
        &scope,
        cancel,
        |specs, closure| {
            if closure != proof.closure() || closure != &prefix.closure {
                return Err(invalid("import copy source closure changed before effects"));
            }
            // Bound any already present destination rows using the same scoped
            // LENGTH probes before comparing collision payloads or writing.
            let _destination_before = rows::preflight(destination, specs, cancel)?;
            // Every collision is checked before the first insert. A concurrent
            // destination writer is still serialized by the caller's transaction.
            for spec in specs {
                visit(source, destination, spec, false, false, cancel)?;
            }
            // The scope's digest order differs from FK insert order: companions
            // must exist before the receipts that reference them.
            for index in [0, 1, 2, 3, 4, 5, 6, 8, 9, 10, 7, 11] {
                visit(source, destination, &specs[index], true, false, cancel)?;
            }
            let preflight = rows::preflight(destination, specs, cancel)?;
            if rows::stream(destination, specs, &preflight, cancel)? != *closure {
                return Err(invalid("import destination exact SQL closure differs"));
            }
            for spec in specs {
                visit(source, destination, spec, false, true, cancel)?;
            }
            super::super::cancellation::check(cancel)?;
            Ok(())
        },
    )
}
