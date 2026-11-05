use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use layrs_core::scm::Timestamp;
use layrs_sync::v3::{AuthorizationSnapshotPurpose, DeniedClaimCode, FetchClaim as WireFetchClaim};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
pub(super) fn to_wire_claim(claim: &FetchClaim) -> WireFetchClaim {
    match claim {
        FetchClaim::Object {
            object,
            root,
            repo_path,
        } => WireFetchClaim::Object {
            object: object.clone(),
            root: root.clone(),
            repo_path: match repo_path {
                Some(path) => Nullable::some(path.clone()),
                None => Nullable::null(),
            },
        },
        FetchClaim::File {
            file_id,
            preferred_manifest_ids,
            root,
            repo_path,
        } => WireFetchClaim::File {
            file_id: file_id.clone(),
            preferred_manifest_ids: preferred_manifest_ids.clone(),
            root: root.clone(),
            repo_path: repo_path.clone(),
        },
        FetchClaim::AuthorizationSnapshot {
            authorization_snapshot_id,
            purpose,
        } => WireFetchClaim::AuthorizationSnapshot {
            authorization_snapshot_id: authorization_snapshot_id.clone(),
            purpose: match purpose {
                super::super::AuditPurpose::Audit => AuthorizationSnapshotPurpose::Audit,
                super::super::AuditPurpose::Administration => {
                    AuthorizationSnapshotPurpose::Administration
                }
            },
        },
    }
}

pub(super) fn to_wire_denied_code(code: DeniedCode) -> DeniedClaimCode {
    match code {
        DeniedCode::NotReachable => DeniedClaimCode::NotReachable,
        DeniedCode::PathDenied => DeniedClaimCode::PathDenied,
        DeniedCode::AuditDenied => DeniedClaimCode::AuditDenied,
        DeniedCode::NotAvailable => DeniedClaimCode::NotAvailable,
        DeniedCode::InvalidRoot => DeniedClaimCode::InvalidRoot,
        DeniedCode::InvalidPath => DeniedClaimCode::InvalidPath,
        DeniedCode::NoVerifiedManifest => DeniedClaimCode::NoVerifiedManifest,
    }
}

pub(super) fn denied_code(code: DeniedCode) -> &'static str {
    match code {
        DeniedCode::NotReachable => "not_reachable",
        DeniedCode::PathDenied => "path_denied",
        DeniedCode::AuditDenied => "audit_denied",
        DeniedCode::NotAvailable => "not_available",
        DeniedCode::InvalidRoot => "invalid_root",
        DeniedCode::InvalidPath => "invalid_path",
        DeniedCode::NoVerifiedManifest => "no_verified_manifest",
    }
}

pub(super) fn claim_kind(claim: &FetchClaim) -> &'static str {
    match claim {
        FetchClaim::Object { .. } => "object",
        FetchClaim::File { .. } => "file",
        FetchClaim::AuthorizationSnapshot { .. } => "authorization_snapshot",
    }
}

pub(super) fn timestamp(unix_seconds: u64) -> V3StoreResult<Timestamp> {
    let value = OffsetDateTime::from_unix_timestamp(i64_from_u64(unix_seconds)?)
        .map_err(|_| V3StoreError::InvalidCapability)?
        .format(&Rfc3339)
        .map_err(|error| V3StoreError::Backend(error.to_string()))?;
    Timestamp::parse(value).map_err(|error| V3StoreError::Backend(error.to_string()))
}

pub(super) fn capability_id(
    kind: &str,
    plan_id: &layrs_sync::v3::PlanId,
    ordinal: usize,
) -> String {
    let digest = blake3::hash(format!("{kind}\0{}\0{ordinal}", plan_id.as_str()).as_bytes());
    format!("capability:{}", digest.to_hex())
}

pub(super) fn token_digest(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}
