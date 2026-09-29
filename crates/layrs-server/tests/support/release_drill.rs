use layrs_client_core::native_scm::{
    CollaborationCommand as Command, NativeRemote, PublishReleaseRequest, RevokeReleaseRequest,
};
use layrs_core::scm::{ObjectId, Release, ReleaseId, ReleasePublicationId, SpaceId};
use serde_json::{Value, json};
fn key() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn id(value: &Value) -> ObjectId {
    serde_json::from_value(value.clone()).unwrap()
}
fn publication(value: &Value) -> ReleasePublicationId {
    serde_json::from_value(value["event"]["publication"]["releasePublicationId"].clone()).unwrap()
}
fn publish(release: &ReleaseId, expected: Option<ObjectId>) -> Command {
    Command::PublishRelease {
        release: release.clone(),
        request: PublishReleaseRequest {
            request_id: key(),
            expected_last_event_id: expected,
        },
    }
}
fn revoke(
    release: &ReleaseId,
    publication: ReleasePublicationId,
    expected: Option<ObjectId>,
) -> Command {
    Command::RevokeRelease {
        release: release.clone(),
        publication,
        request: RevokeReleaseRequest {
            request_id: key(),
            expected_last_event_id: expected,
            reason: "withdrawn after release review".into(),
        },
    }
}
pub fn run(
    remote: &NativeRemote,
    origin: &str,
    token: &str,
    ca: &str,
    space: &SpaceId,
    other: &SpaceId,
    release: &Release,
) {
    let list = remote.collaborate(space, Command::ListReleases).unwrap();
    assert!(
        list["releases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["record"]["releaseId"] == release.id().as_str())
    );
    let original_manifest = list["releases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["record"]["releaseId"] == release.id().as_str())
        .unwrap()
        .clone();
    let history = || {
        remote
            .collaborate(
                space,
                Command::ReleasePublications {
                    release: release.id().clone(),
                },
            )
            .unwrap()
    };
    assert!(history()["events"].as_array().unwrap().is_empty());
    let request = publish(release.id(), None);
    let published = std::thread::scope(|scope| {
        let first = scope.spawn(|| remote.collaborate(space, request.clone()).unwrap());
        let second = remote.collaborate(space, request.clone()).unwrap();
        let first = first.join().unwrap();
        assert_eq!(first, second, "concurrent idempotent publication");
        first
    });
    assert_eq!(published["event"]["kind"], "published");
    assert_eq!(
        published["event"]["stepId"],
        release.target_step_id().as_str()
    );
    assert_eq!(
        remote.collaborate(space, request.clone()).unwrap(),
        published
    );
    assert_eq!(
        remote
            .collaborate(space, publish(release.id(), None))
            .unwrap_err()
            .code(),
        "stale_release_state"
    );
    let pub_id = publication(&published);
    assert_eq!(history()["activePublicationId"], pub_id.as_str());
    assert!(
        remote
            .collaborate(
                other,
                Command::ReleasePublications {
                    release: release.id().clone()
                }
            )
            .is_err()
    );
    let client = reqwest::blocking::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let post = |path: &str, body: Value| {
        let response = client
            .post(format!("{origin}{path}"))
            .bearer_auth(token)
            .json(&body)
            .send()
            .unwrap();
        let status = response.status();
        let value: Value = response.json().unwrap();
        assert!(status.is_success(), "{path}: {status} {value}");
        value
    };
    let account = post(
        "/v3/auth/accounts",
        json!({"login":"release-reader@example.invalid","password":"release reader temporary fixture password","displayName":"Release reader"}),
    );
    let session = NativeRemote::login(
        origin,
        "release-reader@example.invalid",
        "release reader temporary fixture password",
        Some(ca.as_bytes()),
    )
    .unwrap();
    let reader =
        NativeRemote::with_trusted_ca(origin, &session.access_token, ca.as_bytes()).unwrap();
    assert!(
        reader
            .collaborate(
                space,
                Command::ReleasePublications {
                    release: release.id().clone()
                }
            )
            .is_err()
    );
    post(
        &format!("/v3/spaces/{space}/members"),
        json!({"principalId":account["principalId"],"role":"auditor"}),
    );
    assert_eq!(
        reader
            .collaborate(
                space,
                Command::ReleasePublications {
                    release: release.id().clone()
                }
            )
            .unwrap()["activePublicationId"],
        pub_id.as_str()
    );
    assert!(
        reader
            .collaborate(
                space,
                publish(release.id(), Some(id(&published["eventObjectId"])))
            )
            .is_err()
    );
    assert!(
        reader
            .collaborate(
                space,
                revoke(
                    release.id(),
                    pub_id.clone(),
                    Some(id(&published["eventObjectId"]))
                )
            )
            .is_err()
    );
    assert_eq!(
        remote
            .collaborate(space, revoke(release.id(), pub_id.clone(), None))
            .unwrap_err()
            .code(),
        "stale_release_state"
    );
    let revocation = revoke(
        release.id(),
        pub_id.clone(),
        Some(id(&published["eventObjectId"])),
    );
    let revoked = remote.collaborate(space, revocation.clone()).unwrap();
    assert_eq!(revoked["event"]["kind"], "revoked");
    assert_eq!(remote.collaborate(space, revocation).unwrap(), revoked);
    assert!(history()["activePublicationId"].is_null());
    assert_eq!(
        remote.collaborate(space, request).unwrap(),
        published,
        "lost publication response replays even after later revocation"
    );
    let republished = remote
        .collaborate(
            space,
            publish(release.id(), Some(id(&revoked["eventObjectId"]))),
        )
        .unwrap();
    assert_ne!(publication(&republished), pub_id);
    let same = remote.collaborate(space, Command::ListReleases).unwrap();
    assert_eq!(
        same["releases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["record"]["releaseId"] == release.id().as_str())
            .unwrap(),
        &original_manifest
    );
    let who = remote.whoami().unwrap();
    post(
        &format!("/v3/spaces/{space}/members"),
        json!({"principalId":who["principal_id"],"role":"release_manager"}),
    );
    post(
        &format!("/v3/spaces/{space}/members"),
        json!({"principalId":account["principalId"],"role":"release_manager"}),
    );
    let delegated = reader
        .collaborate(
            space,
            revoke(
                release.id(),
                publication(&republished),
                Some(id(&republished["eventObjectId"])),
            ),
        )
        .unwrap();
    assert_ne!(delegated["event"]["actorId"], published["event"]["actorId"]);
    assert_eq!(history()["events"].as_array().unwrap().len(), 4);
}

#[allow(clippy::too_many_arguments)]
pub fn protected(
    remote: &NativeRemote,
    space: &SpaceId,
    release: &Release,
    proved: &Release,
    origin: &str,
    token: &str,
    ca: &str,
    layer: &layrs_core::scm::LayerId,
) {
    let history = remote
        .collaborate(
            space,
            Command::ReleasePublications {
                release: release.id().clone(),
            },
        )
        .unwrap();
    let expected = Some(id(&history["lastEventObjectId"]));
    assert_eq!(
        remote
            .collaborate(space, publish(release.id(), expected))
            .unwrap_err()
            .code(),
        "release_gate_rejected"
    );
    assert_eq!(
        remote
            .collaborate(
                space,
                Command::ReleasePublications {
                    release: release.id().clone()
                }
            )
            .unwrap(),
        history,
        "failed gate must not append a lifecycle event"
    );
    let published = remote
        .collaborate(space, publish(proved.id(), None))
        .unwrap();
    assert_eq!(
        published["event"]["kind"], "published",
        "fresh Proof covers this exact target Step"
    );
    let client = reqwest::blocking::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
        .build()
        .unwrap();
    let response=client.put(format!("{origin}/v3/spaces/{space}/layers/{layer}/protection")).bearer_auth(token).json(&json!({"minimumApprovals":1,"allowSelfApproval":false,"rejectConflicts":true,"recipe":null,"trustedRunnerPrincipalIds":[],"maximumProofAgeSeconds":3600})).send().unwrap();
    assert!(response.status().is_success());
    let revoked = remote
        .collaborate(
            space,
            revoke(
                proved.id(),
                publication(&published),
                Some(id(&published["eventObjectId"])),
            ),
        )
        .unwrap();
    assert_eq!(
        revoked["event"]["kind"], "revoked",
        "revocation remains authorized when publication proofs/reviews no longer pass"
    );
}
