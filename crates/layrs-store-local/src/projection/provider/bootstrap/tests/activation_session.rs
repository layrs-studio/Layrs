//! Owned callback recipes for the local Active SnapshotSource and lifetime lease.
use super::super::preparation::{ProviderActivationOutcome, ProviderPreparationOutcome};
use super::*;
use layrs_core::{
    projection::{AvailabilityRequest, OfflineOperation},
    scm::{RepoPath, WorktreeId},
};
use std::{fs, io::Write};

#[test]
#[ignore = "only the owned active-session recipe launches this exact external reader"]
fn external_reader() {
    let path = std::path::PathBuf::from(
        std::env::var_os("LAYRS_PROVIDER_SESSION_READ_PATH").expect("owned reader path"),
    );
    assert!(path.is_absolute());
    match std::env::var("LAYRS_PROVIDER_SESSION_READ_MODE").as_deref() {
        Ok("resident") => assert_eq!(
            fs::read(path).unwrap(),
            b"native metadata without Store content"
        ),
        Ok("cold") => {
            assert!(fs::read(path).is_err());
        }
        _ => panic!("unknown owned reader mode"),
    }
}

fn external_read(path: &std::path::Path, mode: &str) {
    use std::{
        os::windows::process::CommandExt,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    assert!(matches!(mode, "resident" | "cold"));
    let name = concat!(module_path!(), "::external_reader");
    let name = name.strip_prefix("layrs_store_local::").unwrap_or(name);
    let mut child = child::OwnedChild::new(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                name,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("LAYRS_PROVIDER_SESSION_READ_PATH", path)
            .env("LAYRS_PROVIDER_SESSION_READ_MODE", mode)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .creation_flags(0x08000000)
            .spawn()
            .expect("launch exact owned external reader"),
    );
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("query owned external reader") {
            assert!(status.success(), "external reader exited {status}");
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "external reader exceeded bound"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

macro_rules! owned {
    ($name:ident,$body:block) => {
        #[test]
        #[ignore = "UNQUALIFIED until ROOT releases this exact owned audited-host active-session recipe"]
        fn $name() {
            child::run(concat!(module_path!(), "::", stringify!($name)), 240, || {
                child::declare(); hooks::reset(); $body
            });
        }
    };
}

fn active(
    f: &Fixture,
    source: ProviderBootstrapSource,
) -> super::super::preparation::ProviderActive {
    let published = match f
        .family
        .begin_provider_bootstrap(&f.checkout, &f.stages, &f.worktree, fixture::TX, source)
        .unwrap()
    {
        ProviderControlPublicationOutcome::Published(value) => value,
        _ => panic!("session source did not publish"),
    };
    let prepared = match published.prepare_provider([0x7b; 16]) {
        ProviderPreparationOutcome::Prepared(value) => value,
        ProviderPreparationOutcome::Withheld(value) => {
            panic!("session prepare withheld: {:?}", value.failure())
        }
    };
    match prepared.activate() {
        ProviderActivationOutcome::Active(value) => value,
        ProviderActivationOutcome::Withheld(value) => {
            panic!("session activation withheld: {:?}", value.failure())
        }
    }
}

fn lifecycle_denied(f: &Fixture) {
    let error = f
        .family
        .reconcile_retention()
        .err()
        .expect("GC crossed active session lease");
    match error {
        ProjectionStoreError::Invalid(message) => {
            assert!(message.contains("incompatible process-local lock"));
            assert!(message.contains("lifecycle.lock"));
        }
        other => panic!("GC refusal was not lifecycle contention: {other:?}"),
    }
}

owned!(
    cold_callback_reports_missing_content_and_drains_before_lease_release,
    {
        let f = Fixture::new();
        let source = super::preparation::cold_nonempty_source();
        let mut session = active(&f, source)
            .into_disconnected_session()
            .unwrap_or_else(|e| panic!("local session withheld: {}", e.cause()));
        assert!(!session.is_connected());
        lifecycle_denied(&f);
        session.connect().unwrap();
        external_read(&f.checkout.join("README.md"), "cold");
        let traces = session.hydration_requests();
        assert!(!traces.is_empty());
        assert!(
            traces
                .iter()
                .any(|trace| trace.source_bytes_read == 0 && trace.transferred_bytes == 0)
        );
        assert!(!session.callback_failures().is_empty());
        lifecycle_denied(&f);
        session.disconnect().unwrap();
        assert!(!session.is_connected());
        lifecycle_denied(&f);
        drop(session);
        let exclusive = crate::lock::StoreLock::acquire_existing(
            &f.family.root().join("metadata/lifecycle.lock"),
            crate::lock::SessionMode::Exclusive,
        )
        .unwrap();
        drop(exclusive);
    }
);

owned!(
    resident_callback_reads_verified_local_content_without_network,
    {
        let f = Fixture::new();
        let source = super::preparation::cold_nonempty_source();
        let bytes = b"native metadata without Store content";
        let root = f.outer.join("ordinary");
        fs::create_dir(&root).unwrap();
        let ordinary = f
            .family
            .create_worktree(
                &root,
                WorktreeId::parse("worktree:session-other").unwrap(),
                source.initial().clone(),
                source.selection().clone(),
            )
            .unwrap();
        let request = AvailabilityRequest {
            version: ordinary.current_version().unwrap(),
            other: None,
            paths: vec![RepoPath::parse("README.md").unwrap()],
            operation: OfflineOperation::Read,
        };
        let intent = ordinary.begin_preparation(&request).unwrap();
        ordinary
            .resume_preparation(&intent.id, &mut |_, _, out| out.write_all(bytes))
            .unwrap();
        drop(ordinary);
        let mut session = active(&f, source)
            .into_disconnected_session()
            .unwrap_or_else(|e| panic!("local session withheld: {}", e.cause()));
        assert!(!session.is_connected());
        session.connect().unwrap();
        external_read(&f.checkout.join("README.md"), "resident");
        assert!(
            session
                .hydration_requests()
                .iter()
                .any(|trace| trace.source_bytes_read > 0 && trace.transferred_bytes > 0)
        );
        assert!(session.callback_failures().is_empty());
        session.disconnect().unwrap();
        drop(session);
        let exclusive = crate::lock::StoreLock::acquire_existing(
            &f.family.root().join("metadata/lifecycle.lock"),
            crate::lock::SessionMode::Exclusive,
        )
        .unwrap();
        drop(exclusive);
    }
);
