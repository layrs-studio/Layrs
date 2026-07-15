//! Locked read-only and exclusive metadata session facade.

use crate::error::{StoreError, StoreResult};
use crate::lock::{SessionMode, StoreLock};
use crate::repository::MetadataRepository;
use crate::sqlite::{self, PragmaSnapshot};
use crate::{LocalStore, StoreLayout};

/// A non-reentrant metadata session guarded by process-local and OS locks.
///
/// Shared sessions open SQLite read-only. Exclusive sessions are the only
/// sessions allowed to migrate or mutate metadata.
pub struct LocalStoreSession {
    mode: SessionMode,
    layout: StoreLayout,
    repository: MetadataRepository,
    _lock: StoreLock,
}

impl LocalStoreSession {
    pub fn open(store: &LocalStore, mode: SessionMode) -> StoreResult<Self> {
        Self::open_layout(
            store.layout().clone(),
            store.config().sqlite_options(),
            mode,
        )
    }

    fn open_layout(
        layout: StoreLayout,
        options: sqlite::SqliteOptions,
        mode: SessionMode,
    ) -> StoreResult<Self> {
        crate::layout::validate_store_boundaries(&layout)?;
        let store_lock = StoreLock::acquire_existing(&layout.lock_path(), mode)?;
        let connection = match mode {
            SessionMode::Shared => sqlite::open_read_only(&layout.sqlite_path, options)?,
            SessionMode::Exclusive => sqlite::open_initialized(&layout.sqlite_path, options)?,
        };
        let repository = MetadataRepository::new(connection, mode == SessionMode::Exclusive);
        Ok(Self {
            mode,
            layout,
            repository,
            _lock: store_lock,
        })
    }

    pub fn mode(&self) -> SessionMode {
        self.mode
    }

    pub fn layout(&self) -> &StoreLayout {
        &self.layout
    }

    pub fn repository(&self) -> &MetadataRepository {
        &self.repository
    }

    pub fn repository_mut(&mut self) -> &mut MetadataRepository {
        &mut self.repository
    }

    pub fn pragmas(&self) -> StoreResult<PragmaSnapshot> {
        let mut snapshot = sqlite::pragma_snapshot(self.repository.connection())?;
        if self.mode == SessionMode::Shared {
            snapshot.journal_mode = sqlite::persistent_journal_mode(&self.layout.sqlite_path)?;
        }
        Ok(snapshot)
    }

    pub fn checkpoint_full(&self) -> StoreResult<()> {
        if self.mode != SessionMode::Exclusive {
            return Err(StoreError::ReadOnlySession);
        }
        sqlite::checkpoint_full(self.repository.connection())
    }
}

pub(crate) fn initialize_store(store: &LocalStore) -> StoreResult<()> {
    let session = LocalStoreSession::open(store, SessionMode::Exclusive)?;
    session.checkpoint_full()?;
    Ok(())
}
