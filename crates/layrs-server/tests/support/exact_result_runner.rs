//! L7 exact-result fixtures against the disposable native HTTPS/Postgres server.
//! Only the combined-result success is a real container execution. Negative
//! controls deliberately submit passing fixture verdicts for other exact Steps.
use layrs_client_core::native_scm::{NativeRemote, NativeRemoteError};
use layrs_core::scm::{
    Chunk, ChunkingDescriptor, FileId, FileManifest, LayerHeadSet, LayerId, ObjectId, RepoPath,
    RepoPathComponent, RepositoryView, SpaceId, Step, StepId, Tree, TreeEntry, TreeEntryTarget,
};
use layrs_runner::{Job, Recipe};
use layrs_store_server::v3::ProvisionSpaceResponse;
use layrs_sync::v3::*;
use layrs_weave::{
    graph_merge::{GraphMergeLimits, MergeObjects, plan_structural_merge},
    reconciliation::{FailureAction, FileMergeLimits, FileMergePolicy},
};
use serde_json::{Value, json};
use std::path::Path;

struct Fixture<'a> {
    client: &'a reqwest::blocking::Client,
    origin: &'a str,
    admin: &'a str,
    space: SpaceId,
    layer: LayerId,
}

impl<'a> Fixture<'a> {
    fn new(
        client: &'a reqwest::blocking::Client,
        origin: &'a str,
        admin: &'a str,
        slug: &str,
    ) -> Self {
        let url = reqwest::Url::parse(origin).unwrap();
        assert_eq!(url.scheme(), "https");
        assert!(matches!(
            url.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]")
        ));
        // A separate Workspace prevents grants/protection from invalidating the
        // outer native-client and restricted-author fixtures' authorization.
        let provision: ProvisionSpaceResponse = serde_json::from_value(super::call(
            client, "POST", &format!("{origin}/v3/workspaces/provision"), admin,
            json!({"workspaceSlug":"l7-exact-result","spaceSlug":slug,"name":slug,"initialLayer":null}),
        )).unwrap();
        Self {
            client,
            origin,
            admin,
            space: provision.space_id,
            layer: provision.layer_id,
        }
    }

    fn url(&self, suffix: &str) -> String {
        format!("{}/v3/spaces/{}{}", self.origin, self.space, suffix)
    }

    fn call(&self, method: &str, suffix: &str, token: &str, body: Value) -> Value {
        super::call(self.client, method, &self.url(suffix), token, body)
    }

    fn bootstrap(&self) -> SpaceBootstrap {
        serde_json::from_value(self.call("GET", "/bootstrap", self.admin, Value::Null)).unwrap()
    }

    fn unchanged(&self, before: &SpaceBootstrap) {
        let after = self.bootstrap();
        assert_eq!(
            after.current_repository_view,
            before.current_repository_view
        );
        assert_eq!(
            after.current_repository_view_generation,
            before.current_repository_view_generation
        );
        assert_eq!(after.current_policy_set_id, before.current_policy_set_id);
        assert_eq!(after.authorization_epoch, before.authorization_epoch);
    }

    fn upload(&self, kind: ObjectKind, id: &str, bytes: Vec<u8>) {
        let plan: ObjectPlanResponse = serde_json::from_value(
            self.call(
                "POST",
                "/object-plans",
                self.admin,
                serde_json::to_value(ObjectPlanRequest {
                    protocol: SyncV3Protocol::V3,
                    message_type: ObjectPlanRequestType::Request,
                    space_id: self.space.clone(),
                    client_id: client_id(),
                    request_id: RequestId::parse(format!("request:{}", uuid::Uuid::new_v4()))
                        .unwrap(),
                    objects: vec![ObjectDescriptor {
                        kind,
                        object_id: ObjectId::parse(id).unwrap(),
                        canonical_byte_len: bytes.len() as u64,
                        encoded_byte_len: bytes.len() as u64,
                        compression: Compression::Identity,
                    }],
                })
                .unwrap(),
            ),
        )
        .unwrap();
        if plan.upload_capabilities.is_empty() {
            assert!(plan.missing_objects.is_empty());
            return;
        }
        assert_eq!(plan.upload_capabilities.len(), 1);
        let cap = &plan.upload_capabilities[0];
        let response = self
            .client
            .put(&cap.upload_url)
            .bearer_auth(&cap.token)
            .body(bytes)
            .send()
            .unwrap();
        assert!(
            response.status().is_success(),
            "object upload: {}",
            response.status()
        );
    }

    fn file(&self, bytes: &[u8]) -> FileId {
        let chunk = Chunk::new(bytes.to_vec()).unwrap();
        let manifest =
            FileManifest::from_chunks(ChunkingDescriptor::single(), std::slice::from_ref(&chunk))
                .unwrap();
        self.upload(ObjectKind::Chunk, chunk.id().as_str(), bytes.to_vec());
        self.upload(
            ObjectKind::FileManifest,
            manifest.id().as_str(),
            serde_jcs::to_vec(&manifest).unwrap(),
        );
        manifest.file_id().clone()
    }

    fn tree(&self, entries: Vec<TreeEntry>) -> Tree {
        let tree = Tree::new(entries).unwrap();
        self.upload(
            ObjectKind::Tree,
            tree.id().as_str(),
            serde_jcs::to_vec(&tree).unwrap(),
        );
        tree
    }

    fn transact(
        &self,
        before: &SpaceBootstrap,
        steps: Vec<Step>,
        head: Option<&Step>,
        key: &str,
    ) -> SyncTransactionReceipt {
        let current = &before.current_repository_view.record;
        let view_mutation = head.map(|step| RepositoryViewMutation {
            mutation_id: MutationId::parse(format!("mutation:{key}")).unwrap(),
            expected_current_view_id: before.current_repository_view.repository_view_id.clone(),
            expected_generation: before.current_repository_view_generation,
            proposed_view: ProposedRepositoryView::Inline {
                view: RepositoryView::new(
                    self.space.clone(),
                    current
                        .layer_heads()
                        .iter()
                        .map(|heads| {
                            if heads.layer_id() == &self.layer {
                                LayerHeadSet::new(self.layer.clone(), vec![step.id().clone()])
                                    .unwrap()
                            } else {
                                heads.clone()
                            }
                        })
                        .collect(),
                    current.marker_ids().to_vec(),
                    current.release_ids().to_vec(),
                    vec![],
                )
                .unwrap(),
            },
            layer_mutations: vec![LayerMutationIntent {
                layer_id: self.layer.clone(),
                mode: LayerMutationMode::Advance,
                decision_product_object_id: None,
            }],
            expected_policy_version_object_ids: captured_policy_version_object_ids(
                &before.current_policy_set.record,
            )
            .unwrap(),
        });
        let mut request = SyncTransactionRequest {
            protocol: SyncV3Protocol::V3,
            message_type: TransactionRequestType::Request,
            space_id: self.space.clone(),
            client_id: client_id(),
            idempotency_key: IdempotencyKey::parse(format!("l7-exact-result-{key}")).unwrap(),
            request_digest: before.current_policy_set_id.clone(),
            known_event_cursor: Some(before.watermark_cursor.clone()),
            steps,
            observations: vec![],
            products: vec![],
            view_mutation,
            policy_root_mutation: None,
        };
        request.request_digest = request.computed_request_digest().unwrap();
        serde_json::from_value(self.call(
            "POST",
            "/transactions",
            self.admin,
            serde_json::to_value(request).unwrap(),
        ))
        .unwrap()
    }

    fn service(&self) -> Value {
        let service = super::call(
            self.client,
            "POST",
            &format!("{}/v3/auth/service-accounts", self.origin),
            self.admin,
            json!({"displayName":"L7 exact result fixture","label":"disposable test runner","expiresIn":3600}),
        );
        self.call(
            "POST",
            "/members",
            self.admin,
            json!({"principalId":service["principal_id"],"role":"automation"}),
        );
        service
    }

    fn protection(&self, service: &Value, recipe: Value, approvals: u32) -> Value {
        self.call("PUT", &format!("/layers/{}/protection", self.layer), self.admin,
            json!({"minimumApprovals":approvals,"allowSelfApproval":false,"rejectConflicts":true,
                "recipe":recipe,"trustedRunnerPrincipalIds":[service["principal_id"]],"maximumProofAgeSeconds":3600}))
    }

    fn fixture_proof(
        &self,
        token: &str,
        step: &StepId,
        recipe: &Value,
        key: &str,
    ) -> ProductEnvelope {
        let run = self.call(
            "POST",
            "/runs",
            token,
            json!({"idempotencyKey":key,"stepId":step,"recipeObjectId":recipe}),
        );
        let id = run["runId"].as_str().unwrap();
        self.call("POST", &format!("/runs/{id}/complete"), token,
            json!({"outcome":"passed","log":"synthetic exact-subject gate control; no container execution"}));
        self.proof(token, id, step, recipe)
    }

    fn proof(&self, token: &str, run: &str, step: &StepId, recipe: &Value) -> ProductEnvelope {
        let detail = self.call("GET", &format!("/runs/{run}"), token, Value::Null);
        assert_eq!(detail["stepId"], step.as_str());
        assert_eq!(detail["outcome"], "passed");
        assert_eq!(detail["evidence"][0]["stepId"], step.as_str());
        assert_eq!(&detail["evidence"][0]["recipeObjectId"], recipe);
        assert_eq!(detail["evidence"][0]["outcome"], "passed");
        let current = self.bootstrap();
        assert_eq!(
            detail["evidence"][0]["policySetId"],
            current.current_policy_set_id.as_str()
        );
        assert_eq!(
            detail["evidence"][0]["authorizationEpoch"],
            current.authorization_epoch
        );
        let product: ProductEnvelope = serde_json::from_value(detail["product"].clone()).unwrap();
        product.verify_identity().unwrap();
        let ProductEnvelope::Proof {
            record,
            depends_on_step_ids,
            ..
        } = &product
        else {
            panic!("expected immutable Proof")
        };
        assert_eq!(record.subject, subject(step));
        assert_eq!(depends_on_step_ids, &vec![step.clone()]);
        assert_eq!(record.recipe_object_id.as_str(), recipe.as_str().unwrap());
        product
    }

    fn gate(&self, step: &StepId) -> Value {
        self.call(
            "POST",
            &format!("/layers/{}/gates/evaluate", self.layer),
            self.admin,
            json!({"stepId":step}),
        )
    }
}

