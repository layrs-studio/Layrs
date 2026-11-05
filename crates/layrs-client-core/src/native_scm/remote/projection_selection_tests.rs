//! Isolated HTTP contract fixture: any unexpected content route fails the test.
use super::*;
use serde_json::{Value, json};
use std::{
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

struct Fixture {
    root: std::path::PathBuf,
    state: Arc<Mutex<(Value, Value, Vec<String>)>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    remote: NativeRemote,
}
fn id(n: u8) -> String {
    format!("blake3:{n:064x}")
}
fn path(value: &str) -> RepoPath {
    RepoPath::parse(value).unwrap()
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("layrs-selection-http-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let space = SpaceId::parse("space:demo").unwrap();
        let view = RepositoryView::new(space, vec![], vec![], vec![], vec![]).unwrap();
        let mut base: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/contracts/sync-v3/examples/bootstrap.json"
        )))
        .unwrap();
        base["currentRepositoryView"] = json!({"repositoryViewId":view.id(),"record":view});
        let layer = Layer::new(
            LayerId::parse("layer:test").unwrap(),
            SpaceId::parse("space:demo").unwrap(),
            "Test",
            LayerKind::Base,
            LayerStatus::Active,
            vec![],
            LayerSettings::new(false, 0).unwrap(),
            Timestamp::parse("2026-01-01T00:00:00Z").unwrap(),
            Metadata::empty(),
        )
        .unwrap();
        let product = ProductEnvelope::Layer {
            product_id: layer.id().clone(),
            product_object_id: ObjectId::parse(id(4)).unwrap(),
            depends_on_step_ids: vec![],
            depends_on_product_ids: vec![],
            previous_product_object_id: Nullable::null(),
            record: layer.clone(),
        };
        base["layers"] = serde_json::to_value([LayerProjection {
            product_object_id: product.computed_object_id().unwrap(),
            record: layer,
            head_step_ids: vec![StepId::parse(id(1)).unwrap()],
        }])
        .unwrap();
        let typed: SpaceBootstrap = serde_json::from_value(base.clone()).unwrap();
        typed.validate_v3().unwrap();
        let page = json!({"schemaVersion":"layrs.projection-page.v1","principalId":"principal:alexa",
        "spaceId":"space:demo","stepId":id(1),"rootTreeId":id(2),"policySetId":base["currentPolicySetId"],
        "authorizationEpoch":1,"offset":0,"nextCursor":null,"entries":[
            {"path":"excluded.txt","target":{"kind":"file","fileId":id(5)},"executable":false,"contentReadable":true,"writable":true},
            {"path":"private.txt","target":{"kind":"file","fileId":id(6)},"executable":false,"contentReadable":false,"writable":false}
        ]});
        let state = Arc::new(Mutex::new((base, page, vec![])));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let state = state.clone();
            let stop = stop.clone();
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(value) => value,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(e) => panic!("fixture accept: {e}"),
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut bytes = vec![];
                    let mut buffer = [0u8; 4096];
                    let header_end = loop {
                        let count = stream.read(&mut buffer).unwrap();
                        assert!(count > 0);
                        bytes.extend_from_slice(&buffer[..count]);
                        assert!(bytes.len() < 65536);
                        if let Some(end) = bytes.windows(4).position(|p| p == b"\r\n\r\n") {
                            break end + 4;
                        }
                    };
                    let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
                    let count: usize = headers
                        .lines()
                        .filter_map(|l| l.split_once(':'))
                        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                        .map(|(_, v)| v.trim().parse().unwrap())
                        .unwrap_or(0);
                    while bytes.len() < header_end + count {
                        let n = stream.read(&mut buffer).unwrap();
                        assert!(n > 0);
                        bytes.extend_from_slice(&buffer[..n]);
                    }
                    let request_path = headers.split_whitespace().nth(1).unwrap().to_owned();
                    let (status, value) = {
                        let mut state = state.lock().unwrap();
                        state.2.push(request_path.clone());
                        if request_path.ends_with("/bootstrap") {
                            ("200 OK", state.0.clone())
                        } else if request_path.ends_with("/projection") {
                            let request: Value =
                                serde_json::from_slice(&bytes[header_end..header_end + count])
                                    .unwrap();
                            assert_eq!(request["stepId"], state.1["stepId"]);
                            ("200 OK", state.1.clone())
                        } else {
                            (
                                "403 Forbidden",
                                json!({"error":{"code":"unexpected_content_fetch"}}),
                            )
                        }
                    };
                    let body = serde_json::to_vec(&value).unwrap();
                    write!(stream,"HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
                    stream.write_all(&body).unwrap();
                }
            })
        };
        // Production construction remains HTTPS-only; this private test fixture
        // owns its loopback listener and never uses a real service or credential.
        let remote = NativeRemote {
            http: Client::builder().no_proxy().build().unwrap(),
            origin,
            token: "fixture".into(),
            transfer: None,
            download_cache: None,
        };
        Self {
            root,
            state,
            stop,
            worker: Some(worker),
            remote,
        }
    }
    fn clone_selected(&self) -> ProjectionStore {
        let selection = ProjectionSelection::selected(
            vec![path("private.txt")],
            vec![],
            PathSemantics::PortableWindows,
        )
        .unwrap();
        self.remote
            .clone_projection_selected_into(
                &self.root,
                &SpaceId::parse("space:demo").unwrap(),
                &StepId::parse(id(1)).unwrap(),
                &selection,
            )
            .unwrap()
    }
    fn assert_no_content(&self) {
        let state = self.state.lock().unwrap();
        assert!(!state.2.is_empty());
        assert!(
            state
                .2
                .iter()
                .all(|p| p.ends_with("/bootstrap") || p.ends_with("/projection")),
            "{:?}",
            state.2
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
        assert_eq!(self.root.parent(), Some(std::env::temp_dir().as_path()));
        if !std::thread::panicking() {
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }
}

#[test]
fn selected_clone_and_receive_transfer_no_excluded_or_denied_content() {
    let fixture = Fixture::new();
    let store = fixture.clone_selected();
    let before = store.state().unwrap();
    assert_eq!(before.snapshot.entries[0].state, EntryState::Unavailable);
    assert!(before.snapshot.entries[0].writable);
    assert_eq!(before.snapshot.entries[1].state, EntryState::Reserved);
    assert!(!fixture.root.join("excluded.txt").exists());
    std::fs::write(fixture.root.join("excluded.txt"), b"unowned excluded bytes").unwrap();
    {
        let mut state = fixture.state.lock().unwrap();
        state.0["layers"][0]["headStepIds"] = json!([id(7)]);
        state.1["stepId"] = json!(id(7));
        state.1["rootTreeId"] = json!(id(8));
        state.1["entries"][0]["target"]["fileId"] = json!(id(9));
    }
    let after = fixture
        .remote
        .receive_projection(&store, &LayerId::parse("layer:test").unwrap())
        .unwrap();
    assert_eq!(after.snapshot.entries[0].state, EntryState::Unavailable);
    assert_eq!(
        after.snapshot.entries[0].target,
        Some(TreeEntryTarget::file(FileId::parse(id(9)).unwrap()))
    );
    assert_eq!(
        std::fs::read(fixture.root.join("excluded.txt")).unwrap(),
        b"unowned excluded bytes"
    );
    assert!(
        store
            .capture_patch(after.generation)
            .unwrap()
            .changes
            .is_empty()
    );
    fixture.assert_no_content();
}

#[test]
fn empty_selection_clone_expansion_checks_current_authority_and_cas_without_transfers() {
    let fixture = Fixture::new();
    let store = fixture.clone_selected();
    let before = store.state().unwrap();
    let expanded = ProjectionSelection::selected(
        vec![path("future.txt"), path("private.txt")],
        vec![],
        PathSemantics::PortableWindows,
    )
    .unwrap();
    assert!(
        fixture
            .remote
            .expand_projection_selection(&store, before.generation + 1, &expanded)
            .is_err()
    );
    {
        let mut state = fixture.state.lock().unwrap();
        state.0["authorizationEpoch"] = json!(2);
    }
    assert!(
        fixture
            .remote
            .expand_projection_selection(&store, before.generation, &expanded)
            .is_err()
    );
    assert_eq!(store.state().unwrap(), before);
    fixture.state.lock().unwrap().0["authorizationEpoch"] = json!(1);
    let after = fixture
        .remote
        .expand_projection_selection(&store, before.generation, &expanded)
        .unwrap();
    assert_eq!(after.generation, before.generation + 1);
    assert_eq!(after.snapshot, before.snapshot);
    assert_eq!(store.selection().unwrap(), expanded);
    fixture.assert_no_content();
}

#[test]
fn default_clone_still_attempts_authorized_content_and_failure_leaves_no_metadata() {
    let fixture = Fixture::new();
    assert!(
        fixture
            .remote
            .clone_projection_into(
                &fixture.root,
                &SpaceId::parse("space:demo").unwrap(),
                &StepId::parse(id(1)).unwrap()
            )
            .is_err()
    );
    assert!(!fixture.root.join(".layrs").exists());
    assert!(
        fixture
            .state
            .lock()
            .unwrap()
            .2
            .iter()
            .any(|p| !p.ends_with("/bootstrap") && !p.ends_with("/projection"))
    );
}
