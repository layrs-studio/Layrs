//! Read-only benchmark qualification, deliberately separate from timed CLI work.
//! Root builds one copy of this current auditor for both format-2 binary cohorts.
use layrs_store_local::scm_v1::{self, FsckOptions, FsckSeverity, NativeFsck};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const MAX_FINDINGS: usize = 15_625;
const ROW_LIMIT: u64 = 1_000_000;
const HISTORY_LIMIT: u64 = 256;
const METADATA_BYTES: u64 = 256 * 1024 * 1024;
const CONTENT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const PHYSICAL_ENTRY_LIMIT: u64 = 2_000_000;
const PHYSICAL_DEPTH_LIMIT: usize = 16;
// Keep aligned with Context::check_work_budget. The test below detects table drift.
const WORK_TABLES: &[&str] = &[
    "objects",
    "object_edges",
    "file_manifests",
    "file_manifest_chunks",
    "space_object_admissions",
    "steps",
    "step_parents",
    "repository_views",
    "repository_view_layer_heads",
    "repository_view_markers",
    "repository_view_releases",
    "repository_view_worktrees",
    "layers",
    "layer_parents",
    "step_observations",
    "operations",
    "operation_parents",
    "operation_rewrites",
    "operation_rewrite_targets",
    "worktrees",
    "markers",
    "marker_proofs",
    "releases",
    "release_proofs",
    "release_publications",
    "current_repository_views",
    "operation_heads",
    "object_pins",
    "gc_roots",
    "recovery_operations",
    "recovery_paths",
];

fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message).into())
    }
}

fn plain_metadata(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    require(
        (metadata.is_file() || metadata.is_dir()) && !metadata.file_type().is_symlink(),
        &format!("audit refuses a link or special entry: {}", path.display()),
    )?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        require(
            metadata.file_attributes() & 0x400 == 0,
            &format!("audit refuses a reparse point: {}", path.display()),
        )?;
    }
    Ok(metadata)
}

fn regular_file(path: &Path) -> Result<fs::Metadata> {
    let metadata = plain_metadata(path)?;
    require(metadata.is_file(), "audit input is not a regular file")?;
    Ok(metadata)
}

fn add_count(value: &mut Value, field: &str, amount: u64) -> Result<()> {
    let next = value[field]
        .as_u64()
        .ok_or("missing accounting counter")?
        .checked_add(amount)
        .ok_or("accounting counter overflow")?;
    value[field] = json!(next);
    Ok(())
}

fn walk_store(root: &Path, relative: &Path, depth: usize, physical: &mut Value) -> Result<()> {
    let path = root.join(relative);
    require(
        depth as u64 <= physical["depthLimit"].as_u64().unwrap(),
        &format!("physical Store depth budget exceeded: {}", path.display()),
    )?;
    let metadata = plain_metadata(&path)?;
    if !relative.as_os_str().is_empty() {
        add_count(physical, "visitedEntries", 1)?;
        require(
            physical["visitedEntries"].as_u64().unwrap()
                <= physical["entryLimit"].as_u64().unwrap(),
            "physical Store entry budget exceeded; enumeration incomplete",
        )?;
        let first = relative.components().next().unwrap().as_os_str();
        let category = if first == "objects" {
            "objects"
        } else if relative.starts_with("metadata/repair-backups") {
            "repairBackups"
        } else if first == "metadata" {
            "metadata"
        } else if first == "staging" {
            "staging"
        } else {
            "other"
        };
        if category == "repairBackups" && metadata.is_dir() {
            add_count(&mut physical[category], "dirs", 1)?;
        }
        let database_file = [
            "metadata/layrs.sqlite3",
            "metadata/layrs.sqlite3-wal",
            "metadata/layrs.sqlite3-shm",
        ]
        .iter()
        .any(|name| relative == Path::new(name));
        if category == "repairBackups"
            || category == "other"
            || (category == "metadata" && relative != Path::new("metadata") && !database_file)
        {
            let paths = physical[category]["samplePaths"].as_array_mut().unwrap();
            if paths.len() < 8 {
                paths.push(json!(relative));
            }
        }
        if category == "staging" && relative != Path::new("staging") {
            if relative == Path::new("staging/.lease") && metadata.is_file() {
                add_count(&mut physical["staging"], "leaseFiles", 1)?;
                add_count(&mut physical["staging"], "leaseBytes", metadata.len())?;
            } else {
                add_count(&mut physical["staging"], "pendingEntries", 1)?;
                let paths = physical["staging"]["samplePaths"].as_array_mut().unwrap();
                if paths.len() < 8 {
                    paths.push(json!(relative));
                }
            }
        }
        if metadata.is_file() {
            for counter in [category, "total"] {
                add_count(&mut physical[counter], "files", 1)?;
                add_count(&mut physical[counter], "bytes", metadata.len())?;
            }
        }
    } else {
        require(metadata.is_dir(), "Store root must be a directory")?;
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(&path)? {
            let entry = entry?;
            walk_store(root, &relative.join(entry.file_name()), depth + 1, physical)?;
        }
    }
    Ok(())
}

