//! Bounded, undeltified Git pack v2 construction. Git remains the only object
//! database writer through `index-pack --stdin --strict`.
use super::*;
use flate2::{Compression, write::ZlibEncoder};
use sha1::{Digest, Sha1};
use std::io::{self, Write};

pub(crate) struct Pack {
    output: BoundedBytes,
    limits: GitBridgeLimits,
    objects: BTreeMap<[u8; 20], [u8; 32]>,
    raw_bytes: usize,
}

pub(crate) struct ObjectBuffer(BoundedBytes);
impl ObjectBuffer {
    pub(crate) fn new(cap: usize, initial: &[u8]) -> Result<Self> {
        let mut buffer = Self(BoundedBytes {
            bytes: Vec::new(),
            cap,
        });
        buffer.extend_from_slice(initial)?;
        Ok(buffer)
    }
    pub(crate) fn extend_from_slice(&mut self, bytes: &[u8]) -> Result<()> {
        self.0.write_all(bytes).map_err(|error| {
            if error.kind() == io::ErrorKind::FileTooLarge {
                BridgeError::Limit("Git rebuilt object bytes")
            } else {
                error.into()
            }
        })
    }
    pub(crate) fn push(&mut self, byte: u8) -> Result<()> {
        self.extend_from_slice(&[byte])
    }
}
impl std::ops::Deref for ObjectBuffer {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        &self.0.bytes
    }
}
impl Pack {
    pub(crate) fn new(limits: GitBridgeLimits) -> Result<Self> {
        let cap = limits
            .max_total_bytes
            .checked_sub(20)
            .filter(|cap| *cap >= 12)
            .ok_or(BridgeError::Limit("Git pack bytes"))?;
        let mut output = BoundedBytes {
            bytes: Vec::new(),
            cap,
        };
        output.write_all(b"PACK\0\0\0\x02\0\0\0\0")?;
        Ok(Self {
            output,
            limits,
            objects: BTreeMap::new(),
            raw_bytes: 0,
        })
    }
    pub(crate) fn insert(&mut self, kind: &str, bytes: &[u8]) -> Result<String> {
        let tag = match kind {
            "commit" => 1,
            "tree" => 2,
            "blob" => 3,
            "tag" => 4,
            _ => return Err(invalid("unsupported pack object kind")),
        };
        if bytes.len() > self.limits.max_object_bytes {
            return Err(BridgeError::Limit("Git pack object bytes"));
        }
        let header = format!("{kind} {}\0", bytes.len());
        let mut hash = Sha1::new();
        hash.update(header.as_bytes());
        hash.update(bytes);
        let oid: [u8; 20] = hash.finalize().into();
        let mut stronger = sha2::Sha256::new();
        stronger.update(header.as_bytes());
        stronger.update(bytes);
        let fingerprint: [u8; 32] = stronger.finalize().into();
        let text = hex_bytes(&oid);
        if let Some(old) = self.objects.get(&oid) {
            if old != &fingerprint {
                return Err(invalid("distinct Git objects collide under SHA1"));
            }
            return Ok(text);
        }
        if self.objects.len() >= self.limits.max_objects || self.objects.len() >= u32::MAX as usize
        {
            return Err(BridgeError::Limit("Git pack objects"));
        }
        let total = self
            .raw_bytes
            .checked_add(bytes.len())
            .ok_or(BridgeError::Limit("Git pack input bytes"))?;
        if total > self.limits.max_total_bytes {
            return Err(BridgeError::Limit("Git pack input bytes"));
        }
        let mut size = bytes.len();
        let mut byte = (tag << 4) | (size as u8 & 15);
        size >>= 4;
        while size != 0 {
            self.output.write_all(&[byte | 128]).map_err(pack_error)?;
            byte = (size as u8) & 127;
            size >>= 7;
        }
        self.output.write_all(&[byte]).map_err(pack_error)?;
        let mut compressor = ZlibEncoder::new(&mut self.output, Compression::fast());
        compressor.write_all(bytes).map_err(pack_error)?;
        compressor.finish().map_err(pack_error)?;
        self.raw_bytes = total;
        self.objects.insert(oid, fingerprint);
        Ok(text)
    }
    pub(crate) fn finish(mut self) -> Result<Vec<u8>> {
        self.output.bytes[8..12].copy_from_slice(&(self.objects.len() as u32).to_be_bytes());
        let digest = Sha1::digest(&self.output.bytes);
        self.output.cap = self.limits.max_total_bytes;
        self.output.write_all(&digest).map_err(pack_error)?;
        Ok(self.output.bytes)
    }
}
fn pack_error(error: io::Error) -> BridgeError {
    if error.kind() == io::ErrorKind::FileTooLarge {
        BridgeError::Limit("Git pack bytes")
    } else {
        error.into()
    }
}
struct BoundedBytes {
    bytes: Vec<u8>,
    cap: usize,
}
impl Write for BoundedBytes {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        let length = self
            .bytes
            .len()
            .checked_add(input.len())
            .filter(|length| *length <= self.cap)
            .ok_or_else(|| io::Error::new(io::ErrorKind::FileTooLarge, "Git pack byte limit"))?;
        if length > self.bytes.capacity() {
            let capacity = length
                .max(self.bytes.capacity().saturating_mul(2))
                .min(self.cap);
            self.bytes
                .try_reserve_exact(capacity - self.bytes.len())
                .map_err(io::Error::other)?;
        }
        self.bytes.extend_from_slice(input);
        Ok(input.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn oid_bytes(text: &str) -> Result<[u8; 20]> {
    hex_id(text)?;
    if text.len() != 40 {
        return Err(invalid("pack export requires SHA1 object IDs"));
    }
    let mut bytes = [0; 20];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).map_err(invalid)?;
    }
    Ok(bytes)
}

pub(crate) fn hex_bytes(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| {
            [
                DIGITS[(byte >> 4) as usize] as char,
                DIGITS[(byte & 15) as usize] as char,
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pack_header_checksum_deduplication_and_budgets_are_exact() {
        let mut pack = Pack::new(GitBridgeLimits::default()).unwrap();
        assert_eq!(
            pack.insert("blob", b"test content\n").unwrap(),
            "d670460b4b4aece5915caf5c68d12f560a9fe3e4"
        );
        pack.insert("blob", b"test content\n").unwrap();
        let bytes = pack.finish().unwrap();
        assert_eq!(&bytes[..12], b"PACK\0\0\0\x02\0\0\0\x01");
        assert_eq!(
            &bytes[bytes.len() - 20..],
            Sha1::digest(&bytes[..bytes.len() - 20]).as_slice()
        );
        let mut pack = Pack::new(GitBridgeLimits {
            max_objects: 0,
            ..GitBridgeLimits::default()
        })
        .unwrap();
        assert!(matches!(
            pack.insert("blob", b"x"),
            Err(BridgeError::Limit(_))
        ));
        let mut pack = Pack::new(GitBridgeLimits {
            max_object_bytes: 1,
            ..GitBridgeLimits::default()
        })
        .unwrap();
        assert!(matches!(
            pack.insert("blob", b"xx"),
            Err(BridgeError::Limit(_))
        ));
        let mut pack = Pack::new(GitBridgeLimits {
            max_total_bytes: 32,
            ..GitBridgeLimits::default()
        })
        .unwrap();
        assert!(matches!(
            pack.insert("blob", b"x"),
            Err(BridgeError::Limit(_))
        ));
    }
}
