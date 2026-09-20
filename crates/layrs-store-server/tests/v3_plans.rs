use async_trait::async_trait;
use layrs_core::scm::{ObjectId, RepoPath, SpaceId, StepId};
use layrs_store_server::v3::*;
use layrs_sync::v3::{
    AuthorizationRoot, ClientId, Compression, ObjectDescriptor, ObjectKind, ObjectReference,
    PlanId, PrincipalId, RequestId,
};
use std::collections::BTreeSet;

fn oid(byte: char) -> ObjectId {
    ObjectId::parse(format!("blake3:{}", byte.to_string().repeat(64))).unwrap()
}
fn descriptor(byte: char) -> ObjectDescriptor {
    ObjectDescriptor {
        kind: ObjectKind::Tree,
        object_id: oid(byte),
        canonical_byte_len: 10,
        encoded_byte_len: 10,
        compression: Compression::Identity,
    }
}
fn actor(space: &str) -> CapabilityActor {
    CapabilityActor {
        principal_id: PrincipalId::parse("principal:alice").unwrap(),
        authorization_epoch: 7,
        space_id: SpaceId::parse(space).unwrap(),
        client_id: ClientId::parse("client:desktop").unwrap(),
        request_id: RequestId::parse("request:r1").unwrap(),
        plan_id: PlanId::parse("plan:p1").unwrap(),
    }
}
fn context(space: &str) -> PlanContext {
    let policy = TransferUrlPolicy::new(["https://objects.example.test"], [80, 3000]).unwrap();
    PlanContext {
        actor: actor(space),
        now_unix_seconds: 100,
        expires_at_unix_seconds: 200,
        transfer_base_url: policy
            .validate_base("https://objects.example.test")
            .unwrap(),
    }
}

#[test]
fn transfer_urls_require_exact_allowlisted_origins_or_explicit_loopback_ports() {
    let policy = TransferUrlPolicy::new(
        [
            "https://objects.example.test",
            "https://objects.example.test:8443",
        ],
        [80, 3000],
    )
    .unwrap();

    for accepted in [
        "https://objects.example.test",
        "https://objects.example.test:8443",
        "http://localhost",
        "http://localhost:3000",
        "http://127.42.7.9:3000",
        "http://[::1]:3000",
    ] {
        assert!(policy.validate_base(accepted).is_ok(), "{accepted}");
    }
    for rejected in [
        "https://arbitrary.example.test",
        "https://objects.example.test:9443",
        "https://alice@objects.example.test",
        "https://objects.example.test/api",
        "https://objects.example.test?next=evil",
        "https://objects.example.test#fragment",
        "http://localhost.evil:3000",
        "http://128.0.0.1:3000",
        "http://localhost:4000",
        "ftp://localhost:3000",
    ] {
        assert!(policy.validate_base(rejected).is_err(), "{rejected}");
    }
    assert!(TransferUrlPolicy::new(["https://objects.example.test/path"], [3000]).is_err());
    assert!(TransferUrlPolicy::new(["http://localhost"], [3000]).is_err());
    assert!(TransferUrlPolicy::new(["https://objects.example.test"], [0]).is_err());
}

struct Availability {
    admitted: BTreeSet<(String, String)>,
}
#[async_trait]
impl SpaceObjectAvailability for Availability {
    async fn is_verified_available(
        &self,
        space: &SpaceId,
        descriptor: &ObjectDescriptor,
    ) -> V3StoreResult<bool> {
        Ok(self
            .admitted
            .contains(&(space.to_string(), descriptor.object_id.to_string())))
    }
}

