//! Complete-Store CFAPI integration. Run each named ignored test explicitly
//! with LAYRS_VFS_TEST_PARENT pointing at a disposable directory. All native
//! roots, journals, ordinary exports and host tickets are retained as evidence.
#![cfg(windows)]
use layrs_client_core::native_scm::*;
use layrs_core::scm::*;
use layrs_store::scm::ScmReader;
use layrs_store_local::scm_v1::{
    self,
    vfs_recovery::{VfsEvent, VfsJournalFaults},
    NativeScmReader,
};
use std::{
    cell::RefCell,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Barrier},
    time::{Duration, Instant},
};

struct Fixture {
    root: PathBuf,
    source: NativeWorkspace,
    layer: LayerId,
    step: StepId,
}
impl Fixture {
    fn native() -> Self {
        let parent = PathBuf::from(
            std::env::var_os("LAYRS_VFS_TEST_PARENT")
                .expect("explicit disposable LAYRS_VFS_TEST_PARENT required"),
        );
        assert!(parent.is_absolute() && parent.is_dir());
        let root = parent.join(format!("complete-store-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let ordinary = root.join("ordinary");
        fs::create_dir(&ordinary).unwrap();
        fs::write(ordinary.join("same.txt"), vec![b'b'; 128 * 1024]).unwrap();
        fs::write(ordinary.join("untouched.bin"), pattern()).unwrap();
        let source = NativeWorkspace::initialize(&ordinary, "Complete VFS Store").unwrap();
        let state = source.state().unwrap();
        let own = state
            .view
            .worktrees()
            .iter()
            .find(|w| w.worktree_id() == source.worktree_id())
            .unwrap();
        let layer = own.active_layer_id().unwrap().clone();
        let step = own.base_step_id().unwrap().clone();
        eprintln!("complete VFS fixture retained: {}", root.display());
        Self {
            root,
            source,
            layer,
            step,
        }
    }
    fn empty(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::create_dir(&path).unwrap();
        path
    }
    fn virtual_root(&self, name: &str) -> VfsWorktree {
        VfsWorktree::create(&self.source, &self.empty(name), &self.layer, &self.step).unwrap()
    }
}
fn pattern() -> Vec<u8> {
    (0..3 * 1024 * 1024 + 19)
        .map(|n| ((n * 29 + n / 127) % 251) as u8)
        .collect()
}

fn hash_file(path: &Path) -> (FileId, u64) {
    let mut input = fs::File::open(path).unwrap();
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-file-v1\0");
    let mut buffer = [0u8; 64 * 1024];
    let mut size = 0;
    loop {
        let count = input.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        size += count as u64;
    }
    (
        FileId::parse(format!("blake3:{}", hash.finalize().to_hex())).unwrap(),
        size,
    )
}

fn journal_events(source: &NativeWorkspace, id: &WorktreeId) -> Vec<VfsEvent> {
    let directory = source
        .store_root()
        .join("metadata/vfs-v1")
        .join(blake3::hash(id.as_str().as_bytes()).to_hex().as_str());
    let mut paths: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let record: serde_json::Value =
                serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            serde_json::from_value(record["event"].clone()).unwrap()
        })
        .collect()
}

