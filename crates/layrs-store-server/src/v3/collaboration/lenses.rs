//! Immutable Lens packets and Space pins. No client supplied path is opened on
//! the server: packet paths only index bytes already present in a bounded body.
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use layrs_lens_sdk::{
    LensManifest, LensPin, WasiEntrypointRuntime, sha256_digest, validate_lens_id,
};
use ring::signature::{ED25519, UnparsedPublicKey};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

const MAX_PACKET_BYTES: usize = 8 * 1024 * 1024;
const MAX_FILES: usize = 8;
const MAX_PINS: usize = 32;
const MAX_ASSOCIATIONS: usize = 64;

/// Operator controlled verification keys. The JSON value is a keyId -> base64
/// raw Ed25519 public key map, never metadata from the uploaded packet.
#[derive(Clone, Debug, Default)]
pub struct TrustedLensKeys(BTreeMap<String, Vec<u8>>);

impl TrustedLensKeys {
    pub fn parse(raw: Option<&str>) -> V3StoreResult<Self> {
        let Some(raw) = raw else {
            return Ok(Self::default());
        };
        if raw.len() > 8192 {
            return Err(invalid("lens_trust_roots_too_large"));
        }
        let keys: BTreeMap<String, String> =
            serde_json::from_str(raw).map_err(|_| invalid("invalid_lens_trust_roots"))?;
        if keys.len() > 32 {
            return Err(invalid("too_many_lens_trust_roots"));
        }
        let mut decoded = BTreeMap::new();
        for (id, encoded) in keys {
            if id.is_empty()
                || id.len() > 128
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._:-#".contains(&b))
            {
                return Err(invalid("invalid_lens_trust_key_id"));
            }
            let bytes = STANDARD
                .decode(encoded.as_bytes())
                .map_err(|_| invalid("invalid_lens_trust_key"))?;
            if bytes.len() != 32 || STANDARD.encode(&bytes) != encoded {
                return Err(invalid("invalid_lens_trust_key"));
            }
            decoded.insert(id, bytes);
        }
        Ok(Self(decoded))
    }

