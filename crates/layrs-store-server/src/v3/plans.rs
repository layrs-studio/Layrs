use super::{ByteRange, CapabilityActor, CapabilityCodec, V3StoreError, V3StoreResult};
use crate::v3::capability::digest_json;
use async_trait::async_trait;
use layrs_core::scm::{FileId, FileManifestId, ObjectId, RepoPath, SpaceId};
use layrs_sync::v3::{AuthorizationRoot, ObjectDescriptor, ObjectKind, ObjectReference};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use url::{Host, Url};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanLimits {
    pub max_requested_descriptors: usize,
    pub max_claims: usize,
    pub max_page_objects: usize,
}

impl Default for PlanLimits {
    fn default() -> Self {
        Self {
            max_requested_descriptors: 10_000,
            max_claims: 10_000,
            max_page_objects: 10_000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlanContext {
    pub actor: CapabilityActor,
    pub now_unix_seconds: u64,
    pub expires_at_unix_seconds: u64,
    /// Already parsed and allow-listed handler origin.
    pub transfer_base_url: TransferBaseUrl,
}

impl PlanContext {
    fn validate(&self) -> V3StoreResult<()> {
        if self.actor.authorization_epoch == 0
            || self.expires_at_unix_seconds <= self.now_unix_seconds
        {
            return Err(V3StoreError::InvalidCapability);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferBaseUrl(String);

impl TransferBaseUrl {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferUrlPolicy {
    allowed_https_origins: BTreeSet<String>,
    allowed_loopback_http_ports: BTreeSet<u16>,
}

impl TransferUrlPolicy {
    pub fn new<I, S, J>(
        allowed_https_origins: I,
        allowed_loopback_http_ports: J,
    ) -> V3StoreResult<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
        J: IntoIterator<Item = u16>,
    {
        let mut origins = BTreeSet::new();
        for candidate in allowed_https_origins {
            let url = parse_origin(candidate.as_ref())?;
            if url.scheme() != "https" {
                return Err(V3StoreError::InvalidCapability);
            }
            origins.insert(url.origin().ascii_serialization());
        }
        let ports = allowed_loopback_http_ports
            .into_iter()
            .collect::<BTreeSet<_>>();
        if ports.contains(&0) {
            return Err(V3StoreError::InvalidCapability);
        }
        Ok(Self {
            allowed_https_origins: origins,
            allowed_loopback_http_ports: ports,
        })
    }

    pub fn validate_base(&self, candidate: &str) -> V3StoreResult<TransferBaseUrl> {
        if candidate.len() > 4000 {
            return Err(V3StoreError::InvalidCapability);
        }
        let url = parse_origin(candidate)?;
        let origin = url.origin().ascii_serialization();
        match url.scheme() {
            "https" if self.allowed_https_origins.contains(&origin) => {}
            "http" if is_loopback_host(&url) => {
                let port = url
                    .port_or_known_default()
                    .ok_or(V3StoreError::InvalidCapability)?;
                if !self.allowed_loopback_http_ports.contains(&port) {
                    return Err(V3StoreError::InvalidCapability);
                }
            }
            _ => return Err(V3StoreError::InvalidCapability),
        }
        Ok(TransferBaseUrl(origin))
    }
}

fn parse_origin(candidate: &str) -> V3StoreResult<Url> {
    let url = Url::parse(candidate).map_err(|_| V3StoreError::InvalidCapability)?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.host().is_none()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(V3StoreError::InvalidCapability);
    }
    Ok(url)
}

fn is_loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

#[async_trait]
pub trait SpaceObjectAvailability: Send + Sync {
    /// This intentionally has no global-existence method: implementations must
    /// answer only after verified Space admission, preventing a dedup oracle.
    async fn is_verified_available(
        &self,
        space_id: &SpaceId,
        descriptor: &ObjectDescriptor,
    ) -> V3StoreResult<bool>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadCapability {
    pub actor: CapabilityActor,
    pub descriptor: ObjectDescriptor,
    pub method: &'static str,
    pub upload_url: String,
    pub token: String,
    pub expires_at_unix_seconds: u64,
    /// V3 uploads are atomic whole-object PUTs. Offset resume is forbidden.
    pub full_object_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectPlan {
    pub present_objects: Vec<ObjectDescriptor>,
    pub missing_objects: Vec<ObjectDescriptor>,
    pub upload_capabilities: Vec<UploadCapability>,
}

pub struct ObjectPlanBuilder<'a> {
    availability: &'a dyn SpaceObjectAvailability,
    capabilities: &'a CapabilityCodec,
    limits: PlanLimits,
}

impl<'a> ObjectPlanBuilder<'a> {
    pub fn new(
        availability: &'a dyn SpaceObjectAvailability,
        capabilities: &'a CapabilityCodec,
        limits: PlanLimits,
    ) -> Self {
        Self {
            availability,
            capabilities,
            limits,
        }
    }

    pub async fn build(
        &self,
        context: &PlanContext,
        requested: Vec<ObjectDescriptor>,
    ) -> V3StoreResult<ObjectPlan> {
        context.validate()?;
        if requested.len() > self.limits.max_requested_descriptors {
            return Err(V3StoreError::LimitExceeded {
                resource: "object plan descriptors",
                limit: self.limits.max_requested_descriptors as u64,
            });
        }
        let mut unique = BTreeSet::new();
        let mut identities = BTreeSet::new();
        for descriptor in &requested {
            if descriptor.kind == ObjectKind::AuthorizationSnapshot {
                return Err(V3StoreError::UnsupportedUploadKind);
            }
            if !identities.insert((descriptor.kind, descriptor.object_id.clone()))
                || !unique.insert(descriptor.clone())
            {
                return Err(V3StoreError::DuplicateRequestObject);
            }
        }

        let mut present_objects = Vec::new();
        let mut missing_objects = Vec::new();
        for descriptor in unique {
            if self
                .availability
                .is_verified_available(&context.actor.space_id, &descriptor)
                .await?
            {
                present_objects.push(descriptor);
            } else {
                missing_objects.push(descriptor);
            }
        }
        let mut upload_capabilities = Vec::with_capacity(missing_objects.len());
        for descriptor in missing_objects.iter().cloned() {
            let token = self.capabilities.issue_upload(
                context.actor.clone(),
                descriptor.clone(),
                context.expires_at_unix_seconds,
            )?;
            upload_capabilities.push(UploadCapability {
                actor: context.actor.clone(),
                descriptor,
                method: "PUT",
                upload_url: format!(
                    "{}/v3/transfers/uploads",
                    context.transfer_base_url.as_str()
                ),
                token,
                expires_at_unix_seconds: context.expires_at_unix_seconds,
                full_object_only: true,
            });
        }
        Ok(ObjectPlan {
            present_objects,
            missing_objects,
            upload_capabilities,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum FetchClaim {
    Object {
        object: ObjectReference,
        root: AuthorizationRoot,
        repo_path: Option<RepoPath>,
    },
    File {
        file_id: FileId,
        preferred_manifest_ids: Vec<FileManifestId>,
        root: AuthorizationRoot,
        repo_path: RepoPath,
    },
    AuthorizationSnapshot {
        authorization_snapshot_id: ObjectId,
        purpose: AuditPurpose,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditPurpose {
    Audit,
    Administration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeniedCode {
    NotReachable,
    PathDenied,
    AuditDenied,
    NotAvailable,
    InvalidRoot,
    InvalidPath,
    NoVerifiedManifest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileManifestSelection {
    pub file_id: FileId,
    pub file_manifest_id: FileManifestId,
    pub manifest_descriptor: ObjectDescriptor,
    pub chunk_descriptors: Vec<ObjectDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimResolution {
    Available {
        descriptors: Vec<ObjectDescriptor>,
        file_manifest_selection: Option<FileManifestSelection>,
    },
    Denied(DeniedCode),
}

#[async_trait]
pub trait RootedFetchAvailability: Send + Sync {
    /// Resolves reachability and exact path authorization within `space_id`.
    /// Physical CAS existence outside that Space must never affect the result.
    async fn resolve(
        &self,
        space_id: &SpaceId,
        claim: &FetchClaim,
    ) -> V3StoreResult<ClaimResolution>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadCapability {
    pub actor: CapabilityActor,
    pub claim: FetchClaim,
    pub claim_digest: ObjectId,
    pub descriptor: ObjectDescriptor,
    pub method: &'static str,
    pub download_url: String,
    pub token: String,
    pub resume_token: String,
    pub range: ByteRange,
    pub expires_at_unix_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeniedClaim {
    pub claim: FetchClaim,
    pub code: DeniedCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchPlan {
    pub file_manifest_selections: Vec<FileManifestSelection>,
    pub download_capabilities: Vec<DownloadCapability>,
    pub denied_claims: Vec<DeniedClaim>,
    pub next_page_cursor: Option<String>,
    pub has_more: bool,
}

pub struct FetchPlanBuilder<'a> {
    availability: &'a dyn RootedFetchAvailability,
    capabilities: &'a CapabilityCodec,
    limits: PlanLimits,
}

impl<'a> FetchPlanBuilder<'a> {
    pub fn new(
        availability: &'a dyn RootedFetchAvailability,
        capabilities: &'a CapabilityCodec,
        limits: PlanLimits,
    ) -> Self {
        Self {
            availability,
            capabilities,
            limits,
        }
    }

    pub async fn build(
        &self,
        context: &PlanContext,
        claims: Vec<FetchClaim>,
        page_cursor: Option<&str>,
        requested_max_objects: usize,
    ) -> V3StoreResult<FetchPlan> {
        context.validate()?;
        if claims.is_empty() || claims.len() > self.limits.max_claims {
            return Err(V3StoreError::LimitExceeded {
                resource: "fetch claims",
                limit: self.limits.max_claims as u64,
            });
        }
        let max_objects = requested_max_objects.min(self.limits.max_page_objects);
        if max_objects == 0 {
            return Err(V3StoreError::LimitExceeded {
                resource: "fetch page objects",
                limit: self.limits.max_page_objects as u64,
            });
        }

        let mut seen = BTreeSet::new();
        for claim in &claims {
            let canonical = serde_json::to_vec(claim)
                .map_err(|error| V3StoreError::Backend(error.to_string()))?;
            if !seen.insert(canonical) {
                return Err(V3StoreError::DuplicateRequestObject);
            }
            if let FetchClaim::File {
                preferred_manifest_ids,
                ..
            } = claim
            {
                if preferred_manifest_ids.len() > 128
                    || preferred_manifest_ids
                        .windows(2)
                        .any(|pair| pair[0] >= pair[1])
                {
                    return Err(V3StoreError::InvalidDescriptor(
                        "preferred manifests are not canonical".into(),
                    ));
                }
            }
        }
        let claims_digest = digest_json("layrs.sync.v3/fetch-claims", &claims)?;
        let start = match page_cursor {
            Some(cursor) => {
                self.capabilities
                    .verify_page(
                        cursor,
                        &context.actor,
                        &claims_digest,
                        context.actor.authorization_epoch,
                        context.now_unix_seconds,
                    )?
                    .next_index
            }
            None => 0,
        };
        if start > claims.len() {
            return Err(V3StoreError::InvalidCursor);
        }

        let mut file_manifest_selections = Vec::new();
        let mut download_capabilities = Vec::new();
        let mut denied_claims = Vec::new();
        let mut next_index = start;
        for (claim_index, claim) in claims.iter().enumerate().skip(start) {
            let resolution = self
                .availability
                .resolve(&context.actor.space_id, claim)
                .await?;
            let cost = match &resolution {
                ClaimResolution::Available { descriptors, .. } => descriptors.len().max(1),
                ClaimResolution::Denied(_) => 1,
            };
            let used = download_capabilities.len() + denied_claims.len();
            if used + cost > max_objects {
                if used == 0 {
                    return Err(V3StoreError::LimitExceeded {
                        resource: "single fetch claim expansion",
                        limit: max_objects as u64,
                    });
                }
                break;
            }

            match resolution {
                ClaimResolution::Denied(code) => denied_claims.push(DeniedClaim {
                    claim: claim.clone(),
                    code,
                }),
                ClaimResolution::Available {
                    mut descriptors,
                    file_manifest_selection,
                } => {
                    descriptors.sort();
                    if descriptors.windows(2).any(|pair| {
                        pair[0].kind == pair[1].kind && pair[0].object_id == pair[1].object_id
                    }) {
                        return Err(V3StoreError::DuplicateRequestObject);
                    }
                    if descriptors.is_empty() {
                        return Err(V3StoreError::InvalidDescriptor(
                            "authorized claim has no descriptors".into(),
                        ));
                    }
                    if let Some(selection) = file_manifest_selection {
                        file_manifest_selections.push(selection);
                    }
                    let claim_digest = digest_json("layrs.sync.v3/fetch-claim", claim)?;
                    for (descriptor_index, descriptor) in descriptors.into_iter().enumerate() {
                        let range = ByteRange::full(descriptor.encoded_byte_len)?;
                        let resume_token = format!(
                            "resume:{}:{claim_index}:{descriptor_index}",
                            context.actor.plan_id.as_str()
                        );
                        let token = self.capabilities.issue_download(
                            context.actor.clone(),
                            descriptor.clone(),
                            claim_digest.clone(),
                            range,
                            resume_token.clone(),
                            context.expires_at_unix_seconds,
                        )?;
                        download_capabilities.push(DownloadCapability {
                            actor: context.actor.clone(),
                            claim: claim.clone(),
                            claim_digest: claim_digest.clone(),
                            descriptor,
                            method: "GET",
                            download_url: format!(
                                "{}/v3/transfers/downloads",
                                context.transfer_base_url.as_str()
                            ),
                            token,
                            resume_token,
                            range,
                            expires_at_unix_seconds: context.expires_at_unix_seconds,
                        });
                    }
                }
            }
            next_index = claim_index + 1;
        }

        let has_more = next_index < claims.len();
        let next_page_cursor = if has_more {
            Some(self.capabilities.issue_page(
                context.actor.clone(),
                claims_digest,
                next_index,
                context.expires_at_unix_seconds,
            )?)
        } else {
            None
        };
        Ok(FetchPlan {
            file_manifest_selections,
            download_capabilities,
            denied_claims,
            next_page_cursor,
            has_more,
        })
    }
}
