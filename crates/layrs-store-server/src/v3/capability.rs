use super::{ByteRange, V3StoreError, V3StoreResult};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use layrs_core::scm::{ObjectId, SpaceId};
use layrs_sync::v3::{ClientId, ObjectDescriptor, PlanId, PrincipalId, RequestId};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityActor {
    pub principal_id: PrincipalId,
    pub authorization_epoch: u64,
    pub space_id: SpaceId,
    pub client_id: ClientId,
    pub request_id: RequestId,
    pub plan_id: PlanId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedUploadGrant {
    pub actor: CapabilityActor,
    pub descriptor: ObjectDescriptor,
    pub expires_at_unix_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedDownloadGrant {
    pub actor: CapabilityActor,
    pub descriptor: ObjectDescriptor,
    pub claim_digest: ObjectId,
    pub range: ByteRange,
    pub resume_token: String,
    pub expires_at_unix_seconds: u64,
}

pub struct DownloadVerification<'a> {
    pub token: &'a str,
    pub expected_actor: &'a CapabilityActor,
    pub expected_descriptor: &'a ObjectDescriptor,
    pub expected_claim_digest: &'a ObjectId,
    pub expected_range: ByteRange,
    pub expected_resume_token: &'a str,
    pub current_authorization_epoch: u64,
    pub now_unix_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedPageGrant {
    pub actor: CapabilityActor,
    pub claims_digest: ObjectId,
    pub next_index: usize,
}

#[derive(Debug, Clone)]
pub struct CapabilityCodec {
    signing_key: [u8; 32],
}

impl CapabilityCodec {
    /// Verifies signature and expiry before looking up the persisted binding.
    /// The service MUST subsequently recheck membership, epoch and plan state.
    pub fn authenticate_upload(&self, token: &str, now: u64) -> V3StoreResult<VerifiedUploadGrant> {
        let claims = self.decode(token)?;
        let CapabilityBinding::Upload { descriptor, .. } = &claims.binding else {
            return Err(V3StoreError::CapabilityBindingMismatch);
        };
        self.verify_upload(
            token,
            &claims.actor,
            descriptor,
            claims.actor.authorization_epoch,
            now,
        )
    }

    /// Verifies signature and expiry; persisted authorization is still required.
    pub fn authenticate_download(
        &self,
        token: &str,
        now: u64,
    ) -> V3StoreResult<VerifiedDownloadGrant> {
        let claims = self.decode(token)?;
        let CapabilityBinding::Download {
            descriptor,
            claim_digest,
            range,
            resume_token,
        } = &claims.binding
        else {
            return Err(V3StoreError::CapabilityBindingMismatch);
        };
        self.verify_download(DownloadVerification {
            token,
            expected_actor: &claims.actor,
            expected_descriptor: descriptor,
            expected_claim_digest: claim_digest,
            expected_range: *range,
            expected_resume_token: resume_token,
            current_authorization_epoch: claims.actor.authorization_epoch,
            now_unix_seconds: now,
        })
    }

    /// Independent key for another server-sealed token family. Distinct
    /// contexts never share key material with capability signatures.
    pub(crate) fn derived_key(&self, context: &'static str) -> [u8; 32] {
        blake3::derive_key(context, &self.signing_key)
    }

    pub fn new(signing_key: impl AsRef<[u8]>) -> V3StoreResult<Self> {
        let key = signing_key.as_ref();
        if key.len() < 32 {
            return Err(V3StoreError::InvalidCapability);
        }
        let mut signing_key = [0_u8; 32];
        signing_key.copy_from_slice(&blake3::derive_key("layrs.sync.v3/capability-key", key));
        Ok(Self { signing_key })
    }

    pub fn issue_upload(
        &self,
        actor: CapabilityActor,
        descriptor: ObjectDescriptor,
        expires_at_unix_seconds: u64,
    ) -> V3StoreResult<String> {
        let descriptor_digest = digest_json("layrs.sync.v3/descriptor", &descriptor)?;
        self.encode(&CapabilityClaims {
            version: 1,
            actor,
            expires_at_unix_seconds,
            binding: CapabilityBinding::Upload {
                descriptor,
                descriptor_digest,
            },
        })
    }

    pub fn verify_upload(
        &self,
        token: &str,
        expected_actor: &CapabilityActor,
        expected_descriptor: &ObjectDescriptor,
        current_authorization_epoch: u64,
        now_unix_seconds: u64,
    ) -> V3StoreResult<VerifiedUploadGrant> {
        let claims = self.decode(token)?;
        verify_common(
            &claims,
            expected_actor,
            current_authorization_epoch,
            now_unix_seconds,
        )?;
        let CapabilityBinding::Upload {
            descriptor,
            descriptor_digest,
        } = claims.binding
        else {
            return Err(V3StoreError::CapabilityBindingMismatch);
        };
        if &descriptor != expected_descriptor
            || descriptor_digest != digest_json("layrs.sync.v3/descriptor", &descriptor)?
        {
            return Err(V3StoreError::CapabilityBindingMismatch);
        }
        Ok(VerifiedUploadGrant {
            actor: claims.actor,
            descriptor,
            expires_at_unix_seconds: claims.expires_at_unix_seconds,
        })
    }

    pub fn issue_download(
        &self,
        actor: CapabilityActor,
        descriptor: ObjectDescriptor,
        claim_digest: ObjectId,
        range: ByteRange,
        resume_token: impl Into<String>,
        expires_at_unix_seconds: u64,
    ) -> V3StoreResult<String> {
        if range.end_exclusive > descriptor.encoded_byte_len {
            return Err(V3StoreError::InvalidRange);
        }
        self.encode(&CapabilityClaims {
            version: 1,
            actor,
            expires_at_unix_seconds,
            binding: CapabilityBinding::Download {
                descriptor,
                claim_digest,
                range,
                resume_token: resume_token.into(),
            },
        })
    }

    pub fn verify_download(
        &self,
        verification: DownloadVerification<'_>,
    ) -> V3StoreResult<VerifiedDownloadGrant> {
        let claims = self.decode(verification.token)?;
        verify_common(
            &claims,
            verification.expected_actor,
            verification.current_authorization_epoch,
            verification.now_unix_seconds,
        )?;
        let CapabilityBinding::Download {
            descriptor,
            claim_digest,
            range,
            resume_token,
        } = claims.binding
        else {
            return Err(V3StoreError::CapabilityBindingMismatch);
        };
        if &descriptor != verification.expected_descriptor
            || &claim_digest != verification.expected_claim_digest
            || range != verification.expected_range
            || resume_token != verification.expected_resume_token
        {
            return Err(V3StoreError::CapabilityBindingMismatch);
        }
        Ok(VerifiedDownloadGrant {
            actor: claims.actor,
            descriptor,
            claim_digest,
            range,
            resume_token,
            expires_at_unix_seconds: claims.expires_at_unix_seconds,
        })
    }

    pub(crate) fn issue_page(
        &self,
        actor: CapabilityActor,
        claims_digest: ObjectId,
        next_index: usize,
        expires_at_unix_seconds: u64,
    ) -> V3StoreResult<String> {
        self.encode(&CapabilityClaims {
            version: 1,
            actor,
            expires_at_unix_seconds,
            binding: CapabilityBinding::Page {
                claims_digest,
                next_index,
            },
        })
    }

    pub(crate) fn verify_page(
        &self,
        token: &str,
        expected_actor: &CapabilityActor,
        expected_claims_digest: &ObjectId,
        current_authorization_epoch: u64,
        now_unix_seconds: u64,
    ) -> V3StoreResult<VerifiedPageGrant> {
        let claims = self.decode(token)?;
        verify_common(
            &claims,
            expected_actor,
            current_authorization_epoch,
            now_unix_seconds,
        )
        .map_err(|_| V3StoreError::InvalidCursor)?;
        let CapabilityBinding::Page {
            claims_digest,
            next_index,
        } = claims.binding
        else {
            return Err(V3StoreError::InvalidCursor);
        };
        if &claims_digest != expected_claims_digest {
            return Err(V3StoreError::InvalidCursor);
        }
        Ok(VerifiedPageGrant {
            actor: claims.actor,
            claims_digest,
            next_index,
        })
    }

    fn encode(&self, claims: &CapabilityClaims) -> V3StoreResult<String> {
        let payload =
            serde_json::to_vec(claims).map_err(|error| V3StoreError::Backend(error.to_string()))?;
        let payload = URL_SAFE_NO_PAD.encode(payload);
        let mut mac = HmacSha256::new_from_slice(&self.signing_key)
            .map_err(|_| V3StoreError::InvalidCapability)?;
        mac.update(b"layrs.sync.v3/capability\0");
        mac.update(payload.as_bytes());
        Ok(format!(
            "v1.{payload}.{}",
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        ))
    }

    fn decode(&self, token: &str) -> V3StoreResult<CapabilityClaims> {
        if token.len() > 4096 {
            return Err(V3StoreError::InvalidCapability);
        }
        let mut pieces = token.split('.');
        if pieces.next() != Some("v1") {
            return Err(V3StoreError::InvalidCapability);
        }
        let payload = pieces.next().ok_or(V3StoreError::InvalidCapability)?;
        let signature = pieces.next().ok_or(V3StoreError::InvalidCapability)?;
        if pieces.next().is_some() {
            return Err(V3StoreError::InvalidCapability);
        }
        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| V3StoreError::InvalidCapability)?;
        let mut mac = HmacSha256::new_from_slice(&self.signing_key)
            .map_err(|_| V3StoreError::InvalidCapability)?;
        mac.update(b"layrs.sync.v3/capability\0");
        mac.update(payload.as_bytes());
        mac.verify_slice(&signature)
            .map_err(|_| V3StoreError::InvalidCapability)?;
        let payload = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| V3StoreError::InvalidCapability)?;
        let claims: CapabilityClaims =
            serde_json::from_slice(&payload).map_err(|_| V3StoreError::InvalidCapability)?;
        if claims.version != 1 || claims.actor.authorization_epoch == 0 {
            return Err(V3StoreError::InvalidCapability);
        }
        Ok(claims)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapabilityClaims {
    version: u8,
    actor: CapabilityActor,
    expires_at_unix_seconds: u64,
    binding: CapabilityBinding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CapabilityBinding {
    Upload {
        descriptor: ObjectDescriptor,
        descriptor_digest: ObjectId,
    },
    Download {
        descriptor: ObjectDescriptor,
        claim_digest: ObjectId,
        range: ByteRange,
        resume_token: String,
    },
    Page {
        claims_digest: ObjectId,
        next_index: usize,
    },
}

fn verify_common(
    claims: &CapabilityClaims,
    expected_actor: &CapabilityActor,
    current_authorization_epoch: u64,
    now_unix_seconds: u64,
) -> V3StoreResult<()> {
    if claims.expires_at_unix_seconds <= now_unix_seconds {
        return Err(V3StoreError::ExpiredCapability);
    }
    if claims.actor.authorization_epoch != current_authorization_epoch {
        return Err(V3StoreError::AuthorizationEpochChanged);
    }
    if &claims.actor != expected_actor {
        return Err(V3StoreError::CapabilityBindingMismatch);
    }
    Ok(())
}

pub(crate) fn digest_json(domain: &str, value: &impl Serialize) -> V3StoreResult<ObjectId> {
    let bytes =
        serde_json::to_vec(value).map_err(|error| V3StoreError::Backend(error.to_string()))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain.as_bytes());
    hasher.update(&[0]);
    hasher.update(&bytes);
    ObjectId::parse(format!("blake3:{}", hasher.finalize().to_hex()))
        .map_err(|error| V3StoreError::Backend(error.to_string()))
}
