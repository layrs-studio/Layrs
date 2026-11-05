//! Audited scalar source SQL recipes only. No producer, destination or native
//! authority is implied by these normalized DATA fixtures.
use super::*;
use crate::projection::{
    VersionedProjection,
    provider::{
        database_io::BareConnection,
        process_profile::tests::child,
        records::{
            ProviderSourceDescriptor, activation::ProviderActivationRecord,
            denied_coverage::TransitionDeniedCoverageRecord,
        },
        transition_observation::{
            AuthorityObservationData, DeniedPathData, ObservationCoverageData,
            SourceObservationData, TransitionObservationData,
        },
        validation::{active_fixture, tests::fixture::Fixture},
    },
};
use layrs_core::scm::RepoPath;

pub(in crate::projection::provider) fn planned_with_coverage(
    c: &Connection,
    fixture: &Fixture,
    activation: &ProviderActivationRecord,
    activated: &VersionedProjection,
) -> (ProviderTransitionIntentRecord, DerivedDeniedCoverage) {
    let mut intent = super::tests::planned(c, fixture, activation, activated);
    let mut fresh = fixture.source.initial().clone();
    fresh.observed_authorization_epoch += 1;
    let authority = AuthorityObservationData::ReobservedExpandedScope {
        policy_set_id: fresh.observed_policy_set_id.clone(),
        authorization_epoch: fresh.observed_authorization_epoch,
    };
    let normalized = TransitionObservationData::normalize(
        SourceObservationData {
            prefix: &intent.plan.source_prefix,
            identity: &fixture.allocation.format.identity,
            snapshot: &activated.snapshot,
            snapshot_parent_digest: Some(&activation.source_digest),
            selection: fixture.source.selection(),
            selection_generation: 0,
            selection_snapshot_digest: &activation.source_digest,
        },
        intent.plan.source_prefix.destination_identity.clone(),
        activated.generation + 1,
        fresh,
        fixture.source.selection().clone(),
        fixture
            .source
            .descriptors()
            .iter()
            .map(|row| ProviderSourceDescriptor {
                path: row.path.clone(),
                manifest: row.manifest.clone(),
            })
            .collect(),
        ObservationCoverageData {
            denied_paths: vec![DeniedPathData {
                path: RepoPath::parse("secret.txt").unwrap(),
            }],
        },
        authority,
        None,
    )
    .unwrap();
    let coverage = DerivedDeniedCoverage::from_observation(
        &intent.plan.source_prefix,
        &intent.transaction_id,
        &normalized,
        None,
    )
    .unwrap();
    let body = coverage.record();
    intent.schema_version = "layrs.projection-provider-transition-intent.v2".into();
    intent.plan.denial_coverage = Some(body.reference().unwrap());
    intent.plan.observation.step_id = body.step_id.clone();
    intent.plan.observation.root_tree_id = body.root_tree_id.clone();
    intent.plan.observation.policy_set_id = normalized.snapshot().observed_policy_set_id.clone();
    intent.plan.observation.authorization_epoch =
        normalized.snapshot().observed_authorization_epoch;
    intent.plan.observation.generation = body.observation_generation;
    intent.plan.observation.snapshot_digest = body.observation_snapshot_digest.clone();
    intent.plan.observation.selection_digest = body.observation_selection_digest.clone();
    intent.validate_fields().unwrap();
    (intent, coverage)
}

