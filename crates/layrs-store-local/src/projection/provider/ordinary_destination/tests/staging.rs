//! Combined caller-owned SQL DATA for ordinary SourceStaging and C Staging.
//! The fixture supplies real admitted-prefix interpretation, not filesystem
//! pins, funded metadata, a family reservation or permission to publish C.
use super::super::staging::{self as destination_staging, StageEffect};
use super::*;
use rusqlite::types::ValueRef;

const COPIED_TABLES: [&str; 13] = [
    "projection_snapshots",
    "projection_selections",
    "projection_captures",
    "projection_publications",
    "projection_provider_catalogs",
    "projection_provider_descriptors",
    "projection_provider_activations",
    "projection_provider_transition_receipts",
    "projection_provider_companions",
    "projection_provider_companion_files",
    "projection_provider_companion_deletions",
    "projection_content",
    "projection_restore_history",
];
const LOCAL_TABLES: [&str; 5] = [
    "projection_meta",
    "projection_head",
    "projection_backend",
    "projection_imports",
    "projection_provider_transitions",
];

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}
type Rows = Vec<Vec<Cell>>;

fn rows(c: &Connection, table: &str) -> Rows {
    assert!(COPIED_TABLES.contains(&table) || LOCAL_TABLES.contains(&table));
    let mut statement = c
        .prepare(&format!("SELECT * FROM \"{table}\" LIMIT 129"))
        .unwrap();
    let columns = statement.column_count();
    assert!(columns <= 32);
    let mut query = statement.query([]).unwrap();
    let mut result = Vec::new();
    let mut bytes = 0usize;
    while let Some(row) = query.next().unwrap() {
        assert!(result.len() < 128, "tiny staging fixture row bound");
        let mut cells = Vec::with_capacity(columns);
        for column in 0..columns {
            cells.push(match row.get_ref(column).unwrap() {
                ValueRef::Null => Cell::Null,
                ValueRef::Integer(value) => Cell::Integer(value),
                ValueRef::Real(value) => Cell::Real(value.to_bits()),
                ValueRef::Text(value) => {
                    bytes = bytes.checked_add(value.len()).unwrap();
                    assert!(bytes <= 16 * 1024 * 1024);
                    Cell::Text(value.to_vec())
                }
                ValueRef::Blob(value) => {
                    bytes = bytes.checked_add(value.len()).unwrap();
                    assert!(bytes <= 16 * 1024 * 1024);
                    Cell::Blob(value.to_vec())
                }
            });
        }
        result.push(cells);
    }
    result.sort();
    result
}

fn copied_rows(c: &Connection) -> Vec<(&'static str, Rows)> {
    COPIED_TABLES
        .into_iter()
        .map(|table| (table, rows(c, table)))
        .collect()
}

fn state(c: &Connection) -> Vec<(&'static str, Rows)> {
    COPIED_TABLES
        .into_iter()
        .chain(LOCAL_TABLES)
        .map(|table| (table, rows(c, table)))
        .collect()
}

fn ingress(c: &Connection, marker: &format::Marker) -> (Vec<u8>, Vec<u8>) {
    let admitted = read_admitted_import(c, marker, None).unwrap();
    (
        admitted.record().encode().unwrap(),
        admitted.ready().encode().unwrap(),
    )
}

fn incoming(source: &ProviderTransitionIntentRecord) -> ProviderTransitionIntentRecord {
    let mut result = ProviderTransitionIntentRecord::decode(&source.encode().unwrap()).unwrap();
    result.role = TransitionRole::Destination;
    result.local_generation = 0;
    result.validate_fields().unwrap();
    result
}

fn stored(c: &Connection, id: &str) -> Vec<u8> {
    c.query_row(
        "SELECT payload FROM projection_provider_transitions WHERE id=?1",
        [id],
        |row| row.get(0),
    )
    .unwrap()
}