fn account_store(root: &Path, data: &mut Value) -> Result<()> {
    account_store_with_limits(root, data, PHYSICAL_ENTRY_LIMIT, PHYSICAL_DEPTH_LIMIT)
}

// The same traversal is used by production and small boundary fixtures; the CLI
// exposes no overrides for its fixed entry/depth ceilings.
fn account_store_with_limits(
    root: &Path,
    data: &mut Value,
    entry_limit: u64,
    depth_limit: usize,
) -> Result<()> {
    // CAS stores one raw chunk/canonical envelope per indexed object. Repository
    // admission stores encoded bytes_len; complete fsck verifies every indexed path/length.
    let files = data["preflight"]["tableRows"]["objects"]
        .as_u64()
        .ok_or("missing indexed objects")?;
    let bytes = data["preflight"]["canonicalObjectBytes"]
        .as_u64()
        .ok_or("missing canonical bytes")?
        .checked_add(
            data["preflight"]["chunkBytes"]
                .as_u64()
                .ok_or("missing chunk bytes")?,
        )
        .ok_or("indexed byte count overflow")?;
    data["physicalStore"] = json!({
        "complete": false, "reconciled": false, "visitedEntries": 0,
        "entryLimit": entry_limit, "depthLimit": depth_limit,
        "indexedExpected": { "files": files, "bytes": bytes },
        "objects": { "files": 0, "bytes": 0 }, "metadata": { "files": 0, "bytes": 0, "samplePaths": [] },
        "repairBackups": { "files": 0, "bytes": 0, "dirs": 0, "samplePaths": [] },
        "other": { "files": 0, "bytes": 0, "samplePaths": [] }, "total": { "files": 0, "bytes": 0 },
        "staging": { "files": 0, "bytes": 0, "leaseFiles": 0, "leaseBytes": 0,
            "pendingEntries": 0, "samplePaths": [] }
    });
    let physical = &mut data["physicalStore"];
    let result = (|| -> Result<()> {
        walk_store(root, Path::new(""), 0, physical)?;
        physical["complete"] = json!(true);
        require(
            physical["objects"]["files"] == json!(files)
                && physical["objects"]["bytes"] == json!(bytes),
            "physical CAS file count/bytes differ from indexed objects; files retained",
        )?;
        require(
            physical["staging"]["pendingEntries"] == json!(0)
                && physical["staging"]["leaseFiles"] == json!(1)
                && physical["staging"]["leaseBytes"] == json!(0),
            "completed Store staging must contain only its empty .lease; entries retained",
        )?;
        physical["reconciled"] = json!(true);
        Ok(())
    })();
    if let Err(error) = &result {
        physical["error"] = json!(error.to_string());
    }
    result
}

