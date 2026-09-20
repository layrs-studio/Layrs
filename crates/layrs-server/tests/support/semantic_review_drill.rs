//! Actual rich Steps reviewed through HTTPS. No CI execution/proof is invented.
use layrs_client_core::native_scm::NativeRemote;
use layrs_core::scm::{LayerHeadSet, LayerId, RepositoryView, StepId};
use layrs_sync::v3::*;
use serde_json::{Value, json};

pub struct Review {
    http: reqwest::blocking::Client,
    origin: String,
    token: String,
    seed: SyncTransactionRequest,
    layer: LayerId,
    id: String,
}
impl Review {
    pub fn start(
        remote: &NativeRemote,
        origin: &str,
        token: &str,
        ca: &str,
        seed: &SyncTransactionRequest,
        layer: LayerId,
        step: &StepId,
    ) -> Self {
        let mut review = Self {
            http: reqwest::blocking::Client::builder()
                .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            origin: origin.into(),
            token: token.into(),
            seed: seed.clone(),
            layer,
            id: String::new(),
        };
        review.call("PUT",&format!("layers/{}/protection",review.layer),json!({"minimumApprovals":1,"allowSelfApproval":true,"rejectConflicts":true,"recipe":null,"trustedRunnerPrincipalIds":[],"maximumProofAgeSeconds":3600}));
        review.propose(step, 1);
        review.approve(1);
        review.advance(remote, step, false);
        review
    }
    fn call(&self, method: &str, path: &str, body: Value) -> Value {
        let response = self
            .http
            .request(
                method.parse().unwrap(),
                format!("{}/v3/spaces/{}/{path}", self.origin, self.seed.space_id),
            )
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .unwrap();
        assert!(
            response.status().is_success(),
            "review {method} {path}: {}",
            response.status()
        );
        response.json().unwrap()
    }
    fn propose(&mut self, step: &StepId, version: u32) {
        let path = if version == 1 {
            "reviews".into()
        } else {
            format!("reviews/{}/versions", self.id)
        };
        let result=self.call("POST",&path,json!({"idempotencyKey":format!("rich-proposal-{}",uuid::Uuid::new_v4()),"layerId":self.layer,"proposedStepId":step,"title":"Review exact immutable block resolution","participants":[],"expectedVersion":version-1}));
        if version == 1 {
            self.id = result["reviewId"].as_str().unwrap().into();
        }
        assert_eq!(result["version"], version);
        let detail = self.call("GET", &format!("reviews/{}", self.id), Value::Null);
        assert_eq!(
            detail["decisionState"]["state"], "pending",
            "earlier version approvals must not carry forward"
        );
        assert_eq!(
            detail["versions"].as_array().unwrap().len(),
            version as usize
        );
    }
    fn approve(&self, version: u32) {
        self.call("POST",&format!("reviews/{}/actions",self.id),json!({"idempotencyKey":format!("rich-approve-{}",uuid::Uuid::new_v4()),"version":version,"action":"approve","body":"Approve this exact immutable Step only","anchor":null,"replyTo":null}));
    }
    pub fn partial(&mut self, remote: &NativeRemote, step: &StepId) {
        self.propose(step, 2);
        self.approve(2);
        self.advance(remote, step, false);
    }
    pub fn finish(&mut self, remote: &NativeRemote, step: &StepId) {
        self.propose(step, 3);
        self.advance(remote, step, false);
        self.approve(3);
        self.advance(remote, step, true);
        let detail = self.call("GET", &format!("reviews/{}", self.id), Value::Null);
        assert_eq!(detail["lifecycle"], "integrated");
        assert_eq!(detail["integration"]["record"]["version"], 3);
        assert_eq!(
            detail["integration"]["record"]["proposedStepId"],
            step.as_str()
        );
        assert_eq!(detail["versions"].as_array().unwrap().len(), 3);
        eprintln!(
            "T2/T5 rich review: unresolved/partial refused, exact final version reapproved and integrated"
        );
    }
    fn advance(&self, remote: &NativeRemote, step: &StepId, accept: bool) {
        let bootstrap = remote.bootstrap(&self.seed.space_id).unwrap();
        let current = &bootstrap.current_repository_view.record;
        let heads = current
            .layer_heads()
            .iter()
            .map(|heads| {
                if heads.layer_id() == &self.layer {
                    LayerHeadSet::new(self.layer.clone(), vec![step.clone()]).unwrap()
                } else {
                    heads.clone()
                }
            })
            .collect();
        let view = RepositoryView::new(
            self.seed.space_id.clone(),
            heads,
            current.marker_ids().to_vec(),
            current.release_ids().to_vec(),
            vec![],
        )
        .unwrap();
        let mut request = self.seed.clone();
        request.idempotency_key = IdempotencyKey::parse(uuid::Uuid::new_v4().to_string()).unwrap();
        request.steps.clear();
        request.products.clear();
        request.observations.clear();
        request.known_event_cursor = Some(bootstrap.watermark_cursor.clone());
        request.view_mutation = Some(RepositoryViewMutation {
            mutation_id: MutationId::parse(format!("mutation:{}", uuid::Uuid::new_v4())).unwrap(),
            expected_current_view_id: current.id().clone(),
            expected_generation: bootstrap.current_repository_view_generation,
            proposed_view: ProposedRepositoryView::Inline { view },
            layer_mutations: vec![LayerMutationIntent {
                layer_id: self.layer.clone(),
                mode: LayerMutationMode::Advance,
                decision_product_object_id: None,
            }],
            expected_policy_version_object_ids: captured_policy_version_object_ids(
                &bootstrap.current_policy_set.record,
            )
            .unwrap(),
        });
        request.request_digest = request.computed_request_digest().unwrap();
        let receipt: SyncTransactionReceipt = serde_json::from_value(self.call(
            "POST",
            "transactions",
            serde_json::to_value(request).unwrap(),
        ))
        .unwrap();
        assert_eq!(
            receipt.status,
            if accept {
                ReceiptStatus::Applied
            } else {
                ReceiptStatus::Rejected
            },
            "{:?}",
            receipt.rejections
        );
        if !accept {
            assert!(
                receipt
                    .rejections
                    .iter()
                    .any(|r| r.code == RejectionCode::GateRejected),
                "{:?}",
                receipt.rejections
            );
            assert_eq!(
                remote
                    .bootstrap(&self.seed.space_id)
                    .unwrap()
                    .current_repository_view,
                bootstrap.current_repository_view
            );
        }
    }
}
