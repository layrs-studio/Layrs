use super::*;
use serde_json::{json, Value};

const RESUMES: [&str; 5] = [
    "resumeInstallation",
    "resumeNamespace",
    "resumeRefresh",
    "resumeRestore",
    "resumeRebase",
];
const OFFLINE: [&str; 3] = [
    "recoverDatabases",
    "resumeInitialization",
    "cancelSharedReservation",
];

fn requests(expected: &RecoveryBinding) -> Vec<Value> {
    RESUMES
        .into_iter()
        .map(|action| json!({"action":action,"expected":expected}))
        .chain(OFFLINE.into_iter().map(
            |action| json!({"action":"offline","request":{"action":action,"expected":expected}}),
        ))
        .collect()
}

pub(super) fn assert_all_recovery_refused(
    root: &Path,
    expected: &RecoveryBinding,
    fixture: &Path,
    reason: &str,
) {
    let before = recovery::fingerprints(fixture);
    for request in requests(expected) {
        let action: ProjectionAction = serde_json::from_value(request.clone()).unwrap();
        let error = execute_projection(root, action).unwrap_err();
        assert!(error.contains(reason), "{request}: {error}");
        assert_eq!(
            recovery::fingerprints(fixture),
            before,
            "mutation after refused {request}"
        );
    }
}

#[test]
fn recovery_wire_requires_the_complete_inspected_binding() {
    let (_f, store) = installed_shared(true);
    let reply = execute_projection_offline(
        store.checkout_root(),
        ProjectionOfflineAction::InspectRecovery {},
    )
    .unwrap();
    let expected = binding(&reply);
    for request in requests(&expected) {
        assert!(serde_json::from_value::<ProjectionAction>(request.clone()).is_ok());
        let mutate = |f: fn(&mut serde_json::Map<String, Value>)| {
            let mut changed = request.clone();
            let target = if changed["action"] == "offline" {
                &mut changed["request"]
            } else {
                &mut changed
            };
            f(target.as_object_mut().unwrap());
            assert!(
                serde_json::from_value::<ProjectionAction>(changed.clone()).is_err(),
                "accepted {changed}"
            );
        };
        mutate(|m| {
            m.remove("expected");
        });
        mutate(|m| {
            m.insert("expected".into(), Value::Null);
        });
        mutate(|m| {
            m.insert("hydrate".into(), Value::Bool(true));
        });
        mutate(|m| {
            m.get_mut("expected")
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("extra".into(), Value::Bool(true));
        });
    }
}

#[test]
fn recovery_dispatch_rejects_replaced_checkout_before_sql_or_application_effects() {
    for initialize in [false, true] {
        let f = Fixture::new();
        let family_a = recovery::family(&f);
        let root = f.directory("live");
        let a = family_a
            .create_worktree(
                &root,
                identity().worktree_id,
                snapshot(),
                ProjectionSelection::All,
            )
            .unwrap();
        fs::write(root.join("local-note"), b"retained A bytes").unwrap();
        let inspected_a =
            execute_projection_offline(&root, ProjectionOfflineAction::InspectRecovery {}).unwrap();
        let expected_a = binding(&inspected_a);
        drop(a);
        let retained_a = f.0.join("retained-a");
        fs::rename(&root, &retained_a).unwrap();
        fs::create_dir(&root).unwrap();
        let family_b = PartialStoreFamily::create(
            f.directory("family-b"),
            family_a.identity().clone(),
            SharedQuota {
                bytes: 32 * 1024 * 1024,
            },
        )
        .unwrap();
        if initialize {
            assert!(family_b
                .create_worktree_with_faults(
                    &root,
                    identity().worktree_id,
                    snapshot(),
                    ProjectionSelection::All,
                    &recovery::StopAtMarker
                )
                .is_err());
        } else {
            let b = family_b
                .create_worktree(
                    &root,
                    identity().worktree_id,
                    snapshot(),
                    ProjectionSelection::All,
                )
                .unwrap();
            let mut target = snapshot();
            target.entries[0].state = EntryState::Materialized;
            assert!(b
                .install_projection_with_faults(
                    0,
                    &target,
                    &mut |_, out| out.write_all(b"alpha"),
                    &StopAfterJournal
                )
                .is_err());
        }
        let inspected_b =
            execute_projection_offline(&root, ProjectionOfflineAction::InspectRecovery {}).unwrap();
        let expected_b = binding(&inspected_b);
        assert_eq!(
            expected_a.identity, expected_b.identity,
            "logical identity cannot distinguish these independently valid checkouts"
        );
        assert_ne!(expected_a.checkout_identity, expected_b.checkout_identity);
        let marker: Value =
            serde_json::from_slice(&fs::read(root.join(".layrs/FORMAT")).unwrap()).unwrap();
        let metadata = PathBuf::from(marker["shared"]["metadataPath"].as_str().unwrap());
        assert!(metadata.is_absolute() && metadata.starts_with(family_b.root()));
        recovery::kill_spilled_writer(
            &metadata.join("projection.sqlite3"),
            "owner",
            &f.0.join("spill-ready"),
        );
        assert_all_recovery_refused(&root, &expected_a, &f.0, "binding");
        let recovered = execute_projection_offline(
            &root,
            ProjectionOfflineAction::RecoverDatabases {
                expected: expected_b,
            },
        )
        .unwrap();
        let expected_b = binding(&recovered);
        assert_eq!(
            recovered["recovery"],
            json!([if initialize {
                "initializing"
            } else {
                "filesystemOperation"
            }])
        );
        assert!(!root.join("a.txt").exists());
        assert_all_recovery_refused(&root, &expected_a, &f.0, "binding");
        if initialize {
            let ready = execute_projection_offline(
                &root,
                ProjectionOfflineAction::ResumeInitialization {
                    expected: expected_b,
                },
            )
            .unwrap();
            assert_eq!(ready["recovery"], json!([]));
        } else {
            execute_projection(
                &root,
                ProjectionAction::ResumeInstallation {
                    expected: expected_b,
                },
            )
            .unwrap();
            assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"alpha");
        }
        assert_eq!(
            fs::read(retained_a.join("local-note")).unwrap(),
            b"retained A bytes"
        );
    }
}
