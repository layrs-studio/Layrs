//! Owned durable Active reopening and explicit crash-recovery recipes.
use super::super::preparation::activation::tests::hooks as activation_hooks;
use super::super::preparation::{
    ProviderActivationOutcome as Outcome, ProviderActivationSqlPhase as Phase,
    ProviderActivationWriteStep as Step, ProviderPreparationOutcome,
};
use super::*;
use std::fs;

macro_rules! owned {
    ($name:ident,$body:block) => {
        #[test]
        #[ignore = "UNQUALIFIED until ROOT releases this exact owned audited-host active reopen recipe"]
        fn $name() {
            child::run(concat!(module_path!(), "::", stringify!($name)), 240, || {
                child::declare();
                hooks::reset();
                activation_hooks::reset();
                $body
            });
        }
    };
}

fn binding(value: &ProviderBootstrapBinding) -> ProviderBootstrapBinding {
    ProviderBootstrapBinding::decode(&value.encode().unwrap()).unwrap()
}

fn prepared(
    f: &Fixture,
    nonempty: bool,
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
    let prepared = match published.prepare_provider([0x82; 16]) {
        ProviderPreparationOutcome::Prepared(value) => value,
        ProviderPreparationOutcome::Withheld(value) => {
            panic!("prepare withheld: {:?}", value.failure())
        }
    };
    (expected, prepared)
}

fn open(
    f: &Fixture,
    expected: &ProviderBootstrapBinding,
) -> std::result::Result<
    super::super::preparation::ProviderActiveReopened,
    super::super::preparation::ProviderActiveOpenFailure,
> {
    PartialStoreFamily::open_provider_active(f.family.root(), &f.worktree, fixture::TX, expected)
}

owned!(ready_reopens_readonly_with_deleted_cold_placeholder, {
    let f = Fixture::new();
    let (expected, prepared) = prepared(&f, true);
    let active = match prepared.activate() {
        Outcome::Active(value) => value,
        Outcome::Withheld(value) => panic!("activate withheld: {:?}", value.failure()),
    };
    let digest = active.digest().to_owned();
    drop(active);
    let file = f.checkout.join("README.md");
    assert!(fs::metadata(&file).unwrap().len() > 0);
    fs::remove_file(&file).unwrap();
    let before = f.witness();
    let reopened = open(&f, &expected).unwrap();
    assert_eq!(reopened.root(), f.checkout.as_path());
    assert_eq!(reopened.binding(), &expected);
    assert_eq!(reopened.generation(), 1);
    assert_eq!(reopened.digest(), digest);
    reopened.revalidate().unwrap();
    assert!(!file.exists());
    drop(reopened);
    assert_eq!(f.witness(), before);
});

owned!(initializing_v2_and_v3_refuse_active_open, {
    let f = Fixture::new();
    let (expected, prepared) = prepared(&f, false);
    drop(prepared);
    let before = f.witness();
    assert!(open(&f, &expected).is_err());
    assert_eq!(f.witness(), before);

    let reopened = PartialStoreFamily::open_provider_prepared(
        f.family.root(),
        &f.worktree,
        fixture::TX,
        &expected,
    )
    .unwrap();
    activation_hooks::stop(Step::SqlCommit(Phase::PrivateActivation), true);
    let withheld = match reopened.activate() {
        Outcome::Withheld(value) => value,
        Outcome::Active(_) => panic!("checkpoint did not stop activation"),
    };
    drop(withheld);
    activation_hooks::reset();
    let before = f.witness();
    assert!(open(&f, &expected).is_err());
    assert_eq!(f.witness(), before);
});

owned!(disk_resume_after_private_commit_only_writes_family_ready, {
    let f = Fixture::new();
    let (expected, prepared) = prepared(&f, true);
    activation_hooks::stop(Step::SqlCommit(Phase::PrivateActivation), true);
    let withheld = match prepared.activate() {
        Outcome::Withheld(value) => value,
        Outcome::Active(_) => panic!("checkpoint did not stop activation"),
    };
    drop(withheld);
    activation_hooks::reset();
    let active = match PartialStoreFamily::resume_provider_activation(
        f.family.root(),
        &f.worktree,
        fixture::TX,
        &expected,
    )
    .unwrap()
    {
        Outcome::Active(value) => value,
        Outcome::Withheld(value) => panic!("disk resume withheld: {:?}", value.failure()),
    };
    assert_eq!(active.generation(), 1);
    assert!(active.report().resumed);
    assert!(active.report().previous_effects.is_empty());
    assert!(active.report().effects.iter().all(|effect| !matches!(
        effect.step,
        Step::SqlOpen(Phase::PrivateActivation)
            | Step::SqlTransaction(Phase::PrivateActivation)
            | Step::SqlCommit(Phase::PrivateActivation)
            | Step::SqlClose(Phase::PrivateActivation)
    )));
    assert!(active
        .report()
        .effects
        .iter()
        .any(|effect| effect.step == Step::SqlCommit(Phase::FamilyReady)));
    active.revalidate().unwrap();
    drop(active);
    let before = f.witness();
    let active = match PartialStoreFamily::resume_provider_activation(
        f.family.root(),
        &f.worktree,
        fixture::TX,
        &expected,
    )
    .unwrap()
    {
        Outcome::Active(value) => value,
        Outcome::Withheld(value) => panic!("idempotent resume withheld: {:?}", value.failure()),
    };
    assert!(active.report().effects.is_empty());
    drop(active);
    assert_eq!(f.witness(), before);
});