owned!(
    ordinary_destination_staging_preserves_history_and_replays_across_source_gap,
    {
        with_admitted_source_sql(|source, marker, supplied| {
            let original_ingress = ingress(source, marker);
            let (copying, closure, target, history) = source_copying(source, marker, supplied);
            let copying_inputs = OrdinaryCopyInputs::new(source, marker, &copying, &closure);
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
            assert_eq!(
                initialize_or_replay_copied(c, &target, &copying_inputs, None)
                    .unwrap()
                    .1,
                CopyEffect::Copied
            );
            assert_eq!(
                c.query_row(
                    "SELECT payload FROM projection_restore_history WHERE operation_id=?1",
                    [OPERATION],
                    |row| row.get::<_, Vec<u8>>(0)
                )
                .unwrap(),
                history
            );
            let copied = copied_rows(c);
            assert_eq!(rows(c, "projection_restore_history").len(), 1);
            let source_before = state(source);
            let destination_before = state(c);
            let staging = ordinary_transition::proposed_staging(&copying).unwrap();
            assert_eq!(
                staging.reservation.as_deref(),
                Some(staging.reservation_operation().unwrap().as_str())
            );
            let staging_inputs = OrdinaryCopyInputs::new(source, marker, &staging, &closure);

            // A proposal is not durable SourceStaging, even when its canonical
            // reservation and every other immutable field are correct.
            let changes = (source.total_changes(), c.total_changes());
            assert!(
                destination_staging::read_stageable(c, &target, &staging_inputs, None).is_err()
            );
            assert!(destination_staging::apply_staging(c, &target, &staging_inputs, None).is_err());
            assert_eq!((source.total_changes(), c.total_changes()), changes);
            assert_eq!(state(source), source_before);
            assert_eq!(state(c), destination_before);

            source.execute_batch("SAVEPOINT source_stage").unwrap();
            c.execute_batch("SAVEPOINT destination_stage").unwrap();
            let source_changes = source.total_changes();
            assert_eq!(
                ordinary_transition::apply_staging(
                    source,
                    marker,
                    &copying.plan.source_prefix,
                    &staging,
                    None
                )
                .unwrap(),
                SourceIntentEffect::Updated
            );
            assert_eq!(source.total_changes(), source_changes + 1);
            assert_eq!(stored(source, TX), staging.encode().unwrap());
            assert_eq!(ingress(source, marker), original_ingress);
            assert_eq!(stored(c, TX), incoming(&copying).encode().unwrap());
            let changes = (source.total_changes(), c.total_changes());
            assert_eq!(
                destination_staging::read_stageable(c, &target, &staging_inputs, None).unwrap(),
                TransitionState::Copying
            );
            assert!(
                read_copied(c, &target, &copying_inputs, None).is_err(),
                "an old source phase cannot stand in for durable SourceStaging"
            );
            assert_eq!((source.total_changes(), c.total_changes()), changes);
            assert_eq!(copied_rows(c), copied);

            let destination_changes = c.total_changes();
            assert_eq!(
                destination_staging::apply_staging(c, &target, &staging_inputs, None).unwrap(),
                StageEffect::Updated
            );
            assert_eq!(c.total_changes(), destination_changes + 1);
            assert_eq!(stored(c, TX), incoming(&staging).encode().unwrap());
            assert_eq!(
                destination_staging::read_stageable(c, &target, &staging_inputs, None).unwrap(),
                TransitionState::Staging
            );
            assert_eq!(copied_rows(c), copied);
            assert_eq!(ingress(source, marker), original_ingress);
            assert!(ordinary_transition::has_live(source).unwrap());
            assert!(ordinary_transition::require_no_live(source).is_err());

            let changes = (source.total_changes(), c.total_changes());
            let both = (state(source), state(c));
            assert_eq!(
                ordinary_transition::apply_staging(
                    source,
                    marker,
                    &copying.plan.source_prefix,
                    &staging,
                    None
                )
                .unwrap(),
                SourceIntentEffect::AlreadyPresent
            );
            assert_eq!(
                destination_staging::apply_staging(c, &target, &staging_inputs, None).unwrap(),
                StageEffect::AlreadyPresent
            );
            assert!(ordinary_transition::cancel_planned(source, marker, TX, None).is_err());
            assert_eq!((source.total_changes(), c.total_changes()), changes);
            assert_eq!((state(source), state(c)), both);

            // Each database owns its rollback independently. Reverting C first
            // restores the legitimate gap; reverting B then restores Copying.
            c.execute_batch("ROLLBACK TO destination_stage; RELEASE destination_stage")
                .unwrap();
            assert_eq!(
                destination_staging::read_stageable(c, &target, &staging_inputs, None).unwrap(),
                TransitionState::Copying
            );
            assert_eq!(state(c), destination_before);
            source
                .execute_batch("ROLLBACK TO source_stage; RELEASE source_stage")
                .unwrap();
            assert_eq!(state(source), source_before);
            assert_eq!(stored(source, TX), copying.encode().unwrap());
            read_copied(c, &target, &copying_inputs, None).unwrap();
            assert_eq!(copied_rows(c), copied);
            assert_eq!(ingress(source, marker), original_ingress);
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        });
    }
);

