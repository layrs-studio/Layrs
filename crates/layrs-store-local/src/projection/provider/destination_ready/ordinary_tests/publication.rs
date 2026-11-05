//! Source and family publication exercises of the ordinary destination tests.
use super::*;

pub(super) fn exercise_source_publication(
    source: &Connection,
    destination: &Connection,
    family: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
) {
    use crate::projection::provider::{
        ordinary_transition::{self, PublicationCapacity},
        transition_sql::SourceIntentEffect,
    };
    let marker = inputs.source_marker;
    let staging = inputs.source_staging;
    let ready = inputs.data.ready;
    let receipt = inputs.data.receipt;
    let capacity = PublicationCapacity::new(
        ready,
        receipt,
        inputs.metadata_floor,
        staging.plan.quota_bytes,
    )
    .unwrap();
    assert!(capacity.metadata_floor() > inputs.metadata_floor);
    assert!(PublicationCapacity::new(ready, receipt, inputs.metadata_floor, 0).is_err());
    assert!(PublicationCapacity::new(ready, receipt, 0, staging.plan.quota_bytes).is_err());
    let before = dump(source);
    let destination_before = dump(destination);
    let initializing_family = dump(family);
    assert_source_publication_reader(
        source,
        destination,
        family,
        inputs,
        expected,
        TransitionState::Staging,
        false,
    );
    let changes = family.total_changes();
    assert_eq!(
        inputs
            .retained
            .publish_export_ready(
                family,
                inputs.family_marker_bytes,
                &inputs.publication_inputs(expected),
                None,
            )
            .unwrap(),
        crate::projection::shared::admitted_catalog::ExportPublicationEffect::Updated
    );
    assert_eq!(family.total_changes(), changes + 1);
    let mut exact_family_ready = initializing_family;
    let members = exact_family_ready.get_mut("worktrees").unwrap();
    let owner = Cell::Text(inputs.target.binding().owner.as_str().as_bytes().to_vec());
    let row = members
        .iter_mut()
        .find(|row| row.first() == Some(&owner))
        .unwrap();
    assert_eq!(row[4], Cell::Text(b"initializing".to_vec()));
    row[4] = Cell::Text(b"ready".to_vec());
    members.sort();
    assert_eq!(dump(family), exact_family_ready);
    assert_eq!(dump(source), before);
    assert_eq!(dump(destination), destination_before);
    let family_before = dump(family);
    assert_source_publication_reader(
        source,
        destination,
        family,
        inputs,
        expected,
        TransitionState::Staging,
        true,
    );
    let ingress = read_admitted_import(source, marker, None).unwrap();
    let state = ordinary_transition::read_publication_state(source, marker, TX, None).unwrap();
    assert_eq!(&state.intent, staging);
    assert!(state.receipt.is_none());

    // Canonical receipt without its terminal source owner is corruption. The
    // writer cannot adopt it, even when it is exactly the proposed C receipt.
    source.execute_batch("SAVEPOINT orphan_receipt").unwrap();
    source
        .execute(
            "INSERT INTO projection_provider_transition_receipts VALUES(?1,?2,?3,?4,?5)",
            params![
                receipt.digest().unwrap(),
                receipt.owner_identity_digest,
                receipt.transaction_id,
                receipt.companion_id,
                receipt.encode().unwrap()
            ],
        )
        .unwrap();
    let corrupt = dump(source);
    let changes = source.total_changes();
    assert!(read_admitted_import(source, marker, None).is_err());
    assert!(
        ordinary_transition::apply_ready(source, marker, staging, ready, receipt, &capacity, None)
            .is_err()
    );
    assert_eq!(source.total_changes(), changes);
    assert_eq!(dump(source), corrupt);
    source
        .execute_batch("ROLLBACK TO orphan_receipt; RELEASE orphan_receipt")
        .unwrap();

    let mut wrong = ProviderTransitionReceiptRecord::decode(&receipt.encode().unwrap()).unwrap();
    wrong.transaction_id = "000000000000000000000000000ce199".into();
    let changes = source.total_changes();
    assert!(
        ordinary_transition::apply_ready(source, marker, staging, ready, &wrong, &capacity, None)
            .is_err()
    );
    assert_eq!(source.total_changes(), changes);
    assert_eq!(dump(source), before);
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(
        ordinary_transition::apply_ready(
            source,
            marker,
            staging,
            ready,
            receipt,
            &capacity,
            Some(&cancel),
        )
        .is_err()
    );
    assert_eq!(source.total_changes(), changes);
    assert_eq!(dump(source), before);
    assert_eq!(
        ordinary_transition::apply_ready(source, marker, staging, ready, receipt, &capacity, None)
            .unwrap(),
        SourceIntentEffect::Updated
    );
    assert_eq!(
        source.total_changes(),
        changes + 2,
        "one receipt INSERT and one source CAS"
    );
    let after_ready = dump(source);
    for (table, rows) in &before {
        if table != "projection_provider_transitions"
            && table != "projection_provider_transition_receipts"
        {
            assert_eq!(&after_ready[table], rows, "Ready changed {table}");
        }
    }
    assert!(
        ingress.require(source, None).is_err(),
        "retained outgoing proof must notice the new phase/receipt"
    );
    let admitted_ready = read_admitted_import(source, marker, None).unwrap();
    assert_eq!(admitted_ready.record(), ingress.record());
    assert_eq!(admitted_ready.ready(), ingress.ready());
    assert_eq!(admitted_ready.receipt(), ingress.receipt());
    let source_ready =
        ordinary_transition::read_publication_state(source, marker, TX, None).unwrap();
    assert_eq!(source_ready.intent.state, TransitionState::Ready);
    assert_eq!(source_ready.receipt.as_ref(), Some(receipt));
    assert!(
        read_ordinary_checkout_ready_for_publication(
            source,
            destination,
            family,
            inputs,
            expected,
            None,
        )
        .is_err(),
        "strict Staging reader must refuse actual B Ready"
    );
    assert_source_publication_reader(
        source,
        destination,
        family,
        inputs,
        expected,
        TransitionState::Ready,
        true,
    );
    let mut exact_ready = before.clone();
    let row = exact_ready
        .get_mut("projection_provider_transitions")
        .unwrap()
        .iter_mut()
        .find(|row| row[0] == Cell::Text(TX.as_bytes().to_vec()))
        .unwrap();
    row[2] = Cell::Text(b"ready".to_vec());
    row[5] = Cell::Text(receipt.digest().unwrap().into_bytes());
    row[6] = Cell::Blob(source_ready.intent.encode().unwrap());
    row[7] = Cell::Text(source_ready.intent.digest().unwrap().into_bytes());
    exact_ready
        .get_mut("projection_provider_transitions")
        .unwrap()
        .sort();
    let receipts = exact_ready
        .get_mut("projection_provider_transition_receipts")
        .unwrap();
    receipts.push(vec![
        Cell::Text(receipt.digest().unwrap().into_bytes()),
        Cell::Text(receipt.owner_identity_digest.as_bytes().to_vec()),
        Cell::Text(receipt.transaction_id.as_bytes().to_vec()),
        receipt
            .companion_id
            .as_ref()
            .map_or(Cell::Null, |id| Cell::Text(id.as_bytes().to_vec())),
        Cell::Blob(receipt.encode().unwrap()),
    ]);
    receipts.sort();
    assert_eq!(
        after_ready, exact_ready,
        "Ready must change exactly one source row and insert C's exact receipt"
    );
    assert!(ordinary_transition::has_live(source).unwrap());
    assert!(database::require_idle(source).is_err());
    assert!(
        ordinary_transition::apply_staging(
            source,
            marker,
            &staging.plan.source_prefix,
            staging,
            None,
        )
        .is_err(),
        "old Staging writer must refuse after actual Ready promotion"
    );
    let mut competing = ProviderTransitionIntentRecord::decode(&staging.encode().unwrap()).unwrap();
    competing.transaction_id = "000000000000000000000000000ce198".into();
    competing.state = TransitionState::Planned;
    competing.destination = None;
    competing.reservation = None;
    assert!(
        ordinary_transition::apply_planned(
            source,
            marker,
            &competing.plan.source_prefix,
            &competing,
            None
        )
        .is_err()
    );
    let changes = source.total_changes();
    source.execute_batch("PRAGMA query_only=ON").unwrap();
    assert_eq!(
        ordinary_transition::apply_ready(source, marker, staging, ready, receipt, &capacity, None)
            .unwrap(),
        SourceIntentEffect::AlreadyPresent
    );
    assert_eq!(source.total_changes(), changes);
    assert_eq!(dump(source), after_ready);
    source.execute_batch("PRAGMA query_only=OFF").unwrap();

    for sql in [
        "UPDATE projection_provider_transition_receipts SET owner_identity_digest='blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff' WHERE transaction_id='000000000000000000000000000ce101'",
        "UPDATE projection_provider_transition_receipts SET payload=X'00' WHERE transaction_id='000000000000000000000000000ce101'",
        "UPDATE projection_head SET generation=generation-1",
    ] {
        source
            .execute_batch("SAVEPOINT corrupt_source_ready")
            .unwrap();
        assert_eq!(source.execute(sql, []).unwrap(), 1);
        let corrupt = dump(source);
        let changes = source.total_changes();
        assert!(admitted_ready.require(source, None).is_err());
        assert!(ordinary_transition::read_publication_state(source, marker, TX, None).is_err());
        assert!(
            ordinary_transition::apply_published(
                source,
                marker,
                &source_ready.intent,
                ready,
                receipt,
                &capacity,
                None
            )
            .is_err()
        );
        assert_eq!(source.total_changes(), changes);
        assert_eq!(dump(source), corrupt);
        source
            .execute_batch("ROLLBACK TO corrupt_source_ready; RELEASE corrupt_source_ready")
            .unwrap();
    }
    // Immutable incoming admission remains valid, but a live Ready source
    // cannot publish or replay Ready with an unrepresented operational row.
    for table in [
        "projection_install",
        "projection_refresh_pending",
        "projection_namespace_pending",
        "projection_restore_pending",
        "projection_rebase_pending",
    ] {
        source
            .execute_batch("SAVEPOINT ready_pending_operation")
            .unwrap();
        source
            .execute(
                &format!("INSERT INTO {table}(singleton,payload) VALUES(1,X'')"),
                [],
            )
            .unwrap();
        let corrupt = dump(source);
        let changes = source.total_changes();
        read_admitted_import(source, marker, None).unwrap();
        let error = ordinary_transition::read_publication_state(source, marker, TX, None)
            .err()
            .expect("live Ready must refuse pending operations");
        assert!(error.to_string().contains(table), "{error}");
        let error = ordinary_transition::apply_published(
            source,
            marker,
            &source_ready.intent,
            ready,
            receipt,
            &capacity,
            None,
        )
        .err()
        .expect("Ready publication must refuse before its SQL effect");
        assert!(error.to_string().contains(table), "{error}");
        assert!(
            ordinary_transition::apply_ready(
                source, marker, staging, ready, receipt, &capacity, None,
            )
            .is_err(),
            "Ready replay must not accept an operational pending row"
        );
        assert_eq!(source.total_changes(), changes);
        assert_eq!(dump(source), corrupt);
        source
            .execute_batch("ROLLBACK TO ready_pending_operation; RELEASE ready_pending_operation")
            .unwrap();
    }
    // A capture-only advance also fences Ready even when the head is current.
    source
        .execute_batch("SAVEPOINT ready_capture_frontier")
        .unwrap();
    append_publication_tail(source, marker, false);
    assert!(read_admitted_import(source, marker, None).is_err());
    source
        .execute_batch("ROLLBACK TO ready_capture_frontier; RELEASE ready_capture_frontier")
        .unwrap();
    let changes = source.total_changes();
    assert_eq!(
        ordinary_transition::apply_published(
            source,
            marker,
            &source_ready.intent,
            ready,
            receipt,
            &capacity,
            None
        )
        .unwrap(),
        SourceIntentEffect::Updated
    );
    assert_eq!(
        source.total_changes(),
        changes + 1,
        "Published only changes one source row"
    );
    let after_published = dump(source);
    let mut published =
        ProviderTransitionIntentRecord::decode(&source_ready.intent.encode().unwrap()).unwrap();
    published.state = TransitionState::Published;
    let mut exact_published = after_ready.clone();
    let row = exact_published
        .get_mut("projection_provider_transitions")
        .unwrap()
        .iter_mut()
        .find(|row| row[0] == Cell::Text(TX.as_bytes().to_vec()))
        .unwrap();
    row[2] = Cell::Text(b"published".to_vec());
    row[6] = Cell::Blob(published.encode().unwrap());
    row[7] = Cell::Text(published.digest().unwrap().into_bytes());
    exact_published
        .get_mut("projection_provider_transitions")
        .unwrap()
        .sort();
    assert_eq!(
        after_published, exact_published,
        "Published changes only state/payload/digest of its source row"
    );
    for (table, rows) in &after_ready {
        if table != "projection_provider_transitions" {
            assert_eq!(&after_published[table], rows, "Published changed {table}");
        }
    }
    assert!(!ordinary_transition::has_live(source).unwrap());
    database::require_idle(source).unwrap();
    assert!(
        read_ordinary_checkout_ready_for_publication(
            source,
            destination,
            family,
            inputs,
            expected,
            None,
        )
        .is_err(),
        "strict Staging reader must refuse actual B Published"
    );
    assert_source_publication_reader(
        source,
        destination,
        family,
        inputs,
        expected,
        TransitionState::Published,
        true,
    );
    // Coherent later B generation and capture preserve the immutable Published
    // SQL handoff. No old image is compared to a new current-prefix planner.
    append_publication_tail(source, marker, true);
    let historical = dump(source);
    let changes = source.total_changes();
    source.execute_batch("PRAGMA query_only=ON").unwrap();
    let state = ordinary_transition::read_publication_state(source, marker, TX, None).unwrap();
    assert_eq!(state.intent.state, TransitionState::Published);
    assert_eq!(state.receipt.as_ref(), Some(receipt));
    assert_eq!(
        ordinary_transition::apply_published(
            source,
            marker,
            &source_ready.intent,
            ready,
            receipt,
            &capacity,
            None
        )
        .unwrap(),
        SourceIntentEffect::AlreadyPresent
    );
    assert!(
        ordinary_transition::apply_ready(source, marker, staging, ready, receipt, &capacity, None)
            .is_err(),
        "Ready API must not downgrade or adopt Published"
    );
    assert_eq!(source.total_changes(), changes);
    assert_eq!(dump(source), historical);
    source.execute_batch("PRAGMA query_only=OFF").unwrap();
    assert_eq!(dump(destination), destination_before);
    assert_eq!(dump(family), family_before);
}

