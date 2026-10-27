//! Synthetic SQL DATA only. Owned audited child120 recipes; ROOT alone reviews,
//! releases and executes. Source closes before autonomous interpretation. No
//! physical CAS, checkout, native capability or ordinary admission is asserted.
use super::super::family;
use super::*;
use crate::projection::provider::{
    checkout_journal::{self, CheckoutEntry, CheckoutFile, RowKey},
    database_io::BareConnection,
    destination_sql,
    import_validation::{self, ImportDatabaseKind, VerifiedImportClosure},
    process_profile::tests::child,
    records::{Receipt, Record},
    validation::{private_active_capture_with_source, tests::fixture::Fixture as SourceFixture},
};
use serde_json::json;
#[path = "../tests/fixture.rs"]
mod fixture;
#[path = "../tests/source.rs"]
mod source;
use fixture::Fixture;

macro_rules! owned {
    ($name:ident,$body:block)=>{
        #[test]
        #[ignore = "UNQUALIFIED complete autonomous Ready SQL DATA, owned audited child120; ROOT release required"]
        fn $name(){child::run(concat!(module_path!(),"::",stringify!($name)),120,||{child::declare();$body});}
    };
}

fn complete(c: &Connection, expected: &ExpectedCheckout) {
    checkout_journal::declare(c, expected, None).unwrap();
    let previous = checkout_journal::read(c, expected, None)
        .unwrap()
        .row(RowKey::Stage)
        .unwrap()
        .clone();
    checkout_journal::advance(
        c,
        expected,
        &previous,
        &previous
            .owned_directory("fs:v1:0000000000000001:0000000000001000")
            .unwrap(),
        None,
    )
    .unwrap();
    assert!(expected.directories().is_empty());
    for ordinal in 0..expected.files().len() {
        let mut previous = checkout_journal::read(c, expected, None)
            .unwrap()
            .row(RowKey::File(ordinal as u32))
            .unwrap()
            .clone();
        let next = previous
            .owned_file(&format!("1:0:{}", 2001 + ordinal))
            .unwrap();
        checkout_journal::advance(c, expected, &previous, &next, None).unwrap();
        previous = next;
        let next = previous.verified_file().unwrap();
        checkout_journal::advance(c, expected, &previous, &next, None).unwrap();
        previous = next;
        let next = previous
            .publishing_file(&expected.header().checkout_identity)
            .unwrap();
        checkout_journal::advance(c, expected, &previous, &next, None).unwrap();
        previous = next;
        let next = previous.published_file().unwrap();
        checkout_journal::advance(c, expected, &previous, &next, None).unwrap();
    }
}

