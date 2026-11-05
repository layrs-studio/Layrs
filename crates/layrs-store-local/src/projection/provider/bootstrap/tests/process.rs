//! Test-only exact-owned-child cuts. The existing audited wrapper owns the outer
//! recipe child; this module owns only its explicitly launched barrier helper.
//! ACK records completed source barriers, not power-loss durability.
use super::super::super::database_io::tests::support;
use super::*;
use fixture::{Fixture, failure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    os::windows::process::CommandExt,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
enum Cut {
    Receipt { kind: u8, after: bool, prefix: bool },
    Sql { kind: u8, after: bool },
    Owner,
    Catalog { busy: bool },
    Transport { mode: AckTransportMode },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum AckTransportMode {
    Pauses,
    CatalogExitRace,
    Malformed,
    Foreign,
    NonemptyReady,
    PartialFailure,
    NonzeroExit,
    NoReady,
    ReadyCollision,
    ExitedBarrier,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    nonce: String,
    root_identity: String,
    owner_identity: String,
    owner_digest: String,
    cut: Cut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileAck {
    identity: String,
    bytes: u64,
    digest: String,
}
impl FileAck {
    fn read(path: &Path) -> Self {
        let (identity, bytes, digest) = support::witness(path);
        Self {
            identity,
            bytes,
            digest,
        }
    }
    fn optional(path: &Path) -> Option<Self> {
        match fs::symlink_metadata(path) {
            Ok(_) => Some(Self::read(path)),
            Err(e) if e.raw_os_error() == Some(2) => None,
            Err(e) => panic!("fixed checkpoint unavailable: {e}"),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceiptAck {
    kind: u8,
    after: bool,
    prefix: bool,
    leaf: String,
    parent_identity: String,
    file: FileAck,
    expected_bytes: u64,
    expected_digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SqlAck {
    kind: u8,
    after: bool,
    autocommit: bool,
    view: String,
    database: FileAck,
    journal: Option<FileAck>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogAck {
    busy: bool,
    opens: usize,
    committed: bool,
    identity: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ack {
    nonce: String,
    pid: u32,
    checkpoint: String,
    root_identity: String,
    owner_identity: String,
    owner_digest: String,
    receipt: Option<ReceiptAck>,
    sql: Option<SqlAck>,
    catalog: Option<CatalogAck>,
}
fn checked_ack(
    bytes: &[u8],
    request: &Request,
    pid: u32,
) -> std::result::Result<Ack, &'static str> {
    if bytes.is_empty() || bytes.len() > 4096 {
        return Err("ACK bound");
    }
    let ack: Ack = serde_json::from_slice(bytes).map_err(|_| "ACK schema")?;
    if ack.nonce != request.nonce
        || ack.pid != pid
        || ack.root_identity != request.root_identity
        || ack.owner_identity != request.owner_identity
        || ack.owner_digest != request.owner_digest
    {
        return Err("ACK provenance");
    }
    match request.cut {
        Cut::Receipt {
            kind,
            after,
            prefix,
        } => {
            if kind > 2 {
                return Err("receipt kind");
            }
            let r = ack.receipt.as_ref().ok_or("receipt ACK absent")?;
            if ack.checkpoint != "receipt"
                || ack.sql.is_some()
                || ack.catalog.is_some()
                || (r.kind, r.after, r.prefix) != (kind, after, prefix)
            {
                return Err("receipt checkpoint");
            }
            let (label, _) = receipt(kind).names();
            let head = format!("pb-{label}-{}-", fixture::TX);
            let nonce = r
                .leaf
                .strip_prefix(&head)
                .and_then(|s| s.strip_suffix(".tmp"))
                .ok_or("TEMP leaf")?;
            let rebuilt = receipt_io::temporary_name(receipt(kind), fixture::TX, nonce)
                .map_err(|_| "TEMP leaf")?;
            if rebuilt != r.leaf
                || !r.leaf.is_ascii()
                || r.leaf.len() > 122
                || r.leaf.contains(['/', '\\'])
                || r.parent_identity.is_empty()
                || r.parent_identity.len() > 256
            {
                return Err("TEMP leaf");
            }
            if r.expected_bytes == 0
                || r.expected_bytes > AllocationRecord::MAX_BYTES as u64
                || r.file.bytes
                    != if prefix {
                        r.expected_bytes / 2
                    } else {
                        r.expected_bytes
                    }
                || r.file.digest != r.expected_digest
            {
                return Err("receipt content checkpoint");
            }
        }
        Cut::Sql { kind, after } => {
            if kind > 4 {
                return Err("SQL kind");
            }
            let s = ack.sql.as_ref().ok_or("SQL ACK absent")?;
            if ack.checkpoint != "sql"
                || ack.receipt.is_some()
                || ack.catalog.is_some()
                || (s.kind, s.after) != (kind, after)
                || s.autocommit != after
                || s.view != committed_view(kind)
                || (after && s.journal.is_some())
                || (!after && kind != 1 && s.journal.is_none())
            {
                return Err("SQL checkpoint");
            }
        }
        Cut::Transport { mode } => {
            if mode == AckTransportMode::CatalogExitRace {
                let c = ack.catalog.as_ref().ok_or("catalog ACK absent")?;
                if ack.checkpoint != "catalog"
                    || ack.receipt.is_some()
                    || ack.sql.is_some()
                    || c.busy
                    || !c.committed
                    || c.opens == 0
                {
                    return Err("catalog checkpoint");
                }
            } else if ack.checkpoint != "transport"
                || ack.receipt.is_some()
                || ack.sql.is_some()
                || ack.catalog.is_some()
            {
                return Err("transport checkpoint");
            }
        }
        Cut::Owner => {
            if ack.checkpoint != "owner"
                || ack.receipt.is_some()
                || ack.sql.is_some()
                || ack.catalog.is_some()
            {
                return Err("owner checkpoint");
            }
        }
        Cut::Catalog { busy } => {
            let c = ack.catalog.as_ref().ok_or("catalog ACK absent")?;
            if ack.checkpoint != "catalog"
                || ack.receipt.is_some()
                || ack.sql.is_some()
                || c.busy != busy
                || c.committed == busy
                || (busy && c.opens != 0)
                || (!busy && c.opens == 0)
            {
                return Err("catalog checkpoint");
            }
        }
    }
    let files = ack.receipt.iter().map(|r| &r.file).chain(
        ack.sql
            .iter()
            .flat_map(|s| std::iter::once(&s.database).chain(s.journal.iter())),
    );
    for file in files {
        if file.identity.is_empty()
            || file.identity.len() > 256
            || !file.identity.is_ascii()
            || file.bytes > 16 * 1024 * 1024
            || file.digest.len() != 64
            || !file
                .digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("file witness bound");
        }
    }
    Ok(ack)
}
fn base_ack(checkpoint: &str) -> Ack {
    let path = PathBuf::from(std::env::var_os("LAYRS_WRITER_BARRIER_REQUEST").unwrap());
    let request: Request = serde_json::from_slice(&read_bounded(&path, 4096)).unwrap();
    assert_eq!(
        std::env::var("LAYRS_WRITER_BARRIER_NONCE").unwrap(),
        request.nonce
    );
    Ack {
        nonce: request.nonce,
        pid: std::process::id(),
        checkpoint: checkpoint.into(),
        root_identity: request.root_identity,
        owner_identity: request.owner_identity,
        owner_digest: request.owner_digest,
        receipt: None,
        sql: None,
        catalog: None,
    }
}
fn publish_ack(path: &Path, ack: &Ack) {
    use std::io::Write;
    let mode = publication_mode();
    let mut value = ack.clone();
    if mode == Some(AckTransportMode::Foreign) {
        value.nonce.push_str("-foreign");
        value.pid = value.pid.checked_add(1).unwrap();
    }
    let bytes = if mode == Some(AckTransportMode::Malformed) {
        b"{".to_vec()
    } else {
        serde_json::to_vec(&value).unwrap()
    };
    assert!(!bytes.is_empty() && bytes.len() <= 4096);
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .expect("exclusive ACK creation");
        if mode == Some(AckTransportMode::Pauses) {
            empty_token(&control_path(path, "created")).unwrap();
            helper_wait_token(&control_path(path, "created-go"));
            let split = bytes.len() / 2;
            file.write_all(&bytes[..split]).unwrap();
            file.sync_all().unwrap();
            empty_token(&control_path(path, "partial")).unwrap();
            helper_wait_token(&control_path(path, "partial-go"));
            file.write_all(&bytes[split..]).unwrap();
        } else if mode == Some(AckTransportMode::PartialFailure) {
            file.write_all(&bytes[..bytes.len() / 2]).unwrap();
            file.sync_all().unwrap();
            panic!("synthetic publication failure after actual prefix; no ready marker");
        } else {
            file.write_all(&bytes).unwrap();
        }
        file.sync_all().unwrap();
    } // Payload handle closes BEFORE readiness becomes observable.
    if mode == Some(AckTransportMode::ReadyCollision) {
        empty_token(&control_path(path, "publish-pending")).unwrap();
        helper_wait_token(&control_path(path, "publish-go"));
    }
    if mode == Some(AckTransportMode::NonemptyReady) {
        // Deliberately invalid published transport evidence, not the normal writer.
        support::create(&control_path(path, "ready"), b"invalid");
    } else {
        let published = empty_token(&control_path(path, "ready"));
        if mode == Some(AckTransportMode::ReadyCollision) {
            let error =
                published.expect_err("controlled ready collision must refuse actual create_new");
            assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
            panic!("controlled ready collision rejected with AlreadyExists: {error:?}");
        }
        published.expect("exclusive completed ACK publication");
    }
}
fn wait_for_owned_parent() -> ! {
    let start = HELPER_STARTED.get().expect("exact owned helper start");
    while start.elapsed() < Duration::from_secs(90) {
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("UNQUALIFIED writer helper barrier exceeded 90s; exact child remains parent-owned")
}
pub(super) fn acknowledge_receipt_and_wait(
    path: &Path,
    kind: ProviderBootstrapReceipt,
    after: bool,
    prefix: bool,
    expected: Option<&[u8]>,
) -> ! {
    let temporary = hooks::receipt_path(kind);
    let parent = temporary.parent().unwrap();
    let actual = if after {
        parent.join(kind.names().1)
    } else {
        temporary.clone()
    };
    let file = FileAck::read(&actual);
    if !prefix {
        let encoded = read_bounded(&actual, AllocationRecord::MAX_BYTES as u64);
        let identity = match kind {
            ProviderBootstrapReceipt::Allocation => {
                AllocationRecord::decode(&encoded)
                    .unwrap()
                    .receipt_file_identity
            }
            ProviderBootstrapReceipt::Prepared => {
                PreparedRecord::decode(&encoded)
                    .unwrap()
                    .receipt_file_identity
            }
            ProviderBootstrapReceipt::Abandoned => {
                AbandonedRecord::decode(&encoded)
                    .unwrap()
                    .receipt_file_identity
            }
        };
        assert_eq!(identity, file.identity);
    }
    let (expected_bytes, expected_digest) = match expected {
        Some(bytes) => {
            assert!(prefix);
            (
                bytes.len() as u64,
                blake3::hash(&bytes[..bytes.len() / 2]).to_hex().to_string(),
            )
        }
        None => {
            assert!(!prefix);
            (file.bytes, file.digest.clone())
        }
    };
    let mut ack = base_ack("receipt");
    ack.receipt = Some(ReceiptAck {
        kind: match kind {
            ProviderBootstrapReceipt::Allocation => 0,
            ProviderBootstrapReceipt::Prepared => 1,
            ProviderBootstrapReceipt::Abandoned => 2,
        },
        after,
        prefix,
        leaf: temporary.file_name().unwrap().to_str().unwrap().into(),
        parent_identity: crate::secure_fs::PinnedDirectory::open(parent)
            .unwrap()
            .identity_token(),
        file,
        expected_bytes,
        expected_digest,
    });
    publish_ack(path, &ack);
    wait_for_owned_parent()
}
fn committed_view(kind: u8) -> &'static str {
    match kind {
        0 => "initializing",
        1 => "allocating",
        2 => "control_prepared",
        3 => "control_published",
        4 => "absent",
        _ => panic!("SQL purpose"),
    }
}
fn sql_view(c: &rusqlite::Connection, kind: u8, worktree: &WorktreeId) -> String {
    if kind == 0 || kind == 4 {
        let mut q = c
            .prepare("SELECT state FROM worktrees WHERE id=?1")
            .unwrap();
        let rows = q
            .query_map([worktree.as_str()], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert!(rows.len() <= 1);
        rows.into_iter().next().unwrap_or_else(|| "absent".into())
    } else {
        let version: i64 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 8);
        let count: u64 = c
            .query_row("SELECT count(*) FROM projection_provider_mounts", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
        c.query_row("SELECT phase FROM projection_provider_mounts", [], |r| {
            r.get(0)
        })
        .unwrap()
    }
}
fn journal_path(database: &Path) -> PathBuf {
    let mut p = database.as_os_str().to_owned();
    p.push("-journal");
    p.into()
}
fn acknowledge_sql_and_wait(
    path: &Path,
    kind: u8,
    after: bool,
    c: &rusqlite::Connection,
    database: &Path,
    worktree: &WorktreeId,
) -> ! {
    assert_eq!(c.is_autocommit(), after);
    let view = sql_view(c, kind, worktree);
    assert_eq!(view, committed_view(kind));
    let mut ack = base_ack("sql");
    ack.sql = Some(SqlAck {
        kind,
        after,
        autocommit: c.is_autocommit(),
        view,
        database: FileAck::read(database),
        journal: FileAck::optional(&journal_path(database)),
    });
    publish_ack(path, &ack);
    wait_for_owned_parent()
}

fn receipt(kind: u8) -> ProviderBootstrapReceipt {
    match kind {
        0 => ProviderBootstrapReceipt::Allocation,
        1 => ProviderBootstrapReceipt::Prepared,
        2 => ProviderBootstrapReceipt::Abandoned,
        _ => panic!("invalid owned recipe receipt"),
    }
}
fn purpose(kind: u8) -> ProviderBootstrapSqlWrite {
    match kind {
        0 => ProviderBootstrapSqlWrite::RegisterInitializing,
        1 => ProviderBootstrapSqlWrite::InitializePrivate,
        2 => ProviderBootstrapSqlWrite::PrepareControl,
        3 => ProviderBootstrapSqlWrite::PublishControl,
        4 => ProviderBootstrapSqlWrite::RemoveInitializing,
        _ => panic!("invalid owned recipe SQL operation"),
    }
}
fn read_bounded(path: &Path, max: u64) -> Vec<u8> {
    let f = crate::layout::open_read_no_follow(path).unwrap();
    assert!(f.metadata().unwrap().len() <= max);
    let mut bytes = Vec::new();
    f.take(max + 1).read_to_end(&mut bytes).unwrap();
    assert!(bytes.len() as u64 <= max);
    bytes
}
pub(super) fn acknowledge_and_wait(path: &Path) -> ! {
    publish_ack(path, &base_ack("owner"));
    wait_for_owned_parent()
}

struct Running {
    child: child::OwnedChild,
    ack: PathBuf,
    stdout: PathBuf,
    stderr: PathBuf,
    nonce: String,
    request: Request,
    received: Option<Ack>,
    started: Instant,
    parent: crate::secure_fs::PinnedDirectory,
}
impl Running {
    fn wait_ack(&mut self) {
        self.wait_ack_result(None)
            .expect("exact completed owned ACK");
    }
    fn wait_ack_result(
        &mut self,
        mut after_absence: Option<&mut dyn FnMut(&mut Running)>,
    ) -> std::result::Result<(), AckFailure> {
        loop {
            let mut issue = self.output_bound().err();
            if self.started.elapsed() >= Duration::from_secs(90) && issue.is_none() {
                issue = Some(AckIssue::Deadline);
            }
            if issue.is_none() && self.received.is_none() {
                match self.completed_ack() {
                    Ok(Some(record)) => self.received = Some(record),
                    Ok(None) => (),
                    Err(e) => issue = Some(e),
                }
                if self.received.is_none() && issue.is_none() {
                    if let Some(action) = after_absence.take() {
                        action(self);
                    }
                }
            }
            if self.started.elapsed() >= Duration::from_secs(90) && issue.is_none() {
                issue = Some(AckIssue::Deadline);
            }
            let status = match self.child.try_wait() {
                Ok(status) => status,
                Err(e) => {
                    return Err(AckFailure {
                        issue: Some(AckIssue::Io("query exact owned child", e)),
                        exit: None,
                    });
                }
            };
            if let Some(status) = status {
                // One final check closes absence -> publication -> exit. Invalid
                // published evidence is never retried, and nonzero always fails.
                if issue.is_none() && self.received.is_none() {
                    match self.completed_ack() {
                        Ok(Some(record)) => self.received = Some(record),
                        Ok(None) => issue = Some(AckIssue::MissingReady),
                        Err(e) => issue = Some(e),
                    }
                }
                if let Err(e) = self.output_bound() {
                    if issue.is_none() {
                        issue = Some(e);
                    }
                }
                if !status.success() || issue.is_some() {
                    return Err(AckFailure {
                        issue,
                        exit: Some(status),
                    });
                }
                if self.live_barrier() {
                    return Err(AckFailure {
                        issue: Some(AckIssue::ExitedBarrier),
                        exit: Some(status),
                    });
                }
                assert!(self.received.is_some());
                return Ok(());
            }
            if let Some(issue) = issue {
                return Err(AckFailure {
                    issue: Some(issue),
                    exit: None,
                });
            }
            if self.received.is_some() && self.live_barrier() {
                return Ok(());
            }
            if self.started.elapsed() >= Duration::from_secs(90) {
                return Err(AckFailure {
                    issue: Some(AckIssue::Deadline),
                    exit: None,
                });
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn live_barrier(&self) -> bool {
        matches!(
            self.request.cut,
            Cut::Receipt { .. }
                | Cut::Sql { .. }
                | Cut::Owner
                | Cut::Transport {
                    mode: AckTransportMode::ExitedBarrier
                }
        )
    }
    fn output_bound(&self) -> std::result::Result<(), AckIssue> {
        for path in [&self.stdout, &self.stderr] {
            let metadata =
                fs::metadata(path).map_err(|e| AckIssue::Io("helper output metadata", e))?;
            if metadata.len() > 65536 {
                return Err(AckIssue::OutputBound);
            }
        }
        Ok(())
    }
    fn completed_ack(&self) -> std::result::Result<Option<Ack>, AckIssue> {
        if !token_present(&control_path(&self.ack, "ready"), &self.parent)? {
            return Ok(None);
        }
        self.parent
            .verify_path_identity()
            .map_err(|e| AckIssue::Io("ACK parent", e))?;
        let file = crate::layout::open_read_no_follow(&self.ack)
            .map_err(|e| AckIssue::Io("completed ACK open", e))?;
        let meta = file
            .metadata()
            .map_err(|e| AckIssue::Io("ACK metadata", e))?;
        if !meta.is_file() || meta.len() == 0 || meta.len() > 4096 {
            return Err(AckIssue::Invalid("ACK bound"));
        }
        let mut bytes = Vec::new();
        file.take(4097)
            .read_to_end(&mut bytes)
            .map_err(|e| AckIssue::Io("ACK read", e))?;
        self.parent
            .verify_path_identity()
            .map_err(|e| AckIssue::Io("ACK parent", e))?;
        let record =
            checked_ack(&bytes, &self.request, self.child.id()).map_err(AckIssue::Invalid)?;
        assert_eq!(record.nonce, self.nonce);
        Ok(Some(record))
    }
    fn kill_reap(mut self) {
        assert!(self.child.try_wait().unwrap().is_none());
        self.child.kill().expect("kill only owned helper handle");
        let status = self.child.wait().expect("reap only owned helper");
        assert!(!status.success());
    }
}
fn launch(f: &Fixture, cut: Cut) -> Running {
    launch_checked(f, cut, None).expect("fresh exact ACK/ready paths")
}
fn launch_checked(
    f: &Fixture,
    cut: Cut,
    collision: Option<bool>,
) -> std::result::Result<Running, AckIssue> {
    let nonce = format!(
        "{:x}-{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let owner = support::witness(&f.outer.join("fixture-owner.txt"));
    let request = Request {
        nonce: nonce.clone(),
        root_identity: crate::secure_fs::PinnedDirectory::open(&f.outer)
            .unwrap()
            .identity_token(),
        owner_identity: owner.0,
        owner_digest: owner.2,
        cut,
    };
    let request_path = f.outer.join(format!("writer-request-{nonce}.json"));
    let ack = f.outer.join(format!("writer-ack-{nonce}.json"));
    let parent = crate::secure_fs::PinnedDirectory::open(&f.outer)
        .map_err(|e| AckIssue::Io("launch parent", e))?;
    assert_eq!(parent.identity_token(), request.root_identity);
    if let Some(ready) = collision {
        // Explicit negative setup; the SAME ordinary prelaunch refusal runs next.
        support::create(
            &if ready {
                control_path(&ack, "ready")
            } else {
                ack.clone()
            },
            b"retained prelaunch collision",
        );
    }
    publication_absent(&ack, &parent, "ACK")?;
    publication_absent(&control_path(&ack, "ready"), &parent, "ready")?;
    support::create(&request_path, &serde_json::to_vec(&request).unwrap());
    let stdout = f.outer.join(format!("writer-{nonce}.stdout"));
    let stderr = f.outer.join(format!("writer-{nonce}.stderr"));
    let output = |p: &Path| {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(p)
            .unwrap()
    };
    let target = concat!(module_path!(), "::writer_barrier_helper");
    let target = target.strip_prefix("layrs_store_local::").unwrap_or(target);
    let started = Instant::now();
    let child = child::OwnedChild::new(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                target,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("LAYRS_WRITER_BARRIER_REQUEST", &request_path)
            .env("LAYRS_WRITER_BARRIER_NONCE", &nonce)
            .env("LAYRS_WRITER_BARRIER_ACK", &ack)
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(output(&stdout))
            .stderr(output(&stderr))
            .spawn()
            .expect("owned hidden barrier helper"),
    );
    Ok(Running {
        child,
        ack,
        stdout,
        stderr,
        nonce,
        request,
        received: None,
        started,
        parent,
    })
}
#[test]
#[ignore = "PARENT-ONLY writer barrier helper: never ROOT-selected; requires exact owned request/nonce/native witnesses"]
fn writer_barrier_helper() {
    HELPER_STARTED
        .set(Instant::now())
        .expect("one owned helper invocation");
    assert_eq!(
        std::env::var("LAYRS_PROVIDER_SQLITE_TEST_HOST").as_deref(),
        Ok("audited-layrs-test-host-v1")
    );
    let path = PathBuf::from(
        std::env::var_os("LAYRS_WRITER_BARRIER_REQUEST").expect("parent-only request"),
    );
    assert!(path.is_absolute());
    let request: Request = serde_json::from_slice(&read_bounded(&path, 4096)).unwrap();
    assert!(
        request.nonce.len() <= 128
            && !request.nonce.is_empty()
            && request
                .nonce
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b'-')
    );
    assert_eq!(
        std::env::var("LAYRS_WRITER_BARRIER_NONCE").unwrap(),
        request.nonce
    );
    let outer = path.parent().unwrap();
    assert_eq!(
        path.file_name().unwrap(),
        std::ffi::OsStr::new(&format!("writer-request-{}.json", request.nonce))
    );
    let root_pin = crate::secure_fs::PinnedDirectory::open(outer).unwrap();
    assert_eq!(root_pin.identity_token(), request.root_identity);
    let owner = support::witness(&outer.join("fixture-owner.txt"));
    assert_eq!(owner.0, request.owner_identity);
    assert_eq!(owner.2, request.owner_digest);
    let ack = PathBuf::from(std::env::var_os("LAYRS_WRITER_BARRIER_ACK").unwrap());
    assert_eq!(
        ack,
        outer.join(format!("writer-ack-{}.json", request.nonce))
    );
    HELPER_MODE
        .set(match request.cut {
            Cut::Transport { mode } => Some(mode),
            _ => None,
        })
        .expect("one exact helper mode");
    if let Cut::Transport { mode } = request.cut {
        if mode == AckTransportMode::CatalogExitRace {
            database_io::tests::writer_support::install_open_counter();
            child::declare();
            helper_wait_token(&control_path(&ack, "exit-go"));
            catalog_helper(outer, &ack, false);
        } else {
            transport_helper(&ack, mode);
        }
        return;
    }
    if let Cut::Catalog { busy } = request.cut {
        database_io::tests::writer_support::install_open_counter();
        child::declare();
        catalog_helper(outer, &ack, busy);
        return;
    }
    child::declare();
    hooks::reset();
    let f = Fixture::from_owned(outer.to_owned());
    root_pin.verify_path_identity().unwrap();
    if matches!(request.cut, Cut::Owner) {
        let lock = crate::lock::StoreLock::acquire_existing(
            &f.metadata.join("layrs.lock"),
            crate::lock::SessionMode::Exclusive,
        )
        .unwrap();
        let _retained = lock;
        acknowledge_and_wait(&ack);
    }
    let abandon = matches!(
        request.cut,
        Cut::Receipt { kind: 2, .. } | Cut::Sql { kind: 4, .. }
    );
    if abandon {
        drop(f.published());
        hooks::reset();
    }
    match request.cut {
        Cut::Receipt {
            kind,
            after,
            prefix,
        } => {
            let kind = receipt(kind);
            if prefix {
                hooks::prefix(Some(kind), Some(ack.clone()));
            } else {
                hooks::observe(move |e| {
                    if e.step == ProviderBootstrapWriteStep::ReceiptRename(kind) && e.after == after
                    {
                        acknowledge_receipt_and_wait(&ack, kind, after, false, None);
                    }
                    Ok(())
                });
            }
        }
        Cut::Sql { kind, after } => {
            let purpose = purpose(kind);
            let worktree = f.worktree.clone();
            hooks::on_sql_commit(move |actual_after, actual_purpose, c, database| {
                if actual_purpose == purpose && actual_after == after {
                    acknowledge_sql_and_wait(&ack, kind, after, c, database, &worktree);
                }
                Ok(())
            });
        }
        Cut::Owner | Cut::Catalog { .. } | Cut::Transport { .. } => {
            unreachable!("separate helper branch")
        }
    }
    if abandon {
        let mut recovery = f.recovery();
        let result = recovery.abandon();
        panic!("UNQUALIFIED required abandon barrier not reached: {result:?}");
    } else {
        match f.begin() {
            Ok(_) => panic!("UNQUALIFIED writer completed without requested barrier"),
            Err(e) => panic!("UNQUALIFIED barrier writer failed: {e:?}"),
        }
    }
}
pub(super) fn owner_contention() {
    let f = Fixture::new();
    let outer = f.outer.clone();
    let holder = std::rc::Rc::new(std::cell::RefCell::new(None::<Running>));
    let owned = holder.clone();
    hooks::on_before_owner(move |_| {
        let fixture = Fixture::from_owned(outer.clone());
        let mut running = launch(&fixture, Cut::Owner);
        running.wait_ack();
        *owned.borrow_mut() = Some(running);
        Ok(())
    });
    let before = f.witness();
    let error = failure(f.begin());
    assert!(matches!(
        error.cause,
        Cause::Store(crate::StoreError::LockBusy(_))
    ));
    assert!(
        error
            .write_report
            .as_ref()
            .unwrap()
            .effects
            .iter()
            .all(|e| matches!(
                e.step,
                ProviderBootstrapWriteStep::ExclusiveCreate(
                    ProviderBootstrapCreate::Member
                        | ProviderBootstrapCreate::Metadata
                        | ProviderBootstrapCreate::OwnerLock
                )
            ))
    );
    assert_eq!(before, f.witness());
    assert!(!f.receipt(ProviderBootstrapReceipt::Allocation).exists());
    holder.borrow_mut().take().unwrap().kill_reap();
}
fn cut(cut: Cut) {
    let f = Fixture::new();
    database_io::tests::writer_support::install(&f.family.root().join("metadata/catalog.sqlite3"));
    child::declare();
    hooks::reset();
    let mut child = launch(&f, cut);
    child.wait_ack();
    let acknowledged = child.received.clone().unwrap();
    child.kill_reap();
    if let Some(r) = acknowledged.receipt.as_ref() {
        require_receipt_ack(&f, r);
    }
    if let Some(s) = acknowledged.sql.as_ref() {
        let db = if s.kind == 0 || s.kind == 4 {
            f.family.root().join("metadata/catalog.sqlite3")
        } else {
            f.metadata.join("projection.sqlite3")
        };
        assert_eq!(FileAck::read(&db), s.database);
        assert_eq!(FileAck::optional(&journal_path(&db)), s.journal);
    }
    let opens_before = database_io::tests::writer_support::events();
    let before = f.witness();
    let inspected =
        PartialStoreFamily::inspect_provider_bootstrap(f.family.root(), &f.worktree, fixture::TX);
    // Inspection is read-only even for a real hot/zero-origin interrupted DB.
    assert_eq!(before, f.witness());
    match cut {
        Cut::Receipt {
            kind: 0,
            after: false,
            ..
        } => {
            let e = failure(inspected);
            match e.cause {
                Cause::Validation(e) => assert!(
                    matches!(*e,ProjectionStoreError::Invalid(ref s)if s=="provider allocation receipt absent; no restart adoption")
                ),
                e => panic!("wrong absent A cause {e:?}"),
            }
            assert!(!f.receipt(ProviderBootstrapReceipt::Allocation).exists());
        }
        Cut::Receipt {
            kind,
            after,
            prefix,
        } => {
            assert_eq!(f.receipt(receipt(kind)).exists(), after && !prefix);
            let i = inspected.unwrap();
            assert!(i.write_report.is_none());
            assert!(i.effects.is_empty());
            if kind == 2 && after {
                let r = f.recovery();
                assert_eq!(
                    r.inspect().unwrap().state,
                    ProviderBootstrapState::AbandonIncomplete
                );
            }
        }
        Cut::Sql { kind, after } => {
            let i = inspected.unwrap();
            assert!(i.write_report.is_none());
            assert!(i.effects.is_empty());
            if kind == 0 {
                registration_private_fence(&f, &i)
                    .expect("registration cut must retain exact owned zero private DB");
            }
            let scope = purpose(kind).scope();
            let status = if scope == ProviderDatabaseScope::FamilyCatalog {
                i.family.status
            } else {
                i.private.status
            };
            if after {
                assert_eq!(status, ProviderDatabaseStatus::Validated);
                assert_committed(&f, kind);
            } else {
                assert_previous(&f, kind, &i, acknowledged.sql.as_ref().unwrap());
                if kind == 4 && status == ProviderDatabaseStatus::Validated {
                    let recovery = f.recovery();
                    assert_eq!(
                        recovery.inspect().unwrap().state,
                        ProviderBootstrapState::AbandonIncomplete
                    );
                }
            }
            no_following_effects(&f, kind);
            // Pre-COMMIT zero-origin/private or finite journal may be unavailable;
            // it is not called restored or authorized for initialization replay.
            if kind <= 1 {
                let a = stages(&f);
                assert_eq!(
                    fs::metadata(a.stage_identity().unwrap().stage().join("FORMAT"))
                        .unwrap()
                        .len(),
                    0
                );
            }
            if kind == 4 && after {
                let r = f.recovery();
                assert_eq!(
                    r.inspect().unwrap().state,
                    ProviderBootstrapState::Abandoned
                );
            }
        }
        Cut::Owner | Cut::Catalog { .. } | Cut::Transport { .. } => unreachable!(),
    }
    assert_eq!(before, f.witness());
    if let Some(r) = acknowledged.receipt.as_ref() {
        require_receipt_ack(&f, r);
    }
    for (operation, name, flags) in database_io::tests::writer_support::events()
        .into_iter()
        .skip(opens_before.len())
    {
        assert!(
            !(operation == "open"
                && flags & rusqlite::ffi::SQLITE_OPEN_MAIN_DB != 0
                && flags & rusqlite::ffi::SQLITE_OPEN_READWRITE != 0
                && !name.is_empty()),
            "inspection must not make a counted disk RW open: {name}"
        );
    }
}
fn stages(f: &Fixture) -> AllocationRecord {
    AllocationRecord::decode(&fs::read(f.receipt(ProviderBootstrapReceipt::Allocation)).unwrap())
        .unwrap()
}
fn assert_committed(f: &Fixture, kind: u8) {
    let a = stages(f);
    let family = FamilyAccess::open(f.family.root()).unwrap();
    let _locks = family.locks(&f.worktree, false).unwrap();
    if kind == 0 || kind == 4 {
        let state = database_io::read(&family.catalog, |c| family.validate_catalog(c, &a)).unwrap();
        assert!(
            matches!(state,database_io::ReadState::Valid(value)if value==if kind==4{Registration::Absent}else{Registration::Initializing})
        );
    } else {
        let path = f.metadata.join("projection.sqlite3");
        let database =
            database_io::DatabaseFile::open(path, &a.private.database_file_identity).unwrap();
        let prepared = if f.receipt(ProviderBootstrapReceipt::Prepared).exists() {
            Some(
                PreparedRecord::decode(
                    &fs::read(f.receipt(ProviderBootstrapReceipt::Prepared)).unwrap(),
                )
                .unwrap(),
            )
        } else {
            None
        };
        let state = database_io::read(&database, |c| validation::private(c, &a, prepared.as_ref()))
            .unwrap();
        let expected = match kind {
            1 => MountPhase::Allocating,
            2 => MountPhase::ControlPrepared,
            3 => MountPhase::ControlPublished,
            _ => unreachable!(),
        };
        assert!(
            matches!(state,database_io::ReadState::Valid(validation::PrivateValidation::Initialized(m))if m.phase==expected)
        );
    }
}
macro_rules! recipe {($name:ident,$cut:expr)=>{#[test]#[ignore="UNQUALIFIED until ROOT approves exact owned-host actual process-interruption recipe; retained fixtures"]fn $name(){child::run(concat!(module_path!(),"::",stringify!($name)),240,||{cut($cut);});}};}
recipe!(
    killed_allocation_prefix,
    Cut::Receipt {
        kind: 0,
        after: false,
        prefix: true
    }
);
recipe!(
    killed_allocation_before_rename,
    Cut::Receipt {
        kind: 0,
        after: false,
        prefix: false
    }
);
recipe!(
    killed_allocation_after_rename,
    Cut::Receipt {
        kind: 0,
        after: true,
        prefix: false
    }
);
recipe!(
    killed_prepared_prefix,
    Cut::Receipt {
        kind: 1,
        after: false,
        prefix: true
    }
);
recipe!(
    killed_prepared_before_rename,
    Cut::Receipt {
        kind: 1,
        after: false,
        prefix: false
    }
);
recipe!(
    killed_prepared_after_rename,
    Cut::Receipt {
        kind: 1,
        after: true,
        prefix: false
    }
);
recipe!(
    killed_terminal_prefix,
    Cut::Receipt {
        kind: 2,
        after: false,
        prefix: true
    }
);
recipe!(
    killed_terminal_before_rename,
    Cut::Receipt {
        kind: 2,
        after: false,
        prefix: false
    }
);
recipe!(
    killed_terminal_after_rename,
    Cut::Receipt {
        kind: 2,
        after: true,
        prefix: false
    }
);
recipe!(
    killed_registry_before_commit,
    Cut::Sql {
        kind: 0,
        after: false
    }
);
recipe!(
    killed_registry_after_commit,
    Cut::Sql {
        kind: 0,
        after: true
    }
);
recipe!(
    killed_initialization_before_commit,
    Cut::Sql {
        kind: 1,
        after: false
    }
);
recipe!(
    killed_initialization_after_commit,
    Cut::Sql {
        kind: 1,
        after: true
    }
);
recipe!(
    killed_prepared_phase_before_commit,
    Cut::Sql {
        kind: 2,
        after: false
    }
);
recipe!(
    killed_prepared_phase_after_commit,
    Cut::Sql {
        kind: 2,
        after: true
    }
);
recipe!(
    killed_published_phase_before_commit,
    Cut::Sql {
        kind: 3,
        after: false
    }
);
recipe!(
    killed_published_phase_after_commit,
    Cut::Sql {
        kind: 3,
        after: true
    }
);
recipe!(
    killed_registry_removal_before_commit,
    Cut::Sql {
        kind: 4,
        after: false
    }
);
recipe!(
    killed_registry_removal_after_commit,
    Cut::Sql {
        kind: 4,
        after: true
    }
);

fn catalog_helper(outer: &Path, ack_path: &Path, expect_busy: bool) {
    use database_io::tests::writer_support as observer;
    assert_eq!(
        observer::open_callbacks(),
        0,
        "preparation has opened no SQLite database"
    );
    let family = FamilyAccess::open(&outer.join("f")).unwrap();
    let _lifecycle = family.writer_lifecycle(false).unwrap();
    let catalog = family.writer_catalog();
    let mut ack = base_ack("catalog");
    let identity = FileAck::read(family.catalog.path()).identity;
    match catalog {
        Err(Cause::Store(crate::StoreError::LockBusy(_))) => {
            assert!(expect_busy);
            assert_eq!(observer::open_callbacks(), 0);
            ack.catalog = Some(CatalogAck {
                busy: true,
                opens: 0,
                committed: false,
                identity,
            });
        }
        Err(cause) => panic!("unexpected catalog-gate cause: {cause:?}"),
        Ok(_catalog) => {
            assert!(!expect_busy, "continuous catalog gate was released");
            let worktree =
                super::super::super::validation::tests::fixture::Fixture::new("G1", false)
                    .allocation
                    .format
                    .identity
                    .worktree_id;
            let metadata = family.member(&worktree);
            let a = AllocationRecord::decode(&read_bounded(
                &metadata.join("provider-control/allocation.json"),
                AllocationRecord::MAX_BYTES as u64,
            ))
            .unwrap();
            let admission = database_io::clean_admission(&family.catalog, false, |c| {
                family.validate_catalog(c, &a).map(|_| ())
            })
            .unwrap();
            // This is an independently owned conforming test writer, not a
            // provider bootstrap endpoint. No ordinary recovering accessor.
            let bare = database_io::BareConnection::open(family.catalog.path(), true).unwrap();
            admission.require_geometry(&bare).unwrap();
            let result = (|| -> CauseResult<()> {
                let c = bare.connection();
                c.execute_batch(
                    "BEGIN IMMEDIATE; UPDATE quota SET bytes=bytes WHERE singleton=1; COMMIT;",
                )?;
                family.validate_catalog(c, &a)?;
                Ok(())
            })();
            bare.finish(result).unwrap();
            family.catalog.verify().unwrap();
            assert_eq!(identity, FileAck::read(family.catalog.path()).identity);
            assert!(observer::open_callbacks() > 0);
            ack.catalog = Some(CatalogAck {
                busy: false,
                opens: observer::open_callbacks(),
                committed: true,
                identity,
            });
        }
    }
    publish_ack(ack_path, &ack);
}
pub(super) fn competing_catalog(f: &Fixture, busy: bool) {
    let before = FileAck::read(&f.family.root().join("metadata/catalog.sqlite3"));
    let mut child = launch(f, Cut::Catalog { busy });
    child.wait_ack();
    let ack = child.received.as_ref().unwrap().catalog.as_ref().unwrap();
    assert_eq!(ack.identity, before.identity);
    assert_eq!(ack.busy, busy);
    let start = Instant::now();
    loop {
        if let Some(status) = child.child.try_wait().unwrap() {
            assert!(status.success());
            assert!(
                child
                    .child
                    .wait()
                    .expect("reap exact completed catalog helper")
                    .success()
            );
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "UNQUALIFIED catalog child did not finish"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    if busy {
        assert_eq!(
            before,
            FileAck::read(&f.family.root().join("metadata/catalog.sqlite3"))
        );
    }
}
fn require_receipt_ack(f: &Fixture, r: &ReceiptAck) {
    let parent = f.metadata.join("provider-control");
    assert_eq!(
        crate::secure_fs::PinnedDirectory::open(&parent)
            .unwrap()
            .identity_token(),
        r.parent_identity
    );
    let temporary = parent.join(&r.leaf);
    if r.after {
        assert!(matches!(fs::symlink_metadata(&temporary),Err(e) if e.raw_os_error()==Some(2)));
        assert_eq!(FileAck::read(&f.receipt(receipt(r.kind))), r.file);
    } else {
        assert_eq!(FileAck::read(&temporary), r.file);
        assert!(!f.receipt(receipt(r.kind)).try_exists().unwrap());
    }
    assert_eq!(r.file.digest, r.expected_digest);
}
fn require_hot(i: &ProviderBootstrapInspection, kind: u8, ack: &SqlAck) {
    let scope = purpose(kind).scope();
    let db = if scope == ProviderDatabaseScope::FamilyCatalog {
        &i.family
    } else {
        &i.private
    };
    assert_eq!(db.database, scope);
    assert_eq!(db.status, ProviderDatabaseStatus::RollbackRequired);
    let trigger = match db.cause.as_ref().expect("typed hot cause") {
        Cause::ReadonlyRollback(e) => e,
        e => panic!("wrong hot cause {e:?}"),
    };
    assert!(
        matches!(trigger.as_ref(),rusqlite::Error::SqliteFailure(e,_) if e.extended_code==rusqlite::ffi::SQLITE_READONLY_ROLLBACK)
    );
    let original = i
        .rollback_triggers
        .iter()
        .find(|t| t.database == scope)
        .expect("scoped original");
    assert!(std::ptr::eq(original.error(), trigger.as_ref()));
    assert!(ack.journal.is_some());
    assert!(i.phase.is_none());
    assert_eq!(
        i.state,
        if kind == 4 {
            ProviderBootstrapState::TerminalPendingValidation
        } else {
            ProviderBootstrapState::SqlValidationPending
        }
    );
}
fn assert_previous(f: &Fixture, kind: u8, i: &ProviderBootstrapInspection, ack: &SqlAck) {
    let scope = purpose(kind).scope();
    let status = if scope == ProviderDatabaseScope::FamilyCatalog {
        i.family.status
    } else {
        i.private.status
    };
    if status == ProviderDatabaseStatus::RollbackRequired {
        require_hot(i, kind, ack);
        return;
    }
    if kind == 1 && status == ProviderDatabaseStatus::Unavailable {
        let cause = i.private.cause.as_ref().expect("zero-origin typed cause");
        assert!(
            matches!(cause,Cause::Validation(e) if matches!(e.as_ref(),ProjectionStoreError::Invalid(s) if s=="zero database has a sidecar; SQLite open forbidden"))
        );
        assert_eq!(ack.database.bytes, 0);
        assert!(ack.journal.is_some());
        assert!(i.phase.is_none());
        assert_eq!(i.state, ProviderBootstrapState::SqlValidationPending);
        return;
    }
    let a = stages(f);
    let family = FamilyAccess::open(f.family.root()).unwrap();
    let _locks = family.locks(&f.worktree, false).unwrap();
    if kind == 0 || kind == 4 {
        assert_eq!(status, ProviderDatabaseStatus::Validated);
        let state = database_io::read(&family.catalog, |c| family.validate_catalog(c, &a)).unwrap();
        let expected = if kind == 0 {
            Registration::Absent
        } else {
            Registration::Initializing
        };
        assert!(
            matches!(state,database_io::ReadState::Valid(v) if v==expected),
            "precommit must retain prior registry row/absence"
        );
    } else {
        assert_eq!(
            status,
            if kind == 1 {
                ProviderDatabaseStatus::Uninitialized
            } else {
                ProviderDatabaseStatus::Validated
            }
        );
        let db = database_io::DatabaseFile::open(
            f.metadata.join("projection.sqlite3"),
            &a.private.database_file_identity,
        )
        .unwrap();
        let p = if kind >= 2 {
            Some(
                PreparedRecord::decode(&read_bounded(
                    &f.receipt(ProviderBootstrapReceipt::Prepared),
                    PreparedRecord::MAX_BYTES as u64,
                ))
                .unwrap(),
            )
        } else {
            None
        };
        let state = database_io::read(&db, |c| validation::private(c, &a, p.as_ref())).unwrap();
        match kind {
            1 => assert!(
                matches!(
                    state,
                    database_io::ReadState::Uninitialized
                        | database_io::ReadState::Valid(
                            validation::PrivateValidation::Uninitialized
                        )
                ),
                "precommit initialization must not be accepted schema8"
            ),
            2 | 3 => {
                let expected = if kind == 2 {
                    MountPhase::Allocating
                } else {
                    MountPhase::ControlPrepared
                };
                assert!(
                    matches!(state,database_io::ReadState::Valid(validation::PrivateValidation::Initialized(m)) if m.phase==expected),
                    "precommit must reject committed phase"
                );
            }
            _ => unreachable!(),
        }
    }
}
fn no_following_effects(f: &Fixture, kind: u8) {
    let a = stages(f);
    if kind <= 1 {
        assert_eq!(
            fs::metadata(a.stage_identity().unwrap().stage().join("FORMAT"))
                .unwrap()
                .len(),
            0
        );
        assert!(
            !f.receipt(ProviderBootstrapReceipt::Prepared)
                .try_exists()
                .unwrap()
        );
        assert!(!f.checkout.join(".layrs").try_exists().unwrap());
    }
    if kind == 2 {
        assert!(
            f.receipt(ProviderBootstrapReceipt::Prepared)
                .try_exists()
                .unwrap()
        );
        assert!(!f.checkout.join(".layrs").try_exists().unwrap());
    }
    if kind == 3 {
        assert!(f.checkout.join(".layrs/FORMAT").try_exists().unwrap());
    }
    if kind < 4 {
        assert!(
            !f.receipt(ProviderBootstrapReceipt::Abandoned)
                .try_exists()
                .unwrap()
        );
    }
    if kind == 4 {
        assert!(
            f.receipt(ProviderBootstrapReceipt::Abandoned)
                .try_exists()
                .unwrap()
        );
        assert!(f.metadata.join("projection.sqlite3").try_exists().unwrap());
        assert!(f.checkout.join(".layrs/FORMAT").try_exists().unwrap());
    }
}

pub(super) fn pure_ack_cases() {
    let request = Request {
        nonce: "1234-abcd".into(),
        root_identity: "fs:v1:00000001:0000000000000002".into(),
        owner_identity: "1:0:3".into(),
        owner_digest: "0".repeat(64),
        cut: Cut::Receipt {
            kind: 0,
            after: false,
            prefix: false,
        },
    };
    let ack = Ack {
        nonce: request.nonce.clone(),
        pid: 42,
        checkpoint: "receipt".into(),
        root_identity: request.root_identity.clone(),
        owner_identity: request.owner_identity.clone(),
        owner_digest: request.owner_digest.clone(),
        receipt: Some(ReceiptAck {
            kind: 0,
            after: false,
            prefix: false,
            leaf: receipt_io::temporary_name(
                ProviderBootstrapReceipt::Allocation,
                fixture::TX,
                "1-2-3",
            )
            .unwrap(),
            parent_identity: "fs:v1:00000001:0000000000000004".into(),
            file: FileAck {
                identity: "1:0:5".into(),
                bytes: 12,
                digest: "a".repeat(64),
            },
            expected_bytes: 12,
            expected_digest: "a".repeat(64),
        }),
        sql: None,
        catalog: None,
    };
    assert!(checked_ack(&serde_json::to_vec(&ack).unwrap(), &request, 42).is_ok());
    for field in ["nonce", "root_identity", "owner_identity", "owner_digest"] {
        let mut bad = ack.clone();
        match field {
            "nonce" => bad.nonce.push('1'),
            "root_identity" => bad.root_identity.push('1'),
            "owner_identity" => bad.owner_identity.push('1'),
            _ => bad.owner_digest.push('1'),
        };
        assert_eq!(
            checked_ack(&serde_json::to_vec(&bad).unwrap(), &request, 42).unwrap_err(),
            "ACK provenance"
        );
    }
    assert_eq!(
        checked_ack(&serde_json::to_vec(&ack).unwrap(), &request, 43).unwrap_err(),
        "ACK provenance"
    );
    for leaf in [
        "../foreign",
        r"..\foreign",
        "C:\\foreign",
        "pb-allocation-bad.tmp",
    ] {
        let mut bad = ack.clone();
        bad.receipt.as_mut().unwrap().leaf = leaf.into();
        assert_eq!(
            checked_ack(&serde_json::to_vec(&bad).unwrap(), &request, 42).unwrap_err(),
            "TEMP leaf"
        );
    }
    let mut bad = ack.clone();
    bad.receipt.as_mut().unwrap().kind = 1;
    assert_eq!(
        checked_ack(&serde_json::to_vec(&bad).unwrap(), &request, 42).unwrap_err(),
        "receipt checkpoint"
    );
    let mut extra = serde_json::to_value(&ack).unwrap();
    extra["foreign"] = "ignored?".into();
    assert_eq!(
        checked_ack(&serde_json::to_vec(&extra).unwrap(), &request, 42).unwrap_err(),
        "ACK schema"
    );
    assert_eq!(
        checked_ack(&vec![b' '; 4097], &request, 42).unwrap_err(),
        "ACK bound"
    );
    for kind in 0..5 {
        for after in [false, true] {
            let mut req = request.clone();
            req.cut = Cut::Sql { kind, after };
            let mut good = ack.clone();
            good.checkpoint = "sql".into();
            good.receipt = None;
            let file = FileAck {
                identity: "1:0:6".into(),
                bytes: 4096,
                digest: "b".repeat(64),
            };
            good.sql = Some(SqlAck {
                kind,
                after,
                autocommit: after,
                view: committed_view(kind).into(),
                database: file.clone(),
                journal: if after { None } else { Some(file) },
            });
            assert!(checked_ack(&serde_json::to_vec(&good).unwrap(), &req, 42).is_ok());
            let mut wrong = good.clone();
            wrong.sql.as_mut().unwrap().autocommit = !after;
            assert_eq!(
                checked_ack(&serde_json::to_vec(&wrong).unwrap(), &req, 42).unwrap_err(),
                "SQL checkpoint"
            );
            let mut wrong = good.clone();
            wrong.sql.as_mut().unwrap().view = "invented phase".into();
            assert_eq!(
                checked_ack(&serde_json::to_vec(&wrong).unwrap(), &req, 42).unwrap_err(),
                "SQL checkpoint"
            );
        }
    }
}

// All handshakes are empty exclusive tokens: creation itself exposes complete
// contents. No write-then-observe race is relocated into the control protocol.
static HELPER_STARTED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
static HELPER_MODE: std::sync::OnceLock<Option<AckTransportMode>> = std::sync::OnceLock::new();
fn publication_mode() -> Option<AckTransportMode> {
    HELPER_MODE.get().copied().flatten()
}
#[derive(Debug)]
enum AckIssue {
    Io(&'static str, std::io::Error),
    Invalid(&'static str),
    Collision { label: &'static str, path: PathBuf },
    MissingReady,
    ExitedBarrier,
    OutputBound,
    Deadline,
}
#[derive(Debug)]
struct AckFailure {
    issue: Option<AckIssue>,
    exit: Option<std::process::ExitStatus>,
}
fn control_path(ack: &Path, label: &str) -> PathBuf {
    assert!(matches!(
        label,
        "ready"
            | "created"
            | "created-go"
            | "partial"
            | "partial-go"
            | "exit-go"
            | "publish-pending"
            | "publish-go"
    ));
    let name = ack.file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with("writer-ack-") && name.ends_with(".json"));
    ack.with_extension(label)
}
fn empty_token(path: &Path) -> std::io::Result<()> {
    // No body to tear. Only callers with an already validated owned parent use it.
    drop(
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?,
    );
    Ok(())
}
fn token_present(
    path: &Path,
    parent: &crate::secure_fs::PinnedDirectory,
) -> std::result::Result<bool, AckIssue> {
    parent
        .verify_path_identity()
        .map_err(|e| AckIssue::Io("token parent", e))?;
    let result = match crate::layout::open_read_no_follow(path) {
        Ok(file) => {
            let m = file
                .metadata()
                .map_err(|e| AckIssue::Io("token metadata", e))?;
            if !m.is_file() || m.len() != 0 {
                return Err(AckIssue::Invalid("ready/control token shape"));
            }
            true
        }
        Err(e) if e.raw_os_error() == Some(2) => false,
        Err(e) => return Err(AckIssue::Io("token open", e)),
    };
    parent
        .verify_path_identity()
        .map_err(|e| AckIssue::Io("token parent", e))?;
    Ok(result)
}
fn publication_absent(
    path: &Path,
    parent: &crate::secure_fs::PinnedDirectory,
    label: &'static str,
) -> std::result::Result<(), AckIssue> {
    parent
        .verify_path_identity()
        .map_err(|e| AckIssue::Io("prelaunch parent", e))?;
    let absent = match crate::layout::open_read_no_follow(path) {
        Ok(_) => false,
        Err(e) if e.raw_os_error() == Some(2) => true,
        Err(e) => return Err(AckIssue::Io("prelaunch publication", e)),
    };
    parent
        .verify_path_identity()
        .map_err(|e| AckIssue::Io("prelaunch parent", e))?;
    if absent {
        Ok(())
    } else {
        Err(AckIssue::Collision {
            label,
            path: path.to_owned(),
        })
    }
}
fn helper_wait_token(path: &Path) {
    let parent = crate::secure_fs::PinnedDirectory::open(path.parent().unwrap()).unwrap();
    let start = HELPER_STARTED.get().expect("owned helper start");
    loop {
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "UNQUALIFIED helper control deadline"
        );
        if token_present(path, &parent).expect("complete control token") {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn transport_helper(ack: &Path, mode: AckTransportMode) {
    if mode == AckTransportMode::NoReady {
        return;
    }
    publish_ack(ack, &base_ack("transport"));
    if mode == AckTransportMode::NonzeroExit {
        panic!("synthetic nonzero helper after valid completed ACK");
    }
}
fn wait_control(r: &mut Running, label: &str) {
    loop {
        r.output_bound().expect("bounded transport output");
        assert!(
            r.started.elapsed() < Duration::from_secs(90),
            "UNQUALIFIED control wait deadline"
        );
        if token_present(&control_path(&r.ack, label), &r.parent).expect("complete control marker")
        {
            return;
        }
        assert!(
            r.child.try_wait().unwrap().is_none(),
            "helper exited before control checkpoint"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn wait_exit(r: &mut Running) -> std::process::ExitStatus {
    loop {
        r.output_bound().expect("bounded final output");
        assert!(
            r.started.elapsed() < Duration::from_secs(90),
            "UNQUALIFIED exact child exit deadline"
        );
        if let Some(status) = r.child.try_wait().unwrap() {
            let reaped = r.child.wait().expect("reap exact owned helper");
            assert_eq!(status, reaped);
            return status;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "UNQUALIFIED until ROOT authorizes exact owned-host ACK transport controls and fresh disposable parent"]
fn ack_ready_transport_ignores_created_and_partial_ack_until_ready() {
    child::run(
        concat!(
            module_path!(),
            "::ack_ready_transport_ignores_created_and_partial_ack_until_ready"
        ),
        180,
        || {
            let f = Fixture::new();
            child::declare();
            let mut r = launch(
                &f,
                Cut::Transport {
                    mode: AckTransportMode::Pauses,
                },
            );
            wait_control(&mut r, "created");
            assert_eq!(fs::metadata(&r.ack).unwrap().len(), 0);
            assert!(r.completed_ack().unwrap().is_none());
            empty_token(&control_path(&r.ack, "created-go")).unwrap();
            wait_control(&mut r, "partial");
            let partial = read_bounded(&r.ack, 4096);
            assert!(!partial.is_empty());
            assert!(matches!(
                checked_ack(&partial, &r.request, r.child.id()),
                Err("ACK schema")
            ));
            assert!(r.completed_ack().unwrap().is_none());
            empty_token(&control_path(&r.ack, "partial-go")).unwrap();
            r.wait_ack_result(None).unwrap();
            assert!(r.received.is_some());
            assert!(wait_exit(&mut r).success());
            assert!(token_present(&control_path(&r.ack, "ready"), &r.parent).unwrap());
        },
    );
}
#[test]
#[ignore = "UNQUALIFIED until ROOT authorizes exact owned-host ACK exit reconciliation and fresh disposable parent"]
fn ack_ready_transport_reconciles_publication_before_catalog_exit() {
    child::run(
        concat!(
            module_path!(),
            "::ack_ready_transport_reconciles_publication_before_catalog_exit"
        ),
        180,
        || {
            let f = Fixture::new();
            child::declare();
            hooks::reset();
            drop(f.published());
            let mut r = launch(
                &f,
                Cut::Transport {
                    mode: AckTransportMode::CatalogExitRace,
                },
            );
            let mut reached = false;
            let mut interleave = |r: &mut Running| {
                assert!(!reached);
                reached = true;
                assert!(r.received.is_none());
                empty_token(&control_path(&r.ack, "exit-go")).unwrap();
                assert!(wait_exit(r).success());
            };
            r.wait_ack_result(Some(&mut interleave)).unwrap();
            assert!(reached);
            let c = r.received.as_ref().unwrap().catalog.as_ref().unwrap();
            assert!(!c.busy && c.committed && c.opens > 0);
        },
    );
}
#[test]
#[ignore = "UNQUALIFIED until ROOT authorizes exact owned-host negative ACK protocol controls; retained failures"]
fn ack_ready_transport_refuses_invalid_or_failed_publication() {
    child::run(
        concat!(
            module_path!(),
            "::ack_ready_transport_refuses_invalid_or_failed_publication"
        ),
        180,
        || {
            child::declare();
            for ready in [false, true] {
                let f = Fixture::new();
                let before = f.witness();
                let issue = match launch_checked(
                    &f,
                    Cut::Transport {
                        mode: AckTransportMode::Pauses,
                    },
                    Some(ready),
                ) {
                    Err(e) => e,
                    Ok(_) => panic!("collision bypassed ordinary prelaunch refusal"),
                };
                let AckIssue::Collision { label, path } = issue else {
                    panic!("exact prelaunch collision required")
                };
                assert_eq!(label, if ready { "ready" } else { "ACK" });
                assert_eq!(path.parent(), Some(f.outer.as_path()));
                assert_eq!(read_bounded(&path, 4096), b"retained prelaunch collision");
                assert_eq!(before, f.witness());
            }
            for mode in [
                AckTransportMode::Malformed,
                AckTransportMode::Foreign,
                AckTransportMode::NonemptyReady,
                AckTransportMode::PartialFailure,
                AckTransportMode::NonzeroExit,
                AckTransportMode::NoReady,
                AckTransportMode::ReadyCollision,
                AckTransportMode::ExitedBarrier,
            ] {
                let f = Fixture::new();
                let mut r = launch(&f, Cut::Transport { mode });
                let ready_witness = if mode == AckTransportMode::ReadyCollision {
                    wait_control(&mut r, "publish-pending");
                    // Ordinary launch proved absence; this is explicit later interference.
                    empty_token(&control_path(&r.ack, "ready")).unwrap();
                    let witness = FileAck::read(&control_path(&r.ack, "ready"));
                    empty_token(&control_path(&r.ack, "publish-go")).unwrap();
                    Some(witness)
                } else {
                    None
                };
                let status = wait_exit(&mut r);
                let stderr = String::from_utf8(read_bounded(&r.stderr, 65536))
                    .expect("owned libtest diagnostics UTF8");
                match mode {
                    AckTransportMode::PartialFailure => assert!(stderr.contains(
                        "synthetic publication failure after actual prefix; no ready marker"
                    )),
                    AckTransportMode::NonzeroExit => assert!(
                        stderr.contains("synthetic nonzero helper after valid completed ACK")
                    ),
                    AckTransportMode::ReadyCollision => assert!(
                        stderr.contains("controlled ready collision rejected with AlreadyExists:")
                    ),
                    _ => (),
                }
                let failure = r
                    .wait_ack_result(None)
                    .expect_err("published invalid/failed evidence is terminal");
                assert_eq!(failure.exit, Some(status));
                match mode {
                    AckTransportMode::Malformed => {
                        assert!(status.success());
                        assert!(matches!(
                            failure.issue,
                            Some(AckIssue::Invalid("ACK schema"))
                        ));
                    }
                    AckTransportMode::Foreign => {
                        assert!(status.success());
                        assert!(matches!(
                            failure.issue,
                            Some(AckIssue::Invalid("ACK provenance"))
                        ));
                    }
                    AckTransportMode::NonemptyReady => {
                        assert!(status.success());
                        assert!(matches!(
                            failure.issue,
                            Some(AckIssue::Invalid("ready/control token shape"))
                        ));
                    }
                    AckTransportMode::PartialFailure => {
                        assert!(!status.success());
                        assert!(matches!(failure.issue, Some(AckIssue::MissingReady)));
                        assert!(!read_bounded(&r.ack, 4096).is_empty());
                        assert!(!token_present(&control_path(&r.ack, "ready"), &r.parent).unwrap());
                    }
                    AckTransportMode::ExitedBarrier => {
                        assert!(status.success());
                        assert!(matches!(failure.issue, Some(AckIssue::ExitedBarrier)));
                        assert!(r.received.is_some());
                    }
                    AckTransportMode::NoReady => {
                        assert!(status.success());
                        assert!(matches!(failure.issue, Some(AckIssue::MissingReady)));
                        assert!(
                            matches!(fs::symlink_metadata(&r.ack),Err(e)if e.raw_os_error()==Some(2))
                        );
                    }
                    AckTransportMode::NonzeroExit | AckTransportMode::ReadyCollision => {
                        assert!(!status.success());
                        assert!(failure.issue.is_none());
                        assert!(r.received.is_some());
                        if let Some(old) = ready_witness {
                            assert_eq!(old, FileAck::read(&control_path(&r.ack, "ready")));
                        }
                    }
                    _ => panic!("unselected negative mode"),
                }
            }
        },
    );
}

#[derive(Debug)]
pub(super) enum RegistrationPrivateFence {
    Cause(Cause),
    Path,
    Receipt,
    MetadataIdentity,
    Nonzero(u64),
    Status(ProviderDatabaseStatus),
    UnexpectedAdmission,
}
pub(super) fn registration_private_fence(
    f: &Fixture,
    i: &ProviderBootstrapInspection,
) -> std::result::Result<(), RegistrationPrivateFence> {
    use RegistrationPrivateFence as F;
    let a = stages(f);
    let family = FamilyAccess::open(f.family.root()).map_err(F::Cause)?;
    let _locks = family.locks(&f.worktree, false).map_err(F::Cause)?;
    family.require_allocation(&a).map_err(F::Cause)?;
    if a.data_reference().map_err(|e| F::Cause(e.into()))? != i.binding.allocation
        || FileAck::read(&f.receipt(ProviderBootstrapReceipt::Allocation)).identity
            != a.receipt_file_identity
    {
        return Err(F::Receipt);
    }
    let metadata = family.member(&f.worktree);
    if metadata.as_os_str() != f.metadata.as_os_str()
        || metadata.as_os_str() != Path::new(&a.private.metadata_path).as_os_str()
        || crate::projection::shared::provider_access::canonical_path(&metadata)
            .map_err(F::Cause)?
            != a.private.canonical_metadata_path
    {
        return Err(F::Path);
    }
    let parent =
        crate::secure_fs::PinnedDirectory::open(&metadata).map_err(|e| F::Cause(e.into()))?;
    if parent.identity_token() != a.private.metadata_identity {
        return Err(F::MetadataIdentity);
    }
    let path = metadata.join("projection.sqlite3");
    let database = database_io::DatabaseFile::open(path.clone(), &a.private.database_file_identity)
        .map_err(F::Cause)?;
    database.verify().map_err(F::Cause)?;
    let file = crate::layout::open_read_no_follow(&path).map_err(|e| F::Cause(e.into()))?;
    let identity = crate::projection::file::identity(&file).map_err(|e| F::Cause(e.into()))?;
    if identity != a.private.database_file_identity {
        return Err(F::MetadataIdentity);
    }
    let length = file.metadata().map_err(|e| F::Cause(e.into()))?.len();
    // Reject nonzero/initialized evidence BEFORE any reader could open SQLite.
    if length != 0 {
        return Err(F::Nonzero(length));
    }
    let mut byte = [0u8; 1];
    let count = Read::read(&mut &file, &mut byte).map_err(|e| F::Cause(e.into()))?;
    if count != 0 {
        return Err(F::Nonzero(count as u64));
    }
    drop(file);
    parent
        .verify_path_identity()
        .map_err(|e| F::Cause(e.into()))?;
    database.verify().map_err(F::Cause)?;
    let read = database_io::read::<()>(&database, |_| {
        panic!("zero private DB must never enter SQLite validation")
    })
    .map_err(F::Cause)?;
    match read {
        database_io::ReadState::Uninitialized => (),
        database_io::ReadState::Unavailable(cause)
        | database_io::ReadState::Rejected { cause, .. }
        | database_io::ReadState::ReadButUnavailable(cause) => return Err(F::Cause(cause)),
        _ => return Err(F::UnexpectedAdmission),
    }
    // The unchanged zero path probes all three exact no-follow sidecar names,
    // brackets them with pins, and returns before BareConnection::open.
    let after = FileAck::read(&path);
    if after.identity != a.private.database_file_identity {
        return Err(F::MetadataIdentity);
    }
    if after.bytes != 0 {
        return Err(F::Nonzero(after.bytes));
    }
    parent
        .verify_path_identity()
        .map_err(|e| F::Cause(e.into()))?;
    database.verify().map_err(F::Cause)?;
    if i.private.status != ProviderDatabaseStatus::Uninitialized {
        return Err(F::Status(i.private.status));
    }
    Ok(())
}
