//! Full retained-checkpoint adapter for C13, alongside the still-strict legacy
//! managed adapter. Unknown/protected files are HASHED, not granted authority.
//! No output path, writable hardlink, invented observation or capture is made.
use super::*;
use crate::projection::provider::{
    transition_observation::TransitionObservationData,
    transition_reconcile::{
        CompleteSourceImageData, ImageFileData, ReconcileSourceData, companions::OwnedCompanionData,
    },
    validation::PrivateActiveCaptureValidation,
};

pub(in crate::projection::provider) struct VerifiedCompleteManagedImage<'a, 'c> {
    retained: VerifiedManagedImage<'a, 'c>,
    offline: Option<crate::projection::shared::exact_content::ResidentCasProof>,
    offline_manifests: Vec<FileManifest>,
}

/// All namespace/size/alias checks precede the first content read. Every
/// immutable file verifies FileId AND exact chunks; every NeedsHash entry uses
/// the real retained checkpoint reader. Missing/unsupported bytes propagate.
pub(in crate::projection::provider) fn plan<'a, 'c>(
    checkpoint: &'a mut ExportCheckpoint<'c>,
    bytes: &'a dyn SnapshotSource,
    source: &'a ProviderBootstrapSource,
    activated: &'a ProjectionSnapshot,
    cancel: &Cancellation,
) -> Result<VerifiedCompleteManagedImage<'a, 'c>> {
    if checkpoint.root_guard().is_none() {
        return Err(invalid(
            "complete managed image lacks retained provider root",
        ));
    }
    let data = build_complete(checkpoint, bytes, source, activated, None, cancel)?;
    Ok(VerifiedCompleteManagedImage {
        retained: VerifiedManagedImage {
            checkpoint,
            bytes,
            source,
            activated,
            data,
        },
        offline: None,
        offline_manifests: Vec::new(),
    })
}

impl<'a, 'c> VerifiedCompleteManagedImage<'a, 'c> {
    #[cfg(test)]
    pub(in crate::projection::provider) fn retained_offline_manifests(&self) -> &[FileManifest] {
        &self.offline_manifests
    }
    /// Pin actual already-resident immutable current and required preimage bytes before B's first
    /// reservation. Later complete-image rehashes must not reacquire source
    /// range readers, whose ordinary no-pending family view cannot classify B's
    /// reserved-but-unpublished root. No hydration or new bytes are acquired.
    pub(in crate::projection::provider) fn pin_offline_current(
        &mut self,
        storage: crate::projection::shared::exact_content::ExactStorage,
        required: &[FileManifest],
        cancel: &Cancellation,
    ) -> Result<()> {
        let mut manifests = BTreeMap::new();
        for file in self.retained.files() {
            if let ManagedManifest::Source(manifest) = &file.manifest {
                manifests.insert(manifest.id().clone(), (*manifest).clone());
            }
        }
        for manifest in required {
            cancel.check()?;
            // A current native file can produce any verified representation
            // of its exact FileId/length. Otherwise the descriptor-only branch
            // must retain the original CAS bytes before reservations begin.
            if self.retained.files().iter().any(|file| {
                let current = file.manifest.manifest();
                current.file_id() == manifest.file_id()
                    && current.byte_length() == manifest.byte_length()
            }) {
                continue;
            }
            // The validated complete union also contains superseded Capture
            // replacements and inherited companion representations. Their
            // original native path need not still exist. Retain their actual
            // immutable CAS bytes; a descriptor is not their ownership proof.
            manifests.insert(manifest.id().clone(), manifest.clone());
        }
        let manifests = manifests.into_values().collect::<Vec<_>>();
        if let Some(offline) = &self.offline {
            if self.offline_manifests != manifests {
                return Err(invalid("retained offline manifest requirement changed"));
            }
            return offline.verify(cancel);
        }
        self.offline = Some(crate::projection::shared::exact_content::pin_resident_cas(
            storage, &manifests, cancel,
        )?);
        self.offline_manifests = manifests;
        Ok(())
    }
    fn read_offline(
        &self,
        file: &FileId,
        offset: u64,
        output: &mut [u8],
        cancel: &Cancellation,
    ) -> Result<()> {
        match &self.offline {
            Some(offline) => offline.read_exact_at(file, offset, output, cancel),
            None => self
                .retained
                .bytes
                .read_exact_at(file, offset, output, cancel)
                .map_err(Into::into),
        }
    }
    /// Pin/namespace-only check after C Ready; never rehashes source bytes.
    pub(in crate::projection::provider) fn require_retained(
        &self,
        root: &layrs_vfs::ProviderRootGuard,
        digest: &str,
        cancel: &Cancellation,
    ) -> Result<()> {
        cancel.check()?;
        if digest != self.retained.digest() {
            return Err(invalid("complete retained source image digest differs"));
        }
        self.retained
            .require_owner(root, self.retained.source, self.retained.activated)?;
        cancel.check()?;
        Ok(())
    }

