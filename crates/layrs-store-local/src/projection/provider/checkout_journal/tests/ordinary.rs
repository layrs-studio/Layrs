//! Pure Ordinary inventory DATA. No private admission or physical identity is
//! authenticated by converting the existing structural fixture here.
use super::*;
use crate::projection::provider::records::import::{AdmittedSourceBinding, ImportRowset};

const TOKEN: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
fn rebind(stage: &mut ProviderTransitionIntentRecord) {
    stage.plan.source_prefix_id = stage.plan.source_prefix.digest().unwrap();
    stage.destination.as_mut().unwrap().format.import_digest =
        Some(stage.plan.source_prefix_id.clone());
    stage.reservation = Some(stage.reservation_operation().unwrap());
}
fn ordinary() -> Fixture {
    let mut fixture = Fixture::new();
    let provider = serde_json::to_value(&fixture.staging.plan.source_prefix.source).unwrap();
    fixture.staging.plan.source_prefix.source = serde_json::from_value(serde_json::json!({
        "kind":"ordinary", "identity":provider["identity"],
        "format_digest":TOKEN, "private_schema":9, "version":provider["version"],
        "checkout_identity":"fs:v1:0000000000000001:0000000000000200",
        "admission":AdmittedSourceBinding { admission_digest:TOKEN.into(), incoming_prefix_id:TOKEN.into(), incoming_receipt_id:TOKEN.into(), database_identity:"1:0:500".into() },
    })).unwrap();
    let prefix = &mut fixture.staging.plan.source_prefix;
    prefix.schema_version = "layrs.projection-provider-import-prefix.v2".into();
    prefix.closure.restore_history = Some(ImportRowset {
        count: 0,
        digest: TOKEN.into(),
    });
    prefix.inherited_segments.count = 1;
    prefix.ancestry_root_receipt = Some(TOKEN.into());
    prefix.closure.transition_receipts.count = 1;
    rebind(&mut fixture.staging);
    fixture.staging.validate_fields().unwrap();
    fixture
}

#[test]
fn ordinary_checkout_inventory_keeps_provider_and_ready_constructors_strict() {
    let f = ordinary();
    let expected =
        ExpectedCheckout::ordinary(&f.staging, &f.files, &f.deletions, IMAGE, None).unwrap();
    assert_eq!(expected.files(), f.files);
    assert_eq!(expected.header().prefix_id, f.staging.plan.source_prefix_id);
    assert_eq!(
        expected.header().destination_proof_digest,
        f.staging
            .destination
            .as_ref()
            .unwrap()
            .digest(&f.staging.plan)
            .unwrap()
    );
    assert!(ExpectedCheckout::new(&f.staging, &f.files, &f.deletions, IMAGE, None).is_err());
    let provider = Fixture::new();
    assert!(
        ExpectedCheckout::ordinary(
            &provider.staging,
            &provider.files,
            &provider.deletions,
            IMAGE,
            None
        )
        .is_err()
    );
    assert!(
        ExpectedCheckout::new(
            &provider.staging,
            &provider.files,
            &provider.deletions,
            IMAGE,
            None
        )
        .is_ok()
    );
    let mut ready = ProviderTransitionIntentRecord::decode(&f.staging.encode().unwrap()).unwrap();
    ready.state = TransitionState::Ready;
    ready.receipt_id = Some(TOKEN.into());
    ready.validate_fields().unwrap();
    assert!(ExpectedCheckout::ordinary(&ready, &f.files, &f.deletions, IMAGE, None).is_err());
    assert!(ExpectedCheckout::from_ready(&ready, &f.files, &f.deletions, IMAGE, None).is_err());
    // Provider terminal DATA remains accepted by its existing constructor.
    let mut ready =
        ProviderTransitionIntentRecord::decode(&provider.staging.encode().unwrap()).unwrap();
    ready.state = TransitionState::Ready;
    ready.receipt_id = Some(TOKEN.into());
    assert!(
        ExpectedCheckout::from_ready(&ready, &provider.files, &provider.deletions, IMAGE, None)
            .is_ok()
    );
}

#[test]
fn ordinary_checkout_inventory_refuses_unadmitted_sources_aliases_and_wrong_images() {
    for mutation in [
        "schema",
        "legacy-ordinary",
        "admission",
        "checkout-identity",
        "phase",
        "alias",
        "image",
        "executable",
        "leaf-alias",
    ] {
        let mut f = ordinary();
        let mut image = IMAGE.to_owned();
        match mutation {
            "schema" | "legacy-ordinary" | "admission" | "checkout-identity" => {
                let ImportSource::Ordinary {
                    private_schema,
                    admission,
                    checkout_identity,
                    ..
                } = &mut f.staging.plan.source_prefix.source
                else {
                    unreachable!()
                };
                match mutation {
                    "schema" => *private_schema = 8,
                    "legacy-ordinary" => {
                        *private_schema = 8;
                        *admission = None;
                    }
                    "admission" => *admission = None,
                    _ => *checkout_identity = None,
                }
                if mutation == "legacy-ordinary" {
                    // A canonical pre-admission Ordinary prefix is still not
                    // accepted by this explicitly private9 constructor.
                    rebind(&mut f.staging);
                    f.staging.validate_fields().unwrap();
                }
            }
            "phase" => {
                f.staging.state = TransitionState::Copying;
                f.staging.reservation = None;
            }
            "alias" => {
                f.staging.plan.destination_parent_identity = f
                    .staging
                    .destination
                    .as_ref()
                    .unwrap()
                    .format
                    .checkout_identity()
                    .into();
            }
            "image" => image = "b".repeat(64),
            "executable" => f.files[0].executable = true,
            "leaf-alias" => f.files[1].path = RepoPath::parse("DIR/A").unwrap(),
            _ => unreachable!(),
        }
        assert!(
            ExpectedCheckout::ordinary(&f.staging, &f.files, &f.deletions, &image, None).is_err(),
            "accepted {mutation}"
        );
    }
    let f = ordinary();
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert!(matches!(
        ExpectedCheckout::ordinary(&f.staging, &f.files, &f.deletions, IMAGE, Some(&cancelled)),
        Err(ProviderBootstrapCause::Vfs(layrs_vfs::Error::Cancelled))
    ));
}
