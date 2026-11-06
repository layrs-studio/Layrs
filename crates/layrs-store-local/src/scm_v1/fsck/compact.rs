//! Format 6 audits (ADR 0028): content lives in registered packs v2 and
//! loose objects, not in SQL rows. Every registered copy is verified at its
//! own place; FileId sections, file records and admission bitmaps are
//! checked against the decoded content; references that may name content
//! (pins, GC roots, recovery paths) are resolved through the catalog.
use super::*;
use crate::scm_v1::cas::{RegisteredCatalog, is_content};
use std::collections::HashSet;
use std::sync::Arc;

impl Context<'_> {
    /// The audit CAS view and its registered catalog, loaded once per fsck
    /// run from the metadata on disk now (never the process cache) and
    /// reused by every check.
    pub(super) fn compact_catalog(&mut self) -> Option<(NativeCas, Arc<RegisteredCatalog>)> {
        if let Some((cas, catalog)) = &self.compact_audit {
            return Some((cas.clone(), Arc::clone(catalog)));
        }
        if self.compact_audit_failed {
            return None;
        }
        let opened = NativeCas::open_audit(self.audit).and_then(|audit| {
            let cas = audit.into_inner();
            let catalog = cas
                .compact_catalog()
                .cloned()
                .ok_or(crate::scm_v1::CasError::UnsupportedPublication)?;
            Ok((cas, catalog))
        });
        match opened {
            Ok((cas, catalog)) => {
                self.compact_damaged = catalog.damage().is_some();
                self.compact_audit = Some((cas.clone(), Arc::clone(&catalog)));
                Some((cas, catalog))
            }
            Err(error) => {
                self.compact_audit_failed = true;
                self.finding(
                    FsckFindingCode::CasCorruption,
                    FsckSeverity::Error,
                    Some(self.audit.layout().objects_dir().to_path_buf()),
                    None,
                    None,
                    format!("registered CAS catalog cannot be read: {error}"),
                );
                None
            }
        }
    }

    /// One finding for a catalog answer that failed; later ones are the same
    /// damage and are not repeated.
    fn compact_lookup_failed(&mut self, error: &crate::scm_v1::CasError) {
        if self.compact_lookup_reported {
            return;
        }
        self.compact_lookup_reported = true;
        self.finding(
            FsckFindingCode::CasCorruption,
            FsckSeverity::Error,
            None,
            None,
            None,
            format!("{}: {error}", error.code()),
        );
    }

    /// Non-content `objects` rows (their bytes in a registered pack or a
    /// loose file), then every registered content object.
    pub(super) fn scan_objects_compact(&mut self) -> Result<(), FsckError> {
        let limit = self.row_limit.saturating_add(1);
        let mut rows = {
            let mut statement = self.connection.prepare(
                "SELECT object_id, object_kind, byte_length FROM objects ORDER BY object_id LIMIT ?1",
            )?;
            let rows = statement.query_map([limit as u64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if rows.len() > self.row_limit {
            rows.truncate(self.row_limit);
            self.work_truncated = true;
            self.report.truncated_count = self.report.truncated_count.saturating_add(1);
        }
        let Some((cas, catalog)) = self.compact_catalog() else {
            return Ok(());
        };
        for (id_text, kind_text, byte_length) in rows {
            self.report.checked_rows = self.report.checked_rows.saturating_add(1);
            self.report.checked_objects = self.report.checked_objects.saturating_add(1);
            let Some(kind) = parse_kind(&kind_text).filter(|kind| !is_content(*kind)) else {
                self.finding(
                    FsckFindingCode::ObjectKindMismatch,
                    FsckSeverity::Error,
                    None,
                    Some(id_text),
                    None,
                    format!("format 6 objects row of kind {kind_text}"),
                );
                continue;
            };
            let Some(object_id) = parse_object_id(kind, &id_text) else {
                self.finding(
                    FsckFindingCode::ObjectKindMismatch,
                    FsckSeverity::Error,
                    None,
                    Some(id_text),
                    None,
                    "object ID cannot be parsed for its declared kind",
                );
                continue;
            };
            let location = match cas.locate(&object_id) {
                Ok(Some(location)) => location,
                Ok(None) => {
                    self.finding(
                        FsckFindingCode::MissingCasObject,
                        FsckSeverity::Error,
                        None,
                        Some(id_text),
                        None,
                        "indexed CAS object is missing",
                    );
                    continue;
                }
                Err(error) => {
                    self.finding(
                        FsckFindingCode::CasCorruption,
                        FsckSeverity::Error,
                        None,
                        Some(id_text),
                        None,
                        error.to_string(),
                    );
                    continue;
                }
            };
            if location.length() != byte_length {
                self.finding(
                    FsckFindingCode::ObjectLengthMismatch,
                    FsckSeverity::Error,
                    Some(location.path().to_path_buf()),
                    Some(id_text.clone()),
                    None,
                    format!("indexed length {byte_length}, actual {}", location.length()),
                );
            }
            let budget = self.byte_budget_remaining;
            if !self.reserve_bytes(
                byte_length.max(location.length()),
                budget,
                &format!("CAS object {id_text}"),
            ) {
                break;
            }
            self.read_into_objects(&cas, &catalog, object_id, id_text);
        }
        if self.work_truncated {
            return Ok(());
        }
        // Every registered physical copy, at its own place: a damaged copy is
        // reported even when a sound duplicate serves its logical reads. An
        // unreadable registered pack is reported here, once.
        let mut damaged_copies = HashSet::new();
        let mut stopped = false;
        catalog.audit_registered(&cas, true, |copy| {
            self.report.checked_objects = self.report.checked_objects.saturating_add(1);
            if !self.consume_cpu(1) {
                stopped = true;
                return false;
            }
            if let Err(error) = copy.result {
                self.finding(
                    FsckFindingCode::CasCorruption,
                    FsckSeverity::Error,
                    Some(copy.path),
                    Some(copy.object.clone()),
                    None,
                    format!("{}{}: {error}", copy.message_prefix, error.code()),
                );
                damaged_copies.insert(copy.object);
            }
            true
        });
        if stopped || self.work_truncated {
            return Ok(());
        }
        // Logical objects for closure and reachability. A chunk whose every
        // copy the audit verified is present without a second read; one with
        // a damaged copy is present only if another copy reads.
        let (content, errors) = catalog.content_objects();
        for error in errors {
            self.finding(
                FsckFindingCode::CasCorruption,
                FsckSeverity::Error,
                None,
                None,
                None,
                format!("{}: {error}", error.code()),
            );
        }
        for object_id in content {
            if self.object_ids.contains_key(object_id.as_str()) {
                continue;
            }
            if !self.consume_cpu(1) {
                return Ok(());
            }
            let id_text = object_id.to_string();
            if matches!(object_id, StoreObjectId::ChunkRaw(_)) && !damaged_copies.contains(&id_text)
            {
                self.object_ids.insert(id_text, object_id);
                continue;
            }
            let length = match catalog.stored_length(&object_id) {
                Ok(length) => length.unwrap_or_default(),
                Err(error) => {
                    self.finding(
                        FsckFindingCode::CasCorruption,
                        FsckSeverity::Error,
                        None,
                        Some(id_text),
                        None,
                        format!("{}: {error}", error.code()),
                    );
                    continue;
                }
            };
            if matches!(object_id, StoreObjectId::ChunkRaw(_)) {
                if length > 64 * 1024 * 1024 || length > self.chunk_byte_budget_remaining {
                    self.exhaust_bytes(&format!("streamed Chunk {id_text}"));
                    break;
                }
                self.chunk_byte_budget_remaining -= length;
            } else {
                let budget = self.byte_budget_remaining;
                if !self.reserve_bytes(length, budget, &format!("CAS object {id_text}")) {
                    break;
                }
            }
            self.read_into_objects(&cas, &catalog, object_id, id_text);
        }
        Ok(())
    }

    fn read_into_objects(
        &mut self,
        cas: &NativeCas,
        catalog: &RegisteredCatalog,
        object_id: StoreObjectId,
        id_text: String,
    ) {
        match cas.read_registered(catalog, &object_id) {
            Ok(object) if object.object_id() == object_id => {
                self.object_ids.insert(id_text.clone(), object_id);
                if !matches!(object, CanonicalObject::ChunkRaw(_)) {
                    self.objects.insert(id_text, object);
                }
            }
            Ok(_) => self.finding(
                FsckFindingCode::ObjectKindMismatch,
                FsckSeverity::Error,
                None,
                Some(id_text),
                None,
                "decoded CAS kind or identity differs from its registration",
            ),
            Err(error) => self.finding(
                FsckFindingCode::CasCorruption,
                FsckSeverity::Error,
                None,
                Some(id_text),
                None,
                format!("{}: {error}", error.code()),
            ),
        }
    }

    /// The FileId-section projection of each manifest (file length, and the
    /// manifest listed for its FileId) and its streamed reconstruction.
    pub(super) fn check_manifest_projections_compact(&mut self) -> Result<(), FsckError> {
        let Some((_, catalog)) = self.compact_catalog() else {
            return Ok(());
        };
        let damaged = self.compact_damaged;
        let manifests: Vec<_> = self
            .objects
            .iter()
            .filter_map(|(id, object)| match object {
                CanonicalObject::FileManifest(value) => Some((id.clone(), value.clone())),
                _ => None,
            })
            .collect();
        for (id, manifest) in manifests {
            if let Err(error) = self.reconstruct_manifest_streaming(&manifest) {
                self.finding(
                    FsckFindingCode::ManifestReconstructionFailure,
                    FsckSeverity::Error,
                    None,
                    Some(id.clone()),
                    None,
                    error,
                );
            }
            // With an unreadable registered pack every FileId listing fails
            // the same way: reported once by the audit, skipped here.
            if damaged {
                continue;
            }
            let listed = match catalog.file_manifests(manifest.file_id()) {
                Ok(entries) => entries
                    .into_iter()
                    .find(|entry| &entry.id == manifest.id())
                    .map(|entry| entry.file_length),
                Err(error) => {
                    self.compact_lookup_failed(&error);
                    continue;
                }
            };
            if listed != Some(manifest.byte_length()) {
                self.projection_divergence(
                    "pack FileId section",
                    &id,
                    "manifest identity/length projection differs from canonical CAS",
                );
            }
        }
        Ok(())
    }

    /// Whether some registered manifest exists for a FileId.
    pub(super) fn file_present_compact(
        &mut self,
        file_id: &layrs_core::scm::FileId,
    ) -> Result<bool, FsckError> {
        let Some((_, catalog)) = self.compact_catalog() else {
            return Ok(true);
        };
        // An unreadable registered pack makes presence unknown, not absent:
        // it is reported once, and Tree closure is not judged on it.
        if self.compact_damaged {
            return Ok(true);
        }
        match catalog.file_manifests(file_id) {
            Ok(entries) => Ok(!entries.is_empty()),
            Err(error) => {
                self.compact_lookup_failed(&error);
                Ok(true)
            }
        }
    }

    /// Content admission: every bitmap and loose admission valid, every
    /// chunk of an admitted manifest admitted to the same Space, and every
    /// reference that may name content (pins, GC roots, recovery paths)
    /// resolving to a registered or `objects` row object.
    pub(super) fn check_content_admissions(&mut self) -> Result<(), FsckError> {
        let Some((_, catalog)) = self.compact_catalog() else {
            return Ok(());
        };
        // Admission answers of a catalog with an unreadable registered pack
        // are all errors: reported once by the audit, not judged here.
        if self.compact_damaged {
            return Ok(());
        }
        let spaces = query_strings_no_param(
            self.connection,
            "SELECT space_id FROM space_pack_admissions UNION SELECT space_id FROM space_loose_admissions",
        )?;
        let manifests: Vec<_> = self
            .objects
            .values()
            .filter_map(|object| match object {
                CanonicalObject::FileManifest(value) => Some(value.clone()),
                _ => None,
            })
            .collect();
        for space in spaces {
            let admissions = match catalog.space_admissions(self.connection, &space) {
                Ok(admissions) => admissions,
                Err(error) => {
                    self.finding(
                        FsckFindingCode::AdmissionClosureBroken,
                        FsckSeverity::Error,
                        None,
                        None,
                        Some(space),
                        error.to_string(),
                    );
                    continue;
                }
            };
            for manifest in &manifests {
                if !self.consume_cpu(1 + manifest.chunks().len()) {
                    return Ok(());
                }
                let object = StoreObjectId::FileManifest(manifest.id().clone());
                match catalog.admitted(&admissions, &object) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(error) => {
                        self.compact_lookup_failed(&error);
                        continue;
                    }
                }
                for chunk in manifest.chunks() {
                    let chunk_object = StoreObjectId::ChunkRaw(chunk.chunk_id().clone());
                    let admitted = match catalog.admitted(&admissions, &chunk_object) {
                        Ok(admitted) => admitted,
                        Err(error) => {
                            self.compact_lookup_failed(&error);
                            continue;
                        }
                    };
                    if !admitted {
                        self.finding(
                            FsckFindingCode::AdmissionClosureBroken,
                            FsckSeverity::Error,
                            None,
                            Some(manifest.id().to_string()),
                            Some(space.clone()),
                            format!(
                                "transitive object {} is not admitted to the same Space",
                                chunk.chunk_id()
                            ),
                        );
                    }
                }
            }
        }
        for (table, sql) in [
            ("object_pins", "SELECT object_id FROM object_pins"),
            ("gc_roots", "SELECT object_id FROM gc_roots"),
            (
                "recovery_paths",
                "SELECT expected_object_id FROM recovery_paths WHERE expected_object_id IS NOT NULL",
            ),
        ] {
            for id in query_strings_no_param(self.connection, sql)? {
                if !self.consume_cpu(1) {
                    return Ok(());
                }
                if !self.object_ids.contains_key(&id) {
                    self.finding(
                        FsckFindingCode::ForeignKeyViolation,
                        FsckSeverity::Error,
                        None,
                        Some(id),
                        None,
                        format!(
                            "{table} references an object that is neither registered nor in objects"
                        ),
                    );
                }
            }
        }
        Ok(())
    }
}
