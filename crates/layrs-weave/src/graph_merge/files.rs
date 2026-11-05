//! Resident File content for one planning call.
//!
//! In-memory callers keep supplying every closure File in `MergeObjects.files`
//! and observe the historical behavior: each such File is verified against its
//! FileId and charged to `max_input_bytes` once, during closure validation.
//!
//! A lazy [`MergeFileSource`] instead proves presence without reading content
//! (for a local Store: the File is reachable from an admitted Tree, and durable
//! admission is closed by induction). Bytes are loaded only where planning
//! actually inspects content (multi-side content leaves, semantic payloads,
//! block resolution), verified against the FileId on every load, cached so each
//! File is loaded at most once per plan, and charged to the same input budget.
//! `max_input_bytes` therefore bounds resident bytes, never unread closure Files.
use super::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Lazily loaded immutable File content.
pub trait MergeFileSource {
    /// Presence claim made without reading content. Return true only when the
    /// exact File is durably available to [`MergeFileSource::load`], e.g.
    /// because it is reachable from a Tree admitted to the caller's Space.
    fn contains(&self, id: &FileId) -> bool;
    /// Exact content bytes. The planner re-derives and checks the FileId of
    /// every loaded value; a mismatch rejects the plan.
    fn load(&self, id: &FileId) -> std::result::Result<Vec<u8>, String>;
}

/// Borrowed in-memory bytes or a shared lazily loaded value.
pub(crate) enum FileBytes<'a> {
    Borrowed(&'a [u8]),
    Shared(Rc<Vec<u8>>),
}

impl std::ops::Deref for FileBytes<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Self::Borrowed(bytes) => bytes,
            Self::Shared(bytes) => bytes.as_slice(),
        }
    }
}

pub(crate) struct Files<'a> {
    memory: &'a BTreeMap<FileId, Vec<u8>>,
    source: Option<&'a dyn MergeFileSource>,
    loaded: RefCell<BTreeMap<FileId, Rc<Vec<u8>>>>,
    /// In-memory Files already verified and charged.
    resident: RefCell<BTreeSet<FileId>>,
    bytes: Cell<usize>,
    limit: usize,
}

impl<'a> Files<'a> {
    pub(crate) fn new(
        memory: &'a BTreeMap<FileId, Vec<u8>>,
        source: Option<&'a dyn MergeFileSource>,
        limit: usize,
    ) -> Self {
        Self {
            memory,
            source,
            loaded: RefCell::new(BTreeMap::new()),
            resident: RefCell::new(BTreeSet::new()),
            bytes: Cell::new(0),
            limit,
        }
    }

    /// Unbounded in-memory view for standalone payload validation.
    pub(crate) fn memory_only(memory: &'a BTreeMap<FileId, Vec<u8>>) -> Self {
        Self::new(memory, None, usize::MAX)
    }

    /// Charge resident input bytes (Steps, metadata, File content).
    pub(crate) fn charge(&self, bytes: usize) -> MergeResult<()> {
        let total = self
            .bytes
            .get()
            .checked_add(bytes)
            .ok_or(GraphMergeError::Limit("input bytes"))?;
        if total > self.limit {
            return Err(GraphMergeError::Limit("input bytes"));
        }
        self.bytes.set(total);
        Ok(())
    }

    /// Closure validation of a File entry. In-memory content is verified and
    /// charged exactly as before; source-only content is proven present
    /// without being read or charged.
    pub(crate) fn require(&self, id: &FileId, mismatch: &'static str) -> MergeResult<()> {
        if let Some(bytes) = self.memory.get(id) {
            return self.admit_memory(id, bytes, mismatch);
        }
        if self.loaded.borrow().contains_key(id) || self.source.is_some_and(|s| s.contains(id)) {
            return Ok(());
        }
        Err(GraphMergeError::Missing(id.to_string()))
    }

    fn admit_memory(&self, id: &FileId, bytes: &[u8], mismatch: &'static str) -> MergeResult<()> {
        if self.resident.borrow().contains(id) {
            return Ok(());
        }
        self.charge(bytes.len())?;
        if FileId::from_bytes(bytes) != *id {
            return Err(invalid(mismatch));
        }
        self.resident.borrow_mut().insert(id.clone());
        Ok(())
    }

    /// Verified content; loaded from the source at most once per plan.
    pub(crate) fn get(&self, id: &FileId, mismatch: &'static str) -> MergeResult<FileBytes<'a>> {
        if let Some(bytes) = self.memory.get(id) {
            self.admit_memory(id, bytes, mismatch)?;
            return Ok(FileBytes::Borrowed(bytes));
        }
        if let Some(bytes) = self.loaded.borrow().get(id) {
            return Ok(FileBytes::Shared(bytes.clone()));
        }
        let source = self
            .source
            .filter(|source| source.contains(id))
            .ok_or_else(|| GraphMergeError::Missing(id.to_string()))?;
        let bytes = source
            .load(id)
            .map_err(|error| invalid(format!("File {id} could not be loaded: {error}")))?;
        self.charge(bytes.len())?;
        if FileId::from_bytes(&bytes) != *id {
            return Err(invalid(mismatch));
        }
        let bytes = Rc::new(bytes);
        self.loaded.borrow_mut().insert(id.clone(), bytes.clone());
        Ok(FileBytes::Shared(bytes))
    }
}
