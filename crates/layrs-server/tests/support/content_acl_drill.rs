//! Actual HTTPS content ACLs, including aliases and no local checkout mutation.
use layrs_client_core::native_scm::{NativeRemote, NativeTransferControl, NativeWorkspace};
use layrs_core::scm::{FileId, ObjectId, RepoPath, Step, StepId, Timestamp, Tree, TreeEntryTarget};
use layrs_sync::v3::*;
use serde_json::{Value, json};
use std::path::Path;
#[path = "path_patch_drill.rs"]
mod path_patch_drill;
#[path = "projection_drill.rs"]
mod projection_drill;

struct Http<'a> {
    client: &'a reqwest::blocking::Client,
    origin: &'a str,
    token: &'a str,
    space: layrs_core::scm::SpaceId,
}
impl Http<'_> {
    fn call(&self, method: &str, path: &str, body: Value) -> reqwest::blocking::Response {
        self.client
            .request(method.parse().unwrap(), format!("{}{path}", self.origin))
            .bearer_auth(self.token)
            .json(&body)
            .send()
            .unwrap()
    }
    fn json(&self, method: &str, path: &str, body: Value) -> Value {
        let response = self.call(method, path, body);
        let status = response.status();
        assert!(status.is_success(), "{method} {path}: {status}");
        response.json().unwrap()
    }
    fn plan(&self, claim: FetchClaim) -> FetchPlanResponse {
        let request = FetchPlanRequest {
            protocol: SyncV3Protocol::V3,
            message_type: FetchPlanRequestType::Request,
            space_id: self.space.clone(),
            client_id: ClientId::parse("client:acl-drill").unwrap(),
            request_id: RequestId::parse(format!("request:{}", uuid::Uuid::new_v4())).unwrap(),
            claims: vec![claim],
            page_cursor: Nullable::null(),
            max_objects: 10000,
        };
        serde_json::from_value(self.json(
            "POST",
            &format!("/v3/spaces/{}/fetch-plans", self.space),
            serde_json::to_value(request).unwrap(),
        ))
        .unwrap()
    }
    fn download(&self, cap: &DownloadCapability) -> reqwest::blocking::Response {
        self.client
            .get(&cap.download_url)
            .bearer_auth(&cap.token)
            .send()
            .unwrap()
    }
    fn object(&self, step: &StepId, id: &str, kind: ObjectKind) -> Vec<u8> {
        let plan = self.plan(object_claim(step, id, kind, None));
        assert!(plan.denied_claims.is_empty());
        let response = self.download(&plan.download_capabilities[0]);
        assert!(response.status().is_success());
        response.bytes().unwrap().to_vec()
    }
    fn file_id(&self, step: &StepId, path: &str) -> FileId {
        let s: Step =
            serde_json::from_slice(&self.object(step, step.as_str(), ObjectKind::Step)).unwrap();
        let mut tree = s.root_tree_id().clone();
        let parts: Vec<_> = path.split('/').collect();
        for (index, part) in parts.iter().enumerate() {
            let t: Tree =
                serde_json::from_slice(&self.object(step, tree.as_str(), ObjectKind::Tree))
                    .unwrap();
            let entry = t
                .entries()
                .iter()
                .find(|e| e.name().as_str() == *part)
                .unwrap();
            match entry.target() {
                TreeEntryTarget::Tree { tree_id } => tree = tree_id.clone(),
                TreeEntryTarget::File { file_id } if index + 1 == parts.len() => {
                    return file_id.clone();
                }
                _ => panic!("not a file"),
            }
        }
        panic!("missing file")
    }
    fn activate(
        &self,
        base: &SpaceBootstrap,
        principal: &PrincipalId,
        deny_public: bool,
    ) -> (PolicySet, ProductEnvelope, SyncTransactionReceipt) {
        let rules = json!({"schemaVersion":"layrs.space-policy-rules.v2","rules":[],"contentPolicies":[{"principalIds":[principal],"default":"deny","rules":[{"effect":"allow","selector":{"kind":"subtree","path":"public"}},{"effect":"deny","selector":{"kind":if deny_public {"subtree"} else {"exact"},"path":if deny_public {"public"} else {"public/blocked.txt"}}}]}]});
        let bytes = serde_jcs::to_vec(&rules).unwrap();
        let mut hash = blake3::Hasher::new();
        hash.update(b"layrs-evidence-v1\0");
        hash.update(&bytes);
        let evidence = ObjectId::parse(format!("blake3:{}", hash.finalize())).unwrap();
        let request = ObjectPlanRequest {
            protocol: SyncV3Protocol::V3,
            message_type: ObjectPlanRequestType::Request,
            space_id: self.space.clone(),
            client_id: ClientId::parse("client:acl-admin").unwrap(),
            request_id: RequestId::parse(format!("request:{}", uuid::Uuid::new_v4())).unwrap(),
            objects: vec![ObjectDescriptor {
                kind: ObjectKind::Evidence,
                object_id: evidence.clone(),
                canonical_byte_len: bytes.len() as u64,
                encoded_byte_len: bytes.len() as u64,
                compression: Compression::Identity,
            }],
        };
        let plan: ObjectPlanResponse = serde_json::from_value(self.json(
            "POST",
            &format!("/v3/spaces/{}/object-plans", self.space),
            serde_json::to_value(request).unwrap(),
        ))
        .unwrap();
        if let Some(cap) = plan.upload_capabilities.first() {
            assert!(
                self.client
                    .put(&cap.upload_url)
                    .bearer_auth(&cap.token)
                    .body(bytes)
                    .send()
                    .unwrap()
                    .status()
                    .is_success()
            );
        }
        let suffix = uuid::Uuid::new_v4();
        let policy_id = PolicyId::parse(format!("policy:{suffix}")).unwrap();
        let version = PolicyVersionId::parse(format!("policy-version:{suffix}")).unwrap();
        let mut product = ProductEnvelope::Policy {
            product_id: policy_id.clone(),
            product_object_id: evidence.clone(),
            depends_on_step_ids: vec![],
            depends_on_product_ids: vec![],
            previous_product_object_id: Nullable::null(),
            record: PolicyRecord {
                schema_version: PolicySchemaVersion::V1,
                policy_id: policy_id.clone(),
                version_id: version.clone(),
                space_id: self.space.clone(),
                rules_object_id: evidence.clone(),
                created_at: Timestamp::parse("2026-09-20T00:00:00Z").unwrap(),
            },
        };
        let object = product.computed_object_id().unwrap();
        if let ProductEnvelope::Policy {
            product_object_id, ..
        } = &mut product
        {
            *product_object_id = object.clone();
        }
        let mut set = base.current_policy_set.record.clone();
        set.policy_versions.push(PolicyVersionEntry {
            policy_id,
            policy_version_id: version,
            policy_product_object_id: object,
            dependency_object_ids: vec![evidence],
        });
        set.policy_versions
            .sort_by(|a, b| a.policy_id.cmp(&b.policy_id));
        set.policy_set_id = set.computed_id().unwrap();
        let mut request = SyncTransactionRequest {
            protocol: SyncV3Protocol::V3,
            message_type: TransactionRequestType::Request,
            space_id: self.space.clone(),
            client_id: ClientId::parse("client:acl-admin").unwrap(),
            idempotency_key: IdempotencyKey::parse(format!("acl-{suffix}")).unwrap(),
            request_digest: set.policy_set_id.clone(),
            known_event_cursor: None,
            steps: vec![],
            observations: vec![],
            products: vec![product.clone()],
            view_mutation: None,
            policy_root_mutation: Some(PolicyRootMutation {
                mutation_id: MutationId::parse(format!("mutation:{suffix}")).unwrap(),
                expected_policy_set_id: base.current_policy_set_id.clone(),
                expected_authorization_epoch: base.authorization_epoch,
                proposed_policy_set: ProposedPolicySet::Inline {
                    policy_set: set.clone(),
                },
            }),
        };
        request.request_digest = request.computed_request_digest().unwrap();
        let receipt: SyncTransactionReceipt = serde_json::from_value(self.json(
            "POST",
            &format!("/v3/spaces/{}/transactions", self.space),
            serde_json::to_value(request).unwrap(),
        ))
        .unwrap();
        assert_eq!(
            receipt.status,
            ReceiptStatus::Applied,
            "{:?}",
            receipt.rejections
        );
        (set, product, receipt)
    }
}
fn object_claim(step: &StepId, id: &str, kind: ObjectKind, path: Option<&str>) -> FetchClaim {
    FetchClaim::Object {
        object: ObjectReference {
            kind,
            object_id: ObjectId::parse(id).unwrap(),
        },
        root: AuthorizationRoot::Step {
            step_id: step.clone(),
        },
        repo_path: match path {
            Some(p) => Nullable::some(RepoPath::parse(p).unwrap()),
            None => Nullable::null(),
        },
    }
}
fn file_claim(step: &StepId, id: &FileId, path: &str) -> FetchClaim {
    FetchClaim::File {
        file_id: id.clone(),
        preferred_manifest_ids: vec![],
        root: AuthorizationRoot::Step {
            step_id: step.clone(),
        },
        repo_path: RepoPath::parse(path).unwrap(),
    }
}
fn denied(plan: FetchPlanResponse) {
    assert!(plan.download_capabilities.is_empty());
    assert_eq!(plan.denied_claims.len(), 1);
}

