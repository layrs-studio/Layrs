use super::*;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::projection) enum Phase {
    Declared,
    Allocated,
    Verified,
    Publishing,
    Published,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum Piece {
    Chunk(u32),
    Manifest,
}
impl Piece {
    pub(super) fn name(self) -> String {
        match self {
            Self::Chunk(n) => format!("chunk-{n}"),
            Self::Manifest => "manifest".into(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Active {
    pub piece: Piece,
    pub inode: String,
    pub from_cas: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct ExactRecord {
    schema_version: String,
    pub binding: ExactBinding,
    pub manifest_id: FileManifestId,
    pub file_id: FileId,
    pub bytes: u64,
    pub directory: String,
    pub directory_identity: Option<String>,
    pub phase: Phase,
    pub spool_cursor: u32,
    pub publish_cursor: u32,
    pub(super) active: Option<Active>,
    charged_chunks: String,
    resident_chunks: String,
    pub charged_manifest: bool,
    pub debit_bytes: u64,
}
impl ExactRecord {
    pub(super) fn declared(
        binding: ExactBinding,
        manifest: &FileManifest,
        directory: String,
    ) -> Self {
        Self {
            schema_version: "layrs.shared-exact-installation.v1".into(),
            binding,
            manifest_id: manifest.id().clone(),
            file_id: manifest.file_id().clone(),
            bytes: manifest.byte_length(),
            directory,
            directory_identity: None,
            phase: Phase::Declared,
            spool_cursor: 0,
            publish_cursor: 0,
            active: None,
            charged_chunks: "0".repeat(256),
            resident_chunks: "0".repeat(256),
            charged_manifest: false,
            debit_bytes: PREPARE_CHARGE,
        }
    }
    pub(super) fn encode(&self, manifest: &FileManifest) -> Result<Vec<u8>> {
        self.validate(manifest)?;
        let bytes = canonical_json(self).map_err(converted)?;
        if bytes.len() > 8192 {
            return Err(invalid("exact installation payload exceeds bound"));
        }
        Ok(bytes)
    }
    pub(super) fn decode(bytes: &[u8], manifest: &FileManifest) -> Result<Self> {
        if bytes.is_empty() || bytes.len() > 8192 {
            return Err(invalid("exact installation payload exceeds bound"));
        }
        let record: Self = serde_json::from_slice(bytes).map_err(converted)?;
        if record.encode(manifest)? != bytes {
            return Err(invalid("exact installation journal is not canonical"));
        }
        Ok(record)
    }
    pub(super) fn verified(&self) -> bool {
        matches!(
            self.phase,
            Phase::Verified | Phase::Publishing | Phase::Published
        )
    }
    pub(super) fn charged_chunk(&self, ordinal: usize) -> bool {
        self.charged_chunks
            .as_bytes()
            .get(ordinal / 4)
            .is_some_and(|b| digit(*b) & (1 << (ordinal % 4)) != 0)
    }
    pub(super) fn resident_chunk(&self, ordinal: usize) -> bool {
        self.resident_chunks
            .as_bytes()
            .get(ordinal / 4)
            .is_some_and(|b| digit(*b) & (1 << (ordinal % 4)) != 0)
    }
    pub(super) fn mark_resident(&mut self, ordinal: usize) -> Result<()> {
        if ordinal >= 1024 {
            return Err(invalid("exact resident ordinal exceeds bound"));
        }
        let mut bits = self.resident_chunks.clone().into_bytes();
        let index = ordinal / 4;
        bits[index] = b"0123456789abcdef"[(digit(bits[index]) | (1 << (ordinal % 4))) as usize];
        self.resident_chunks = String::from_utf8(bits).map_err(converted)?;
        Ok(())
    }
    pub(super) fn charge_chunk(&mut self, ordinal: usize, bytes: u64) -> Result<()> {
        if ordinal >= 1024 || self.charged_chunk(ordinal) {
            return Err(invalid("exact chunk debit duplicated"));
        }
        let mut bits = self.charged_chunks.clone().into_bytes();
        let index = ordinal / 4;
        bits[index] = b"0123456789abcdef"[(digit(bits[index]) | (1 << (ordinal % 4))) as usize];
        self.charged_chunks = String::from_utf8(bits).map_err(converted)?;
        self.debit_bytes = self
            .debit_bytes
            .checked_add(bytes)
            .ok_or_else(|| invalid("exact debit overflow"))?;
        Ok(())
    }
    pub(super) fn validate(&self, manifest: &FileManifest) -> Result<()> {
        self.binding.validate()?;
        let payload = manifest_bytes(manifest)?;
        let count = manifest.chunks().len() as u32;
        if self.schema_version != "layrs.shared-exact-installation.v1"
            || self.manifest_id != *manifest.id()
            || self.file_id != *manifest.file_id()
            || self.bytes != manifest.byte_length()
            || self.directory.is_empty()
            || self.directory.len() > 128
            || !self
                .directory
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'-')
            || self.spool_cursor > count
            || self.publish_cursor > count
            || !hex(&self.charged_chunks, 256)
            || !hex(&self.resident_chunks, 256)
        {
            return Err(invalid(
                "exact installation journal binding or cursor differs",
            ));
        }
        let allocated = self.phase != Phase::Declared;
        if self.directory_identity.is_some() != allocated
            || self
                .directory_identity
                .as_ref()
                .is_some_and(|value| value.is_empty() || value.len() > 512 || value.contains('\0'))
            || self.phase == Phase::Declared
                && (self.spool_cursor != 0 || self.publish_cursor != 0 || self.active.is_some())
            || self.verified() && self.spool_cursor != count
            || !matches!(self.phase, Phase::Publishing | Phase::Published)
                && self.publish_cursor != 0
            || self.phase == Phase::Published
                && (self.publish_cursor != count || self.active.is_some())
        {
            return Err(invalid("exact installation phase projection differs"));
        }
        if let Some(active) = &self.active {
            if active.inode.is_empty() || active.inode.len() > 512 || active.inode.contains('\0') {
                return Err(invalid("exact active inode exceeds bound"));
            }
            match (self.phase, active.piece) {
                (Phase::Allocated, Piece::Chunk(n)) if n == self.spool_cursor && n < count => (),
                (Phase::Allocated, Piece::Manifest) if self.spool_cursor == count => (),
                (Phase::Publishing, Piece::Chunk(n)) if n == self.publish_cursor && n < count => (),
                (Phase::Publishing, Piece::Manifest) if self.publish_cursor == count => (),
                _ => return Err(invalid("exact active object differs from phase cursor")),
            }
            if active.from_cas && active.piece == Piece::Manifest {
                return Err(invalid("exact manifest spool cannot be a resident input"));
            }
            if let Piece::Chunk(n) = active.piece {
                if self.phase == Phase::Publishing
                    && active.from_cas != self.resident_chunk(n as usize)
                {
                    return Err(invalid("exact publication resident binding differs"));
                }
            }
        }
        let mut debit = PREPARE_CHARGE;
        if self.verified() {
            debit = debit
                .checked_add(payload.len() as u64)
                .ok_or_else(|| invalid("exact debit overflow"))?;
        }
        let mut charged = BTreeSet::new();
        for n in 0..1024 {
            if self.resident_chunk(n)
                && (n >= self.spool_cursor as usize
                    || n >= manifest.chunks().len()
                    || self.charged_chunk(n))
            {
                return Err(invalid("exact resident bitmap differs from spooled prefix"));
            }
            if !self.charged_chunk(n) {
                continue;
            }
            if n >= self.publish_cursor as usize
                || n >= manifest.chunks().len()
                || !charged.insert(manifest.chunks()[n].chunk_id())
            {
                return Err(invalid(
                    "exact charged chunk does not belong to published prefix",
                ));
            }
            debit = debit
                .checked_add(manifest.chunks()[n].byte_length())
                .ok_or_else(|| invalid("exact debit overflow"))?;
        }
        if self.charged_manifest {
            if self.phase != Phase::Published {
                return Err(invalid("exact manifest charged before publication"));
            }
            debit = debit
                .checked_add(payload.len() as u64)
                .ok_or_else(|| invalid("exact debit overflow"))?;
        }
        if debit != self.debit_bytes || debit > self.binding.quota_bytes {
            return Err(invalid("exact installation debit projection differs"));
        }
        Ok(())
    }
}
fn digit(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => 0,
    }
}

pub(super) fn manifest_bytes(manifest: &FileManifest) -> Result<Vec<u8>> {
    if manifest.byte_length() > 1024 * 1024 * 1024
        || manifest.chunks().len() > 1024
        || manifest
            .chunks()
            .iter()
            .any(|chunk| chunk.byte_length() == 0 || chunk.byte_length() > 1024 * 1024)
    {
        return Err(invalid("exact manifest exceeds supported reader bounds"));
    }
    let bytes = canonical_json(manifest).map_err(converted)?;
    if bytes.len() > 262144 {
        return Err(invalid("exact manifest payload exceeds bound"));
    }
    let decoded: FileManifest = serde_json::from_slice(&bytes).map_err(converted)?;
    if decoded != *manifest {
        return Err(invalid("exact manifest roundtrip differs"));
    }
    Ok(bytes)
}
