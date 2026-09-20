//! Opt-in real HTTPS large-file qualification. Never retain a whole file in the fixture.
use layrs_client_core::native_scm::{NativeRemote, NativeWorkspace};
use layrs_core::scm::{FileId, StepId, TreeEntryTarget};
use layrs_store::scm::ScmReader;
use layrs_store_local::scm_v1::{self, NativeScmReader};
use layrs_sync::v3::{LayerMutationMode, ReceiptStatus};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
#[path = "streaming_volume_memory.rs"]
mod memory;
const BLOCK: usize = 1024 * 1024;
const ASSET: &str = "unique-large.bin";

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    file: FileId,
    digest: String,
    bytes: u64,
}
impl Identity {
    fn json(&self) -> Value {
        json!({"fileId":self.file,"blake3":self.digest,"bytes":self.bytes})
    }
}

fn fill_block(buffer: &mut [u8], index: u64, changed: bool, case_mib: usize) {
    let mut seed = blake3::Hasher::new();
    seed.update(b"layrs-streaming-volume-fixture-v1\0");
    seed.update(&(case_mib as u64).to_le_bytes());
    seed.update(&index.to_le_bytes());
    seed.update(&[u8::from(changed)]);
    seed.finalize_xof().fill(buffer);
}
fn file_hasher() -> blake3::Hasher {
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-file-v1\0");
    hash
}
fn finish(file: blake3::Hasher, raw: blake3::Hasher, bytes: u64) -> Identity {
    Identity {
        file: FileId::parse(format!("blake3:{}", file.finalize())).unwrap(),
        digest: raw.finalize().to_hex().to_string(),
        bytes,
    }
}
fn generate(path: &Path, blocks: usize) -> Identity {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    let mut block = vec![0; BLOCK];
    let mut file = file_hasher();
    let mut raw = blake3::Hasher::new();
    let mut unique = BTreeSet::new();
    for index in 0..blocks {
        fill_block(&mut block, index as u64, false, blocks);
        assert!(unique.insert(blake3::hash(&block).to_hex().to_string()));
        output.write_all(&block).unwrap();
        file.update(&block);
        raw.update(&block);
    }
    output.sync_all().unwrap();
    finish(file, raw, blocks as u64 * BLOCK as u64)
}
fn verify(path: &Path, blocks: usize, edited: bool) -> Identity {
    let mut input = File::open(path).unwrap();
    assert_eq!(input.metadata().unwrap().len(), (blocks * BLOCK) as u64);
    let mut actual = vec![0; BLOCK];
    let mut expected = vec![0; BLOCK];
    let mut file = file_hasher();
    let mut raw = blake3::Hasher::new();
    for index in 0..blocks {
        input.read_exact(&mut actual).unwrap();
        fill_block(
            &mut expected,
            index as u64,
            edited && index == blocks / 2,
            blocks,
        );
        assert!(actual == expected, "streamed block {index} differs");
        file.update(&actual);
        raw.update(&actual);
    }
    assert_eq!(input.read(&mut [0u8; 1]).unwrap(), 0);
    finish(file, raw, (blocks * BLOCK) as u64)
}
fn edit(path: &Path, blocks: usize) {
    let mut block = vec![0; BLOCK];
    fill_block(&mut block, (blocks / 2) as u64, true, blocks);
    let mut file = OpenOptions::new().write(true).open(path).unwrap();
    file.seek(SeekFrom::Start((blocks / 2 * BLOCK) as u64))
        .unwrap();
    file.write_all(&block).unwrap();
    file.sync_all().unwrap();
}
fn tip(workspace: &NativeWorkspace) -> StepId {
    let state = workspace.state().unwrap();
    assert_eq!(state.view.layer_heads().len(), 1);
    let heads = state.view.layer_heads()[0].step_ids();
    assert_eq!(heads.len(), 1);
    heads[0].clone()
}
fn verify_tree(workspace: &NativeWorkspace, step: &StepId, expected: &Identity) {
    let store = scm_v1::open(workspace.store_root()).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let step = reader.get_step(step).unwrap().unwrap();
    let tree = reader.get_tree(step.root_tree_id()).unwrap().unwrap();
    let entry = tree
        .entries()
        .iter()
        .find(|entry| entry.name().as_str() == ASSET)
        .unwrap();
    let TreeEntryTarget::File { file_id } = entry.target() else {
        panic!("ordinary file expected");
    };
    assert_eq!(file_id, &expected.file);
}