fn io_child(root: &Path, action: &str, value: &str) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "vfs_child_io", "--nocapture"])
        .env("LAYRS_COMPLETE_VFS_ROOT", root)
        .env("LAYRS_COMPLETE_VFS_ACTION", action)
        .env("LAYRS_COMPLETE_VFS_VALUE", value)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap()
}
fn wait_child(child: &mut Child) {
    wait_child_with_timeout(child, Duration::from_secs(60));
}
fn wait_child_with_timeout(child: &mut Child, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            return;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("VFS child timed out");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
fn child_write(root: &Path, value: &str) {
    wait_child(&mut io_child(root, "write", value));
}

struct HostProcess {
    child: Child,
    ticket: VfsHostTicket,
}
impl HostProcess {
    fn start(root: &Path, ticket_path: &Path) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "vfs_child_host", "--nocapture"])
            .env("LAYRS_COMPLETE_VFS_ROOT", root)
            .env("LAYRS_COMPLETE_VFS_TICKET", ticket_path)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let ticket = loop {
            if let Ok(bytes) = fs::read(ticket_path) {
                if let Ok(ticket) = serde_json::from_slice(&bytes) {
                    break ticket;
                }
            }
            if let Some(status) = child.try_wait().unwrap() {
                panic!("VFS host exited before ready: {status}");
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                panic!("VFS host readiness timed out");
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        let result = Self { child, ticket };
        assert!(matches!(
            result
                .client()
                .request_with_timeout(VfsHostCommand::Status {}, Duration::from_secs(3))
                .unwrap(),
            VfsHostResponse::Ok { .. }
        ));
        result
    }
    fn client(&self) -> VfsHostClient {
        VfsHostClient::open(&self.ticket).unwrap()
    }
    fn stop(mut self) {
        assert!(matches!(
            self.client().request(VfsHostCommand::Stop {}).unwrap(),
            VfsHostResponse::Ok { .. }
        ));
        wait_child(&mut self.child);
    }
}
impl Drop for HostProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[test]
#[ignore = "real complete-Store CFAPI recipe; explicit disposable parent required"]
fn complete_store_two_virtual_roots_capture_reconnect_export() {
    let fixture = Fixture::native();
    let source_operation = fixture.source.state().unwrap().operation.id().clone();
    let mut a = fixture.virtual_root("a");
    let mut b = fixture.virtual_root("b");
    let root_a = a.checkout_root().to_path_buf();
    let root_b = b.checkout_root().to_path_buf();
    assert_ne!(a.worktree_id(), b.worktree_id());
    assert_eq!(a.store_id(), b.store_id());
    assert_eq!(a.source_read_counts(), (0, 0));
    assert!(inspect_vfs_checkout(&root_a).unwrap().is_some());
    assert!(
        inspect_vfs_checkout(&fixture.source.checkout_root().unwrap())
            .unwrap()
            .is_none()
    );
    assert!(
        NativeWorkspace::open(&root_a).is_err(),
        "ordinary open must fail closed"
    );
    assert!(
        scm_v1::worktrees::read_locator(&root_a).is_err(),
        "old locator reader must reject VFS"
    );
    for worktree in [&mut a, &mut b] {
        let clean = worktree.capture_quiesced().unwrap();
        assert!(!clean.changed);
        assert_eq!(clean.counts.hashed_bytes, 0);
        assert_eq!(clean.counts.reused_files, 2);
        assert_eq!(
            worktree.source_read_counts(),
            (0, 0),
            "clean capture must not hydrate leaves"
        );
    }
    // An actual external kernel read must be served from the complete Store,
    // while the other root remains unhydrated and independent.
    wait_child(&mut io_child(&root_a, "pattern", ""));
    assert!(a.source_read_counts().0 > 0);
    assert_eq!(b.source_read_counts(), (0, 0));
    let id_a = a.worktree_id().clone();
    let id_b = b.worktree_id().clone();
    drop(a);
    drop(b);
    let host_a = HostProcess::start(&root_a, &fixture.root.join("host-a-ticket.json"));
    let host_b = HostProcess::start(&root_b, &fixture.root.join("host-b-ticket.json"));
    let mut write_a = io_child(&root_a, "write", "left private bytes");
    let mut write_b = io_child(&root_b, "write", "right private bytes");
    wait_child(&mut write_a);
    wait_child(&mut write_b);
    let client_a = host_a.client();
    let client_b = host_b.client();
    let (result_a, result_b) = std::thread::scope(|scope| {
        let left = scope.spawn(|| {
            client_a
                .request(VfsHostCommand::Capture {
                    writers_quiesced: true,
                })
                .unwrap()
        });
        let right = scope.spawn(|| {
            client_b
                .request(VfsHostCommand::Capture {
                    writers_quiesced: true,
                })
                .unwrap()
        });
        (left.join().unwrap(), right.join().unwrap())
    });
    let accepted = |response| match response {
        VfsHostResponse::Ok { value } => StepId::parse(value["stepId"].as_str().unwrap()).unwrap(),
        other => panic!("capture failed: {other:?}"),
    };
    let left = accepted(result_a);
    let right = accepted(result_b);
    assert_ne!(left, right);
    host_a.stop();
    host_b.stop();
    let mut a = VfsWorktree::open(&root_a).unwrap();
    let b = VfsWorktree::open(&root_b).unwrap();
    assert_eq!(a.status().unwrap().accepted_step_id, left);
    assert_eq!(b.status().unwrap().accepted_step_id, right);
    let store = fixture.source.open_store().unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let operation_a = reader
        .get_worktree(&id_a)
        .unwrap()
        .unwrap()
        .active_operation_id()
        .clone();
    let operation_b = reader
        .get_worktree(&id_b)
        .unwrap()
        .unwrap()
        .active_operation_id()
        .clone();
    let heads = reader
        .operation_heads(fixture.source.state().unwrap().space.id())
        .unwrap();
    assert!(heads.contains(&operation_a) && heads.contains(&operation_b));
    assert_eq!(
        reader.get_step(&left).unwrap().unwrap().parent_step_ids(),
        &[fixture.step.clone()]
    );
    assert_eq!(
        reader.get_step(&right).unwrap().unwrap().parent_step_ids(),
        &[fixture.step.clone()]
    );
    drop(reader);
    drop(store);
    assert_eq!(
        fixture.source.state().unwrap().operation.id(),
        &source_operation
    );
    let export = a.export_quiesced(&fixture.root).unwrap();
    assert_eq!(
        fs::read(export.tree_root.join("same.txt")).unwrap(),
        b"left private bytes"
    );
    assert_eq!(
        fs::read(export.tree_root.join("untouched.bin")).unwrap(),
        pattern()
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&export.manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["complete"], true);
    assert_eq!(manifest["files"].as_array().unwrap().len(), 2);
    fs::write(
        export.tree_root.join("same.txt"),
        b"ordinary export edited independently",
    )
    .unwrap();
    wait_child(&mut io_child(&root_a, "verify", "left private bytes"));
    let report =
        scm_v1::fsck_existing(fixture.source.store_root(), scm_v1::FsckOptions::default()).unwrap();
    assert!(report.is_clean(), "{:?}", report.findings);
    for step in [&fixture.step, &left, &right] {
        assert!(!report
            .gc_reachability
            .unreachable_objects
            .contains(&step.to_string()));
    }
}

struct FailAfterAccept;

