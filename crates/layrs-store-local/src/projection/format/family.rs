use super::*;

pub(in crate::projection) fn create(
    root: &Path,
    family: &super::super::shared::PartialStoreFamily,
    worktree_id: WorktreeId,
    initial: ProjectionSnapshot,
    selection: &layrs_core::projection::ProjectionSelection,
) -> Result<ProjectionStore> {
    create_with_faults(
        root,
        family,
        worktree_id,
        initial,
        selection,
        &NoCreationFaults,
    )
}
pub(in crate::projection) fn create_with_faults(
    root: &Path,
    family: &super::super::shared::PartialStoreFamily,
    worktree_id: WorktreeId,
    initial: ProjectionSnapshot,
    selection: &layrs_core::projection::ProjectionSelection,
    faults: &dyn CreationFaults,
) -> Result<ProjectionStore> {
    create_inner(root, family, worktree_id, initial, selection, None, faults)
}
pub(in crate::projection) fn create_import(
    root: &Path,
    family: &super::super::shared::PartialStoreFamily,
    worktree_id: WorktreeId,
    initial: ProjectionSnapshot,
    selection: &layrs_core::projection::ProjectionSelection,
    provenance: String,
) -> Result<ProjectionStore> {
    create_inner(
        root,
        family,
        worktree_id,
        initial,
        selection,
        Some(provenance),
        &NoCreationFaults,
    )
}
fn create_inner(
    root: &Path,
    family: &super::super::shared::PartialStoreFamily,
    worktree_id: WorktreeId,
    initial: ProjectionSnapshot,
    selection: &layrs_core::projection::ProjectionSelection,
    provenance: Option<String>,
    faults: &dyn CreationFaults,
) -> Result<ProjectionStore> {
    initial.canonical_bytes()?;
    selection.validate_snapshot(&initial)?;
    validate_platform(&initial)?;
    if initial
        .entries
        .iter()
        .any(|e| e.state == layrs_core::projection::EntryState::Materialized)
    {
        return Err(invalid("shared bootstrap cannot claim materialized files"));
    }
    require_fresh_control(root)?;
    let checkout = crate::scm_v1::checkout::CheckoutReader::open(root, None)?;
    let root = checkout.root().to_owned();
    if root.starts_with(family.root()) || family.root().starts_with(&root) {
        return Err(invalid(
            "shared Store and checkout require separate nonnested roots",
        ));
    }
    let lease = family.lease(SessionMode::Shared)?;
    let family_root = PinnedDirectory::open(family.root())?;
    let member_path = family.member_metadata(&worktree_id);
    let key = member_path
        .parent()
        .and_then(Path::file_name)
        .ok_or_else(|| invalid("invalid member path"))?;
    let members = family_root.open_child("worktrees".as_ref())?;
    // Creation is exclusive; existing identities are never silently adopted.
    fs::create_dir(
        member_path
            .parent()
            .ok_or_else(|| invalid("invalid metadata parent"))?,
    )?;
    members.sync()?;
    let member = members.open_child(key)?;
    let metadata = member.ensure_private_child("metadata".as_ref())?;
    metadata
        .create_file("layrs.lock".as_ref())?
        .file()
        .sync_all()?;
    let db = metadata.create_file(DATABASE.as_ref())?;
    db.file().sync_all()?;
    drop(db);
    metadata.sync()?;
    fs::create_dir(root.join(".layrs"))?;
    sync_directory(&root)?;
    let local = PinnedDirectory::open(&root.join(".layrs"))?;
    let identity = ProjectionIdentity {
        store_id: family.identity().store_id.clone(),
        worktree_id,
        space_id: family.identity().space_id.clone(),
        origin: family.identity().origin.clone(),
        principal_id: family.identity().principal_id.clone(),
    };
    let binding = family.binding(
        &identity.worktree_id,
        super::super::file::DatabasePin::open(&member_path.join(DATABASE))?
            .identity()
            .to_owned(),
    );
    let marker = Marker {
        schema_version: "layrs.projection-store-format.v1".into(),
        magic: "LAYRS_NATIVE_STORE".into(),
        format_version: 4,
        min_reader_version: 4,
        identity,
        checkout_root: root
            .to_str()
            .ok_or_else(|| invalid("non-UTF8 checkout root"))?
            .into(),
        checkout_identity: checkout.identity(),
        store_identity: local.identity_token(),
        metadata_identity: metadata.identity_token(),
        initial_snapshot_digest: database::digest(0, None, &initial.canonical_bytes()?),
        initial_selection_digest: if matches!(
            selection,
            layrs_core::projection::ProjectionSelection::All
        ) {
            None
        } else {
            Some(selection::initial_digest(selection)?)
        },
        shared: Some(binding),
        import_digest: provenance,
    };
    let mut c = database::connect(&member_path.join(DATABASE), true)?;
    database::initialize(&mut c, &marker, &initial, selection)?;
    drop(c);
    metadata.sync()?;
    // Private bootstrap identity and registry intent precede the checkout marker.
    // Losing that marker can never make the owner impossible to classify safely.
    lease.register(&marker, false)?;
    faults.check(CreationPoint::DatabaseDurable)?;
    let mut file = local.create_file("FORMAT".as_ref())?;
    file.file_mut().write_all(&marker.bytes()?)?;
    file.file().sync_all()?;
    local.sync()?;
    faults.check(CreationPoint::MarkerDurable)?;
    if marker.import_digest.is_none() {
        lease.register(&marker, true)?;
    }
    let store = ProjectionStore { root, marker };
    if store.marker.import_digest.is_none() {
        drop(lease);
        open(&store.root)
    } else {
        Ok(store)
    }
}

