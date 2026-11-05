//! Actual source interpretation and full ingress readers; scalar SQL fixtures
//! confer no native destination, family reservation, Ready or remote authority.
use super::*;
use crate::projection::provider::{destination_coverage, transition_sql};

#[test]
#[ignore = "owned SQL: atomic current document ingress, prefix copy and staging"]
fn current_document_ingress_copy_staging_and_no_repair_replay_are_exact() {
    child::run(
        concat!(
            module_path!(),
            "::current_document_ingress_copy_staging_and_no_repair_replay_are_exact"
        ),
        120,
        || {
            child::declare();
            let source = BareConnection::reference().unwrap();
            let s = source.connection();
            let mut data = inputs(s);
            s.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
            let active = private_active_capture_with_source(
                s,
                &data.fixture.allocation,
                &data.fixture.prepared,
                &data.activation.binding,
                Some([0x41; 16]),
                None,
            )
            .unwrap();
            let (planned, coverage) = transition_sql::coverage_tests::planned_with_coverage(
                s,
                &data.fixture,
                &data.activation,
                &active.active.activated,
            );
            data.prefix =
                ProviderImportPrefixRecord::decode(&planned.plan.source_prefix.encode().unwrap())
                    .unwrap();
            data.marker.identity = data.prefix.destination_identity.clone();
            data.marker.checkout_root = format!(
                "{}\\{}",
                planned.plan.destination_parent, planned.plan.destination_leaf_nonce
            );
            data.marker.import_digest = Some(data.prefix.digest().unwrap());
            let mut destination_proof = data.intent.destination.take().unwrap();
            destination_proof.format = data.marker.clone();
            data.intent =
                ProviderTransitionIntentRecord::decode(&planned.encode().unwrap()).unwrap();
            data.intent.role = TransitionRole::Destination;
            data.intent.state = TransitionState::Copying;
            data.intent.local_generation = 0;
            data.intent.destination = Some(destination_proof);
            let prefix_id = data.prefix.digest().unwrap();
            let proof = verify_provider_prefix(s, &mut data);
            assert_eq!(data.prefix.digest().unwrap(), prefix_id);
            transition_sql::apply_source_intent_with_coverage(
                s,
                &data.fixture.allocation,
                &data.fixture.prepared,
                &data.activation.binding,
                [0x41; 16],
                None,
                &planned,
                Some(&coverage),
                None,
            )
            .unwrap();
            // Persist the actual source Copying row with the same complete C
            // proof DATA before obtaining the sealed source transport scope.
            let mut source_copying =
                ProviderTransitionIntentRecord::decode(&data.intent.encode().unwrap()).unwrap();
            source_copying.role = TransitionRole::Source;
            source_copying.local_generation = planned.local_generation;
            transition_sql::apply_source_intent_with_coverage(
                s,
                &data.fixture.allocation,
                &data.fixture.prepared,
                &data.activation.binding,
                [0x41; 16],
                Some(&planned),
                &source_copying,
                Some(&coverage),
                None,
            )
            .unwrap();
            let current = destination_coverage::read_source(
                s,
                &data.fixture.allocation,
                &data.fixture.prepared,
                &data.activation.binding,
                [0x41; 16],
                &source_copying,
                None,
            )
            .unwrap();
            let destination = BareConnection::reference().unwrap();
            let d = destination.connection();
            d.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
            // A different codec-valid physical-proof DATA must be refused by
            // the source/destination link before the first fresh C DDL.
            let mut wrong =
                ProviderTransitionIntentRecord::decode(&data.intent.encode().unwrap()).unwrap();
            wrong.destination.as_mut().unwrap().format_file_identity = "1:0:999".into();
            wrong.validate_fields().unwrap();
            let before = d.total_changes();
            let error = initialize_empty_with_current(
                d,
                &data.marker,
                &data.fixture.source,
                &data.fixture.allocation,
                &data.fixture.prepared,
                &data.activation,
                &data.prefix,
                &wrong,
                &current,
                None,
            )
            .err()
            .expect("mismatched source destination proof initialized C");
            assert!(matches!(error,
                crate::projection::provider::recovery::error::ProviderBootstrapCause::Validation(ref cause)
                if matches!(cause.as_ref(),crate::projection::ProjectionStoreError::Invalid(message)
                    if message=="current coverage requires unreserved destination Copying with matching destination proof")
            ));
            assert_eq!(d.total_changes(), before);
            assert!(database_io::schema_inventory(d).unwrap().is_empty());
            let initialize_current = || {
                initialize_empty_with_current(
                    d,
                    &data.marker,
                    &data.fixture.source,
                    &data.fixture.allocation,
                    &data.fixture.prepared,
                    &data.activation,
                    &data.prefix,
                    &data.intent,
                    &current,
                    None,
                )
            };
            let before = d.total_changes();
            assert!(
                initialize(d, &data).is_err(),
                "legacy initializer must not fabricate current body"
            );
            assert_eq!(d.total_changes(), before);
            assert!(database_io::schema_inventory(d).unwrap().is_empty());
            initialize_current().unwrap();
            inspect(d, &data).unwrap();
            assert_eq!(
                destination_coverage::read_current(d, &data.intent, None)
                    .unwrap()
                    .as_ref(),
                Some(coverage.record())
            );
            for damaged in ["missing", "substituted", "extra"] {
                d.execute_batch("SAVEPOINT corrupted").unwrap();
                match damaged {
                    "missing" => {
                        d.execute("DELETE FROM projection_observations", [])
                            .unwrap();
                    }
                    "substituted" => {
                        d.execute("UPDATE projection_observations SET payload=x'7b7d'", [])
                            .unwrap();
                    }
                    _ => {
                        d.execute(
                            "INSERT INTO projection_observations VALUES(?1,x'7b7d')",
                            [TOKEN],
                        )
                        .unwrap();
                    }
                }
                let before = d.total_changes();
                assert!(inspect(d, &data).is_err(), "accepted {damaged}");
                assert_eq!(d.total_changes(), before);
                d.execute_batch("ROLLBACK TO corrupted; RELEASE corrupted")
                    .unwrap();
            }
            let promote = || {
                promote_or_replay_copied(
                    s,
                    d,
                    &data.marker,
                    &data.fixture.source,
                    &data.fixture.allocation,
                    &data.fixture.prepared,
                    &data.activation,
                    &data.prefix,
                    &data.intent,
                    &proof,
                    None,
                )
            };
            promote().unwrap();
            let before = d.total_changes();
            promote().unwrap();
            assert_eq!(d.total_changes(), before);
            for damaged in ["missing", "extra"] {
                d.execute_batch("SAVEPOINT copied_corruption").unwrap();
                if damaged == "missing" {
                    d.execute("DELETE FROM projection_observations", [])
                        .unwrap();
                } else {
                    d.execute(
                        "INSERT INTO projection_observations VALUES(?1,x'7b7d')",
                        [TOKEN],
                    )
                    .unwrap();
                }
                let before = d.total_changes();
                assert!(promote().is_err(), "copied replay accepted {damaged}");
                assert_eq!(d.total_changes(), before, "replay repaired {damaged}");
                d.execute_batch("ROLLBACK TO copied_corruption; RELEASE copied_corruption")
                    .unwrap();
            }
            let mut staging =
                ProviderTransitionIntentRecord::decode(&data.intent.encode().unwrap()).unwrap();
            staging.state = TransitionState::Staging;
            staging.reservation = Some(staging.reservation_operation().unwrap());
            let stage = || {
                stage_copied(
                    s,
                    d,
                    &data.marker,
                    &data.fixture.source,
                    &data.fixture.allocation,
                    &data.fixture.prepared,
                    &data.activation,
                    &data.prefix,
                    &data.intent,
                    &staging,
                    &proof,
                    None,
                )
            };
            assert_eq!(stage().unwrap(), CopiedStagingEffect::Updated);
            let before = d.total_changes();
            assert_eq!(stage().unwrap(), CopiedStagingEffect::AlreadyPresent);
            assert_eq!(d.total_changes(), before);
            assert_eq!(
                destination_coverage::read_current(d, &staging, None)
                    .unwrap()
                    .as_ref(),
                Some(coverage.record())
            );
            d.execute_batch("ROLLBACK").unwrap();
            assert!(database_io::schema_inventory(d).unwrap().is_empty());
            d.execute_batch("BEGIN IMMEDIATE").unwrap();
            initialize_current().unwrap();
            inspect(d, &data).unwrap();
            d.execute_batch("ROLLBACK").unwrap();
            drop(current);
            s.execute_batch("ROLLBACK").unwrap();
            destination.finish(Ok(())).unwrap();
            source.finish(Ok(())).unwrap();
        },
    );
}