#[test]
#[ignore = "real CFAPI streaming recovery export above 64MiB; disposable parent required"]
fn large_hydrated_and_relocated_files_export_with_streaming_hashes() {
    let mut fixture = Fixture::native();
    let size = 70 * 1024 * 1024 + 17u64;
    let large = fixture.source.checkout_root().unwrap().join("large.bin");
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&large)
        .unwrap();
    let block = [b'L'; 64 * 1024];
    let mut remaining = size;
    while remaining != 0 {
        let count = remaining.min(block.len() as u64) as usize;
        output.write_all(&block[..count]).unwrap();
        remaining -= count as u64;
    }
    output.sync_all().unwrap();
    drop(output);
    let expected_file = hash_file(&large).0;
    fixture.source.capture().unwrap();
    fixture.step = fixture
        .source
        .state()
        .unwrap()
        .view
        .worktrees()
        .iter()
        .find(|entry| entry.worktree_id() == fixture.source.worktree_id())
        .unwrap()
        .base_step_id()
        .unwrap()
        .clone();
    let source_operation = fixture.source.state().unwrap().operation.id().clone();
    for relocated in [false, true] {
        let name = if relocated {
            "relocated-large"
        } else {
            "hydrated-large"
        };
        let mut worktree = fixture.virtual_root(name);
        let root = worktree.checkout_root().to_path_buf();
        let before = worktree.status().unwrap();
        let journal_before =
            serde_json::to_vec(&journal_events(&fixture.source, worktree.worktree_id())).unwrap();
        let actual = if relocated {
            assert_eq!(worktree.source_read_counts(), (0, 0));
            drop(worktree);
            fs::rename(root.join("large.bin"), root.join("MovedLarge.bin")).unwrap();
            worktree = VfsWorktree::open(&root).unwrap();
            "MovedLarge.bin"
        } else {
            // Bounded large-recipe budget follows measured sustained range progress.
            // This is functional-test budgeting, not an SLA or performance fix.
            wait_child_with_timeout(
                &mut io_child(&root, "large", expected_file.as_str()),
                Duration::from_secs(180),
            );
            "large.bin"
        };
        let capture_error = worktree.capture_quiesced().unwrap_err();
        assert!(
            if relocated {
                capture_error.to_string().contains("Unsupported")
            } else {
                capture_error
                    .to_string()
                    .contains("VFS dirty byte budget exceeded")
            },
            "{capture_error}"
        );
        let exported = worktree.export_quiesced(&fixture.root).unwrap();
        assert_eq!(
            hash_file(&exported.tree_root.join(actual)),
            (expected_file.clone(), size)
        );
        if relocated {
            assert!(!exported.tree_root.join("large.bin").exists());
        }
        let expected_tree = Tree::new(vec![
            TreeEntry::new(
                RepoPathComponent::parse(actual).unwrap(),
                TreeEntryTarget::file(expected_file.clone()),
                false,
            ),
            TreeEntry::new(
                RepoPathComponent::parse("same.txt").unwrap(),
                TreeEntryTarget::file(FileId::from_bytes(&vec![b'b'; 128 * 1024])),
                false,
            ),
            TreeEntry::new(
                RepoPathComponent::parse("untouched.bin").unwrap(),
                TreeEntryTarget::file(FileId::from_bytes(&pattern())),
                false,
            ),
        ])
        .unwrap();
        assert_eq!(&exported.root_tree_id, expected_tree.id());
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&exported.manifest_path).unwrap()).unwrap();
        assert_eq!(manifest["rootTreeId"], expected_tree.id().to_string());
        assert_eq!(manifest["complete"], true);
        let entry = manifest["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["path"] == actual)
            .unwrap();
        assert_eq!(entry["fileId"], expected_file.to_string());
        assert_eq!(entry["byteLength"], size);
        assert_eq!(
            worktree.status().unwrap().active_operation_id,
            before.active_operation_id
        );
        assert_eq!(
            worktree.status().unwrap().accepted_step_id,
            before.accepted_step_id
        );
        assert_eq!(
            serde_json::to_vec(&journal_events(&fixture.source, worktree.worktree_id())).unwrap(),
            journal_before
        );
        assert_eq!(hash_file(&root.join(actual)), (expected_file.clone(), size));
        assert_eq!(
            fixture.source.state().unwrap().operation.id(),
            &source_operation
        );
    }
}

#[test]
#[ignore = "real CFAPI export parent identity refusal; disposable parent required"]
fn export_rejects_provider_root_descendant_and_case_alias_before_effects() {
    let fixture = Fixture::native();
    let mut worktree = fixture.virtual_root("export-parent");
    let root = worktree.checkout_root().to_path_buf();
    let child = root.join("UserDirectory");
    fs::create_dir(&child).unwrap();
    let names = || {
        let mut names: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let before_names = names();
    let before = worktree.status().unwrap();
    let journal_before =
        serde_json::to_vec(&journal_events(&fixture.source, worktree.worktree_id())).unwrap();
    for parent in [&root, &child].into_iter().flat_map(|path| {
        [
            path.clone(),
            PathBuf::from(path.to_str().unwrap().to_ascii_uppercase()),
        ]
    }) {
        let error = worktree.export_quiesced(&parent).unwrap_err();
        assert!(
            matches!(
                &error,
                NativeClientError::InvalidState(
                    "VFS recovery export parent must be outside the provider checkout"
                )
            ),
            "export parent {} returned {error:?}",
            parent.display()
        );
        assert_eq!(names(), before_names);
        assert!(fs::read_dir(&child).unwrap().next().is_none());
        assert_eq!(worktree.source_read_counts(), (0, 0));
        assert_eq!(
            worktree.status().unwrap().active_operation_id,
            before.active_operation_id
        );
        assert_eq!(
            worktree.status().unwrap().accepted_step_id,
            before.accepted_step_id
        );
        assert_eq!(
            serde_json::to_vec(&journal_events(&fixture.source, worktree.worktree_id())).unwrap(),
            journal_before
        );
    }
}

#[test]
#[ignore = "real CFAPI ordinary Unicode/available8.3 export parent; disposable parent required"]
fn unicode_and_available_short_name_parents_export_with_same_native_identity() {
    use std::os::windows::process::CommandExt;
    let fixture = Fixture::native();
    let mut worktree = fixture.virtual_root("parent-spellings");
    let parent = fixture.empty("Exports-é-測試-with-a-long-directory-name");
    let expected_identity = scm_v1::checkout::CheckoutReader::open(&parent, None)
        .unwrap()
        .identity();
    let before = worktree.status().unwrap();
    let journal_before =
        serde_json::to_vec(&journal_events(&fixture.source, worktree.worktree_id())).unwrap();
    let mut parents = vec![parent.clone()];
    // cmd's built-in path modifier queries the existing short spelling. The
    // fixed command receives the owned path through an environment variable;
    // no native FFI/dependency or volume-policy change is needed for this test.
    let probe_path = fixture.root.join("short-parent-query.utf16");
    let probe = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe_path)
        .unwrap();
    let mut child = Command::new("cmd.exe")
        .args(["/D", "/U", "/V:OFF", "/C"])
        .raw_arg("for %I in (\"%LAYRS_VFS_SHORT_PARENT%\") do @echo %~sI")
        .env("LAYRS_VFS_SHORT_PARENT", &parent)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .stdout(Stdio::from(probe))
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    wait_child(&mut child);
    let metadata = fs::metadata(&probe_path).unwrap();
    assert!(metadata.len() <= 64 * 1024 && metadata.len() % 2 == 0);
    let bytes = fs::read(&probe_path).unwrap();
    let wide: Vec<_> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let spelling = String::from_utf16(&wide).unwrap();
    let alias = PathBuf::from(spelling.trim_matches(['\r', '\n', '\u{feff}']));
    assert!(
        alias.is_absolute(),
        "invalid short-name query output: {spelling:?}"
    );
    assert_eq!(
        scm_v1::checkout::CheckoutReader::open(&alias, None)
            .unwrap_or_else(|error| panic!("invalid short-name path {alias:?}: {error}"))
            .identity(),
        expected_identity,
        "short-name query must resolve the exact owned parent"
    );
    if alias != parent
        && alias
            .components()
            .any(|part| part.as_os_str().to_string_lossy().contains('~'))
    {
        eprintln!("EXERCISED ordinary 8.3 export parent: {}", alias.display());
        parents.push(alias);
    } else {
        eprintln!(
            "UNAVAILABLE ordinary 8.3 export parent: filesystem returned no distinct short alias"
        );
    }
    for spelling in parents {
        let exported = worktree.export_quiesced(&spelling).unwrap();
        assert_eq!(
            fs::read(exported.tree_root.join("same.txt")).unwrap(),
            vec![b'b'; 128 * 1024]
        );
        assert_eq!(
            hash_file(&exported.tree_root.join("untouched.bin")),
            (FileId::from_bytes(&pattern()), pattern().len() as u64)
        );
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&exported.manifest_path).unwrap()).unwrap();
        assert_eq!(manifest["complete"], true);
        assert_eq!(
            scm_v1::checkout::CheckoutReader::open(&parent, None)
                .unwrap()
                .identity(),
            expected_identity
        );
        assert_eq!(
            worktree.status().unwrap().active_operation_id,
            before.active_operation_id
        );
        assert_eq!(
            worktree.status().unwrap().accepted_step_id,
            before.accepted_step_id
        );
        assert_eq!(
            serde_json::to_vec(&journal_events(&fixture.source, worktree.worktree_id())).unwrap(),
            journal_before
        );
    }
}