#[tokio::test]
async fn object_plan_is_exact_partition_and_not_a_cross_space_oracle() {
    let a = descriptor('a');
    let b = descriptor('b');
    let availability = Availability {
        admitted: BTreeSet::from([("space:a".into(), a.object_id.to_string())]),
    };
    let codec = CapabilityCodec::new([9_u8; 32]).unwrap();
    let builder = ObjectPlanBuilder::new(&availability, &codec, PlanLimits::default());
    let plan = builder
        .build(&context("space:a"), vec![b.clone(), a.clone()])
        .await
        .unwrap();
    assert_eq!(plan.present_objects, vec![a.clone()]);
    assert_eq!(plan.missing_objects, vec![b.clone()]);
    assert_eq!(plan.upload_capabilities.len(), 1);
    assert!(plan.upload_capabilities[0].full_object_only);
    assert_eq!(
        plan.upload_capabilities[0].upload_url,
        "https://objects.example.test/v3/transfers/uploads"
    );
    assert!(
        !plan.upload_capabilities[0]
            .upload_url
            .contains(&plan.upload_capabilities[0].token)
    );
    codec
        .verify_upload(
            &plan.upload_capabilities[0].token,
            &actor("space:a"),
            &b,
            7,
            150,
        )
        .unwrap();

    let other = builder
        .build(&context("space:b"), vec![a.clone()])
        .await
        .unwrap();
    assert!(other.present_objects.is_empty());
    assert_eq!(other.missing_objects, vec![a]);
}

#[test]
fn capability_rejects_tamper_expiry_epoch_and_binding_changes() {
    let codec = CapabilityCodec::new([3_u8; 32]).unwrap();
    let capability_actor = actor("space:a");
    let descriptor = descriptor('c');
    let token = codec
        .issue_upload(capability_actor.clone(), descriptor.clone(), 200)
        .unwrap();
    let mut tampered = token.into_bytes();
    let last = tampered.len() - 1;
    tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
    assert!(matches!(
        codec.verify_upload(
            std::str::from_utf8(&tampered).unwrap(),
            &capability_actor,
            &descriptor,
            7,
            100
        ),
        Err(V3StoreError::InvalidCapability)
    ));
    let token = codec
        .issue_upload(capability_actor.clone(), descriptor.clone(), 200)
        .unwrap();
    assert!(matches!(
        codec.verify_upload(&token, &capability_actor, &descriptor, 7, 201),
        Err(V3StoreError::ExpiredCapability)
    ));
    assert!(matches!(
        codec.verify_upload(&token, &capability_actor, &descriptor, 8, 100),
        Err(V3StoreError::AuthorizationEpochChanged)
    ));
    assert!(matches!(
        codec.verify_upload(&token, &actor("space:b"), &descriptor, 7, 100),
        Err(V3StoreError::CapabilityBindingMismatch)
    ));

    let claim_digest = oid('d');
    let range = ByteRange::new(2, 5).unwrap();
    let download = codec
        .issue_download(
            capability_actor.clone(),
            descriptor.clone(),
            claim_digest.clone(),
            range,
            "resume:bound-token",
            200,
        )
        .unwrap();
    codec
        .verify_download(DownloadVerification {
            token: &download,
            expected_actor: &capability_actor,
            expected_descriptor: &descriptor,
            expected_claim_digest: &claim_digest,
            expected_range: range,
            expected_resume_token: "resume:bound-token",
            current_authorization_epoch: 7,
            now_unix_seconds: 100,
        })
        .unwrap();
    assert!(matches!(
        codec.verify_download(DownloadVerification {
            token: &download,
            expected_actor: &capability_actor,
            expected_descriptor: &descriptor,
            expected_claim_digest: &claim_digest,
            expected_range: ByteRange::new(3, 5).unwrap(),
            expected_resume_token: "resume:bound-token",
            current_authorization_epoch: 7,
            now_unix_seconds: 100,
        }),
        Err(V3StoreError::CapabilityBindingMismatch)
    ));
}

struct Fetches {
    descriptor: ObjectDescriptor,
}
#[async_trait]
impl RootedFetchAvailability for Fetches {
    async fn resolve(
        &self,
        _space: &SpaceId,
        claim: &FetchClaim,
    ) -> V3StoreResult<ClaimResolution> {
        match claim {
            FetchClaim::Object {
                repo_path: Some(_), ..
            } => Ok(ClaimResolution::Available {
                descriptors: vec![self.descriptor.clone()],
                file_manifest_selection: None,
            }),
            _ => Ok(ClaimResolution::Denied(DeniedCode::PathDenied)),
        }
    }
}

fn claim(byte: char, path: &str) -> FetchClaim {
    FetchClaim::Object {
        object: ObjectReference {
            kind: ObjectKind::Tree,
            object_id: oid(byte),
        },
        root: AuthorizationRoot::Step {
            step_id: StepId::parse(format!("blake3:{}", "d".repeat(64))).unwrap(),
        },
        repo_path: Some(RepoPath::parse(path).unwrap()),
    }
}

