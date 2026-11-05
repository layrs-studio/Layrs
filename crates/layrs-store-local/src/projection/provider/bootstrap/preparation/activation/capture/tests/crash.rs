//! Exact owned-process cuts after observable provider capture closes. ACK is a
//! test barrier, not a claim about power-loss durability.
use super::*;
use crate::{
    projection::provider::{
        bootstrap::tests::fixture::Fixture, database_io::tests::support, records::Record,
    },
    secure_fs::PinnedDirectory,
};
use layrs_core::scm::canonical_json;
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const TRANSACTION: &str = "capture-crash-replay";
const REPLACEMENT: &[u8] = b"provider crash replay replacement";
const REQUEST_LIMIT: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Cut {
    Staging,
    Accepted,
}
impl Cut {
    fn label(self) -> &'static str {
        match self {
            Self::Staging => "staging",
            Self::Accepted => "accepted",
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    nonce: String,
    outer_identity: String,
    owner_identity: String,
    owner_digest: String,
    cut: Cut,
    allocation: Vec<u8>,
    prepared: Vec<u8>,
    binding: Vec<u8>,
}
impl Request {
    fn records(&self) -> (AllocationRecord, PreparedRecord, ProviderBootstrapBinding) {
        let allocation = AllocationRecord::decode(&self.allocation).unwrap();
        let prepared = PreparedRecord::decode(&self.prepared).unwrap();
        let binding = ProviderBootstrapBinding::decode(&self.binding).unwrap();
        // No terminal record yet: the binding check says so, nothing more.
        assert_eq!(
            binding
                .require_references(&allocation, Some(&prepared), None)
                .unwrap(),
            crate::projection::provider::records::ReceiptBindingCheck::NoTerminal
        );
        (allocation, prepared, binding)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ack {
    nonce: String,
    pid: u32,
    cut: Cut,
    source_catalog_id: String,
    transaction_digest: String,
}

fn read_request(path: &Path) -> (Request, PathBuf) {
    assert!(path.is_absolute());
    let outer = path.parent().expect("request parent").to_path_buf();
    let pin = PinnedDirectory::open(&outer).unwrap();
    let file = crate::layout::open_read_no_follow(path).unwrap();
    assert!(file.metadata().unwrap().len() <= REQUEST_LIMIT);
    let mut bytes = Vec::new();
    file.take(REQUEST_LIMIT + 1)
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(!bytes.is_empty() && bytes.len() as u64 <= REQUEST_LIMIT);
    let digest = blake3::hash(&bytes).to_hex().to_string();
    assert_eq!(
        std::env::var("LAYRS_CAPTURE_CRASH_REQUEST_DIGEST").unwrap(),
        digest
    );
    let request: Request = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(canonical_json(&request).unwrap(), bytes);
    assert!(!request.nonce.is_empty() && request.nonce.len() <= 128);
    assert!(
        request
            .nonce
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    );
    assert_eq!(
        path.file_name().unwrap(),
        std::ffi::OsStr::new(&format!("capture-request-{}.json", request.nonce))
    );
    assert_eq!(pin.identity_token(), request.outer_identity);
    let owner = support::witness(&outer.join("fixture-owner.txt"));
    assert_eq!(
        (owner.0, owner.2),
        (request.owner_identity.clone(), request.owner_digest.clone())
    );
    request.records();
    pin.verify_path_identity().unwrap();
    (request, outer)
}

/// Called only by the two `#[cfg(test)]` sites after their writer has closed.
/// A malformed or foreign request fails this exact test child before ACK.
pub(in crate::projection::provider::bootstrap::preparation::activation::capture) fn barrier(
    label: &str,
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    record: &ProviderCaptureTransactionRecord,
) {
    let Some(path) = std::env::var_os("LAYRS_CAPTURE_CRASH_HOOK_REQUEST") else {
        return;
    };
    let path = PathBuf::from(path);
    let (request, outer) = read_request(&path);
    if request.cut.label() != label {
        return;
    }
    assert_eq!(held.family.root(), outer.join("f"));
    assert_eq!(root.root(), outer.join("c"));
    assert_eq!(record.transaction_id, TRANSACTION);
    let observed = capture_sql::observe(held, root, mount, activation).unwrap();
    let exact = observed
        .capture()
        .transactions
        .iter()
        .find(|row| row.transaction_id == TRANSACTION)
        .unwrap();
    assert_eq!(exact, record);
    match request.cut {
        Cut::Staging => {
            assert!(matches!(record.state, ProviderCaptureState::Staging { .. }));
            assert!(observed.capture().patches.is_empty());
            assert!(observed.capture().receipts.is_empty());
            assert!(observed.capture().claims.is_empty());
            assert!(observed.capture().content.is_empty());
        }
        Cut::Accepted => {
            assert!(matches!(
                record.state,
                ProviderCaptureState::Accepted { .. }
            ));
            assert_eq!(observed.capture().patches.len(), 1);
            assert_eq!(observed.capture().receipts.len(), 1);
            assert_eq!(observed.capture().claims.len(), 1);
            assert_eq!(observed.capture().content.len(), 1);
        }
    }
    let ack = Ack {
        nonce: request.nonce.clone(),
        pid: std::process::id(),
        cut: request.cut,
        source_catalog_id: observed.active().source.commitment().catalog_id.clone(),
        transaction_digest: record.digest().unwrap(),
    };
    let ack_path = outer.join(format!("capture-cut-{}.json", request.nonce));
    let pending = outer.join(format!("capture-cut-{}-pending.json", request.nonce));
    support::create(&pending, &canonical_json(&ack).unwrap());
    fs::rename(&pending, &ack_path).expect("publish complete cut ACK atomically");
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(90) {
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("UNQUALIFIED: capture cut child was not terminated at its exact ACK");
}

fn child_command(request_path: &Path, digest: &str, mode: &str) -> Command {
    let target = concat!(module_path!(), "::capture_replay_child");
    let target = target.strip_prefix("layrs_store_local::").unwrap_or(target);
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            target,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("LAYRS_CAPTURE_CRASH_REQUEST", request_path)
        .env("LAYRS_CAPTURE_CRASH_REQUEST_DIGEST", digest)
        .env("LAYRS_CAPTURE_CRASH_MODE", mode)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .creation_flags(0x08000000);
    if mode == "cut" {
        command.env("LAYRS_CAPTURE_CRASH_HOOK_REQUEST", request_path);
    }
    command
}

fn wait_for_ack(child: &mut child::OwnedChild, outer: &Path, request: &Request) -> Ack {
    let path = outer.join(format!("capture-cut-{}.json", request.nonce));
    let started = Instant::now();
    loop {
        if path.try_exists().unwrap() {
            let bytes = fs::read(&path).unwrap();
            assert!(!bytes.is_empty() && bytes.len() <= 4096);
            let ack: Ack = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(ack.nonce, request.nonce);
            assert_eq!(ack.pid, child.id());
            assert_eq!(ack.cut, request.cut);
            assert_eq!(
                ack.source_catalog_id,
                Fixture::writable_file_source().commitment().catalog_id
            );
            assert_eq!(ack.transaction_digest.len(), 71);
            return ack;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("UNQUALIFIED: capture cut exited before ACK: {status}");
        }
        if started.elapsed() > Duration::from_secs(90) {
            panic!("UNQUALIFIED: exact owned capture cut ACK timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_success(child: &mut child::OwnedChild) {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(
                status.success(),
                "UNQUALIFIED: capture replay child {status}"
            );
            return;
        }
        if started.elapsed() > Duration::from_secs(90) {
            panic!("UNQUALIFIED: exact owned capture replay timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn family_counts(f: &Fixture, reservation: &str) -> (u64, u64, u64, u64, u64) {
    let c = Connection::open_with_flags(
        f.family.root().join("metadata/catalog.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    c.execute_batch("BEGIN DEFERRED").unwrap();
    let values = (
        c.query_row("SELECT count(*) FROM reservations WHERE id=?1 AND owner=?2", rusqlite::params![reservation, f.worktree.as_str()], |r| r.get(0)).unwrap(),
        c.query_row("SELECT count(*) FROM installations WHERE reservation=?1", [reservation], |r| r.get(0)).unwrap(),
        c.query_row("SELECT count(*) FROM roots WHERE owner=?1", [f.worktree.as_str()], |r| r.get(0)).unwrap(),
        c.query_row("SELECT count(*) FROM manifests", [], |r| r.get(0)).unwrap(),
        c.query_row("SELECT COALESCE((SELECT SUM(bytes) FROM chunks),0)+COALESCE((SELECT SUM(length(payload)) FROM manifests),0)", [], |r| r.get(0)).unwrap(),
    );
    c.close().unwrap();
    values
}

fn inspect_private(f: &Fixture, request: &Request) -> (validation::PrivateCaptureProof, String) {
    let (allocation, prepared, binding) = request.records();
    let c = Connection::open_with_flags(
        f.metadata.join("projection.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    c.execute_batch("BEGIN DEFERRED").unwrap();
    let inspected = validation::private_active_capture_with_source(
        &c,
        &allocation,
        &prepared,
        &binding,
        Some([0x63; 16]),
        None,
    )
    .unwrap();
    assert_eq!(
        inspected.active.source.commitment(),
        Fixture::writable_file_source().commitment(),
        "the original source binding survives the cut"
    );
    let source_catalog = inspected.active.source.commitment().catalog_id.clone();
    c.close().unwrap();
    (inspected.capture, source_catalog)
}

fn recipe(cut: Cut) {
    let (f, holder, allocation, prepared, binding) = active_holder_with_writable_file_private();
    holder.revalidate().unwrap();
    let base = Fixture::writable_file_source()
        .descriptors()
        .iter()
        .find(|row| row.path.as_str() == "README.md")
        .unwrap()
        .file_id
        .clone();
    let path = f.checkout.join("README.md");
    assert!(path.exists());
    remove_placeholder(&path);
    fs::write(&path, REPLACEMENT).unwrap();
    let outer_pin = PinnedDirectory::open(&f.outer).unwrap();
    let owner = support::witness(&f.outer.join("fixture-owner.txt"));
    let nonce = format!(
        "{:x}-{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let request = Request {
        nonce: nonce.clone(),
        outer_identity: outer_pin.identity_token(),
        owner_identity: owner.0,
        owner_digest: owner.2,
        cut,
        allocation: allocation.encode().unwrap(),
        prepared: prepared.encode().unwrap(),
        binding: binding.encode().unwrap(),
    };
    let request_path = f.outer.join(format!("capture-request-{nonce}.json"));
    let request_bytes = canonical_json(&request).unwrap();
    assert!(request_bytes.len() as u64 <= REQUEST_LIMIT);
    support::create(&request_path, &request_bytes);
    let request_digest = blake3::hash(&request_bytes).to_hex().to_string();
    let before = family_counts(&f, "unused");
    assert_eq!((before.0, before.1, before.2), (0, 0, 0));
    drop(holder);
    let mut child = child::OwnedChild::new(
        child_command(&request_path, &request_digest, "cut")
            .spawn()
            .expect("owned hidden capture cut child"),
    );
    let ack = wait_for_ack(&mut child, &f.outer, &request);
    assert!(child.try_wait().unwrap().is_none());
    child
        .kill()
        .expect("terminate only this exact owned cut handle");
    let status = child.wait().expect("reap only this exact owned cut handle");
    assert_eq!(status.code(), Some(1));
    outer_pin.verify_path_identity().unwrap();
    assert_eq!(fs::read(&request_path).unwrap(), request_bytes);
    let (prefix, source_catalog) = inspect_private(&f, &request);
    assert_eq!(ack.source_catalog_id, source_catalog);
    assert_eq!(prefix.transactions.len(), 1);
    assert_eq!(
        prefix.transactions[0].digest().unwrap(),
        ack.transaction_digest
    );
    let stage = match &prefix.transactions[0].state {
        ProviderCaptureState::Staging { stage } | ProviderCaptureState::Accepted { stage, .. } => {
            stage
        }
        other => panic!("cut left wrong capture state: {other:?}"),
    };
    let reservation = stage.reservation.clone();
    assert_eq!(stage.changes.len(), 1);
    assert_eq!(stage.changes[0].path.as_str(), "README.md");
    assert_eq!(stage.changes[0].expected.as_ref().unwrap().file_id, base);
    assert_eq!(
        stage.changes[0].replacement.as_ref().unwrap().file_id,
        FileId::from_bytes(REPLACEMENT)
    );
    let before_replay = family_counts(&f, &reservation);
    match cut {
        Cut::Staging => {
            assert!(prefix.patches.is_empty() && prefix.receipts.is_empty());
            assert!(prefix.claims.is_empty() && prefix.content.is_empty());
            assert_eq!(
                before_replay, before,
                "Staging cut precedes all family effects"
            );
        }
        Cut::Accepted => {
            assert_eq!(prefix.patches.len(), 1);
            assert_eq!(prefix.receipts.len(), 1);
            assert_eq!(prefix.claims.len(), 1);
            assert_eq!(prefix.content.len(), 1);
            assert_eq!(before_replay.0, 1, "quota reservation remains before ACK");
            assert_eq!(before_replay.1, 0, "installation is fully published");
            assert_eq!(before_replay.2, 1, "family protection is durable");
            assert_eq!(before_replay.3, before.3 + 1);
            assert!(before_replay.4 > before.4);
        }
    }
    let reopened = reopen(&f, &binding);
    reopened.revalidate().unwrap();
    drop(reopened);
    let mut replay = child::OwnedChild::new(
        child_command(&request_path, &request_digest, "resume")
            .spawn()
            .expect("owned hidden capture replay child"),
    );
    wait_success(&mut replay);
    let resume_path = f.outer.join(format!("capture-resume-{nonce}.json"));
    let resume_bytes = fs::read(&resume_path).unwrap();
    assert!(!resume_bytes.is_empty() && resume_bytes.len() <= 4096);
    let replay_ack: Ack = serde_json::from_slice(&resume_bytes).unwrap();
    assert_eq!(replay_ack.nonce, nonce);
    assert_eq!(replay_ack.pid, replay.id());
    assert_eq!(replay_ack.cut, cut);
    assert_eq!(replay_ack.source_catalog_id, source_catalog);
    let (complete, _) = inspect_private(&f, &request);
    assert_eq!(complete.transactions.len(), 1);
    assert_eq!(complete.patches.len(), 1);
    assert_eq!(complete.receipts.len(), 1);
    assert_eq!(complete.claims.len(), 1);
    assert_eq!(complete.content.len(), 1);
    assert_eq!(complete.transactions[0].transaction_id, TRANSACTION);
    assert_eq!(complete.receipts[0].transaction_id, TRANSACTION);
    assert_eq!(
        complete.transactions[0].digest().unwrap(),
        replay_ack.transaction_digest
    );
    if cut == Cut::Accepted {
        assert_eq!(complete.patches, prefix.patches);
        assert_eq!(complete.receipts, prefix.receipts);
        assert_eq!(complete.claims[0].root_id, prefix.claims[0].root_id);
    }
    let after_replay = family_counts(&f, &reservation);
    assert_eq!(
        after_replay.0, 0,
        "acknowledged quota reservation is released"
    );
    assert_eq!(after_replay.1, 0);
    assert_eq!(after_replay.2, 1);
    assert_eq!(after_replay.3, before.3 + 1);
    assert!(after_replay.4 > before.4);
    if cut == Cut::Accepted {
        assert_eq!(
            after_replay.4, before_replay.4,
            "replay did not double-store content"
        );
    }
    let reopened = reopen(&f, &binding);
    reopened.revalidate().unwrap();
    let mut one = [0u8; 1];
    assert!(matches!(
        reopened.read_resident_range(&base, 0, &mut one, &Cancellation::default()),
        Err(ProviderActiveRangeFailure::NeedsHydration { .. })
    ));
    assert_eq!(fs::read(&path).unwrap(), REPLACEMENT);
    outer_pin.verify_path_identity().unwrap();
}

#[test]
#[ignore = "UNQUALIFIED until ROOT reviews exact owned process cut and disposable fixture"]
fn killed_after_staging_replays_same_capture() {
    child::run(
        concat!(
            module_path!(),
            "::killed_after_staging_replays_same_capture"
        ),
        240,
        || {
            child::declare();
            recipe(Cut::Staging);
        },
    );
}

#[test]
#[ignore = "UNQUALIFIED until ROOT reviews exact owned process cut and disposable fixture"]
fn killed_after_accepted_before_ack_replays_receipt() {
    child::run(
        concat!(
            module_path!(),
            "::killed_after_accepted_before_ack_replays_receipt"
        ),
        240,
        || {
            child::declare();
            recipe(Cut::Accepted);
        },
    );
}

#[test]
#[ignore = "PARENT-ONLY exact owned capture cut/replay helper"]
fn capture_replay_child() {
    assert_eq!(
        std::env::var("LAYRS_PROVIDER_SQLITE_TEST_HOST").as_deref(),
        Ok("audited-layrs-test-host-v1")
    );
    let path = PathBuf::from(
        std::env::var_os("LAYRS_CAPTURE_CRASH_REQUEST").expect("parent-owned capture request"),
    );
    let (request, outer) = read_request(&path);
    let mode = std::env::var("LAYRS_CAPTURE_CRASH_MODE").unwrap();
    assert!(matches!(mode.as_str(), "cut" | "resume"));
    if mode == "cut" {
        assert_eq!(
            std::env::var_os("LAYRS_CAPTURE_CRASH_HOOK_REQUEST"),
            Some(path.clone().into_os_string())
        );
    } else {
        assert!(std::env::var_os("LAYRS_CAPTURE_CRASH_HOOK_REQUEST").is_none());
    }
    child::declare();
    let f = Fixture::from_owned(outer);
    let (_, _, binding) = request.records();
    let reopened = reopen(&f, &binding);
    reopened.revalidate().unwrap();
    let mut session = reopened
        .into_disconnected_session()
        .unwrap_or_else(|error| panic!("capture crash child session withheld: {}", error.cause()));
    session.connect().unwrap();
    let acknowledgement = session
        .capture(TRANSACTION, &Cancellation::default())
        .unwrap();
    session.disconnect().unwrap();
    assert_eq!(mode, "resume", "UNQUALIFIED: cut barrier was never reached");
    let (proof, source_catalog_id) = inspect_private(&f, &request);
    let exact = proof
        .transactions
        .iter()
        .find(|row| row.transaction_id == TRANSACTION)
        .unwrap();
    assert_eq!(
        acknowledgement.receipt_digest,
        proof.receipts[0].digest().unwrap()
    );
    let ack = Ack {
        nonce: request.nonce.clone(),
        pid: std::process::id(),
        cut: request.cut,
        source_catalog_id,
        transaction_digest: exact.digest().unwrap(),
    };
    support::create(
        &f.outer
            .join(format!("capture-resume-{}.json", request.nonce)),
        &canonical_json(&ack).unwrap(),
    );
}