    pub(in crate::projection::provider) fn digest(&self) -> &str {
        self.retained.digest()
    }

    /// The import prefix must have been constructed with this complete digest.
    /// from_checkpoint independently binds the entire inventory to that prefix
    /// and real source control/FORMAT. No caller-selected inventory is accepted.
    pub(in crate::projection::provider) fn reconcile(
        &mut self,
        source: &ReconcileSourceData<'_>,
        observation: &TransitionObservationData,
        proof: &PrivateActiveCaptureValidation,
        manifests: &[FileManifest],
        metadata_without_companions: u64,
        cancel: &Cancellation,
    ) -> Result<OwnedCompanionData> {
        self.revalidate(cancel)?;
        let result = self.reconcile_capture_retained(
            source,
            observation,
            proof,
            manifests,
            metadata_without_companions,
            cancel,
        )?;
        self.revalidate(cancel)?;
        Ok(result)
    }

    /// Rebuild DATA under the retained source transaction after the caller's
    /// image rehash. This does not acquire range readers while family gates
    /// are held and never turns Capture proof into publication authority.
    pub(in crate::projection::provider) fn reconcile_capture_retained(
        &self,
        source: &ReconcileSourceData<'_>,
        observation: &TransitionObservationData,
        proof: &PrivateActiveCaptureValidation,
        manifests: &[FileManifest],
        metadata_without_companions: u64,
        cancel: &Cancellation,
    ) -> Result<OwnedCompanionData> {
        cancel.check()?;
        let source = source.provider_descriptor_view(proof, Some(cancel))?;
        let (_, _, fixed, activated, _) = proof.import_parts();
        self.retained.require_source(fixed, &activated.snapshot)?;
        let views = self
            .retained
            .files()
            .iter()
            .map(|file| ImageFileData {
                path: &file.path,
                manifest: file.manifest.manifest(),
                executable: Some(file.executable),
            })
            .collect::<Vec<_>>();
        let complete = CompleteSourceImageData::from_checkpoint(
            &source,
            &*self.retained.checkpoint,
            &views,
            Some(cancel),
        )?;
        OwnedCompanionData::derive(
            &source,
            &complete,
            observation,
            manifests,
            metadata_without_companions,
            Some(proof),
            Some(cancel),
        )
    }

    pub(in crate::projection::provider) fn retained(&self) -> &VerifiedManagedImage<'a, 'c> {
        &self.retained
    }
    pub(in crate::projection::provider) fn require_owner(
        &self,
        root: &layrs_vfs::ProviderRootGuard,
        source: &ProviderBootstrapSource,
        activated: &ProjectionSnapshot,
    ) -> Result<()> {
        self.retained.require_owner(root, source, activated)
    }
    pub(in crate::projection::provider) fn reconcile_publication_retained(
        &self,
        source: &ReconcileSourceData<'_>,
        observation: &TransitionObservationData,
        proof: &crate::projection::provider::validation::PrivateActivePublicationValidation,
        manifests: &[FileManifest],
        metadata: u64,
        cancel: &Cancellation,
    ) -> Result<OwnedCompanionData> {
        if observation.binding().denied_path_count != 0 {
            return Err(invalid(
                "C14 durable denied-absence contract required before effects",
            ));
        }
        self.reconcile_publication_rows(source, observation, proof, manifests, metadata, cancel)
    }