fn client_id() -> ClientId {
    ClientId::parse("client:l7-exact-result").unwrap()
}
fn subject(step: &StepId) -> ImmutableSubject {
    ImmutableSubject::Step {
        object_id: ObjectId::parse(step.as_str()).unwrap(),
    }
}
fn entry(name: &str, file: &FileId) -> TreeEntry {
    TreeEntry::new(
        RepoPathComponent::parse(name).unwrap(),
        TreeEntryTarget::file(file.clone()),
        false,
    )
}
fn applied(receipt: &SyncTransactionReceipt) {
    assert_eq!(
        receipt.status,
        ReceiptStatus::Applied,
        "{:?}",
        receipt.rejections
    );
}

pub(super) fn combined(
    client: &reqwest::blocking::Client,
    origin: &str,
    admin: &str,
    ca: &str,
    root: &Path,
) {
    let fixture = Fixture::new(client, origin, admin, "combined-runner");
    let dependency = fixture.file(b"unchanged dependency v1\n");
    let a_file = fixture.file(b"change A\n");
    let b_file = fixture.file(b"change B\n");
    let seed_tree = fixture.tree(vec![entry("dependency.txt", &dependency)]);
    let a_tree = fixture.tree(vec![
        entry("dependency.txt", &dependency),
        entry("a.txt", &a_file),
    ]);
    let b_tree = fixture.tree(vec![
        entry("dependency.txt", &dependency),
        entry("b.txt", &b_file),
    ]);
    let seed = Step::new(vec![], seed_tree.id().clone()).unwrap();
    let a = Step::new(vec![seed.id().clone()], a_tree.id().clone()).unwrap();
    let b = Step::new(vec![seed.id().clone()], b_tree.id().clone()).unwrap();
    let mut objects = MergeObjects::default();
    for step in [&seed, &a, &b] {
        objects.steps.insert(step.id().clone(), step.clone());
    }
    for tree in [&seed_tree, &a_tree, &b_tree] {
        objects.trees.insert(tree.id().clone(), tree.clone());
    }
    objects.files.extend([
        (dependency.clone(), b"unchanged dependency v1\n".to_vec()),
        (a_file.clone(), b"change A\n".to_vec()),
        (b_file.clone(), b"change B\n".to_vec()),
    ]);
    let plan = plan_structural_merge(
        &objects,
        std::slice::from_ref(a.id()),
        std::slice::from_ref(b.id()),
        FileMergePolicy {
            unsupported: FailureAction::StructuralFallback,
            quota: FailureAction::Reject,
            limits: FileMergeLimits::default(),
        },
        GraphMergeLimits::default(),
    )
    .unwrap();
    assert!(plan.conflicts().is_empty());
    assert_eq!(plan.merge_bases(), std::slice::from_ref(seed.id()));
    for tree in plan.trees().values() {
        fixture.upload(
            ObjectKind::Tree,
            tree.id().as_str(),
            serde_jcs::to_vec(tree).unwrap(),
        );
    }
    assert!(
        plan.files().is_empty(),
        "disjoint additions must preserve original FileIds"
    );
    let combined = plan.step();
    let mut parents = vec![a.id().clone(), b.id().clone()];
    parents.sort();
    assert_eq!(combined.parent_step_ids(), parents);
    let expected = Tree::new(vec![
        entry("dependency.txt", &dependency),
        entry("a.txt", &a_file),
        entry("b.txt", &b_file),
    ])
    .unwrap();
    assert_eq!(combined.root_tree_id(), expected.id());
    // Same parents, different result: this passing sibling must not authorize the merge.
    let sibling = Step::new(parents, a_tree.id().clone()).unwrap();
    assert_ne!(combined.id(), sibling.id());
    let initial = fixture.bootstrap();
    applied(&fixture.transact(
        &initial,
        vec![
            seed.clone(),
            a.clone(),
            b.clone(),
            combined.clone(),
            sibling.clone(),
        ],
        Some(&b),
        "combined-seed",
    ));
    let service = fixture.service();
    let token = service["access_token"].as_str().unwrap();
    let recipe = Recipe {
        image: std::env::var("LAYRS_TEST_RUNNER_IMAGE").unwrap(),
        command: vec![
            "/bin/sh".into(),
            "-c".into(),
            concat!(
                "set -eu; test \"$(cat a.txt)\" = 'change A'; ",
                "test \"$(cat b.txt)\" = 'change B'; ",
                "test \"$(cat dependency.txt)\" = 'unchanged dependency v1'; ",
                "test -z \"${LAYRS_RUNNER_TOKEN_FILE:-}\"; ",
                "set -- /sys/class/net/*; test \"$#\" -eq 1; test \"${1##*/}\" = lo; ",
                "printf 'combined A+B with unchanged dependency; network none'"
            )
            .into(),
        ],
        timeout_seconds: 15,
        maximum_log_bytes: 4096,
    };
    let protection = fixture.protection(&service, serde_json::to_value(&recipe).unwrap(), 0);
    assert_eq!(protection["recipeObjectId"], recipe.object_id().unwrap());
    let before = fixture.bootstrap();
    assert_eq!(before.layers[0].head_step_ids, vec![b.id().clone()]);
    assert_eq!(fixture.gate(combined.id())["outcome"], "failed");
    for (label, step) in [("parent-a", &a), ("parent-b", &b), ("sibling", &sibling)] {
        fixture.fixture_proof(
            token,
            step.id(),
            &protection["recipeObjectId"],
            &format!("combined-wrong-proof-{label}"),
        );
        assert_eq!(
            fixture.gate(step.id())["outcome"],
            "passed",
            "control Proof must be valid for its own Step"
        );
        let gate = fixture.gate(combined.id());
        assert_eq!(gate["outcome"], "failed");
        assert_eq!(
            gate["rejection"]["message"],
            "missing fresh passing Proof for exact Step, recipe and policy"
        );
        let refused = fixture.transact(
            &before,
            vec![combined.clone()],
            Some(combined),
            &format!("wrong-proof-{label}"),
        );
        assert_eq!(refused.status, ReceiptStatus::Rejected);
        assert_eq!(refused.rejections[0].code, RejectionCode::GateRejected);
        fixture.unchanged(&before);
    }
    // No native working copy participates: the runner fetches the admitted,
    // unintegrated two-parent Step and all content, including the unchanged file.
    let result = layrs_runner::run(
        Job {
            origin: origin.into(),
            space: fixture.space.clone(),
            step: combined.id().clone(),
            layer: fixture.layer.clone(),
            recipe,
            directory: root.join("l7-combined-ci"),
            ca_pem: Some(ca.as_bytes().to_vec()),
        },
        token,
    )
    .unwrap();
    assert_eq!(result.outcome, "passed", "{}", result.log);
    assert_eq!(result.step_id, *combined.id());
    assert!(
        result
            .log
            .contains("combined A+B with unchanged dependency; network none")
    );
    assert!(!result.log.contains(token));
    layrs_runner::submit(&result, token, Some(ca.as_bytes())).unwrap();
    let proof = fixture.proof(
        token,
        &result.run_id,
        combined.id(),
        &protection["recipeObjectId"],
    );
    fixture.unchanged(&before);
    let gate = fixture.gate(combined.id());
    assert_eq!(gate["outcome"], "passed");
    let product: ProductEnvelope = serde_json::from_value(gate["product"].clone()).unwrap();
    product.verify_identity().unwrap();
    let ProductEnvelope::GateEvaluation { record, .. } = product else {
        panic!("expected GateEvaluation")
    };
    assert_eq!(record.subject, subject(combined.id()));
    assert_eq!(record.proof_object_ids, vec![proof.object_id().clone()]);
    applied(&fixture.transact(
        &before,
        vec![combined.clone()],
        Some(combined),
        "combined-proof-integrate",
    ));
    let after = fixture.bootstrap();
    assert_eq!(after.layers[0].head_step_ids, vec![combined.id().clone()]);
    eprintln!(
        "L7 real combined A/B runner, unchanged dependency, network none, exact-Step Proof gate PASS"
    );
}