owned!(binding_and_active_mount_tamper_refuse_without_writes, {
    for mode in 0..2 {
        let f = Fixture::new();
        let (mut expected, prepared) = prepared(&f, false);
        let active = match prepared.activate() {
            Outcome::Active(value) => value,
            Outcome::Withheld(value) => panic!("activate withheld: {:?}", value.failure()),
        };
        drop(active);
        if mode == 0 {
            expected.transaction_id = "00000000000000000000000000000002".into();
        } else {
            let c = rusqlite::Connection::open_with_flags(
                f.metadata.join("projection.sqlite3"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .unwrap();
            assert_eq!(
                c.execute(
                    "UPDATE projection_provider_mounts SET digest=?1",
                    [format!("blake3:{}", "0".repeat(64))],
                )
                .unwrap(),
                1
            );
            c.close().unwrap();
        }
        let before = f.witness();
        assert!(open(&f, &expected).is_err());
        assert_eq!(f.witness(), before);
    }
});

owned!(
    canonical_alternate_native_ticket_passes_sql_but_refuses_reopen,
    {
        use crate::projection::provider::records::{
            activation::{ProviderActivationRecord, ProviderActiveMountRecord},
        preparation::PreparationState,
            Record,
        };
        let f = Fixture::new();
        let (expected, prepared) = prepared(&f, false);
        let active = match prepared.activate() {
            Outcome::Active(value) => value,
            Outcome::Withheld(value) => panic!("activate withheld: {:?}", value.failure()),
        };
        drop(active);
        let private = f.metadata.join("projection.sqlite3");
        let c = rusqlite::Connection::open_with_flags(
            &private,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        let (mount_bytes, activation_bytes): (Vec<u8>, Vec<u8>) = (
            c.query_row("SELECT payload FROM projection_provider_mounts", [], |r| {
                r.get(0)
            })
            .unwrap(),
            c.query_row(
                "SELECT payload FROM projection_provider_activations",
                [],
                |r| r.get(0),
            )
            .unwrap(),
        );
        let mut mount = ProviderActiveMountRecord::decode(&mount_bytes).unwrap();
        let mut ticket: Vec<u8> = mount
            .prepared
            .registration_ticket_hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        assert_eq!(&ticket[..8], b"LYRTICK2");
        ticket[8..24].copy_from_slice(&[0x83; 16]);
        mount.prepared.registration_ticket_hex =
            ticket.iter().map(|byte| format!("{byte:02x}")).collect();
        mount.prepared.root_id_hex = "83".repeat(16);
        mount.prepared.state = PreparationState::RegistrationPending;
        let pending_digest = mount.prepared.digest().unwrap();
        mount.prepared.state = PreparationState::ProviderPrepared { pending_digest };
        let mut activation = ProviderActivationRecord::decode(&activation_bytes).unwrap();
        activation.prepared_digest = mount.prepared.digest().unwrap();
        mount.activation_digest = activation.digest().unwrap();
        assert_eq!(
            c.execute(
                "UPDATE projection_provider_activations SET id=?1,payload=?2",
                rusqlite::params![activation.digest().unwrap(), activation.encode().unwrap()],
            )
            .unwrap(),
            1
        );
        assert_eq!(
            c.execute(
                "UPDATE projection_provider_mounts SET payload=?1,digest=?2",
                rusqlite::params![mount.encode().unwrap(), mount.digest().unwrap()],
            )
            .unwrap(),
            1
        );
        c.close().unwrap();
        let allocation = AllocationRecord::decode(
            &fs::read(f.receipt(ProviderBootstrapReceipt::Allocation)).unwrap(),
        )
        .unwrap();
        let prepared = PreparedRecord::decode(
            &fs::read(f.receipt(ProviderBootstrapReceipt::Prepared)).unwrap(),
        )
        .unwrap();
        let c = rusqlite::Connection::open_with_flags(
            &private,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let interpreted = super::super::super::validation::private_active_with_source(
            &c,
            &allocation,
            &prepared,
            &expected,
            None,
            None,
        )
        .unwrap();
        assert_eq!(interpreted.mount, mount);
        assert_eq!(interpreted.activation, activation);
        c.close().unwrap();
        let before = f.witness();
        let error = open(&f, &expected)
            .err()
            .expect("foreign native READY accepted");
        assert!(matches!(error.cause, Cause::Vfs(_)));
        assert_eq!(f.witness(), before);
    }
);
