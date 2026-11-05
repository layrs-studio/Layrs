//! Audited SQL recipes; no live authority or destination acceptance evidence.
use super::*;
use crate::projection::{
    provider::{
        database_io::{BareConnection, with_reference},
        process_profile::tests::child,
        records::{denied_coverage::TransitionDeniedCoverageRecord, import::ImportRowset},
        transition_sql::coverage_tests::planned_with_coverage,
        validation::{self, active_fixture},
    },
    retention,
};
use layrs_core::{
    projection::PathSemantics,
    scm::{StepId, TreeId},
};
use rusqlite::params;

#[test]
#[ignore = "owned audited Windows SQLite process; complete source denial inventory"]
fn source_denial_reader_binds_namespace_and_legacy_modes_keep_empty_guards() {
    child::run(
        concat!(
            module_path!(),
            "::source_denial_reader_binds_namespace_and_legacy_modes_keep_empty_guards"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            let (f, _, _, activation, activated) = active_fixture(c, false);
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            let (intent, coverage) = planned_with_coverage(c, &f, &activation, &activated);
            let reference = coverage.record().reference().unwrap();
            crate::projection::provider::denied_coverage_storage::insert_exact(
                c, &coverage, &reference, None,
            )
            .unwrap();
            c.execute(
            "INSERT INTO projection_provider_transitions(id,role,state,local_generation,reservation,receipt_id,payload,digest) VALUES(?1,'source','planned',?2,NULL,NULL,?3,?4)",
            params![intent.transaction_id, intent.local_generation, intent.encode().unwrap(), intent.digest().unwrap()],
        ).unwrap();
            let read = || {
                validation::private_active_transition_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &activation.binding,
                    None,
                    None,
                )
            };
            assert_eq!(read().unwrap().transitions[0], intent);
            assert!(
                validation::private_active_publication_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &activation.binding,
                    None,
                    None,
                )
                .is_ok()
            );
            c.execute_batch("COMMIT; PRAGMA query_only=ON; BEGIN DEFERRED")
                .unwrap();
            let before = c.total_changes();
            assert_eq!(read().unwrap().transitions[0], intent);
            assert_eq!(c.total_changes(), before);
            c.execute_batch("ROLLBACK; PRAGMA query_only=OFF; BEGIN IMMEDIATE")
                .unwrap();
            for field in 0..4 {
                c.execute_batch("SAVEPOINT relabelled_denial").unwrap();
                let mut body =
                    TransitionDeniedCoverageRecord::decode(&coverage.record().encode().unwrap())
                        .unwrap();
                let mut linked =
                    ProviderTransitionIntentRecord::decode(&intent.encode().unwrap()).unwrap();
                let different = format!("blake3:{}", "2".repeat(64));
                match field {
                    0 => {
                        body.path_semantics = match body.path_semantics {
                            PathSemantics::CaseSensitive => PathSemantics::PortableWindows,
                            PathSemantics::PortableWindows => PathSemantics::CaseSensitive,
                        }
                    }
                    1 => {
                        body.step_id = StepId::parse(if body.step_id.as_str() == different {
                            "blake3:3333333333333333333333333333333333333333333333333333333333333333"
                        } else {
                            &different
                        }).unwrap();
                        linked.plan.observation.step_id = body.step_id.clone();
                    }
                    2 => {
                        body.root_tree_id = TreeId::parse(if body.root_tree_id.as_str() == different {
                            "blake3:3333333333333333333333333333333333333333333333333333333333333333"
                        } else {
                            &different
                        }).unwrap();
                        linked.plan.observation.root_tree_id = body.root_tree_id.clone();
                    }
                    _ => {
                        linked.plan.source_prefix.schema_version =
                            "layrs.projection-provider-import-prefix.v3".into();
                        linked.plan.source_prefix.closure.restore_history = Some(ImportRowset {
                            count: 0,
                            digest: different.clone(),
                        });
                        linked.plan.source_prefix.closure.denial_observations =
                            Some(ImportRowset {
                                count: 0,
                                digest: different.clone(),
                            });
                        linked.plan.source_prefix_id = linked.plan.source_prefix.digest().unwrap();
                        body.source_prefix_id = linked.plan.source_prefix_id.clone();
                    }
                }
                linked.plan.denial_coverage = Some(body.reference().unwrap());
                // Both canonical links are internally valid; actual source context
                // must still refuse relabelling and unsupported incoming scopes.
                body.require_intent(&linked, None).unwrap();
                c.execute(
                    "UPDATE projection_observations SET digest=?1,payload=?2",
                    params![body.digest().unwrap(), body.encode().unwrap()],
                )
                .unwrap();
                c.execute(
                    "UPDATE projection_provider_transitions SET payload=?1,digest=?2",
                    params![linked.encode().unwrap(), linked.digest().unwrap()],
                )
                .unwrap();
                assert!(read().is_err(), "accepted relabelled field {field}");
                c.execute_batch("ROLLBACK TO relabelled_denial; RELEASE relabelled_denial")
                    .unwrap();
            }
            c.execute_batch("SAVEPOINT legacy_guard").unwrap();
            let mut legacy =
                ProviderTransitionIntentRecord::decode(&intent.encode().unwrap()).unwrap();
            legacy.schema_version = "layrs.projection-provider-transition-intent.v1".into();
            legacy.plan.denial_coverage = None;
            c.execute(
                "UPDATE projection_provider_transitions SET payload=?1,digest=?2",
                params![legacy.encode().unwrap(), legacy.digest().unwrap()],
            )
            .unwrap();
            assert!(read().is_err()); // v1-only history cannot own the retained body.
            c.execute_batch("DELETE FROM projection_observations")
                .unwrap();
            assert_eq!(read().unwrap().transitions[0], legacy);
            c.execute_batch("ROLLBACK TO legacy_guard; RELEASE legacy_guard; SAVEPOINT observations_only; DELETE FROM projection_provider_transitions").unwrap();
            assert!(read().is_err()); // a canonical unowned observation is refused.
            assert!(
                validation::private_active_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &activation.binding,
                    None,
                    None
                )
                .is_err()
            );
            assert!(
                validation::private_active_capture_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &activation.binding,
                    None,
                    None
                )
                .is_err()
            );
            c.execute_batch("ROLLBACK TO observations_only; RELEASE observations_only; ROLLBACK")
                .unwrap();
            bare.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; denial SQL preflight bounds"]