fn reengage(s: &Connection, f: &mut Fixture, files: &[CheckoutFile]) -> VerifiedImportClosure {
    // The older shared fixture deliberately uses a synthetic image digest. This
    // new recipe engages the real deterministic metadata commitment before any
    // destination row exists; the original fixture assertions are untouched.
    f.capture.changes[0]
        .replacement
        .as_mut()
        .unwrap()
        .executable = false;
    f.capture.digest = f.capture.computed_digest().unwrap();
    let image = inventory::image_digest(&f.capture, files, &[], None).unwrap();
    let ImportSource::Provider { version, .. } = &mut f.prefix.source else {
        unreachable!()
    };
    version.effective_image_digest = image.clone();
    f.marker.import_digest = Some(f.prefix.digest().unwrap());
    f.copying.plan.source_prefix =
        ProviderImportPrefixRecord::decode(&f.prefix.encode().unwrap()).unwrap();
    f.copying.plan.source_prefix_id = f.prefix.digest().unwrap();
    f.copying.plan.capture_digest = f.capture.digest.clone();
    let encoded = canonical_json(
        &json!({"change":&f.capture.changes[0],"manifestId":&f.overlay[0].manifest_id}),
    )
    .unwrap();
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-provider-managed-overlay-v1\0");
    hash.update(&1u64.to_be_bytes());
    hash.update(&(encoded.len() as u64).to_be_bytes());
    hash.update(&encoded);
    f.copying.plan.working_overlay.digest = format!("blake3:{}", hash.finalize());
    f.copying.plan.quota_bytes =
        managed_budget::quota(&f.required, files.iter().map(|f| f.bytes), 32 * 1024 * 1024)
            .unwrap();
    f.copying.destination.as_mut().unwrap().format = f.marker.clone();
    f.staging = ProviderTransitionIntentRecord::decode(&f.copying.encode().unwrap()).unwrap();
    f.staging.state = TransitionState::Staging;
    f.staging.reservation = Some(f.staging.reservation_operation().unwrap());
    f.receipt.source_prefix =
        ProviderImportPrefixRecord::decode(&f.prefix.encode().unwrap()).unwrap();
    f.receipt.source_prefix_id = f.prefix.digest().unwrap();
    f.receipt.transition_plan_digest = f.staging.plan.digest().unwrap();
    let proof = f.staging.destination.as_ref().unwrap();
    f.receipt.destination_proof_digest = proof.digest(&f.staging.plan).unwrap();
    f.receipt.owner_identity_digest = proof.owner_digest(&f.staging.plan).unwrap();
    f.receipt.destination_capture_digest = f.capture.digest.clone();
    f.ready = ProviderTransitionIntentRecord::decode(&f.staging.encode().unwrap()).unwrap();
    f.ready.state = TransitionState::Ready;
    f.ready.receipt_id = Some(f.receipt.digest().unwrap());
    f.ready.require_transition(&f.staging).unwrap();
    f.ready.require_receipt(&f.receipt).unwrap();
    let active = private_active_capture_with_source(
        s,
        &f.source.allocation,
        &f.source.prepared,
        &f.activation.binding,
        Some([0x41; 16]),
        None,
    )
    .unwrap();
    import_validation::verify_source(
        s,
        &f.source.allocation.format,
        ImportDatabaseKind::Provider,
        &f.prefix,
        crate::projection::provider::records::import::ImportSourceReferences::Provider {
            allocation: &f.source.allocation,
            prepared: &f.source.prepared,
            activation: &f.activation,
        },
        Some(&active),
        &image,
        None,
    )
    .unwrap()
}