#[test]
#[ignore = "real CFAPI disconnected rename recovery export; disposable parent required"]
fn disconnected_renamed_placeholders_export_actual_bytes_without_acceptance() {
    for state in ["clean-unread", "clean-full", "dirty-full"] {
        let fixture = Fixture::native();
        let worktree = fixture.virtual_root(state);
        let root = worktree.checkout_root().to_path_buf();
        let worktree_id = worktree.worktree_id().clone();
        let before = worktree.status().unwrap();
        let source_operation = fixture.source.state().unwrap().operation.id().clone();
        let (original, expected) = if state == "dirty-full" {
            child_write(&root, "dirty relocated private bytes");
            ("same.txt", b"dirty relocated private bytes".to_vec())
        } else {
            if state == "clean-full" {
                wait_child(&mut io_child(&root, "pattern", ""));
            } else {
                assert_eq!(worktree.source_read_counts(), (0, 0));
            }
            ("untouched.bin", pattern())
        };
        let actual = format!("Relocated-{state}.bin");
        drop(worktree); // Native rename is performed while the provider is disconnected.
        fs::rename(root.join(original), root.join(&actual)).unwrap();
        let mut worktree = VfsWorktree::open(&root).unwrap();
        let journal_before =
            serde_json::to_vec(&journal_events(&fixture.source, &worktree_id)).unwrap();
        let capture_error = worktree.capture_quiesced().unwrap_err();
        assert!(
            capture_error.to_string().contains("Unsupported"),
            "{capture_error}"
        );
        assert_eq!(worktree.source_read_counts(), (0, 0));
        if state == "clean-unread" {
            let chunk_path = {
                let store = fixture.source.open_store().unwrap();
                let reader = NativeScmReader::new(&store).unwrap();
                let layrs_store::scm::FileAvailability::Present(verified) = reader
                    .file_availability(&FileId::from_bytes(&expected))
                    .unwrap()
                else {
                    panic!("fresh complete Store must contain the immutable backing");
                };
                let manifest = reader
                    .get_file_manifest(verified.verified_manifest_ids().first().unwrap())
                    .unwrap()
                    .unwrap();
                scm_v1::cas::NativeCas::open(&store)
                    .unwrap()
                    .object_path(&layrs_store::scm::StoreObjectId::ChunkRaw(
                        manifest.chunks()[0].chunk_id().clone(),
                    ))
                    .unwrap()
            };
            let saved = fixture.root.join("retained-original-chunk");
            let original_bytes = fs::read(&chunk_path).unwrap();
            fs::rename(&chunk_path, &saved).unwrap();
            let failed_exports = fixture.empty("missing-backing-exports");
            let error = worktree.export_quiesced(&failed_exports).unwrap_err();
            assert!(
                error.to_string().contains(&actual),
                "export failure must identify actual path: {error}"
            );
            assert!(
                fs::read_dir(&failed_exports).unwrap().next().is_none(),
                "failed hydration must not publish a complete recovery manifest"
            );
            assert_eq!(
                fs::metadata(root.join(&actual)).unwrap().len(),
                expected.len() as u64
            );
            assert_eq!(fs::read(&saved).unwrap(), original_bytes);
            assert_eq!(
                serde_json::to_vec(&journal_events(&fixture.source, &worktree_id)).unwrap(),
                journal_before
            );
            fs::rename(&saved, &chunk_path).unwrap();
        }
        let exported = worktree.export_quiesced(&fixture.root).unwrap();
        assert_eq!(
            fs::read(exported.tree_root.join(&actual)).unwrap(),
            expected
        );
        assert!(!exported.tree_root.join(original).exists());
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&exported.manifest_path).unwrap()).unwrap();
        assert_eq!(manifest["complete"], true);
        let files = manifest["files"].as_array().unwrap();
        assert!(!files.iter().any(|file| file["path"] == original));
        let actual_record = files.iter().find(|file| file["path"] == actual).unwrap();
        assert_eq!(
            actual_record["fileId"],
            FileId::from_bytes(&expected).to_string()
        );
        assert_eq!(actual_record["byteLength"], expected.len() as u64);
        assert!(worktree
            .capture_quiesced()
            .unwrap_err()
            .to_string()
            .contains("Unsupported"));
        assert_eq!(
            serde_json::to_vec(&journal_events(&fixture.source, &worktree_id)).unwrap(),
            journal_before
        );
        let after = worktree.status().unwrap();
        assert_eq!(after.active_operation_id, before.active_operation_id);
        assert_eq!(after.accepted_step_id, before.accepted_step_id);
        assert_eq!(
            fixture.source.state().unwrap().operation.id(),
            &source_operation
        );
        assert_eq!(fs::read(root.join(&actual)).unwrap(), expected);
    }
}