fn source_denial_preflight_bounds_keys_count_and_wire_without_small_total_cap() {
    child::run(
        concat!(
            module_path!(),
            "::source_denial_preflight_bounds_keys_count_and_wire_without_small_total_cap"
        ),
        120,
        || {
            child::declare();
            with_reference(retention::SQL, |c| {
            c.execute_batch("BEGIN IMMEDIATE")?;
            let id = format!("blake3:{}", "1".repeat(64));
            // Shape preflight does not decode these synthetic bytes. A document
            // larger than the legacy 16MiB transition aggregate remains allowed.
            c.execute("INSERT INTO projection_observations(digest,payload) VALUES(?1,zeroblob(?2))", params![id, 16 * 1024 * 1024 + 1])?;
            assert_eq!(preflight(c, None)?, 1);
            c.execute_batch("DELETE FROM projection_observations; PRAGMA ignore_check_constraints=ON")?;
            for shape in 0..4 {
                c.execute_batch("SAVEPOINT bad_shape")?;
                match shape {
                    0 => { c.execute("INSERT INTO projection_observations(digest,payload) VALUES('invalid',x'01')", [])?; }
                    1 => {
                        // STRICT type enforcement remains active even when
                        // CHECK constraints are disabled in this owned fixture.
                        assert!(c.execute("INSERT INTO projection_observations(digest,payload) VALUES(?1,'text')", [&id]).is_err());
                        assert_eq!(preflight(c, None)?, 0);
                    }
                    2 => { c.execute("INSERT INTO projection_observations(digest,payload) VALUES(?1,x'')", [&id])?; }
                    _ => { c.execute("INSERT INTO projection_observations(digest,payload) VALUES(?1,zeroblob(?2))", params![id, TransitionDeniedCoverageRecord::MAX_BYTES as u64 + 1])?; }
                }
                if shape != 1 {
                    assert!(preflight(c, None).is_err());
                }
                c.execute_batch("ROLLBACK TO bad_shape; RELEASE bad_shape")?;
            }
            c.execute_batch("PRAGMA ignore_check_constraints=OFF; WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<4097) INSERT INTO projection_observations(digest,payload) SELECT 'blake3:'||printf('%064x',x),x'01' FROM n")?;
            let error = preflight(c, None).unwrap_err();
            assert!(error.to_string().contains("row count"));
            let cancel = Cancellation::default();
            cancel.cancel();
            assert!(matches!(preflight(c, Some(&cancel)), Err(crate::projection::provider::recovery::error::ProviderBootstrapCause::Vfs(layrs_vfs::Error::Cancelled))));
            c.execute_batch("ROLLBACK")?;
            Ok(())
        }).unwrap();
        },
    );
}