/// Each publication phase gets fresh B provenance, a newly interpreted original
/// copied closure, and a new closed actual-phase source. Old proof reuse must
/// not hide the outgoing row/receipt changes performed by the preceding CAS.
pub(super) fn assert_source_publication_reader(
    source: &Connection,
    destination: &Connection,
    family: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
    phase: TransitionState,
    family_ready: bool,
) {
    use crate::projection::provider::ordinary_publication::OrdinaryPublicationSource;
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    let admitted = read_admitted_import(source, inputs.source_marker, None).unwrap();
    let actual = admitted_ordinary::inspect(source, inputs.source_marker, None).unwrap();
    let retained = AdmittedFamilyContext::new(&actual).unwrap();
    let original = &inputs.source_staging.plan.source_prefix;
    let (prefix, closure) = plan_admitted_import(
        source,
        inputs.source_marker,
        original.destination_identity.clone(),
        &original.source.version().effective_image_digest,
        None,
    )
    .unwrap();
    assert_eq!(&prefix, original);
    let fresh = OrdinaryReadyInputs {
        marker: inputs.marker,
        source_marker: inputs.source_marker,
        source_staging: inputs.source_staging,
        closure: &closure,
        retained: &retained,
        target: inputs.target,
        protection: inputs.protection,
        family_marker_bytes: inputs.family_marker_bytes,
        metadata_floor: inputs.metadata_floor,
        data: inputs.data,
    };
    let publication = OrdinaryPublicationSource::new(
        source,
        fresh.source_marker,
        fresh.closure,
        fresh.source_staging,
        fresh.data.ready,
        fresh.data.receipt,
        None,
    )
    .unwrap();
    assert_eq!(publication.revalidate(None).unwrap().state, phase);
    let actual = read_ordinary_ready_for_source_publication(
        source,
        destination,
        family,
        &publication,
        &fresh,
        expected,
        None,
    );
    if family_ready {
        let actual = actual.unwrap();
        assert_eq!(actual.ready(), inputs.data.ready);
        assert_eq!(actual.receipt(), inputs.data.receipt);
        assert_eq!(
            actual.ready().encode().unwrap(),
            inputs.data.ready.encode().unwrap()
        );
        assert_eq!(
            actual.receipt().encode().unwrap(),
            inputs.data.receipt.encode().unwrap()
        );

        let mut stale_receipt =
            ProviderTransitionReceiptRecord::decode(&inputs.data.receipt.encode().unwrap())
                .unwrap();
        stale_receipt.transaction_id = "000000000000000000000000000ce197".into();
        assert!(
            OrdinaryPublicationSource::new(
                source,
                fresh.source_marker,
                fresh.closure,
                fresh.source_staging,
                fresh.data.ready,
                &stale_receipt,
                None,
            )
            .is_err(),
            "closed source must reject a stale expected C receipt"
        );
        let stale_data = ManagedReadyData {
            observation: fresh.data.observation,
            placement: fresh.data.placement,
            capture: fresh.data.capture,
            overlay: fresh.data.overlay,
            required: fresh.data.required,
            receipt: &stale_receipt,
            ready: fresh.data.ready,
            claims: fresh.data.claims,
        };
        let stale = OrdinaryReadyInputs {
            data: &stale_data,
            ..fresh
        };
        assert!(
            read_ordinary_ready_for_source_publication(
                source,
                destination,
                family,
                &publication,
                &stale,
                expected,
                None,
            )
            .is_err(),
            "reader must reject stale expected receipt inputs against the closed source"
        );
    } else {
        let error = actual
            .err()
            .expect("Initializing family must refuse source publication");
        assert!(error.to_string().contains("actual family Ready"), "{error}");
    }
    admitted.require(source, None).unwrap();
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
}