pub(crate) fn scoped_recipe(
    client: &reqwest::blocking::Client,
    origin: &str,
    admin: &str,
    author: &str,
) {
    let fixture = Fixture::new(client, origin, admin, "scoped-recipe");
    let old = fixture.file(b"scoped base\n");
    let tree = fixture.tree(vec![entry("file.txt", &old)]);
    let seed = Step::new(vec![], tree.id().clone()).unwrap();
    applied(&fixture.transact(
        &fixture.bootstrap(),
        vec![seed.clone()],
        Some(&seed),
        "scoped-seed",
    ));
    // Reuse the authenticated content-ACL author, granting only this new Space.
    let who = super::call(
        client,
        "GET",
        &format!("{origin}/v3/auth/whoami"),
        author,
        Value::Null,
    );
    fixture.call(
        "POST",
        "/members",
        admin,
        json!({"principalId":who["principal_id"],"role":"member"}),
    );
    let service = fixture.service();
    let token = service["access_token"].as_str().unwrap();
    let protection = fixture.protection(
        &service,
        json!({"command":"scoped exact-candidate fixture"}),
        1,
    );
    let before = fixture.bootstrap();
    let bytes = b"reviewed scoped candidate\n";
    let manifest = FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(SCOPED_CHUNK_BYTES).unwrap(),
        &[Chunk::new(bytes.to_vec()).unwrap()],
    )
    .unwrap();
    let path = RepoPath::parse("file.txt").unwrap();
    let mut patch = ScopedPathPatchRequest {
        schema_version: ScopedPathPatchSchema::V2,
        intent: ScopedPathPatchIntent::Candidate,
        space_id: fixture.space.clone(),
        client_id: client_id(),
        idempotency_key: IdempotencyKey::parse("l7-scoped-recipe-candidate").unwrap(),
        request_digest: before.current_policy_set_id.clone(),
        capture_digest: ObjectId::parse(format!("blake3:{}", "c".repeat(64))).unwrap(),
        layer_id: fixture.layer.clone(),
        base_step_id: seed.id().clone(),
        base_root_tree_id: tree.id().clone(),
        expected_repository_view_id: before.current_repository_view.repository_view_id.clone(),
        expected_generation: before.current_repository_view_generation,
        expected_policy_set_id: before.current_policy_set_id.clone(),
        expected_authorization_epoch: before.authorization_epoch,
        changes: vec![PathPatchChange {
            path: path.clone(),
            expected: Nullable::some(PathPatchEntry {
                target: TreeEntryTarget::file(old),
                executable: false,
            }),
            replacement: Nullable::some(PathPatchEntry {
                target: TreeEntryTarget::file(manifest.file_id().clone()),
                executable: false,
            }),
        }],
        files: vec![ScopedPatchFile { path, manifest }],
    };
    patch.request_digest = patch.computed_request_digest().unwrap();
    patch.validate().unwrap();
    let progress: ScopedPatchProgress = serde_json::from_value(fixture.call(
        "POST",
        "/path-patches/scoped",
        author,
        serde_json::to_value(&patch).unwrap(),
    ))
    .unwrap();
    assert_eq!(progress.missing.len(), 1);
    let response = client
        .put(fixture.url(&format!(
            "/path-patches/scoped/{}/files/0/chunks/0",
            progress.session_id
        )))
        .bearer_auth(author)
        .body(bytes.to_vec())
        .send()
        .unwrap();
    assert!(response.status().is_success());
    let admitted: ScopedPathPatchReceipt = serde_json::from_value(fixture.call(
        "POST",
        &format!("/path-patches/scoped/{}/finalize", progress.session_id),
        author,
        json!({}),
    ))
    .unwrap();
    applied(&admitted.receipt);
    assert!(admitted.receipt.view_result.0.is_none());
    fixture.unchanged(&before);
    let review: ScopedReview = serde_json::from_value(fixture.call("POST", "/scoped-reviews", author,
        json!({"candidateId":admitted.candidate_id.as_ref().unwrap(),"idempotencyKey":"l7-scoped-recipe-review","title":"Exact candidate recipe gate"}))).unwrap();
    assert_ne!(review.candidate_step_id, *seed.id());
    let review_path = format!("/scoped-reviews/{}", review.review_id);
    fixture.call("POST", &format!("{review_path}/actions"), admin,
        json!({"idempotencyKey":"l7-scoped-recipe-approve","action":"approve","body":"Approved exact scoped candidate","anchorPath":null,"replyTo":null}));
    let integrate = json!({"idempotencyKey":"l7-scoped-recipe-integrate"});
    for wrong_proof in [false, true] {
        if wrong_proof {
            fixture.fixture_proof(
                token,
                seed.id(),
                &protection["recipeObjectId"],
                "l7-scoped-wrong-step-proof",
            );
        }
        let detail: ScopedReviewDetail =
            serde_json::from_value(fixture.call("GET", &review_path, author, Value::Null)).unwrap();
        assert_eq!(detail.integration.state, "approved");
        assert_eq!(detail.integration.proof_status, "pending");
        let response = client
            .post(fixture.url(&format!("{review_path}/integrate")))
            .bearer_auth(author)
            .json(&integrate)
            .send()
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        // scoped_review_gate_pending is deliberately exposed as invalid_descriptor.
        assert_eq!(
            response.json::<Value>().unwrap()["error"]["code"],
            "invalid_descriptor"
        );
        fixture.unchanged(&before);
    }
    // This control tests scoped proof selection, not execution. Real Docker
    // evidence for a recomputed result is exercised by combined() above.
    let proof = fixture.fixture_proof(
        token,
        &review.candidate_step_id,
        &protection["recipeObjectId"],
        "l7-scoped-exact-step-proof",
    );
    assert_eq!(proof.dependencies().0, &[review.candidate_step_id.clone()]);
    let detail: ScopedReviewDetail =
        serde_json::from_value(fixture.call("GET", &review_path, author, Value::Null)).unwrap();
    assert_eq!(detail.integration.state, "approved");
    assert_eq!(detail.integration.proof_status, "passed");
    fixture.unchanged(&before);
    let receipt: SyncTransactionReceipt = serde_json::from_value(fixture.call(
        "POST",
        &format!("{review_path}/integrate"),
        author,
        integrate.clone(),
    ))
    .unwrap();
    applied(&receipt);
    assert_eq!(
        fixture.bootstrap().layers[0].head_step_ids,
        vec![review.candidate_step_id]
    );
    let replay: SyncTransactionReceipt = serde_json::from_value(fixture.call(
        "POST",
        &format!("{review_path}/integrate"),
        author,
        integrate,
    ))
    .unwrap();
    assert_eq!(replay, receipt);
    eprintln!(
        "L7 scoped recipe missing/wrong-Step refusal, unchanged heads, exact-candidate Proof and replay PASS"
    );
}

