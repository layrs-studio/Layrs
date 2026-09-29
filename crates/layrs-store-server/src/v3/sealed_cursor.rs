//! Opaque, principal-bound page cursors for listings whose resume position may
//! name an item the caller is not allowed to see (for example a Space skipped
//! by the authorized catalog). The position is therefore encrypted, not only
//! signed: deterministic authenticated encryption in the SIV style, with an
//! HMAC-SHA256 tag over (domain, principal, position) used as the synthetic IV
//! of a keyed BLAKE3 XOF keystream. A cursor opened by another principal, a
//! different listing or a tampered token fails authentication.
use super::{CapabilityCodec, V3StoreError, V3StoreResult};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use layrs_sync::v3::PrincipalId;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;
const TAG_BYTES: usize = 32;
/// Upper bound of an encoded cursor accepted from clients.
pub const SEALED_CURSOR_MAX_CHARS: usize = 2048;
const MAX_POSITION_BYTES: usize = (SEALED_CURSOR_MAX_CHARS / 4) * 3 - TAG_BYTES;

#[derive(Clone)]
pub(crate) struct SealedCursorCodec {
    domain: &'static str,
    mac_key: [u8; 32],
    encryption_key: [u8; 32],
}

impl SealedCursorCodec {
    /// `domain` separates listings: a cursor issued for one never opens another.
    pub(crate) fn new(capabilities: &CapabilityCodec, domain: &'static str) -> Self {
        Self {
            domain,
            mac_key: capabilities.derived_key("layrs.sync.v3/sealed-cursor-mac"),
            encryption_key: capabilities.derived_key("layrs.sync.v3/sealed-cursor-encryption"),
        }
    }

    pub(crate) fn seal(&self, principal: &PrincipalId, position: &[u8]) -> V3StoreResult<String> {
        if position.len() > MAX_POSITION_BYTES {
            return Err(V3StoreError::LimitExceeded {
                resource: "sealed cursor position",
                limit: MAX_POSITION_BYTES as u64,
            });
        }
        let tag = self.tag(principal, position)?;
        let mut sealed = Vec::with_capacity(TAG_BYTES + position.len());
        sealed.extend_from_slice(&tag);
        sealed.extend_from_slice(position);
        self.apply_keystream(&tag, &mut sealed[TAG_BYTES..]);
        Ok(URL_SAFE_NO_PAD.encode(sealed))
    }

    pub(crate) fn open(&self, principal: &PrincipalId, token: &str) -> V3StoreResult<Vec<u8>> {
        if token.is_empty() || token.len() > SEALED_CURSOR_MAX_CHARS {
            return Err(V3StoreError::InvalidCursor);
        }
        let sealed = URL_SAFE_NO_PAD
            .decode(token)
            .map_err(|_| V3StoreError::InvalidCursor)?;
        if sealed.len() < TAG_BYTES {
            return Err(V3StoreError::InvalidCursor);
        }
        let (tag, ciphertext) = sealed.split_at(TAG_BYTES);
        let mut position = ciphertext.to_vec();
        self.apply_keystream(tag, &mut position);
        self.mac(principal, &position)?
            .verify_slice(tag)
            .map_err(|_| V3StoreError::InvalidCursor)?;
        Ok(position)
    }

    fn mac(&self, principal: &PrincipalId, position: &[u8]) -> V3StoreResult<HmacSha256> {
        let mut mac = HmacSha256::new_from_slice(&self.mac_key)
            .map_err(|_| V3StoreError::InvalidCapability)?;
        for field in [self.domain.as_bytes(), principal.as_str().as_bytes()] {
            mac.update(&(field.len() as u64).to_be_bytes());
            mac.update(field);
        }
        mac.update(position);
        Ok(mac)
    }

    fn tag(&self, principal: &PrincipalId, position: &[u8]) -> V3StoreResult<[u8; TAG_BYTES]> {
        Ok(self
            .mac(principal, position)?
            .finalize()
            .into_bytes()
            .into())
    }

    fn apply_keystream(&self, tag: &[u8], bytes: &mut [u8]) {
        let mut keystream = vec![0_u8; bytes.len()];
        blake3::Hasher::new_keyed(&self.encryption_key)
            .update(self.domain.as_bytes())
            .update(&[0])
            .update(tag)
            .finalize_xof()
            .fill(&mut keystream);
        for (byte, key) in bytes.iter_mut().zip(keystream) {
            *byte ^= key;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codec(domain: &'static str) -> SealedCursorCodec {
        SealedCursorCodec::new(&CapabilityCodec::new([7_u8; 32]).unwrap(), domain)
    }
    fn principal(value: &str) -> PrincipalId {
        PrincipalId::parse(value).unwrap()
    }

    #[test]
    fn round_trip_hides_the_position() {
        let alice = principal("principal:alice");
        let token = codec("catalog").seal(&alice, b"secret project").unwrap();
        assert!(!token.contains("secret"));
        let decoded = URL_SAFE_NO_PAD.decode(&token).unwrap();
        assert!(!decoded.windows(6).any(|w| w == b"secret"));
        assert_eq!(
            codec("catalog").open(&alice, &token).unwrap(),
            b"secret project"
        );
    }

    #[test]
    fn other_principal_domain_key_or_tampering_is_rejected() {
        let alice = principal("principal:alice");
        let token = codec("catalog").seal(&alice, b"position").unwrap();
        assert!(
            codec("catalog")
                .open(&principal("principal:bob"), &token)
                .is_err()
        );
        assert!(codec("reviews").open(&alice, &token).is_err());
        let other_key =
            SealedCursorCodec::new(&CapabilityCodec::new([8_u8; 32]).unwrap(), "catalog");
        assert!(other_key.open(&alice, &token).is_err());
        let mut bytes = URL_SAFE_NO_PAD.decode(&token).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(
            codec("catalog")
                .open(&alice, &URL_SAFE_NO_PAD.encode(bytes))
                .is_err()
        );
        assert!(codec("catalog").open(&alice, "").is_err());
        assert!(codec("catalog").open(&alice, "not base64!").is_err());
        assert!(
            codec("catalog")
                .open(&alice, &"A".repeat(SEALED_CURSOR_MAX_CHARS + 1))
                .is_err()
        );
    }

    #[test]
    fn oversized_positions_are_refused_and_bounded_ones_fit() {
        let alice = principal("principal:alice");
        let fits = vec![b'x'; MAX_POSITION_BYTES];
        let token = codec("catalog").seal(&alice, &fits).unwrap();
        assert!(token.len() <= SEALED_CURSOR_MAX_CHARS);
        assert!(
            codec("catalog")
                .seal(&alice, &vec![b'x'; MAX_POSITION_BYTES + 1])
                .is_err()
        );
    }
}
