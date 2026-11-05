//! Opt-in real TLS volume measurement; all paths belong to the disposable fixture.
use layrs_client_core::native_scm::{NativeRemote, NativeWorkspace};
use layrs_sync::v3::{LayerMutationMode, ReceiptStatus};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Instant,
};

fn relative(index: usize) -> PathBuf {
    PathBuf::from(format!("group-{:03}/file-{index:06}.bin", index / 1000))
}
fn payload(index: usize) -> Vec<u8> {
    vec![(index % 200) as u8; 256]
}
fn verify(root: &Path, count: usize, edited: bool) {
    for index in 0..count {
        let expected = if edited && index == count / 2 {
            b"incremental volume change\0\xff".to_vec()
        } else {
            payload(index)
        };
        assert_eq!(fs::read(root.join(relative(index))).unwrap(), expected);
    }
}
fn save(file: &mut File, report: &Value) {
    let bytes = serde_json::to_vec_pretty(report).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(&bytes).unwrap();
    file.set_len(bytes.len() as u64).unwrap();
    file.sync_all().unwrap();
}
fn stage(file: &mut File, report: &mut Value, case: usize, name: &str, elapsed: u128) {
    report["cases"][case]["measurements"]
        .as_array_mut()
        .unwrap()
        .push(json!({"phase":name,"elapsedMs":elapsed as u64}));
    save(file, report);
    eprintln!(
        "TLS volume {}: {name} {elapsed}ms",
        report["cases"][case]["files"]
    );
}

pub fn run(remote: &NativeRemote, root: &Path) {
    let counts = std::env::var("LAYRS_TEST_SYNC_VOLUME_COUNTS")
        .unwrap_or_else(|_| "10000,99000".into())
        .split(',')
        .map(|value| value.parse::<usize>().unwrap())
        .collect::<Vec<_>>();
    assert!(!counts.is_empty() && counts.len() <= 2);
    assert!(counts.iter().all(|n| [10_000, 99_000].contains(n)));
    let report_path = std::env::var_os("LAYRS_TEST_SYNC_VOLUMES_REPORT")
        .expect("volume recipe requires a new measurement report path");
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(report_path)
        .unwrap();
    let mut report = json!({
        "schemaVersion":"layrs.tls-volume-measurement.v1", "result":"running",
        "platform":std::env::consts::OS,"profile":"debug","cases":[],
        "limitations":[
            "Synthetic 256-byte files with only 200 unique payloads, directories counted separately.",
            "Real verified HTTPS and PostgreSQL, one publisher and one independent clone per case.",
            "One local run with warm caches and concurrent builds; not a production performance guarantee.",
            "Peak process memory, unique large files and 100 MiB/1 GiB file support are not measured.",
            "Partial report records completed phases; fixture cleanup belongs to native_clients_v3."
        ]
    });
    save(&mut output, &report);
    for (case, count) in counts.into_iter().enumerate() {
        report["cases"].as_array_mut().unwrap().push(json!({
            "files":count,"payloadBytes":count*256,"directories":count.div_ceil(1000),
            "result":"running","measurements":[]
        }));
        save(&mut output, &report);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let source = root.join(format!("volume-{count}-source"));
            let destination = root.join(format!("volume-{count}-clone"));
            fs::create_dir(&source).unwrap();
            fs::create_dir(&destination).unwrap();
            let began = Instant::now();
            for index in 0..count {
                let file = source.join(relative(index));
                if index % 1000 == 0 {
                    fs::create_dir(file.parent().unwrap()).unwrap();
                }
                fs::write(file, payload(index)).unwrap();
            }
            stage(
                &mut output,
                &mut report,
                case,
                "generate",
                began.elapsed().as_millis(),
            );
            let began = Instant::now();
            let workspace = NativeWorkspace::initialize(&source, "Volume source").unwrap();
            let initial = workspace.state().unwrap();
            stage(
                &mut output,
                &mut report,
                case,
                "initialize",
                began.elapsed().as_millis(),
            );
            let began = Instant::now();
            let project = remote
                .provision(&workspace, &format!("volume-{count}"), "project")
                .unwrap();
            assert_eq!(
                remote
                    .publish(&workspace, LayerMutationMode::Advance)
                    .unwrap()
                    .status,
                ReceiptStatus::Applied
            );
            stage(
                &mut output,
                &mut report,
                case,
                "provision-publish",
                began.elapsed().as_millis(),
            );
            let began = Instant::now();
            let clone = remote
                .clone_into(&destination, &project.space_id, "Volume clone")
                .unwrap();
            stage(
                &mut output,
                &mut report,
                case,
                "clone",
                began.elapsed().as_millis(),
            );
            let began = Instant::now();
            verify(&destination, count, false);
            assert_eq!(
                clone.state().unwrap().view.layer_heads(),
                initial.view.layer_heads()
            );
            assert!(clone.working_diff().unwrap().is_empty());
            stage(
                &mut output,
                &mut report,
                case,
                "verify-all-clone-bytes",
                began.elapsed().as_millis(),
            );
            let began = Instant::now();
            let unchanged = remote.receive(&clone).unwrap();
            assert!(!unchanged.materialized && !unchanged.divergent);
            stage(
                &mut output,
                &mut report,
                case,
                "receive-unchanged",
                began.elapsed().as_millis(),
            );
            fs::write(
                source.join(relative(count / 2)),
                b"incremental volume change\0\xff",
            )
            .unwrap();
            let began = Instant::now();
            workspace.capture().unwrap();
            assert_eq!(
                remote
                    .publish(&workspace, LayerMutationMode::Advance)
                    .unwrap()
                    .status,
                ReceiptStatus::Applied
            );
            stage(
                &mut output,
                &mut report,
                case,
                "capture-publish-one-file",
                began.elapsed().as_millis(),
            );
            let began = Instant::now();
            let changed = remote.receive(&clone).unwrap();
            assert!(changed.materialized && !changed.divergent);
            stage(
                &mut output,
                &mut report,
                case,
                "receive-one-file",
                began.elapsed().as_millis(),
            );
            let began = Instant::now();
            verify(&destination, count, true);
            assert_eq!(
                clone.state().unwrap().view.layer_heads(),
                workspace.state().unwrap().view.layer_heads()
            );
            assert!(clone.working_diff().unwrap().is_empty());
            stage(
                &mut output,
                &mut report,
                case,
                "verify-all-updated-bytes",
                began.elapsed().as_millis(),
            );
            report["cases"][case]["result"] = json!("passed");
            save(&mut output, &report);
        }));
        if let Err(failure) = outcome {
            let message = failure
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| failure.downcast_ref::<&str>().copied())
                .unwrap_or("volume scenario panicked");
            report["cases"][case]["result"] = json!("failed");
            report["cases"][case]["error"] = json!(message);
            report["result"] = json!("failed");
            save(&mut output, &report);
            std::panic::resume_unwind(failure);
        }
    }
    report["result"] = json!("passed");
    save(&mut output, &report);
}