owned!(
    ordinary_destination_staging_refuses_forged_inputs_and_partial_or_suffix_c,
    {
        with_admitted_source_sql(|source, marker, supplied| {
            let (copying, closure, target, _) = source_copying(source, marker, supplied);
            let copying_inputs = OrdinaryCopyInputs::new(source, marker, &copying, &closure);
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
            initialize_or_replay_copied(c, &target, &copying_inputs, None).unwrap();
            let staging = ordinary_transition::proposed_staging(&copying).unwrap();
            ordinary_transition::apply_staging(
                source,
                marker,
                &copying.plan.source_prefix,
                &staging,
                None,
            )
            .unwrap();
            let staging_inputs = OrdinaryCopyInputs::new(source, marker, &staging, &closure);
            let original_ingress = ingress(source, marker);
            for mutation in ["reservation", "plan", "proof"] {
                let mut forged =
                    ProviderTransitionIntentRecord::decode(&staging.encode().unwrap()).unwrap();
                match mutation {
                    "reservation" => {
                        let reservation = forged.reservation.as_mut().unwrap();
                        let last = reservation.pop().unwrap();
                        reservation.push(if last == '0' { '1' } else { '0' });
                        assert!(
                            forged.encode().is_err(),
                            "reservation is derived, not caller-selected"
                        );
                    }
                    "plan" => {
                        forged.plan.destination_leaf_nonce =
                            "000000000000000000000000000000a4".into();
                        forged.destination.as_mut().unwrap().format.checkout_root =
                            std::path::Path::new(&forged.plan.destination_parent)
                                .join(&forged.plan.destination_leaf_nonce)
                                .to_str()
                                .unwrap()
                                .into();
                        forged.encode().unwrap();
                    }
                    "proof" => {
                        forged.destination.as_mut().unwrap().format_file_identity =
                            "1:0:883".into();
                        forged.encode().unwrap();
                    }
                    _ => unreachable!(),
                }
                let wrong = OrdinaryCopyInputs::new(source, marker, &forged, &closure);
                let changes = (source.total_changes(), c.total_changes());
                let before = (state(source), state(c));
                assert!(
                    ordinary_transition::apply_staging(
                        source,
                        marker,
                        &copying.plan.source_prefix,
                        &forged,
                        None
                    )
                    .is_err(),
                    "source accepted {mutation}"
                );
                assert!(
                    destination_staging::read_stageable(c, &target, &wrong, None).is_err(),
                    "reader accepted {mutation}"
                );
                assert!(
                    destination_staging::apply_staging(c, &target, &wrong, None).is_err(),
                    "destination accepted {mutation}"
                );
                assert_eq!((source.total_changes(), c.total_changes()), changes);
                assert_eq!((state(source), state(c)), before);
                assert_eq!(ingress(source, marker), original_ingress);
            }
            for mutation in ["partial-prefix", "suffix-generation", "reservation-column"] {
                let before = state(c);
                c.execute_batch("SAVEPOINT incomplete_destination").unwrap();
                match mutation {
                    "partial-prefix" => {
                        c.execute("DELETE FROM projection_imports WHERE singleton=1", [])
                            .unwrap();
                    }
                    "suffix-generation" => {
                        let current =
                            offline::snapshot_at(source, copying.local_generation, None).unwrap();
                        let generation = current.generation + 1;
                        let payload = current.snapshot.canonical_bytes().unwrap();
                        let digest = database::digest(generation, Some(&current.digest), &payload);
                        c.execute(
                            "INSERT INTO projection_snapshots VALUES(?1,?2,?3,?4)",
                            params![generation, digest, current.digest, payload],
                        )
                        .unwrap();
                        selection::insert(
                            c,
                            generation,
                            &selection::at(source, copying.local_generation).unwrap(),
                        )
                        .unwrap();
                    }
                    "reservation-column" => {
                        c.execute(
                            "UPDATE projection_provider_transitions SET reservation=?1 WHERE id=?2",
                            params![staging.reservation, TX],
                        )
                        .unwrap();
                    }
                    _ => unreachable!(),
                }
                let changes = (source.total_changes(), c.total_changes());
                let altered = (state(source), state(c));
                assert!(
                    destination_staging::read_stageable(c, &target, &staging_inputs, None).is_err(),
                    "reader accepted {mutation}"
                );
                assert!(
                    destination_staging::apply_staging(c, &target, &staging_inputs, None).is_err(),
                    "writer accepted {mutation}"
                );
                assert_eq!((source.total_changes(), c.total_changes()), changes);
                assert_eq!((state(source), state(c)), altered);
                c.execute_batch(
                    "ROLLBACK TO incomplete_destination; RELEASE incomplete_destination",
                )
                .unwrap();
                assert_eq!(state(c), before);
                assert_eq!(
                    destination_staging::read_stageable(c, &target, &staging_inputs, None).unwrap(),
                    TransitionState::Copying
                );
            }
            assert_eq!(ingress(source, marker), original_ingress);
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        });
    }
);