    /// C14 DATA route retains the sealed complete denial document. Actual SQL
    /// body/phase and the live source anchor remain the enclosing owner's duty.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::projection::provider) fn reconcile_denial_publication_retained(
        &self,
        source: &ReconcileSourceData<'_>,
        observation: &TransitionObservationData,
        proof: &crate::projection::provider::validation::PrivateActivePublicationValidation,
        staging: &crate::projection::provider::records::transition::ProviderTransitionIntentRecord,
        coverage: &crate::projection::provider::records::denied_coverage::DerivedDeniedCoverage,
        manifests: &[FileManifest],
        metadata: u64,
        cancel: &Cancellation,
    ) -> Result<OwnedCompanionData> {
        coverage.record().require_intent(staging, Some(cancel))?;
        coverage.record().require_observation(
            &staging.plan.source_prefix,
            &staging.transaction_id,
            observation,
            Some(cancel),
        )?;
        self.reconcile_publication_rows(source, observation, proof, manifests, metadata, cancel)
    }

    fn reconcile_publication_rows(
        &self,
        source: &ReconcileSourceData<'_>,
        observation: &TransitionObservationData,
        proof: &crate::projection::provider::validation::PrivateActivePublicationValidation,
        manifests: &[FileManifest],
        metadata: u64,
        cancel: &Cancellation,
    ) -> Result<OwnedCompanionData> {
        cancel.check()?;
        let source = source.publication_descriptor_view(proof, Some(cancel))?;
        self.retained
            .require_source(&proof.active.source, &proof.active.activated.snapshot)?;
        let views = self
            .retained
            .files()
            .iter()
            .map(|f| ImageFileData {
                path: &f.path,
                manifest: f.manifest.manifest(),
                executable: Some(f.executable),
            })
            .collect::<Vec<_>>();
        let complete = CompleteSourceImageData::from_checkpoint(
            &source,
            &*self.retained.checkpoint,
            &views,
            Some(cancel),
        )?;
        OwnedCompanionData::derive_publication(
            &source,
            &complete,
            observation,
            manifests,
            metadata,
            Some(proof),
            Some(cancel),
        )
    }
    /// Exact-engine producer from the complete checkpoint or the retained
    /// offline pin for this exact required manifest, including historical
    /// captures and inherited representations. The engine verifies each
    /// requested chunk and the complete FileId; a pinset miss is refused.
    pub(in crate::projection::provider) fn copy_required_chunk(
        &mut self,
        manifest: &FileManifest,
        ordinal: usize,
        output: &mut dyn Write,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.revalidate(cancel)?;
        if let Some(file) = self.retained.files().iter().find(|f| {
            let actual = f.manifest.manifest();
            actual.file_id() == manifest.file_id() && actual.byte_length() == manifest.byte_length()
        }) {
            let path = file.path.clone();
            let native = matches!(file.manifest, ManagedManifest::Native(_));
            if native {
                let chunk = stream::exact_chunk(manifest, ordinal)?;
                let mut consumer_error = None;
                let result = self.retained.checkpoint.with_range_reader(
                    &path,
                    chunk.offset(),
                    chunk.byte_length(),
                    |reader| {
                        let copied = stream::copy_verified_chunk(
                            manifest,
                            ordinal,
                            output,
                            cancel,
                            |_, buffer| reader.read_exact(buffer).map_err(Into::into),
                        );
                        if let Err(e) = copied {
                            consumer_error = Some(e);
                            return Err(layrs_vfs::Error::Io(io::Error::other(
                                "complete image chunk consumer failed",
                            )));
                        }
                        Ok(())
                    },
                );
                finish_native_copy(result, consumer_error)?;
            } else {
                stream::copy_verified_chunk(
                    manifest,
                    ordinal,
                    output,
                    cancel,
                    |offset, buffer| self.read_offline(manifest.file_id(), offset, buffer, cancel),
                )?;
            }
        } else {
            if self.offline.is_none() || !self.offline_manifests.contains(manifest) {
                return Err(invalid(
                    "historical representation has no retained exact offline byte owner",
                ));
            }
            stream::copy_verified_chunk(manifest, ordinal, output, cancel, |offset, buffer| {
                self.read_offline(manifest.file_id(), offset, buffer, cancel)
            })?;
        }
        self.revalidate(cancel)
    }

    /// Exact current-image lookup includes recovery-only files; companions are
    /// not filtered out before installation. Historical/fresh-base bytes still
    /// require the existing separately owned family/acquisition reader.
    pub(in crate::projection::provider) fn current_path(
        &self,
        manifest: &FileManifest,
    ) -> Option<&RepoPath> {
        self.retained
            .files()
            .iter()
            .find(|file| file.manifest.manifest() == manifest)
            .map(|file| &file.path)
    }
    pub(in crate::projection::provider) fn copy_exact_chunk(
        &mut self,
        path: &RepoPath,
        manifest: &FileManifest,
        ordinal: usize,
        output: &mut dyn Write,
        cancel: &Cancellation,
    ) -> Result<()> {
        exact_file(self.retained.files(), path, manifest)?;
        if self.offline.is_some() {
            return self.copy_required_chunk(manifest, ordinal, output, cancel);
        }
        self.revalidate(cancel)?;
        self.retained
            .copy_exact_chunk(path, manifest, ordinal, output, cancel)?;
        self.revalidate(cancel)
    }
    pub(in crate::projection::provider) fn copy_exact_file(
        &mut self,
        path: &RepoPath,
        manifest: &FileManifest,
        output: &mut dyn Write,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.revalidate(cancel)?;
        let file = exact_file(self.retained.files(), path, manifest)?;
        if self.offline.is_some() && matches!(&file.manifest, ManagedManifest::Source(_)) {
            let mut offset = 0u64;
            stream::copy_verified(manifest, output, cancel, |buffer| {
                self.read_offline(manifest.file_id(), offset, buffer, cancel)?;
                offset = offset
                    .checked_add(buffer.len() as u64)
                    .ok_or_else(|| invalid("offline complete file offset overflow"))?;
                Ok(())
            })?;
        } else {
            self.retained
                .copy_exact_file(path, manifest, output, cancel)?;
        }
        self.revalidate(cancel)
    }
    pub(in crate::projection::provider) fn revalidate(
        &mut self,
        cancel: &Cancellation,
    ) -> Result<()> {
        let refreshed = build_complete(
            self.retained.checkpoint,
            self.retained.bytes,
            self.retained.source,
            self.retained.activated,
            self.offline.as_ref(),
            cancel,
        )?;
        if !self.retained.data.matches(&refreshed) {
            return Err(invalid("complete managed image bytes/namespace changed"));
        }
        Ok(())
    }
}

