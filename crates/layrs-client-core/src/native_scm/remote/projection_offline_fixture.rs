//! Private counted TLS service. It owns every credential, socket and filesystem path.
use super::*;
use layrs_core::projection::PathSemantics;
use layrs_store_local::projection::{PartialStoreFamily, PartialStoreIdentity, SharedQuota};
use serde_json::{Value, json};
use std::{
    net::TcpListener,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

pub(super) fn path(value: &str) -> RepoPath {
    RepoPath::parse(value).unwrap()
}

pub(super) struct Server {
    pub base: SpaceBootstrap,
    pub step: Step,
    pub objects: BTreeMap<ObjectId, CanonicalObject>,
    pub caps: BTreeMap<String, DownloadCapability>,
    pub events: Vec<String>,
    pub access: Vec<ProjectionAccessRequest>,
    pub claims: Vec<FetchClaim>,
    pub gets: Vec<DownloadCapability>,
    pub denied: BTreeSet<RepoPath>,
    pub read_only: BTreeSet<RepoPath>,
    pub probe: Option<PathBuf>,
    pub advance_after_download: Option<PathBuf>,
    pub revoke_after_download: bool,
    pub corrupt_payload: bool,
    pub wrong_root: bool,
}
pub(super) struct Fixture {
    pub root: PathBuf,
    pub remote: NativeRemote,
    pub server: Arc<Mutex<Server>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    pub fn new() -> Self {
        Self::with_excluded_bytes(b"exact offline bytes\n")
    }
    pub fn with_excluded_bytes(excluded: &[u8]) -> Self {
        let root = std::env::temp_dir().join(format!("layrs-offline-tls-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let chunk = Chunk::new(b"exact offline bytes\n".to_vec()).unwrap();
        let manifest =
            FileManifest::from_chunks(ChunkingDescriptor::single(), std::slice::from_ref(&chunk))
                .unwrap();
        let excluded_chunk = Chunk::new(excluded.to_vec()).unwrap();
        let excluded_manifest = FileManifest::from_chunks(
            ChunkingDescriptor::single(),
            std::slice::from_ref(&excluded_chunk),
        )
        .unwrap();
        let tree = Tree::new(
            ["allowed.txt", "excluded.txt", "private.txt"]
                .into_iter()
                .map(|name| {
                    TreeEntry::new(
                        RepoPathComponent::parse(name).unwrap(),
                        TreeEntryTarget::file(if name == "excluded.txt" {
                            excluded_manifest.file_id().clone()
                        } else {
                            manifest.file_id().clone()
                        }),
                        false,
                    )
                })
                .collect(),
        )
        .unwrap();
        let step = Step::new(vec![], tree.id().clone()).unwrap();
        let space = SpaceId::parse("space:demo").unwrap();
        let layer = LayerId::parse("layer:test").unwrap();
        let view = RepositoryView::new(
            space.clone(),
            vec![LayerHeadSet::new(layer, vec![step.id().clone()]).unwrap()],
            vec![],
            vec![],
            vec![],
        )
        .unwrap();
        let mut base: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/contracts/sync-v3/examples/bootstrap.json"
        )))
        .unwrap();
        base["currentRepositoryView"] = json!({"repositoryViewId":view.id(),"record":view});
        let record = Layer::new(
            LayerId::parse("layer:test").unwrap(),
            space,
            "Test",
            LayerKind::Base,
            LayerStatus::Active,
            vec![],
            LayerSettings::new(false, 0).unwrap(),
            Timestamp::parse("2026-01-01T00:00:00Z").unwrap(),
            Metadata::empty(),
        )
        .unwrap();
        let envelope = ProductEnvelope::Layer {
            product_id: record.id().clone(),
            product_object_id: ObjectId::parse(step.id().as_str()).unwrap(),
            depends_on_step_ids: vec![],
            depends_on_product_ids: vec![],
            previous_product_object_id: Nullable::null(),
            record: record.clone(),
        };
        base["layers"] = serde_json::to_value([LayerProjection {
            product_object_id: envelope.computed_object_id().unwrap(),
            record,
            head_step_ids: vec![step.id().clone()],
        }])
        .unwrap();
        base["graphRootStepIds"] = json!([step.id()]);
        let base: SpaceBootstrap = serde_json::from_value(base).unwrap();
        base.validate_v3().unwrap();
        let objects = [
            CanonicalObject::ChunkRaw(chunk),
            CanonicalObject::FileManifest(manifest),
            CanonicalObject::ChunkRaw(excluded_chunk),
            CanonicalObject::FileManifest(excluded_manifest),
            CanonicalObject::Tree(tree),
            CanonicalObject::Step(step.clone()),
        ]
        .into_iter()
        .map(|o| (ObjectId::parse(o.object_id().as_str()).unwrap(), o))
        .collect();
        let server = Arc::new(Mutex::new(Server {
            base,
            step,
            objects,
            caps: BTreeMap::new(),
            events: vec![],
            access: vec![],
            claims: vec![],
            gets: vec![],
            denied: BTreeSet::from([path("private.txt")]),
            read_only: BTreeSet::new(),
            probe: None,
            advance_after_download: None,
            revoke_after_download: false,
            corrupt_payload: false,
            wrong_root: false,
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
            let server = server.clone();
            let stop = stop.clone();
            let origin = origin.clone();
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let (socket, _) = match listener.accept() {
                        Ok(v) => v,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                            continue;
                        }
                        Err(e) => panic!("fixture accept: {e}"),
                    };
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
                    let mut bytes = Vec::new();
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
                            .find(|(k, _)| k.eq_ignore_ascii_case(name))
                            .map(|(_, v)| v.trim())
                    };
                    let length = header("content-length")
                        .map(|v| v.parse::<usize>().unwrap())
                        .unwrap_or(0);
                    assert!(length < 256 * 1024);
                    while bytes.len() < end + length {
                        let n = stream.read(&mut buffer).unwrap();
                        assert!(n > 0);
                        bytes.extend_from_slice(&buffer[..n]);
                    }
                    let route = headers.split_whitespace().nth(1).unwrap();
                    let body = if length == 0 {
                        Value::Null
                    } else {
                        serde_json::from_slice(&bytes[end..end + length]).unwrap()
                    };
                    let mut state = server.lock().unwrap();
                    if let Some(root) = &state.probe {
                        ProjectionStore::open(root)
                            .unwrap()
                            .state()
                            .expect("network must hold no Store lock");
                    }
                    state.events.push(route.into());
                    let (status, body) =
                        state.respond(route, body, header("authorization"), &origin);
                    drop(state);
                    write!(
                        stream,
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .unwrap();
                    stream.write_all(&body).unwrap();
                    stream.flush().unwrap();
                }
            })
        };
        let remote =
            NativeRemote::with_trusted_ca(&origin, "private-fixture", cert.cert.pem().as_bytes())
                .unwrap();
        Self {
            root,
            remote,
            server,
            stop,
            worker: Some(worker),
        }
    }
    pub fn snapshot(&self) -> ProjectionSnapshot {
        let s = self.server.lock().unwrap();
        let CanonicalObject::Tree(tree) =
            &s.objects[&ObjectId::parse(s.step.root_tree_id().as_str()).unwrap()]
        else {
            panic!()
        };
        ProjectionSnapshot {
            schema_version: "layrs.projection-snapshot.v1".into(),
            base_step_id: s.step.id().clone(),
            base_root_tree_id: s.step.root_tree_id().clone(),
            observed_policy_set_id: s.base.current_policy_set_id.clone(),
            observed_authorization_epoch: s.base.authorization_epoch,
            path_semantics: PathSemantics::PortableWindows,
            entries: tree
                .entries()
                .iter()
                .map(|e| ProjectionEntry {
                    path: path(e.name().as_str()),
                    target: Some(e.target().clone()),
                    executable: false,
                    writable: !s.denied.contains(&path(e.name().as_str())),
                    state: if s.denied.contains(&path(e.name().as_str())) {
                        EntryState::Reserved
                    } else {
                        EntryState::Unavailable
                    },
                })
                .collect(),
        }
    }
    pub fn family(&self) -> PartialStoreFamily {
        let root = self.root.join(format!("family-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let s = self.server.lock().unwrap();
        PartialStoreFamily::create(
            root,
            PartialStoreIdentity {
                store_id: StoreId::parse(format!("store:{}", uuid::Uuid::new_v4())).unwrap(),
                space_id: s.base.space_id.clone(),
                origin: self.remote.origin().trim_end_matches('/').into(),
                principal_id: s.base.principal_id.to_string(),
            },
            SharedQuota {
                bytes: 16 * 1024 * 1024 * 1024,
            },
        )
        .unwrap()
    }
    pub fn store(&self) -> ProjectionStore {
        let root = self.root.join("checkout");
        std::fs::create_dir(&root).unwrap();
        let store = self
            .family()
            .create_worktree(
                &root,
                WorktreeId::parse("worktree:destination").unwrap(),
                self.snapshot(),
                ProjectionSelection::selected(
                    vec![path("allowed.txt")],
                    vec![],
                    PathSemantics::PortableWindows,
                )
                .unwrap(),
            )
            .unwrap();
        self.server.lock().unwrap().probe = Some(root);
        store
    }
    pub fn file_gets(&self) -> usize {
        self.server
            .lock()
            .unwrap()
            .gets
            .iter()
            .filter(|c| matches!(c.claim, FetchClaim::File { .. }))
            .count()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let joined = self.worker.take().unwrap().join();
        if !std::thread::panicking() {
            joined.unwrap();
            assert_eq!(self.root.parent(), Some(std::env::temp_dir().as_path()));
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }
}
impl Server {
    pub fn advance(&mut self, bytes: &[u8]) {
        let chunk = Chunk::new(bytes.to_vec()).unwrap();
        let manifest =
            FileManifest::from_chunks(ChunkingDescriptor::single(), std::slice::from_ref(&chunk))
                .unwrap();
        let CanonicalObject::Tree(old) =
            &self.objects[&ObjectId::parse(self.step.root_tree_id().as_str()).unwrap()]
        else {
            panic!()
        };
        let entries = old
            .entries()
            .iter()
            .map(|e| {
                if e.name().as_str() == "allowed.txt" {
                    TreeEntry::new(
                        e.name().clone(),
                        TreeEntryTarget::file(manifest.file_id().clone()),
                        false,
                    )
                } else {
                    e.clone()
                }
            })
            .collect();
        let tree = Tree::new(entries).unwrap();
        let step = Step::new(vec![self.step.id().clone()], tree.id().clone()).unwrap();
        self.base.layers[0].head_step_ids = vec![step.id().clone()];
        self.base.graph_root_step_ids = vec![step.id().clone()];
        let view = RepositoryView::new(
            self.base.space_id.clone(),
            vec![
                LayerHeadSet::new(
                    self.base.layers[0].record.id().clone(),
                    vec![step.id().clone()],
                )
                .unwrap(),
            ],
            vec![],
            vec![],
            vec![],
        )
        .unwrap();
        self.base.current_repository_view.repository_view_id = view.id().clone();
        self.base.current_repository_view.record = view;
        self.base.current_repository_view_generation += 1;
        for object in [
            CanonicalObject::ChunkRaw(chunk),
            CanonicalObject::FileManifest(manifest),
            CanonicalObject::Tree(tree),
            CanonicalObject::Step(step.clone()),
        ] {
            self.objects.insert(
                ObjectId::parse(object.object_id().as_str()).unwrap(),
                object,
            );
        }
        self.step = step;
    }
    fn descriptor(&self, id: &ObjectId) -> ObjectDescriptor {
        let (kind, bytes) = encode(&self.objects[id]).unwrap();
        ObjectDescriptor {
            kind,
            object_id: id.clone(),
            canonical_byte_len: bytes.len() as u64,
            encoded_byte_len: bytes.len() as u64,
            compression: Compression::Identity,
        }
    }
    fn respond(
        &mut self,
        route: &str,
        body: Value,
        bearer: Option<&str>,
        origin: &str,
    ) -> (&'static str, Vec<u8>) {
        let value = if route.ends_with("/bootstrap") {
            if self
                .gets
                .iter()
                .any(|c| c.descriptor.kind == ObjectKind::Chunk)
            {
                if self.revoke_after_download {
                    self.base.authorization_epoch += 1;
                    self.revoke_after_download = false;
                }
                if let Some(root) = self.advance_after_download.take() {
                    let store = ProjectionStore::open(root).unwrap();
                    let state = store.state().unwrap();
                    store
                        .compare_and_swap(state.generation, &state.snapshot)
                        .unwrap();
                }
            }
            serde_json::to_value(&self.base).unwrap()
        } else if route.ends_with("/whoami") {
            json!({"principal_id": self.base.principal_id})
        } else if route.ends_with("/projection") {
            let request: ProjectionPageRequest = serde_json::from_value(body).unwrap();
            assert_eq!(request.step_id, *self.step.id());
            let CanonicalObject::Tree(tree) =
                &self.objects[&ObjectId::parse(self.step.root_tree_id().as_str()).unwrap()]
            else {
                panic!()
            };
            let entries: Vec<_> = tree.entries().iter().map(|e| json!({"path":e.name().as_str(),"target":e.target(),"executable":false,"contentReadable":!self.denied.contains(&path(e.name().as_str())),"writable":!self.denied.contains(&path(e.name().as_str())) && !self.read_only.contains(&path(e.name().as_str()))})).collect();
            json!({"schemaVersion":"layrs.projection-page.v1","principalId":self.base.principal_id,"spaceId":self.base.space_id,"stepId":self.step.id(),"rootTreeId":self.step.root_tree_id(),"policySetId":self.base.current_policy_set_id,"authorizationEpoch":self.base.authorization_epoch,"offset":0,"nextCursor":null,"entries":entries})
        } else if route.ends_with("/projection/access") {
            let request: ProjectionAccessRequest = serde_json::from_value(body).unwrap();
            assert_eq!(
                request.expected_policy_set_id,
                self.base.current_policy_set_id
            );
            assert_eq!(
                request.expected_authorization_epoch,
                self.base.authorization_epoch
            );
            self.access.push(request.clone());
            let CanonicalObject::Step(step) =
                &self.objects[&ObjectId::parse(request.step_id.as_str()).unwrap()]
            else {
                panic!()
            };
            let CanonicalObject::Tree(tree) =
                &self.objects[&ObjectId::parse(step.root_tree_id().as_str()).unwrap()]
            else {
                panic!()
            };
            let entries: Vec<_> = request.paths.iter().map(|p| {
                let e = tree.entries().iter().find(|e| e.name().as_str() == p.as_str());
                json!({"path":p, "target":e.map(|e|e.target()), "executable":false,"contentReadable":!self.denied.contains(p),"writable":!self.denied.contains(p) && !self.read_only.contains(p)})
            }).collect();
            let root = if self.wrong_root {
                Tree::new(vec![]).unwrap().id().clone()
            } else {
                step.root_tree_id().clone()
            };
            json!({"schemaVersion":"layrs.projection-access.v1","principalId":self.base.principal_id,"spaceId":self.base.space_id,"stepId":step.id(),"rootTreeId":root,"policySetId":self.base.current_policy_set_id,"authorizationEpoch":self.base.authorization_epoch,"entries":entries})
        } else if route.ends_with("/fetch-plans") {
            serde_json::to_value(self.plan(serde_json::from_value(body).unwrap(), origin)).unwrap()
        } else if route == "/v3/transfers/downloads" {
            let cap = self.caps[bearer.unwrap().strip_prefix("Bearer ").unwrap()].clone();
            self.gets.push(cap.clone());
            let mut bytes = encode(&self.objects[&cap.descriptor.object_id]).unwrap().1;
            if self.corrupt_payload && cap.descriptor.kind == ObjectKind::Chunk {
                bytes[0] ^= 1;
            }
            return ("200 OK", bytes);
        } else {
            panic!("unexpected network route: {route}");
        };
        ("200 OK", serde_json::to_vec(&value).unwrap())
    }
    fn plan(&mut self, request: FetchPlanRequest, origin: &str) -> FetchPlanResponse {
        self.claims.extend(request.claims.clone());
        let plan_id = PlanId::parse(format!("plan:{}", self.claims.len())).unwrap();
        let expiry = Timestamp::parse("2999-01-01T00:00:00Z").unwrap();
        let mut plan = FetchPlanResponse {
            protocol: SyncV3Protocol::V3,
            message_type: FetchPlanResponseType::Response,
            principal_id: self.base.principal_id.clone(),
            authorization_epoch: self.base.authorization_epoch,
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
        for claim in request.claims {
            if matches!(&claim,FetchClaim::File { repo_path,.. } if self.denied.contains(repo_path))
            {
                plan.denied_claims.push(DeniedClaim {
                    claim,
                    code: DeniedClaimCode::PathDenied,
                });
                continue;
            }
            let descriptors = match &claim {
                FetchClaim::Object { object, .. } => vec![self.descriptor(&object.object_id)],
                FetchClaim::File { file_id, .. } => {
                    let m = self
                        .objects
                        .values()
                        .find_map(|o| match o {
                            CanonicalObject::FileManifest(m) if m.file_id() == file_id => Some(m),
                            _ => None,
                        })
                        .unwrap();
                    let d = self.descriptor(&ObjectId::parse(m.id().as_str()).unwrap());
                    let chunks: Vec<_> = m
                        .chunks()
                        .iter()
                        .map(|c| self.descriptor(&ObjectId::parse(c.chunk_id().as_str()).unwrap()))
                        .collect();
                    plan.file_manifest_selections.push(FileManifestSelection {
                        file_id: file_id.clone(),
                        file_manifest_id: m.id().clone(),
                        manifest_descriptor: d.clone(),
                        chunk_descriptors: chunks.clone(),
                    });
                    std::iter::once(d).chain(chunks).collect()
                }
                _ => panic!("unsupported fixture claim"),
            };
            for descriptor in descriptors {
                let token = format!("offline-fixture-download-token-{}", self.caps.len());
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
                        blake3::hash(&canonical_json(&claim).unwrap())
                    ))
                    .unwrap(),
                    descriptor,
                    method: DownloadMethod::Get,
                    download_url: format!("{origin}v3/transfers/downloads"),
                    token: token.clone(),
                    expires_at: expiry.clone(),
                    resume_token: "offline-fixture-resume".into(),
                    accept_ranges: false,
                };
                self.caps.insert(token, cap.clone());
                plan.download_capabilities.push(cap);
            }
        }
        plan
    }
}
