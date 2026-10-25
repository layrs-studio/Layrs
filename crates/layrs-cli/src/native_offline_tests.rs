use super::*;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "layrs-offline-output-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.0.parent() == Some(std::env::temp_dir().as_path()) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}

#[test]
fn read_decodes_complete_binary_before_creating_and_never_overwrites() {
    let f = Fixture::new();
    let output = f.0.join("binary");
    let response = json!({"encoding":"base64","bytes":3,"content":STANDARD.encode([0,255,27])});
    for invalid in [
        json!({"encoding":"base64","bytes":2,"content":"AP8b"}),
        json!({"encoding":"base64","bytes":3,"content":"invalid"}),
        json!({"encoding":"text","bytes":3,"content":"abc"}),
    ] {
        assert!(write_read_result(invalid, &output, 3).is_err());
        assert!(!output.exists());
    }
    assert!(write_read_result(response.clone(), &output, 2).is_err());
    assert!(!output.exists());
    let result = write_read_result(response.clone(), &output, 3).unwrap();
    assert!(result.data.get("content").is_none());
    assert_eq!(fs::read(&output).unwrap(), [0, 255, 27]);
    assert!(write_read_result(response, &output, 3).is_err());
    assert_eq!(fs::read(output).unwrap(), [0, 255, 27]);
}

#[test]
fn version_file_accepts_only_bounded_typed_version_or_successful_receipt() {
    let f = Fixture::new();
    let file = f.0.join("version.json");
    let version = json!({"kind":"generation","worktreeId":"worktree:fixture","generation":1,"digest":"from-store"});
    for document in [
        version.clone(),
        json!({"ok":true,"data":{"version":version}}),
    ] {
        fs::write(&file, document.to_string()).unwrap();
        assert!(versions::read_file(&file).is_ok());
    }
    for document in [
        json!({"ok":false,"data":{"version":version}}),
        json!({"ok":true,"data":{}}),
        json!({"kind":"latest"}),
        json!({"kind":"generation","worktreeId":"worktree:fixture","generation":1,"digest":"from-store","hydrate":true}),
    ] {
        fs::write(&file, document.to_string()).unwrap();
        assert!(versions::read_file(&file).is_err());
    }
    fs::write(&file, vec![b' '; 64 * 1024 + 1]).unwrap();
    assert!(versions::read_file(&file).is_err());
}

#[test]
fn restore_artifacts_are_bounded_typed_digest_pinned_and_never_overwritten() {
    use layrs_core::scm::{FileId, ObjectId};
    let f = Fixture::new();
    let artifact = f.0.join("preview.json");
    let digest = ObjectId::parse(format!("blake3:{}", "a".repeat(64))).unwrap();
    let preview = json!({
        "operationId":"1-2", "projectionGeneration":1, "projectionDigest":"generation-digest",
        "checkoutDigest":"before-digest", "afterCheckoutDigest":"after-digest", "digest":digest,
        "destination":{"identity":{"storeId":"store:fixture","worktreeId":"worktree:fixture","spaceId":"space:fixture","origin":"https://example.test","principalId":"principal:fixture"},"checkoutIdentity":"native-checkout-identity"},
        "source":{"kind":"version","version":{"kind":"generation","worktreeId":"worktree:fixture","generation":1,"digest":"generation-digest"},"paths":["a.bin"]},
        "before":{"a.bin":{"fileId":FileId::from_bytes(b"before"),"bytes":6,"executable":false}},
        "after":{"a.bin":{"fileId":FileId::from_bytes(b"after"),"bytes":5,"executable":false}},
    });
    let rendered = restore::save_preview(json!({"preview":preview}), &artifact).unwrap();
    let original = fs::read(&artifact).unwrap();
    let loaded = restore::read_preview(&artifact, &digest).unwrap();
    assert_eq!(serde_json::to_value(loaded).unwrap(), preview);
    for detail in [
        "Read-only local restore preview",
        "Destination Store: store:fixture. Worktree: worktree:fixture.",
        "replace a.bin",
        "6 bytes",
        "5 bytes",
        "executable=false",
        digest.as_str(),
    ] {
        assert!(rendered.human.contains(detail), "missing {detail}");
    }
    assert!(restore::save_preview(json!({"preview":preview}), &artifact).is_err());
    assert_eq!(fs::read(&artifact).unwrap(), original);
    let wrong = ObjectId::parse(format!("blake3:{}", "b".repeat(64))).unwrap();
    assert!(restore::read_preview(&artifact, &wrong).is_err());
    assert!(restore::read_preview(&f.0, &digest).is_err());
    for destination in [None, Some(Value::Null)] {
        let mut unbound = preview.clone();
        unbound.as_object_mut().unwrap().remove("destination");
        if let Some(destination) = destination {
            unbound["destination"] = destination;
        }
        // Legacy history remains deserializable, but cannot be exported or applied as approval.
        let typed: layrs_store_local::projection::RestorePreview =
            serde_json::from_value(unbound.clone()).unwrap();
        assert!(typed.destination.is_none());
        fs::write(&artifact, unbound.to_string()).unwrap();
        assert!(
            restore::read_preview(&artifact, &digest)
                .unwrap_err()
                .message
                .contains("destination")
        );
        let rejected = f.0.join("unbound.json");
        assert!(restore::save_preview(json!({"preview":unbound}), &rejected).is_err());
        assert!(!rejected.exists());
    }
    for invalid in [json!({"ok":false,"error":{"message":"failed"}}), {
        let mut value = preview.clone();
        value["unexpected"] = json!(true);
        value
    }] {
        fs::write(&artifact, invalid.to_string()).unwrap();
        assert!(restore::read_preview(&artifact, &digest).is_err());
    }
    fs::write(&artifact, vec![b' '; 1024 * 1024 + 1]).unwrap();
    assert!(restore::read_preview(&artifact, &digest).is_err());
    let mut large = preview;
    large["operationId"] = json!("1".repeat(1024 * 1024));
    let oversized = f.0.join("oversized.json");
    assert!(restore::save_preview(json!({"preview":large}), &oversized).is_err());
    assert!(!oversized.exists());
}
