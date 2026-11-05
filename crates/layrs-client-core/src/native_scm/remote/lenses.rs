//! Authenticated project Lens selection and verified local module cache.
//! Cache paths are derived only from SHA-256 values. No packet path is opened.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use layrs_lens_sdk::{sha256_digest, LensManifest, LensPin};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MAX_LENSES: usize = 32;
const MAX_MODULE: usize = 2 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProjectLensCache {
    pub(crate) origin: String,
    pub(crate) space_id: SpaceId,
    pub(crate) revision: u64,
    pub(crate) pins: Vec<LensPin>,
    pub(crate) associations: Vec<LensAssociation>,
    #[serde(default)]
    pub(crate) catalogue: Vec<LensManifest>,
    pub(crate) modules: Vec<CachedModule>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LensAssociation {
    pub(crate) extension: String,
    pub(crate) lens_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CachedModule {
    pub(crate) pin: LensPin,
    pub(crate) manifest: LensManifest,
    pub(crate) module_digest: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteLensConfig {
    revision: u64,
    pins: Vec<LensPin>,
    associations: Vec<LensAssociation>,
    available_catalog: Vec<RemoteCatalogItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteCatalogItem {
    pin: LensPin,
    available: bool,
    manifest: LensManifest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemotePacket {
    manifest: LensManifest,
    files: Vec<RemotePacketFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemotePacketFile {
    path: String,
    bytes_base64: String,
}

fn cache_path(root: &Path) -> PathBuf {
    root.join("native-lenses.json")
}
fn module_path(root: &Path, digest: &str) -> RemoteResult<PathBuf> {
    let suffix = digest
        .strip_prefix("sha256:")
        .filter(|suffix| suffix.len() == 64 && suffix.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or(NativeRemoteError::Invalid("invalid Lens module digest"))?;
    Ok(root.join(format!("native-lens-{suffix}.wasm")))
}

fn verify_manifest(
    manifest: &LensManifest,
    pin: &LensPin,
    module_digest: &str,
) -> RemoteResult<()> {
    verify_manifest_identity(manifest, pin)?;
    if manifest
        .bundle
        .as_ref()
        .and_then(|bundle| bundle.entrypoints.reconcile.as_ref())
        .is_none_or(|entry| entry.digest != module_digest || entry.export != "layrs_reconcile")
    {
        return Err(NativeRemoteError::Invalid("cached Lens identity mismatch"));
    }
    Ok(())
}

fn verify_manifest_identity(manifest: &LensManifest, pin: &LensPin) -> RemoteResult<()> {
    manifest
        .validate()
        .map_err(|_| NativeRemoteError::Invalid("invalid cached Lens manifest"))?;
    if manifest.pin().as_ref() != Some(pin) {
        return Err(NativeRemoteError::Invalid("cached Lens identity mismatch"));
    }
    let mut identity = manifest.clone();
    identity.digest = None;
    identity
        .bundle
        .as_mut()
        .ok_or(NativeRemoteError::Invalid("cached Lens bundle missing"))?
        .signature = None;
    let bytes = serde_jcs::to_vec(&identity)?;
    if sha256_digest(&bytes) != pin.digest {
        return Err(NativeRemoteError::Invalid(
            "cached Lens manifest digest mismatch",
        ));
    }
    Ok(())
}

pub(crate) fn read_cache(workspace: &NativeWorkspace) -> RemoteResult<Option<ProjectLensCache>> {
    let path = cache_path(workspace.store_root());
    if !path.try_exists()? {
        return Ok(None);
    }
    let cache: ProjectLensCache = serde_json::from_slice(&bounded_file(&path, 1024 * 1024)?)?;
    if cache.space_id != *workspace.state()?.space.id()
        || cache.pins.len() > MAX_LENSES
        || cache.modules.len() > MAX_LENSES
        || cache.associations.len() > 64
        || cache.catalogue.len() > MAX_LENSES
    {
        return Err(NativeRemoteError::Invalid("invalid cached project Lenses"));
    }
    let binding: Binding = serde_json::from_slice(&bounded_file(
        &workspace.store_root().join("native-remote.json"),
        MAX_OBJECT,
    )?)?;
    if binding.space_id != cache.space_id || binding.origin != cache.origin {
        return Err(NativeRemoteError::Invalid(
            "cached project Lens origin mismatch",
        ));
    }
    for module in &cache.modules {
        if !cache.pins.contains(&module.pin) {
            return Err(NativeRemoteError::Invalid("cached module is not selected"));
        }
        verify_manifest(&module.manifest, &module.pin, &module.module_digest)?;
    }
    for manifest in &cache.catalogue {
        let pin = manifest
            .pin()
            .ok_or(NativeRemoteError::Invalid("invalid catalogue pin"))?;
        if !cache.pins.contains(&pin) {
            return Err(NativeRemoteError::Invalid("catalogue Lens is not selected"));
        }
        verify_manifest_identity(manifest, &pin)?;
    }
    Ok(Some(cache))
}

pub(crate) fn read_module(root: &Path, digest: &str) -> RemoteResult<Vec<u8>> {
    let bytes = bounded_file(&module_path(root, digest)?, MAX_MODULE as u64)?;
    layrs_weave::wasm_host::validate_guest_module(&bytes, digest)
        .map_err(|_| NativeRemoteError::Invalid("cached Lens module failed validation"))?;
    Ok(bytes)
}

fn store_module(root: &Path, digest: &str, bytes: &[u8]) -> RemoteResult<()> {
    let path = module_path(root, digest)?;
    if path.try_exists()? && read_module(root, digest).is_ok() {
        return Ok(());
    }
    let temp = root.join(format!("native-lens-{}.tmp", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(temp, path)?;
    Ok(())
}

fn store_cache(workspace: &NativeWorkspace, cache: &ProjectLensCache) -> RemoteResult<()> {
    let root = workspace.store_root();
    let temp = root.join(format!("native-lenses-{}.tmp", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)?;
    file.write_all(&serde_jcs::to_vec(cache)?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(temp, cache_path(root))?;
    #[cfg(unix)]
    File::open(root)?.sync_all()?;
    Ok(())
}

impl NativeRemote {
    pub(super) fn sync_project_lenses(&self, workspace: &NativeWorkspace) -> RemoteResult<()> {
        let binding = self.binding(workspace)?;
        if binding.origin != self.origin().trim_end_matches('/') && binding.origin != self.origin()
        {
            return Err(NativeRemoteError::Invalid("Lens cache origin mismatch"));
        }
        let config: RemoteLensConfig =
            match self.get(&format!("v3/spaces/{}/lenses", binding.space_id)) {
                Ok(config) => config,
                // An older server has no Lens catalogue; do not fabricate pins.
                Err(NativeRemoteError::Http(404)) => return Ok(()),
                Err(error) => return Err(error),
            };
        if config.pins.len() > MAX_LENSES || config.associations.len() > 64 {
            return Err(NativeRemoteError::Invalid(
                "project Lens catalogue exceeds limit",
            ));
        }
        let available: Vec<_> = config
            .available_catalog
            .into_iter()
            .filter(|item| item.available)
            .collect();
        let active_pins: Vec<_> = config
            .pins
            .into_iter()
            .filter(|pin| available.iter().any(|item| &item.pin == pin))
            .collect();
        let catalogue: Vec<_> = available
            .into_iter()
            .filter(|item| active_pins.contains(&item.pin))
            .map(|item| {
                verify_manifest_identity(&item.manifest, &item.pin)?;
                Ok(item.manifest)
            })
            .collect::<RemoteResult<_>>()?;
        let active_associations: Vec<_> = config
            .associations
            .into_iter()
            .filter(|association| {
                active_pins
                    .iter()
                    .any(|pin| pin.lens_id == association.lens_id)
            })
            .collect();
        let mut cache = ProjectLensCache {
            origin: self.origin().into(),
            space_id: binding.space_id.clone(),
            revision: config.revision,
            pins: active_pins,
            associations: active_associations,
            catalogue,
            modules: Vec::new(),
        };
        let previous = read_cache(workspace)?.filter(|old| old.origin == cache.origin);
        if let Some(previous) = &previous {
            cache.modules = previous
                .modules
                .iter()
                .filter(|module| {
                    cache.pins.contains(&module.pin)
                        && read_module(workspace.store_root(), &module.module_digest).is_ok()
                })
                .cloned()
                .collect();
        }
        // Publish the newly observed availability before any packet transfer.
        // A failed download can no longer leave a revoked module selected.
        store_cache(workspace, &cache)?;
        for pin in &cache.pins {
            pin.validate()
                .map_err(|_| NativeRemoteError::Invalid("invalid project Lens pin"))?;
            if cache.modules.iter().any(|module| &module.pin == pin) {
                continue;
            }
            let packet: RemotePacket = self.get(&format!(
                "v3/spaces/{}/lenses/{}/{}/{}",
                binding.space_id, pin.lens_id, pin.version, pin.digest
            ))?;
            let Some(entrypoint) = packet
                .manifest
                .bundle
                .as_ref()
                .and_then(|bundle| bundle.entrypoints.reconcile.as_ref())
            else {
                continue;
            };
            let module_digest = entrypoint.digest.clone();
            verify_manifest(&packet.manifest, pin, &module_digest)?;
            if packet.files.len() > 8 {
                return Err(NativeRemoteError::Invalid(
                    "project Lens packet exceeds file limit",
                ));
            }
            let file = packet
                .files
                .iter()
                .find(|file| file.path == entrypoint.path)
                .ok_or(NativeRemoteError::Invalid("project Lens module missing"))?;
            let bytes = STANDARD
                .decode(&file.bytes_base64)
                .map_err(|_| NativeRemoteError::Invalid("invalid project Lens module encoding"))?;
            if bytes.len() > MAX_MODULE || sha256_digest(&bytes) != module_digest {
                return Err(NativeRemoteError::Invalid(
                    "project Lens module digest mismatch",
                ));
            }
            layrs_weave::wasm_host::validate_guest_module(&bytes, &module_digest)
                .map_err(|_| NativeRemoteError::Invalid("invalid project Lens module"))?;
            store_module(workspace.store_root(), &module_digest, &bytes)?;
            cache.modules.push(CachedModule {
                pin: pin.clone(),
                manifest: packet.manifest,
                module_digest,
            });
        }
        for association in &cache.associations {
            if !cache
                .pins
                .iter()
                .any(|pin| pin.lens_id == association.lens_id)
            {
                return Err(NativeRemoteError::Invalid(
                    "unmatched project Lens association",
                ));
            }
        }
        store_cache(workspace, &cache)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_only_catalogue_manifest_keeps_its_exact_identity() {
        let packet: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../deploy/lenses/example-markdown.layrslens.json"
        ))
        .unwrap();
        let manifest: LensManifest = serde_json::from_value(packet["manifest"].clone()).unwrap();
        let pin = manifest.pin().unwrap();
        assert!(manifest
            .bundle
            .as_ref()
            .unwrap()
            .entrypoints
            .reconcile
            .is_none());
        verify_manifest_identity(&manifest, &pin).unwrap();
        let mut changed = manifest;
        changed.analyzer.file_extensions.push("txt".into());
        assert!(verify_manifest_identity(&changed, &pin).is_err());
    }
}
