//! An HTTPS project created by the Web API has no published Step yet.
use layrs_client_core::native_scm::{NativeRemote, NativeTransferControl};
use layrs_core::scm::{LayerKind, SpaceId};
use layrs_sync::v3::{LayerMutationMode, ReceiptStatus};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn run(origin: &str, token: &str, ca: &str, root: &Path) -> SpaceId {
    let remote = NativeRemote::with_trusted_ca(origin, token, ca.as_bytes()).unwrap();
    let contexts = remote.list_contexts().unwrap();
    let context_id = contexts["defaultContextId"].as_str().unwrap();
    let http = reqwest::blocking::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
        .build()
        .unwrap();
    let catalog: Value = http
        .get(format!("{origin}/v3/lenses/catalog"))
        .bearer_auth(token)
        .send()
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .unwrap();
    let items = catalog
        .as_array()
        .or_else(|| catalog["items"].as_array())
        .unwrap();
    let text_lens = items
        .iter()
        .find(|item| {
            item["manifest"]["analyzer"]["fileExtensions"]
                .as_array()
                .is_some_and(|extensions| extensions.iter().any(|extension| extension == "txt"))
        })
        .expect("installed text Lens");
    let pin = text_lens["pin"].clone();
    let association = json!({"extension":".txt","lensId":pin["lensId"]});
    let created = remote
        .create_project(&json!({
            "contextId": context_id,
            "spaceSlug": "empty-web-clone",
            "name": "Empty Web clone",
            "lensConfiguration": {
                "pins": [pin.clone()],
                "associations": [association.clone()]
            }
        }))
        .unwrap();
    let space = SpaceId::parse(created["spaceId"].as_str().unwrap()).unwrap();
    let initial = remote.bootstrap(&space).unwrap();
    assert_eq!(initial.current_repository_view_generation, 0);
    assert_eq!(initial.layers.len(), 1);
    assert_eq!(initial.layers[0].record.kind(), LayerKind::Base);
    assert!(initial.layers[0].record.parent_layer_ids().is_empty());
    assert!(initial.layers[0].head_step_ids.is_empty());
    assert!(initial.graph_root_step_ids.is_empty());
    assert!(initial.markers.is_empty() && initial.releases.is_empty());

    let first_root = root.join("empty-first");
    let second_root = root.join("empty-second");
    let occupied_root = root.join("empty-occupied");
    for path in [&first_root, &second_root, &occupied_root] {
        std::fs::create_dir(path).unwrap();
    }
    std::fs::write(occupied_root.join("keep.txt"), b"untouched").unwrap();
    assert!(
        remote
            .clone_into(&occupied_root, &space, "Occupied")
            .is_err()
    );
    assert_eq!(
        std::fs::read(occupied_root.join("keep.txt")).unwrap(),
        b"untouched"
    );
    assert!(!occupied_root.join(".layrs").exists());

    let first = remote
        .clone_into_cancellable(&first_root, &space, "First", &NativeTransferControl::new())
        .unwrap();
    let second = remote.clone_into(&second_root, &space, "Second").unwrap();
    assert_ne!(first.worktree_id(), second.worktree_id());
    assert_eq!(first.state().unwrap().space.id(), &space);
    assert_eq!(second.state().unwrap().space.id(), &space);
    assert_eq!(
        remote.bootstrap(&space).unwrap(),
        initial,
        "clone must not publish"
    );

    let first_heads = first.state().unwrap().view.layer_heads().to_vec();
    let second_heads = second.state().unwrap().view.layer_heads().to_vec();
    assert_eq!(
        first_heads, second_heads,
        "pristine local Step is deterministic"
    );
    assert_eq!(first_heads.len(), 1);
    assert_eq!(
        first_heads[0].layer_id(),
        first.state().unwrap().layers[0].id()
    );
    assert_eq!(first_heads[0].step_ids().len(), 1);
    let cached: Value = serde_json::from_slice(
        &std::fs::read(first.store_root().join("native-lenses.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(cached["revision"], 1);
    assert_eq!(cached["pins"], json!([pin]));
    assert_eq!(cached["associations"], json!([association]));

    // Simulate a process stopping after accepting the clone, before clearing
    // its journal flag. Receive must complete without duplicating local state.
    let binding_path = second.store_root().join("native-remote.json");
    let mut binding: Value =
        serde_json::from_slice(&std::fs::read(&binding_path).unwrap()).unwrap();
    binding["clonePending"] = json!(true);
    std::fs::write(&binding_path, serde_json::to_vec(&binding).unwrap()).unwrap();
    std::fs::write(second_root.join("uncommitted.txt"), b"keep this edit").unwrap();
    assert!(
        remote
            .receive_cancellable(&second, &NativeTransferControl::new())
            .is_err()
    );
    assert_eq!(
        std::fs::read(second_root.join("uncommitted.txt")).unwrap(),
        b"keep this edit"
    );
    std::fs::remove_file(second_root.join("uncommitted.txt")).unwrap();
    let resumed = remote
        .receive_cancellable(&second, &NativeTransferControl::new())
        .unwrap();
    assert!(!resumed.divergent);
    assert_eq!(second.state().unwrap().view.layer_heads(), second_heads);
    let binding: Value = serde_json::from_slice(&std::fs::read(&binding_path).unwrap()).unwrap();
    assert_eq!(binding["clonePending"], false);

    std::fs::write(first_root.join("hello.txt"), b"created after Web clone").unwrap();
    first.capture().unwrap();
    let captured = first.state().unwrap().view.layer_heads()[0].step_ids()[0].clone();
    let published = remote.publish(&first, LayerMutationMode::Advance).unwrap();
    assert_eq!(
        published.status,
        ReceiptStatus::Applied,
        "{:?}",
        published.rejections
    );
    assert!(
        published
            .accepted_step_ids
            .contains(&first_heads[0].step_ids()[0])
    );
    assert!(published.accepted_step_ids.contains(&captured));
    assert_eq!(
        remote.bootstrap(&space).unwrap().layers[0]
            .head_step_ids
            .len(),
        1
    );

    let received = remote.receive(&second).unwrap();
    assert!(received.materialized && !received.divergent);
    assert_eq!(
        std::fs::read(second_root.join("hello.txt")).unwrap(),
        b"created after Web clone"
    );
    let third_root = root.join("empty-third");
    std::fs::create_dir(&third_root).unwrap();
    let third = remote.clone_into(&third_root, &space, "Third").unwrap();
    assert_eq!(third.state().unwrap().space.id(), &space);
    assert_eq!(
        third.state().unwrap().layers[0].id(),
        first.state().unwrap().layers[0].id()
    );
    assert_eq!(
        std::fs::read(third_root.join("hello.txt")).unwrap(),
        b"created after Web clone"
    );

    // A second pristine Web project must also publish with no file Capture.
    let empty_created = remote
        .create_project(&json!({
            "contextId": context_id,
            "spaceSlug": "empty-publish-only",
            "name": "Empty publish only"
        }))
        .unwrap();
    let empty_space = SpaceId::parse(empty_created["spaceId"].as_str().unwrap()).unwrap();
    assert!(
        remote
            .bootstrap(&empty_space)
            .unwrap()
            .graph_root_step_ids
            .is_empty()
    );
    let publish_root = root.join("empty-publish-only");
    std::fs::create_dir(&publish_root).unwrap();
    let publish_checkout = remote
        .clone_into(&publish_root, &empty_space, "Empty publish only")
        .unwrap();
    let genesis = publish_checkout.state().unwrap().view.layer_heads()[0].step_ids()[0].clone();
    let empty_publish = remote
        .publish(&publish_checkout, LayerMutationMode::Advance)
        .unwrap();
    assert_eq!(
        empty_publish.status,
        ReceiptStatus::Applied,
        "{:?}",
        empty_publish.rejections
    );
    assert!(empty_publish.accepted_step_ids.contains(&genesis));
    assert_eq!(
        remote.bootstrap(&empty_space).unwrap().layers[0].head_step_ids,
        vec![genesis]
    );
    space
}
