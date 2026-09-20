use layrs_core::scm::*;
use layrs_store::scm::*;
use layrs_store_local::scm_v1::*;
use layrs_sync::v3::{Nullable, ProductEnvelope};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "layrs-products-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir(&root).unwrap();
        drop(init(&root, StoreId::parse("store:products").unwrap()).unwrap());
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.0.parent() == Some(std::env::temp_dir().as_path()) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn timestamp() -> Timestamp {
    Timestamp::parse("2026-09-20T12:00:00Z").unwrap()
}
fn space() -> Space {
    Space::new(
        SpaceId::parse("space:products").unwrap(),
        "products",
        timestamp(),
        Metadata::empty(),
    )
    .unwrap()
}
fn product() -> CanonicalProduct {
    let layer = Layer::new(
        LayerId::parse("layer:products").unwrap(),
        space().id().clone(),
        "main",
        LayerKind::Base,
        LayerStatus::Active,
        vec![],
        LayerSettings::new(true, 500).unwrap(),
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let mut envelope = ProductEnvelope::Layer {
        product_id: layer.id().clone(),
        product_object_id: ObjectId::parse(format!("blake3:{}", "0".repeat(64))).unwrap(),
        depends_on_step_ids: vec![],
        depends_on_product_ids: vec![],
        previous_product_object_id: Nullable(None),
        record: layer,
    };
    let id = envelope.computed_object_id().unwrap();
    if let ProductEnvelope::Layer {
        product_object_id, ..
    } = &mut envelope
    {
        *product_object_id = id;
    }
    CanonicalProduct::from_envelope(envelope).unwrap()
}
fn batch(product: CanonicalProduct) -> WriteBatch {
    let id = StoreObjectId::Product(product.id().clone());
    WriteBatch::new(
        TransactionId::new("transaction:product").unwrap(),
        vec![CanonicalObject::Product(product)],
        vec![EntityRecord::Space(space())],
        vec![SpaceObjectAdmission::new(space().id().clone(), id)],
        vec![],
    )
    .unwrap()
}
struct Fault(RepositoryFaultPoint);
impl RepositoryFaultInjector for Fault {
    fn check(&self, point: RepositoryFaultPoint) -> std::result::Result<(), String> {
        if point == self.0 {
            Err("injected Product transaction fault".into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn product_is_canonical_cas_admitted_replayable_and_retained_by_fsck() {
    let fixture = Fixture::new();
    let product = product();
    let batch = batch(product.clone());
    let mut repository = NativeScmRepository::new(&fixture.0);
    assert!(!repository.apply_batch(&batch).unwrap().was_replay());
    assert!(repository.apply_batch(&batch).unwrap().was_replay());
    assert_eq!(
        repository.get_product(product.id()).unwrap(),
        Some(product.clone())
    );
    assert!(
        repository
            .object_admitted_to_space(&StoreObjectId::Product(product.id().clone()), space().id())
            .unwrap()
    );
    let report = fsck_existing(&fixture.0, FsckOptions::default()).unwrap();
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.gc_reachability().reachable_objects, 1);
}

#[test]
fn product_metadata_rolls_back_and_commit_response_loss_replays_exactly() {
    for point in [
        RepositoryFaultPoint::AfterObjects,
        RepositoryFaultPoint::AfterAdmissions,
        RepositoryFaultPoint::AfterReceiptBeforeCommit,
        RepositoryFaultPoint::AfterCommit,
    ] {
        let fixture = Fixture::new();
        let product = product();
        let batch = batch(product.clone());
        let mut repository =
            NativeScmRepository::with_fault_injector(&fixture.0, Arc::new(Fault(point)));
        assert!(repository.apply_batch(&batch).is_err());
        let mut reopened = NativeScmRepository::new(&fixture.0);
        let committed = point == RepositoryFaultPoint::AfterCommit;
        assert_eq!(
            reopened.get_product(product.id()).unwrap().is_some(),
            committed
        );
        assert_eq!(
            reopened.apply_batch(&batch).unwrap().was_replay(),
            committed
        );
        assert_eq!(reopened.get_product(product.id()).unwrap(), Some(product));
    }
}

#[test]
fn product_rejects_noncanonical_bytes_and_wrong_identity() {
    let product = product();
    let mut bytes = product.canonical_json().to_vec();
    bytes.push(b'\n');
    assert!(CanonicalProduct::new(product.id().clone(), bytes).is_err());
    assert!(
        CanonicalProduct::new(
            ObjectId::parse(format!("blake3:{}", "1".repeat(64))).unwrap(),
            product.canonical_json()
        )
        .is_err()
    );
}

#[test]
fn previous_native_version_is_refused_without_mutation() {
    let fixture = Fixture::new();
    let marker = fixture.0.join(FORMAT_FILE_NAME);
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
    value["formatVersion"] = 1.into();
    value["minReaderVersion"] = 1.into();
    let bytes = canonical_json(&value).unwrap();
    fs::write(&marker, &bytes).unwrap();
    assert!(open(&fixture.0).is_err());
    assert_eq!(fs::read(&marker).unwrap(), bytes);
}

#[test]
fn product_admission_cannot_cross_spaces() {
    let fixture = Fixture::new();
    let product = product();
    let other = Space::new(
        SpaceId::parse("space:other").unwrap(),
        "other",
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let batch = WriteBatch::new(
        TransactionId::new("transaction:cross-space").unwrap(),
        vec![CanonicalObject::Product(product.clone())],
        vec![EntityRecord::Space(other.clone())],
        vec![SpaceObjectAdmission::new(
            other.id().clone(),
            StoreObjectId::Product(product.id().clone()),
        )],
        vec![],
    )
    .unwrap();
    let mut repository = NativeScmRepository::new(&fixture.0);
    assert!(repository.apply_batch(&batch).is_err());
    assert!(repository.get_product(product.id()).unwrap().is_none());
}
