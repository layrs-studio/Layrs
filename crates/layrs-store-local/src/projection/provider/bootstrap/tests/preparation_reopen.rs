//! Exact owned-host recipes for strictly read-only durable READY reopening.
use super::super::preparation::tests::hooks as preparation_hooks;
use super::super::preparation::{
    ProviderPreparationOutcome as Outcome, ProviderPreparationSqlPhase as Phase,
    ProviderPreparationWriteStep as Step,
};
use super::*;
use std::fs;

macro_rules! owned {
    ($name:ident,$seconds:literal,$body:block) => {
        #[test]
        #[ignore = "UNQUALIFIED until ROOT releases this exact owned audited-host reopen recipe"]
        fn $name() {
            child::run(
                concat!(module_path!(), "::", stringify!($name)),
                $seconds,
                || {
                    child::declare();
                    hooks::reset();
                    preparation_hooks::reset();
                    $body
                },
            );
        }
    };
}

fn binding(value: &ProviderBootstrapBinding) -> ProviderBootstrapBinding {
    ProviderBootstrapBinding::decode(&value.encode().unwrap()).unwrap()
}

fn prepare(
    f: &Fixture,
    nonempty: bool,
    root_id: u8,
) -> (
    ProviderBootstrapBinding,
    super::super::preparation::ProviderPrepared,
) {
    let published = if nonempty {
        match f
            .family
            .begin_provider_bootstrap(
                &f.checkout,
                &f.stages,
                &f.worktree,
                fixture::TX,
                super::preparation::cold_nonempty_source(),
            )
            .unwrap()
        {
            ProviderControlPublicationOutcome::Published(value) => value,
            _ => panic!("cold source did not publish"),
        }
    } else {
        f.published()
    };
    let expected = binding(published.binding());
    let prepared = match published.prepare_provider([root_id; 16]) {
        Outcome::Prepared(value) => value,
        Outcome::Withheld(value) => panic!("setup prepare withheld: {:?}", value.failure()),
    };
    (expected, prepared)
}

fn reopen(
    f: &Fixture,
    binding: &ProviderBootstrapBinding,
) -> std::result::Result<
    super::super::preparation::ProviderPreparedReopened,
    super::super::preparation::ProviderPreparedOpenFailure,
> {
    PartialStoreFamily::open_provider_prepared(f.family.root(), &f.worktree, fixture::TX, binding)
}

owned!(ready_reopens_readonly_after_all_handles_drop, 240, {
    let f = Fixture::new();
    let (expected, prepared) = prepare(&f, true, 0x60);
    drop(prepared);
    let before = f.witness();
    let reopened = reopen(&f, &expected).unwrap();
    assert_eq!(reopened.root(), f.checkout.as_path());
    assert_eq!(reopened.binding(), &expected);
    reopened.revalidate().unwrap();
    drop(reopened);
    assert_eq!(f.witness(), before);
});

owned!(
    ready_reopens_with_user_deleted_placeholder_unchanged,
    240,
    {
        let f = Fixture::new();
        let (expected, prepared) = prepare(&f, true, 0x61);
        drop(prepared);
        let file = f.checkout.join("README.md");
        assert!(fs::metadata(&file).unwrap().len() > 0);
        fs::remove_file(&file).unwrap();
        let before = f.witness();
        let reopened = reopen(&f, &expected).unwrap();
        assert!(!file.exists());
        reopened.revalidate().unwrap();
        drop(reopened);
        assert_eq!(f.witness(), before);
    }
);

owned!(pending_unregistered_and_ready_are_both_refused, 240, {
    for (root_id, stop) in [
        (0x62, Step::SqlClose(Phase::RegistrationPending)),
        (0x63, Step::NativePopulation),
    ] {
        let f = Fixture::new();
        let published = f.published();
        let expected = binding(published.binding());
        preparation_hooks::stop(stop, true);
        let withheld = match published.prepare_provider([root_id; 16]) {
            Outcome::Withheld(value) => value,
            Outcome::Prepared(_) => panic!("pending test unexpectedly prepared"),
        };
        assert!(withheld.pending_candidate_retained());
        assert_eq!(
            withheld.retained_native_ready_proof(),
            stop == Step::NativePopulation
        );
        drop(withheld);
        preparation_hooks::reset();
        let error = reopen(&f, &expected)
            .err()
            .expect("pending must refuse reopen");
        assert_eq!(error.database, Some(ProviderDatabaseScope::PrivateMetadata));
    }
});

