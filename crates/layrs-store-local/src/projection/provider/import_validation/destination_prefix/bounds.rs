//! Conservative autonomous-reader envelope before historical interpreters.
//! These whole-table probes do not authenticate a row or expand prefix scope.
//! They bound every variable-width SQL value reached by the transitive readers,
//! including rows reached through capture/publication references. In particular
//! SQLite length(TEXT) stops at NUL; all probes measure the complete BLOB bytes.
use super::*;

struct Column {
    name: &'static str,
    bytes: u64,
    exact: bool,
}
const fn exact(name: &'static str, bytes: u64) -> Column {
    Column {
        name,
        bytes,
        exact: true,
    }
}
const fn bounded(name: &'static str, bytes: u64) -> Column {
    Column {
        name,
        bytes,
        exact: false,
    }
}

pub(super) fn preflight(c: &Connection, cancel: Option<&Cancellation>) -> CauseResult<()> {
    let tables: &[(&str, &[Column])] = &[
        (
            "projection_snapshots",
            &[
                exact("digest", 64),
                exact("parent_digest", 64),
                bounded("payload", 67108864),
            ],
        ),
        (
            "projection_selections",
            &[exact("digest", 64), bounded("payload", 16777216)],
        ),
        (
            "projection_captures",
            &[exact("digest", 71), bounded("payload", 8388608)],
        ),
        (
            "projection_publications",
            &[
                exact("request_digest", 71),
                exact("receipt_digest", 71),
                bounded("receipt_bytes", 25165824),
                bounded("next_snapshot", 67108864),
                exact("projection_digest", 64),
                bounded("after_state", 68157440),
            ],
        ),
        (
            "projection_provider_catalogs",
            &[
                exact("id", 71),
                exact("source_digest", 64),
                exact("selection_digest", 64),
                bounded("payload", 67108864),
            ],
        ),
        (
            "projection_provider_descriptors",
            &[
                exact("catalog_id", 71),
                bounded("path", 4096),
                exact("file_id", 71),
                exact("manifest_id", 71),
                bounded("manifest", 262144),
            ],
        ),
        (
            "projection_provider_activations",
            &[
                exact("id", 71),
                exact("owner_identity_digest", 71),
                bounded("mount_id", 128),
                exact("catalog_id", 71),
                exact("source_digest", 64),
                exact("activated_digest", 64),
                bounded("payload", 1048576),
            ],
        ),
        (
            "projection_provider_transition_receipts",
            &[
                exact("id", 71),
                exact("owner_identity_digest", 71),
                bounded("transaction_id", 128),
                exact("companion_id", 71),
                bounded("payload", 65536),
            ],
        ),
        (
            "projection_provider_companions",
            &[
                exact("id", 71),
                exact("files_digest", 71),
                exact("deletions_digest", 71),
                bounded("payload", 65536),
            ],
        ),
        (
            "projection_provider_companion_files",
            &[
                exact("companion_id", 71),
                bounded("path", 4096),
                exact("file_id", 71),
                exact("manifest_id", 71),
                exact("base_file_id", 71),
                bounded("reason", 64),
                exact("digest", 71),
            ],
        ),
        (
            "projection_provider_companion_deletions",
            &[
                exact("companion_id", 71),
                bounded("path", 4096),
                exact("base_file_id", 71),
                exact("base_manifest_id", 71),
                bounded("reason", 64),
                exact("digest", 71),
            ],
        ),
        (
            "projection_content",
            &[exact("file_id", 71), exact("manifest_id", 71)],
        ),
    ];
    let mut aggregate = 0u64;
    for (table, columns) in tables {
        super::super::super::cancellation::check(cancel)?;
        // The schema is fingerprinted by our sole caller before entry. These
        // identifiers come exclusively from the static list above.
        let count: u64 = c.query_row(
            &format!("SELECT count(*) FROM (SELECT 1 FROM {table} LIMIT 1000001)"),
            [],
            |r| r.get(0),
        )?;
        if count > 1_000_000 {
            return Err(invalid(format!(
                "destination prefix historical row bound: {table}"
            )));
        }
        let bad = columns
            .iter()
            .map(|column| {
                let op = if column.exact { "!=" } else { ">" };
                format!("length(CAST({} AS BLOB)){op}{}", column.name, column.bytes)
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        if c.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE {bad})"),
            [],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid(format!(
                "destination prefix historical scalar byte bound: {table}"
            )));
        }
        // NULL optional fields contribute no bytes. Every INTEGER has a fixed
        // allocation size; a conservative 64-byte row frame covers all of them.
        let projected = columns
            .iter()
            .map(|column| format!("coalesce(length(CAST({} AS BLOB)),0)", column.name))
            .collect::<Vec<_>>()
            .join("+");
        let bytes: u64 = c.query_row(
            &format!("SELECT coalesce(sum(64+{projected}),0) FROM {table}"),
            [],
            |r| r.get(0),
        )?;
        aggregate = aggregate
            .checked_add(bytes)
            .filter(|n| *n <= 2 * 1024 * 1024 * 1024)
            .ok_or_else(|| invalid("destination prefix historical aggregate byte bound"))?;
    }
    Ok(())
}
