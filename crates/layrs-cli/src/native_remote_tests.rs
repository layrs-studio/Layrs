use super::*;
use crate::args::{CliCommand, GlobalFlags};
use layrs_core::projection::ProjectionSelection;
use serde_json::json;

fn request(args: &[&str]) -> Result<PreparedRequest, CliError> {
    let cli = Cli {
        globals: GlobalFlags::default(),
        command: CliCommand::Help,
    };
    let args = args
        .iter()
        .map(|arg| (*arg).to_owned())
        .chain(["--endpoint".into(), "https://example.invalid".into()])
        .collect::<Vec<_>>();
    prepare(&cli, &args)
}

#[test]
fn selected_clone_keeps_exact_paths_separate_from_history_filter() {
    let step = format!("blake3:{}", "1".repeat(64));
    let prepared = request(&[
        "clone-partial",
        "space:test",
        &step,
        "checkout",
        "--include",
        "a.txt",
        "--include-tree=src",
        "--include",
        "b.txt",
    ])
    .unwrap();
    assert_eq!(prepared.root, PathBuf::from("checkout"));
    assert_eq!(
        serde_json::to_value(prepared.action).unwrap(),
        json!({
            "action":"cloneProjectionSelected", "space":"space:test", "step":step,
            "selection":{"kind":"selected","paths":["a.txt","b.txt"],"subtrees":["src"]}
        })
    );
    assert!(matches!(
        request(&["clone-partial", "space:test", &step, "checkout"])
            .unwrap()
            .action,
        Action::CloneProjection { .. }
    ));
    assert!(
        request(&[
            "clone-partial",
            "space:test",
            &step,
            "checkout",
            "--path",
            "a.txt"
        ])
        .is_err()
    );
    assert!(request(&["history", &step, "--include", "a.txt"]).is_err());
    let history = request(&["history", &step, "--path", "a.txt"]).unwrap();
    assert!(matches!(
        history.action,
        Action::Collaboration {
            command: layrs_client_core::native_scm::CollaborationCommand::StepHistory {
                anchor,
                path: Some(path),
                ..
            }
        } if anchor.as_str() == step && path.as_str() == "a.txt"
    ));
    // Remote history and partial clones take immutable Step IDs. Local moving
    // selectors are resolved only by commands that explicitly support them.
    assert!(request(&["history", "@layer:main", "--path", "a.txt"]).is_err());
    assert!(request(&["clone-partial", "space:test", "@layer:main", "checkout"]).is_err());
    assert!(
        request(&[
            "clone-partial",
            "space:test",
            "@layer:main",
            "checkout",
            "--include",
            "a.txt",
        ])
        .is_err()
    );
}

#[test]
fn expansion_requires_generation_and_selection_with_alias_parity() {
    for action in ["partial-select", "partial-hydrate"] {
        let prepared = request(&[
            action,
            "--expected-generation",
            "4",
            "--include-tree",
            "docs",
        ])
        .unwrap();
        assert!(matches!(
            prepared.action,
            Action::ExpandProjectionSelection {
                expected_generation: 4,
                selection: ProjectionSelection::Selected { .. }
            }
        ));
        assert!(request(&[action, "--include", "a.txt"]).is_err());
        assert!(request(&[action, "--expected-generation", "4"]).is_err());
    }
    assert!(request(&["receive", "--expected-generation", "4"]).is_err());
}

#[test]
fn selection_flags_reject_unknown_duplicate_missing_and_unsafe_values() {
    for suffix in [
        vec!["--include"],
        vec!["--include="],
        vec!["--include", "../secret"],
        vec!["--include", "/absolute"],
        vec!["--include", "a", "--include", "a"],
        vec!["--include-tree", "src", "--include-tree=src"],
        vec!["--include", "a", "--unknown"],
        vec!["--include", "a", "--expected-generation", "4"],
    ] {
        let mut args = vec!["partial-select", "--expected-generation", "4"];
        args.extend(suffix);
        assert!(request(&args).is_err(), "accepted {args:?}");
    }
    for generation in ["-1", "x", "18446744073709551616"] {
        assert!(
            request(&[
                "partial-select",
                "--expected-generation",
                generation,
                "--include",
                "a"
            ])
            .is_err()
        );
    }
}

#[test]
fn historical_applied_and_rejected_receipts_preserve_original_identity_in_cli_json() {
    for status in ["applied", "rejected"] {
        let result = json!({
            "historical":true,"pendingLocalAcceptance":false,
            "projection":{"generation":3,"digest":"retained-projection"},
            "receipt":{
                "requestDigest":"original-request","captureDigest":"original-capture",
                "clientId":"client:source","idempotencyKey":"original-key","intent":"candidate",
                "receipt":{
                    "status":status,"clientId":"client:source","principalId":"principal:source",
                    "spaceId":"space:original","transactionId":"transaction:original",
                    "rejections":if status == "rejected" { json!([{"code":"permission_denied","message":"original refusal"}]) } else { json!([]) },
                }
            }
        });
        let rendered = present_partial_publication(result.clone()).unwrap();
        assert!(rendered.human.contains(&format!(
            "Completed historical publication receipt: {status}"
        )));
        for original in ["client:source", "principal:source", "space:original"] {
            assert!(rendered.human.contains(original));
        }
        assert!(!rendered.human.contains("Receive"));
        assert!(!rendered.human.contains("Retry"));
        assert_eq!(rendered.data, result);
        let envelope: serde_json::Value =
            serde_json::from_str(&crate::render::ok_json(rendered.data)).unwrap();
        assert_eq!(envelope["ok"], true);
        assert_eq!(envelope["data"], result);
    }
}

#[test]
fn only_completed_historical_receipts_bypass_ordinary_publication_refusals() {
    let rejected = json!({"historical":false,"pendingLocalAcceptance":false,"receipt":{"receipt":{"status":"rejected"}}});
    assert!(
        present_partial_publication(rejected)
            .unwrap_err()
            .message
            .contains("Partial publication refused")
    );
    let pending = json!({"historical":false,"pendingLocalAcceptance":true,"receipt":{"receipt":{"status":"applied"}}});
    assert!(
        present_partial_publication(pending)
            .unwrap_err()
            .message
            .contains("local acceptance is pending")
    );
    let applied = json!({"historical":false,"pendingLocalAcceptance":false,"receipt":{"receipt":{"status":"applied"}}});
    assert_eq!(
        present_partial_publication(applied.clone()).unwrap().data,
        applied
    );
    for invalid in [
        json!({"historical":true,"pendingLocalAcceptance":true,"receipt":{"receipt":{"status":"applied"}}}),
        json!({"historical":true,"pendingLocalAcceptance":false,"receipt":{"receipt":{"status":"pending"}}}),
        json!({"historical":true,"receipt":{"receipt":{"status":"rejected"}}}),
    ] {
        let message = present_partial_publication(invalid).unwrap_err().message;
        assert!(message.contains("not a completed terminal receipt"));
        assert!(!message.contains("Receive"));
    }
}