pub(super) fn append_publication_tail(
    source: &Connection,
    marker: &format::Marker,
    advance_head: bool,
) {
    let mut current = database::current(source).unwrap();
    if advance_head {
        let generation = current.generation + 1;
        let payload = current.snapshot.canonical_bytes().unwrap();
        let digest = database::digest(generation, Some(&current.digest), &payload);
        source
            .execute(
                "INSERT INTO projection_snapshots VALUES(?1,?2,?3,?4)",
                params![generation, digest, current.digest, payload],
            )
            .unwrap();
        let selected = selection::at(source, current.generation).unwrap();
        selection::insert(source, generation, &selected).unwrap();
        source
            .execute("UPDATE projection_head SET generation=?1", [generation])
            .unwrap();
        current = database::current(source).unwrap();
    }
    let (sequence, digest): (u64, String) = source
        .query_row(
            "SELECT sequence,digest FROM projection_captures ORDER BY sequence DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let mut capture = crate::projection::capture::CapturedPatch {
        schema_version: "layrs.projection-capture.v1".into(),
        sequence: sequence + 1,
        previous_patch_digest: Some(digest),
        projection_generation: current.generation,
        projection_digest: current.digest,
        identity: marker.identity.clone(),
        changes: vec![],
        digest: String::new(),
    };
    capture.digest = capture.computed_digest().unwrap();
    source
        .execute(
            "INSERT INTO projection_captures VALUES(?1,?2,?3,?4)",
            params![
                capture.sequence,
                capture.digest,
                capture.projection_generation,
                canonical_json(&capture).unwrap()
            ],
        )
        .unwrap();
}

