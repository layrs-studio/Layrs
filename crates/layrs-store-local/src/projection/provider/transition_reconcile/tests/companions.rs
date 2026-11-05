//! Pure DATA recipes using the actual reconcile owner and managed-plan kernel.
//! Execution evidence is retained by ROOT per compiled candidate.
use super::super::companions::OwnedCompanionData;
use super::*;
use crate::projection::provider::records::companion_v2::{
    CompanionBaseV2, CompanionHeaderRecord, CompanionHeaderV2, CompanionIntentV2,
};

fn derive(
    data: &Data,
    observed: &TransitionObservationData,
    files: &[ImageFileData<'_>],
    deleted: &[RepoPath],
    manifests: &[FileManifest],
) -> Result<OwnedCompanionData> {
    let source =
        source::from_test_data(data.input(), &data.captures, &data.mappings, &[], observed)?;
    let image = image::from_test_inventory(&source, files, deleted)?;
    OwnedCompanionData::derive(&source, &image, observed, manifests, 0, None, None)
}

#[test]
fn observed_alternate_representation_requires_durable_reopen_provenance_before_planning() {
    let data = Data::new(true);
    let alternate = manifest(b"captured creation A", true);
    assert_eq!(alternate.file_id(), data.a.file_id());
    assert_ne!(alternate.id(), data.a.id());
    let mut fresh = data.snapshot.clone();
    fresh.observed_authorization_epoch += 1;
    let authority = AuthorityObservationData::ReobservedExpandedScope {
        policy_set_id: fresh.observed_policy_set_id.clone(),
        authorization_epoch: fresh.observed_authorization_epoch,
    };
    let observed = TransitionObservationData::normalize(
        data.input(),
        data.prefix.destination_identity.clone(),
        8,
        fresh,
        data.selected.clone(),
        vec![
            ProviderSourceDescriptor {
                path: p("README.md"),
                manifest: data.readme.clone(),
            },
            ProviderSourceDescriptor {
                path: p("new.txt"),
                manifest: alternate.clone(),
            },
        ],
        ObservationCoverageData::default(),
        authority,
        None,
    )
    .unwrap();
    let readme = p("README.md");
    let path = p("new.txt");
    let files = [
        ImageFileData {
            path: &readme,
            manifest: &data.readme,
            executable: Some(false),
        },
        ImageFileData {
            path: &path,
            manifest: &data.b,
            executable: Some(false),
        },
    ];
    let baseline = derive(
        &data,
        &data.observation(false),
        &files,
        &[],
        &[data.readme.clone(), data.a.clone(), data.b.clone()],
    )
    .unwrap();
    assert!(baseline.header().is_none());
    let error = derive(
        &data,
        &observed,
        &files,
        &[],
        &[
            data.readme.clone(),
            data.a.clone(),
            alternate.clone(),
            data.b.clone(),
        ],
    )
    .err()
    .expect("observation-only alternate must refuse before planning");
    assert!(
        error
            .to_string()
            .contains("union cannot be reconstructed by autonomous Ready")
    );

    // The alternate is supported when it survives in the actual checkout;
    // this must not become a blanket ban on multiple representations.
    let files = [
        ImageFileData {
            path: &readme,
            manifest: &data.readme,
            executable: Some(false),
        },
        ImageFileData {
            path: &path,
            manifest: &data.a,
            executable: Some(false),
        },
    ];
    let supported = derive(
        &data,
        &observed,
        &files,
        &[],
        &[data.readme.clone(), data.a.clone(), alternate.clone()],
    )
    .unwrap();
    assert!(
        supported
            .authorized_files()
            .iter()
            .any(|f| f.manifest_id == *alternate.id())
    );
}

#[test]
fn fresh_remote_change_or_creation_at_known_path_is_refused_by_normalizer() {
    for absent in [false, true] {
        let data = Data::new(!absent);
        let mut fresh = data.snapshot.clone();
        fresh.observed_authorization_epoch += 1;
        let entry = fresh
            .entries
            .iter_mut()
            .find(|e| e.path == p("new.txt"))
            .unwrap();
        entry.target = Some(TreeEntryTarget::file(data.b.file_id().clone()));
        entry.state = EntryState::Unavailable;
        let authority = AuthorityObservationData::ReobservedExpandedScope {
            policy_set_id: fresh.observed_policy_set_id.clone(),
            authorization_epoch: fresh.observed_authorization_epoch,
        };
        let error = TransitionObservationData::normalize(
            data.input(),
            data.prefix.destination_identity.clone(),
            8,
            fresh,
            data.selected.clone(),
            vec![
                ProviderSourceDescriptor {
                    path: p("README.md"),
                    manifest: data.readme.clone(),
                },
                ProviderSourceDescriptor {
                    path: p("new.txt"),
                    manifest: data.b.clone(),
                },
            ],
            ObservationCoverageData::default(),
            authority,
            None,
        )
        .err()
        .expect("immutable known target cannot change");
        assert!(
            error
                .to_string()
                .contains("changed an immutable source target")
        );
    }
}

#[test]
fn newly_readable_original_reserved_file_without_source_image_refuses_before_planning() {
    let mut data = Data::new(true);
    data.captures.clear();
    data.prefix.closure.captures.count = 0;
    let entry = data
        .snapshot
        .entries
        .iter_mut()
        .find(|e| e.path == p("new.txt"))
        .unwrap();
    entry.state = EntryState::Reserved;
    entry.writable = false;
    let ImportSource::Ordinary { version, .. } = &mut data.prefix.source else {
        unreachable!()
    };
    version.capture_count = 0;
    version.capture_head = None;
    version.snapshot_digest =
        database::digest(7, Some(PARENT), &data.snapshot.canonical_bytes().unwrap());
    version.selection_digest = database::digest(
        7,
        Some(&version.snapshot_digest),
        &selection::encode(&data.selected).unwrap(),
    );
    let mut fresh = data.snapshot.clone();
    fresh.observed_authorization_epoch += 1;
    let entry = fresh
        .entries
        .iter_mut()
        .find(|e| e.path == p("new.txt"))
        .unwrap();
    entry.state = EntryState::Unavailable;
    entry.writable = true;
    let authority = AuthorityObservationData::ReobservedExpandedScope {
        policy_set_id: fresh.observed_policy_set_id.clone(),
        authorization_epoch: fresh.observed_authorization_epoch,
    };
    let observed = TransitionObservationData::normalize(
        data.input(),
        data.prefix.destination_identity.clone(),
        8,
        fresh,
        data.selected.clone(),
        vec![
            ProviderSourceDescriptor {
                path: p("README.md"),
                manifest: data.readme.clone(),
            },
            ProviderSourceDescriptor {
                path: p("new.txt"),
                manifest: data.a.clone(),
            },
        ],
        ObservationCoverageData::default(),
        authority,
        None,
    )
    .unwrap();
    let readme = p("README.md");
    let files = [ImageFileData {
        path: &readme,
        manifest: &data.readme,
        executable: Some(false),
    }];
    let error = derive(
        &data,
        &observed,
        &files,
        &[],
        &[data.readme.clone(), data.a.clone()],
    )
    .err()
    .expect("remote-only selected path has no reconstructible B image evidence");
    assert!(
        error
            .to_string()
            .contains("checkout invents an original image file")
    );
}

#[test]
fn c13_unchanged_file_revoked_directly_or_by_tree_is_preserved_in_companion() {
    for tree in [false, true] {
        let mut data = Data::new(true);
        data.prefix.kind = ProviderImportKind::ManagedOrdinary;
        data.captures.clear();
        data.prefix.closure.captures.count = 0;
        let hidden = p(if tree { "dir/new.txt" } else { "new.txt" });
        data.snapshot
            .entries
            .iter_mut()
            .find(|e| e.path == p("new.txt"))
            .unwrap()
            .path = hidden.clone();
        data.snapshot.entries.sort_by(|a, b| a.path.cmp(&b.path));
        let ImportSource::Ordinary { version, .. } = &mut data.prefix.source else {
            unreachable!()
        };
        version.capture_count = 0;
        version.capture_head = None;
        version.snapshot_digest =
            database::digest(7, Some(PARENT), &data.snapshot.canonical_bytes().unwrap());
        version.selection_digest = database::digest(
            7,
            Some(&version.snapshot_digest),
            &selection::encode(&data.selected).unwrap(),
        );
        let mut fresh = data.snapshot.clone();
        fresh.observed_authorization_epoch += 1;
        fresh
            .entries
            .iter_mut()
            .find(|e| e.path == p("README.md"))
            .unwrap()
            .writable = false;
        if tree {
            fresh.entries.retain(|e| e.path != hidden);
            fresh.entries.push(ProjectionEntry {
                path: p("dir"),
                target: Some(TreeEntryTarget::Tree {
                    tree_id: fresh.base_root_tree_id.clone(),
                }),
                executable: false,
                writable: false,
                state: EntryState::Reserved,
            });
            fresh.entries.sort_by(|a, b| a.path.cmp(&b.path));
        } else {
            let entry = fresh.entries.iter_mut().find(|e| e.path == hidden).unwrap();
            entry.state = EntryState::Reserved;
            entry.writable = false;
        }
        let authority = AuthorityObservationData::ReobservedExpandedScope {
            policy_set_id: fresh.observed_policy_set_id.clone(),
            authorization_epoch: fresh.observed_authorization_epoch,
        };
        let observed = TransitionObservationData::normalize(
            data.input(),
            data.prefix.destination_identity.clone(),
            8,
            fresh,
            data.selected.clone(),
            vec![ProviderSourceDescriptor {
                path: p("README.md"),
                manifest: data.readme.clone(),
            }],
            ObservationCoverageData::default(),
            authority,
            None,
        )
        .unwrap();
        let readme = p("README.md");
        let source =
            source::from_test_data(data.input(), &data.captures, &data.mappings, &[], &observed)
                .unwrap();
        let files = [
            ImageFileData {
                path: &readme,
                manifest: &data.b,
                executable: Some(false),
            },
            ImageFileData {
                path: &hidden,
                manifest: &data.a,
                executable: Some(false),
            },
        ];
        let image = image::from_test_inventory(&source, &files, &[]).unwrap();
        let reconciled = reconcile(&source, &image, &observed, None).unwrap();
        assert!(reconciled.overlay.is_empty());
        assert_eq!(reconciled.recovery_files.len(), 2);
        let preserved = reconciled
            .recovery_files
            .iter()
            .find(|f| f.row.path == hidden)
            .unwrap();
        assert_eq!(preserved.row.file_id, *data.a.file_id());
        assert_eq!(preserved.row.base_file_id.as_ref(), Some(data.a.file_id()));
        let owned = derive(
            &data,
            &observed,
            &files,
            &[],
            &[data.readme.clone(), data.a.clone(), data.b.clone()],
        )
        .unwrap();
        assert_eq!(owned.header().unwrap().file_count, 2);
        assert_eq!(owned.authorized_files().len(), 1);
        assert_eq!(owned.authorized_files()[0].path, readme);
        let plan = crate::projection::provider::bootstrap::derive_reconciled_data(
            &data.prefix,
            &owned,
            &observed,
            data.prefix.destination_identity.clone(),
            "C:\\c13-parent",
            "fs:v1:1111111111111111:2222222222222222",
            "00000000000000000000000000000001",
            "00000000000000000000000000000002",
        )
        .unwrap();
        assert!(plan.capture.changes.is_empty());
        assert_eq!(plan.plan.companion_files.count, 2);
        assert_eq!(plan.plan.protected_manifests.count, 3);
        assert_eq!(plan.quota_bytes, owned.quota_bytes());
    }
}

#[test]
fn c13_reserved_present_file_is_retained_as_companion_without_checkout_or_capture() {
    let mut data = Data::new(true);
    data.prefix.kind = ProviderImportKind::ManagedOrdinary;
    data.captures.clear();
    data.prefix.closure.captures.count = 0;
    let ImportSource::Ordinary { version, .. } = &mut data.prefix.source else {
        unreachable!()
    };
    version.capture_count = 0;
    version.capture_head = None;
    let mut fresh = data.snapshot.clone();
    fresh.observed_authorization_epoch += 1;
    let reserved = fresh
        .entries
        .iter_mut()
        .find(|e| e.path == p("new.txt"))
        .unwrap();
    reserved.state = EntryState::Reserved;
    reserved.writable = false;
    let authority = AuthorityObservationData::ReobservedExpandedScope {
        policy_set_id: fresh.observed_policy_set_id.clone(),
        authorization_epoch: fresh.observed_authorization_epoch,
    };
    let observed = TransitionObservationData::normalize(
        data.input(),
        data.prefix.destination_identity.clone(),
        8,
        fresh,
        data.selected.clone(),
        vec![ProviderSourceDescriptor {
            path: p("README.md"),
            manifest: data.readme.clone(),
        }],
        ObservationCoverageData::default(),
        authority,
        None,
    )
    .unwrap();
    let readme = p("README.md");
    let recovery = p("new.txt");
    let owned = derive(
        &data,
        &observed,
        &[
            ImageFileData {
                path: &readme,
                manifest: &data.readme,
                executable: Some(false),
            },
            ImageFileData {
                path: &recovery,
                manifest: &data.b,
                executable: Some(false),
            },
        ],
        &[],
        &[data.readme.clone(), data.a.clone(), data.b.clone()],
    )
    .unwrap();
    assert_eq!(owned.header().unwrap().file_count, 1);
    assert_eq!(owned.authorized_files().len(), 1);
    assert_eq!(owned.authorized_files()[0].path, readme);
    let plan = crate::projection::provider::bootstrap::derive_reconciled_data(
        &data.prefix,
        &owned,
        &observed,
        data.prefix.destination_identity.clone(),
        "C:\\c13-parent",
        "fs:v1:1111111111111111:2222222222222222",
        "00000000000000000000000000000001",
        "00000000000000000000000000000002",
    )
    .unwrap();
    assert!(plan.capture.changes.is_empty());
    let kept = plan
        .placement
        .entries
        .iter()
        .find(|e| e.path == recovery)
        .unwrap();
    assert_eq!(kept.state, EntryState::Reserved);
    assert!(!kept.writable);
    assert_eq!(
        kept.target,
        Some(TreeEntryTarget::file(data.a.file_id().clone()))
    );
    assert_eq!(plan.plan.companion_files.count, 1);
    assert_eq!(plan.plan.protected_manifests.count, 3);
    assert_eq!(plan.quota_bytes, owned.quota_bytes());
}

#[test]
fn c13_v2_accepted_deletion_roundtrip_retains_payload_and_preimage() {
    let data = Data::new(true);
    let observed = data.observation(true);
    let readme = p("README.md");
    let files = [ImageFileData {
        path: &readme,
        manifest: &data.readme,
        executable: Some(false),
    }];
    let owned = derive(
        &data,
        &observed,
        &files,
        &[p("new.txt")],
        &[data.readme.clone(), data.a.clone()],
    )
    .unwrap();
    assert!(owned.overlay().is_empty());
    assert_eq!(owned.authorized_files().len(), 1);
    let header = owned.header().unwrap();
    assert_eq!(header.deletion_count, 1);
    let CompanionIntentV2::RecoveryAcceptedDeletion {
        accepted, reason, ..
    } = &header.intents[0]
    else {
        panic!("accepted deletion was flattened into an effective-file row");
    };
    assert_eq!(*reason, CompanionReason::RevokedRights);
    assert_eq!(accepted.change, data.captures[0].changes[0]);
    assert_eq!(accepted.capture_digest, data.captures[0].digest);
    assert_eq!(accepted.preimage_manifest_id, *data.a.id());
    let payload = header.encode().unwrap();
    let inherited = CompanionHeaderRecord::decode(&payload).unwrap();
    assert_eq!(inherited.as_ref().encode().unwrap(), payload);
    assert_eq!(
        inherited.as_ref().digest().unwrap(),
        header.digest().unwrap()
    );
    assert_eq!(inherited.as_ref().columns().deletion_count, 1);
    assert_eq!(CompanionHeaderV2::decode(&payload).unwrap(), *header);
    assert!(matches!(
        CompanionHeaderRecord::decode(&payload).unwrap(),
        CompanionHeaderRecord::V2(_)
    ));
    assert!(ProviderCompanionHeaderRecord::decode(&payload).is_err());
    assert!(owned.source_mappings().contains_key(data.a.file_id()));
}

#[test]
fn c13_v2_satisfied_captured_creation_is_durable_without_none_none_capture() {
    let data = Data::new(false);
    let observed = data.observation(false);
    let readme = p("README.md");
    let owned = derive(
        &data,
        &observed,
        &[ImageFileData {
            path: &readme,
            manifest: &data.readme,
            executable: Some(false),
        }],
        &[],
        &[data.readme.clone(), data.a.clone()],
    )
    .unwrap();
    assert!(owned.overlay().is_empty());
    let header = owned.header().unwrap();
    assert_eq!(header.deletion_count, 0);
    let CompanionIntentV2::SatisfiedDeletion {
        path,
        source_effective_base,
        accepted,
        ..
    } = &header.intents[0]
    else {
        panic!("satisfied intent lost");
    };
    assert_eq!(*path, p("new.txt"));
    assert_eq!(
        source_effective_base.as_ref().unwrap().file_id,
        *data.a.file_id()
    );
    assert!(accepted.is_none());
    let encoded = header.encode().unwrap();
    assert_eq!(CompanionHeaderV2::decode(&encoded).unwrap(), *header);
    assert!(owned.metadata_bytes() >= 2 * encoded.len() as u64);
}

#[test]
fn c13_v2_accepted_deletion_refuses_changed_immutable_base_and_retains_authorized_diff() {
    let data = Data::new(true);
    let capture_before = data.captures[0].clone();
    let mut wrong = data.snapshot.clone();
    wrong.observed_authorization_epoch += 1;
    let entry = wrong
        .entries
        .iter_mut()
        .find(|e| e.path == p("new.txt"))
        .unwrap();
    entry.target = None;
    entry.state = EntryState::Absent;
    let policy_set_id = wrong.observed_policy_set_id.clone();
    let authorization_epoch = wrong.observed_authorization_epoch;
    // The transition retains the SAME immutable Step/root. It cannot claim
    // that a previously present base disappeared remotely. Captures are full
    // diffs against that base, not patches applied to the preceding Capture.
    let error = TransitionObservationData::normalize(
        data.input(),
        data.prefix.destination_identity.clone(),
        8,
        wrong,
        data.selected.clone(),
        vec![ProviderSourceDescriptor {
            path: p("README.md"),
            manifest: data.readme.clone(),
        }],
        ObservationCoverageData::default(),
        AuthorityObservationData::ReobservedExpandedScope {
            policy_set_id,
            authorization_epoch,
        },
        None,
    )
    .err()
    .expect("immutable target substitution must refuse");
    assert!(
        error
            .to_string()
            .contains("changed an immutable source target")
    );
    // A legitimate reobservation keeps the base. The accepted deletion is
    // transported as the authorized destination diff; its original Capture
    // and exact preimage remain in the imported source history and union.
    let observed = data.observation(false);
    let readme = p("README.md");
    let owned = derive(
        &data,
        &observed,
        &[ImageFileData {
            path: &readme,
            manifest: &data.readme,
            executable: Some(false),
        }],
        &[p("new.txt")],
        &[data.readme.clone(), data.a.clone()],
    )
    .unwrap();
    assert_eq!(owned.overlay(), data.captures[0].changes.as_slice());
    assert!(owned.header().is_none());
    assert_eq!(data.captures[0], capture_before);
    assert!(owned.manifests().iter().any(|m| m.id() == data.a.id()));
}

fn readonly_data() -> Data {
    let mut data = Data::new(false);
    data.snapshot
        .entries
        .iter_mut()
        .find(|e| e.path == p("README.md"))
        .unwrap()
        .writable = false;
    let digest = database::digest(7, Some(PARENT), &data.snapshot.canonical_bytes().unwrap());
    data.captures[0].projection_digest = digest.clone();
    data.captures[0].digest = data.captures[0].computed_digest().unwrap();
    let ImportSource::Ordinary { version, .. } = &mut data.prefix.source else {
        unreachable!()
    };
    version.snapshot_digest = digest.clone();
    version.selection_digest = database::digest(
        7,
        Some(&digest),
        &selection::encode(&data.selected).unwrap(),
    );
    version.capture_head = Some(data.captures[0].digest.clone());
    data
}

#[test]
fn c13_v2_mixed_authorized_unknown_readonly_are_separate_and_budgeted() {
    let data = readonly_data();
    let observed = data.observation(false);
    let readme = p("README.md");
    let created = p("new.txt");
    let unknown = p("unknown.txt");
    let files = [
        ImageFileData {
            path: &readme,
            manifest: &data.b,
            executable: Some(false),
        },
        ImageFileData {
            path: &created,
            manifest: &data.a,
            executable: Some(false),
        },
        ImageFileData {
            path: &unknown,
            manifest: &data.b,
            executable: Some(false),
        },
    ];
    let supplied = [data.readme.clone(), data.a.clone(), data.b.clone()];
    let owned = derive(&data, &observed, &files, &[], &supplied).unwrap();
    assert_eq!(owned.overlay().len(), 1);
    assert_eq!(owned.overlay()[0].path, created);
    assert_eq!(owned.header().unwrap().file_count, 2);
    let paths = owned
        .authorized_files()
        .iter()
        .map(|f| f.path.clone())
        .collect::<Vec<_>>();
    assert_eq!(paths, vec![readme.clone(), created.clone()]);
    assert_eq!(owned.authorized_files()[0].file_id, *data.readme.file_id());
    assert!(owned.source_mappings().contains_key(data.b.file_id()));
    assert_eq!(owned.manifests().len(), 3); // shared recovery FileId stored once
    let expected = crate::projection::provider::managed_budget::quota(
        owned.manifests(),
        owned.authorized_files().iter().map(|f| f.bytes),
        owned.metadata_bytes(),
    )
    .unwrap();
    assert_eq!(owned.quota_bytes(), expected);
    assert!(derive(&data, &observed, &files, &[], &supplied[..2]).is_err());
    let mut extra = supplied.to_vec();
    extra.push(manifest(b"unengaged bytes", true));
    assert!(derive(&data, &observed, &files, &[], &extra).is_err());
}

#[test]
fn c13_v2_two_manifests_same_fileid_are_both_retained() {
    let data = Data::new(false);
    let observed = data.observation(true);
    let alternate = manifest(b"captured creation A", true);
    assert_eq!(alternate.file_id(), data.a.file_id());
    assert_ne!(alternate.id(), data.a.id());
    let readme = p("README.md");
    let created = p("new.txt");
    let owned = derive(
        &data,
        &observed,
        &[
            ImageFileData {
                path: &readme,
                manifest: &data.readme,
                executable: Some(false),
            },
            ImageFileData {
                path: &created,
                manifest: &alternate,
                executable: Some(false),
            },
        ],
        &[],
        &[data.readme.clone(), data.a.clone(), alternate.clone()],
    )
    .unwrap();
    assert_eq!(
        owned
            .representations()
            .iter()
            .filter(|r| r.file_id == *data.a.file_id())
            .count(),
        2
    );
    assert!(owned.manifests().iter().any(|m| m.id() == alternate.id()));
    assert!(owned.manifests().iter().any(|m| m.id() == data.a.id()));
    assert_eq!(owned.header().unwrap().preserved_representations.len(), 2);
    assert!(
        owned
            .header()
            .unwrap()
            .preserved_representations
            .iter()
            .any(|r| r.manifest_id == *alternate.id())
    );
}

#[test]
fn c13_v2_inherited_payload_keeps_alternate_representation_after_image_disappears() {
    let data = Data::new(false);
    let observed = data.observation(true);
    let alternate = manifest(b"captured creation A", true);
    let readme = p("README.md");
    let created = p("new.txt");
    let previous = derive(
        &data,
        &observed,
        &[
            ImageFileData {
                path: &readme,
                manifest: &data.readme,
                executable: Some(false),
            },
            ImageFileData {
                path: &created,
                manifest: &alternate,
                executable: Some(false),
            },
        ],
        &[],
        &[data.readme.clone(), data.a.clone(), alternate.clone()],
    )
    .unwrap();
    let persisted =
        CompanionHeaderRecord::decode(&previous.header().unwrap().encode().unwrap()).unwrap();
    let rows = [ProviderCompanionFileRecord {
        schema_version: "layrs.projection-provider-companion-file.v1".into(),
        path: created,
        file_id: data.a.file_id().clone(),
        bytes: data.a.byte_length(),
        executable: false,
        manifest_id: data.a.id().clone(),
        base_file_id: Some(data.a.file_id().clone()),
        base_executable: Some(false),
        reason: CompanionReason::RevokedRights,
    }];
    persisted.as_ref().validate_rows(&rows, &[]).unwrap();
    let inherited = [InheritedCompanionData {
        header: persisted.as_ref(),
        files: &rows,
        deletions: &[],
    }];
    // DATA fixture only: this proves union preservation, not receipt/SQL
    // authentication. Production normalize() still requires the closed import.
    let current = data.observation(false);
    let source = source::from_test_data(
        data.input(),
        &data.captures,
        &data.mappings,
        &inherited,
        &current,
    )
    .unwrap();
    let image = image::from_test_inventory(
        &source,
        &[ImageFileData {
            path: &readme,
            manifest: &data.readme,
            executable: Some(false),
        }],
        &[],
    )
    .unwrap();
    let result = reconcile(&source, &image, &current, None).unwrap();
    assert!(
        result
            .representations
            .iter()
            .any(|r| r.file_id == *data.a.file_id() && r.manifest_id == *alternate.id())
    );
    assert_eq!(
        persisted.as_ref().encode().unwrap(),
        previous.header().unwrap().encode().unwrap()
    );
}

#[test]
fn c13_v2_unsupported_duplicate_version_and_tampered_intent_refuse() {
    let data = Data::new(true);
    let observed = data.observation(true);
    let readme = p("README.md");
    let owned = derive(
        &data,
        &observed,
        &[ImageFileData {
            path: &readme,
            manifest: &data.readme,
            executable: Some(false),
        }],
        &[p("new.txt")],
        &[data.readme.clone(), data.a.clone()],
    )
    .unwrap();
    let bytes = owned.header().unwrap().encode().unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(
        CompanionHeaderRecord::decode(text.replace("companion.v2", "companion.v3").as_bytes())
            .is_err()
    );
    let duplicated = text.replacen(
        '{',
        "{\"schemaVersion\":\"layrs.projection-provider-companion.v2\",",
        1,
    );
    assert!(CompanionHeaderRecord::decode(duplicated.as_bytes()).is_err());
    let mut header = CompanionHeaderV2::decode(&bytes).unwrap();
    let CompanionIntentV2::RecoveryAcceptedDeletion { accepted, .. } = &mut header.intents[0]
    else {
        unreachable!()
    };
    accepted.source_snapshot_digest = "f".repeat(64);
    assert!(header.encode().is_err());
    header.schema_version = "layrs.projection-provider-companion.v3".into();
    assert!(header.encode().is_err());
}

#[test]
fn c13_v2_reconciled_plan_binds_companions_overlay_union_and_original_budget() {
    let mut data = readonly_data();
    data.prefix.kind = ProviderImportKind::ManagedOrdinary;
    let observed = data.observation(false);
    let readme = p("README.md");
    let created = p("new.txt");
    let unknown = p("unknown.txt");
    let owned = derive(
        &data,
        &observed,
        &[
            ImageFileData {
                path: &readme,
                manifest: &data.b,
                executable: Some(false),
            },
            ImageFileData {
                path: &created,
                manifest: &data.a,
                executable: Some(false),
            },
            ImageFileData {
                path: &unknown,
                manifest: &data.b,
                executable: Some(false),
            },
        ],
        &[],
        &[data.readme.clone(), data.a.clone(), data.b.clone()],
    )
    .unwrap();
    let plan = crate::projection::provider::bootstrap::derive_reconciled_data(
        &data.prefix,
        &owned,
        &observed,
        data.prefix.destination_identity.clone(),
        "C:\\c13-parent",
        "fs:v1:1111111111111111:2222222222222222",
        "00000000000000000000000000000001",
        "00000000000000000000000000000002",
    )
    .unwrap();
    assert_eq!(
        plan.plan.companion_id.as_deref(),
        Some(owned.header().unwrap().digest().unwrap().as_str())
    );
    assert_eq!(plan.plan.companion_files.count, 2);
    assert_eq!(plan.plan.protected_manifests.count, 3);
    assert_eq!(plan.capture.changes, owned.overlay());
    assert_eq!(plan.quota_bytes, owned.quota_bytes());
    assert_eq!(
        plan.plan.observation.snapshot_digest,
        observed.binding().snapshot_digest
    );
    assert!(plan.placement.entries.iter().all(|e| e.path != unknown));
    let changed = data.observation(true);
    assert!(
        crate::projection::provider::bootstrap::derive_reconciled_data(
            &data.prefix,
            &owned,
            &changed,
            data.prefix.destination_identity.clone(),
            "C:\\c13-parent",
            "fs:v1:1111111111111111:2222222222222222",
            "00000000000000000000000000000001",
            "00000000000000000000000000000002"
        )
        .is_err()
    );
}

#[test]
fn c13_v2_header_exact_64k_boundary_refuses_next_byte_without_mutation() {
    let data = Data::new(true);
    let observed = data.observation(true);
    let readme = p("README.md");
    let owned = derive(
        &data,
        &observed,
        &[ImageFileData {
            path: &readme,
            manifest: &data.readme,
            executable: Some(false),
        }],
        &[p("new.txt")],
        &[data.readme.clone(), data.a.clone()],
    )
    .unwrap();
    let original = owned.header().unwrap().encode().unwrap();
    let source_before = data.prefix.encode().unwrap();
    let mut header = CompanionHeaderV2::decode(&original).unwrap();
    // A valid bounded header may describe more than one durable intent. Use
    // real fixed-size identities/digests and distinct paths; never padding in
    // an unknown JSON field that the strict decoder would already reject.
    for n in 0..300 {
        header.intents.push(CompanionIntentV2::SatisfiedDeletion {
            path: p(&format!("z-boundary-{n:04}.txt")),
            source_effective_base: Some(CompanionBaseV2 {
                file_id: data.a.file_id().clone(),
                executable: false,
            }),
            accepted: None,
            destination_generation: observed.binding().generation,
            destination_snapshot_digest: observed.binding().snapshot_digest.clone(),
        });
        let size = layrs_core::scm::canonical_json(&header).unwrap().len();
        if size > CompanionHeaderV2::MAX_BYTES - 4000 {
            break;
        }
    }
    let size = layrs_core::scm::canonical_json(&header).unwrap().len();
    assert!(size <= CompanionHeaderV2::MAX_BYTES);
    let last = header.intents.last_mut().unwrap();
    let CompanionIntentV2::SatisfiedDeletion { path, .. } = last else {
        unreachable!()
    };
    let prefix = path.as_str().to_owned();
    let mut padded = prefix;
    let mut remaining = CompanionHeaderV2::MAX_BYTES - size;
    while remaining > 200 {
        padded.push('/');
        padded.push_str(&"x".repeat(199));
        remaining -= 200;
    }
    if remaining > 1 {
        padded.push('/');
        padded.push_str(&"x".repeat(remaining - 1));
    } else if remaining == 1 {
        padded.push('x');
    }
    *path = p(&padded);
    let exact = header.encode().unwrap();
    assert_eq!(exact.len(), 64 * 1024);
    assert_eq!(
        CompanionHeaderRecord::decode(&exact)
            .unwrap()
            .as_ref()
            .encode()
            .unwrap(),
        exact
    );
    let CompanionIntentV2::SatisfiedDeletion { path, .. } = header.intents.last_mut().unwrap()
    else {
        unreachable!()
    };
    *path = p(&format!("{}x", path.as_str()));
    assert_eq!(
        layrs_core::scm::canonical_json(&header).unwrap().len(),
        64 * 1024 + 1
    );
    assert!(header.encode().is_err());
    assert!(
        CompanionHeaderRecord::decode(&layrs_core::scm::canonical_json(&header).unwrap()).is_err()
    );
    assert_eq!(owned.header().unwrap().encode().unwrap(), original);
    assert_eq!(data.prefix.encode().unwrap(), source_before);
}