owned!(
    ordinary_destination_staging_refuses_changed_source_frontiers_without_repair,
    {
        with_admitted_source_sql(|source, marker, supplied| {
            let (copying, closure, target, _) = source_copying(source, marker, supplied);
            let copying_inputs = OrdinaryCopyInputs::new(source, marker, &copying, &closure);
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
            initialize_or_replay_copied(c, &target, &copying_inputs, None).unwrap();
            let staging = ordinary_transition::proposed_staging(&copying).unwrap();
            ordinary_transition::apply_staging(
                source,
                marker,
                &copying.plan.source_prefix,
                &staging,
                None,
            )
            .unwrap();
            let staging_inputs = OrdinaryCopyInputs::new(source, marker, &staging, &closure);
            let original_ingress = ingress(source, marker);
            for mutation in ["generation", "capture"] {
                let before = state(source);
                source.execute_batch("SAVEPOINT advanced_source").unwrap();
                let current = offline::snapshot_at(source, copying.local_generation, None).unwrap();
                match mutation {
                    "generation" => {
                        let generation = current.generation + 1;
                        let payload = current.snapshot.canonical_bytes().unwrap();
                        let digest = database::digest(generation, Some(&current.digest), &payload);
                        source
                            .execute(
                                "INSERT INTO projection_snapshots VALUES(?1,?2,?3,?4)",
                                params![generation, digest, current.digest, payload],
                            )
                            .unwrap();
                        selection::insert(
                            source,
                            generation,
                            &selection::at(source, current.generation).unwrap(),
                        )
                        .unwrap();
                        source
                            .execute(
                                "UPDATE projection_head SET generation=?1 WHERE singleton=1",
                                [generation],
                            )
                            .unwrap();
                    }
                    "capture" => {
                        let (sequence, previous): (u64, String) = source.query_row("SELECT sequence,digest FROM projection_captures ORDER BY sequence DESC LIMIT 1", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
                        let mut patch = CapturedPatch {
                            schema_version: "layrs.projection-capture.v1".into(),
                            sequence: sequence + 1,
                            previous_patch_digest: Some(previous),
                            projection_generation: current.generation,
                            projection_digest: current.digest,
                            identity: marker.identity.clone(),
                            changes: vec![],
                            digest: String::new(),
                        };
                        patch.digest = patch.computed_digest().unwrap();
                        source
                            .execute(
                                "INSERT INTO projection_captures VALUES(?1,?2,?3,?4)",
                                params![
                                    patch.sequence,
                                    patch.digest,
                                    patch.projection_generation,
                                    canonical_json(&patch).unwrap()
                                ],
                            )
                            .unwrap();
                    }
                    _ => unreachable!(),
                }
                let changes = (source.total_changes(), c.total_changes());
                let altered = (state(source), state(c));
                assert!(
                    read_admitted_import(source, marker, None).is_err(),
                    "live Staging accepted later {mutation}"
                );
                assert!(ordinary_transition::apply_staging(
                    source,
                    marker,
                    &copying.plan.source_prefix,
                    &staging,
                    None
                )
                .is_err());
                assert!(
                    destination_staging::read_stageable(c, &target, &staging_inputs, None).is_err()
                );
                assert!(
                    destination_staging::apply_staging(c, &target, &staging_inputs, None).is_err()
                );
                assert_eq!((source.total_changes(), c.total_changes()), changes);
                assert_eq!((state(source), state(c)), altered);
                source
                    .execute_batch("ROLLBACK TO advanced_source; RELEASE advanced_source")
                    .unwrap();
                assert_eq!(state(source), before);
                assert_eq!(ingress(source, marker), original_ingress);
                assert_eq!(
                    destination_staging::read_stageable(c, &target, &staging_inputs, None).unwrap(),
                    TransitionState::Copying
                );
            }
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        });
    }
);