#[test]
#[ignore = "real CFAPI exact-case replacement capture/export; disposable parent required"]
fn case_replaced_directory_preserves_actual_bytes_in_capture_and_export() {
    let mut fixture = Fixture::native();
    let ordinary = fixture.source.checkout_root().unwrap();
    fs::create_dir(ordinary.join("Dir")).unwrap();
    fs::write(ordinary.join("Dir/x"), b"immutable original").unwrap();
    fixture.source.capture().unwrap();
    fixture.step = fixture
        .source
        .state()
        .unwrap()
        .view
        .worktrees()
        .iter()
        .find(|entry| entry.worktree_id() == fixture.source.worktree_id())
        .unwrap()
        .base_step_id()
        .unwrap()
        .clone();
    let mut worktree = fixture.virtual_root("case-replacement");
    let root = worktree.checkout_root().to_path_buf();
    fs::remove_file(root.join("Dir/x")).unwrap();
    fs::remove_dir(root.join("Dir")).unwrap();
    fs::create_dir(root.join("dir")).unwrap();
    let dirty = b"actual ordinary replacement must survive";
    fs::write(root.join("dir/x"), dirty).unwrap();
    let captured = worktree.capture_quiesced().unwrap();
    assert!(captured.changed);
    assert_eq!(captured.counts.hashed_bytes, dirty.len() as u64);
    assert_eq!(worktree.source_read_counts(), (0, 0));
    let export = worktree.export_quiesced(&fixture.root).unwrap();
    assert_eq!(fs::read(export.tree_root.join("dir/x")).unwrap(), dirty);
    let spellings: Vec<_> = fs::read_dir(&export.tree_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(spellings.iter().any(|name| name == "dir"));
    assert!(!spellings.iter().any(|name| name == "Dir"));
    drop(worktree);
    let mut reopened = VfsWorktree::open(&root).unwrap();
    assert!(!reopened.capture_quiesced().unwrap().changed);
    assert_eq!(
        reopened.status().unwrap().accepted_step_id,
        captured.step_id
    );
    assert_eq!(fs::read(root.join("dir/x")).unwrap(), dirty);
}

impl VfsJournalFaults for FailAfterAccept {}
impl VfsWorktreeFaults for FailAfterAccept {
    fn after_capture_accept(&self) -> io::Result<()> {
        Err(io::Error::other("injected lost acceptance reply"))
    }
}

#[test]
#[ignore = "real CFAPI same-Worktree acceptance interleaving; disposable parent required"]
fn capture_refuses_same_worktree_advance_between_scan_and_base_read() {
    same_worktree_external_advance(false, false);
    same_worktree_external_advance(true, false);
}

#[test]
#[ignore = "real CFAPI competing acceptance after durable capture intent; disposable parent required"]
fn capture_resolves_prepared_intent_false_after_same_worktree_competitor() {
    same_worktree_external_advance(false, true);
}

fn same_worktree_external_advance(after_provider_capture: bool, after_intent: bool) {
    struct AdvanceSameWorktree {
        client: NativeClient,
        change: RefCell<Option<PreparedMutation>>,
        intents: RefCell<usize>,
        resolutions: RefCell<Vec<bool>>,
        after_intent: bool,
    }
    impl AdvanceSameWorktree {
        fn accept_competitor(&self) -> io::Result<()> {
            let change = self
                .change
                .borrow_mut()
                .take()
                .expect("interleaving runs once");
            self.client.accept(&change).map_err(io::Error::other)?;
            Ok(())
        }
    }
    impl VfsJournalFaults for AdvanceSameWorktree {
        fn after_event(&self, event: &VfsEvent) -> io::Result<()> {
            if matches!(event, VfsEvent::CapturePrepared { .. }) {
                *self.intents.borrow_mut() += 1;
                if self.after_intent {
                    self.accept_competitor()?;
                }
            }
            if let VfsEvent::CaptureResolved { accepted, .. } = event {
                self.resolutions.borrow_mut().push(*accepted);
            }
            Ok(())
        }
    }
    impl VfsWorktreeFaults for AdvanceSameWorktree {
        fn before_capture_base(&self) -> io::Result<()> {
            // The scan and preliminary own-Operation check already succeeded.
            // Publish through the public client on this SAME Worktree, without
            // touching the provider's guarded files or holding a Store reader.
            if !self.after_intent {
                self.accept_competitor()?;
            }
            Ok(())
        }
    }

    let fixture = Fixture::native();
    let mut worktree = fixture.virtual_root("own-operation-race");
    let root = worktree.checkout_root().to_path_buf();
    if after_provider_capture {
        child_write(&root, "prior provider-owned capture");
        assert!(worktree.capture_quiesced().unwrap().changed);
    }
    let original = worktree.status().unwrap();
    assert_eq!(original.capture_blocked_reason, None);
    child_write(&root, "dirty bytes scanned before competing acceptance");
    let client = NativeClient::open(fixture.source.store_root()).unwrap();
    let base = client.capture_base(worktree.worktree_id()).unwrap();
    let mut snapshot = SnapshotBuilder::new(SnapshotLimits::default());
    snapshot
        .add_file(
            RepoPath::parse("same.txt").unwrap(),
            b"separately accepted bytes",
            false,
        )
        .unwrap();
    snapshot
        .add_file(RepoPath::parse("untouched.bin").unwrap(), &pattern(), false)
        .unwrap();
    let stamp = MutationStamp::fresh().unwrap();
    let accepted_operation = stamp.operation_id.clone();
    let CapturePlan::Change(change) = client
        .prepare_capture(&base, snapshot.finish().unwrap(), stamp)
        .unwrap()
    else {
        panic!("competing capture must change this Worktree");
    };
    let accepted_step = change.step_id().clone();
    let fault = AdvanceSameWorktree {
        client,
        change: RefCell::new(Some(change)),
        intents: RefCell::new(0),
        resolutions: RefCell::new(vec![]),
        after_intent,
    };
    assert!(matches!(
        worktree.capture_quiesced_with_faults(&fault),
        Err(NativeClientError::StaleCapture)
    ));
    assert!(fault.change.borrow().is_none());
    assert_eq!(
        *fault.intents.borrow(),
        usize::from(after_intent),
        "exactly one intent only when competitor runs after its durable publication"
    );
    assert_eq!(
        *fault.resolutions.borrow(),
        if after_intent { vec![false] } else { vec![] }
    );
    let status = worktree.status().unwrap();
    assert_ne!(status.active_operation_id, original.active_operation_id);
    assert_eq!(status.active_operation_id, accepted_operation);
    assert_eq!(status.accepted_step_id, accepted_step);
    assert!(!status.capture_ack_pending);
    assert_eq!(
        status.capture_blocked_reason,
        Some(VfsCaptureBlockedReason::ExternalWorktreeAdvance)
    );
    assert!(matches!(
        worktree.capture_quiesced_with_faults(&fault),
        Err(NativeClientError::InvalidState(cause)) if cause.contains("NeedsIntegration")
    ));
    assert_eq!(*fault.intents.borrow(), usize::from(after_intent));
    let store = fixture.source.open_store().unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert_eq!(
        reader
            .get_operation(&accepted_operation)
            .unwrap()
            .unwrap()
            .parent_operation_ids(),
        &[original.active_operation_id]
    );
    assert_eq!(
        reader
            .get_step(&accepted_step)
            .unwrap()
            .unwrap()
            .parent_step_ids(),
        &[original.accepted_step_id]
    );
    drop(reader);
    drop(store);
    wait_child(&mut io_child(
        &root,
        "verify",
        "dirty bytes scanned before competing acceptance",
    ));
    let export = worktree.export_quiesced(&fixture.root).unwrap();
    assert_eq!(
        fs::read(export.tree_root.join("same.txt")).unwrap(),
        b"dirty bytes scanned before competing acceptance"
    );
    drop(worktree);
    let mut reopened = VfsWorktree::open(&root).unwrap();
    assert!(matches!(
        reopened.capture_quiesced_with_faults(&fault),
        Err(NativeClientError::InvalidState(cause)) if cause.contains("NeedsIntegration")
    ));
    assert_eq!(*fault.intents.borrow(), usize::from(after_intent));
    assert_eq!(
        *fault.resolutions.borrow(),
        if after_intent { vec![false] } else { vec![] }
    );
    assert_eq!(
        reopened.status().unwrap().active_operation_id,
        accepted_operation
    );
    assert_eq!(reopened.status().unwrap().accepted_step_id, accepted_step);
    assert!(!reopened.status().unwrap().capture_ack_pending);
    assert_eq!(
        reopened.status().unwrap().capture_blocked_reason,
        Some(VfsCaptureBlockedReason::ExternalWorktreeAdvance)
    );
    let exported = reopened.export_quiesced(&fixture.root).unwrap();
    assert_eq!(
        fs::read(exported.tree_root.join("same.txt")).unwrap(),
        b"dirty bytes scanned before competing acceptance"
    );
    if after_intent {
        let events = journal_events(&fixture.source, reopened.worktree_id());
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, VfsEvent::CapturePrepared { .. }))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    VfsEvent::CaptureResolved {
                        accepted: false,
                        ..
                    }
                ))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, VfsEvent::CaptureResolved { accepted: true, .. }))
                .count(),
            0
        );
    }
}