fn check_sidecars(database: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm"] {
        let mut name = database.as_os_str().to_os_string();
        name.push(suffix);
        let path = PathBuf::from(name);
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                let metadata = regular_file(&path)?;
                require(
                    suffix != "-wal" || metadata.len() == 0,
                    "read-only audit refuses an uncheckpointed WAL",
                )?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

// Same encoding rules as scm_v1/sqlite.rs; that connection API is crate-private.
// Encode URI metacharacters so '?' or '#' in a fixture path cannot add options.
fn immutable_uri(path: &Path) -> Result<String> {
    let canonical = fs::canonicalize(path)?;
    let text = canonical
        .to_str()
        .ok_or("database path is not UTF-8")?
        .replace('\\', "/");
    Ok(encode_uri(&text))
}

fn encode_uri(path: &str) -> String {
    let normalized = if let Some(rest) = path.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else {
        path.strip_prefix("//?/").unwrap_or(path).to_owned()
    };
    let mut uri = String::from("file:");
    for byte in normalized.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'-' | b'_' | b'.' | b'~') {
            uri.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(&mut uri, "%{byte:02X}").expect("String formatting cannot fail");
        }
    }
    uri.push_str("?immutable=1");
    uri
}

fn count_rows(connection: &Connection, table: &str, limit: u64) -> Result<u64> {
    // table comes only from the fixed lists in this example, never from argv.
    let sql = format!("SELECT count(*) FROM (SELECT 1 FROM {table} LIMIT ?1)");
    Ok(connection.query_row(&sql, [limit + 1], |row| row.get(0))?)
}

fn bounded_id(row: &rusqlite::Row<'_>, text: usize, length: usize) -> rusqlite::Result<String> {
    let size: i64 = row.get(length)?;
    if !(1..=160).contains(&size) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    row.get(text)
}

