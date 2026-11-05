//! Storage growth qualification for the same AutomaticIdle path as Desktop.
use layrs_client_core::native_scm::{CapturePlan, NativeWorkspace};
use layrs_core::scm::CaptureReason;
use layrs_store_local::scm_v1::{self, NativeStoreLayout};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::Path, time::Instant};

fn payload(version: u64) -> Vec<u8> {
    let mut state = version + 1;
    (0..64 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}
fn tree_bytes(root: &Path) -> u64 {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            assert!(!metadata.is_symlink());
            if metadata.is_dir() {
                tree_bytes(&entry.path())
            } else {
                metadata.len()
            }
        })
        .sum()
}
fn measurement(workspace: &NativeWorkspace) -> Value {
    let layout = NativeStoreLayout::new(workspace.store_root()).unwrap();
    let connection =
        Connection::open_with_flags(layout.sqlite_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let count = |table: &str| {
        connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap()
    };
    let objects = connection
        .prepare("SELECT object_kind,count(*) FROM objects GROUP BY object_kind")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
        })
        .unwrap()
        .collect::<Result<BTreeMap<_, _>, _>>()
        .unwrap();
    let automatic: u64 = connection
        .query_row(
            "SELECT count(*) FROM step_observations WHERE capture_reason='automatic_idle'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    json!({"steps":count("steps"), "operations":count("operations"), "observations":count("step_observations"),
        "automaticObservations":automatic,"objectsByKind":objects,
        "casBytes":tree_bytes(layout.objects_dir()), "storeBytes":tree_bytes(workspace.store_root()),
        "sqliteFileBytes":fs::metadata(layout.sqlite_path()).unwrap().len()})
}
fn noop(workspace: &NativeWorkspace) {
    for _ in 0..64 {
        assert!(matches!(
            workspace
                .capture_with_reason(CaptureReason::AutomaticIdle)
                .unwrap(),
            CapturePlan::Unchanged { .. }
        ));
    }
}

#[test]
fn automatic_capture_growth_preserves_history_deduplicates_payload_and_skips_idle() {
    let started = Instant::now();
    let root = std::env::temp_dir().join(format!("layrs-capture-growth-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    fs::write(root.join("file.bin"), payload(0)).unwrap();
    let workspace = NativeWorkspace::initialize(&root, "Automatic capture growth").unwrap();
    let baseline = measurement(&workspace);
    noop(&workspace);
    let idle = measurement(&workspace);
    assert_eq!(
        baseline, idle,
        "idle polling creates no objects, history or byte growth"
    );
    let mut history = vec![];
    let unique_started = Instant::now();
    for version in 1..=32 {
        fs::write(root.join("file.bin"), payload(version)).unwrap();
        assert!(matches!(
            workspace
                .capture_with_reason(CaptureReason::AutomaticIdle)
                .unwrap(),
            CapturePlan::Change(_)
        ));
        history.push(
            workspace.state().unwrap().view.worktrees()[0]
                .base_step_id()
                .unwrap()
                .clone(),
        );
    }
    let unique_ms = unique_started.elapsed().as_millis();
    let unique = measurement(&workspace);
    let reused_started = Instant::now();
    for version in 1..=32 {
        fs::write(root.join("file.bin"), payload(version)).unwrap();
        assert!(matches!(
            workspace
                .capture_with_reason(CaptureReason::AutomaticIdle)
                .unwrap(),
            CapturePlan::Change(_)
        ));
    }
    let reused_ms = reused_started.elapsed().as_millis();
    let reused = measurement(&workspace);
    assert_eq!(unique["objectsByKind"]["chunk_raw"].as_u64(), Some(33));
    assert_eq!(reused["objectsByKind"]["chunk_raw"].as_u64(), Some(33));
    assert_eq!(
        unique["objectsByKind"]["file_manifest"],
        reused["objectsByKind"]["file_manifest"]
    );
    assert_eq!(
        unique["objectsByKind"]["tree"],
        reused["objectsByKind"]["tree"]
    );
    assert_eq!(
        reused["steps"].as_u64().unwrap() - baseline["steps"].as_u64().unwrap(),
        64
    );
    assert_eq!(reused["automaticObservations"], 64);
    let active = workspace.state().unwrap().operation.id().clone();
    noop(&workspace);
    assert_eq!(reused, measurement(&workspace));
    assert_eq!(workspace.state().unwrap().operation.id(), &active);
    for (index, step) in history.iter().enumerate() {
        let files = workspace.read_step_files(step).unwrap();
        let scm_v1::checkout::CheckoutContent::File { bytes, .. } = &files[0].content else {
            panic!("expected file")
        };
        assert_eq!(bytes, &payload(index as u64 + 1));
    }
    assert!(
        scm_v1::fsck_existing(workspace.store_root(), scm_v1::FsckOptions::default())
            .unwrap()
            .is_clean()
    );
    println!(
        "AUTOCAPTURE_GROWTH={}",
        json!({
            "schemaVersion":"layrs.auto-capture-growth.v1", "platform":std::env::consts::OS,
            "payloadBytes":65536, "uniqueChanges":32, "reusedChanges":32, "unchangedPolls":128,
            "baseline":baseline, "afterUnique":unique, "afterReused":reused,
            "uniqueCaptureMs":unique_ms, "reusedCaptureMs":reused_ms, "totalMs":started.elapsed().as_millis(),
            "historicalBytesVerified":true, "fsckClean":true,
            "limitations":["Debug synthetic fixture; not a throughput SLA.", "AutomaticIdle core path; Desktop debounce/timer cadence not simulated.", "Full history retained; no pruning and no disk allocation/block measurement."]
        })
    );
    drop(workspace);
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    fs::remove_dir_all(root).unwrap();
}
