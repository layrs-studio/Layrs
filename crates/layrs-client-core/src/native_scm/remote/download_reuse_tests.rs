//! Counted HTTPS fixture: real plans, ranged responses and native admission.
use super::*;
use serde_json::json;
use std::{
    net::TcpListener,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};

#[derive(Clone, Debug, Default)]
struct Counts {
    plans: usize,
    gets: usize,
    bytes: usize,
    claims: Vec<FetchClaim>,
    ranges: Vec<(usize, usize)>,
    download_events: Vec<(u16, DownloadCapability)>,
    plan_claims: Vec<(PlanId, Vec<FetchClaim>)>,
}
#[derive(Clone, Copy)]
enum RenewalDrift {
    None,
    Epoch,
    Closure,
}
struct Server {
    bootstrap: SpaceBootstrap,
    objects: BTreeMap<ObjectId, CanonicalObject>,
    caps: BTreeMap<String, DownloadCapability>,
    counts: Counts,
    deny_path: Option<String>,
    deny_download: bool,
    corrupt_download: bool,
    omit_capability: bool,
    page_claims: usize,
    snapshot_claims: usize,
    compression: Compression,
    accept_ranges: bool,
    cancel: Option<NativeTransferControl>,
    expiry_path: Option<String>,
    expired_plan: Option<PlanId>,
    expiry_rejected: bool,
    renewal_drift: RenewalDrift,
}
struct Fixture {
    root: PathBuf,
    workspace: NativeWorkspace,
    remote: NativeRemote,
    binding: Binding,
    state: Arc<Mutex<Server>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("layrs-transfer-reuse-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let checkout = root.join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        for (name, byte) in [("a.bin", 1), ("b.bin", 2), ("c.bin", 3)] {
            std::fs::write(checkout.join(name), vec![byte; 32 * 1024]).unwrap();
        }
        let workspace = NativeWorkspace::initialize(&checkout, "Transfer reuse").unwrap();
        let source = workspace.state().unwrap();
        let mut graph = GraphObjects::local(&workspace, &source).unwrap();
        let base = source.view.layer_heads()[0].step_ids()[0].clone();
        let base_step = graph.steps[&base].clone();
        let CanonicalObject::Tree(tree) =
            &graph.objects[&StoreObjectId::Tree(base_step.root_tree_id().clone())]
        else {
            panic!()
        };
        let mut entries = tree.entries().to_vec();
        let chunk = Chunk::new(b"small delta".to_vec()).unwrap();
        let manifest =
            FileManifest::from_chunks(ChunkingDescriptor::single(), std::slice::from_ref(&chunk))
                .unwrap();
        entries.push(TreeEntry::new(
            RepoPathComponent::parse("delta.txt").unwrap(),
            TreeEntryTarget::file(manifest.file_id().clone()),
            false,
        ));
        let tree = Tree::new(entries).unwrap();
        let step = Step::new(vec![base], tree.id().clone()).unwrap();
        graph.insert(CanonicalObject::ChunkRaw(chunk)).unwrap();
        graph
            .insert(CanonicalObject::FileManifest(manifest))
            .unwrap();
        graph.insert(CanonicalObject::Tree(tree)).unwrap();
        graph.insert(CanonicalObject::Step(step.clone())).unwrap();
        let view = RepositoryView::new(
            source.space.id().clone(),
            vec![
                LayerHeadSet::new(source.layers[0].id().clone(), vec![step.id().clone()]).unwrap(),
            ],
            vec![],
            vec![],
            vec![],
        )
        .unwrap();
        let mut bootstrap: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/contracts/sync-v3/examples/bootstrap.json"
        )))
        .unwrap();
        bootstrap["spaceId"] = json!(source.space.id());
        bootstrap["currentPolicySet"]["record"]["spaceId"] = json!(source.space.id());
        bootstrap["currentRepositoryView"] = json!({"repositoryViewId": view.id(), "record": view});
        bootstrap["layers"] = json!([{"productObjectId": step.id(), "record": source.layers[0], "headStepIds": [step.id()]}]);
        bootstrap["graphRootStepIds"] = json!([step.id()]);
        let bootstrap: SpaceBootstrap = serde_json::from_value(bootstrap).unwrap();
        bootstrap.validate_v3().unwrap();
        let mut objects = BTreeMap::new();
        for object in graph
            .objects
            .values()
            .cloned()
            .chain(graph.chunk_objects().map(|object| object.unwrap()))
        {
            objects.insert(
                ObjectId::parse(object.object_id().as_str()).unwrap(),
                object,
            );
        }
        let state = Arc::new(Mutex::new(Server {
            bootstrap,
            objects,
            caps: BTreeMap::new(),
            counts: Counts::default(),
            deny_path: None,
            deny_download: false,
            corrupt_download: false,
            omit_capability: false,
            page_claims: usize::MAX,
            snapshot_claims: usize::MAX,
            compression: Compression::Identity,
            accept_ranges: true,
            cancel: None,
            expiry_path: None,
            expired_plan: None,
            expiry_rejected: false,
            renewal_drift: RenewalDrift::None,
        }));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("https://{}/", listener.local_addr().unwrap());
        let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()).into(),
        )
        .unwrap();
        let config = Arc::new(config);
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let state = state.clone();
            let stop = stop.clone();
            let origin = origin.clone();
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let (socket, _) = match listener.accept() {
                        Ok(value) => value,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                            continue;
                        }
                        Err(e) => panic!("accept: {e}"),
                    };
                    // Windows accepted sockets inherit the listener's mode.
                    socket.set_nonblocking(false).unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    socket
                        .set_write_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut stream = rustls::StreamOwned::new(
                        rustls::ServerConnection::new(config.clone()).unwrap(),
                        socket,
                    );
                    let mut bytes = vec![];
                    let mut buffer = [0; 8192];
                    let end = loop {
                        let n = stream.read(&mut buffer).unwrap();
                        assert!(n > 0);
                        bytes.extend_from_slice(&buffer[..n]);
                        assert!(bytes.len() < 256 * 1024);
                        if let Some(i) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                            break i + 4;
                        }
                    };
                    let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
                    let header = |name: &str| {
                        headers
                            .lines()
                            .filter_map(|l| l.split_once(':'))
                            .find(|(key, _)| key.eq_ignore_ascii_case(name))
                            .map(|(_, v)| v.trim())
                    };
                    let length = header("content-length")
                        .map(|v| v.parse::<usize>().unwrap())
                        .unwrap_or(0);
                    while bytes.len() < end + length {
                        let n = stream.read(&mut buffer).unwrap();
                        assert!(n > 0);
                        bytes.extend_from_slice(&buffer[..n]);
                    }
                    let path = headers.split_whitespace().nth(1).unwrap();
                    let mut state = state.lock().unwrap();
                    let (status, extra, body) = if path.ends_with("/bootstrap") {
                        (
                            "200 OK",
                            String::new(),
                            serde_json::to_vec(&state.bootstrap).unwrap(),
                        )
                    } else if path.ends_with("/fetch-plans") {
                        let request: FetchPlanRequest =
                            serde_json::from_slice(&bytes[end..end + length]).unwrap();
                        if request.claims.len() > state.snapshot_claims {
                            (
                                "413 Payload Too Large",
                                String::new(),
                                br#"{"error":{"code":"payload_too_large"}}"#.to_vec(),
                            )
                        } else {
                            let plan = state.plan(request, &origin);
                            ("200 OK", String::new(), serde_json::to_vec(&plan).unwrap())
                        }
                    } else {
                        assert_eq!(path, "/v3/transfers/downloads");
                        let cap = state.caps[header("authorization")
                            .unwrap()
                            .strip_prefix("Bearer ")
                            .unwrap()]
                        .clone();
                        state.counts.gets += 1;
                        if let Some(control) = state.cancel.take() {
                            control.request_cancel();
                        }
                        if cap.expires_at.as_str() == "2000-01-01T00:00:00Z" {
                            state.expiry_rejected = true;
                            state.counts.download_events.push((404, cap.clone()));
                            (
                                "404 Not Found",
                                String::new(),
                                br#"{"error":{"code":"transfer_unavailable"}}"#.to_vec(),
                            )
                        } else if state.deny_download {
                            state.counts.download_events.push((403, cap.clone()));
                            (
                                "403 Forbidden",
                                String::new(),
                                br#"{"error":{"code":"path_denied"}}"#.to_vec(),
                            )
                        } else {
                            let bytes = state.encoded(&cap.descriptor.object_id);
                            let (status, extra, mut body) = if let Some(range) = header("range") {
                                let (start, end) = range
                                    .strip_prefix("bytes=")
                                    .unwrap()
                                    .split_once('-')
                                    .unwrap();
                                let start: usize = start.parse().unwrap();
                                let end: usize = end.parse().unwrap();
                                state.counts.ranges.push((start, end));
                                (
                                    "206 Partial Content",
                                    format!(
                                        "Content-Range: bytes {start}-{end}/{}\r\n",
                                        bytes.len()
                                    ),
                                    bytes[start..=end].to_vec(),
                                )
                            } else {
                                ("200 OK", String::new(), bytes)
                            };
                            if state.corrupt_download {
                                *body.last_mut().unwrap() ^= 1;
                            }
                            state.counts.bytes += body.len();
                            state.counts.download_events.push((
                                if status == "206 Partial Content" {
                                    206
                                } else {
                                    200
                                },
                                cap.clone(),
                            ));
                            (status, extra, body)
                        }
                    };
                    drop(state);
                    // Cancellation can close the peer before its response arrives.
                    let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n", body.len()).and_then(|_| stream.write_all(&body)).and_then(|_| stream.flush());
                }
            })
        };
        let remote =
            NativeRemote::with_trusted_ca(&origin, "fixture-bearer", &cert.cert.pem().into_bytes())
                .unwrap()
                .with_download_cache(root.join("cache"));
        let binding = Binding {
            origin,
            space_id: source.space.id().clone(),
            client_id: ClientId::parse("client:fixture").unwrap(),
            clone_pending: false,
            observed: None,
            pending: None,
            last_receipt: None,
        };
        Self {
            root,
            workspace,
            remote,
            binding,
            state,
            stop,
            worker: Some(worker),
        }
    }
    fn bootstrap(&self) -> SpaceBootstrap {
        self.state.lock().unwrap().bootstrap.clone()
    }
    fn counts(&self) -> Counts {
        self.state.lock().unwrap().counts.clone()
    }
    fn reset_counts(&self) {
        self.state.lock().unwrap().counts = Counts::default();
    }
    fn graph(&self, reuse: bool) -> RemoteResult<GraphObjects> {
        self.remote.download_graph_from_store(
            &self.binding,
            &self.bootstrap(),
            reuse.then_some(&self.workspace),
        )
    }
    fn bind(&self) {
        save_binding(&self.workspace, &self.binding).unwrap();
    }
    fn add_remote_alias(&self) {
        let mut state = self.state.lock().unwrap();
        let id = ObjectId::parse(state.bootstrap.graph_root_step_ids[0].as_str()).unwrap();
        let CanonicalObject::Step(step) = &state.objects[&id] else {
            panic!()
        };
        let parents = step.parent_step_ids().to_vec();
        let CanonicalObject::Tree(tree) =
            &state.objects[&ObjectId::parse(step.root_tree_id().as_str()).unwrap()]
        else {
            panic!()
        };
        let mut entries = tree.entries().to_vec();
        entries.push(TreeEntry::new(
            RepoPathComponent::parse("aa.bin").unwrap(),
            TreeEntryTarget::file(FileId::from_bytes(&vec![1; 32 * 1024])),
            false,
        ));
        let tree = Tree::new(entries).unwrap();
        let step = Step::new(parents, tree.id().clone()).unwrap();
        state.bootstrap.graph_root_step_ids = vec![step.id().clone()];
        state.bootstrap.layers[0].head_step_ids = vec![step.id().clone()];
        let view = RepositoryView::new(
            state.bootstrap.space_id.clone(),
            vec![LayerHeadSet::new(
                state.bootstrap.layers[0].record.id().clone(),
                vec![step.id().clone()],
            )
            .unwrap()],
            vec![],
            vec![],
            vec![],
        )
        .unwrap();
        state.bootstrap.current_repository_view = RepositoryViewProjection {
            repository_view_id: view.id().clone(),
            record: view,
        };
        state.objects.insert(
            ObjectId::parse(tree.id().as_str()).unwrap(),
            CanonicalObject::Tree(tree),
        );
        state.objects.insert(
            ObjectId::parse(step.id().as_str()).unwrap(),
            CanonicalObject::Step(step),
        );
        state.expiry_path = Some("aa.bin".into());
    }
}
impl Server {
    fn encoded(&self, id: &ObjectId) -> Vec<u8> {
        let bytes = encode(&self.objects[id]).unwrap().1;
        match self.compression {
            Compression::Identity => bytes,
            Compression::Zstd => zstd::stream::encode_all(bytes.as_slice(), 1).unwrap(),
        }
    }
    fn descriptor(&self, id: &ObjectId) -> ObjectDescriptor {
        let (kind, bytes) = encode(&self.objects[id]).unwrap();
        ObjectDescriptor {
            kind,
            object_id: id.clone(),
            canonical_byte_len: bytes.len() as u64,
            encoded_byte_len: self.encoded(id).len() as u64,
            compression: self.compression,
        }
    }
    fn plan(&mut self, request: FetchPlanRequest, origin: &str) -> FetchPlanResponse {
        self.counts.plans += 1;
        self.counts.claims.extend(request.claims.clone());
        let plan_id = PlanId::parse(format!("plan:{}", self.counts.plans)).unwrap();
        self.counts
            .plan_claims
            .push((plan_id.clone(), request.claims.clone()));
        let expiry = Timestamp::parse("2999-01-01T00:00:00Z").unwrap();
        if self.expired_plan.is_none() && request.claims.iter().any(|claim| matches!(claim,
            FetchClaim::File { repo_path, .. } if Some(repo_path.as_str()) == self.expiry_path.as_deref())) {
            self.expired_plan = Some(plan_id.clone());
        }
        let mut plan = FetchPlanResponse {
            protocol: SyncV3Protocol::V3,
            message_type: FetchPlanResponseType::Response,
            principal_id: self.bootstrap.principal_id.clone(),
            authorization_epoch: self.bootstrap.authorization_epoch,
            space_id: request.space_id.clone(),
            client_id: request.client_id.clone(),
            request_id: request.request_id.clone(),
            plan_id: plan_id.clone(),
            file_manifest_selections: vec![],
            download_capabilities: vec![],
            denied_claims: vec![],
            next_page_cursor: Nullable::null(),
            has_more: false,
            expires_at: expiry.clone(),
        };
        for claim in request.claims.iter().take(self.page_claims) {
            if matches!(claim, FetchClaim::File { repo_path, .. } if self.deny_path.as_deref() == Some(repo_path.as_str()))
            {
                plan.denied_claims.push(DeniedClaim {
                    claim: claim.clone(),
                    code: DeniedClaimCode::PathDenied,
                });
                continue;
            }
            let descriptors = match claim {
                FetchClaim::Object { object, .. } => vec![self.descriptor(&object.object_id)],
                FetchClaim::File { file_id, .. } => {
                    let manifest = self
                        .objects
                        .values()
                        .find_map(|o| match o {
                            CanonicalObject::FileManifest(m) if m.file_id() == file_id => Some(m),
                            _ => None,
                        })
                        .unwrap();
                    let descriptor =
                        self.descriptor(&ObjectId::parse(manifest.id().as_str()).unwrap());
                    let mut chunks: Vec<_> = manifest
                        .chunks()
                        .iter()
                        .map(|c| self.descriptor(&ObjectId::parse(c.chunk_id().as_str()).unwrap()))
                        .collect();
                    chunks.sort_by(|a, b| a.object_id.cmp(&b.object_id));
                    chunks.dedup_by(|a, b| a.object_id == b.object_id);
                    plan.file_manifest_selections.push(FileManifestSelection {
                        file_id: file_id.clone(),
                        file_manifest_id: manifest.id().clone(),
                        manifest_descriptor: descriptor.clone(),
                        chunk_descriptors: chunks.clone(),
                    });
                    std::iter::once(descriptor).chain(chunks).collect()
                }
                _ => panic!("unsupported fixture claim"),
            };
            for descriptor in descriptors {
                let token = format!("fixture-download-capability-token-{}", self.caps.len());
                let cap = DownloadCapability {
                    principal_id: plan.principal_id.clone(),
                    authorization_epoch: plan.authorization_epoch,
                    space_id: request.space_id.clone(),
                    client_id: request.client_id.clone(),
                    request_id: request.request_id.clone(),
                    plan_id: plan_id.clone(),
                    claim: claim.clone(),
                    claim_digest: ObjectId::parse(format!(
                        "blake3:{}",
                        blake3::hash(&serde_jcs::to_vec(claim).unwrap()).to_hex()
                    ))
                    .unwrap(),
                    descriptor,
                    method: DownloadMethod::Get,
                    download_url: format!("{origin}v3/transfers/downloads"),
                    token: token.clone(),
                    expires_at: if self.expired_plan.as_ref() == Some(&plan_id)
                        && matches!(claim,
                        FetchClaim::File { repo_path, .. } if Some(repo_path.as_str()) == self.expiry_path.as_deref())
                    {
                        Timestamp::parse("2000-01-01T00:00:00Z").unwrap()
                    } else {
                        expiry.clone()
                    },
                    resume_token: "fixture-resume-token".into(),
                    accept_ranges: self.accept_ranges,
                };
                self.caps.insert(token, cap.clone());
                plan.download_capabilities.push(cap);
            }
        }
        if request.claims.len() > self.page_claims {
            plan.has_more = true;
            plan.next_page_cursor = Nullable::some(Cursor::parse("fixture-next-page").unwrap());
        }
        if self.omit_capability {
            plan.download_capabilities.pop();
        }
        if self.expiry_rejected {
            match self.renewal_drift {
                RenewalDrift::None => (),
                RenewalDrift::Epoch => {
                    plan.authorization_epoch += 1;
                    for cap in &mut plan.download_capabilities {
                        cap.authorization_epoch = plan.authorization_epoch;
                    }
                }
                RenewalDrift::Closure => {
                    // A well-formed but different descriptor/selection must
                    // fail exact renewal, before issuing another object GET.
                    let changed = plan.download_capabilities[0].descriptor.clone();
                    for cap in &mut plan.download_capabilities {
                        if cap.descriptor == changed {
                            cap.descriptor.canonical_byte_len += 1;
                            cap.descriptor.encoded_byte_len += 1;
                        }
                    }
                    for selection in &mut plan.file_manifest_selections {
                        for descriptor in std::iter::once(&mut selection.manifest_descriptor)
                            .chain(&mut selection.chunk_descriptors)
                        {
                            if *descriptor == changed {
                                descriptor.canonical_byte_len += 1;
                                descriptor.encoded_byte_len += 1;
                            }
                        }
                    }
                }
            }
            // Renewal must select by exact claim and descriptor, regardless
            // of order and equal descriptors under other paths.
            plan.download_capabilities.reverse();
            for cap in &plan.download_capabilities {
                self.caps.insert(cap.token.clone(), cap.clone());
            }
        }
        plan
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let worker = self.worker.take().unwrap().join();
        if !thread::panicking() {
            worker.unwrap();
        }
        if !thread::panicking() && self.root.parent() == Some(std::env::temp_dir().as_path()) {
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }
}

#[test]
fn admitted_small_delta_reuses_bytes_and_batches_exact_path_claims() {
    let fixture = Fixture::new();
    let cold = fixture.graph(false).unwrap();
    let cold_counts = fixture.counts();
    fixture.reset_counts();
    let warm = fixture.graph(true).unwrap();
    let warm_counts = fixture.counts();
    let step = fixture.bootstrap().graph_root_step_ids[0].clone();
    assert_eq!(cold.entries(&step).unwrap(), warm.entries(&step).unwrap());
    assert!(warm_counts.bytes < cold_counts.bytes / 4);
    assert!(warm_counts.ranges.iter().filter(|(a, b)| a == b).count() >= 8);
    // Compare batching to the original single-claim transport with precisely
    // the same path/Step authorization work and resulting canonical closure.
    fixture.reset_counts();
    let mut sequential = GraphObjects::default();
    for claim in &warm_counts.claims {
        fixture
            .remote
            .fetch_visit(&fixture.binding, claim.clone(), |o| sequential.insert(o))
            .unwrap();
    }
    let before = fixture.counts();
    assert_eq!(
        sequential.entries(&step).unwrap(),
        warm.entries(&step).unwrap()
    );
    assert!(warm_counts.plans + warm_counts.gets < before.plans + before.gets);
    println!("small delta, same exact claims: sequential cold plans={} GETs={} object_bytes={}; batched cold plans={} GETs={} object_bytes={}; admitted reuse plans={} GETs={} object_bytes={}", before.plans,before.gets,before.bytes,cold_counts.plans,cold_counts.gets,cold_counts.bytes,warm_counts.plans,warm_counts.gets,warm_counts.bytes);
    fixture.bind();
    fixture.remote.receive(&fixture.workspace).unwrap();
    assert_eq!(
        std::fs::read(fixture.workspace.checkout_root().unwrap().join("delta.txt")).unwrap(),
        b"small delta"
    );
    let audit = scm_v1::fsck_existing(
        fixture.workspace.store_root(),
        scm_v1::FsckOptions::default(),
    )
    .unwrap();
    assert!(audit.is_clean(), "{:?}", audit.findings);
    assert!(!audit.is_truncated());
    fixture.reset_counts();
    fixture.remote.receive(&fixture.workspace).unwrap();
    assert_eq!(fixture.counts().plans, 0);
    assert_eq!(fixture.counts().gets, 0);
}

#[test]
fn admitted_bytes_never_override_revocation_or_cancelled_receive() {
    let fixture = Fixture::new();
    fixture.bind();
    let before = fixture.workspace.state().unwrap();
    fixture.state.lock().unwrap().deny_download = true;
    assert!(fixture.remote.receive(&fixture.workspace).is_err());
    assert_eq!(
        fixture.workspace.state().unwrap().operation,
        before.operation
    );
    assert_eq!(fixture.counts().gets, 1);
    fixture.state.lock().unwrap().deny_download = false;
    let control = NativeTransferControl::new();
    fixture.state.lock().unwrap().cancel = Some(control.clone());
    assert!(matches!(
        fixture
            .remote
            .receive_cancellable(&fixture.workspace, &control),
        Err(NativeRemoteError::Cancelled)
    ));
    assert_eq!(
        fixture.workspace.state().unwrap().operation,
        before.operation
    );
    assert!(!fixture
        .workspace
        .checkout_root()
        .unwrap()
        .join("delta.txt")
        .exists());
}

#[test]
fn equal_file_bytes_at_another_path_do_not_share_authorization() {
    let fixture = Fixture::new();
    let bootstrap = fixture.bootstrap();
    let step = &bootstrap.graph_root_step_ids[0];
    let root = AuthorizationRoot::Step {
        step_id: step.clone(),
    };
    let file = FileId::from_bytes(&vec![1; 32 * 1024]);
    let claim = |path: &str| FetchClaim::File {
        file_id: file.clone(),
        preferred_manifest_ids: vec![],
        root: root.clone(),
        repo_path: RepoPath::parse(path).unwrap(),
    };
    fixture.state.lock().unwrap().deny_path = Some("private.bin".into());
    let mut visits = 0;
    assert!(fixture
        .remote
        .fetch_visit_from_store(
            &fixture.binding,
            &[claim("a.bin"), claim("private.bin")],
            Some(&fixture.workspace),
            |_| {
                visits += 1;
                Ok(())
            }
        )
        .is_err());
    assert_eq!(visits, 0);
    assert_eq!(fixture.counts().gets, 0);
    fixture.state.lock().unwrap().deny_path = None;
    fixture
        .remote
        .fetch_visit_from_store(
            &fixture.binding,
            &[claim("a.bin"), claim("private.bin")],
            Some(&fixture.workspace),
            |_| {
                visits += 1;
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(visits, 4);
    assert_eq!(fixture.counts().gets, 4);
}

#[test]
fn paginated_batches_split_but_denied_or_incomplete_closures_never_succeed() {
    let fixture = Fixture::new();
    fixture.state.lock().unwrap().page_claims = 1;
    let graph = fixture.graph(true).unwrap();
    assert_eq!(
        graph
            .entries(&fixture.bootstrap().graph_root_step_ids[0])
            .unwrap()
            .len(),
        4
    );
    fixture.state.lock().unwrap().page_claims = 0;
    assert!(fixture.graph(true).is_err());
    fixture.state.lock().unwrap().page_claims = usize::MAX;
    fixture.state.lock().unwrap().snapshot_claims = 1;
    fixture.graph(true).unwrap();
    fixture.state.lock().unwrap().snapshot_claims = 0;
    assert!(fixture.graph(true).is_err());
    fixture.state.lock().unwrap().snapshot_claims = usize::MAX;
    fixture.state.lock().unwrap().omit_capability = true;
    assert!(fixture.graph(true).is_err());
}

#[test]
fn compressed_or_nonranged_objects_keep_full_download_verification() {
    let fixture = Fixture::new();
    for (compression, ranges) in [(Compression::Identity, false), (Compression::Zstd, true)] {
        fixture.state.lock().unwrap().compression = compression;
        fixture.state.lock().unwrap().accept_ranges = ranges;
        fixture.reset_counts();
        let cold = fixture.graph(false).unwrap();
        let before = fixture.counts();
        fixture.reset_counts();
        let warm = fixture.graph(true).unwrap();
        let after = fixture.counts();
        assert_eq!(before.bytes, after.bytes);
        assert_eq!(
            cold.entries(&fixture.bootstrap().graph_root_step_ids[0])
                .unwrap(),
            warm.entries(&fixture.bootstrap().graph_root_step_ids[0])
                .unwrap()
        );
    }
}

#[test]
fn complete_resume_prefix_still_requires_fresh_authorized_final_byte() {
    let fixture = Fixture::new();
    let claim = FetchClaim::Object {
        object: ObjectReference {
            kind: ObjectKind::Step,
            object_id: ObjectId::parse(fixture.bootstrap().graph_root_step_ids[0].as_str())
                .unwrap(),
        },
        root: AuthorizationRoot::Step {
            step_id: fixture.bootstrap().graph_root_step_ids[0].clone(),
        },
        repo_path: Nullable::null(),
    };
    let plan = fixture
        .remote
        .fetch_plan(&fixture.binding, &claim, Duration::from_secs(5))
        .unwrap();
    let cap = &plan.download_capabilities[0];
    let mut spool = temporary_file().unwrap();
    let mut resume = fixture
        .remote
        .resume_download(cap, &mut spool)
        .unwrap()
        .unwrap();
    let bytes = fixture
        .state
        .lock()
        .unwrap()
        .encoded(&cap.descriptor.object_id);
    spool.write_all(&bytes).unwrap();
    resume.save(&mut spool, bytes.len() as u64).unwrap();
    drop(resume);
    drop(spool);
    fixture.state.lock().unwrap().deny_download = true;
    assert!(fixture
        .remote
        .download_object(cap, |_| panic!("denial must not renew"))
        .is_err());
    fixture.state.lock().unwrap().deny_download = false;
    fixture.reset_counts();
    fixture
        .remote
        .download_object(cap, |_| panic!("unexpired capability"))
        .unwrap();
    assert_eq!(fixture.counts().bytes, 1);
    assert_eq!(fixture.counts().gets, 1);
}

#[test]
fn local_bytes_require_same_space_admission_and_exact_descriptor() {
    let fixture = Fixture::new();
    let claim = FetchClaim::File {
        file_id: FileId::from_bytes(&vec![1; 32 * 1024]),
        preferred_manifest_ids: vec![],
        root: AuthorizationRoot::Step {
            step_id: fixture.bootstrap().graph_root_step_ids[0].clone(),
        },
        repo_path: RepoPath::parse("a.bin").unwrap(),
    };
    let plan = fixture
        .remote
        .fetch_plan(&fixture.binding, &claim, Duration::from_secs(5))
        .unwrap();
    let cap = plan
        .download_capabilities
        .iter()
        .find(|cap| cap.descriptor.kind == ObjectKind::Chunk)
        .unwrap();
    assert!(local::read(&fixture.workspace, cap).unwrap().is_some());
    let mut foreign = cap.clone();
    foreign.space_id = SpaceId::parse("space:other").unwrap();
    assert!(local::read(&fixture.workspace, &foreign).unwrap().is_none());
    let mut incorrect = cap.clone();
    incorrect.descriptor.canonical_byte_len += 1;
    assert!(local::read(&fixture.workspace, &incorrect).is_err());
    assert_eq!(fixture.counts().gets, 0);
}

#[test]
fn local_corruption_and_changed_final_byte_fail_without_admission() {
    let fixture = Fixture::new();
    fixture.state.lock().unwrap().corrupt_download = true;
    assert!(fixture.graph(true).is_err());
    fixture.state.lock().unwrap().corrupt_download = false;
    let chunk = Chunk::new(vec![1; 32 * 1024]).unwrap();
    let path = {
        let store = fixture.workspace.open_store().unwrap();
        scm_v1::cas::NativeCas::open(&store)
            .unwrap()
            .object_path(&StoreObjectId::ChunkRaw(chunk.id().clone()))
            .unwrap()
    };
    // Same-size corruption must be detected by the safe CAS reader's digest.
    std::fs::write(path, vec![9; 32 * 1024]).unwrap();
    assert!(fixture.graph(true).is_err());
    assert!(!fixture
        .workspace
        .checkout_root()
        .unwrap()
        .join("delta.txt")
        .exists());
}

#[test]
fn secondary_receive_preserves_foreign_operation_and_checkout() {
    let fixture = Fixture::new();
    let initial = fixture.workspace.state().unwrap();
    let path = fixture.root.join("secondary");
    std::fs::create_dir(&path).unwrap();
    let secondary = fixture
        .workspace
        .create_worktree(
            &path,
            initial.layers[0].id(),
            &initial.view.layer_heads()[0].step_ids()[0],
        )
        .unwrap();
    std::fs::write(
        fixture
            .workspace
            .checkout_root()
            .unwrap()
            .join("primary.txt"),
        b"private primary",
    )
    .unwrap();
    fixture.workspace.capture().unwrap();
    let primary = fixture.workspace.state().unwrap();
    let before = secondary.state().unwrap();
    save_binding(&secondary, &fixture.binding).unwrap();
    fixture
        .remote
        .receive_cancellable(&secondary, &NativeTransferControl::new())
        .unwrap();
    let after = secondary.state().unwrap();
    assert_eq!(
        after.operation.parent_operation_ids(),
        &[before.operation.id().clone()]
    );
    assert_eq!(
        fixture.workspace.state().unwrap().operation,
        primary.operation
    );
    assert_eq!(
        std::fs::read(path.join("delta.txt")).unwrap(),
        b"small delta"
    );
    assert!(!path.join("primary.txt").exists());
    assert!(!fixture
        .workspace
        .checkout_root()
        .unwrap()
        .join("delta.txt")
        .exists());
    let store = fixture.workspace.open_store().unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let heads = reader.operation_heads(initial.space.id()).unwrap();
    assert!(heads.contains(primary.operation.id()));
    assert!(heads.contains(after.operation.id()));
    assert!(!heads.contains(before.operation.id()));
}

#[test]
fn midway_batch_expiry_renews_equal_descriptors_under_exact_path_authority() {
    let fixture = Fixture::new();
    fixture.add_remote_alias();
    fixture.bind();
    fixture
        .remote
        .receive_cancellable(&fixture.workspace, &NativeTransferControl::new())
        .unwrap();
    let counts = fixture.counts();
    let failures: Vec<_> = counts
        .download_events
        .iter()
        .enumerate()
        .filter(|(_, (status, _))| *status == 404)
        .collect();
    assert_eq!(failures.len(), 1);
    let (index, (_, expired)) = failures[0];
    assert!(counts.download_events[..index]
        .iter()
        .any(|(status, cap)| *status == 206
            && cap.plan_id == expired.plan_id
            && cap.descriptor == expired.descriptor
            && cap.claim != expired.claim));
    let (status, renewed) = &counts.download_events[index + 1];
    assert_eq!(*status, 206);
    assert_eq!(renewed.claim, expired.claim);
    assert_eq!(renewed.claim_digest, expired.claim_digest);
    assert_eq!(renewed.descriptor, expired.descriptor);
    assert_eq!(renewed.principal_id, expired.principal_id);
    assert_eq!(renewed.authorization_epoch, expired.authorization_epoch);
    assert_ne!(renewed.plan_id, expired.plan_id);
    let original = counts
        .plan_claims
        .iter()
        .find(|(id, _)| id == &expired.plan_id)
        .unwrap();
    let replacement = counts
        .plan_claims
        .iter()
        .find(|(id, _)| id == &renewed.plan_id)
        .unwrap();
    assert_eq!(original.1, replacement.1);
    assert!(counts.download_events[index + 1..]
        .iter()
        .filter(|(_, cap)| cap.plan_id == renewed.plan_id)
        .all(|(status, _)| *status == 206));
    let root = fixture.workspace.checkout_root().unwrap();
    assert_eq!(
        std::fs::read(root.join("a.bin")).unwrap(),
        std::fs::read(root.join("aa.bin")).unwrap()
    );
    assert_eq!(
        std::fs::read(root.join("delta.txt")).unwrap(),
        b"small delta"
    );
}

#[test]
fn midway_batch_renewal_authority_or_closure_drift_never_advances_operation() {
    for drift in [RenewalDrift::Epoch, RenewalDrift::Closure] {
        let fixture = Fixture::new();
        fixture.add_remote_alias();
        fixture.bind();
        fixture.state.lock().unwrap().renewal_drift = drift;
        let before = fixture.workspace.state().unwrap();
        let error = fixture
            .remote
            .receive_cancellable(&fixture.workspace, &NativeTransferControl::new())
            .unwrap_err();
        assert!(
            matches!(
                error,
                NativeRemoteError::Invalid("download renewal changed authority or object closure")
            ),
            "{error}"
        );
        let counts = fixture.counts();
        let (last_status, expired) = counts.download_events.last().unwrap();
        assert_eq!(*last_status, 404);
        assert!(counts
            .download_events
            .iter()
            .any(|(status, cap)| *status == 206 && cap.plan_id == expired.plan_id));
        let after = fixture.workspace.state().unwrap();
        assert_eq!(after.operation, before.operation);
        assert_eq!(after.view, before.view);
        let root = fixture.workspace.checkout_root().unwrap();
        assert!(!root.join("aa.bin").exists());
        assert!(!root.join("delta.txt").exists());
        let store = fixture.workspace.open_store().unwrap();
        let reader = NativeScmReader::new(&store).unwrap();
        assert!(!reader
            .object_admitted_to_space(
                &StoreObjectId::Step(fixture.bootstrap().graph_root_step_ids[0].clone()),
                before.space.id()
            )
            .unwrap());
    }
}
