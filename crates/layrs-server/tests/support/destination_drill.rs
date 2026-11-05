//! Existing native folders must be inspected without replacing local files.
use layrs_client_core::native_scm::{NativeRemote, NativeWorkspace};
use layrs_core::scm::SpaceId;
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn run(origin: &str, token: &str, ca: &str, root: &Path, published: &SpaceId) {
    let remote = NativeRemote::with_trusted_ca(origin, token, ca.as_bytes()).unwrap();
    let known = root.join("empty-first");
    let same = remote.inspect_clone_destination(&known, Some(published), None);
    assert_eq!(same.kind, "sameProject");
    assert_eq!(same.local_space_id.as_ref(), Some(published));
    assert!(same.context_id.is_some());
    let other = remote.inspect_clone_destination(&known, None, None);
    assert_eq!(other.kind, "otherProject");
    assert_eq!(other.local_space_id.as_ref(), Some(published));

    let empty = root.join("destination-empty");
    std::fs::create_dir(&empty).unwrap();
    assert_eq!(
        remote
            .inspect_clone_destination(&empty, Some(published), None)
            .kind,
        "empty"
    );
    let raw = root.join("destination-raw");
    std::fs::create_dir(&raw).unwrap();
    std::fs::write(raw.join("keep.txt"), b"preserve").unwrap();
    assert_eq!(
        remote
            .inspect_clone_destination(&raw, Some(published), None)
            .kind,
        "blocked"
    );
    assert_eq!(std::fs::read(raw.join("keep.txt")).unwrap(), b"preserve");
    let corrupt = root.join("destination-corrupt");
    std::fs::create_dir(&corrupt).unwrap();
    std::fs::create_dir(corrupt.join(".layrs")).unwrap();
    assert_eq!(
        remote
            .inspect_clone_destination(&corrupt, Some(published), None)
            .kind,
        "blocked"
    );

    let unpublished_root = root.join("destination-unpublished");
    std::fs::create_dir(&unpublished_root).unwrap();
    std::fs::write(unpublished_root.join("keep.txt"), b"my local work").unwrap();
    let unpublished = NativeWorkspace::initialize(&unpublished_root, "My local work").unwrap();
    let local_space = unpublished.state().unwrap().space.id().clone();
    let inspected = remote.inspect_clone_destination(&unpublished_root, Some(published), None);
    assert_eq!(inspected.kind, "unpublishedProject");
    assert_eq!(inspected.local_space_id.as_ref(), Some(&local_space));
    let context = inspected.context_id.unwrap();
    let (prepared, selected_context) = remote
        .prepare_existing_publication(&unpublished_root, &local_space, &context)
        .unwrap();
    assert_eq!(selected_context, context);
    assert_eq!(prepared.space.id(), &local_space);
    assert_eq!(
        std::fs::read(unpublished_root.join("keep.txt")).unwrap(),
        b"my local work"
    );
    assert!(!unpublished.store_root().join("native-remote.json").exists());

    let http = reqwest::blocking::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
        .build()
        .unwrap();
    let occupied: Value = json!({"spaceId":published,"contextId":context});
    let response = http
        .post(format!("{origin}/v3/projects/check-local"))
        .bearer_auth(token)
        .json(&occupied)
        .send()
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    let response = http
        .post(format!("{origin}/v3/projects/check-local"))
        .bearer_auth(token)
        .json(&json!({"spaceId":local_space,"contextId":"workspace:missing"}))
        .send()
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);

    let binding_path = unpublished.store_root().join("native-remote.json");
    let provision = remote
        .provision_existing(
            &unpublished_root,
            &local_space,
            &context,
            "unused",
            "destination-native",
        )
        .unwrap();
    assert_eq!(provision.space_id, local_space);
    assert_eq!(
        remote
            .provision_existing(
                &unpublished_root,
                &local_space,
                &context,
                "unused",
                "destination-native"
            )
            .unwrap()
            .space_id,
        local_space,
        "retry after a lost response must replay the same provision"
    );
    assert!(
        remote
            .provision_existing(
                &unpublished_root,
                &local_space,
                &context,
                "unused",
                "different-slug"
            )
            .is_err()
    );
    assert!(
        remote
            .prepare_existing_publication(&unpublished_root, &local_space, &context)
            .is_err()
    );
    assert_eq!(
        remote
            .inspect_clone_destination(&unpublished_root, Some(published), None)
            .kind,
        "otherProject"
    );
    assert!(!binding_path.exists());
    let state = remote
        .link_existing(&unpublished_root, &local_space)
        .unwrap();
    assert_eq!(state.space.id(), &local_space);
    assert!(binding_path.exists());
    assert_eq!(
        std::fs::read(unpublished_root.join("keep.txt")).unwrap(),
        b"my local work"
    );
    let before = std::fs::read(&binding_path).unwrap();
    remote
        .link_existing(&unpublished_root, &local_space)
        .unwrap();
    assert_eq!(std::fs::read(&binding_path).unwrap(), before);

    let mut foreign: Value = serde_json::from_slice(&before).unwrap();
    foreign["origin"] = json!("https://different.example");
    std::fs::write(&binding_path, serde_json::to_vec(&foreign).unwrap()).unwrap();
    assert_eq!(
        remote
            .inspect_clone_destination(&unpublished_root, Some(published), None)
            .kind,
        "blocked"
    );
    assert!(
        remote
            .link_existing(&unpublished_root, &local_space)
            .is_err()
    );
    assert_eq!(
        std::fs::read(unpublished_root.join("keep.txt")).unwrap(),
        b"my local work"
    );
    std::fs::write(&binding_path, &before).unwrap();
    let mut pending: Value = serde_json::from_slice(&before).unwrap();
    pending["clonePending"] = json!(true);
    std::fs::write(&binding_path, serde_json::to_vec(&pending).unwrap()).unwrap();
    assert_eq!(
        remote
            .inspect_clone_destination(&unpublished_root, Some(published), None)
            .kind,
        "blocked"
    );
    assert!(
        remote
            .link_existing(&unpublished_root, &local_space)
            .is_err()
    );
    std::fs::write(&binding_path, &before).unwrap();

    let substituted = root.join("destination-substituted");
    std::fs::create_dir(&substituted).unwrap();
    let unrelated = NativeWorkspace::initialize(&substituted, "Unrelated").unwrap();
    assert!(remote.link_existing(&substituted, &local_space).is_err());
    assert!(!unrelated.store_root().join("native-remote.json").exists());
}
