//! Exact owned native recipes for the provider-only recovery export.
use super::*;
use crate::projection::provider::bootstrap::preparation::{
    ProviderRecoveryExportClass, ProviderRecoveryExportOutcome, ProviderRecoveryExportStep,
};
use layrs_core::scm::RepoPath;
use layrs_vfs::Cancellation;
use std::fs;

fn database_witness(f: &Fixture) -> [(String, u64, String); 2] {
    [
        f.family.root().join("metadata/catalog.sqlite3"),
        f.metadata.join("projection.sqlite3"),
    ]
    .map(|path| crate::projection::provider::database_io::tests::support::witness(&path))
}

macro_rules! owned {
    ($name:ident,$body:block) => {
        #[test]
        #[ignore = "UNQUALIFIED until ROOT releases this exact audited-host provider recovery-export recipe"]
        fn $name() {
            child::run(concat!(module_path!(), "::", stringify!($name)), 240, || {
                child::declare(); hooks::reset(); $body
            });
        }
    };
}

owned!(
    ordinary_new_file_is_recovery_only_and_missing_cold_source_is_gap,
    {
        let (f, active, _, _, _) = capture_family::active_holder_with_writable_file_private();
        let mut session = active
            .into_disconnected_session()
            .unwrap_or_else(|e| panic!("session: {}", e.cause()));
        session.connect().unwrap();
        fs::write(f.checkout.join("notes.txt"), b"ordinary recovery bytes").unwrap();
        let destination = f.outer.join("exports");
        fs::create_dir(&destination).unwrap();
        let before = database_witness(&f);
        let outcome = session
            .export_quiesced(&destination, &Cancellation::default())
            .unwrap();
        let ProviderRecoveryExportOutcome::Incomplete(report) = outcome else {
            panic!("cold source gap was not reported")
        };
        assert!(!report.complete_selected);
        assert_eq!(report.gaps.len(), 1);
        assert_eq!(report.gaps[0].path, RepoPath::parse("README.md").unwrap());
        assert!(matches!(
            &report.gaps[0].cause,
            layrs_vfs::Error::MissingBytes { .. }
        ));
        let notes = report
            .files
            .iter()
            .find(|item| item.path.as_str() == "notes.txt")
            .unwrap();
        assert_eq!(
            notes.class,
            ProviderRecoveryExportClass::RecoveryOnlyUnknown
        );
        assert_eq!(
            fs::read(report.tree_root.join("notes.txt")).unwrap(),
            b"ordinary recovery bytes"
        );
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&report.manifest_path).unwrap()).unwrap();
        assert_eq!(manifest["completeSelected"], false);
        assert_eq!(database_witness(&f), before);
        session.disconnect().unwrap();
    }
);

owned!(readonly_edited_file_is_recovery_only_with_actual_bytes, {
    let (f, active, _, _, _) = capture_family::active_holder_with_private();
    let mut session = active
        .into_disconnected_session()
        .unwrap_or_else(|e| panic!("session: {}", e.cause()));
    session.connect().unwrap();
    fs::write(f.checkout.join("README.md"), b"private read-only edit").unwrap();
    let destination = f.outer.join("exports");
    fs::create_dir(&destination).unwrap();
    let outcome = session
        .export_quiesced(&destination, &Cancellation::default())
        .unwrap();
    let ProviderRecoveryExportOutcome::Complete(report) = outcome else {
        panic!("edited file unexpectedly incomplete")
    };
    let readme = report
        .files
        .iter()
        .find(|item| item.path.as_str() == "README.md")
        .unwrap();
    assert_eq!(
        readme.class,
        ProviderRecoveryExportClass::RecoveryOnlyProtected
    );
    assert_eq!(
        fs::read(report.tree_root.join("README.md")).unwrap(),
        b"private read-only edit"
    );
    assert!(report.complete_selected);
    session.disconnect().unwrap();
});

owned!(ordinary_replacement_at_known_writable_path_is_verified, {
    let (f, active, _, _, _) = capture_family::active_holder_with_writable_file_private();
    let mut session = active
        .into_disconnected_session()
        .unwrap_or_else(|e| panic!("session: {}", e.cause()));
    session.connect().unwrap();
    let path = f.checkout.join("README.md");
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"ordinary known replacement").unwrap();
    let destination = f.outer.join("exports");
    fs::create_dir(&destination).unwrap();
    let outcome = session
        .export_quiesced(&destination, &Cancellation::default())
        .unwrap();
    let ProviderRecoveryExportOutcome::Complete(report) = outcome else {
        panic!("ordinary replacement unexpectedly incomplete")
    };
    let readme = report
        .files
        .iter()
        .find(|item| item.path.as_str() == "README.md")
        .unwrap();
    assert_eq!(readme.class, ProviderRecoveryExportClass::ManagedCandidate);
    assert_eq!(
        fs::read(report.tree_root.join("README.md")).unwrap(),
        b"ordinary known replacement"
    );
    session.disconnect().unwrap();
});

owned!(
    export_parent_inside_family_refuses_before_creating_output,
    {
        let (f, active, _, _, _) = capture_family::active_holder_with_private();
        let mut session = active
            .into_disconnected_session()
            .unwrap_or_else(|e| panic!("session: {}", e.cause()));
        session.connect().unwrap();
        let before = database_witness(&f);
        let parent = f.family.root().join("metadata");
        let failure = session
            .export_quiesced(&parent, &Cancellation::default())
            .unwrap_err();
        assert_eq!(failure.step, ProviderRecoveryExportStep::Destination);
        assert!(failure.container_root.is_none());
        assert_eq!(database_witness(&f), before);
        session.disconnect().unwrap();
    }
);