#[test]
#[ignore = "real complete-Store receipt acknowledgment under another writer's lock"]
fn receipt_ack_waits_for_other_store_writer_without_duplicate_operation() {
    struct OtherWriter {
        store_root: PathBuf,
        worktree_id: layrs_core::scm::WorktreeId,
        accepted: RefCell<Option<layrs_core::scm::OperationId>>,
        writer: RefCell<Option<std::thread::JoinHandle<()>>>,
    }
    impl VfsJournalFaults for OtherWriter {}
    impl VfsWorktreeFaults for OtherWriter {
        fn after_capture_accept(&self) -> io::Result<()> {
            let store = scm_v1::open(&self.store_root).unwrap();
            let reader = scm_v1::NativeScmReader::new(&store).unwrap();
            *self.accepted.borrow_mut() = Some(
                reader
                    .get_worktree(&self.worktree_id)
                    .unwrap()
                    .unwrap()
                    .active_operation_id()
                    .clone(),
            );
            drop(reader);
            drop(store);
            let root = self.store_root.clone();
            let (held_tx, held_rx) = std::sync::mpsc::channel();
            *self.writer.borrow_mut() = Some(std::thread::spawn(move || {
                let writer = scm_v1::open_exclusive(root).unwrap();
                held_tx.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(400));
                drop(writer);
            }));
            // The writer owns its exclusive capability before receipt resolve
            // begins. It performs no acceptance on behalf of this Worktree.
            held_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            Ok(())
        }
    }
    let fixture = Fixture::native();
    let mut worktree = fixture.virtual_root("receipt-contention");
    let root = worktree.checkout_root().to_path_buf();
    child_write(&root, "accepted once despite competing Store writer");
    let fault = OtherWriter {
        store_root: fixture.source.store_root().into(),
        worktree_id: worktree.worktree_id().clone(),
        accepted: RefCell::new(None),
        writer: RefCell::new(None),
    };
    let captured = worktree.capture_quiesced_with_faults(&fault).unwrap();
    fault.writer.borrow_mut().take().unwrap().join().unwrap();
    assert!(captured.changed);
    assert_eq!(
        Some(&captured.active_operation_id),
        fault.accepted.borrow().as_ref()
    );
    assert!(!worktree.status().unwrap().capture_ack_pending);
    drop(worktree);
    let mut reopened = VfsWorktree::open(&root).unwrap();
    let unchanged = reopened.capture_quiesced().unwrap();
    assert!(!unchanged.changed);
    assert_eq!(unchanged.active_operation_id, captured.active_operation_id);
    assert_eq!(unchanged.step_id, captured.step_id);
}

#[test]
#[ignore = "real complete-Store CFAPI receipt and process-kill recipe"]
fn accepted_receipt_replay_and_killed_host_preserve_unaccepted_work() {
    let fixture = Fixture::native();
    let mut worktree = fixture.virtual_root("crash");
    let root = worktree.checkout_root().to_path_buf();
    child_write(&root, "accepted before journal acknowledgment");
    assert!(worktree
        .capture_quiesced_with_faults(&FailAfterAccept)
        .is_err());
    let accepted = worktree.status().unwrap();
    assert!(accepted.capture_ack_pending);
    assert_eq!(
        accepted.capture_blocked_reason,
        Some(VfsCaptureBlockedReason::AckPending)
    );
    assert_eq!(
        inspect_vfs_checkout(&root)
            .unwrap()
            .unwrap()
            .capture_blocked_reason,
        Some(VfsCaptureBlockedReason::AckPending)
    );
    drop(worktree);
    let mut worktree = VfsWorktree::open(&root).unwrap();
    let replayed = worktree.status().unwrap();
    assert!(!replayed.capture_ack_pending);
    assert_eq!(replayed.capture_blocked_reason, None);
    assert_eq!(replayed.active_operation_id, accepted.active_operation_id);
    assert!(!worktree.capture_quiesced().unwrap().changed);
    drop(worktree);
    let mut host = HostProcess::start(&root, &fixture.root.join("crash-host-ticket.json"));
    child_write(&root, "private bytes never accepted before kill");
    let client = host.client();
    host.child.kill().unwrap();
    host.child.wait().unwrap();
    assert!(client
        .request_with_timeout(VfsHostCommand::Status {}, Duration::from_secs(1))
        .is_err());
    let mut recovered = VfsWorktree::open(&root).unwrap();
    assert_eq!(
        recovered.status().unwrap().accepted_step_id,
        accepted.accepted_step_id
    );
    wait_child(&mut io_child(
        &root,
        "verify",
        "private bytes never accepted before kill",
    ));
    let exported = recovered.export_quiesced(&fixture.root).unwrap();
    assert_eq!(
        fs::read(exported.tree_root.join("same.txt")).unwrap(),
        b"private bytes never accepted before kill"
    );
    assert!(recovered.capture_quiesced().unwrap().changed);
}