struct Report {
    output: File,
    started: Instant,
}
impl Report {
    fn event(&mut self, value: Value) {
        serde_json::to_writer(&mut self.output, &value).unwrap();
        self.output.write_all(b"\n").unwrap();
        self.output.sync_all().unwrap();
    }
    fn phase<T>(&mut self, blocks: usize, name: &str, run: impl FnOnce() -> T) -> T {
        self.event(json!({"event":"phase","state":"running","sizeMiB":blocks,
            "phase":name,"sinceStartMs":self.started.elapsed().as_millis() as u64}));
        let started = Instant::now();
        let monitor = memory::Monitor::start();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let counters = monitor.finish();
        self.event(
            json!({"event":"phase","state":if outcome.is_ok(){"passed"}else{"failed"},
            "sizeMiB":blocks,"phase":name,"elapsedMs":elapsed_ms,
            "sampledProcessMemory":counters}),
        );
        match outcome {
            Ok(value) => value,
            Err(error) => {
                // Do not serialize panic payloads, URLs or credentials into the report.
                self.event(
                    json!({"event":"result","state":"failed","sizeMiB":blocks,"phase":name}),
                );
                std::panic::resume_unwind(error)
            }
        }
    }
}

pub fn run(remote: &NativeRemote, root: &Path) {
    let path = std::env::var_os("LAYRS_TEST_STREAMING_VOLUME_REPORT")
        .expect("streaming volume recipe requires a new report path");
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    let mut report = Report {
        output,
        started: Instant::now(),
    };
    let sizes = std::env::var("LAYRS_TEST_STREAMING_VOLUME_MIB")
        .unwrap_or_else(|_| "100,1024".into())
        .split(',')
        .map(|size| size.parse::<usize>().unwrap())
        .collect::<Vec<_>>();
    assert!(
        !sizes.is_empty()
            && sizes.len() <= 2
            && sizes.iter().all(|size| [100, 1024].contains(size))
    );
    assert!(sizes.windows(2).all(|sizes| sizes[0] < sizes[1]));
    report.event(json!({"schemaVersion":"layrs.https-streaming-volume.v1","event":"begin",
        "unixSeconds":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
        "platform":std::env::consts::OS,"processId":std::process::id(),"profile":"debug",
        "sizesMiB":sizes,"generator":"BLAKE3-XOF per unique 1MiB block, v1","sampleIntervalMs":50,
        "limits":["Fixture generation uses 1MiB, byte verification 2MiB plus bounded digest metadata.",
            "Process counters include the in-process HTTPS server and native clients, exclude PostgreSQL and OS cache.",
            "Sampled phase peaks may miss short spikes; OS lifetime peak is cumulative, not a per-phase delta.",
            "One local debug run, no production throughput guarantee. macOS memory counters unsupported.",
            "JSONL checkpoints survive phase failures; abrupt process death leaves its last phase running."]}));
    for blocks in sizes {
        let source = root.join(format!("stream-{blocks}-source"));
        let destination = root.join(format!("stream-{blocks}-clone"));
        report.phase(blocks, "create-directories", || {
            fs::create_dir(&source).unwrap();
            fs::create_dir(&destination).unwrap();
        });
        let original = report.phase(blocks, "generate-unique-bytes", || {
            generate(&source.join(ASSET), blocks)
        });
        report.event(json!({"event":"expected-content","sizeMiB":blocks,"version":"initial","identity":original.json()}));
        let workspace = report.phase(blocks, "initialize-capture", || {
            NativeWorkspace::initialize(&source, "Streaming source").unwrap()
        });
        let first_step = tip(&workspace);
        report.phase(blocks, "verify-initial-tree-file-id", || {
            verify_tree(&workspace, &first_step, &original)
        });
        let project = report.phase(blocks, "provision", || {
            remote
                .provision(&workspace, &format!("stream-{blocks}"), "project")
                .unwrap()
        });
        report.phase(blocks, "publish", || {
            assert_eq!(
                remote
                    .publish(&workspace, LayerMutationMode::Advance)
                    .unwrap()
                    .status,
                ReceiptStatus::Applied
            )
        });
        let clone = report.phase(blocks, "clone", || {
            remote
                .clone_into(&destination, &project.space_id, "Streaming clone")
                .unwrap()
        });
        report.phase(blocks, "verify-clone-bytes-and-file-id", || {
            assert_eq!(verify(&destination.join(ASSET), blocks, false), original);
            assert_eq!(tip(&clone), first_step);
            verify_tree(&clone, &first_step, &original);
        });
        report.phase(blocks, "receive-unchanged", || {
            let before = clone.state().unwrap().operation.id().clone();
            let result = remote.receive(&clone).unwrap();
            assert!(!result.materialized && !result.divergent);
            assert_eq!(clone.state().unwrap().operation.id(), &before);
        });
        report.phase(blocks, "edit-one-unique-block", || {
            edit(&source.join(ASSET), blocks)
        });
        let edited = report.phase(blocks, "verify-edited-source", || {
            verify(&source.join(ASSET), blocks, true)
        });
        assert_ne!(original.file, edited.file);
        report.event(json!({"event":"expected-content","sizeMiB":blocks,"version":"edited","identity":edited.json()}));
        report.phase(blocks, "capture-one-block", || {
            workspace.capture().unwrap();
        });
        let next_step = tip(&workspace);
        report.phase(blocks, "verify-edited-tree-file-id", || {
            assert_ne!(first_step, next_step);
            verify_tree(&workspace, &next_step, &edited);
        });
        report.phase(blocks, "publish-one-block", || {
            assert_eq!(
                remote
                    .publish(&workspace, LayerMutationMode::Advance)
                    .unwrap()
                    .status,
                ReceiptStatus::Applied
            )
        });
        report.phase(blocks, "receive-one-block", || {
            let result = remote.receive(&clone).unwrap();
            assert!(result.materialized && !result.divergent);
        });
        report.phase(blocks, "verify-received-bytes-and-file-id", || {
            assert_eq!(verify(&destination.join(ASSET), blocks, true), edited);
            assert_eq!(tip(&clone), next_step);
            verify_tree(&clone, &next_step, &edited);
        });
        if std::env::var_os("LAYRS_TEST_STREAMING_VOLUME_RESTORE").is_some() {
            let restored = report.phase(blocks, "restore-initial-step", || {
                clone.restore_step(&first_step, None).unwrap()
            });
            report.phase(blocks, "verify-restored-bytes-and-file-id", || {
                assert_eq!(verify(&destination.join(ASSET), blocks, false), original);
                verify_tree(&clone, &restored, &original);
            });
        } else {
            report.event(json!({"event":"phase","state":"skipped","sizeMiB":blocks,
                "phase":"restore-initial-step","reason":"Enable only after streaming restore API is ready."}));
        }
        report.event(json!({"event":"case-result","state":"passed","sizeMiB":blocks}));
    }
    report.event(json!({"event":"result","state":"passed","elapsedMs":report.started.elapsed().as_millis() as u64}));
}

#[test]
fn streaming_fixture_identity_matches_native_file_domain_and_changed_block() {
    let mut bytes = vec![0; BLOCK];
    fill_block(&mut bytes, 7, false, 100);
    let mut file = file_hasher();
    file.update(&bytes);
    let mut raw = blake3::Hasher::new();
    raw.update(&bytes);
    assert_eq!(
        finish(file, raw, BLOCK as u64).file,
        FileId::from_bytes(&bytes)
    );
    let original = blake3::hash(&bytes);
    fill_block(&mut bytes, 8, false, 100);
    assert_ne!(blake3::hash(&bytes), original);
    fill_block(&mut bytes, 7, true, 100);
    assert_ne!(blake3::hash(&bytes), original);
    fill_block(&mut bytes, 7, false, 1024);
    assert_ne!(blake3::hash(&bytes), original);
}