    pub fn verify(&self, manifest: &LensManifest) -> V3StoreResult<bool> {
        let Some(signature) = manifest
            .bundle
            .as_ref()
            .and_then(|bundle| bundle.signature.as_ref())
        else {
            return Ok(false);
        };
        if signature.algorithm != "ed25519" {
            return Err(invalid("unsupported_lens_signature_algorithm"));
        }
        let key = self
            .0
            .get(&signature.key_id)
            .ok_or_else(|| invalid("unknown_lens_signing_key"))?;
        let encoded = signature
            .signature
            .strip_prefix("base64:")
            .ok_or_else(|| invalid("invalid_lens_signature"))?;
        let bytes = STANDARD
            .decode(encoded.as_bytes())
            .map_err(|_| invalid("invalid_lens_signature"))?;
        if bytes.len() != 64 || STANDARD.encode(&bytes) != encoded {
            return Err(invalid("invalid_lens_signature"));
        }
        // Stable signing contract: Ed25519 over the ASCII sha256:<hex> identity
        // digest, after canonical JCS hashing of the unsigned manifest.
        UnparsedPublicKey::new(&ED25519, key)
            .verify(signature.signed_digest.as_bytes(), &bytes)
            .map_err(|_| invalid("invalid_lens_signature"))?;
        Ok(true)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LensPacketFile {
    pub path: String,
    pub bytes_base64: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LensPacket {
    pub manifest: LensManifest,
    pub files: Vec<LensPacketFile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LensAssociation {
    pub extension: String,
    pub lens_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitialLensConfig {
    #[serde(default)]
    pub pins: Vec<LensPin>,
    #[serde(default)]
    pub associations: Vec<LensAssociation>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LensConfigRequest {
    pub expected_revision: u64,
    pub pins: Vec<LensPin>,
    pub associations: Vec<LensAssociation>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LensCatalogItem {
    pub manifest: LensManifest,
    pub pin: LensPin,
    pub source: String,
    pub state: String,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_count: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceLensConfig {
    pub revision: u64,
    pub pins: Vec<LensPin>,
    pub associations: Vec<LensAssociation>,
    pub can_manage: bool,
    pub available_catalog: Vec<LensCatalogItem>,
}

pub struct VerifiedLensPacket {
    pub pin: LensPin,
    pub manifest_jcs: String,
    pub packet_jcs: String,
}

fn invalid(code: &'static str) -> V3StoreError {
    V3StoreError::InvalidDescriptor(code.into())
}

/// Validates every supplied byte and the canonical manifest identity before a
/// packet can become available. Operator trust is checked separately at install.
pub fn verify_lens_packet(packet: &LensPacket) -> V3StoreResult<VerifiedLensPacket> {
    let manifest = &packet.manifest;
    manifest
        .validate()
        .map_err(|_| invalid("invalid_lens_manifest"))?;
    let bundle = manifest
        .bundle
        .as_ref()
        .ok_or_else(|| invalid("lens_bundle_required"))?;
    let mut identity = manifest.clone();
    identity.digest = None;
    identity.bundle.as_mut().expect("bundle checked").signature = None;
    let identity_jcs = serde_jcs::to_vec(&identity).map_err(|_| invalid("lens_identity_encode"))?;
    let expected = sha256_digest(&identity_jcs);
    if manifest.digest.as_deref() != Some(expected.as_str()) {
        return Err(invalid("lens_identity_digest_mismatch"));
    }
    let pin = manifest
        .try_pin()
        .map_err(|_| invalid("invalid_lens_pin"))?
        .ok_or_else(|| invalid("lens_digest_required"))?;
    if packet.files.is_empty() || packet.files.len() > MAX_FILES {
        return Err(invalid("invalid_lens_file_count"));
    }
    let mut declared = BTreeMap::<&str, &str>::new();
    let points = &bundle.entrypoints;
    if let Some(entry) = &points.viewer {
        if manifest.viewer.component != "layrs.viewer.worker.v1" || entry.export != "render" {
            return Err(invalid("unsupported_lens_viewer_abi"));
        }
        if declared
            .insert(entry.path.as_str(), entry.digest.as_str())
            .is_some()
        {
            return Err(invalid("duplicate_lens_entrypoint_path"));
        }
    }
    if points.analyzer.is_some() {
        return Err(invalid("lens_analyzer_runtime_unavailable"));
    }
    if let Some(entry) = &points.reconcile {
        if entry.runtime != WasiEntrypointRuntime::Wasm || entry.export != "layrs_reconcile" {
            return Err(invalid("unsupported_lens_reconcile_abi"));
        }
        if declared
            .insert(entry.path.as_str(), entry.digest.as_str())
            .is_some()
        {
            return Err(invalid("duplicate_lens_entrypoint_path"));
        }
    }
    if declared.len() != packet.files.len() {
        return Err(invalid("lens_file_set_mismatch"));
    }
    let mut seen = BTreeSet::new();
    let mut total = 0usize;
    for file in &packet.files {
        let digest = declared
            .get(file.path.as_str())
            .ok_or_else(|| invalid("lens_file_not_declared"))?;
        if !seen.insert(file.path.as_str())
            || file.bytes_base64.len() > MAX_PACKET_BYTES * 4 / 3 + 8
        {
            return Err(invalid("lens_file_duplicate_or_oversize"));
        }
        let bytes = STANDARD
            .decode(file.bytes_base64.as_bytes())
            .map_err(|_| invalid("lens_file_base64"))?;
        if STANDARD.encode(&bytes) != file.bytes_base64 || sha256_digest(&bytes) != *digest {
            return Err(invalid("lens_file_digest_mismatch"));
        }
        total = total
            .checked_add(bytes.len())
            .ok_or_else(|| invalid("lens_packet_oversize"))?;
        if total > MAX_PACKET_BYTES {
            return Err(invalid("lens_packet_oversize"));
        }
        if points
            .viewer
            .as_ref()
            .is_some_and(|entry| entry.path == file.path)
        {
            std::str::from_utf8(&bytes).map_err(|_| invalid("lens_viewer_not_utf8"))?;
        }
        if points
            .reconcile
            .as_ref()
            .is_some_and(|entry| entry.path == file.path)
        {
            // The strict ABI validator in layrs-weave also rejects imports,
            // start functions and wrong exported signatures.
            layrs_weave::wasm_host::validate_guest_module(&bytes, digest)
                .map_err(|_| invalid("invalid_lens_reconcile_module"))?;
        }
    }
    let manifest_jcs =
        serde_jcs::to_string(manifest).map_err(|_| invalid("lens_manifest_encode"))?;
    let packet_jcs = serde_jcs::to_string(packet).map_err(|_| invalid("lens_packet_encode"))?;
    if packet_jcs.len() > 12 * 1024 * 1024 {
        return Err(invalid("lens_packet_oversize"));
    }
    Ok(VerifiedLensPacket {
        pin,
        manifest_jcs,
        packet_jcs,
    })
}

fn normalized(mut config: InitialLensConfig) -> V3StoreResult<InitialLensConfig> {
    if config.pins.len() > MAX_PINS || config.associations.len() > MAX_ASSOCIATIONS {
        return Err(invalid("lens_configuration_too_large"));
    }
    for pin in &config.pins {
        pin.validate().map_err(|_| invalid("invalid_lens_pin"))?;
    }
    config.pins.sort_by(|a, b| a.lens_id.cmp(&b.lens_id));
    if config
        .pins
        .windows(2)
        .any(|pair| pair[0].lens_id == pair[1].lens_id)
    {
        return Err(invalid("duplicate_lens_pin"));
    }
    for association in &config.associations {
        validate_lens_id(&association.lens_id).map_err(|_| invalid("invalid_lens_association"))?;
        if association.extension.len() < 2
            || association.extension.len() > 32
            || !association.extension.starts_with('.')
            || !association.extension[1..]
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || !config
                .pins
                .iter()
                .any(|pin| pin.lens_id == association.lens_id)
        {
            return Err(invalid("invalid_lens_association"));
        }
    }
    config
        .associations
        .sort_by(|a, b| a.extension.cmp(&b.extension));
    if config
        .associations
        .windows(2)
        .any(|pair| pair[0].extension == pair[1].extension)
    {
        return Err(invalid("duplicate_lens_extension"));
    }
    Ok(config)
}

async fn active_principal(
    tx: &mut Transaction<'_, Postgres>,
    principal: &PrincipalContext,
    administrator: bool,
) -> V3StoreResult<()> {
    let kind: Option<String> = sqlx::query_scalar("SELECT principal_kind FROM principals WHERE principal_id=$1 AND deployment_id=$2 AND subject_digest=$3 AND status='active' FOR SHARE")
        .bind(principal.principal_id.as_str()).bind(&principal.deployment_id).bind(&principal.subject_digest)
        .fetch_optional(&mut **tx).await.map_err(pg_error)?;
    if !kind.as_deref().is_some_and(|kind| {
        if administrator {
            kind == "bootstrap_admin"
        } else {
            kind == "bootstrap_admin" || kind == "account"
        }
    }) {
        return Err(V3StoreError::InvalidCapability);
    }
    Ok(())
}

async fn install_packet_tx(
    tx: &mut Transaction<'_, Postgres>,
    deployment: &str,
    packet: &LensPacket,
    verified: &VerifiedLensPacket,
    origin: &str,
    installed_by: Option<&str>,
) -> V3StoreResult<()> {
    sqlx::query("INSERT INTO layrs_collaboration.lens_versions(deployment_id,lens_id,version,digest,manifest_jcs,packet_jcs,origin,installed_by) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT DO NOTHING")
        .bind(deployment).bind(&verified.pin.lens_id).bind(&verified.pin.version).bind(&verified.pin.digest)
        .bind(&verified.manifest_jcs).bind(&verified.packet_jcs).bind(origin).bind(installed_by)
        .execute(&mut **tx).await.map_err(pg_error)?;
    let row = sqlx::query("SELECT digest,manifest_jcs,packet_jcs,origin,state FROM layrs_collaboration.lens_versions WHERE deployment_id=$1 AND lens_id=$2 AND version=$3 FOR SHARE")
        .bind(deployment).bind(&packet.manifest.id).bind(&packet.manifest.version)
        .fetch_one(&mut **tx).await.map_err(pg_error)?;
    if row.get::<String, _>("digest") != verified.pin.digest
        || row.get::<String, _>("manifest_jcs") != verified.manifest_jcs
        || row.get::<String, _>("packet_jcs") != verified.packet_jcs
        || row.get::<String, _>("origin") != origin
    {
        return Err(invalid("lens_version_immutable_conflict"));
    }
    Ok(())
}

/// Called during checked server installation, after its deployment row exists.
pub async fn install_builtin_lens_packets(
    tx: &mut Transaction<'_, Postgres>,
    deployment: &str,
    packets: &[LensPacket],
) -> V3StoreResult<()> {
    for packet in packets {
        let verified = verify_lens_packet(packet)?;
        install_packet_tx(tx, deployment, packet, &verified, "builtin", None).await?;
    }
    Ok(())
}

async fn deployment_for_space(
    tx: &mut Transaction<'_, Postgres>,
    space: &SpaceId,
) -> V3StoreResult<String> {
    sqlx::query_scalar("SELECT w.deployment_id FROM spaces s JOIN workspaces w USING(workspace_id) WHERE s.space_id=$1")
        .bind(space.as_str()).fetch_optional(&mut **tx).await.map_err(pg_error)?
        .ok_or(V3StoreError::NotFound)
}

async fn validate_pins(
    tx: &mut Transaction<'_, Postgres>,
    space: &SpaceId,
    deployment: &str,
    config: &InitialLensConfig,
) -> V3StoreResult<()> {
    let mut manifests = BTreeMap::new();
    for pin in &config.pins {
        let row = sqlx::query("SELECT manifest_jcs,state FROM layrs_collaboration.lens_versions WHERE deployment_id=$1 AND lens_id=$2 AND version=$3 AND digest=$4 FOR SHARE")
            .bind(deployment).bind(&pin.lens_id).bind(&pin.version).bind(&pin.digest)
            .fetch_optional(&mut **tx).await.map_err(pg_error)?.ok_or_else(|| invalid("lens_pin_unavailable"))?;
        let state: String = row.get("state");
        if state == "revoked" {
            return Err(invalid("revoked_lens_cannot_be_pinned"));
        }
        if state != "active" {
            let already: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM layrs_collaboration.space_lens_pins WHERE space_id=$1 AND deployment_id=$2 AND lens_id=$3 AND version=$4 AND digest=$5)")
                .bind(space.as_str()).bind(deployment).bind(&pin.lens_id).bind(&pin.version).bind(&pin.digest)
                .fetch_one(&mut **tx).await.map_err(pg_error)?;
            if !already {
                return Err(invalid("retired_lens_cannot_be_newly_pinned"));
            }
        }
        let manifest: LensManifest = serde_json::from_str(&row.get::<String, _>("manifest_jcs"))
            .map_err(|_| V3StoreError::CorruptObject("stored Lens manifest".into()))?;
        manifests.insert(pin.lens_id.as_str(), manifest);
    }
    for association in &config.associations {
        let manifest = manifests
            .get(association.lens_id.as_str())
            .ok_or_else(|| invalid("association_without_pin"))?;
        if !manifest
            .analyzer
            .file_extensions
            .iter()
            .any(|ext| ext == association.extension.trim_start_matches('.'))
        {
            return Err(invalid("lens_extension_not_supported"));
        }
    }
    Ok(())
}

async fn write_config(
    tx: &mut Transaction<'_, Postgres>,
    space: &SpaceId,
    deployment: &str,
    config: &InitialLensConfig,
    revision: i64,
    updated_by: Option<&str>,
) -> V3StoreResult<()> {
    let pins_jcs = serde_jcs::to_string(&config.pins).map_err(|_| invalid("lens_config_encode"))?;
    let associations_jcs =
        serde_jcs::to_string(&config.associations).map_err(|_| invalid("lens_config_encode"))?;
    sqlx::query("INSERT INTO layrs_collaboration.space_lens_configs(space_id,revision,pins_jcs,associations_jcs,updated_by) VALUES($1,$2,$3,$4,$5) ON CONFLICT(space_id) DO UPDATE SET revision=EXCLUDED.revision,pins_jcs=EXCLUDED.pins_jcs,associations_jcs=EXCLUDED.associations_jcs,updated_by=EXCLUDED.updated_by,updated_at=transaction_timestamp()")
        .bind(space.as_str()).bind(revision).bind(pins_jcs).bind(associations_jcs).bind(updated_by)
        .execute(&mut **tx).await.map_err(pg_error)?;
    sqlx::query("DELETE FROM layrs_collaboration.space_lens_pins WHERE space_id=$1")
        .bind(space.as_str())
        .execute(&mut **tx)
        .await
        .map_err(pg_error)?;
    for pin in &config.pins {
        sqlx::query("INSERT INTO layrs_collaboration.space_lens_pins(space_id,deployment_id,lens_id,version,digest) VALUES($1,$2,$3,$4,$5)")
            .bind(space.as_str()).bind(deployment).bind(&pin.lens_id).bind(&pin.version).bind(&pin.digest)
            .execute(&mut **tx).await.map_err(pg_error)?;
        sqlx::query("INSERT INTO layrs_collaboration.space_lens_history(space_id,deployment_id,lens_id,version,digest) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
            .bind(space.as_str()).bind(deployment).bind(&pin.lens_id).bind(&pin.version).bind(&pin.digest)
            .execute(&mut **tx).await.map_err(pg_error)?;
    }
    Ok(())
}

pub async fn validate_and_insert_initial_lens_config(
    tx: &mut Transaction<'_, Postgres>,
    space: &SpaceId,
    config: &InitialLensConfig,
) -> V3StoreResult<()> {
    let config = normalized(config.clone())?;
    let deployment = deployment_for_space(tx, space).await?;
    validate_pins(tx, space, &deployment, &config).await?;
    write_config(tx, space, &deployment, &config, 1, None).await
}

pub async fn initial_lens_config_matches(
    tx: &mut Transaction<'_, Postgres>,
    space: &SpaceId,
    config: &InitialLensConfig,
) -> V3StoreResult<bool> {
    let expected = normalized(config.clone())?;
    let row = sqlx::query("SELECT pins_jcs,associations_jcs FROM layrs_collaboration.space_lens_configs WHERE space_id=$1")
        .bind(space.as_str()).fetch_optional(&mut **tx).await.map_err(pg_error)?;
    let Some(row) = row else {
        return Ok(expected.pins.is_empty() && expected.associations.is_empty());
    };
    let pins: Vec<LensPin> = serde_json::from_str(&row.get::<String, _>("pins_jcs"))
        .map_err(|_| V3StoreError::CorruptObject("stored Lens pins".into()))?;
    let associations: Vec<LensAssociation> =
        serde_json::from_str(&row.get::<String, _>("associations_jcs"))
            .map_err(|_| V3StoreError::CorruptObject("stored Lens associations".into()))?;
    Ok(expected.pins == pins && expected.associations == associations)
}

async fn catalog_tx(
    tx: &mut Transaction<'_, Postgres>,
    deployment: &str,
    active_only: bool,
    reveal_usage: bool,
) -> V3StoreResult<Vec<LensCatalogItem>> {
    let rows = sqlx::query("SELECT v.lens_id,v.version,v.digest,v.manifest_jcs,v.origin,v.state,COUNT(p.space_id) AS usage_count FROM layrs_collaboration.lens_versions v LEFT JOIN layrs_collaboration.space_lens_pins p ON p.deployment_id=v.deployment_id AND p.lens_id=v.lens_id AND p.version=v.version AND p.digest=v.digest WHERE v.deployment_id=$1 AND ($2=false OR v.state='active') GROUP BY v.deployment_id,v.lens_id,v.version,v.digest ORDER BY v.lens_id,v.version")
        .bind(deployment).bind(active_only).fetch_all(&mut **tx).await.map_err(pg_error)?;
    rows.into_iter()
        .map(|row| {
            let manifest: LensManifest =
                serde_json::from_str(&row.get::<String, _>("manifest_jcs"))
                    .map_err(|_| V3StoreError::CorruptObject("stored Lens manifest".into()))?;
            let state: String = row.get("state");
            Ok(LensCatalogItem {
                pin: LensPin::try_new(
                    row.get::<String, _>("lens_id"),
                    row.get::<String, _>("version"),
                    row.get::<String, _>("digest"),
                )
                .map_err(|_| V3StoreError::CorruptObject("stored Lens pin".into()))?,
                manifest,
                source: row.get("origin"),
                available: state == "active",
                state,
                usage_count: reveal_usage.then(|| row.get("usage_count")),
            })
        })
        .collect()
}

async fn read_config_tx(
    tx: &mut Transaction<'_, Postgres>,
    space: &SpaceId,
) -> V3StoreResult<(u64, InitialLensConfig)> {
    let row = sqlx::query("SELECT revision,pins_jcs,associations_jcs FROM layrs_collaboration.space_lens_configs WHERE space_id=$1")
        .bind(space.as_str()).fetch_optional(&mut **tx).await.map_err(pg_error)?;
    let Some(row) = row else {
        return Ok((0, InitialLensConfig::default()));
    };
    let pins = serde_json::from_str(&row.get::<String, _>("pins_jcs"))
        .map_err(|_| V3StoreError::CorruptObject("stored Lens pins".into()))?;
    let associations = serde_json::from_str(&row.get::<String, _>("associations_jcs"))
        .map_err(|_| V3StoreError::CorruptObject("stored Lens associations".into()))?;
    Ok((
        row.get::<i64, _>("revision") as u64,
        InitialLensConfig { pins, associations },
    ))
}

async fn lock_authorized_read(
    tx: &mut Transaction<'_, Postgres>,
    principal: &PrincipalContext,
    authority: &SpaceAuthority,
) -> V3StoreResult<()> {
    super::super::transaction_store::lock_identity(tx, principal, &authority.space_id).await?;
    let epoch: Option<i64> = sqlx::query_scalar("SELECT authorization_epoch FROM spaces WHERE space_id=$1 AND lifecycle_state='ready' AND current_policy_set_id=$2 FOR SHARE")
        .bind(authority.space_id.as_str()).bind(authority.policy_set_id.as_str())
        .fetch_optional(&mut **tx).await.map_err(pg_error)?;
    if epoch != Some(authority.authorization_epoch as i64) {
        return Err(V3StoreError::AuthorizationEpochChanged);
    }
    Ok(())
}

impl PgCollaborationStore {
    pub async fn lens_catalog(
        &self,
        principal: &PrincipalContext,
        administrator: bool,
    ) -> V3StoreResult<Vec<LensCatalogItem>> {
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        active_principal(&mut tx, principal, administrator).await?;
        let items = catalog_tx(
            &mut tx,
            &principal.deployment_id,
            !administrator,
            administrator,
        )
        .await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(items)
    }

    pub async fn install_lens(
        &self,
        principal: &PrincipalContext,
        packet: LensPacket,
        approve_unsigned: bool,
        trusted_keys: &TrustedLensKeys,
    ) -> V3StoreResult<LensCatalogItem> {
        let verified = verify_lens_packet(&packet)?;
        if !trusted_keys.verify(&packet.manifest)? && !approve_unsigned {
            return Err(invalid("unsigned_lens_requires_explicit_approval"));
        }
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        active_principal(&mut tx, principal, true).await?;
        install_packet_tx(
            &mut tx,
            &principal.deployment_id,
            &packet,
            &verified,
            "installed",
            Some(principal.principal_id.as_str()),
        )
        .await?;
        let item = catalog_tx(&mut tx, &principal.deployment_id, false, true)
            .await?
            .into_iter()
            .find(|item| item.pin == verified.pin)
            .ok_or(V3StoreError::NotFound)?;
        tx.commit().await.map_err(pg_error)?;
        Ok(item)
    }

    pub async fn change_lens_state(
        &self,
        principal: &PrincipalContext,
        pin: &LensPin,
        target: &str,
    ) -> V3StoreResult<LensCatalogItem> {
        if target != "retired" && target != "revoked" {
            return Err(invalid("invalid_lens_state"));
        }
        pin.validate().map_err(|_| invalid("invalid_lens_pin"))?;
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        active_principal(&mut tx, principal, true).await?;
        let previous: Option<String> = sqlx::query_scalar("SELECT state FROM layrs_collaboration.lens_versions WHERE deployment_id=$1 AND lens_id=$2 AND version=$3 AND digest=$4 FOR UPDATE")
            .bind(&principal.deployment_id).bind(&pin.lens_id).bind(&pin.version).bind(&pin.digest)
            .fetch_optional(&mut *tx).await.map_err(pg_error)?;
        let previous = previous.ok_or(V3StoreError::NotFound)?;
        if previous == "revoked" && target != "revoked" {
            return Err(invalid("revoked_lens_cannot_be_restored"));
        }
        if previous == "retired" && target == "active" {
            return Err(invalid("retired_lens_cannot_be_restored"));
        }
        if previous != target {
            sqlx::query("UPDATE layrs_collaboration.lens_versions SET state=$5 WHERE deployment_id=$1 AND lens_id=$2 AND version=$3 AND digest=$4")
                .bind(&principal.deployment_id).bind(&pin.lens_id).bind(&pin.version).bind(&pin.digest).bind(target)
                .execute(&mut *tx).await.map_err(pg_error)?;
            sqlx::query("INSERT INTO layrs_collaboration.lens_state_events(deployment_id,lens_id,version,digest,state,actor_principal_id) VALUES($1,$2,$3,$4,$5,$6)")
                .bind(&principal.deployment_id).bind(&pin.lens_id).bind(&pin.version).bind(&pin.digest).bind(target).bind(principal.principal_id.as_str())
                .execute(&mut *tx).await.map_err(pg_error)?;
        }
        let item = catalog_tx(&mut tx, &principal.deployment_id, false, true)
            .await?
            .into_iter()
            .find(|item| &item.pin == pin)
            .ok_or(V3StoreError::NotFound)?;
        tx.commit().await.map_err(pg_error)?;
        Ok(item)
    }

    pub async fn space_lenses(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
    ) -> V3StoreResult<SpaceLensConfig> {
        let authority = self.current_authority(space).await?;
        if !self
            .decide(principal, &authority, V3Action::Bootstrap)
            .await?
            .allowed
        {
            return Err(V3StoreError::NotFound);
        }
        let can_manage = super::super::space_access::full_access(
            &self
                .decide(principal, &authority, V3Action::AdministerPolicy)
                .await?,
        );
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        lock_authorized_read(&mut tx, principal, &authority).await?;
        let deployment = deployment_for_space(&mut tx, space).await?;
        if deployment != principal.deployment_id {
            return Err(V3StoreError::NotFound);
        }
        let can_manage = can_manage
            && super::super::space_access::workspace_administrator(&mut *tx, principal, space)
                .await?;
        let (revision, config) = read_config_tx(&mut tx, space).await?;
        let mut available_catalog = catalog_tx(&mut tx, &deployment, false, false).await?;
        available_catalog.retain(|item| item.state == "active" || config.pins.contains(&item.pin));
        for item in &mut available_catalog {
            item.available = item.state != "revoked"
                && (item.state == "active" || config.pins.contains(&item.pin));
        }
        tx.commit().await.map_err(pg_error)?;
        Ok(SpaceLensConfig {
            revision,
            pins: config.pins,
            associations: config.associations,
            can_manage,
            available_catalog,
        })
    }

    pub async fn configure_space_lenses(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        request: LensConfigRequest,
    ) -> V3StoreResult<SpaceLensConfig> {
        let config = normalized(InitialLensConfig {
            pins: request.pins,
            associations: request.associations,
        })?;
        if request.expected_revision >= 9_007_199_254_740_991 {
            return Err(invalid("lens_revision_out_of_range"));
        }
        let authority = self
            .authority(principal, space, V3Action::AdministerPolicy)
            .await?;
        let mut tx = self.locked(principal, &authority).await?;
        if !super::super::space_access::workspace_administrator(&mut *tx, principal, space).await? {
            return Err(V3StoreError::InvalidCapability);
        }
        let deployment = deployment_for_space(&mut tx, space).await?;
        if deployment != principal.deployment_id {
            return Err(V3StoreError::NotFound);
        }
        let (revision, _) = read_config_tx(&mut tx, space).await?;
        if revision != request.expected_revision {
            return Err(V3StoreError::LensConfigConflict);
        }
        validate_pins(&mut tx, space, &deployment, &config).await?;
        write_config(
            &mut tx,
            space,
            &deployment,
            &config,
            (revision + 1) as i64,
            Some(principal.principal_id.as_str()),
        )
        .await?;
        let mut available_catalog = catalog_tx(&mut tx, &deployment, false, false).await?;
        available_catalog.retain(|item| item.state == "active" || config.pins.contains(&item.pin));
        for item in &mut available_catalog {
            item.available = item.state != "revoked"
                && (item.state == "active" || config.pins.contains(&item.pin));
        }
        tx.commit().await.map_err(pg_error)?;
        Ok(SpaceLensConfig {
            revision: revision + 1,
            pins: config.pins,
            associations: config.associations,
            can_manage: true,
            available_catalog,
        })
    }

    pub async fn fetch_lens_packet(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        pin: &LensPin,
    ) -> V3StoreResult<LensPacket> {
        pin.validate().map_err(|_| invalid("invalid_lens_pin"))?;
        let authority = self.current_authority(space).await?;
        if !self
            .decide(principal, &authority, V3Action::Bootstrap)
            .await?
            .allowed
        {
            return Err(V3StoreError::NotFound);
        }
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        lock_authorized_read(&mut tx, principal, &authority).await?;
        let packet: Option<String> = sqlx::query_scalar("SELECT v.packet_jcs FROM layrs_collaboration.space_lens_pins p JOIN layrs_collaboration.lens_versions v ON (v.deployment_id,v.lens_id,v.version,v.digest)=(p.deployment_id,p.lens_id,p.version,p.digest) JOIN spaces s ON s.space_id=p.space_id JOIN workspaces w ON w.workspace_id=s.workspace_id WHERE p.space_id=$1 AND p.lens_id=$2 AND p.version=$3 AND p.digest=$4 AND p.deployment_id=$5 AND w.deployment_id=$5 AND v.state<>'revoked' FOR SHARE OF p,v")
            .bind(space.as_str()).bind(&pin.lens_id).bind(&pin.version).bind(&pin.digest).bind(&principal.deployment_id)
            .fetch_optional(&mut *tx).await.map_err(pg_error)?;
        let packet = packet.ok_or(V3StoreError::NotFound)?;
        let packet = serde_json::from_str(&packet)
            .map_err(|_| V3StoreError::CorruptObject("stored Lens packet".into()))?;
        tx.commit().await.map_err(pg_error)?;
        Ok(packet)
    }
}

fn check_execution_manifest(raw: &str, implementation_digest: &str) -> V3StoreResult<()> {
    let manifest: LensManifest = serde_json::from_str(raw)
        .map_err(|_| V3StoreError::CorruptObject("stored Lens manifest".into()))?;
    let reconcile = manifest
        .bundle
        .as_ref()
        .and_then(|bundle| bundle.entrypoints.reconcile.as_ref())
        .ok_or(V3StoreError::InvalidCapability)?;
    if reconcile.digest != implementation_digest {
        return Err(V3StoreError::InvalidCapability);
    }
    Ok(())
}

impl PgV3Repository {
    /// Compatibility is only for projects which have never configured lenses.
    /// The admission caller holds the Space lock through commit, also used by
    /// configure_space_lenses, so a newly inserted configuration cannot race it.
    pub async fn validate_legacy_execution_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        space: &SpaceId,
    ) -> V3StoreResult<()> {
        let configured: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM layrs_collaboration.space_lens_configs WHERE space_id=$1)",
        )
        .bind(space.as_str())
        .fetch_one(&mut **tx)
        .await
        .map_err(pg_error)?;
        if configured {
            return Err(V3StoreError::InvalidCapability);
        }
        Ok(())
    }

    pub async fn validate_legacy_execution(&self, space: &SpaceId) -> V3StoreResult<()> {
        let mut tx = self.pool().begin().await.map_err(pg_error)?;
        self.validate_legacy_execution_in(&mut tx, space).await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(())
    }

    /// New execution must still have an active project pin at the commit lock.
    pub async fn validate_installed_execution_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        space: &SpaceId,
        pin: &LensPin,
        implementation_digest: &str,
    ) -> V3StoreResult<()> {
        pin.validate().map_err(|_| invalid("invalid_lens_pin"))?;
        let row = sqlx::query("SELECT v.manifest_jcs FROM layrs_collaboration.space_lens_pins p JOIN layrs_collaboration.lens_versions v ON (v.deployment_id,v.lens_id,v.version,v.digest)=(p.deployment_id,p.lens_id,p.version,p.digest) JOIN spaces s ON s.space_id=p.space_id JOIN workspaces w ON w.workspace_id=s.workspace_id AND w.deployment_id=p.deployment_id WHERE p.space_id=$1 AND p.lens_id=$2 AND p.version=$3 AND p.digest=$4 AND v.state<>'revoked' FOR SHARE OF p,v")
            .bind(space.as_str()).bind(&pin.lens_id).bind(&pin.version).bind(&pin.digest)
            .fetch_optional(&mut **tx).await.map_err(pg_error)?.ok_or(V3StoreError::InvalidCapability)?;
        check_execution_manifest(&row.get::<String, _>("manifest_jcs"), implementation_digest)
    }

    pub async fn validate_installed_execution(
        &self,
        space: &SpaceId,
        pin: &LensPin,
        implementation_digest: &str,
    ) -> V3StoreResult<()> {
        let mut tx = self.pool().begin().await.map_err(pg_error)?;
        self.validate_installed_execution_in(&mut tx, space, pin, implementation_digest)
            .await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(())
    }

    /// Old immutable receipts stay admissible after unpin or revocation only
    /// if this Space had selected the exact immutable package at least once.
    pub async fn validate_historical_execution(
        &self,
        space: &SpaceId,
        pin: &LensPin,
        implementation_digest: &str,
    ) -> V3StoreResult<()> {
        pin.validate().map_err(|_| invalid("invalid_lens_pin"))?;
        let raw: Option<String> = sqlx::query_scalar("SELECT v.manifest_jcs FROM layrs_collaboration.space_lens_history h JOIN layrs_collaboration.lens_versions v ON (v.deployment_id,v.lens_id,v.version,v.digest)=(h.deployment_id,h.lens_id,h.version,h.digest) JOIN spaces s ON s.space_id=h.space_id JOIN workspaces w ON w.workspace_id=s.workspace_id AND w.deployment_id=h.deployment_id WHERE h.space_id=$1 AND h.lens_id=$2 AND h.version=$3 AND h.digest=$4")
            .bind(space.as_str()).bind(&pin.lens_id).bind(&pin.version).bind(&pin.digest)
            .fetch_optional(self.pool()).await.map_err(pg_error)?;
        check_execution_manifest(
            &raw.ok_or(V3StoreError::InvalidCapability)?,
            implementation_digest,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layrs_lens_sdk::LensSignatureMetadata;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    fn builtin() -> LensPacket {
        let packets: Vec<LensPacket> = serde_json::from_str(include_str!(
            "../../../../../deploy/lenses/builtin-packets.json"
        ))
        .unwrap();
        packets.into_iter().next().unwrap()
    }

    #[test]
    fn shipped_builtin_is_a_verified_immutable_packet() {
        let packets: Vec<LensPacket> = serde_json::from_str(include_str!(
            "../../../../../deploy/lenses/builtin-packets.json"
        ))
        .unwrap();
        assert!(!packets.is_empty());
        for packet in packets {
            let verified = verify_lens_packet(&packet).unwrap();
            assert_eq!(verified.pin.lens_id, packet.manifest.id);
            assert_eq!(verified.pin.digest, packet.manifest.digest.unwrap());
        }
    }

    #[test]
    fn packet_rejects_changed_bytes_identity_and_undeclared_files() {
        let mut packet = builtin();
        packet.files[0].bytes_base64.push('A');
        assert!(verify_lens_packet(&packet).is_err());
        let mut packet = builtin();
        packet.manifest.name.push('!');
        assert!(verify_lens_packet(&packet).is_err());
        let mut packet = builtin();
        packet.files.push(LensPacketFile {
            path: "../outside.js".into(),
            bytes_base64: "YQ==".into(),
        });
        assert!(verify_lens_packet(&packet).is_err());
    }

    #[test]
    fn configuration_rejects_duplicate_extensions_and_unpinned_associations() {
        let packet = builtin();
        let pin = verify_lens_packet(&packet).unwrap().pin;
        let association = LensAssociation {
            extension: ".txt".into(),
            lens_id: pin.lens_id.clone(),
        };
        assert!(
            normalized(InitialLensConfig {
                pins: vec![pin.clone()],
                associations: vec![association.clone(), association]
            })
            .is_err()
        );
        assert!(
            normalized(InitialLensConfig {
                pins: vec![],
                associations: vec![LensAssociation {
                    extension: ".txt".into(),
                    lens_id: pin.lens_id
                }]
            })
            .is_err()
        );
    }

    #[test]
    fn trusted_ed25519_signature_covers_canonical_identity_digest() {
        let mut packet = builtin();
        let seed = [7u8; 32];
        let key = Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
        let digest = packet.manifest.digest.clone().unwrap();
        packet.manifest.bundle.as_mut().unwrap().signature = Some(LensSignatureMetadata {
            algorithm: "ed25519".into(),
            key_id: "operator-key-1".into(),
            signed_digest: digest.clone(),
            signature: format!(
                "base64:{}",
                STANDARD.encode(key.sign(digest.as_bytes()).as_ref())
            ),
        });
        let trusted = TrustedLensKeys::parse(Some(
            &serde_json::json!({"operator-key-1":STANDARD.encode(key.public_key().as_ref())})
                .to_string(),
        ))
        .unwrap();
        verify_lens_packet(&packet).unwrap();
        assert!(trusted.verify(&packet.manifest).unwrap());
        assert!(TrustedLensKeys::default().verify(&packet.manifest).is_err());
        packet
            .manifest
            .bundle
            .as_mut()
            .unwrap()
            .signature
            .as_mut()
            .unwrap()
            .signature = format!("base64:{}", STANDARD.encode([0u8; 64]));
        assert!(trusted.verify(&packet.manifest).is_err());
    }

    #[test]
    fn trust_roots_are_bounded_and_exact() {
        assert!(TrustedLensKeys::parse(Some("{\"bad\":\"YQ==\"}")).is_err());
        assert!(TrustedLensKeys::parse(Some(&"x".repeat(8193))).is_err());
        assert!(
            !TrustedLensKeys::default()
                .verify(&builtin().manifest)
                .unwrap()
        );
    }
}