/// Isolated opt-in regression while the graph owner assesses the export gap.
/// The empty root is supported; a named empty directory must not silently vanish.
pub(super) fn empty_tree_export(
    client: &reqwest::blocking::Client,
    origin: &str,
    admin: &str,
    ca: &str,
) {
    let fixture = Fixture::new(client, origin, admin, "empty-tree-export");
    let empty = fixture.tree(vec![]);
    let nested = fixture.tree(vec![TreeEntry::new(
        RepoPathComponent::parse("empty-dir").unwrap(),
        TreeEntryTarget::tree(empty.id().clone()),
        false,
    )]);
    let seed = Step::new(vec![], empty.id().clone()).unwrap();
    let target = Step::new(vec![seed.id().clone()], nested.id().clone()).unwrap();
    applied(&fixture.transact(
        &fixture.bootstrap(),
        vec![seed.clone(), target.clone()],
        Some(&seed),
        "empty-tree-seed",
    ));
    let before = fixture.bootstrap();
    let remote = NativeRemote::with_trusted_ca(origin, admin, ca.as_bytes()).unwrap();
    assert!(
        remote
            .fetch_step_files(&fixture.space, seed.id())
            .unwrap()
            .is_empty()
    );
    let result = remote.fetch_step_files(&fixture.space, target.id());
    fixture.unchanged(&before);
    match result {
        Err(NativeRemoteError::Invalid(message)) if message.contains("empty director") => {}
        Err(error) => panic!("expected explicit unsupported empty-directory export, got {error}"),
        Ok(entries) => panic!(
            "exact-Step export silently omitted the named empty Tree; exported {} entries",
            entries.len()
        ),
    }
    eprintln!("L7 empty root supported; named empty Tree explicitly refused PASS");
}