struct FailEvent {
    phase: &'static str,
    locator: RefCell<Option<VfsWorktreeLocator>>,
}
impl VfsJournalFaults for FailEvent {
    fn after_event(&self, event: &VfsEvent) -> io::Result<()> {
        let name = match event {
            VfsEvent::Reserved { locator } => {
                *self.locator.borrow_mut() = Some(VfsWorktreeLocator::decode(locator).unwrap());
                "reserved"
            }
            VfsEvent::Registered => "registered",
            VfsEvent::ProviderPrepared { .. } => "prepared",
            VfsEvent::ProviderPopulated => "populated",
            VfsEvent::ControlPrepared { .. } => "control_prepared",
            VfsEvent::LocatorPublished { .. } => "locator",
            VfsEvent::ActivationPrepared { .. } => "activation_prepared",
            VfsEvent::ActivationResolved { accepted: true, .. } => "ready",
            VfsEvent::Ready => "ready",
            _ => "capture",
        };
        if name == self.phase {
            Err(io::Error::other("injected durable creation boundary"))
        } else {
            Ok(())
        }
    }
}
impl VfsWorktreeFaults for FailEvent {
    fn after_activation_accept(&self) -> io::Result<()> {
        if self.phase == "activation_accepted" {
            Err(io::Error::other("injected lost activation acknowledgment"))
        } else {
            Ok(())
        }
    }
}

struct CreationBarrier(Arc<Barrier>);
impl VfsJournalFaults for CreationBarrier {}
impl VfsWorktreeFaults for CreationBarrier {
    fn before_creation_lease(&self) -> io::Result<()> {
        self.0.wait();
        Ok(())
    }
}
impl WorktreeCreationFaults for CreationBarrier {
    fn before_creation_lease(&self) -> io::Result<()> {
        self.0.wait();
        Ok(())
    }
    fn after_locator(&self) -> io::Result<()> {
        Ok(())
    }
}

fn same_root_creation_race(mixed: bool) {
    let fixture = Fixture::native();
    let root = fixture.empty("contested");
    let original_operation = fixture.source.state().unwrap().operation.id().clone();
    let original_registrations = fixture.source.list_worktrees().unwrap().len();
    let identity = scm_v1::checkout::CheckoutReader::open(&root, None)
        .unwrap()
        .identity();
    let barrier = Arc::new(Barrier::new(2));
    let mut workers = Vec::new();
    for index in 0..2 {
        let ordinary_source = fixture.source.checkout_root().unwrap().to_path_buf();
        let target = root.clone();
        let layer = fixture.layer.clone();
        let step = fixture.step.clone();
        let faults = CreationBarrier(Arc::clone(&barrier));
        workers.push(std::thread::spawn(move || {
            let source = NativeWorkspace::open(ordinary_source).unwrap();
            if mixed && index == 0 {
                source
                    .create_worktree_with_creation_faults(
                        &target,
                        &layer,
                        &step,
                        &scm_v1::materialize::NoMaterializationFaults,
                        &faults,
                    )
                    .map(|worktree| (worktree.worktree_id().clone(), false))
                    .map_err(|error| error.to_string())
            } else {
                VfsWorktree::create_with_faults(&source, &target, &layer, &step, &faults)
                    .map(|worktree| (worktree.worktree_id().clone(), true))
                    .map_err(|error| error.to_string())
            }
        }));
    }
    let outcomes: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
        1,
        "{outcomes:?}"
    );
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_err()).count(),
        1,
        "{outcomes:?}"
    );
    let (winner, virtual_backend) = outcomes
        .into_iter()
        .find_map(|outcome| outcome.ok())
        .unwrap();
    let registrations = fixture.source.list_worktrees().unwrap();
    assert_eq!(registrations.len(), original_registrations + 1);
    let matching: Vec<_> = registrations
        .iter()
        .filter(|worktree| worktree.filesystem_identity() == identity)
        .collect();
    assert_eq!(matching.len(), 1);
    assert_eq!(matching[0].id(), &winner);
    let store = fixture.source.open_store().unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let operation = reader
        .get_operation(matching[0].active_operation_id())
        .unwrap()
        .unwrap();
    let view = reader
        .get_repository_view(operation.resulting_view_id())
        .unwrap()
        .unwrap();
    assert_eq!(
        view.worktrees()
            .iter()
            .find(|worktree| worktree.worktree_id() == &winner)
            .unwrap()
            .status(),
        WorktreeStatus::Ready
    );
    drop(reader);
    drop(store);
    assert_eq!(
        fixture.source.state().unwrap().operation.id(),
        &original_operation
    );
    if virtual_backend {
        let worktree = VfsWorktree::open(&root).unwrap();
        assert_eq!(worktree.worktree_id(), &winner);
        assert_eq!(worktree.status().unwrap().source_step_id, fixture.step);
    } else {
        assert_eq!(NativeWorkspace::open(&root).unwrap().worktree_id(), &winner);
        assert_eq!(fs::read(root.join("untouched.bin")).unwrap(), pattern());
    }
    // A duplicate Stale VFS registration poisons this full-namespace check even
    // though the new target is unrelated to the contested root.
    let followup = fixture.virtual_root("unrelated-after-race");
    assert_eq!(followup.status().unwrap().source_step_id, fixture.step);
    assert_eq!(
        fixture.source.list_worktrees().unwrap().len(),
        original_registrations + 2
    );
}

