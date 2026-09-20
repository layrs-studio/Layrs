//! Private temporary chunk storage; every read verifies its immutable identity.
use super::*;
use std::cell::RefCell;
use std::io::{Seek, SeekFrom};

#[derive(Default)]
pub(super) struct ChunkSpool {
    file: RefCell<Option<std::fs::File>>,
    entries: BTreeMap<ChunkId, (u64, usize)>,
    length: u64,
}

impl ChunkSpool {
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn contains(&self, id: &ChunkId) -> bool {
        self.entries.contains_key(id)
    }
    pub(super) fn descriptors(&self) -> Vec<(ChunkId, u64)> {
        self.entries
            .iter()
            .map(|(id, (_, length))| (id.clone(), *length as u64))
            .collect()
    }

    pub(super) fn insert(&mut self, chunk: &Chunk) -> RemoteResult<()> {
        if self.contains(chunk.id()) {
            return Ok(());
        }
        let length = chunk.bytes().len();
        let next = self
            .length
            .checked_add(length as u64)
            .filter(|length| *length <= MAX_STREAM_BYTES)
            .ok_or(NativeRemoteError::Invalid("chunk spool exceeds limit"))?;
        let file = self.file.get_mut();
        if file.is_none() {
            *file = Some(download::temporary_file()?);
        }
        let file = file.as_mut().expect("private spool opened");
        file.seek(SeekFrom::Start(self.length))?;
        file.write_all(chunk.bytes())?;
        self.entries
            .insert(chunk.id().clone(), (self.length, length));
        self.length = next;
        Ok(())
    }

    pub(super) fn get(&self, id: &ChunkId) -> RemoteResult<Chunk> {
        let &(offset, length) = self
            .entries
            .get(id)
            .ok_or(NativeRemoteError::Invalid("missing spooled chunk"))?;
        if length as u64 > MAX_OBJECT {
            return Err(NativeRemoteError::Invalid("spooled chunk exceeds limit"));
        }
        let mut file = self.file.borrow_mut();
        let file = file
            .as_mut()
            .ok_or(NativeRemoteError::Invalid("missing chunk spool"))?;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; length];
        file.read_exact(&mut bytes)?;
        let chunk = Chunk::new(bytes)?;
        if chunk.id() != id {
            return Err(NativeRemoteError::Invalid("chunk spool identity mismatch"));
        }
        Ok(chunk)
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = RemoteResult<CanonicalObject>> + '_ {
        self.entries
            .keys()
            .map(|id| self.get(id).map(CanonicalObject::ChunkRaw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_spool_rechecks_bytes_and_preserves_distinct_chunks() {
        let mut spool = ChunkSpool::default();
        let first = Chunk::new(b"first".to_vec()).unwrap();
        let second = Chunk::new(b"second".to_vec()).unwrap();
        spool.insert(&first).unwrap();
        spool.insert(&second).unwrap();
        spool.insert(&first).unwrap();
        assert_eq!(spool.len(), 2);
        assert_eq!(spool.length, 11);
        assert_eq!(spool.get(second.id()).unwrap(), second);
        assert_eq!(spool.get(first.id()).unwrap(), first);
        let file = spool.file.get_mut().as_mut().unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(b"wrong").unwrap();
        assert!(spool.get(first.id()).is_err());
        assert_eq!(spool.get(second.id()).unwrap(), second);
        spool.file.get_mut().as_mut().unwrap().set_len(6).unwrap();
        assert!(spool.get(second.id()).is_err());
    }
}