fn with_ready(body: impl FnOnce(&Connection, &Connection, &Fixture)) {
    let source = BareConnection::reference().unwrap();
    let destination = BareConnection::reference().unwrap();
    let family = BareConnection::reference().unwrap();
    let (mut f, old) = Fixture::new(source.connection());
    drop(old);
    let files = [CheckoutFile {
        path: f.capture.changes[0].path.clone(),
        manifest_id: f.current.id().clone(),
        file_id: f.current.file_id().clone(),
        bytes: f.current.byte_length(),
        executable: false,
    }];
    let closure = reengage(source.connection(), &mut f, &files);
    f.stage(source.connection(), destination.connection(), &closure);
    f.populate_family(family.connection());
    family
        .connection()
        .execute("UPDATE quota SET bytes=67108864", [])
        .unwrap();
    destination
        .connection()
        .execute_batch("BEGIN IMMEDIATE")
        .unwrap();
    let image = match &f.prefix.source {
        ImportSource::Provider { version, .. } => &version.effective_image_digest,
        _ => unreachable!(),
    };
    let expected = ExpectedCheckout::new(&f.staging, &files, &[], image, None).unwrap();
    complete(destination.connection(), &expected);
    let data = f.data();
    super::super::finalize_checkout_or_replay(
        source.connection(),
        destination.connection(),
        family.connection(),
        &f.inputs(&closure, &data),
        &expected,
        None,
    )
    .unwrap();
    family
        .connection()
        .execute(
            "UPDATE worktrees SET state='ready' WHERE id=?1",
            [f.marker.identity.worktree_id.as_str()],
        )
        .unwrap();
    let owned =
        family_read::read(family.connection(), &f.marker, &f.ready, &f.claims[0], None).unwrap();
    family
        .connection()
        .execute(
            "INSERT INTO reservations VALUES(?1,?2,?1,?3)",
            params![
                owned.binding.reservation,
                owned.binding.owner.as_str(),
                owned.binding.quota_bytes
            ],
        )
        .unwrap();
    let mut debit = 0u64;
    for (ordinal, manifest) in owned.required.iter().enumerate() {
        let chunks = manifest.chunks().len();
        assert!(chunks <= 1024);
        // All CAS objects were indexed by populate_family. Mark every actual
        // chunk resident (including no bits for an empty manifest), with no
        // chunk/object publication debit. A nibble represents four ordinals.
        let resident: String = (0..256)
            .map(|n| {
                let bits = chunks.saturating_sub(n * 4).min(4);
                b"0123456789abcdef"[(1usize << bits) - 1] as char
            })
            .collect();
        let payload = canonical_json(manifest).unwrap();
        let charge = 8192 + payload.len() as u64;
        debit = debit.checked_add(charge).unwrap();
        let record = json!({"schemaVersion":"layrs.shared-exact-installation.v1","binding":&owned.binding,"manifestId":manifest.id(),"fileId":manifest.file_id(),"bytes":manifest.byte_length(),"directory":format!("1-2-{ordinal}"),"directoryIdentity":"fs:v1:0000000000000001:0000000000000009","phase":"published","spoolCursor":chunks,"publishCursor":chunks,"active":null,"chargedChunks":"0".repeat(256),"residentChunks":resident,"chargedManifest":false,"debitBytes":charge});
        family
            .connection()
            .execute(
                "INSERT INTO exact_installations VALUES(?1,?2,?3,?4,?5,?6,1)",
                params![
                    owned.binding.reservation,
                    manifest.id().as_str(),
                    manifest.file_id().as_str(),
                    manifest.byte_length(),
                    canonical_json(&record).unwrap(),
                    payload
                ],
            )
            .unwrap();
    }
    family
        .connection()
        .execute(
            "UPDATE reservations SET remaining=?1 WHERE id=?2",
            params![owned.binding.quota_bytes - debit, owned.binding.reservation],
        )
        .unwrap();
    // Simulate an already retired source registry member, entirely in fixture
    // DATA. The production reader must never depend on that current member.
    family
        .connection()
        .execute(
            "DELETE FROM root_manifests WHERE root_id=?1",
            [&f.source_root.0],
        )
        .unwrap();
    family
        .connection()
        .execute("DELETE FROM roots WHERE id=?1", [&f.source_root.0])
        .unwrap();
    family
        .connection()
        .execute(
            "DELETE FROM worktrees WHERE id=?1",
            [f.source.allocation.format.identity.worktree_id.as_str()],
        )
        .unwrap();
    family
        .connection()
        .execute_batch("COMMIT; BEGIN DEFERRED")
        .unwrap();
    destination
        .connection()
        .execute_batch("COMMIT; BEGIN DEFERRED")
        .unwrap();
    drop(closure);
    source.connection().execute_batch("ROLLBACK").unwrap();
    source.finish(Ok(())).unwrap();
    body(destination.connection(), family.connection(), &f);
    for c in [destination.connection(), family.connection()] {
        if !c.is_autocommit() {
            c.execute_batch("ROLLBACK").unwrap();
        }
    }
    destination.finish(Ok(())).unwrap();
    family.finish(Ok(())).unwrap();
}

fn refuses<T>(result: Result<T>, expected: &str) {
    let error = match result {
        Ok(_) => panic!("expected refusal {expected}"),
        Err(e) => e,
    };
    assert!(
        format!("{error:?}").contains(expected),
        "unexpected refusal {error:?}"
    );
}