pub(super) fn exercise_publication(
    source: &Connection,
    destination: &Connection,
    family: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
) {
    use crate::projection::shared::admitted_catalog::{
        ExportPublicationEffect, ExportPublicationMember,
    };
    let publication = inputs.publication_inputs(expected);
    let retained = inputs.retained;
    let marker = inputs.family_marker_bytes;
    let source_before = dump(source);
    let destination_before = dump(destination);
    let family_before = dump(family);
    let read_changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    let private_before =
        read_ordinary_private_ready(destination, inputs.marker, family, None).unwrap();
    assert_eq!(
        private_before.member(),
        ExportPublicationMember::Initializing
    );
    assert_eq!(private_before.ready(), inputs.data.ready);
    assert_eq!(private_before.receipt(), inputs.data.receipt);
    assert_eq!(private_before.metadata_floor(), inputs.metadata_floor);
    let complete_before =
        read_ordinary_family_publication(source, destination, family, inputs, expected, None)
            .unwrap();
    private_before
        .require_same(complete_before.snapshot())
        .unwrap();
    assert!(
        read_ordinary_managed_ready(destination, inputs.marker, family, None).is_err(),
        "Initializing must not enter the terminal Ready reader",
    );
    read_ordinary_checkout_ready_for_publication(
        source,
        destination,
        family,
        inputs,
        expected,
        None,
    )
    .unwrap();
    let before = retained
        .read_export_publication(family, marker, &publication, None)
        .unwrap()
        .1;
    assert_eq!(before.member, ExportPublicationMember::Initializing);
    assert_eq!(complete_before.state(), before);
    let snapshot_before = complete_before.into_snapshot();

    // The same canonical representations may arrive in a different order.
    // Their actual ManifestIds and complete bodies remain exact.
    let reversed_required: Vec<_> = private_before.required().iter().rev().cloned().collect();
    assert!(reversed_required.len() > 1);
    assert_ne!(reversed_required.as_slice(), private_before.required());
    let reversed_data = ManagedReadyData {
        required: &reversed_required,
        ..*inputs.data
    };
    let reversed = OrdinaryReadyInputs {
        data: &reversed_data,
        ..*inputs
    };
    let reordered =
        read_ordinary_family_publication(source, destination, family, &reversed, expected, None)
            .unwrap();
    assert_eq!(reordered.state(), before);
    snapshot_before
        .require_same(&reordered.into_snapshot())
        .unwrap();

    // A different canonical ManifestId for the same FileId/length is not a
    // substitute, even though its exact chunk bodies describe the same bytes.
    let original = &reversed_required[0];
    let single = layrs_core::scm::ChunkingDescriptor::single();
    let chunking = if original.chunking() == &single {
        layrs_core::scm::ChunkingDescriptor::deterministic_fixed(1048576).unwrap()
    } else {
        single
    };
    let substitute = FileManifest::new(
        original.file_id().clone(),
        original.byte_length(),
        chunking,
        original.chunks().to_vec(),
    )
    .unwrap();
    assert_eq!(substitute.file_id(), original.file_id());
    assert_eq!(substitute.byte_length(), original.byte_length());
    assert_ne!(substitute.id(), original.id());
    let mut substituted_required = reversed_required.clone();
    substituted_required[0] = substitute;
    let substituted_data = ManagedReadyData {
        required: &substituted_required,
        ..*inputs.data
    };
    let substituted = OrdinaryReadyInputs {
        data: &substituted_data,
        ..*inputs
    };
    assert!(read_ordinary_family_publication(
        source,
        destination,
        family,
        &substituted,
        expected,
        None,
    )
    .is_err());
    let mut duplicated_required = reversed_required.clone();
    duplicated_required[0] = duplicated_required[1].clone();
    let duplicated_data = ManagedReadyData {
        required: &duplicated_required,
        ..*inputs.data
    };
    let duplicated = OrdinaryReadyInputs {
        data: &duplicated_data,
        ..*inputs
    };
    assert!(
        read_ordinary_family_publication(source, destination, family, &duplicated, expected, None,)
            .is_err()
    );
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        read_changes
    );
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), destination_before);
    assert_eq!(dump(family), family_before);

    let cancel = Cancellation::default();
    cancel.cancel();
    let changes = family.total_changes();
    assert!(
        read_ordinary_private_ready(destination, inputs.marker, family, Some(&cancel),).is_err()
    );
    assert!(
        read_ordinary_family_publication(
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
        retained
            .publish_export_ready(family, marker, &publication, Some(&cancel))
            .is_err()
    );
    assert_eq!(family.total_changes(), changes);
    assert_eq!(dump(family), family_before);
    // A Ready record borrowed from another transaction must not authorize this CAS.
    let mut wrong_receipt =
        ProviderTransitionReceiptRecord::decode(&inputs.data.receipt.encode().unwrap()).unwrap();
    wrong_receipt.transaction_id = "000000000000000000000000000ce199".into();
    let wrong_data = ManagedReadyData {
        receipt: &wrong_receipt,
        ..*inputs.data
    };
    let wrong_inputs = OrdinaryReadyInputs {
        data: &wrong_data,
        ..*inputs
    };
    assert!(
        read_ordinary_family_publication(
            source,
            destination,
            family,
            &wrong_inputs,
            expected,
            None,
        )
        .is_err()
    );
    let mut wrong = inputs.publication_inputs(expected);
    wrong.receipt = &wrong_receipt;
    assert!(
        retained
            .publish_export_ready(family, marker, &wrong, None)
            .is_err()
    );
    assert_eq!(family.total_changes(), changes);
    assert_eq!(dump(family), family_before);
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        read_changes
    );
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), destination_before);

    assert_family_publication_corruption(source, destination, family, inputs, expected);
    // C's provisional private DATA does not validate B's retained family root.
    // Losing that whole known group must still fail the complete actual-B gate.
    family
        .execute_batch("SAVEPOINT missing_retained_publication_root")
        .unwrap();
    assert!(
        family
            .execute(
                "DELETE FROM root_manifests WHERE root_id IN (SELECT id FROM roots WHERE owner=?1)",
                [inputs.source_marker.identity.worktree_id.as_str()],
            )
            .unwrap()
            > 0
    );
    assert_eq!(
        family
            .execute(
                "DELETE FROM roots WHERE owner=?1",
                [inputs.source_marker.identity.worktree_id.as_str()],
            )
            .unwrap(),
        1
    );
    let missing = dump(family);
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    let still_private =
        read_ordinary_private_ready(destination, inputs.marker, family, None).unwrap();
    private_before.require_same(&still_private).unwrap();
    assert!(
        read_ordinary_family_publication(source, destination, family, inputs, expected, None,)
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
    assert_eq!(dump(family), missing);
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), destination_before);
    family.execute_batch("ROLLBACK TO missing_retained_publication_root; RELEASE missing_retained_publication_root").unwrap();
    assert_eq!(dump(family), family_before);

    family
        .execute_batch("SAVEPOINT publication_rollback")
        .unwrap();
    assert_eq!(
        retained
            .publish_export_ready(family, marker, &publication, None)
            .unwrap(),
        ExportPublicationEffect::Updated
    );
    read_ordinary_checkout_ready_for_publication(
        source,
        destination,
        family,
        inputs,
        expected,
        None,
    )
    .unwrap();
    family
        .execute_batch("ROLLBACK TO publication_rollback; RELEASE publication_rollback")
        .unwrap();
    assert_eq!(dump(family), family_before);
    read_ordinary_checkout_ready(source, destination, family, inputs, expected, None).unwrap();
    let changes = family.total_changes();
    assert_eq!(
        retained
            .publish_export_ready(family, marker, &publication, None)
            .unwrap(),
        ExportPublicationEffect::Updated
    );
    assert_eq!(family.total_changes() - changes, 1);
    let after = retained
        .read_export_publication(family, marker, &publication, None)
        .unwrap()
        .1;
    assert_eq!(after.member, ExportPublicationMember::Ready);
    assert_eq!(after.protection, before.protection);
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    let private_after =
        read_ordinary_private_ready(destination, inputs.marker, family, None).unwrap();
    assert_eq!(private_after.member(), ExportPublicationMember::Ready);
    assert!(
        private_before.require_same(&private_after).is_err(),
        "provisional comparison must retain its exact observed member phase"
    );
    let complete_after =
        read_ordinary_family_publication(source, destination, family, inputs, expected, None)
            .unwrap();
    private_after
        .require_same(complete_after.snapshot())
        .unwrap();
    assert_eq!(complete_after.state(), after);
    snapshot_before
        .require_same(&complete_after.into_snapshot())
        .unwrap();
    assert_eq!(
        (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes()
        ),
        changes
    );
    let mut expected_family = family_before;
    let members = expected_family.get_mut("worktrees").unwrap();
    let owner = Cell::Text(inputs.target.binding().owner.as_str().as_bytes().to_vec());
    let row = members
        .iter_mut()
        .find(|row| row.first() == Some(&owner))
        .unwrap();
    assert_eq!(row.len(), 5);
    assert_eq!(row[4], Cell::Text(b"initializing".to_vec()));
    row[4] = Cell::Text(b"ready".to_vec());
    members.sort();
    assert_eq!(dump(family), expected_family);
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), destination_before);
    read_ordinary_checkout_ready_for_publication(
        source,
        destination,
        family,
        inputs,
        expected,
        None,
    )
    .unwrap();
    assert!(
        read_ordinary_checkout_ready(source, destination, family, inputs, expected, None).is_err()
    );
    assert!(
        finalize_ordinary_checkout_or_replay(source, destination, family, inputs, expected, None)
            .is_err()
    );
    assert!(
        retained
            .validate_export_protection(
                family,
                marker,
                inputs.target,
                inputs.data.required,
                inputs.protection,
                None
            )
            .is_err()
    );
    assert_family_publication_corruption(source, destination, family, inputs, expected);
    family
        .execute_batch("COMMIT; PRAGMA foreign_keys=OFF; PRAGMA query_only=ON; BEGIN DEFERRED")
        .unwrap();
    source.execute_batch("PRAGMA query_only=ON").unwrap();
    let changes = (
        source.total_changes(),
        destination.total_changes(),
        family.total_changes(),
    );
    let readonly_private =
        read_ordinary_private_ready(destination, inputs.marker, family, None).unwrap();
    private_after.require_same(&readonly_private).unwrap();
    let readonly =
        read_ordinary_family_publication(source, destination, family, inputs, expected, None)
            .unwrap();
    assert_eq!(readonly.state(), after);
    snapshot_before
        .require_same(&readonly.into_snapshot())
        .unwrap();
    read_ordinary_checkout_ready_for_publication(
        source,
        destination,
        family,
        inputs,
        expected,
        None,
    )
    .unwrap();
    assert_eq!(
        retained
            .publish_export_ready(family, marker, &publication, None)
            .unwrap(),
        ExportPublicationEffect::AlreadyReady
    );
    assert!(
        read_ordinary_checkout_ready_for_publication(
            source,
            destination,
            family,
            inputs,
            expected,
            Some(&cancel)
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
    assert_eq!(dump(family), expected_family);
    assert_eq!(dump(source), source_before);
    assert_eq!(dump(destination), destination_before);
    source.execute_batch("PRAGMA query_only=OFF").unwrap();
}

pub(super) fn assert_family_publication_corruption(
    source: &Connection,
    destination: &Connection,
    family: &Connection,
    inputs: &OrdinaryReadyInputs<'_>,
    expected: &ExpectedCheckout,
) {
    let source_before = dump(source);
    let destination_before = dump(destination);
    let family_before = dump(family);
    for sql in [
        "UPDATE worktrees SET metadata_path='foreign' WHERE id=?1",
        "UPDATE worktrees SET database_identity='foreign' WHERE id=?1",
        "UPDATE worktrees SET marker=x'00' WHERE id=?1",
        "UPDATE roots SET payload=x'00' WHERE owner=?1",
        "UPDATE reservations SET remaining=remaining+1 WHERE owner=?1",
    ] {
        family
            .execute_batch("SAVEPOINT corrupt_publication")
            .unwrap();
        assert_eq!(
            family
                .execute(sql, [inputs.target.binding().owner.as_str()])
                .unwrap(),
            1
        );
        let corrupt = dump(family);
        let changes = (
            source.total_changes(),
            destination.total_changes(),
            family.total_changes(),
        );
        assert!(
            read_ordinary_family_publication(source, destination, family, inputs, expected, None,)
                .is_err(),
            "complete family publication accepted {sql}"
        );
        assert!(
            read_ordinary_checkout_ready_for_publication(
                source,
                destination,
                family,
                inputs,
                expected,
                None,
            )
            .is_err()
        );
        assert!(
            inputs
                .retained
                .publish_export_ready(
                    family,
                    inputs.family_marker_bytes,
                    &inputs.publication_inputs(expected),
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
        assert_eq!(dump(family), corrupt);
        assert_eq!(dump(source), source_before);
        assert_eq!(dump(destination), destination_before);
        family
            .execute_batch("ROLLBACK TO corrupt_publication; RELEASE corrupt_publication")
            .unwrap();
        assert_eq!(dump(family), family_before);
    }
}
