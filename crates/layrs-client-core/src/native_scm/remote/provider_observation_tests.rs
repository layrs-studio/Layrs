//! Authored source-only TLS contract tests. Never executed in the proposal task.
//! The fixture pattern follows projection_offline_fixture, without any Store.
use super::*;
use layrs_core::projection::PathSemantics;
use serde_json::{json, Value};
use std::{
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};

fn path(value: &str) -> RepoPath {
    RepoPath::parse(value).unwrap()
}
fn id(n: u64) -> ObjectId {
    ObjectId::parse(format!("blake3:{n:064x}")).unwrap()
}

struct Fault {
    route: &'static str,
    occurrence: usize,
    pointer: &'static str,
    value: Value,
}
struct Server {
    base: SpaceBootstrap,
    step: Step,
    namespace: Vec<ProjectionEntry>,
    manifests: BTreeMap<FileId, FileManifest>,
    objects: BTreeMap<ObjectId, CanonicalObject>,
    denied: BTreeSet<RepoPath>,
    readonly: BTreeSet<RepoPath>,
    events: Vec<(String, Value)>,
    gets: Vec<ObjectKind>,
    caps: BTreeMap<String, DownloadCapability>,
    faults: Vec<Fault>,
    page_size: usize,
    expire_manifest_once: bool,
    expired: bool,
    drift_whole_plan: bool,
    drift_renewal_epoch: bool,
    drift_after_manifest: bool,
    corrupt_manifest: bool,
    substitute_manifest: bool,
    cancel_on_access: Option<NativeTransferControl>,
    validate_faulted_renewal: bool,
    validated_renewals: usize,
}
struct Fixture {
    remote: NativeRemote,
    server: Arc<Mutex<Server>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn new() -> Self {
        let chunk =
            Chunk::new(b"metadata only; these bytes must never be requested".to_vec()).unwrap();
        let manifest =
            FileManifest::from_chunks(ChunkingDescriptor::single(), std::slice::from_ref(&chunk))
                .unwrap();
        let tree = Tree::new(
            ["a.txt", "b.txt", "c.txt"]
                .into_iter()
                .map(|n| {
                    TreeEntry::new(
                        RepoPathComponent::parse(n).unwrap(),
                        TreeEntryTarget::file(manifest.file_id().clone()),
                        false,
                    )
                })
                .collect(),
        )
        .unwrap();
        let step = Step::new(vec![], tree.id().clone()).unwrap();
        let space = SpaceId::parse("space:demo").unwrap();
        let view = RepositoryView::new(space.clone(), vec![], vec![], vec![], vec![]).unwrap();
        let mut value: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/contracts/sync-v3/examples/bootstrap.json"
        )))
        .unwrap();
        value["currentRepositoryView"] = json!({"repositoryViewId":view.id(),"record":view});
        let layer = Layer::new(
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
        let product = ProductEnvelope::Layer {
            product_id: layer.id().clone(),
            product_object_id: id(4),
            depends_on_step_ids: vec![],
            depends_on_product_ids: vec![],
            previous_product_object_id: Nullable::null(),
            record: layer.clone(),
        };
        value["layers"] = serde_json::to_value([LayerProjection {
            product_object_id: product.computed_object_id().unwrap(),
            record: layer,
            head_step_ids: vec![step.id().clone()],
        }])
        .unwrap();
        value["graphRootStepIds"] = json!([step.id()]);
        let base: SpaceBootstrap = serde_json::from_value(value).unwrap();
        base.validate_v3().unwrap();
        let namespace = tree
            .entries()
            .iter()
            .map(|e| ProjectionEntry {
                path: path(e.name().as_str()),
                target: e.target().clone(),
                executable: false,
                content_readable: true,
                writable: true,
            })
            .collect();
        let objects = [
            CanonicalObject::ChunkRaw(chunk),
            CanonicalObject::FileManifest(manifest.clone()),
            CanonicalObject::Tree(tree),
            CanonicalObject::Step(step.clone()),
        ]
        .into_iter()
        .map(|o| (ObjectId::parse(o.object_id().as_str()).unwrap(), o))
        .collect();
        let server = Arc::new(Mutex::new(Server {
            base,
            step,
            namespace,
            manifests: BTreeMap::from([(manifest.file_id().clone(), manifest)]),
            objects,
            denied: BTreeSet::new(),
            readonly: BTreeSet::new(),
            events: vec![],
            gets: vec![],
            caps: BTreeMap::new(),
            faults: vec![],
            page_size: 1,
            expire_manifest_once: false,
            expired: false,
            drift_whole_plan: false,
            drift_renewal_epoch: false,
            drift_after_manifest: false,
            corrupt_manifest: false,
            substitute_manifest: false,
            cancel_on_access: None,
            validate_faulted_renewal: false,
            validated_renewals: 0,
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
                    let (status, body) = server.lock().unwrap().respond(
                        route,
                        body,
                        header("authorization"),
                        &origin,
                    );
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
        let remote = NativeRemote::with_trusted_ca(
            &origin,
            "private-fixture-bearer",
            cert.cert.pem().as_bytes(),
        )
        .unwrap();
        Self {
            remote,
            server,
            stop,
            worker: Some(worker),
        }
    }
    fn original(&self) -> ProjectionSnapshot {
        let s = self.server.lock().unwrap();
        ProjectionSnapshot {
            schema_version: "layrs.projection-snapshot.v1".into(),
            base_step_id: s.step.id().clone(),
            base_root_tree_id: s.step.root_tree_id().clone(),
            observed_policy_set_id: s.base.current_policy_set_id.clone(),
            observed_authorization_epoch: s.base.authorization_epoch,
            path_semantics: PathSemantics::PortableWindows,
            entries: s
                .namespace
                .iter()
                .map(|e| LocalEntry {
                    path: e.path.clone(),
                    target: Some(e.target.clone()),
                    executable: e.executable,
                    writable: e.writable,
                    state: EntryState::Unavailable,
                })
                .collect(),
        }
    }
    fn observe(
        &self,
        original: &ProjectionSnapshot,
        strategy: ProviderTransitionObservationStrategy,
        expanded: &ProjectionSelection,
        cached: &[ProviderTransitionDescriptor],
        absence: &[RepoPath],
        control: Option<&NativeTransferControl>,
    ) -> RemoteResult<ObservedProviderTransitionScope> {
        let (space, principal) = {
            let s = self.server.lock().unwrap();
            (s.base.space_id.clone(), s.base.principal_id.clone())
        };
        self.remote
            .observe_provider_transition(&ProviderTransitionObservationRequest {
                source: &anchor(),
                origin: self.remote.origin(),
                space_id: &space,
                principal_id: &principal,
                original,
                original_selection: &selected(&["a.txt"]),
                expanded_selection: expanded,
                explicit_absence_paths: absence,
                historical_descriptors: cached,
                strategy,
                control,
            })
    }
    fn fault(&self, route: &'static str, occurrence: usize, pointer: &'static str, value: Value) {
        self.server.lock().unwrap().faults.push(Fault {
            route,
            occurrence,
            pointer,
            value,
        });
    }
    fn assert_no_chunks(&self) {
        let s = self.server.lock().unwrap();
        assert!(s.gets.iter().all(|k| *k == ObjectKind::FileManifest));
        for (route, body) in &s.events {
            if route.ends_with("/fetch-plans") {
                let request: FetchPlanRequest = serde_json::from_value(body.clone()).unwrap();
                assert_eq!(request.claims.len(), 1);
                assert!(
                    matches!(&request.claims[0], FetchClaim::File { root: AuthorizationRoot::Step { step_id }, .. } if step_id == s.step.id())
                );
            }
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}
fn selected(paths: &[&str]) -> ProjectionSelection {
    ProjectionSelection::selected(
        paths.iter().map(|p| path(p)).collect(),
        vec![],
        PathSemantics::PortableWindows,
    )
    .unwrap()
}
fn anchor() -> ProviderTransitionSourceAnchor {
    ProviderTransitionSourceAnchor {
        store_id: StoreId::parse("store:source").unwrap(),
        worktree_id: WorktreeId::parse("worktree:source").unwrap(),
        source_prefix_id: id(90),
        generation: 0,
        snapshot_digest: "1".repeat(64),
        snapshot_parent_digest: None,
        selection_generation: 0,
        selection_snapshot_digest: "1".repeat(64),
        selection_digest: "2".repeat(64),
        capture_count: 0,
        capture_head: None,
        effective_image_digest: "3".repeat(64),
    }
}
impl Server {
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
        if route != "/v3/transfers/downloads" {
            assert_eq!(bearer, Some("Bearer private-fixture-bearer"));
        }
        self.events.push((route.into(), body.clone()));
        let occurrence = self.events.iter().filter(|(r, _)| r == route).count();
        let mut value = if route.ends_with("/bootstrap") {
            serde_json::to_value(&self.base).unwrap()
        } else if route.ends_with("/whoami") {
            json!({"principal_id":self.base.principal_id})
        } else if route.ends_with("/projection") {
            let request: ProjectionPageRequest = serde_json::from_value(body).unwrap();
            assert_eq!(request.step_id, *self.step.id());
            let offset = request
                .cursor
                .as_ref()
                .map(|s| s.parse::<usize>().unwrap())
                .unwrap_or(0);
            let count = self.page_size.min(request.limit as usize);
            let entries: Vec<_> = self
                .namespace
                .iter()
                .skip(offset)
                .take(count)
                .cloned()
                .map(|mut e| {
                    e.content_readable = !self.denied.contains(&e.path);
                    e.writable = e.content_readable && !self.readonly.contains(&e.path);
                    e
                })
                .collect();
            let next = offset + entries.len();
            json!({"schemaVersion":"layrs.projection-page.v1","principalId":self.base.principal_id,"spaceId":self.base.space_id,"stepId":self.step.id(),"rootTreeId":self.step.root_tree_id(),"policySetId":self.base.current_policy_set_id,"authorizationEpoch":self.base.authorization_epoch,"offset":offset,"nextCursor":if next < self.namespace.len() { Some(next.to_string()) } else { None },"entries":entries})
        } else if route.ends_with("/projection/access") {
            let request: ProjectionAccessRequest = serde_json::from_value(body).unwrap();
            request.validate().unwrap();
            let entries: Vec<_> = request.paths.iter().map(|p| {
                let entry = self.namespace.iter().find(|e| &e.path == p);
                json!({"path":p,"target":entry.map(|e| &e.target),"executable":entry.is_some_and(|e| e.executable),"contentReadable":!self.denied.contains(p),"writable":!self.denied.contains(p) && !self.readonly.contains(p)})
            }).collect();
            if let Some(control) = &self.cancel_on_access {
                control.request_cancel();
            }
            json!({"schemaVersion":"layrs.projection-access.v1","principalId":self.base.principal_id,"spaceId":self.base.space_id,"stepId":self.step.id(),"rootTreeId":self.step.root_tree_id(),"policySetId":self.base.current_policy_set_id,"authorizationEpoch":self.base.authorization_epoch,"entries":entries})
        } else if route.ends_with("/fetch-plans") {
            let request: FetchPlanRequest = serde_json::from_value(body).unwrap();
            let plan_id = PlanId::parse(format!("plan:{occurrence}")).unwrap();
            let expiry = Timestamp::parse(if self.expire_manifest_once && occurrence == 1 {
                "2000-01-01T00:00:00Z"
            } else {
                "2999-01-01T00:00:00Z"
            })
            .unwrap();
            let FetchClaim::File {
                file_id,
                repo_path,
                root,
                ..
            } = &request.claims[0]
            else {
                panic!("collector fetched non-file closure");
            };
            assert!(!self.denied.contains(repo_path));
            assert_eq!(
                root,
                &AuthorizationRoot::Step {
                    step_id: self.step.id().clone()
                }
            );
            let manifest = &self.manifests[file_id];
            let descriptor = self.descriptor(&ObjectId::parse(manifest.id().as_str()).unwrap());
            let chunks: Vec<_> = manifest
                .chunks()
                .iter()
                .map(|c| self.descriptor(&ObjectId::parse(c.chunk_id().as_str()).unwrap()))
                .collect();
            let selection = FileManifestSelection {
                file_id: file_id.clone(),
                file_manifest_id: manifest.id().clone(),
                manifest_descriptor: descriptor.clone(),
                chunk_descriptors: chunks.clone(),
            };
            let epoch = self.base.authorization_epoch
                + u64::from(self.drift_whole_plan || (self.drift_renewal_epoch && occurrence > 1));
            let mut caps = Vec::new();
            for descriptor in std::iter::once(descriptor).chain(chunks) {
                let token = format!("private-download-capability-token-{}", self.caps.len());
                let cap = DownloadCapability {
                    principal_id: self.base.principal_id.clone(),
                    authorization_epoch: epoch,
                    space_id: request.space_id.clone(),
                    client_id: request.client_id.clone(),
                    request_id: request.request_id.clone(),
                    plan_id: plan_id.clone(),
                    claim: request.claims[0].clone(),
                    descriptor,
                    claim_digest: ObjectId::parse(format!(
                        "blake3:{}",
                        blake3::hash(
                            &[
                                b"opaque-fixture-claim-domain\0".as_slice(),
                                canonical_json(&request.claims[0]).unwrap().as_slice(),
                            ]
                            .concat()
                        )
                    ))
                    .unwrap(),
                    method: DownloadMethod::Get,
                    download_url: format!("{origin}v3/transfers/downloads"),
                    token: token.clone(),
                    expires_at: expiry.clone(),
                    resume_token: "private-resume-token".into(),
                    accept_ranges: false,
                };
                self.caps.insert(token, cap.clone());
                caps.push(cap);
            }
            serde_json::to_value(FetchPlanResponse {
                protocol: SyncV3Protocol::V3,
                message_type: FetchPlanResponseType::Response,
                principal_id: self.base.principal_id.clone(),
                authorization_epoch: epoch,
                space_id: request.space_id,
                client_id: request.client_id,
                request_id: request.request_id,
                plan_id,
                file_manifest_selections: vec![selection],
                download_capabilities: caps,
                denied_claims: vec![],
                next_page_cursor: Nullable::null(),
                has_more: false,
                expires_at: expiry,
            })
            .unwrap()
        } else if route == "/v3/transfers/downloads" {
            let cap = &self.caps[bearer.unwrap().strip_prefix("Bearer ").unwrap()];
            self.gets.push(cap.descriptor.kind);
            if self.expire_manifest_once && !self.expired {
                self.expired = true;
                return (
                    "404 Not Found",
                    serde_json::to_vec(&json!({"error":{"code":"transfer_unavailable"}})).unwrap(),
                );
            }
            let mut bytes = encode(&self.objects[&cap.descriptor.object_id]).unwrap().1;
            if self.substitute_manifest {
                let CanonicalObject::FileManifest(manifest) =
                    &self.objects[&cap.descriptor.object_id]
                else {
                    panic!("unexpected chunk download");
                };
                let replacement = Chunk::new(vec![b'x'; manifest.byte_length() as usize]).unwrap();
                let replacement =
                    FileManifest::from_chunks(ChunkingDescriptor::single(), &[replacement])
                        .unwrap();
                bytes = canonical_json(&replacement).unwrap();
            }
            if self.corrupt_manifest {
                bytes[0] ^= 1;
            }
            if self.drift_after_manifest {
                self.base.authorization_epoch += 1;
            }
            return ("200 OK", bytes);
        } else {
            panic!("unexpected route: {route}");
        };
        for fault in &self.faults {
            if route.ends_with(fault.route) && occurrence == fault.occurrence {
                *value
                    .pointer_mut(fault.pointer)
                    .expect("fixture mutation pointer") = fault.value.clone();
            }
        }
        if self.validate_faulted_renewal && route.ends_with("/fetch-plans") && occurrence == 2 {
            let coherent: FetchPlanResponse = serde_json::from_value(value.clone()).unwrap();
            coherent.validate_v3().unwrap();
            validate_fetch_descriptors(&coherent, &coherent.download_capabilities[0].claim)
                .unwrap();
            self.validated_renewals += 1;
        }
        ("200 OK", serde_json::to_vec(&value).unwrap())
    }
}

#[test]
fn both_strategies_collect_complete_metadata_and_revalidate_without_chunks() {
    for strategy in [
        ProviderTransitionObservationStrategy::PreservedEpoch,
        ProviderTransitionObservationStrategy::ReobservedExpandedScope,
    ] {
        let f = Fixture::new();
        let old = f.original();
        if strategy == ProviderTransitionObservationStrategy::ReobservedExpandedScope {
            f.server.lock().unwrap().base.authorization_epoch += 1;
        }
        let scope = f
            .observe(
                &old,
                strategy,
                &selected(&["a.txt", "b.txt"]),
                &[],
                &[],
                None,
            )
            .unwrap();
        assert_eq!(scope.namespace().len(), 3);
        assert_eq!(scope.access().len(), 3);
        assert_eq!(scope.descriptors().len(), 2);
        assert_eq!(scope.original_snapshot(), &old);
        assert_eq!(scope.source_anchor(), &anchor());
        assert!(scope
            .snapshot()
            .entries
            .iter()
            .all(|e| e.state != EntryState::Materialized));
        f.remote
            .revalidate_provider_transition_scope(&scope)
            .unwrap();
        let s = f.server.lock().unwrap();
        let routes: Vec<_> = s
            .events
            .iter()
            .map(|(p, _)| p.rsplit('/').next().unwrap())
            .collect();
        assert_eq!(
            routes,
            [
                "bootstrap",
                "projection",
                "projection",
                "projection",
                "whoami",
                "access",
                "fetch-plans",
                "downloads",
                "fetch-plans",
                "downloads",
                "bootstrap",
                "bootstrap",
                "projection",
                "bootstrap"
            ]
        );
        assert_eq!(
            s.events
                .iter()
                .filter(|(p, _)| p.ends_with("/projection/access"))
                .count(),
            1
        );
        assert_eq!(
            s.events
                .iter()
                .filter(|(p, _)| p.ends_with("/fetch-plans"))
                .count(),
            2
        );
        assert_eq!(
            s.gets,
            vec![ObjectKind::FileManifest, ObjectKind::FileManifest]
        );
        let first_plan = s
            .events
            .iter()
            .position(|(p, _)| p.ends_with("/fetch-plans"))
            .unwrap();
        let last_access = s
            .events
            .iter()
            .rposition(|(p, _)| p.ends_with("/projection/access"))
            .unwrap();
        assert!(last_access < first_plan);
        drop(s);
        f.assert_no_chunks();
    }
}

#[test]
fn preserved_epoch_refuses_historical_mismatch_before_enumeration() {
    let f = Fixture::new();
    let old = f.original();
    f.server.lock().unwrap().base.authorization_epoch += 1;
    assert!(f
        .observe(
            &old,
            ProviderTransitionObservationStrategy::PreservedEpoch,
            &ProjectionSelection::All,
            &[],
            &[],
            None
        )
        .is_err());
    assert_eq!(f.server.lock().unwrap().events.len(), 1);
}

#[test]
fn page_and_access_drift_in_every_binding_field_is_rejected() {
    for (edge, occurrence) in [("/projection", 2), ("/projection/access", 1)] {
        for (pointer, value) in [
            ("/principalId", json!("principal:other")),
            ("/spaceId", json!("space:other")),
            ("/stepId", json!(id(91))),
            ("/rootTreeId", json!(id(92))),
            ("/policySetId", json!(id(93))),
            ("/authorizationEpoch", json!(2)),
        ] {
            let f = Fixture::new();
            let old = f.original();
            f.fault(edge, occurrence, pointer, value);
            assert!(
                f.observe(
                    &old,
                    ProviderTransitionObservationStrategy::PreservedEpoch,
                    &ProjectionSelection::All,
                    &[],
                    &[],
                    None
                )
                .is_err(),
                "{edge} {pointer}"
            );
            f.assert_no_chunks();
        }
    }
}

#[test]
fn bootstrap_principal_space_and_final_fence_drift_are_rejected() {
    for occurrence in [1, 2] {
        for (pointer, value) in [
            ("/principalId", json!("principal:other")),
            ("/spaceId", json!("space:other")),
            ("/authorizationEpoch", json!(2)),
            ("/currentPolicySetId", json!(id(99))),
        ] {
            let f = Fixture::new();
            let old = f.original();
            f.fault("/bootstrap", occurrence, pointer, value);
            assert!(f
                .observe(
                    &old,
                    ProviderTransitionObservationStrategy::PreservedEpoch,
                    &ProjectionSelection::All,
                    &[],
                    &[],
                    None
                )
                .is_err());
            f.assert_no_chunks();
        }
    }
}

#[test]
fn duplicate_paths_nonadvance_and_cursor_cycles_are_rejected() {
    for (occurrence, pointer, value) in [
        (2, "/entries/0/path", json!("a.txt")),
        (2, "/offset", json!(0)),
        (2, "/nextCursor", json!("1")),
        (3, "/nextCursor", json!("1")),
    ] {
        let f = Fixture::new();
        let old = f.original();
        f.fault("/projection", occurrence, pointer, value);
        assert!(f
            .observe(
                &old,
                ProviderTransitionObservationStrategy::PreservedEpoch,
                &ProjectionSelection::All,
                &[],
                &[],
                None
            )
            .is_err());
        assert!(f.server.lock().unwrap().gets.is_empty());
    }
}

#[test]
fn missing_structural_parent_and_missing_original_scope_are_rejected() {
    for orphan in [false, true] {
        let f = Fixture::new();
        let old = f.original();
        if orphan {
            f.server.lock().unwrap().namespace[2].path = path("missing/c.txt");
        } else {
            f.server.lock().unwrap().namespace.remove(0);
        }
        assert!(f
            .observe(
                &old,
                ProviderTransitionObservationStrategy::ReobservedExpandedScope,
                &ProjectionSelection::All,
                &[],
                &[],
                None
            )
            .is_err());
        assert!(f.server.lock().unwrap().gets.is_empty());
    }
}

#[test]
fn cached_old_file_is_reclassified_revoked_or_readonly_at_each_exact_path() {
    for revoked in [false, true] {
        let f = Fixture::new();
        let old = f.original();
        let cached = {
            let mut s = f.server.lock().unwrap();
            s.base.authorization_epoch += 1;
            if revoked {
                s.denied.insert(path("a.txt"));
            } else {
                s.readonly.insert(path("a.txt"));
            }
            vec![ProviderTransitionDescriptor {
                path: path("a.txt"),
                manifest: s.manifests.values().next().unwrap().clone(),
            }]
        };
        let scope = f
            .observe(
                &old,
                ProviderTransitionObservationStrategy::ReobservedExpandedScope,
                &selected(&["a.txt"]),
                &cached,
                &[],
                None,
            )
            .unwrap();
        assert!(!scope.snapshot().entries[0].writable);
        assert_eq!(
            scope.snapshot().entries[0].state,
            if revoked {
                EntryState::Reserved
            } else {
                EntryState::Unavailable
            }
        );
        assert_eq!(scope.access()[0].content_readable, !revoked);
        assert_eq!(scope.historical_descriptors(), cached);
        assert_eq!(scope.descriptors().len(), usize::from(!revoked));
        let s = f.server.lock().unwrap();
        assert!(s
            .events
            .iter()
            .any(|(p, _)| p.ends_with("/projection/access")));
        assert_eq!(
            s.events
                .iter()
                .filter(|(p, _)| p.ends_with("/fetch-plans"))
                .count(),
            usize::from(!revoked)
        );
        assert!(s.gets.is_empty());
        drop(s);
        f.assert_no_chunks();
    }
}

#[test]
fn readable_absence_and_denied_absence_never_share_absent_or_write_grants() {
    let f = Fixture::new();
    let old = f.original();
    f.server.lock().unwrap().denied.insert(path("denied.txt"));
    let absence = [path("denied.txt"), path("future.txt")];
    let scope = f
        .observe(
            &old,
            ProviderTransitionObservationStrategy::ReobservedExpandedScope,
            &selected(&["a.txt", "denied.txt", "future.txt"]),
            &[],
            &absence,
            None,
        )
        .unwrap();
    assert_eq!(scope.denied_absence_paths(), &[path("denied.txt")]);
    assert!(!scope
        .snapshot()
        .entries
        .iter()
        .any(|e| e.path == path("denied.txt")));
    let readable = scope
        .snapshot()
        .entries
        .iter()
        .find(|e| e.path == path("future.txt"))
        .unwrap();
    assert_eq!(readable.state, EntryState::Absent);
    assert!(readable.writable);
    let denied = scope
        .access()
        .iter()
        .find(|e| e.path == path("denied.txt"))
        .unwrap();
    assert!(!denied.content_readable && !denied.writable && denied.target.is_none());
    f.assert_no_chunks();
}

#[test]
fn metadata_plan_and_capability_epoch_or_manifest_substitution_are_rejected() {
    for case in 0..8 {
        let f = Fixture::new();
        let old = f.original();
        match case {
            0 => f.server.lock().unwrap().drift_whole_plan = true,
            1 => f.fault(
                "/fetch-plans",
                1,
                "/downloadCapabilities/0/authorizationEpoch",
                json!(2),
            ),
            2 => f.fault("/fetch-plans", 1, "/principalId", json!("principal:other")),
            3 => f.fault(
                "/fetch-plans",
                1,
                "/fileManifestSelections/0/fileManifestId",
                json!(id(99)),
            ),
            4 => f.fault(
                "/fetch-plans",
                1,
                "/downloadCapabilities/0/claim/repoPath",
                json!("b.txt"),
            ),
            5 => f.server.lock().unwrap().corrupt_manifest = true,
            6 => f.fault(
                "/fetch-plans",
                1,
                "/downloadCapabilities/0/claimDigest",
                json!(id(99)),
            ),
            _ => f.server.lock().unwrap().substitute_manifest = true,
        }
        assert!(
            f.observe(
                &old,
                ProviderTransitionObservationStrategy::PreservedEpoch,
                &selected(&["a.txt"]),
                &[],
                &[],
                None
            )
            .is_err(),
            "case {case}"
        );
        f.assert_no_chunks();
    }
}

#[test]
fn renewal_keeps_exact_observation_and_refuses_changed_epoch_or_descriptor() {
    for case in 0..8 {
        let f = Fixture::new();
        let old = f.original();
        f.server.lock().unwrap().expire_manifest_once = true;
        match case {
            1 => f.fault("/fetch-plans", 2, "/authorizationEpoch", json!(2)),
            2 => f.fault(
                "/fetch-plans",
                2,
                "/fileManifestSelections/0/manifestDescriptor/canonicalByteLen",
                json!(1),
            ),
            3 => f.fault(
                "/fetch-plans",
                2,
                "/downloadCapabilities/0/authorizationEpoch",
                json!(2),
            ),
            4 => f.server.lock().unwrap().drift_renewal_epoch = true,
            5 => {
                f.server.lock().unwrap().validate_faulted_renewal = true;
                for pointer in [
                    "/fileManifestSelections/0/manifestDescriptor/canonicalByteLen",
                    "/fileManifestSelections/0/manifestDescriptor/encodedByteLen",
                    "/downloadCapabilities/0/descriptor/canonicalByteLen",
                    "/downloadCapabilities/0/descriptor/encodedByteLen",
                ] {
                    f.fault("/fetch-plans", 2, pointer, json!(1));
                }
            }
            6 => {
                f.server.lock().unwrap().validate_faulted_renewal = true;
                for pointer in [
                    "/downloadCapabilities/0/claimDigest",
                    "/downloadCapabilities/1/claimDigest",
                ] {
                    f.fault("/fetch-plans", 2, pointer, json!(id(99)));
                }
            }
            7 => {
                f.server.lock().unwrap().validate_faulted_renewal = true;
                f.fault(
                    "/fetch-plans",
                    2,
                    "/downloadCapabilities/0/acceptRanges",
                    json!(true),
                );
            }
            _ => (),
        }
        let result = f.observe(
            &old,
            ProviderTransitionObservationStrategy::PreservedEpoch,
            &selected(&["a.txt"]),
            &[],
            &[],
            None,
        );
        assert_eq!(result.is_ok(), case == 0);
        assert_eq!(
            f.server
                .lock()
                .unwrap()
                .events
                .iter()
                .filter(|(p, _)| p.ends_with("/fetch-plans"))
                .count(),
            2
        );
        assert_eq!(
            f.server.lock().unwrap().validated_renewals,
            usize::from(case >= 5)
        );
        f.assert_no_chunks();
    }
}

#[test]
fn manifest_acquisition_then_epoch_drift_cannot_return_scope() {
    let f = Fixture::new();
    let old = f.original();
    f.server.lock().unwrap().drift_after_manifest = true;
    assert!(f
        .observe(
            &old,
            ProviderTransitionObservationStrategy::PreservedEpoch,
            &selected(&["a.txt"]),
            &[],
            &[],
            None
        )
        .is_err());
    f.assert_no_chunks();
}

#[test]
fn revalidation_checks_origin_principal_space_step_root_and_epoch() {
    for (pointer, value) in [
        ("/principalId", json!("principal:other")),
        ("/spaceId", json!("space:other")),
        ("/stepId", json!(id(98))),
        ("/rootTreeId", json!(id(99))),
        ("/authorizationEpoch", json!(2)),
    ] {
        let f = Fixture::new();
        let old = f.original();
        let scope = f
            .observe(
                &old,
                ProviderTransitionObservationStrategy::PreservedEpoch,
                &selected(&["a.txt"]),
                &[],
                &[],
                None,
            )
            .unwrap();
        f.fault("/projection", 4, pointer, value);
        assert!(f
            .remote
            .revalidate_provider_transition_scope(&scope)
            .is_err());
        f.assert_no_chunks();
    }
    let f = Fixture::new();
    let old = f.original();
    let scope = f
        .observe(
            &old,
            ProviderTransitionObservationStrategy::PreservedEpoch,
            &selected(&["a.txt"]),
            &[],
            &[],
            None,
        )
        .unwrap();
    let other = NativeRemote::new("https://other.invalid", "unused").unwrap();
    assert!(other.revalidate_provider_transition_scope(&scope).is_err());
}

#[test]
fn cancellation_before_network_and_after_access_uses_transfer_control() {
    for early in [false, true] {
        let f = Fixture::new();
        let old = f.original();
        let control = NativeTransferControl::new();
        if early {
            control.request_cancel();
        } else {
            f.server.lock().unwrap().cancel_on_access = Some(control.clone());
        }
        assert!(matches!(
            f.observe(
                &old,
                ProviderTransitionObservationStrategy::PreservedEpoch,
                &ProjectionSelection::All,
                &[],
                &[],
                Some(&control)
            ),
            Err(NativeRemoteError::Cancelled)
        ));
        if early {
            assert!(f.server.lock().unwrap().events.is_empty());
        }
        assert!(f.server.lock().unwrap().gets.is_empty());
    }
}

#[test]
fn input_identity_selection_and_absence_bounds_fail_before_http() {
    let f = Fixture::new();
    let old = f.original();
    let space = SpaceId::parse("space:demo").unwrap();
    let principal = PrincipalId::parse("principal:alexa").unwrap();
    let original_selection = selected(&["a.txt"]);
    let shrink = selected(&[]);
    let good_anchor = anchor();
    let mut bad_anchor = anchor();
    bad_anchor.capture_count = 1;
    // Sorted unique valid paths isolate the count bound from shape/duplicate refusals.
    let over_limit: Vec<_> = (0..=100_000).map(|i| path(&format!("z/{i:06}"))).collect();
    for case in 0..6 {
        let unnormalized = ProjectionSelection::Selected {
            paths: vec![path("b.txt"), path("a.txt")],
            subtrees: vec![],
        };
        let absence = [path("z"), path("a")];
        let request = ProviderTransitionObservationRequest {
            source: if case == 0 { &bad_anchor } else { &good_anchor },
            origin: if case == 1 {
                "https://other.invalid"
            } else {
                f.remote.origin()
            },
            space_id: &space,
            principal_id: &principal,
            original: &old,
            original_selection: &original_selection,
            expanded_selection: if case == 2 {
                &shrink
            } else if case == 3 {
                &unnormalized
            } else {
                &ProjectionSelection::All
            },
            explicit_absence_paths: if case == 4 {
                &absence
            } else if case == 5 {
                &over_limit
            } else {
                &[]
            },
            historical_descriptors: &[],
            strategy: ProviderTransitionObservationStrategy::PreservedEpoch,
            control: None,
        };
        assert!(f.remote.observe_provider_transition(&request).is_err());
    }
    assert!(f.server.lock().unwrap().events.is_empty());
}

#[test]
fn exact_rights_are_sorted_unique_in_200_path_batches_and_late_drift_fails() {
    for drift in [false, true] {
        let f = Fixture::new();
        let old = f.original();
        {
            let mut s = f.server.lock().unwrap();
            let template = s.namespace[0].clone();
            for i in 0..400 {
                let mut e = template.clone();
                e.path = path(&format!("x{i:03}.txt"));
                s.namespace.push(e);
            }
            s.page_size = 200;
        }
        if drift {
            f.fault("/projection/access", 2, "/authorizationEpoch", json!(2));
        }
        let result = f.observe(
            &old,
            ProviderTransitionObservationStrategy::PreservedEpoch,
            &selected(&["a.txt"]),
            &[],
            &[],
            None,
        );
        assert_eq!(result.is_ok(), !drift);
        let s = f.server.lock().unwrap();
        let requests: Vec<ProjectionAccessRequest> = s
            .events
            .iter()
            .filter(|(p, _)| p.ends_with("/projection/access"))
            .map(|(_, body)| serde_json::from_value(body.clone()).unwrap())
            .collect();
        assert_eq!(
            requests.iter().map(|r| r.paths.len()).collect::<Vec<_>>(),
            if drift {
                vec![200, 200]
            } else {
                vec![200, 200, 3]
            }
        );
        let paths: Vec<_> = requests.iter().flat_map(|r| r.paths.iter()).collect();
        assert!(paths.windows(2).all(|p| p[0] < p[1]));
        if drift {
            assert!(s.gets.is_empty());
        }
        drop(s);
        f.assert_no_chunks();
    }
}

#[test]
fn directory_grant_does_not_grant_child_and_original_protected_frontier_survives() {
    for protected in [false, true] {
        let f = Fixture::new();
        {
            let mut s = f.server.lock().unwrap();
            let mut child = s.namespace[0].clone();
            child.path = path("dir/child.txt");
            s.namespace.push(ProjectionEntry {
                path: path("dir"),
                target: TreeEntryTarget::tree(TreeId::parse(id(88).as_str()).unwrap()),
                executable: false,
                content_readable: true,
                writable: true,
            });
            s.namespace.push(child);
            if !protected {
                s.denied.insert(path("dir/child.txt"));
            }
        }
        let mut old = f.original();
        if protected {
            old.entries.retain(|e| e.path != path("dir/child.txt"));
            let directory = old
                .entries
                .iter_mut()
                .find(|e| e.path == path("dir"))
                .unwrap();
            directory.state = EntryState::Reserved;
            directory.writable = false;
        } else {
            let child = old
                .entries
                .iter_mut()
                .find(|e| e.path == path("dir/child.txt"))
                .unwrap();
            child.state = EntryState::Reserved;
            child.writable = false;
        }
        let scope = f
            .observe(
                &old,
                ProviderTransitionObservationStrategy::ReobservedExpandedScope,
                &ProjectionSelection::All,
                &[],
                &[],
                None,
            )
            .unwrap();
        let child = scope
            .access()
            .iter()
            .find(|e| e.path == path("dir/child.txt"))
            .unwrap();
        assert_eq!(child.content_readable, protected);
        assert!(!scope
            .descriptors()
            .iter()
            .any(|d| d.path == path("dir/child.txt")));
        let directory = scope
            .snapshot()
            .entries
            .iter()
            .find(|e| e.path == path("dir"))
            .unwrap();
        assert_eq!(
            directory.state,
            if protected {
                EntryState::Reserved
            } else {
                EntryState::Unavailable
            }
        );
        if protected {
            assert_eq!(scope.protected_frontiers(), &[path("dir")]);
            assert!(!scope
                .snapshot()
                .entries
                .iter()
                .any(|e| e.path == path("dir/child.txt")));
        } else {
            assert!(directory.writable);
            let child = scope
                .snapshot()
                .entries
                .iter()
                .find(|e| e.path == path("dir/child.txt"))
                .unwrap();
            assert_eq!(child.state, EntryState::Reserved);
            assert!(!child.writable);
        }
        f.assert_no_chunks();
    }
}

#[test]
fn reobserved_policy_history_requires_forward_epoch_and_preserved_requires_exact_policy() {
    let f = Fixture::new();
    let mut old = f.original();
    old.observed_policy_set_id = id(98);
    assert!(f
        .observe(
            &old,
            ProviderTransitionObservationStrategy::PreservedEpoch,
            &selected(&["a.txt"]),
            &[],
            &[],
            None
        )
        .is_err());
    f.server.lock().unwrap().base.authorization_epoch = 2;
    let scope = f
        .observe(
            &old,
            ProviderTransitionObservationStrategy::ReobservedExpandedScope,
            &selected(&["a.txt"]),
            &[],
            &[],
            None,
        )
        .unwrap();
    assert_eq!(scope.original_snapshot().observed_policy_set_id, id(98));
    assert_ne!(scope.policy_set_id(), &id(98));
    assert_eq!(scope.authorization_epoch(), 2);
    old.observed_authorization_epoch = 3;
    assert!(f
        .observe(
            &old,
            ProviderTransitionObservationStrategy::ReobservedExpandedScope,
            &selected(&["a.txt"]),
            &[],
            &[],
            None
        )
        .is_err());
    f.assert_no_chunks();
}

#[test]
fn denied_absence_never_masquerades_as_previously_readable_absence() {
    let f = Fixture::new();
    let mut old = f.original();
    old.entries.push(LocalEntry {
        path: path("missing.txt"),
        target: None,
        executable: false,
        writable: true,
        state: EntryState::Absent,
    });
    let s = f.server.lock().unwrap();
    let space = s.base.space_id.clone();
    let principal = s.base.principal_id.clone();
    drop(s);
    f.server.lock().unwrap().base.authorization_epoch = 2;
    f.server.lock().unwrap().denied.insert(path("missing.txt"));
    let selected = selected(&["a.txt", "missing.txt"]);
    let scope = f
        .remote
        .observe_provider_transition(&ProviderTransitionObservationRequest {
            source: &anchor(),
            origin: f.remote.origin(),
            space_id: &space,
            principal_id: &principal,
            original: &old,
            original_selection: &selected,
            expanded_selection: &selected,
            explicit_absence_paths: &[path("missing.txt")],
            historical_descriptors: &[],
            strategy: ProviderTransitionObservationStrategy::ReobservedExpandedScope,
            control: None,
        })
        .unwrap();
    assert_eq!(scope.denied_absence_paths(), &[path("missing.txt")]);
    assert!(!scope
        .snapshot()
        .entries
        .iter()
        .any(|e| e.path == path("missing.txt")));
    assert_eq!(
        scope.original_snapshot().entries.last().unwrap().state,
        EntryState::Absent
    );
    f.assert_no_chunks();
}

#[test]
fn explicit_absence_that_is_present_and_page_access_rights_drift_are_rejected() {
    for case in 0..3 {
        let f = Fixture::new();
        let old = f.original();
        let absent = [path("a.txt")];
        match case {
            1 => f.fault("/projection/access", 1, "/entries/0/writable", json!(false)),
            2 => {
                f.fault(
                    "/projection/access",
                    1,
                    "/entries/0/contentReadable",
                    json!(false),
                );
                f.fault("/projection/access", 1, "/entries/0/writable", json!(false));
            }
            _ => (),
        }
        assert!(f
            .observe(
                &old,
                ProviderTransitionObservationStrategy::ReobservedExpandedScope,
                &ProjectionSelection::All,
                &[],
                if case == 0 { &absent } else { &[] },
                None
            )
            .is_err());
        assert!(f.server.lock().unwrap().gets.is_empty());
    }
}

#[test]
fn cached_metadata_still_requires_fresh_plan_epoch() {
    let f = Fixture::new();
    let old = f.original();
    let cached = {
        let mut s = f.server.lock().unwrap();
        s.drift_whole_plan = true;
        vec![ProviderTransitionDescriptor {
            path: path("a.txt"),
            manifest: s.manifests.values().next().unwrap().clone(),
        }]
    };
    assert!(f
        .observe(
            &old,
            ProviderTransitionObservationStrategy::PreservedEpoch,
            &selected(&["a.txt"]),
            &cached,
            &[],
            None
        )
        .is_err());
    let s = f.server.lock().unwrap();
    assert!(s
        .events
        .iter()
        .any(|(p, _)| p.ends_with("/projection/access")));
    assert!(s.events.iter().any(|(p, _)| p.ends_with("/fetch-plans")));
    assert!(s.gets.is_empty());
}

#[test]
fn whoami_drift_platform_alias_and_denied_absence_write_are_rejected() {
    for case in 0..3 {
        let f = Fixture::new();
        let old = f.original();
        let absent = [path("missing.txt")];
        match case {
            0 => f.fault("/whoami", 1, "/principal_id", json!("principal:other")),
            1 => f.fault("/projection", 2, "/entries/0/path", json!("A.txt")),
            _ => {
                f.server.lock().unwrap().denied.insert(path("missing.txt"));
                f.fault("/projection/access", 1, "/entries/3/writable", json!(true));
            }
        }
        assert!(f
            .observe(
                &old,
                ProviderTransitionObservationStrategy::ReobservedExpandedScope,
                &ProjectionSelection::All,
                &[],
                &absent,
                None
            )
            .is_err());
        assert!(f.server.lock().unwrap().gets.is_empty());
    }
}