owned!(wrong_binding_ticket_and_family_registry_refuse, 240, {
    for mode in 0..3 {
        let f = Fixture::new();
        let (mut expected, prepared) = prepare(&f, false, 0x64 + mode);
        drop(prepared);
        if mode == 0 {
            expected.transaction_id = "00000000000000000000000000000002".into();
        } else {
            let path = if mode == 1 {
                f.metadata.join("projection.sqlite3")
            } else {
                f.family.root().join("metadata/catalog.sqlite3")
            };
            let c = rusqlite::Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .unwrap();
            if mode == 1 {
                let payload: Vec<u8> = c
                    .query_row(
                        "SELECT payload FROM projection_provider_mounts",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                let mut document: serde_json::Value = serde_json::from_slice(&payload).unwrap();
                let ticket = document["registrationTicketHex"].as_str().unwrap();
                let mut changed = ticket.to_owned();
                let replacement = if changed.ends_with('0') { "1" } else { "0" };
                let last = changed.len() - 1;
                changed.replace_range(last.., replacement);
                document["registrationTicketHex"] = changed.into();
                let changed = c
                    .execute(
                        "UPDATE projection_provider_mounts SET payload=?1",
                        [serde_json::to_vec(&document).unwrap()],
                    )
                    .unwrap();
                assert_eq!(changed, 1);
            } else {
                assert_eq!(
                    c.execute("DELETE FROM worktrees WHERE id=?1", [f.worktree.as_str()])
                        .unwrap(),
                    1
                );
            }
            c.close().unwrap();
        }
        let error = reopen(&f, &expected)
            .err()
            .expect("tamper must refuse reopen");
        if mode == 1 {
            assert_eq!(error.database, Some(ProviderDatabaseScope::PrivateMetadata));
        }
        if mode == 2 {
            assert_eq!(error.database, Some(ProviderDatabaseScope::FamilyCatalog));
        }
    }
});

owned!(
    canonical_alternate_root_ticket_passes_sql_but_native_ready_refuses,
    240,
    {
        use super::super::super::records::{
            Receipt,
            preparation::{PreparationState, ProviderPreparationRecord},
        };
        let f = Fixture::new();
        let (expected, prepared) = prepare(&f, false, 0x68);
        drop(prepared);
        let private = f.metadata.join("projection.sqlite3");
        let c = rusqlite::Connection::open_with_flags(
            &private,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        let payload: Vec<u8> = c
            .query_row(
                "SELECT payload FROM projection_provider_mounts",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut record = ProviderPreparationRecord::decode(&payload).unwrap();
        let mut ticket: Vec<u8> = record
            .registration_ticket_hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        assert_eq!(&ticket[..8], b"LYRTICK2");
        ticket[8..24].copy_from_slice(&[0x69; 16]);
        record.registration_ticket_hex = ticket.iter().map(|byte| format!("{byte:02x}")).collect();
        record.root_id_hex = "69".repeat(16);
        record.state = PreparationState::RegistrationPending;
        let pending_digest = record.digest().unwrap();
        record.state = PreparationState::ProviderPrepared { pending_digest };
        assert_eq!(
            c.execute(
                "UPDATE projection_provider_mounts SET payload=?1,digest=?2",
                rusqlite::params![record.encode().unwrap(), record.digest().unwrap()],
            )
            .unwrap(),
            1
        );
        c.close().unwrap();

        let a = AllocationRecord::decode(
            &fs::read(f.receipt(ProviderBootstrapReceipt::Allocation)).unwrap(),
        )
        .unwrap();
        let p = PreparedRecord::decode(
            &fs::read(f.receipt(ProviderBootstrapReceipt::Prepared)).unwrap(),
        )
        .unwrap();
        let read = rusqlite::Connection::open_with_flags(
            &private,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let validated = super::super::super::validation::private_preparation_reopen_with_source(
            &read, &a, &p, &expected,
        )
        .unwrap();
        assert_eq!(validated.mount, record);
        read.close().unwrap();

        let before = f.witness();
        let error = reopen(&f, &expected)
            .err()
            .expect("different READY registration must refuse");
        assert!(matches!(error.cause, Cause::Vfs(_)));
        assert_eq!(f.witness(), before);
    }
);
