use layrs_core::scm::{ActorId, DeviceId, SpaceId, StoreId};
use layrs_store::scm::StoreError;
use layrs_store_local::scm_v1::{self, LocalIdentity, NativeScmReader};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "layrs-identity-{}-{nonce}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[test]
fn registry_scope_canonical_bytes_and_limits_are_authoritative() {
    let dir = Directory::new();
    let store = scm_v1::init(&dir.0, StoreId::parse("store:identity-tests").unwrap()).unwrap();
    assert!(scm_v1::local_identity(&store).unwrap().is_none());
    drop(store);
    let identity = LocalIdentity {
        actor_id: ActorId::parse("actor:test").unwrap(),
        device_id: Some(DeviceId::parse("device:test").unwrap()),
        display_name: Some("Test".into()),
    };
    let store = scm_v1::open_exclusive(&dir.0).unwrap();
    scm_v1::configure_local_identity(&store, identity.clone()).unwrap();
    let path = store.layout().metadata_dir().join("local-identities.json");
    let original = fs::read(&path).unwrap();
    let invalid = LocalIdentity {
        display_name: Some("\n".into()),
        ..identity.clone()
    };
    assert!(scm_v1::configure_local_identity(&store, invalid).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    drop(store);
    let store = scm_v1::open(&dir.0).unwrap();
    assert_eq!(scm_v1::local_identity(&store).unwrap(), Some(identity));
    drop(store);
    for damaged in [
        String::from_utf8(original.clone())
            .unwrap()
            .replace("store:identity-tests", "store:other"),
        format!(" {}", String::from_utf8(original.clone()).unwrap()),
        String::from_utf8(original.clone()).unwrap().replace(
            "layrs.local-identity-registry.v1",
            "layrs.local-identity-registry.v2",
        ),
        "x".repeat(65537),
    ] {
        fs::write(&path, damaged).unwrap();
        let store = scm_v1::open(&dir.0).unwrap();
        assert!(NativeScmReader::new(&store).is_err());
    }
    fs::write(&path, original).unwrap();
}

#[test]
fn observation_projection_rejects_oversized_row_population_before_decoding() {
    let dir = Directory::new();
    let store = scm_v1::init(&dir.0, StoreId::parse("store:history-budget").unwrap()).unwrap();
    let path = store.layout().sqlite_path().to_owned();
    drop(store);
    // Deliberate corruption injection in the storage layer. Missing references
    // must not make a huge source allocate/decode before enforcing row limits.
    let connection = rusqlite::Connection::open(path).unwrap();
    connection.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    connection.execute_batch("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<100001) INSERT INTO step_observations(observation_id,step_id,space_id,observed_at,capture_reason,payload_json) SELECT 'observation:'||x,'step:missing','space:budget','2020-01-01T00:00:00Z','received','{}' FROM n").unwrap();
    drop(connection);
    let store = scm_v1::open(&dir.0).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    assert!(matches!(
        reader.observation_projection(&SpaceId::parse("space:budget").unwrap(), None),
        Err(StoreError::AdmissionBudgetExceeded { .. })
    ));
}