pub fn run(
    remote: &NativeRemote,
    root: &Path,
    origin: &str,
    token: &str,
    ca: &str,
    lose_response: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let aroot = root.join("content-acl-author");
    std::fs::create_dir_all(aroot.join("public")).unwrap();
    for path in ["public/allowed.txt", "private.txt", "public/blocked.txt"] {
        std::fs::write(
            aroot.join(path),
            b"same bytes with three distinct access paths",
        )
        .unwrap();
    }
    std::fs::write(aroot.join("secret.txt"), b"private unique payload").unwrap();
    std::fs::write(aroot.join("public/conflict.bin"), [0, 255, 0]).unwrap();
    let a = NativeWorkspace::initialize(&aroot, "Content ACL fixture").unwrap();
    let provision = remote.provision(&a, "content-acl", "acl").unwrap();
    assert_eq!(
        remote
            .publish(&a, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    let http = reqwest::blocking::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap();
    let admin = Http {
        client: &http,
        origin,
        token,
        space: provision.space_id.clone(),
    };
    let account=admin.json("POST","/v3/auth/accounts",json!({"login":"content-reader@example.com","password":"content reader long test password phrase","displayName":"Content Reader"}));
    let principal: PrincipalId = serde_json::from_value(account["principalId"].clone()).unwrap();
    admin.json(
        "POST",
        &format!("/v3/spaces/{}/members", provision.space_id),
        json!({"principalId":principal,"role":"member"}),
    );
    let session = NativeRemote::login(
        origin,
        "content-reader@example.com",
        "content reader long test password phrase",
        Some(ca.as_bytes()),
    )
    .unwrap();
    let reader =
        NativeRemote::with_trusted_ca(origin, &session.access_token, ca.as_bytes()).unwrap();
    let read = Http {
        client: &http,
        origin,
        token: &session.access_token,
        space: provision.space_id.clone(),
    };
    let broot = root.join("content-acl-reader");
    std::fs::create_dir(&broot).unwrap();
    let b = reader
        .clone_into(&broot, &provision.space_id, "Cached complete checkout")
        .unwrap();
    let before = b.state().unwrap();
    let journal = std::fs::read(b.store_root().join("native-remote.json")).unwrap();
    remote.receive(&a).unwrap(); // Observe the membership grant's new epoch before a new publication.
    // A real admitted structural Conflict under an otherwise allowed subtree.
    a.create_layer("acl-topic").unwrap();
    a.switch_layer("acl-topic").unwrap();
    std::fs::write(aroot.join("public/conflict.bin"), [0, 255, 1]).unwrap();
    a.capture().unwrap();
    let source = a
        .state()
        .unwrap()
        .view
        .worktrees()
        .iter()
        .find(|w| w.worktree_id() == a.worktree_id())
        .unwrap()
        .base_step_id()
        .unwrap()
        .clone();
    a.switch_layer("main").unwrap();
    std::fs::write(aroot.join("public/conflict.bin"), [0, 255, 2]).unwrap();
    a.capture().unwrap();
    let preview = a
        .preview_structural_merge(
            &[source],
            &provision.layer_id,
            layrs_weave::reconciliation::FileMergePolicy {
                unsupported: layrs_weave::reconciliation::FailureAction::StructuralFallback,
                quota: layrs_weave::reconciliation::FailureAction::Reject,
                limits: Default::default(),
            },
            Default::default(),
        )
        .unwrap();
    let merge = a.accept_merge(&preview).unwrap();
    assert_eq!(merge.conflict_ids.len(), 1);
    assert_eq!(
        remote
            .publish(&a, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    let bootstrap = remote.bootstrap(&provision.space_id).unwrap();
    assert!(
        serde_json::to_value(&bootstrap)
            .unwrap()
            .get("contentAccess")
            .is_none()
    );
    let step = merge.step_id.clone();
    let file = read.file_id(&step, "public/allowed.txt");
    assert_eq!(file, read.file_id(&step, "private.txt"));
    let old = read.plan(file_claim(&step, &file, "private.txt"));
    assert!(old.denied_claims.is_empty());
    let (set, product, receipt) = admin.activate(&bootstrap, &principal, false);
    let restricted = reader.bootstrap(&provision.space_id).unwrap();
    assert!(restricted.content_access.is_some());
    let projection_cursor = projection_drill::verify(&read, &reader, &restricted, &step);
    assert!(
        remote
            .bootstrap(&provision.space_id)
            .unwrap()
            .content_access
            .is_none()
    );
    // Old capabilities, including Range, cannot survive the changed epoch.
    assert!(
        !read
            .download(&old.download_capabilities[0])
            .status()
            .is_success()
    );
    assert!(
        !http
            .get(&old.download_capabilities[0].download_url)
            .bearer_auth(&old.download_capabilities[0].token)
            .header("Range", "bytes=0-1")
            .send()
            .unwrap()
            .status()
            .is_success()
    );
    let allowed = read.plan(file_claim(&step, &file, "public/allowed.txt"));
    assert!(allowed.denied_claims.is_empty());
    assert!(!allowed.download_capabilities.is_empty());
    for cap in &allowed.download_capabilities {
        assert!(read.download(cap).status().is_success());
    }
    let (read_id, read_bytes) = reader
        .read_remote_file(&b, &step, &RepoPath::parse("public/allowed.txt").unwrap())
        .unwrap();
    assert_eq!(read_id, file);
    assert_eq!(read_bytes, b"same bytes with three distinct access paths");
    assert!(
        reader
            .read_remote_file(&b, &step, &RepoPath::parse("private.txt").unwrap())
            .is_err()
    );
    let conflict = merge.conflict_ids.first().unwrap();
    let conflict_claim = object_claim(
        &step,
        conflict.as_str(),
        ObjectKind::Conflict,
        Some("public/conflict.bin"),
    );
    assert!(admin.plan(conflict_claim.clone()).denied_claims.is_empty());
    denied(read.plan(conflict_claim));
    for path in [
        "private.txt",
        "public/blocked.txt",
        "publicity/allowed.txt",
        "public/missing.txt",
    ] {
        denied(read.plan(file_claim(&step, &file, path)));
    }
    let secret = admin.file_id(&step, "secret.txt");
    denied(read.plan(file_claim(&step, &secret, "public/allowed.txt")));
    let secret_plan = admin.plan(file_claim(&step, &secret, "secret.txt"));
    for cap in &secret_plan.download_capabilities {
        denied(read.plan(object_claim(
            &step,
            cap.descriptor.object_id.as_str(),
            cap.descriptor.kind,
            Some("public/allowed.txt"),
        )));
    }
    // Product roots do not turn opaque dependencies into readable content.
    let root_product = ProductRoot::Policy {
        logical_id: match &product {
            ProductEnvelope::Policy { product_id, .. } => product_id.clone(),
            _ => unreachable!(),
        },
        object_id: product.object_id().clone(),
    };
    for (kind, id) in [
        (ObjectKind::Product, product.object_id().clone()),
        (
            ObjectKind::Evidence,
            match &product {
                ProductEnvelope::Policy { record, .. } => record.rules_object_id.clone(),
                _ => unreachable!(),
            },
        ),
    ] {
        denied(read.plan(FetchClaim::Object {
            object: ObjectReference {
                kind,
                object_id: id,
            },
            root: AuthorizationRoot::Product {
                product: root_product.clone(),
            },
            repo_path: Nullable::null(),
        }));
    }
    denied(read.plan(FetchClaim::AuthorizationSnapshot {
        authorization_snapshot_id: receipt.authorization_snapshot_id.clone(),
        purpose: AuthorizationSnapshotPurpose::Audit,
    }));
    let mut through_view = file_claim(&step, &file, "public/allowed.txt");
    if let FetchClaim::File { root, .. } = &mut through_view {
        *root = AuthorizationRoot::RepositoryView {
            repository_view_id: bootstrap.current_repository_view.repository_view_id.clone(),
        };
    }
    denied(read.plan(through_view));
    // Full APIs fail before any implicit capture, shortcut or new checkout metadata.
    assert_eq!(
        reader.receive(&b).unwrap_err().code(),
        "content_access_restricted"
    );
    assert_eq!(
        reader
            .receive_cancellable(&b, &NativeTransferControl::new())
            .unwrap_err()
            .code(),
        "content_access_restricted"
    );
    std::fs::write(
        broot.join("local-unsaved.txt"),
        b"must not be implicitly captured",
    )
    .unwrap();
    assert_eq!(
        reader.receive(&b).unwrap_err().code(),
        "content_access_restricted"
    );
    assert_eq!(b.state().unwrap().operation.id(), before.operation.id());
    assert_eq!(b.state().unwrap().view, before.view);
    assert_eq!(
        std::fs::read(b.store_root().join("native-remote.json")).unwrap(),
        journal
    );
    assert_eq!(
        std::fs::read(broot.join("private.txt")).unwrap(),
        b"same bytes with three distinct access paths"
    );
    let dest = root.join("content-acl-denied-clone");
    std::fs::create_dir(&dest).unwrap();
    assert_eq!(
        reader
            .clone_into(&dest, &provision.space_id, "Denied")
            .err()
            .unwrap()
            .code(),
        "content_access_restricted"
    );
    assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 0);
    assert_eq!(
        reader
            .fetch_step_files(&provision.space_id, &step)
            .unwrap_err()
            .code(),
        "content_access_restricted"
    );
    for path in [
        "reviews",
        "runs",
        "runs/run:not-present",
        "teams",
        "policy-management",
    ] {
        assert_eq!(
            read.call(
                "GET",
                &format!("/v3/spaces/{}/{path}", provision.space_id),
                Value::Null
            )
            .status()
            .as_u16(),
            403
        );
    }
    assert_eq!(
        read.call(
            "POST",
            &format!("/v3/spaces/{}/compare", provision.space_id),
            json!({"before":step,"after":step})
        )
        .status()
        .as_u16(),
        403
    );
    // Structural history is visible and canonical despite byte restrictions.
    assert!(
        read.call(
            "POST",
            &format!("/v3/spaces/{}/history", provision.space_id),
            json!({"anchor":step,"limit":10,"cursor":null})
        )
        .status()
        .is_success()
    );
    let mut write = SyncTransactionRequest {
        protocol: SyncV3Protocol::V3,
        message_type: TransactionRequestType::Request,
        space_id: provision.space_id.clone(),
        client_id: ClientId::parse("client:acl-reader").unwrap(),
        idempotency_key: IdempotencyKey::parse("acl-reader-policy-attempt").unwrap(),
        request_digest: set.policy_set_id.clone(),
        known_event_cursor: None,
        steps: vec![],
        observations: vec![],
        products: vec![],
        view_mutation: None,
        policy_root_mutation: Some(PolicyRootMutation {
            mutation_id: MutationId::parse("mutation:acl-reader-policy").unwrap(),
            expected_policy_set_id: set.policy_set_id.clone(),
            expected_authorization_epoch: restricted.authorization_epoch,
            proposed_policy_set: ProposedPolicySet::Stored {
                policy_set_id: bootstrap.current_policy_set_id.clone(),
            },
        }),
    };
    write.request_digest = write.computed_request_digest().unwrap();
    let refusal: SyncTransactionReceipt = serde_json::from_value(read.json(
        "POST",
        &format!("/v3/spaces/{}/transactions", provision.space_id),
        serde_json::to_value(&write).unwrap(),
    ))
    .unwrap();
    assert_eq!(refusal.status, ReceiptStatus::Rejected);
    let replay: SyncTransactionReceipt = serde_json::from_value(read.json(
        "POST",
        &format!("/v3/spaces/{}/transactions", provision.space_id),
        serde_json::to_value(&write).unwrap(),
    ))
    .unwrap();
    assert_eq!(refusal, replay);
    // Ordinary candidate admission was granted to this member under v1; content
    // restriction must close it even without an administrative mutation.
    let original: Step =
        serde_json::from_slice(&admin.object(&step, step.as_str(), ObjectKind::Step)).unwrap();
    let candidate = Step::new(vec![step.clone()], original.root_tree_id().clone()).unwrap();
    write.steps = vec![candidate.clone()];
    write.policy_root_mutation = None;
    write.idempotency_key = IdempotencyKey::parse("acl-reader-candidate-attempt").unwrap();
    write.request_digest = write.computed_request_digest().unwrap();
    let candidate_refusal: SyncTransactionReceipt = serde_json::from_value(read.json(
        "POST",
        &format!("/v3/spaces/{}/transactions", provision.space_id),
        serde_json::to_value(&write).unwrap(),
    ))
    .unwrap();
    assert_eq!(candidate_refusal.status, ReceiptStatus::Rejected);
    assert!(candidate_refusal.accepted_step_ids.is_empty());
    denied(admin.plan(object_claim(
        candidate.id(),
        candidate.id().as_str(),
        ObjectKind::Step,
        None,
    )));
    assert_eq!(
        remote
            .bootstrap(&provision.space_id)
            .unwrap()
            .current_policy_set_id,
        set.policy_set_id
    );
    let now = remote.bootstrap(&provision.space_id).unwrap();
    admin.activate(&now, &principal, true);
    projection_drill::stale(&read, projection_cursor);
    assert!(
        !read
            .download(&allowed.download_capabilities[0])
            .status()
            .is_success()
    );
    denied(read.plan(file_claim(&step, &file, "public/allowed.txt")));
    path_patch_drill::run(
        remote,
        &a,
        &reader,
        &read,
        &bootstrap,
        &step,
        &principal,
        &admin,
        lose_response,
    );
    let layer = bootstrap
        .layers
        .iter()
        .find(|l| l.head_step_ids.contains(&step))
        .unwrap()
        .record
        .id();
    projection_drill::selected_publication(remote, &admin, layer);
    eprintln!(
        "HTTPS ACL v2: exact/subtree deny, alias FileId, foreign content/root and opaque refusal, Range revocation, writes replay, full clone/receive/cache preserve local files and Operations PASS"
    );
}