fn ids(connection: &Connection, table: &str, column: &str) -> Result<Vec<String>> {
    let sql = format!(
        "SELECT substr({column},1,160),length(CAST({column} AS BLOB)) FROM {table} ORDER BY {column} LIMIT ?1"
    );
    let mut statement = connection.prepare(&sql)?;
    let result = statement
        .query_map([HISTORY_LIMIT + 1], |row| bounded_id(row, 0, 1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    require(
        result.len() as u64 <= HISTORY_LIMIT,
        "history ID budget exceeded",
    )?;
    Ok(result)
}

fn preflight(connection: &Connection, data: &mut Value) -> Result<()> {
    let mut rows = 0_u64;
    for table in WORK_TABLES {
        let count = count_rows(connection, table, ROW_LIMIT - rows)?;
        rows += count;
        data["preflight"]["tableRows"][*table] = json!(count);
        data["preflight"]["workRows"] = json!(rows);
        require(
            rows <= ROW_LIMIT,
            "actual fsck work rows exceed 1000000; preflight is incomplete",
        )?;
    }
    // Spaces are checked by fsck's entity pass but not in its initial work-row union.
    let spaces = count_rows(connection, "spaces", HISTORY_LIMIT)?;
    data["preflight"]["tableRows"]["spaces"] = json!(spaces);
    for table in [
        "spaces",
        "steps",
        "operations",
        "worktrees",
        "step_parents",
        "operation_parents",
    ] {
        require(
            data["preflight"]["tableRows"][table]
                .as_u64()
                .is_some_and(|n| n <= HISTORY_LIMIT),
            "benchmark history preflight exceeds 256 rows in a history table",
        )?;
    }
    for (field, table, column) in [
        ("stepIds", "steps", "step_id"),
        ("operationIds", "operations", "operation_id"),
        ("worktreeIds", "worktrees", "worktree_id"),
        ("spaceIds", "spaces", "space_id"),
    ] {
        data["preflight"][field] = json!(ids(connection, table, column)?);
    }
    let mut statement = connection.prepare(
        "SELECT substr(operation_id,1,160),length(CAST(operation_id AS BLOB)), \
         substr(parent_operation_id,1,160),length(CAST(parent_operation_id AS BLOB)) \
         FROM operation_parents ORDER BY operation_id,parent_operation_id LIMIT ?1",
    )?;
    let parents = statement
        .query_map([HISTORY_LIMIT + 1], |row| {
            Ok([bounded_id(row, 0, 1)?, bounded_id(row, 2, 3)?])
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    require(
        parents.len() as u64 <= HISTORY_LIMIT,
        "Operation parent budget exceeded",
    )?;
    data["preflight"]["operationParents"] = json!(parents);
    let (canonical, chunks, unique_chunks): (u64, u64, u64) = connection.query_row(
        "SELECT coalesce(sum(CASE WHEN object_kind!='chunk_raw' THEN byte_length ELSE 0 END),0), \
         coalesce(sum(CASE WHEN object_kind='chunk_raw' THEN byte_length ELSE 0 END),0), \
         coalesce(sum(CASE WHEN object_kind='chunk_raw' THEN 1 ELSE 0 END),0) FROM objects",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let files: u64 = connection.query_row(
        "SELECT coalesce(sum(file_byte_length),0) FROM file_manifests",
        [],
        |row| row.get(0),
    )?;
    data["preflight"]["canonicalObjectBytes"] = json!(canonical);
    data["preflight"]["chunkBytes"] = json!(chunks);
    data["preflight"]["uniqueChunks"] = json!(unique_chunks);
    data["preflight"]["fileBytes"] = json!(files);
    require(
        canonical <= METADATA_BYTES && chunks <= CONTENT_BYTES && files <= CONTENT_BYTES,
        "observed object/file bytes exceed the fixed audit budgets",
    )?;
    data["preflight"]["complete"] = json!(true);
    data["preflight"]["observedCountsWithinBudgets"] = json!(true);
    Ok(())
}

fn parse_args(args: &[OsString]) -> Result<(PathBuf, Vec<PathBuf>)> {
    require(
        matches!(args.len(), 3 | 5 | 7) && args[0] == "--json" && args[1] == "--store",
        "usage: measure_worktree_audit --json --store ABSOLUTE_STORE_ROOT [--checkout ABSOLUTE_ROOT] (at most two checkouts)",
    )?;
    let root = PathBuf::from(&args[2]);
    require(root.is_absolute(), "Store root must be absolute")?;
    let mut checkouts = Vec::new();
    for pair in args[3..].chunks_exact(2) {
        let checkout = PathBuf::from(&pair[1]);
        require(
            pair[0] == "--checkout" && checkout.is_absolute(),
            "expected --checkout ABSOLUTE_ROOT",
        )?;
        require(!checkouts.contains(&checkout), "duplicate checkout root")?;
        checkouts.push(checkout);
    }
    Ok((root, checkouts))
}

fn validate_checkout(root: &Path, store_root: &Path) -> Result<()> {
    // Existing read-only admission validates the registered checkout and locator identities.
    let resolved = scm_v1::worktree_audit::resolve_store_for_audit(root)?;
    require(
        resolved == store_root,
        "checkout binding resolves to a different Store",
    )
}

fn run(data: &mut Value) -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let (root, checkouts) = parse_args(&args)?;
    // Existing layout/format admission rejects links and unsupported/non-ready Stores.
    let layout = scm_v1::NativeStoreLayout::new(&root)?;
    require(
        regular_file(layout.format_path())?.len() <= 64 * 1024,
        "FORMAT exceeds audit size limit",
    )?;
    let audit = scm_v1::open_audit(&root)?;
    require(
        scm_v1::FORMAT_VERSION == 2,
        "this benchmark auditor requires Store format 2",
    )?;
    data["storeRoot"] = json!(audit.layout().root());
    data["storeId"] = json!(audit.store_id());
    data["formatVersion"] = json!(2);
    let database = audit.layout().sqlite_path();
    regular_file(database)?;
    check_sidecars(database)?;
    let connection = Connection::open_with_flags(
        immutable_uri(database)?,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    connection.busy_timeout(Duration::from_secs(5))?;
    preflight(&connection, data)?;
    drop(connection);
    for checkout in &checkouts {
        validate_checkout(checkout, audit.layout().root())?;
        data["checkoutBindings"].as_array_mut().expect("initialized array").push(json!({
            "checkoutRoot": checkout, "storeRoot": audit.layout().root(), "validatedBeforeAndAfter": false,
        }));
    }
    let report = NativeFsck::run(
        &audit,
        FsckOptions {
            max_findings: MAX_FINDINGS,
        },
    )?;
    check_sidecars(database)?;
    for (index, checkout) in checkouts.iter().enumerate() {
        validate_checkout(checkout, audit.layout().root())?;
        data["checkoutBindings"][index]["validatedBeforeAndAfter"] = json!(true);
    }
    data["clean"] = json!(report.is_clean());
    data["complete"] = json!(!report.is_truncated());
    data["truncatedCount"] = json!(report.truncated_count);
    data["checkedObjects"] = json!(report.checked_objects);
    data["checkedRows"] = json!(report.checked_rows);
    data["retentionRoots"] = json!(report.gc_reachability.roots);
    data["reachableObjects"] = json!(report.gc_reachability.reachable_objects);
    data["unreachableObjects"] = json!(report.gc_reachability.unreachable_objects.len());
    data["reconstructibleProjections"] = json!(report.projection_rebuild_plan.actions.len());
    data["findings"] = json!(report.findings.iter().map(|finding| json!({
        "code": finding.code.as_str(), "severity": match finding.severity {
            FsckSeverity::Error => "error", FsckSeverity::Warning => "warning", FsckSeverity::Info => "info"
        }, "path": finding.path, "objectId": finding.object_id, "entityId": finding.entity_id,
        "message": finding.message,
    })).collect::<Vec<_>>());
    require(
        report.is_clean() && !report.is_truncated(),
        "audit must be clean and complete; no truncated pass",
    )?;
    account_store(audit.layout().root(), data)
}

fn main() {
    let mut data = json!({
        "schemaVersion": "layrs.worktree-audit.v4", "readOnly": true,
        "storeRoot": null, "storeId": null, "formatVersion": null,
        "checkoutBindings": [],
        "clean": false, "complete": false, "truncatedCount": null,
        "budgets": { "maxFindings": MAX_FINDINGS, "workRows": ROW_LIMIT, "historyRowsPerTable": HISTORY_LIMIT,
            "canonicalMetadataBytes": METADATA_BYTES, "chunkBytes": CONTENT_BYTES, "fileBytes": CONTENT_BYTES,
            "traversalWorkUnits": MAX_FINDINGS * 256 },
        "preflight": { "complete": false, "observedCountsWithinBudgets": false, "workRows": 0, "tableRows": {},
            "byteScope": "Metadata byte totals are lower bounds; final fsck enforces all read/traversal budgets." }
    });
    let mut error = run(&mut data).err().map(|error| error.to_string());
    let envelope = |data: &Value, error: &Option<String>| {
        json!({
            "ok": error.is_none(), "data": data,
            "error": error.as_ref().map(|message| json!({ "message": message }))
        })
    };
    let mut output = envelope(&data, &error).to_string();
    if output.len() > OUTPUT_BYTES {
        error = Some("audit JSON exceeded the 16 MiB report limit".into());
        data["findings"] = json!([]);
        data["outputTruncated"] = json!(true);
        data["clean"] = json!(false);
        data["complete"] = json!(false);
        output = envelope(&data, &error).to_string();
    }
    if writeln!(io::stdout().lock(), "{output}").is_err() {
        std::process::exit(1);
    }
    if error.is_some() {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn accounting_fixture() -> (PathBuf, Value) {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "layrs-worktree-accounting-{}-{stamp}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        for directory in [
            "objects/tree/aa/bb",
            "objects/chunk_raw/cc/dd",
            "metadata",
            "staging",
        ] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        for (path, bytes) in [
            ("objects/tree/aa/bb/canonical", 3),
            ("objects/chunk_raw/cc/dd/chunk", 5),
            ("metadata/layrs.sqlite3", 11),
            ("staging/.lease", 0),
            ("FORMAT", 2),
        ] {
            fs::write(root.join(path), vec![42; bytes]).unwrap();
        }
        eprintln!(
            "Retained physical-accounting fixture (not a real Store): {}",
            root.display()
        );
        (
            root,
            json!({ "preflight": { "tableRows": { "objects": 2 }, "canonicalObjectBytes": 3, "chunkBytes": 5 } }),
        )
    }

    #[test]
    fn physical_accounting_reconciles_categories_and_empty_stage_lease() {
        let (root, mut data) = accounting_fixture();
        account_store(&root, &mut data).unwrap();
        let physical = &data["physicalStore"];
        assert_eq!(physical["reconciled"], true);
        assert_eq!(physical["objects"], json!({ "files": 2, "bytes": 8 }));
        assert_eq!(
            physical["metadata"],
            json!({ "files": 1, "bytes": 11, "samplePaths": [] })
        );
        assert_eq!(
            physical["other"],
            json!({ "files": 1, "bytes": 2, "samplePaths": ["FORMAT"] })
        );
        assert_eq!(physical["staging"]["leaseFiles"], 1);
        assert_eq!(physical["staging"]["pendingEntries"], 0);
        assert_eq!(physical["total"], json!({ "files": 5, "bytes": 21 }));
        assert_eq!(physical["entryLimit"], 2_000_000);
        assert_eq!(physical["depthLimit"], 16);
    }

    #[test]
    fn repair_backup_payloads_are_separate_and_totals_remain_exact() {
        let (root, mut data) = accounting_fixture();
        fs::create_dir_all(root.join("metadata/repair-backups/displaced")).unwrap();
        fs::write(
            root.join("metadata/repair-backups/displaced/code.mjs"),
            [7; 19],
        )
        .unwrap();
        fs::write(
            root.join("metadata/repair-backups/displaced/asset.bin"),
            [8; 23],
        )
        .unwrap();
        account_store(&root, &mut data).unwrap();
        let physical = &data["physicalStore"];
        assert_eq!(physical["repairBackups"]["files"], 2);
        assert_eq!(physical["repairBackups"]["bytes"], 42);
        assert_eq!(physical["repairBackups"]["dirs"], 2); // Includes the backup subtree root.
        assert_eq!(
            physical["repairBackups"]["samplePaths"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
        assert_eq!(physical["metadata"]["bytes"], 11);
        assert_eq!(physical["metadata"]["files"], 1);
        assert_eq!(physical["total"], json!({ "files": 7, "bytes": 63 }));
        assert_eq!(
            fs::read(root.join("metadata/repair-backups/displaced/code.mjs")).unwrap(),
            vec![7; 19]
        );
    }

    #[test]
    fn category_samples_are_bounded_and_exclude_database_and_backup_payloads_from_metadata() {
        let (root, mut data) = accounting_fixture();
        fs::create_dir(root.join("metadata/repair-backups")).unwrap();
        // This prefix neighbor must remain metadata rather than being treated as backups.
        fs::write(root.join("metadata/repair-backups-extra"), [1]).unwrap();
        for index in 0..10 {
            for prefix in [
                "metadata/repair-backups/payload",
                "metadata/control",
                "other",
            ] {
                fs::write(root.join(format!("{prefix}-{index}")), [1]).unwrap();
            }
        }
        account_store(&root, &mut data).unwrap();
        for category in ["repairBackups", "metadata", "other"] {
            assert_eq!(
                data["physicalStore"][category]["samplePaths"]
                    .as_array()
                    .unwrap()
                    .len(),
                8
            );
        }
        assert_eq!(data["physicalStore"]["metadata"]["files"], 12);
        assert_eq!(data["physicalStore"]["metadata"]["bytes"], 22);
        assert_eq!(data["physicalStore"]["repairBackups"]["files"], 10);
        for sample in data["physicalStore"]["metadata"]["samplePaths"]
            .as_array()
            .unwrap()
        {
            let path = Path::new(sample.as_str().unwrap());
            assert!(!path.starts_with("metadata/repair-backups"));
            assert_ne!(path, Path::new("metadata/layrs.sqlite3"));
        }
    }

    #[test]
    fn production_traversal_accepts_exact_entry_and_depth_boundaries_and_rejects_one_less() {
        // This fixture has exactly 14 entries excluding root and maximum depth 5.
        for (entries, depth, error) in [
            (14, 5, None),
            (15, 6, None),
            (13, 5, Some("entry budget")),
            (14, 4, Some("depth budget")),
        ] {
            let (root, mut data) = accounting_fixture();
            let result = account_store_with_limits(&root, &mut data, entries, depth);
            if let Some(message) = error {
                assert!(result.is_err());
                assert_eq!(data["physicalStore"]["complete"], false);
                assert_eq!(data["physicalStore"]["reconciled"], false);
                assert!(
                    data["physicalStore"]["error"]
                        .as_str()
                        .unwrap()
                        .contains(message)
                );
            } else {
                result.unwrap();
                assert_eq!(data["physicalStore"]["visitedEntries"], 14);
                assert_eq!(data["physicalStore"]["reconciled"], true);
            }
            assert_eq!(
                fs::read(root.join("objects/tree/aa/bb/canonical")).unwrap(),
                vec![42; 3]
            );
        }
    }

    #[test]
    fn physical_accounting_rejects_extra_unindexed_file_and_wrong_bytes() {
        for extra_file in [true, false] {
            let (root, mut data) = accounting_fixture();
            if extra_file {
                fs::write(root.join("objects/unindexed"), []).unwrap();
            } else {
                data["preflight"]["canonicalObjectBytes"] = json!(2);
            }
            assert!(account_store(&root, &mut data).is_err());
            assert_eq!(data["physicalStore"]["complete"], true);
            assert_eq!(data["physicalStore"]["reconciled"], false);
            assert_eq!(
                data["physicalStore"]["objects"]["files"],
                if extra_file { 3 } else { 2 }
            );
            assert_eq!(data["physicalStore"]["objects"]["bytes"], 8);
            assert!(
                data["physicalStore"]["error"]
                    .as_str()
                    .unwrap()
                    .contains("physical CAS")
            );
            if extra_file {
                assert!(root.join("objects/unindexed").is_file());
            }
        }
    }

    #[test]
    fn physical_accounting_rejects_and_retains_stage_files_and_empty_directories() {
        for directory in [false, true] {
            let (root, mut data) = accounting_fixture();
            let pending = root.join("staging/unfinished");
            if directory {
                fs::create_dir(&pending).unwrap();
            } else {
                fs::write(&pending, []).unwrap();
            }
            assert!(account_store(&root, &mut data).is_err());
            assert_eq!(data["physicalStore"]["complete"], true);
            assert_eq!(data["physicalStore"]["staging"]["pendingEntries"], 1);
            assert_eq!(
                data["physicalStore"]["staging"]["samplePaths"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            assert!(pending.exists());
        }
    }

    #[test]
    fn physical_accounting_rejects_nonempty_control_lease() {
        let (root, mut data) = accounting_fixture();
        fs::write(root.join("staging/.lease"), [1]).unwrap();
        assert!(account_store(&root, &mut data).is_err());
        assert_eq!(data["physicalStore"]["staging"]["leaseBytes"], 1);
        assert_eq!(fs::read(root.join("staging/.lease")).unwrap(), vec![1]);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn physical_accounting_rejects_directory_symlink_or_windows_junction() {
        let (root, mut data) = accounting_fixture();
        fs::create_dir(root.join("outside")).unwrap();
        fs::write(root.join("outside/retained"), b"unchanged").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("outside"), root.join("objects/linked")).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Fixed relative operands only: no path interpolation into cmd syntax.
            let output = std::process::Command::new("cmd.exe")
                .current_dir(&root)
                .args(["/d", "/c", "mklink", "/J", "objects\\linked", "outside"])
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "junction fixture creation failed: {:?}",
                output.stderr
            );
        }
        assert!(account_store(&root, &mut data).is_err());
        assert_eq!(data["physicalStore"]["complete"], false);
        assert_eq!(data["physicalStore"]["reconciled"], false);
        let error = data["physicalStore"]["error"].as_str().unwrap();
        assert!(error.contains("link") || error.contains("reparse"));
        assert_eq!(
            fs::read(root.join("outside/retained")).unwrap(),
            b"unchanged"
        );
    }

    #[cfg(unix)]
    #[test]
    fn physical_accounting_rejects_special_file() {
        let (root, mut data) = accounting_fixture();
        let _socket = std::os::unix::net::UnixListener::bind(root.join("objects/socket")).unwrap();
        assert!(account_store(&root, &mut data).is_err());
        assert_eq!(data["physicalStore"]["complete"], false);
        assert!(
            data["physicalStore"]["error"]
                .as_str()
                .unwrap()
                .contains("special")
        );
    }

    #[test]
    fn preflight_table_set_matches_current_fsck() {
        let source = include_str!("../src/scm_v1/fsck.rs");
        let sql = source
            .split("const WORK_ROWS: &str = \"")
            .nth(1)
            .unwrap()
            .split("\";")
            .next()
            .unwrap();
        let tables: BTreeSet<_> = sql
            .split("FROM ")
            .skip(1)
            .map(|part| part.split_whitespace().next().unwrap())
            .collect();
        assert_eq!(tables, WORK_TABLES.iter().copied().collect());
        assert_eq!(FsckOptions::default().max_findings, 1024);
    }

    #[test]
    fn immutable_uri_encodes_metacharacters_and_windows_paths() {
        assert_eq!(
            encode_uri("//?/C:/test ?#/x%é.db"),
            "file:C:/test%20%3F%23/x%25%C3%A9.db?immutable=1"
        );
        assert_eq!(
            encode_uri("//?/UNC/server/share/a.db"),
            "file://server/share/a.db?immutable=1"
        );
    }

    #[test]
    fn bounded_row_count_distinguishes_limit_from_overflow() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE objects(id INTEGER); INSERT INTO objects VALUES(1),(2),(3);",
            )
            .unwrap();
        assert_eq!(count_rows(&connection, "objects", 2).unwrap(), 3);
        assert_eq!(count_rows(&connection, "objects", 3).unwrap(), 3);
        assert_eq!(count_rows(&connection, "objects", 0).unwrap(), 1);
    }

    #[test]
    fn checkout_arguments_are_absolute_unique_and_bounded() {
        let root = std::env::current_dir().unwrap();
        let mut args: Vec<OsString> = vec![
            "--json".into(),
            "--store".into(),
            root.join(".layrs").into(),
        ];
        assert!(parse_args(&args).unwrap().1.is_empty());
        args.extend(["--checkout".into(), root.clone().into()]);
        assert_eq!(parse_args(&args).unwrap().1, vec![root.clone()]);
        args.extend(["--checkout".into(), root.clone().into()]);
        assert!(parse_args(&args).is_err());
        args[6] = root.join("second").into();
        assert_eq!(parse_args(&args).unwrap().1.len(), 2);
        args.extend(["--checkout".into(), root.join("third").into()]);
        assert!(parse_args(&args).is_err());
        assert!(parse_args(&[]).is_err());
        assert!(parse_args(&["--json".into(), "--store".into(), "relative".into()]).is_err());
    }
}