#[test]
#[ignore = "real CFAPI simultaneous creation; explicit disposable parent required"]
fn same_root_vfs_creation_accepts_exactly_one_worktree() {
    same_root_creation_race(false);
}

#[test]
#[ignore = "ordinary/CFAPI simultaneous creation; explicit disposable parent required"]
fn same_root_ordinary_and_vfs_creation_accepts_exactly_one_worktree() {
    same_root_creation_race(true);
}

#[test]
#[ignore = "real CFAPI creation fault recipe; explicit disposable parent required"]
fn durable_creation_boundaries_resume_exact_base() {
    let fixture = Fixture::native();
    for phase in [
        "reserved",
        "registered",
        "prepared",
        "populated",
        "control_prepared",
        "locator",
        "activation_prepared",
        "activation_accepted",
        "ready",
    ] {
        let fault = FailEvent {
            phase,
            locator: RefCell::new(None),
        };
        let root = fixture.empty(phase);
        assert!(VfsWorktree::create_with_faults(
            &fixture.source,
            &root,
            &fixture.layer,
            &fixture.step,
            &fault
        )
        .is_err());
        let locator = fault.locator.borrow().clone().unwrap();
        let unaccepted_activation = if phase == "activation_prepared" {
            let store = fixture.source.open_store().unwrap();
            let journal =
                scm_v1::vfs_recovery::VfsRecovery::read_reservation(&store, locator.worktree_id())
                    .unwrap();
            let pending = journal.pending_activation.as_ref().unwrap();
            let reader = NativeScmReader::new(&store).unwrap();
            assert_eq!(
                reader
                    .worktree_active_operation(locator.worktree_id())
                    .unwrap()
                    .as_ref(),
                Some(&pending.expected_registration_operation_id)
            );
            Some(pending.transaction_id.clone())
        } else {
            None
        };
        if phase == "activation_accepted" {
            assert_eq!(
                inspect_vfs_checkout(&root)
                    .unwrap()
                    .unwrap()
                    .capture_blocked_reason,
                Some(VfsCaptureBlockedReason::AckPending)
            );
        }
        let activation_operation = if matches!(phase, "activation_accepted" | "ready") {
            let store = fixture.source.open_store().unwrap();
            let reader = NativeScmReader::new(&store).unwrap();
            let worktree = reader.get_worktree(locator.worktree_id()).unwrap().unwrap();
            let journal =
                scm_v1::vfs_recovery::VfsRecovery::read_registration(&store, &worktree).unwrap();
            let intent = journal
                .activation
                .as_ref()
                .or(journal.pending_activation.as_ref())
                .unwrap();
            assert_eq!(worktree.active_operation_id(), &intent.operation_id);
            assert_eq!(intent.step_id, fixture.step);
            Some(intent.operation_id.clone())
        } else {
            None
        };
        // Source advancement must not retarget the durable creation intent.
        fs::write(
            fixture.source.checkout_root().unwrap().join("same.txt"),
            format!("source after {phase}"),
        )
        .unwrap();
        fixture.source.capture().unwrap();
        let mut resumed =
            VfsWorktree::resume_registered(&fixture.source, locator.worktree_id()).unwrap();
        assert_eq!(resumed.worktree_id(), locator.worktree_id());
        assert_eq!(resumed.checkout_root(), locator.checkout_root());
        assert_eq!(resumed.status().unwrap().source_step_id, fixture.step);
        assert_eq!(resumed.status().unwrap().capture_blocked_reason, None);
        if let Some(unaccepted) = unaccepted_activation {
            let events = journal_events(&fixture.source, resumed.worktree_id());
            let prepared: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    VfsEvent::ActivationPrepared { intent } => Some(&intent.transaction_id),
                    _ => None,
                })
                .collect();
            assert_eq!(prepared.len(), 2);
            assert_eq!(prepared[0], &unaccepted);
            assert_ne!(
                prepared[0], prepared[1],
                "activation retry needs its own exact transaction"
            );
            assert_eq!(events.iter().filter(|event| matches!(event,
                VfsEvent::ActivationResolved { transaction_id, accepted: false } if transaction_id == &unaccepted)).count(), 1);
        }
        if let Some(expected) = activation_operation {
            assert_eq!(resumed.status().unwrap().active_operation_id, expected);
            drop(resumed);
            resumed = VfsWorktree::open(&root).unwrap();
            assert_eq!(resumed.status().unwrap().active_operation_id, expected);
        }
        assert!(!resumed.capture_quiesced().unwrap().changed);
        assert_eq!(resumed.source_read_counts(), (0, 0));
    }
}

#[test]
#[ignore = "helper invoked by explicitly selected complete-Store VFS recipe"]
fn vfs_child_io() {
    let root = PathBuf::from(std::env::var_os("LAYRS_COMPLETE_VFS_ROOT").unwrap());
    let action = std::env::var("LAYRS_COMPLETE_VFS_ACTION").unwrap();
    let value = std::env::var("LAYRS_COMPLETE_VFS_VALUE").unwrap();
    match action.as_str() {
        "write" => {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(root.join("same.txt"))
                .unwrap();
            file.write_all(value.as_bytes()).unwrap();
            file.sync_all().unwrap();
        }
        "verify" => assert_eq!(fs::read(root.join("same.txt")).unwrap(), value.as_bytes()),
        "pattern" => assert_eq!(fs::read(root.join("untouched.bin")).unwrap(), pattern()),
        "large" => assert_eq!(hash_file(&root.join("large.bin")).0.as_str(), value),
        _ => panic!("unexpected native test IO action"),
    }
}

#[test]
#[ignore = "foreground host helper invoked by explicitly selected complete-Store recipe"]
fn vfs_child_host() {
    let root = PathBuf::from(std::env::var_os("LAYRS_COMPLETE_VFS_ROOT").unwrap());
    let ticket = PathBuf::from(std::env::var_os("LAYRS_COMPLETE_VFS_TICKET").unwrap());
    let host = VfsHost::bind(VfsWorktree::open(&root).unwrap()).unwrap();
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(ticket)
        .unwrap();
    output
        .write_all(&serde_json::to_vec(host.ticket()).unwrap())
        .unwrap();
    output.sync_all().unwrap();
    host.serve().unwrap();
}