#[tokio::test]
async fn rooted_fetch_pages_are_bounded_and_cursor_bound() {
    let descriptor = descriptor('e');
    let provider = Fetches {
        descriptor: descriptor.clone(),
    };
    let codec = CapabilityCodec::new([5_u8; 32]).unwrap();
    let builder = FetchPlanBuilder::new(
        &provider,
        &codec,
        PlanLimits {
            max_requested_descriptors: 10,
            max_claims: 10,
            max_page_objects: 1,
        },
    );
    let claims = vec![claim('1', "src/a"), claim('2', "src/b")];
    let first = builder
        .build(&context("space:a"), claims.clone(), None, 1)
        .await
        .unwrap();
    assert_eq!(first.download_capabilities.len(), 1);
    assert_eq!(
        first.download_capabilities[0].download_url,
        "https://objects.example.test/v3/transfers/downloads"
    );
    assert!(
        !first.download_capabilities[0]
            .download_url
            .contains(&first.download_capabilities[0].token)
    );
    assert!(first.has_more);
    let cursor = first.next_page_cursor.as_deref().unwrap();
    let second = builder
        .build(&context("space:a"), claims, Some(cursor), 1)
        .await
        .unwrap();
    assert_eq!(second.download_capabilities.len(), 1);
    assert!(!second.has_more);
    let capability = &first.download_capabilities[0];
    let expected_actor = context("space:a").actor;
    codec
        .verify_download(DownloadVerification {
            token: &capability.token,
            expected_actor: &expected_actor,
            expected_descriptor: &descriptor,
            expected_claim_digest: &capability.claim_digest,
            expected_range: capability.range,
            expected_resume_token: &capability.resume_token,
            current_authorization_epoch: 7,
            now_unix_seconds: 150,
        })
        .unwrap();
    assert!(
        builder
            .build(
                &context("space:b"),
                vec![claim('1', "src/a"), claim('2', "src/b")],
                Some(cursor),
                1
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn finalized_fetch_snapshot_pages_resolve_each_claim_once_and_keep_total_bound() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Counted(AtomicUsize);
    #[async_trait]
    impl RootedFetchAvailability for Counted {
        async fn resolve(
            &self,
            _space: &SpaceId,
            claim: &FetchClaim,
        ) -> V3StoreResult<ClaimResolution> {
            self.0.fetch_add(1, Ordering::SeqCst);
            let FetchClaim::Object { object, .. } = claim else {
                unreachable!()
            };
            let mut value = descriptor('a');
            value.object_id = object.object_id.clone();
            Ok(ClaimResolution::Available {
                descriptors: vec![value],
                file_manifest_selection: None,
            })
        }
    }
    let provider = Counted(AtomicUsize::new(0));
    let codec = CapabilityCodec::new([5; 32]).unwrap();
    let context = context("space:a");
    let claims = vec![claim('1', "a"), claim('2', "b")];
    let limits = PlanLimits {
        max_claims: 3,
        max_requested_descriptors: 3,
        max_page_objects: 2,
    };
    let (snapshot, page) =
        build_fetch_snapshot_page(&provider, &codec, limits, &context, claims.clone(), None, 1)
            .await
            .unwrap();
    assert_eq!(
        provider.0.load(Ordering::SeqCst),
        2,
        "page must reuse exactly the frozen resolutions"
    );
    assert!(!snapshot.has_more);
    assert_eq!(snapshot.download_capabilities.len(), 2);
    assert!(page.has_more);
    assert_eq!(page.download_capabilities.len(), 1);
    let (same, next) = build_fetch_snapshot_page(
        &provider,
        &codec,
        limits,
        &context,
        claims,
        page.next_page_cursor.as_deref(),
        1,
    )
    .await
    .unwrap();
    assert_eq!(same, snapshot);
    assert!(!next.has_more);
    assert_eq!(
        next.download_capabilities[0],
        snapshot.download_capabilities[1]
    );
    let too_large = vec![claim('1', "a"), claim('2', "b"), claim('3', "c")];
    assert!(
        matches!(
            build_fetch_snapshot_page(&provider, &codec, limits, &context, too_large, None, 1)
                .await,
            Err(V3StoreError::LimitExceeded { .. })
        ),
        "small pages never bypass the total snapshot budget"
    );
}