#[test]
#[ignore = "owned audited Windows SQLite process; source v2 Planned and denial body"]
fn source_coverage_planned_insert_replay_phase_and_rollback_are_exact() {
    child::run(
        concat!(
            module_path!(),
            "::source_coverage_planned_insert_replay_phase_and_rollback_are_exact"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            let (f, _, _, activation, activated) = active_fixture(c, false);
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            let (initial, coverage) = planned_with_coverage(c, &f, &activation, &activated);
            let apply = |previous, next, body| {
                apply_source_intent_with_coverage(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &activation.binding,
                    [0x41; 16],
                    previous,
                    next,
                    body,
                    None,
                )
            };
            let before = c.total_changes();
            assert!(apply(None, &initial, None).is_err());
            let mut substituted =
                ProviderTransitionIntentRecord::decode(&initial.encode().unwrap()).unwrap();
            substituted.plan.denial_coverage.as_mut().unwrap().id =
                format!("blake3:{}", "2".repeat(64));
            assert!(apply(None, &substituted, Some(&coverage)).is_err());
            assert_eq!(c.total_changes(), before);
            assert_eq!(
                apply(None, &initial, Some(&coverage)).unwrap(),
                SourceIntentEffect::Inserted
            );
            assert_eq!(c.total_changes(), before + 2);
            let before = c.total_changes();
            assert_eq!(
                apply(None, &initial, Some(&coverage)).unwrap(),
                SourceIntentEffect::AlreadyPresent
            );
            // Existing rows use their actual retained body, with no supplied repair.
            assert_eq!(
                apply(None, &initial, None).unwrap(),
                SourceIntentEffect::AlreadyPresent
            );
            assert_eq!(c.total_changes(), before);
            let mut cancelled =
                ProviderTransitionIntentRecord::decode(&initial.encode().unwrap()).unwrap();
            cancelled.state = TransitionState::Cancelled;
            assert_eq!(
                apply(Some(&initial), &cancelled, None).unwrap(),
                SourceIntentEffect::Updated
            );
            let before = c.total_changes();
            assert_eq!(
                apply(Some(&initial), &cancelled, None).unwrap(),
                SourceIntentEffect::AlreadyPresent
            );
            assert_eq!(c.total_changes(), before);
            let retained = validation::private_active_transition_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &activation.binding,
                None,
                None,
            )
            .unwrap();
            assert_eq!(
                retained.transitions.as_slice(),
                std::slice::from_ref(&cancelled)
            );
            let funding =
                crate::projection::shared::source_denial::SourceDeniedCoverageReservation::new(
                    &f.allocation,
                    &initial,
                    coverage.record(),
                    None,
                )
                .unwrap();
            assert_eq!(
                retained.capture.denial_reservations(),
                std::slice::from_ref(&funding)
            );
            // Exercise the same classifier used by the complete family reader.
            // Both databases are disposable SQL references; this proves no
            // filesystem authority, cross-database atomicity or close ordering.
            let family = BareConnection::reference().unwrap();
            let catalog = family.connection();
            crate::projection::shared::schema_v2::initialize_test_reference(catalog).unwrap();
            catalog.execute_batch("BEGIN IMMEDIATE").unwrap();
            catalog
                .execute("INSERT INTO family VALUES(1,?1)", [b"family".as_slice()])
                .unwrap();
            catalog
                .execute(
                    "INSERT INTO quota VALUES(1,?1)",
                    [funding.reservation_bytes()],
                )
                .unwrap();
            catalog
                .execute(
                    "INSERT INTO worktrees VALUES(?1,'source',?2,?3,'ready')",
                    params![
                        f.allocation.format.identity.worktree_id.as_str(),
                        f.allocation.private.database_file_identity,
                        b"source".as_slice()
                    ],
                )
                .unwrap();
            let classify = || {
                crate::projection::shared::provider_access::provider_capture::classify_denial_reservations(
                catalog, &f.allocation, &retained.capture, None,
            )
            };
            let before = catalog.total_changes();
            assert!(classify().is_err());
            assert_eq!(catalog.total_changes(), before);
            catalog.execute_batch("SAVEPOINT funding").unwrap();
            funding.reserve(catalog, None).unwrap();
            let before = catalog.total_changes();
            assert_eq!(
                classify().unwrap(),
                std::collections::BTreeSet::from([funding.operation().to_owned()])
            );
            assert_eq!(catalog.total_changes(), before);
            catalog
                .execute("UPDATE reservations SET remaining=remaining-1", [])
                .unwrap();
            let before = catalog.total_changes();
            assert!(classify().is_err());
            assert_eq!(catalog.total_changes(), before);
            catalog
                .execute_batch("ROLLBACK TO funding; RELEASE funding")
                .unwrap();
            assert!(classify().is_err());
            catalog.execute_batch("ROLLBACK").unwrap();
            family.finish(Ok(())).unwrap();
            c.execute_batch("ROLLBACK; BEGIN IMMEDIATE").unwrap();
            let counts: (u64, u64) = c.query_row(
            "SELECT (SELECT count(*) FROM projection_provider_transitions),(SELECT count(*) FROM projection_observations)", [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
            assert_eq!(counts, (0, 0));
            assert_eq!(
                apply(None, &initial, Some(&coverage)).unwrap(),
                SourceIntentEffect::Inserted
            );
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; source denial replay never repairs"]
fn source_coverage_missing_substituted_and_extra_body_refuse_before_replay() {
    child::run(
        concat!(
            module_path!(),
            "::source_coverage_missing_substituted_and_extra_body_refuse_before_replay"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            let (f, _, _, activation, activated) = active_fixture(c, false);
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            let (intent, coverage) = planned_with_coverage(c, &f, &activation, &activated);
            let apply = || {
                apply_source_intent_with_coverage(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &activation.binding,
                    [0x41; 16],
                    None,
                    &intent,
                    Some(&coverage),
                    None,
                )
            };
            apply().unwrap();
            let reference = coverage.record().reference().unwrap();
            for corruption in 0..3 {
                c.execute_batch("SAVEPOINT damaged_denial").unwrap();
                match corruption {
                    0 => {
                        c.execute(
                            "DELETE FROM projection_observations WHERE digest=?1",
                            [&reference.id],
                        )
                        .unwrap();
                    }
                    1 => {
                        let mut other = TransitionDeniedCoverageRecord::decode(
                            &coverage.record().encode().unwrap(),
                        )
                        .unwrap();
                        other.transaction_id = "00000000000000000000000000000003".into();
                        c.execute(
                            "UPDATE projection_observations SET payload=?1 WHERE digest=?2",
                            params![other.encode().unwrap(), reference.id],
                        )
                        .unwrap();
                    }
                    _ => {
                        let mut other = TransitionDeniedCoverageRecord::decode(
                            &coverage.record().encode().unwrap(),
                        )
                        .unwrap();
                        other.transaction_id = "00000000000000000000000000000003".into();
                        c.execute(
                            "INSERT INTO projection_observations(digest,payload) VALUES(?1,?2)",
                            params![other.digest().unwrap(), other.encode().unwrap()],
                        )
                        .unwrap();
                    }
                }
                let before = c.total_changes();
                assert!(apply().is_err());
                assert_eq!(c.total_changes(), before);
                c.execute_batch("ROLLBACK TO damaged_denial; RELEASE damaged_denial")
                    .unwrap();
            }
            assert_eq!(apply().unwrap(), SourceIntentEffect::AlreadyPresent);
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; source denial transaction gates"]
fn source_coverage_cancellation_fk_and_main_write_state_have_no_effects() {
    child::run(
        concat!(
            module_path!(),
            "::source_coverage_cancellation_fk_and_main_write_state_have_no_effects"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            let (f, _, _, activation, activated) = active_fixture(c, false);
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            let (intent, coverage) = planned_with_coverage(c, &f, &activation, &activated);
            c.execute_batch("ROLLBACK").unwrap();
            let apply = |cancel| {
                apply_source_intent_with_coverage(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &activation.binding,
                    [0x41; 16],
                    None,
                    &intent,
                    Some(&coverage),
                    cancel,
                )
            };
            let before = c.total_changes();
            assert!(apply(None).is_err());
            for begin in ["BEGIN DEFERRED", "SAVEPOINT denial_outer"] {
                c.execute_batch(begin).unwrap();
                let error = apply(None).unwrap_err();
                assert!(error.to_string().contains("active main write transaction"));
                c.execute_batch("ROLLBACK").unwrap();
            }
            c.execute_batch("PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE")
                .unwrap();
            assert!(apply(None).is_err());
            c.execute_batch(
                "ROLLBACK; PRAGMA foreign_keys=ON; BEGIN IMMEDIATE; PRAGMA query_only=ON",
            )
            .unwrap();
            assert!(apply(None).is_err());
            c.execute_batch("PRAGMA query_only=OFF").unwrap();
            let cancel = Cancellation::default();
            cancel.cancel();
            assert!(matches!(
                apply(Some(&cancel)),
                Err(super::super::recovery::error::ProviderBootstrapCause::Vfs(
                    layrs_vfs::Error::Cancelled
                ))
            ));
            assert_eq!(c.total_changes(), before);
            assert_eq!(apply(None).unwrap(), SourceIntentEffect::Inserted);
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        },
    );
}