owned!(
    autonomous_managed_ready_survives_source_removal_and_reads_ro,
    {
        with_ready(|c, family, f| {
            for connection in [c, family] {
                connection
                    .execute_batch("COMMIT; PRAGMA query_only=ON; BEGIN DEFERRED")
                    .unwrap();
            }
            let changes = (c.total_changes(), family.total_changes());
            let read = read_managed_ready(c, &f.marker, family, None).unwrap();
            assert_eq!(read.receipt(), &f.receipt);
            assert_eq!(read.ready(), &f.ready);
            assert_eq!(read.required().len(), 3);
            assert!(read.journal().is_complete());
            assert_ne!(f.historical.id(), f.current.id());
            assert_eq!(f.historical.file_id(), f.current.file_id());
            assert_eq!(read.expected().files()[0].manifest_id, *f.current.id());
            assert_eq!(read.installation().published, 3);
            assert_eq!(read.installation().records, 3);
            assert_eq!(
                read.installation().remaining + read.installation().debit_bytes,
                f.ready.plan.quota_bytes
            );
            read.require(c, family, None).unwrap();
            let (source,destination):(u64,u64)=family.query_row("SELECT (SELECT count(*) FROM worktrees WHERE id=?1),(SELECT count(*) FROM worktrees WHERE id=?2 AND state='ready')",params![f.source.allocation.format.identity.worktree_id.as_str(),f.marker.identity.worktree_id.as_str()],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
            assert_eq!((source, destination), (0, 1));
            let cancel = Cancellation::default();
            cancel.cancel();
            assert!(read_managed_ready(c, &f.marker, family, Some(&cancel)).is_err());
            assert_eq!((c.total_changes(), family.total_changes()), changes);
            c.execute_batch("COMMIT").unwrap();
            assert!(read.require(c, family, None).is_err());
        });
    }
);

owned!(
    autonomous_managed_ready_rejects_suffix_journal_phase_and_debit_corruption,
    {
        with_ready(|c, family, f| {
            let read = read_managed_ready(c, &f.marker, family, None).unwrap();
            c.execute_batch("SAVEPOINT probe").unwrap();
            c.execute("UPDATE projection_head SET generation=2", [])
                .unwrap();
            let changes = c.total_changes();
            refuses(
                read_managed_ready(c, &f.marker, family, None),
                "destination Ready head differs",
            );
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK TO probe; RELEASE probe").unwrap();
            c.execute_batch("SAVEPOINT probe").unwrap();
            let mut row = read.journal().row(RowKey::File(0)).unwrap().clone();
            let CheckoutEntry::File { file, .. } = &mut row.entry else {
                unreachable!()
            };
            file.manifest_id = f.historical.id().clone();
            c.execute(
                "UPDATE projection_prejournal SET payload=?1,digest=?2 WHERE path=?3",
                params![
                    row.encode().unwrap(),
                    row.digest().unwrap(),
                    read.expected().sql_key(RowKey::File(0)).unwrap()
                ],
            )
            .unwrap();
            let changes = c.total_changes();
            assert!(read_managed_ready(c, &f.marker, family, None).is_err());
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK TO probe; RELEASE probe").unwrap();
            family.execute_batch("SAVEPOINT probe").unwrap();
            family
                .execute(
                    "UPDATE worktrees SET state='initializing' WHERE id=?1",
                    [f.marker.identity.worktree_id.as_str()],
                )
                .unwrap();
            let changes = family.total_changes();
            refuses(
                read_managed_ready(c, &f.marker, family, None),
                "autonomous Ready destination family member differs",
            );
            assert_eq!(family.total_changes(), changes);
            family
                .execute_batch("ROLLBACK TO probe; RELEASE probe")
                .unwrap();
            family.execute_batch("SAVEPOINT probe").unwrap();
            family
                .execute(
                    "UPDATE reservations SET remaining=remaining-1 WHERE owner=?1",
                    [f.marker.identity.worktree_id.as_str()],
                )
                .unwrap();
            let changes = family.total_changes();
            refuses(
                read_managed_ready(c, &f.marker, family, None),
                "exact reservation does not equal quota minus durable debits",
            );
            assert_eq!(family.total_changes(), changes);
            family
                .execute_batch("ROLLBACK TO probe; RELEASE probe")
                .unwrap();
            read.require(c, family, None).unwrap();
            let (source, destination) = managed_budget::original_staging_sizes(&f.ready).unwrap();
            assert_eq!(
                destination.payload,
                managed_budget::intent_size(&f.staging).unwrap().payload
            );
            assert_eq!(
                destination.scalars,
                managed_budget::intent_size(&f.staging).unwrap().scalars
            );
            let mut source_data =
                ProviderTransitionIntentRecord::decode(&f.staging.encode().unwrap()).unwrap();
            source_data.role = TransitionRole::Source;
            source_data.local_generation = 1;
            assert_eq!(
                source.payload,
                managed_budget::intent_size(&source_data).unwrap().payload
            );
            assert_eq!(
                source.scalars,
                managed_budget::intent_size(&source_data).unwrap().scalars
            );
        });
    }
);
