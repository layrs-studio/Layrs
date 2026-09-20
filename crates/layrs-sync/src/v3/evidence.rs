use layrs_core::scm::ObjectId;
use std::{error::Error, fmt, io::Read};

pub const EVIDENCE_IDENTITY_DOMAIN: &str = "layrs-evidence-v1";
pub const MAX_EVIDENCE_BYTE_LEN: u64 = 67_108_864;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceIdentity {
    pub object_id: ObjectId,
    pub byte_len: u64,
}

#[derive(Debug)]
pub enum EvidenceIdentityError {
    TooLarge {
        max: u64,
        actual: u64,
    },
    Io(std::io::Error),
    LengthMismatch {
        claimed: u64,
        actual: u64,
    },
    ClaimMismatch {
        claimed: ObjectId,
        computed: ObjectId,
    },
}
impl fmt::Display for EvidenceIdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { max, actual } => {
                write!(f, "Evidence is {actual} bytes; maximum is {max}")
            }
            Self::Io(error) => write!(f, "cannot read Evidence bytes: {error}"),
            Self::LengthMismatch { claimed, actual } => {
                write!(f, "Evidence length claim {claimed} differs from {actual}")
            }
            Self::ClaimMismatch { claimed, computed } => {
                write!(f, "Evidence claim {claimed} differs from {computed}")
            }
        }
    }
}
impl Error for EvidenceIdentityError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}
impl From<std::io::Error> for EvidenceIdentityError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

/// Incremental Evidence identity verifier. It never buffers the whole object.
pub struct EvidenceHasher {
    hasher: blake3::Hasher,
    byte_len: u64,
}
impl Default for EvidenceHasher {
    fn default() -> Self {
        Self::new()
    }
}
impl EvidenceHasher {
    pub fn new() -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(EVIDENCE_IDENTITY_DOMAIN.as_bytes());
        hasher.update(&[0]);
        Self {
            hasher,
            byte_len: 0,
        }
    }

    pub fn update(&mut self, bytes: &[u8]) -> Result<(), EvidenceIdentityError> {
        let actual = self
            .byte_len
            .checked_add(bytes.len() as u64)
            .unwrap_or(u64::MAX);
        if actual > MAX_EVIDENCE_BYTE_LEN {
            return Err(EvidenceIdentityError::TooLarge {
                max: MAX_EVIDENCE_BYTE_LEN,
                actual,
            });
        }
        self.hasher.update(bytes);
        self.byte_len = actual;
        Ok(())
    }

    pub fn finalize(self) -> EvidenceIdentity {
        EvidenceIdentity {
            object_id: ObjectId::parse(format!("blake3:{}", self.hasher.finalize()))
                .expect("BLAKE3 is a valid ObjectId"),
            byte_len: self.byte_len,
        }
    }
}

pub fn hash_evidence_reader<R: Read>(
    mut reader: R,
) -> Result<EvidenceIdentity, EvidenceIdentityError> {
    let mut verifier = EvidenceHasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        verifier.update(&buffer[..read])?;
    }
    Ok(verifier.finalize())
}

pub fn verify_evidence_reader<R: Read>(
    reader: R,
    claimed_object_id: &ObjectId,
    claimed_byte_len: u64,
) -> Result<EvidenceIdentity, EvidenceIdentityError> {
    if claimed_byte_len > MAX_EVIDENCE_BYTE_LEN {
        return Err(EvidenceIdentityError::TooLarge {
            max: MAX_EVIDENCE_BYTE_LEN,
            actual: claimed_byte_len,
        });
    }
    let identity = hash_evidence_reader(reader)?;
    if identity.byte_len != claimed_byte_len {
        return Err(EvidenceIdentityError::LengthMismatch {
            claimed: claimed_byte_len,
            actual: identity.byte_len,
        });
    }
    if &identity.object_id != claimed_object_id {
        return Err(EvidenceIdentityError::ClaimMismatch {
            claimed: claimed_object_id.clone(),
            computed: identity.object_id,
        });
    }
    Ok(identity)
}
