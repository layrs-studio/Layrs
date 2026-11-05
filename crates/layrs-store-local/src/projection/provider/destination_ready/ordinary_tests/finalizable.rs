//! Finalizable-checkout SQL exercises of the ordinary destination tests.
use super::*;

pub(super) fn exercise_finalizable_sql(
    source: &Connection,
    destination: &Connection,
    family: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
) {
    use crate::projection::shared::read_manifest_exact;
    let source_before = dump(source);
    let destination_before = dump(destination);
    let family_before = dump(family);
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    source.execute_batch("PRAGMA query_only=ON").unwrap();
    destination.execute_batch("PRAGMA query_only=ON").unwrap();
    family.execute_batch("PRAGMA query_only=ON").unwrap();
    let journal =
        read_ordinary_checkout_finalizable(source, destination, family, inputs, expected, None)
            .unwrap();
    assert!(journal.is_complete());
    let progress =
        read_ordinary_checkout_progress(source, destination, family, inputs, expected, None)
            .unwrap()
            .expect("complete journal remains observed progress");
    assert!(progress.is_complete());
    assert_eq!(progress.rows(), journal.rows());
    assert_eq!(
        journal.rows(),
        checkout_journal::read(destination, expected, None)
            .unwrap()
            .rows()
    );
    for manifest in inputs.data.required {
        assert_eq!(
            read_manifest_exact(family, manifest.id(), None)
                .unwrap()
                .as_ref(),
            Some(manifest)
        );
    }
    let original = &inputs.data.required[0];
    let alternate = FileManifest::new(
        original.file_id().clone(),
        original.byte_length(),
        layrs_core::scm::ChunkingDescriptor::deterministic_fixed(524288).unwrap(),
        original.chunks().to_vec(),
    )
    .unwrap();
    assert_eq!(alternate.file_id(), original.file_id());
    assert_eq!(alternate.byte_length(), original.byte_length());
    assert_ne!(alternate.id(), original.id());
    assert!(
        !inputs
            .data
            .required
            .iter()
            .any(|manifest| manifest.id() == alternate.id())
    );
    assert!(
        read_manifest_exact(family, alternate.id(), None)
            .unwrap()
            .is_none(),
        "an absent exact ManifestId must not fall back to the retained same-FileId body"
    );
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(
        read_ordinary_checkout_progress(
            source,
            destination,
            family,
            inputs,
            expected,
            Some(&cancel),
        )
        .is_err()
    );
    assert!(
        read_ordinary_checkout_finalizable(
            source,
            destination,
            family,
            inputs,
            expected,
            Some(&cancel),
        )
        .is_err()
    );
    assert!(read_manifest_exact(family, original.id(), Some(&cancel)).is_err());
    let missing_data = ManagedReadyData {
        required: &inputs.data.required[1..],
        ..*inputs.data
    };
    let missing = OrdinaryReadyInputs {
        data: &missing_data,
        ..*inputs
    };
    assert!(
        read_ordinary_checkout_progress(source, destination, family, &missing, expected, None,)
            .is_err()
    );
    assert!(
        read_ordinary_checkout_finalizable(source, destination, family, &missing, expected, None,)
            .is_err()
    );
    let mut substituted_required = inputs.data.required.to_vec();
    substituted_required[0] = alternate;
    let substituted_data = ManagedReadyData {
        required: &substituted_required,
        ..*inputs.data
    };
    let substituted = OrdinaryReadyInputs {
        data: &substituted_data,
        ..*inputs
    };
    assert!(
        read_ordinary_checkout_progress(source, destination, family, &substituted, expected, None,)
            .is_err()
    );
    assert!(
        read_ordinary_checkout_finalizable(
            source,
            destination,
            family,
            &substituted,
            expected,
            None,
        )
        .is_err()
    );
    let wrong_floor = OrdinaryReadyInputs {
        metadata_floor: inputs.metadata_floor + 1,
        ..*inputs
    };
    assert!(
        read_ordinary_checkout_progress(source, destination, family, &wrong_floor, expected, None,)
            .is_err()
    );
    let wrong_marker = OrdinaryReadyInputs {
        source_marker: inputs.marker,
        ..*inputs
    };
    assert!(read_ordinary_checkout_progress(
        source,
        destination,
        family,
        &wrong_marker,
        expected,
        None,
    )
    .is_err());
    assert!(
        read_ordinary_checkout_finalizable(
            source,
            destination,
            family,
            &wrong_floor,
            expected,
            None,
        )
        .is_err()
    );
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), destination_before);
    assert_eq!(dump(family), family_before);
    source.execute_batch("PRAGMA query_only=OFF").unwrap();
    destination.execute_batch("PRAGMA query_only=OFF").unwrap();
    family.execute_batch("PRAGMA query_only=OFF").unwrap();

    // SQL-only absent predecessor. No filesystem state or ownership is
    // synthesized; a physical driver must separately validate an empty root.
    destination
        .execute_batch("SAVEPOINT checkout_absent")
        .unwrap();
    let declared_rows = destination
        .execute("DELETE FROM projection_prejournal", [])
        .unwrap();
    assert!(declared_rows > 0);
    let absent = dump(destination);
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    source.execute_batch("PRAGMA query_only=ON").unwrap();
    destination.execute_batch("PRAGMA query_only=ON").unwrap();
    family.execute_batch("PRAGMA query_only=ON").unwrap();
    assert!(
        read_ordinary_checkout_progress(source, destination, family, inputs, expected, None)
            .unwrap()
            .is_none()
    );
    let copy = OrdinaryCopyInputs::new(
        source,
        inputs.source_marker,
        inputs.source_staging,
        inputs.closure,
    );
    assert!(
        ordinary_destination::read_checkout_progress(
            destination,
            inputs.marker,
            &copy,
            expected,
            None
        )
        .unwrap()
        .is_none()
    );
    assert!(
        ordinary_destination::read_checkout_staging(
            destination,
            inputs.marker,
            &copy,
            expected,
            None
        )
        .is_err()
    );
    assert!(
        read_ordinary_checkout_finalizable(source, destination, family, inputs, expected, None)
            .is_err()
    );
    assert!(
        read_ordinary_checkout_progress(
            source,
            destination,
            family,
            inputs,
            expected,
            Some(&cancel)
        )
        .is_err()
    );
    assert!(
        read_ordinary_checkout_progress(source, destination, family, &missing, expected, None)
            .is_err()
    );
    assert!(
        read_ordinary_checkout_progress(source, destination, family, &substituted, expected, None)
            .is_err()
    );
    assert!(
        read_ordinary_checkout_progress(source, destination, family, &wrong_floor, expected, None)
            .is_err()
    );
    assert!(
        read_ordinary_checkout_progress(source, destination, family, &wrong_marker, expected, None)
            .is_err()
    );
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), absent);
    assert_eq!(dump(family), family_before);
    source.execute_batch("PRAGMA query_only=OFF").unwrap();
    destination.execute_batch("PRAGMA query_only=OFF").unwrap();
    family.execute_batch("PRAGMA query_only=OFF").unwrap();
    // Even without a journal, the full retained/protected family is required.
    family
        .execute_batch("SAVEPOINT checkout_absent_family")
        .unwrap();
    assert_eq!(
        family
            .execute(
                "UPDATE worktrees SET state='ready' WHERE id=?1",
                [inputs.target.binding().owner.as_str()]
            )
            .unwrap(),
        1
    );
    let corrupt_family = dump(family);
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    assert!(
        read_ordinary_checkout_progress(source, destination, family, inputs, expected, None)
            .is_err()
    );
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), absent);
    assert_eq!(dump(family), corrupt_family);
    family
        .execute_batch("ROLLBACK TO checkout_absent_family; RELEASE checkout_absent_family")
        .unwrap();

    // A canonical Copying predecessor is a legitimate staging gap, but must
    // never be accepted as checkout progress merely because its journal is empty.
    destination
        .execute_batch("SAVEPOINT checkout_absent_copying")
        .unwrap();
    let raw: Vec<u8> = destination
        .query_row(
            "SELECT payload FROM projection_provider_transitions WHERE id=?1 AND role='destination'",
            [&inputs.source_staging.transaction_id],
            |r| r.get(0),
        )
        .unwrap();
    let mut copying = ProviderTransitionIntentRecord::decode(&raw).unwrap();
    copying.state = TransitionState::Copying;
    copying.reservation = None;
    copying.validate_fields().unwrap();
    assert_eq!(destination.execute(
        "UPDATE projection_provider_transitions SET state='copying',reservation=NULL,payload=?1,digest=?2 WHERE id=?3 AND role='destination'",
        params![copying.encode().unwrap(), copying.digest().unwrap(), copying.transaction_id],
    ).unwrap(), 1);
    let copy = OrdinaryCopyInputs::new(
        source,
        inputs.source_marker,
        inputs.source_staging,
        inputs.closure,
    );
    assert_eq!(
        ordinary_destination::staging::read_stageable(destination, inputs.marker, &copy, None)
            .unwrap(),
        TransitionState::Copying
    );
    let before = (dump(source), dump(destination), dump(family));
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    source.execute_batch("PRAGMA query_only=ON").unwrap();
    destination.execute_batch("PRAGMA query_only=ON").unwrap();
    family.execute_batch("PRAGMA query_only=ON").unwrap();
    assert!(
        read_ordinary_checkout_progress(source, destination, family, inputs, expected, None)
            .is_err()
    );
    assert!(
        ordinary_destination::read_checkout_progress(
            destination,
            inputs.marker,
            &copy,
            expected,
            None
        )
        .is_err()
    );
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
    assert_eq!((dump(source), dump(destination), dump(family)), before);
    source.execute_batch("PRAGMA query_only=OFF").unwrap();
    destination.execute_batch("PRAGMA query_only=OFF").unwrap();
    family.execute_batch("PRAGMA query_only=OFF").unwrap();
    destination
        .execute_batch("ROLLBACK TO checkout_absent_copying; RELEASE checkout_absent_copying")
        .unwrap();

    // A single opaque key must select strict journal interpretation, not None.
    destination
        .execute_batch("SAVEPOINT checkout_opaque_only")
        .unwrap();
    let header = journal.row(RowKey::Header).unwrap();
    assert_eq!(destination.execute(
        "INSERT INTO projection_prejournal(path,operation,digest,payload,state) VALUES(?1,?2,?3,?4,'owned')",
        params![format!("{}:foreign", expected.sql_key(RowKey::Header).unwrap()), expected.operation(), header.digest().unwrap(), header.encode().unwrap()],
    ).unwrap(), 1);
    let before = (dump(source), dump(destination), dump(family));
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    source.execute_batch("PRAGMA query_only=ON").unwrap();
    destination.execute_batch("PRAGMA query_only=ON").unwrap();
    family.execute_batch("PRAGMA query_only=ON").unwrap();
    assert!(
        read_ordinary_checkout_progress(source, destination, family, inputs, expected, None)
            .is_err()
    );
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
    assert_eq!((dump(source), dump(destination), dump(family)), before);
    source.execute_batch("PRAGMA query_only=OFF").unwrap();
    destination.execute_batch("PRAGMA query_only=OFF").unwrap();
    family.execute_batch("PRAGMA query_only=OFF").unwrap();
    destination
        .execute_batch("ROLLBACK TO checkout_opaque_only; RELEASE checkout_opaque_only")
        .unwrap();

    // Genuine SQL declaration creates the complete typed inventory, all in
    // Declared phases. This remains SQL DATA with no filesystem ownership.
    assert_eq!(
        checkout_journal::declare(destination, expected, None).unwrap(),
        checkout_journal::DeclareEffect::Inserted
    );
    let before = (dump(source), dump(destination), dump(family));
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    source.execute_batch("PRAGMA query_only=ON").unwrap();
    destination.execute_batch("PRAGMA query_only=ON").unwrap();
    family.execute_batch("PRAGMA query_only=ON").unwrap();
    let declared =
        read_ordinary_checkout_progress(source, destination, family, inputs, expected, None)
            .unwrap()
            .expect("declared journal must remain observable");
    assert!(!declared.is_complete());
    for row in declared.rows() {
        assert_eq!(row, &expected.declared_row(row.key()).unwrap());
    }
    assert!(
        read_ordinary_checkout_finalizable(source, destination, family, inputs, expected, None)
            .is_err()
    );
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
    assert_eq!((dump(source), dump(destination), dump(family)), before);
    source.execute_batch("PRAGMA query_only=OFF").unwrap();
    destination.execute_batch("PRAGMA query_only=OFF").unwrap();
    family.execute_batch("PRAGMA query_only=OFF").unwrap();
    destination
        .execute_batch("ROLLBACK TO checkout_absent; RELEASE checkout_absent")
        .unwrap();
    assert_eq!(dump(destination), destination_before);

    source
        .execute_batch("SAVEPOINT checkout_changed_source")
        .unwrap();
    assert_eq!(source.execute(
        "UPDATE projection_provider_transitions SET state='ready' WHERE id=?1 AND role='source' AND state='staging'",
        [&inputs.source_staging.transaction_id],
    ).unwrap(), 1);
    let changed_source = dump(source);
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    assert!(
        read_ordinary_checkout_progress(source, destination, family, inputs, expected, None)
            .is_err()
    );
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
    assert_eq!(dump(source), changed_source);
    assert_eq!(dump(destination), destination_before);
    assert_eq!(dump(family), family_before);
    source
        .execute_batch("ROLLBACK TO checkout_changed_source; RELEASE checkout_changed_source")
        .unwrap();

    // A complete row inventory with a legal intermediate file phase must be
    // refused specifically by the finalizable entry's completion requirement.
    for phase in 0..3 {
        destination
            .execute_batch("SAVEPOINT finalizable_progress")
            .unwrap();
        let mut incomplete = journal.row(RowKey::File(0)).unwrap().clone();
        let checkout_journal::CheckoutEntry::File { state, .. } = &mut incomplete.entry else {
            panic!("fixture file row required")
        };
        let checkout_journal::FileState::Published {
            inode,
            parent_identity,
        } = state
        else {
            panic!("fixture published file required")
        };
        *state = match phase {
            0 => checkout_journal::FileState::Owned {
                inode: inode.clone(),
            },
            1 => checkout_journal::FileState::Verified {
                inode: inode.clone(),
            },
            _ => checkout_journal::FileState::Publishing {
                inode: inode.clone(),
                parent_identity: parent_identity.clone(),
            },
        };
        assert_eq!(
            destination
                .execute(
                    "UPDATE projection_prejournal SET digest=?1,payload=?2 WHERE path=?3",
                    params![
                        incomplete.digest().unwrap(),
                        incomplete.encode().unwrap(),
                        expected.sql_key(RowKey::File(0)).unwrap()
                    ],
                )
                .unwrap(),
            1
        );
        let progress = dump(destination);
        let changes = (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes(),
        );
        let copy = OrdinaryCopyInputs::new(
            source,
            inputs.source_marker,
            inputs.source_staging,
            inputs.closure,
        );
        source.execute_batch("PRAGMA query_only=ON").unwrap();
        destination.execute_batch("PRAGMA query_only=ON").unwrap();
        family.execute_batch("PRAGMA query_only=ON").unwrap();
        let actual =
            read_ordinary_checkout_progress(source, destination, family, inputs, expected, None)
                .unwrap()
                .expect("legal partial journal must remain observable");
        assert!(!actual.is_complete());
        assert_eq!(actual.row(RowKey::File(0)).unwrap(), &incomplete);
        assert!(
            !ordinary_destination::read_checkout_staging(
                destination,
                inputs.marker,
                &copy,
                expected,
                None
            )
            .unwrap()
            .is_complete()
        );
        let error =
            read_ordinary_checkout_finalizable(source, destination, family, inputs, expected, None)
                .err()
                .expect("legal intermediate journal must not be finalizable");
        assert!(
            error.to_string().contains("complete typed journal"),
            "{error}"
        );
        assert_eq!(
            (
                source.total_changes(),
                destination.total_changes(),
                family.total_changes()
            ),
            changes
        );
        assert_eq!(dump(destination), progress);
        assert_eq!(dump(source), source_before);
        assert_eq!(dump(family), family_before);
        source.execute_batch("PRAGMA query_only=OFF").unwrap();
        destination.execute_batch("PRAGMA query_only=OFF").unwrap();
        family.execute_batch("PRAGMA query_only=OFF").unwrap();
        destination
            .execute_batch("ROLLBACK TO finalizable_progress; RELEASE finalizable_progress")
            .unwrap();
        assert_eq!(dump(destination), destination_before);
    }

    for sql in [
        "DELETE FROM projection_prejournal WHERE path=?1",
        "UPDATE projection_prejournal SET path=path||':foreign' WHERE path=?1",
        "UPDATE projection_prejournal SET payload=x'00' WHERE path=?1",
    ] {
        destination
            .execute_batch("SAVEPOINT checkout_foreign_journal")
            .unwrap();
        assert_eq!(
            destination
                .execute(sql, [expected.sql_key(RowKey::Header).unwrap()])
                .unwrap(),
            1
        );
        let corrupt = dump(destination);
        let changes = (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes(),
        );
        assert!(
            read_ordinary_checkout_progress(source, destination, family, inputs, expected, None)
                .is_err(),
            "progress accepted {sql}"
        );
        assert!(
            read_ordinary_checkout_finalizable(source, destination, family, inputs, expected, None)
                .is_err()
        );
        assert_eq!(
            (
                source.total_changes(),
                destination.total_changes(),
                family.total_changes()
            ),
            changes
        );
        assert_eq!(dump(source), source_before);
        assert_eq!(dump(destination), corrupt);
        assert_eq!(dump(family), family_before);
        destination
            .execute_batch("ROLLBACK TO checkout_foreign_journal; RELEASE checkout_foreign_journal")
            .unwrap();
    }

    let target_owner = inputs.target.binding().owner.as_str();
    for (sql, owner) in [
        (
            "UPDATE worktrees SET state='ready' WHERE id=?1",
            Some(target_owner),
        ),
        (
            "UPDATE worktrees SET marker=x'00' WHERE id=?1",
            Some(target_owner),
        ),
        (
            "UPDATE roots SET payload=x'00' WHERE owner=?1",
            Some(target_owner),
        ),
        (
            "UPDATE reservations SET remaining=remaining+1 WHERE owner=?1",
            Some(target_owner),
        ),
        (
            "UPDATE roots SET payload=x'00' WHERE owner=?1",
            Some(inputs.source_marker.identity.worktree_id.as_str()),
        ),
        ("UPDATE family SET marker=x'00' WHERE singleton=1", None),
    ] {
        family
            .execute_batch("SAVEPOINT finalizable_family")
            .unwrap();
        let changed = match owner {
            Some(owner) => family.execute(sql, [owner]).unwrap(),
            None => family.execute(sql, []).unwrap(),
        };
        assert_eq!(changed, 1);
        let corrupt = dump(family);
        let changes = (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes(),
        );
        assert!(
            read_ordinary_checkout_progress(source, destination, family, inputs, expected, None,)
                .is_err(),
            "progress reader accepted {sql}"
        );
        assert!(read_ordinary_checkout_finalizable(
            source, destination, family, inputs, expected, None,
        ).is_err(), "finalizable reader accepted {sql}");
        assert_eq!(
            (
                source.total_changes(),
                destination.total_changes(),
                family.total_changes()
            ),
            changes
        );
        assert_eq!(dump(family), corrupt);
        assert_eq!(dump(source), source_before);
        assert_eq!(dump(destination), destination_before);
        family
            .execute_batch("ROLLBACK TO finalizable_family; RELEASE finalizable_family")
            .unwrap();
    }
    for sql in [
        "UPDATE manifests SET payload=x'00' WHERE id=?1",
        "DELETE FROM manifest_chunks WHERE manifest_id=?1 AND ordinal=0",
    ] {
        family
            .execute_batch("SAVEPOINT finalizable_exact_manifest")
            .unwrap();
        assert_eq!(family.execute(sql, [original.id().as_str()]).unwrap(), 1);
        let corrupt = dump(family);
        let changes = family.total_changes();
        assert!(read_manifest_exact(family, original.id(), None).is_err());
        let all_changes = (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes(),
        );
        assert!(
            read_ordinary_checkout_progress(source, destination, family, inputs, expected, None)
                .is_err()
        );
        assert_eq!(
            (
                source.total_changes(),
                destination.total_changes(),
                family.total_changes()
            ),
            all_changes
        );
        assert_eq!(family.total_changes(), changes);
        assert_eq!(dump(family), corrupt);
        assert_eq!(dump(source), source_before);
        assert_eq!(dump(destination), destination_before);
        family
            .execute_batch(
                "ROLLBACK TO finalizable_exact_manifest; RELEASE finalizable_exact_manifest",
            )
            .unwrap();
    }
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), destination_before);
    assert_eq!(dump(family), family_before);
}
