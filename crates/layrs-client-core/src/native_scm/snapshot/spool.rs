use layrs_core::scm::{Chunk, ChunkId};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    sync::Mutex,
};

/// Prepared capture content is accessible only through this private capability.
/// No pathname is exported or trusted when accepting a prepared mutation.
#[derive(Debug)]
pub(crate) struct SnapshotChunkSpool {
    /// Created with the first chunk: a capture that reads nothing new (most
    /// captures) creates no file.
    file: Mutex<Option<File>>,
    entries: BTreeMap<ChunkId, (u64, usize)>,
    length: u64,
}

impl SnapshotChunkSpool {
    pub(crate) fn new() -> io::Result<Self> {
        Ok(Self {
            file: Mutex::new(None),
            entries: BTreeMap::new(),
            length: 0,
        })
    }
    pub(crate) fn insert(&mut self, chunk: &Chunk) -> io::Result<()> {
        if self.entries.contains_key(chunk.id()) {
            return Ok(());
        }
        let next = self
            .length
            .checked_add(chunk.bytes().len() as u64)
            .ok_or_else(|| io::Error::other("capture spool length overflow"))?;
        // Bounded like a batch (MAX_BATCH_OBJECTS); captures spool only chunks
        // new to the Space, initial captures every chunk of the checkout. The
        // spool's bytes are bounded by the disk only (ADR 0027).
        if self.entries.len() >= 262_144
            || chunk.bytes().len() as u64 > layrs_core::scm::MAX_FIXED_CHUNK_BYTES
        {
            return Err(io::Error::other("capture spool chunk budget exceeded"));
        }
        let file = self
            .file
            .get_mut()
            .map_err(|_| io::Error::other("capture spool poisoned"))?;
        let file = match file {
            Some(file) => file,
            None => file.insert(layrs_store_local::private_spool_file()?),
        };
        file.seek(SeekFrom::Start(self.length))?;
        file.write_all(chunk.bytes())?;
        self.entries
            .insert(chunk.id().clone(), (self.length, chunk.bytes().len()));
        self.length = next;
        Ok(())
    }
    pub(crate) fn descriptors(&self) -> Vec<(ChunkId, u64)> {
        self.entries
            .iter()
            .map(|(id, (_, len))| (id.clone(), *len as u64))
            .collect()
    }
    /// Known (already admitted) chunks are never spooled by a capture.
    pub(crate) fn contains(&self, id: &ChunkId) -> bool {
        self.entries.contains_key(id)
    }
    pub(crate) fn read(&self, id: &ChunkId) -> layrs_store::scm::Result<Vec<u8>> {
        let result = (|| -> io::Result<Vec<u8>> {
            let (offset, len) = self
                .entries
                .get(id)
                .ok_or_else(|| io::Error::other("prepared chunk absent"))?;
            let mut file = self
                .file
                .lock()
                .map_err(|_| io::Error::other("capture spool poisoned"))?;
            let file = file
                .as_mut()
                .ok_or_else(|| io::Error::other("prepared chunk absent"))?;
            file.seek(SeekFrom::Start(*offset))?;
            let mut bytes = vec![0; *len];
            file.read_exact(&mut bytes)?;
            let chunk = Chunk::new(bytes).map_err(io::Error::other)?;
            if chunk.id() != id {
                return Err(io::Error::other("prepared chunk identity changed"));
            }
            Ok(chunk.bytes().to_vec())
        })();
        result.map_err(|e| layrs_store::scm::StoreError::Corruption {
            message: e.to_string(),
        })
    }
    pub(super) fn retain(&mut self, ids: &std::collections::BTreeSet<ChunkId>) {
        self.entries.retain(|id, _| ids.contains(id));
    }
    /// Forgets every chunk `known` answers true for (already admitted to the
    /// target Space): such a chunk is neither re-read nor re-installed.
    pub(crate) fn forget_known<E>(
        &mut self,
        mut known: impl FnMut(&ChunkId) -> std::result::Result<bool, E>,
    ) -> std::result::Result<(), E> {
        let mut forgotten = Vec::new();
        for id in self.entries.keys() {
            if known(id)? {
                forgotten.push(id.clone());
            }
        }
        for id in forgotten {
            self.entries.remove(&id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_spool_rehashes_private_content_and_rejects_truncation() {
        let chunk = Chunk::new(b"prepared bytes".to_vec()).unwrap();
        let mut spool = SnapshotChunkSpool::new().unwrap();
        spool.insert(&chunk).unwrap();
        spool.insert(&chunk).unwrap();
        assert_eq!(spool.descriptors().len(), 1);
        assert_eq!(spool.read(chunk.id()).unwrap(), chunk.bytes());
        let file = spool.file.get_mut().unwrap().as_mut().unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(b"!").unwrap();
        assert!(spool.read(chunk.id()).is_err());
        spool
            .file
            .get_mut()
            .unwrap()
            .as_mut()
            .unwrap()
            .set_len(0)
            .unwrap();
        assert!(spool.read(chunk.id()).is_err());
    }
}
