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
    file: Mutex<File>,
    entries: BTreeMap<ChunkId, (u64, usize)>,
    length: u64,
}

impl SnapshotChunkSpool {
    pub(crate) fn new() -> io::Result<Self> {
        let file = layrs_store_local::private_spool_file()?;
        Ok(Self {
            file: Mutex::new(file),
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
            .filter(|n| *n <= 4 * 1024 * 1024 * 1024)
            .ok_or_else(|| io::Error::other("capture spool exceeds4GiB"))?;
        if self.entries.len() >= 16_384 || chunk.bytes().len() > 1024 * 1024 {
            return Err(io::Error::other("capture spool chunk budget exceeded"));
        }
        let file = self
            .file
            .get_mut()
            .map_err(|_| io::Error::other("capture spool poisoned"))?;
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
        let file = spool.file.get_mut().unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(b"!").unwrap();
        assert!(spool.read(chunk.id()).is_err());
        spool.file.get_mut().unwrap().set_len(0).unwrap();
        assert!(spool.read(chunk.id()).is_err());
    }
}
