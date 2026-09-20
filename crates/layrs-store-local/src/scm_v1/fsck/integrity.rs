//! Format, schema, SQLite and canonical object integrity.
use super::*;
impl Context<'_> {
    pub(super) fn check_format(&mut self) -> Result<(), FsckError> {
        let format_path = self.audit.layout().format_path();
        let mut file = open_read_no_follow(format_path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            self.finding(
                FsckFindingCode::FormatInvalid,
                FsckSeverity::Error,
                Some(format_path.to_path_buf()),
                None,
                None,
                "FORMAT is not a regular file",
            );
            return Ok(());
        }
        let length = metadata.len();
        if !self.reserve_bytes(length, MAX_FORMAT_BYTES, "FORMAT") {
            return Ok(());
        }
        let mut bytes = Vec::with_capacity(length as usize);
        file.by_ref()
            .take(MAX_FORMAT_BYTES.saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != length {
            self.finding(
                FsckFindingCode::FormatInvalid,
                FsckSeverity::Error,
                Some(format_path.to_path_buf()),
                None,
                None,
                "FORMAT size changed during bounded inspection",
            );
            return Ok(());
        }
        let marker: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|error| FsckError::Native(format!("FORMAT is not valid JSON: {error}")))?;
        let canonical = canonical_json(&marker)
            .map_err(|error| FsckError::Native(format!("FORMAT is not canonical: {error}")))?;
        if canonical != bytes {
            self.finding(
                FsckFindingCode::FormatInvalid,
                FsckSeverity::Error,
                Some(self.audit.layout().format_path().to_path_buf()),
                None,
                None,
                "FORMAT marker is not exact canonical JSON",
            );
        }
        if marker.get("storeId").and_then(serde_json::Value::as_str)
            != Some(self.audit.store_id().as_str())
        {
            self.finding(
                FsckFindingCode::StoreIdMismatch,
                FsckSeverity::Error,
                Some(self.audit.layout().format_path().to_path_buf()),
                None,
                None,
                "FORMAT StoreId differs from the audit capability",
            );
        }
        let store_count = self
            .connection
            .query_row("SELECT COUNT(*) FROM stores", [], |row| {
                row.get::<_, u64>(0)
            });
        let indexed = if store_count == Ok(1) {
            self.connection
                .query_row(
                    "SELECT store_id, length(CAST(payload_json AS BLOB)) FROM stores",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
                )
                .ok()
        } else {
            None
        };
        let indexed = match indexed {
            Some((id, length))
                if self.reserve_bytes(length, MAX_ENTITY_PAYLOAD_BYTES, "Store payload_json") =>
            {
                let payload = self.connection.query_row(
                    "SELECT payload_json FROM stores WHERE store_id=?1",
                    [&id],
                    |row| row.get::<_, String>(0),
                )?;
                Some((id, payload))
            }
            Some(_) if self.work_truncated => return Ok(()),
            _ => None,
        };
        let expected_payload =
            canonical_json(&serde_json::json!({"storeId": self.audit.store_id().as_str()}))
                .map_err(|error| FsckError::Native(error.to_string()))?;
        let valid = matches!(
            indexed,
            Some((ref id, ref payload))
                if id == self.audit.store_id().as_str()
                    && payload.as_bytes() == expected_payload.as_slice()
                    && store_count == Ok(1)
        );
        if !valid {
            self.error(
                FsckFindingCode::StoreIdMismatch,
                Some(self.audit.store_id().to_string()),
                "stores must contain exactly the canonical FORMAT Store payload and no replacement fields",
            );
        }
        Ok(())
    }

    pub(super) fn check_schema(&mut self) {
        let meta = self.connection.query_row(
            "SELECT magic, schema_version, schema_checksum FROM meta WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        );
        match meta {
            Ok((magic, version, checksum))
                if magic == SCHEMA_MAGIC
                    && version == SCHEMA_VERSION
                    && checksum == SCHEMA_CHECKSUM => {}
            Ok((magic, version, checksum)) => self.error(
                FsckFindingCode::SchemaMetadataMismatch,
                None,
                format!("meta mismatch: magic={magic}, version={version}, checksum={checksum}"),
            ),
            Err(error) => self.error(
                FsckFindingCode::SchemaMetadataMismatch,
                None,
                format!("meta cannot be read: {error}"),
            ),
        }
        let migration = self.connection.query_row(
            "SELECT name, checksum FROM migrations WHERE version = ?1",
            [SCHEMA_VERSION],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        );
        if !matches!(migration, Ok((ref name, ref checksum)) if name == BASELINE_NAME && checksum == SCHEMA_CHECKSUM)
        {
            self.error(
                FsckFindingCode::SchemaMetadataMismatch,
                None,
                "baseline migration ledger differs from the published schema",
            );
        }
        match sqlite::schema_fingerprint(self.connection) {
            Ok(actual) if actual == SCHEMA_FINGERPRINT => {}
            Ok(actual) => self.error(
                FsckFindingCode::SchemaFingerprintMismatch,
                None,
                format!("schema fingerprint {actual} differs from {SCHEMA_FINGERPRINT}"),
            ),
            Err(error) => self.error(
                FsckFindingCode::SchemaFingerprintMismatch,
                None,
                format!("schema fingerprint failed: {error}"),
            ),
        }
    }

    pub(super) fn has_required_tables(&mut self) -> bool {
        let result = (|| -> rusqlite::Result<BTreeSet<String>> {
            let mut statement = self.connection.prepare(
                "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            )?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect()
        })();
        let Ok(actual) = result else {
            return false;
        };
        REQUIRED_TABLES.iter().all(|table| actual.contains(*table))
    }

    pub(super) fn check_sqlite_integrity(&mut self) {
        let result = (|| -> rusqlite::Result<Vec<String>> {
            let mut statement = self.connection.prepare("PRAGMA integrity_check")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect()
        })();
        match result {
            Ok(rows) => {
                for row in rows.into_iter().filter(|value| value != "ok") {
                    self.error(FsckFindingCode::SqliteIntegrityFailure, None, row);
                }
            }
            Err(error) => self.error(
                FsckFindingCode::SqliteIntegrityFailure,
                None,
                error.to_string(),
            ),
        }
    }

    pub(super) fn check_foreign_keys(&mut self) {
        let result = (|| -> rusqlite::Result<Vec<(String, i64, String, i64)>> {
            let mut statement = self.connection.prepare("PRAGMA foreign_key_check")?;
            let rows = statement.query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?;
            rows.collect()
        })();
        match result {
            Ok(rows) => {
                for (table, rowid, parent, constraint) in rows {
                    self.error(
                        FsckFindingCode::ForeignKeyViolation,
                        Some(format!("{table}:{rowid}")),
                        format!("references {parent}, foreign-key index {constraint}"),
                    );
                }
            }
            Err(error) => self.error(
                FsckFindingCode::ForeignKeyViolation,
                None,
                error.to_string(),
            ),
        }
    }

    pub(super) fn scan_objects(&mut self) -> Result<(), FsckError> {
        let limit = self.row_limit.saturating_add(1);
        let rows = {
            let mut statement = self.connection.prepare(
                "SELECT object_id, object_kind, storage_path, byte_length FROM objects ORDER BY object_id LIMIT ?1",
            )?;
            let rows = statement.query_map([limit as u64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u64>(3)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut rows = rows;
        if rows.len() > self.row_limit {
            rows.truncate(self.row_limit);
            self.work_truncated = true;
            self.report.truncated_count = self.report.truncated_count.saturating_add(1);
        }
        let cas = NativeCas::open_audit(self.audit)
            .map_err(|error| FsckError::Native(error.to_string()))?;
        for (id_text, kind_text, storage_path, byte_length) in rows {
            self.report.checked_rows = self.report.checked_rows.saturating_add(1);
            self.report.checked_objects = self.report.checked_objects.saturating_add(1);
            let Some(kind) = parse_kind(&kind_text) else {
                self.finding(
                    FsckFindingCode::ObjectKindMismatch,
                    FsckSeverity::Error,
                    None,
                    Some(id_text),
                    None,
                    format!("unknown object kind {kind_text}"),
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
            let expected_path = cas.object_path(&object_id).map_err(|error| {
                FsckError::Native(format!("cannot derive CAS path for {id_text}: {error}"))
            })?;
            let expected_storage = expected_path
                .strip_prefix(self.audit.layout().root())
                .unwrap_or(&expected_path)
                .to_string_lossy()
                .replace('\\', "/");
            if storage_path.replace('\\', "/") != expected_storage {
                self.finding(
                    FsckFindingCode::ObjectPathMismatch,
                    FsckSeverity::Error,
                    Some(expected_path.clone()),
                    Some(id_text.clone()),
                    None,
                    format!("stored path {storage_path:?} differs from {expected_storage:?}"),
                );
            }
            let actual_length = match fs::symlink_metadata(&expected_path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.finding(
                        FsckFindingCode::MissingCasObject,
                        FsckSeverity::Error,
                        Some(expected_path),
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
                        Some(expected_path),
                        Some(id_text),
                        None,
                        error.to_string(),
                    );
                    continue;
                }
                Ok(metadata) => {
                    if !metadata.is_file() || metadata.file_type().is_symlink() {
                        self.finding(
                            FsckFindingCode::CasCorruption,
                            FsckSeverity::Error,
                            Some(expected_path.clone()),
                            Some(id_text.clone()),
                            None,
                            "CAS entry is not a regular file",
                        );
                        continue;
                    }
                    if metadata.len() != byte_length {
                        self.finding(
                            FsckFindingCode::ObjectLengthMismatch,
                            FsckSeverity::Error,
                            Some(expected_path.clone()),
                            Some(id_text.clone()),
                            None,
                            format!("indexed length {byte_length}, actual {}", metadata.len()),
                        );
                    }
                    metadata.len()
                }
            };
            let inspection_length = byte_length.max(actual_length);
            if matches!(object_id, StoreObjectId::ChunkRaw(_)) {
                if inspection_length > 64 * 1024 * 1024
                    || inspection_length > self.chunk_byte_budget_remaining
                {
                    self.exhaust_bytes(&format!("streamed Chunk {id_text}"));
                    break;
                }
                self.chunk_byte_budget_remaining -= inspection_length;
            } else {
                let object_budget = self.byte_budget_remaining;
                if !self.reserve_bytes(
                    inspection_length,
                    object_budget,
                    &format!("CAS object {id_text}"),
                ) {
                    break;
                }
            }
            if matches!(object_id, StoreObjectId::ChunkRaw(_)) {
                match cas.verify(&object_id) {
                    Ok(()) => {
                        self.object_ids.insert(id_text, object_id);
                    }
                    Err(error) => self.finding(
                        FsckFindingCode::CasCorruption,
                        FsckSeverity::Error,
                        Some(expected_path),
                        Some(id_text),
                        None,
                        format!("{}: {error}", error.code()),
                    ),
                }
                continue;
            }
            match cas.read(&object_id) {
                Ok(object) => {
                    if object.kind() != kind || object.object_id() != object_id {
                        self.finding(
                            FsckFindingCode::ObjectKindMismatch,
                            FsckSeverity::Error,
                            Some(expected_path),
                            Some(id_text.clone()),
                            None,
                            "decoded CAS kind or identity differs from the objects row",
                        );
                    } else {
                        self.object_ids.insert(id_text.clone(), object_id);
                        self.objects.insert(id_text, object);
                    }
                }
                Err(error) => self.finding(
                    FsckFindingCode::CasCorruption,
                    FsckSeverity::Error,
                    Some(expected_path),
                    Some(id_text),
                    None,
                    format!("{}: {error}", error.code()),
                ),
            }
        }
        Ok(())
    }
}