pub(in crate::projection) fn resume(
    root: &Path,
    family: &super::super::shared::PartialStoreFamily,
    initial: &ProjectionSnapshot,
    selection: &layrs_core::projection::ProjectionSelection,
) -> Result<ProjectionStore> {
    let root = std::path::absolute(root)?;
    let marker = read_marker(&root)?;
    let binding = marker
        .shared
        .as_ref()
        .ok_or_else(|| invalid("bootstrap has no shared binding"))?;
    if marker.import_digest.is_some() {
        return Err(invalid("conversion requires its source-preserving resume"));
    }
    if binding
        != &family.binding(
            &marker.identity.worktree_id,
            binding.database_identity.clone(),
        )
        || marker.initial_snapshot_digest != database::digest(0, None, &initial.canonical_bytes()?)
    {
        return Err(invalid("bootstrap identity or snapshot differs"));
    }
    selection.validate_snapshot(initial)?;
    let selection_digest = if matches!(selection, layrs_core::projection::ProjectionSelection::All)
    {
        None
    } else {
        Some(selection::initial_digest(selection)?)
    };
    if marker.initial_selection_digest != selection_digest
        || marker.identity.store_id != family.identity().store_id
        || marker.identity.space_id != family.identity().space_id
        || marker.identity.origin != family.identity().origin
        || marker.identity.principal_id != family.identity().principal_id
    {
        return Err(invalid("bootstrap selection or authority differs"));
    }
    validate_platform(initial)?;
    if initial
        .entries
        .iter()
        .any(|e| e.state == layrs_core::projection::EntryState::Materialized)
    {
        return Err(invalid("bootstrap cannot assert materialized files"));
    }
    let lease = family.lease(SessionMode::Shared)?;
    let _lock = StoreLock::acquire_existing(
        &binding.metadata_path.join("layrs.lock"),
        SessionMode::Exclusive,
    )
    .map_err(invalid)?;
    let metadata = PinnedDirectory::open(&binding.metadata_path)?;
    let db = super::super::file::DatabasePin::open(&binding.metadata_path.join(DATABASE))?;
    if db.identity() != binding.database_identity {
        return Err(invalid("bootstrap database changed"));
    }
    let boundaries = preliminary_pins(&root, &marker, &metadata, &db)?;
    let read = database::connect(&binding.metadata_path.join(DATABASE), false)?;
    let version: u32 = read.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != 0 {
        database::validate(&read, &marker)?;
    } else if !database::schema(&read)?.is_empty() {
        return Err(invalid("bootstrap schema is not empty"));
    }
    drop(read);
    boundaries.verify(&root, &marker)?;
    verify_boundaries(&root, &marker, &metadata, &db)?;
    let mut c = database::connect(&binding.metadata_path.join(DATABASE), true)?;
    let version: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version == 0 {
        database::initialize(&mut c, &marker, initial, selection)?;
    }
    database::validate(&c, &marker)?;
    let current = database::current(&c)?;
    if current.generation != 0 || current.snapshot != *initial || selection::current(&c)? != *selection
        || c.query_row("SELECT EXISTS(SELECT 1 FROM projection_captures) OR EXISTS(SELECT 1 FROM projection_claims)", [], |r| r.get::<_, bool>(0))? {
        return Err(invalid("bootstrap already contains accepted history"));
    }
    database::require_mutable(&c)?;
    lease.register(&marker, true)?;
    drop(c);
    drop(_lock);
    drop(lease);
    open(&root)
}

/// The bound recovery handle already holds the exact marker/private database,
/// an exclusive owner lock, and a nonmigrating validated writer. No root reopen
/// or caller-supplied snapshot may substitute a different member at acceptance.
pub(in crate::projection) fn finish_initialization(
    store: &ProjectionStore,
    access: &Access,
) -> Result<()> {
    if store.marker.shared.is_none() || store.marker.import_digest.is_some() {
        return Err(invalid(
            "ordinary initialization requires a non-import shared member",
        ));
    }
    let lease = access
        .family
        .as_ref()
        .ok_or_else(|| invalid("missing initialization family"))?;
    access.verify()?;
    let state = lease.registered_state(&store.marker)?;
    if state == "ready" {
        return Ok(());
    }
    if state != "initializing" {
        return Err(invalid("unknown bootstrap registration"));
    }
    database::require_mutable(&access.connection)?;
    let current = database::current(&access.connection)?;
    let initial = super::super::offline::snapshot_at(&access.connection, 0, None)?.snapshot;
    let selection = super::super::selection::at(&access.connection, 0)?;
    store.marker.validate_initial_selection(&selection)?;
    validate_platform(&initial)?;
    if current.generation != 0 || current.snapshot != initial
        || super::super::selection::current(&access.connection)? != selection
        || initial.entries.iter().any(|e| e.state == layrs_core::projection::EntryState::Materialized)
        || access.connection.query_row("SELECT EXISTS(SELECT 1 FROM projection_captures) OR EXISTS(SELECT 1 FROM projection_claims) OR EXISTS(SELECT 1 FROM projection_preparations) OR EXISTS(SELECT 1 FROM projection_budgets) OR EXISTS(SELECT 1 FROM projection_prejournal) OR EXISTS(SELECT 1 FROM projection_publications)", [], |r| r.get::<_, bool>(0))?
    {
        return Err(invalid("bootstrap already contains accepted work"));
    }
    access.verify()?;
    lease.register(&store.marker, true)?;
    access.verify()
}
