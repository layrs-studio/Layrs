use bytes::Bytes;
use layrs_core::scm::{
    ObjectId, RepositoryView, SpaceId, StoreId, Timestamp, WorktreeId, WorktreeStatus,
    WorktreeViewState,
};
use layrs_store_server::v3::{CasLimits, CatalogedCas, CatalogedFsCas, FsServerCas, V3StoreError};
use layrs_sync::v3::{
    AuthorizationRequestBinding, AuthorizationSnapshot, AuthorizationSnapshotSchemaVersion,
    AuthorizationSubject, ClientId, IdempotencyKey, ObjectKind, PrincipalId,
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static TEMP: AtomicU64 = AtomicU64::new(1);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "layrs-cataloged-cas-{}-{}",
            std::process::id(),
            TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn object_id(byte: char) -> ObjectId {
    ObjectId::parse(format!("blake3:{}", byte.to_string().repeat(64))).unwrap()
}

fn server_view() -> RepositoryView {
    RepositoryView::new(
        SpaceId::parse("space:cataloged").unwrap(),
        vec![],
        vec![],
        vec![],
        vec![],
    )
    .unwrap()
}

fn authorization_snapshot() -> AuthorizationSnapshot {
    let mut snapshot = AuthorizationSnapshot {
        schema_version: AuthorizationSnapshotSchemaVersion::V1,
        authorization_snapshot_id: object_id('0'),
        space_id: SpaceId::parse("space:cataloged").unwrap(),
        subject: AuthorizationSubject::Principal {
            principal_id: PrincipalId::parse("principal:alice").unwrap(),
        },
        policy_set_id: object_id('1'),
        authorization_epoch: 7,
        request_binding: AuthorizationRequestBinding {
            path_patch_request_digest: None,
            client_id: ClientId::parse("client:cataloged-test").unwrap(),
            idempotency_key: IdempotencyKey::parse("cataloged-test-key-0001").unwrap(),
            request_digest: object_id('2'),
        },
        decisions: vec![],
        evaluated_at: Timestamp::parse("2026-07-20T10:00:00Z").unwrap(),
    };
    snapshot.authorization_snapshot_id = snapshot.computed_id().unwrap();
    snapshot
}

async fn filesystem_cataloged_cas(root: &TestDir) -> CatalogedFsCas {
    let inner = FsServerCas::open(&root.0, CasLimits::default())
        .await
        .unwrap();
    CatalogedFsCas::new(StoreId::parse("store:fs-cataloged").unwrap(), inner)
}

#[tokio::test]
async fn generated_view_and_authorization_snapshot_are_exact_and_replay_stably() {
    let root = TestDir::new();
    let cas = filesystem_cataloged_cas(&root).await;

    let view_bytes = Bytes::from(serde_jcs::to_vec(&server_view()).unwrap());
    let (left, right) = tokio::join!(
        cas.prepare_generated(ObjectKind::RepositoryView, view_bytes.clone()),
        cas.prepare_generated(ObjectKind::RepositoryView, view_bytes.clone())
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left, right);
    assert_eq!(left.store_id, "store:fs-cataloged");
    assert!(left.storage_key.starts_with("objects/repository_view/"));
    assert_eq!(left.backend_etag, None);
    assert_eq!(cas.verify_existing(&left.descriptor).await.unwrap(), left);

    let snapshot = authorization_snapshot();
    let snapshot_bytes = Bytes::from(serde_jcs::to_vec(&snapshot).unwrap());
    let prepared = cas
        .prepare_generated(ObjectKind::AuthorizationSnapshot, snapshot_bytes)
        .await
        .unwrap();
    assert_eq!(
        prepared.descriptor.object_id,
        snapshot.authorization_snapshot_id
    );
    assert!(
        prepared
            .storage_key
            .starts_with("objects/authorization_snapshot/")
    );
}

#[tokio::test]
async fn generated_records_reject_non_jcs_identity_kind_and_local_worktrees() {
    let root = TestDir::new();
    let cas = filesystem_cataloged_cas(&root).await;

    let pretty = serde_json::to_vec_pretty(&server_view()).unwrap();
    assert!(matches!(
        cas.prepare_generated(ObjectKind::RepositoryView, Bytes::from(pretty))
            .await,
        Err(V3StoreError::CorruptObject(_))
    ));

    let snapshot = authorization_snapshot();
    let snapshot_bytes = Bytes::from(serde_jcs::to_vec(&snapshot).unwrap());
    assert!(matches!(
        cas.prepare_generated(ObjectKind::Step, snapshot_bytes)
            .await,
        Err(V3StoreError::InvalidDescriptor(_))
    ));

    let worktree = WorktreeViewState::new(
        WorktreeId::parse("worktree:local-only").unwrap(),
        None,
        None,
        None,
        WorktreeStatus::Ready,
    );
    let local_view = RepositoryView::new(
        SpaceId::parse("space:cataloged").unwrap(),
        vec![],
        vec![],
        vec![],
        vec![worktree],
    )
    .unwrap();
    assert!(matches!(
        cas.prepare_generated(
            ObjectKind::RepositoryView,
            Bytes::from(serde_jcs::to_vec(&local_view).unwrap())
        )
        .await,
        Err(V3StoreError::CorruptObject(_))
    ));

    let mut wrong_identity = authorization_snapshot();
    wrong_identity.authorization_snapshot_id = object_id('f');
    assert!(matches!(
        cas.prepare_generated(
            ObjectKind::AuthorizationSnapshot,
            Bytes::from(serde_jcs::to_vec(&wrong_identity).unwrap())
        )
        .await,
        Err(V3StoreError::CorruptObject(_)) | Err(V3StoreError::IdentityMismatch)
    ));
}

#[tokio::test]
async fn existing_verification_rejects_kind_digest_length_and_corruption() {
    let root = TestDir::new();
    let cas = filesystem_cataloged_cas(&root).await;
    let prepared = cas
        .prepare_generated(
            ObjectKind::RepositoryView,
            Bytes::from(serde_jcs::to_vec(&server_view()).unwrap()),
        )
        .await
        .unwrap();

    let mut wrong_kind = prepared.descriptor.clone();
    wrong_kind.kind = ObjectKind::Step;
    assert!(matches!(
        cas.verify_existing(&wrong_kind).await,
        Err(V3StoreError::NotFound)
    ));

    let mut wrong_digest = prepared.descriptor.clone();
    wrong_digest.object_id = object_id('e');
    assert!(matches!(
        cas.verify_existing(&wrong_digest).await,
        Err(V3StoreError::NotFound)
    ));

    let mut wrong_length = prepared.descriptor.clone();
    wrong_length.canonical_byte_len += 1;
    wrong_length.encoded_byte_len += 1;
    assert!(matches!(
        cas.verify_existing(&wrong_length).await,
        Err(V3StoreError::LengthMismatch { .. })
    ));

    let object_path = root.0.join(&prepared.storage_key);
    std::fs::write(object_path, b"corrupt").unwrap();
    assert!(matches!(
        cas.verify_existing(&prepared.descriptor).await,
        Err(V3StoreError::LengthMismatch { .. }) | Err(V3StoreError::CorruptObject(_))
    ));
}
