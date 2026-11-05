use super::*;
use crate::args::{CliCommand, parse_args};

fn parsed(args: &[&str]) -> Result<OfflineCommand, CliParseError> {
    parse(args.iter().map(|v| (*v).into()).collect())
}

#[test]
fn common_paths_default_to_current_and_comparisons_name_both_operands() {
    let OfflineCommand::Check(spec) = parsed(&["check", "read", "z", "a"]).unwrap() else {
        panic!()
    };
    assert_eq!(spec.version, VersionSelector::Current);
    assert_eq!(
        spec.paths,
        vec![RepoPath::parse("a").unwrap(), RepoPath::parse("z").unwrap()]
    );
    assert_eq!(spec.operation, OfflineOperation::Read);
    let OfflineCommand::Compare(spec) =
        parsed(&["compare", "generation:0", "capture:1", "file", "--content"]).unwrap()
    else {
        panic!()
    };
    assert_eq!(spec.version, VersionSelector::Generation(0));
    assert_eq!(spec.other, Some(VersionSelector::Capture(1)));
    assert_eq!(spec.operation, OfflineOperation::CompareContent);
    assert!(matches!(
        parse_args(["--space", "checkout", "partial", "offline", "status"])
            .unwrap()
            .command,
        CliCommand::Offline(OfflineCommand::Status)
    ));
    assert!(matches!(
        parsed(&["read", "binary", "--output", "new.bin"]).unwrap(),
        OfflineCommand::Read {
            max_bytes: 65536,
            version: VersionSelector::Current,
            ..
        }
    ));
}

#[test]
fn offline_flags_are_strict_and_cannot_trigger_network_or_overwrite_output() {
    for args in [
        vec!["status", "--endpoint", "https://example.test"],
        vec!["check", "read"],
        vec!["check", "read", "../outside"],
        vec!["check", "read", "a", "a"],
        vec!["check", "read", "a", "--hydrate"],
        vec!["check", "read", "a", "--other", "current"],
        vec!["check", "compare-content", "a"],
        vec!["check", "read", "a", "--version", "HEAD"],
        vec!["check", "read", "a", "--version", "capture:0"],
        vec!["check", "read", "a", "--version", "generation:-1"],
        vec!["check", "read", "a", "--version", "version-file:"],
        vec![
            "check",
            "read",
            "a",
            "--version",
            "current",
            "--version",
            "current",
        ],
        vec!["read", "a"],
        vec!["read", "a", "--output", "-"],
        vec!["read", "a", "--output", "out", "--max-bytes", "0"],
        vec!["read", "a", "--output", "out", "--max-bytes", "1048577"],
        vec!["read", "a", "--output", "out", "--overwrite"],
        vec!["quota", "--bytes", "18446744073709551615"],
        vec!["evict", "--all"],
        vec!["release", "../other"],
        vec!["family", "create", "family"],
        vec!["family", "convert", "--family", "family"],
        vec!["family", "resume"],
    ] {
        assert!(parsed(&args).is_err(), "accepted {args:?}");
    }
    assert!(parsed(&["check", "read", &"a".repeat(10000)]).is_err());
}

#[test]
fn families_require_explicit_paths_and_allow_no_source_deletion_option() {
    assert!(matches!(
        parsed(&["family", "create", "empty", "--quota-bytes", "1024"]).unwrap(),
        OfflineCommand::CreateFamily {
            quota_bytes: 1024,
            ..
        }
    ));
    assert!(matches!(
        parsed(&[
            "family",
            "convert",
            "--family",
            "family",
            "--destination",
            "new"
        ])
        .unwrap(),
        OfflineCommand::Convert { .. }
    ));
    assert!(matches!(
        parsed(&["family", "resume", "--destination", "retained"]).unwrap(),
        OfflineCommand::ResumeConversion { .. }
    ));
    assert!(
        parsed(&[
            "family",
            "convert",
            "--family",
            "family",
            "--destination",
            "new",
            "--remove-source"
        ])
        .is_err()
    );
}

#[test]
fn network_preparation_requires_exact_generation_and_separate_observation() {
    let args = vec!["partial-prepare".into(), "read".into(), "a".into()];
    assert!(parse_remote(args.clone(), None).is_err());
    assert!(parse_remote(args.clone(), Some("+1".into())).is_err());
    assert!(matches!(
        parse_remote(args, Some("2".into())).unwrap(),
        RemoteOfflineCommand::Prepare {
            expected_generation: 2,
            ..
        }
    ));
    assert!(
        parse_remote(
            vec!["partial-observe".into(), "@layer:main".into(), "a".into()],
            Some("1".into())
        )
        .is_err()
    );
    let step = format!("blake3:{}", "1".repeat(64));
    assert!(matches!(
        parse_remote(
            vec!["partial-observe".into(), step, "a".into()],
            Some("1".into())
        )
        .unwrap(),
        RemoteOfflineCommand::Observe {
            expected_generation: 1,
            ..
        }
    ));
    assert!(
        parse_remote(
            vec!["partial-resume-preparation".into(), "../../bad".into()],
            Some("1".into())
        )
        .is_err()
    );
}