fn build_complete<'a>(
    checkpoint: &mut ExportCheckpoint<'_>,
    bytes: &dyn SnapshotSource,
    source: &'a ProviderBootstrapSource,
    activated: &ProjectionSnapshot,
    offline: Option<&crate::projection::shared::exact_content::ResidentCasProof>,
    cancel: &Cancellation,
) -> Result<ImageData<'a>> {
    cancel.check()?;
    checkpoint.revalidate_namespace()?;
    if activated != &activation_snapshot(source)? {
        return Err(invalid(
            "complete image activation differs from fixed source",
        ));
    }
    let count = bound_observation_paths(checkpoint.entries().iter().map(|e| &e.path), cancel)?;
    let mut observations = Vec::with_capacity(count);
    for entry in checkpoint.entries() {
        cancel.check()?;
        observations.push(Observed {
            path: entry.path.clone(),
            evidence: entry.evidence.clone(),
            original_path: checkpoint.original_path(&entry.path)?,
        });
    }
    let planned = recovery_export::plan(&observations, source, activated)?;
    let descriptors: BTreeMap<_, _> = source.descriptors().iter().map(|d| (&d.path, d)).collect();
    let mut files = Vec::with_capacity(planned.files.len());
    let mut report = Vec::with_capacity(planned.files.len());
    let mut metadata = 0u64;
    for file in &planned.files {
        cancel.check()?;
        let (manifest, executable) = match file.reader {
            ReaderKind::ImmutableSource => {
                let descriptor = descriptors
                    .get(&file.path)
                    .ok_or_else(|| invalid("complete immutable descriptor absent"))?;
                verify_source_manifest_stream(
                    &descriptor.manifest,
                    cancel,
                    |offset, output, cancel| match offline {
                        None => bytes.read_exact_at(&descriptor.file_id, offset, output, cancel),
                        Some(offline) => offline
                            .read_exact_at(&descriptor.file_id, offset, output, cancel)
                            .map_err(|e| layrs_vfs::Error::Io(io::Error::other(e))),
                    },
                )?;
                (
                    ManagedManifest::Source(&descriptor.manifest),
                    file.executable
                        .ok_or_else(|| invalid("complete immutable mode unknown"))?,
                )
            }
            ReaderKind::Native => {
                let manifest = checkpoint.with_reader(&file.path, |reader| {
                    capture_image::hash_reader(reader, file.size, cancel)
                })?;
                // This module is Windows-only. Native ordinary capture observes
                // false on this platform (capture/scan.rs::executable), even for
                // an unobserved creation. Do not copy a remote executable bit
                // onto a native replacement or invent a cross-platform mode.
                (ManagedManifest::Native(manifest), false)
            }
        };
        if manifest.manifest().byte_length() != file.size {
            return Err(invalid("complete native/source size differs"));
        }
        // Bound cumulative owned manifest metadata, in addition to the 4GiB
        // complete image and per-file bounds applied by recovery_export::plan.
        crate::projection::provider::records::codec::preflight(manifest.manifest(), 256 * 1024)?;
        let encoded = layrs_core::scm::canonical_json(manifest.manifest())
            .map_err(|e| invalid(e.to_string()))?;
        metadata = metadata
            .checked_add(encoded.len() as u64)
            .filter(|n| *n <= 256 * 1024 * 1024)
            .ok_or_else(|| invalid("complete manifest metadata exceeds bound"))?;
        let class = recovery_export::verified_class(file, manifest.manifest().file_id());
        report.push(ProviderRecoveryExportFile {
            path: file.path.clone(),
            file_id: manifest.manifest().file_id().clone(),
            length: file.size,
            executable: Some(executable),
            class,
        });
        files.push(ManagedFile {
            path: file.path.clone(),
            manifest,
            executable,
            expected: file.expected.clone(),
        });
    }
    let mut deletions = Vec::with_capacity(planned.deletions.len());
    for deletion in &planned.deletions {
        cancel.check()?;
        deletions.push(ManagedDeletion {
            path: deletion.path.clone(),
            base: deletion.base.clone(),
            executable: deletion
                .expected_executable
                .ok_or_else(|| invalid("complete deletion source mode unknown"))?,
        });
    }
    let token = recovery_export::image_digest(&report, &planned.deletions, &[])?;
    let digest = token
        .strip_prefix("blake3:")
        .filter(|s| s.len() == 64)
        .ok_or_else(|| invalid("complete managed image digest malformed"))?
        .to_owned();
    checkpoint.revalidate_namespace()?;
    cancel.check()?;
    Ok(ImageData {
        files,
        deletions,
        digest,
    })
}