#[test]
fn recovery_requires_explicit_bounded_targets_without_automatic_actions() {
    assert_eq!(
        parsed(&["cancel-reservation"]).unwrap(),
        OfflineCommand::CancelReservation
    );
    assert_eq!(
        parsed(&["release-generation", "0"]).unwrap(),
        OfflineCommand::ReleaseGeneration { generation: 0 }
    );
    assert_eq!(
        parsed(&["release-generation", "9223372036854775807"]).unwrap(),
        OfflineCommand::ReleaseGeneration {
            generation: i64::MAX as u64
        }
    );
    assert_eq!(
        parsed(&[
            "family",
            "abandon",
            "--family",
            "family",
            "--destination",
            "retained"
        ])
        .unwrap(),
        OfflineCommand::AbandonConversion {
            family: "family".into(),
            destination: "retained".into()
        }
    );
    for args in [
        vec!["cancel-reservation", "123"],
        vec!["cancel-reservation", "--all"],
        vec!["cancel-reservation", "--endpoint", "https://example.test"],
        vec!["release-generation"],
        vec!["release-generation", "-1"],
        vec!["release-generation", "+1"],
        vec!["release-generation", "9223372036854775808"],
        vec!["release-generation", "current"],
        vec!["release-generation", "1", "2"],
        vec!["release-generation", "1", "--force"],
        vec!["family", "abandon"],
        vec!["family", "abandon", "--family", "family"],
        vec!["family", "abandon", "--destination", "retained"],
        vec![
            "family",
            "abandon",
            "--family",
            "family",
            "--family",
            "other",
            "--destination",
            "retained",
        ],
        vec![
            "family",
            "abandon",
            "--family",
            "family",
            "--destination",
            "retained",
            "--destination",
            "other",
        ],
        vec![
            "family",
            "abandon",
            "--family",
            "family",
            "--destination",
            "retained",
            "--delete-files",
        ],
    ] {
        assert!(parsed(&args).is_err(), "accepted {args:?}");
    }
}

#[test]
fn restore_requires_a_separate_reviewed_artifact_and_exact_expectations() {
    let digest = format!("blake3:{}", "a".repeat(64));
    assert!(matches!(
        parsed(&[
            "restore-preview",
            "a.bin",
            "--expected-generation",
            "1",
            "--output",
            "new.json"
        ])
        .unwrap(),
        OfflineCommand::RestorePreview {
            version: VersionSelector::Current,
            expected_generation: 1,
            ..
        }
    ));
    assert!(matches!(
        parsed(&[
            "restore-preview",
            "a.bin",
            "--version",
            "capture:2",
            "--expected-generation",
            "0",
            "--output",
            "new.json"
        ])
        .unwrap(),
        OfflineCommand::RestorePreview {
            version: VersionSelector::Capture(2),
            expected_generation: 0,
            ..
        }
    ));
    assert!(
        matches!(parsed(&["restore-apply", "reviewed.json", "--expected-digest", &digest]).unwrap(),
        OfflineCommand::RestoreApply { expected_digest, .. } if expected_digest.as_str() == digest)
    );
    for args in [
        vec![
            "restore-preview",
            "a.bin",
            "--expected-generation",
            "1",
            "--expected-generation",
            "1",
            "--output",
            "new.json",
        ],
        vec![
            "restore-preview",
            "a.bin",
            "--expected-generation",
            "1",
            "--output",
            "new.json",
            "--output",
            "other.json",
        ],
        vec![
            "restore-preview",
            "a.bin",
            "--version",
            "current",
            "--version",
            "capture:1",
            "--expected-generation",
            "1",
            "--output",
            "new.json",
        ],
        vec!["restore-preview", "a.bin", "--output", "new.json"],
        vec!["restore-preview", "a.bin", "--expected-generation", "1"],
        vec![
            "restore-preview",
            "--expected-generation",
            "1",
            "--output",
            "new.json",
        ],
        vec![
            "restore-preview",
            "a.bin",
            "a.bin",
            "--expected-generation",
            "1",
            "--output",
            "new.json",
        ],
        vec![
            "restore-preview",
            "../foreign",
            "--expected-generation",
            "1",
            "--output",
            "new.json",
        ],
        vec![
            "restore-preview",
            "a.bin",
            "--expected-generation",
            "-1",
            "--output",
            "new.json",
        ],
        vec![
            "restore-preview",
            "a.bin",
            "--expected-generation",
            "1",
            "--output",
            "-",
        ],
        vec![
            "restore-preview",
            "a.bin",
            "--expected-generation",
            "1",
            "--output",
            "new.json",
            "--apply",
        ],
        vec!["restore-apply", "reviewed.json"],
        vec![
            "restore-apply",
            "reviewed.json",
            "--expected-digest",
            "latest",
        ],
        vec![
            "restore-apply",
            "reviewed.json",
            "--expected-digest",
            &digest,
            "--expected-digest",
            &digest,
        ],
        vec![
            "restore-apply",
            "reviewed.json",
            "--expected-digest",
            &digest,
            "--force",
        ],
        vec![
            "restore-apply",
            "reviewed.json",
            "--expected-digest",
            &digest,
            "--endpoint",
            "https://example.test",
        ],
        vec!["restore-apply", "-", "--expected-digest", &digest],
    ] {
        assert!(parsed(&args).is_err(), "accepted {args:?}");
    }
}
