//! Pure codec/normalizer evidence only; no SQL, network or native fixture.
use super::*;
use layrs_core::{
    projection::{EntryState, ProjectionEntry},
    scm::{SpaceId, StoreId, WorktreeId},
};

const TOKEN: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
const TX: &str = "00000000000000000000000000000001";

fn path(value: &str) -> RepoPath {
    RepoPath::parse(value).unwrap()
}
fn record(paths: Vec<RepoPath>) -> TransitionDeniedCoverageRecord {
    let identity = ProjectionIdentity {
        store_id: StoreId::parse("store:coverage").unwrap(),
        worktree_id: WorktreeId::parse("worktree:coverage-source").unwrap(),
        space_id: SpaceId::parse("space:coverage").unwrap(),
        origin: "https://example.invalid".into(),
        principal_id: "principal:coverage".into(),
    };
    let mut destination = identity.clone();
    destination.worktree_id = WorktreeId::parse("worktree:coverage-destination").unwrap();
    let authority = DeniedAuthorityData::ReobservedExpandedScope {
        policy_set_id: ObjectId::parse(TOKEN).unwrap(),
        authorization_epoch: 2,
    };
    let (path_bytes, rows_digest) =
        validate_paths(&paths, PathSemantics::PortableWindows, None).unwrap();
    TransitionDeniedCoverageRecord {
        schema_version: SCHEMA.into(),
        transaction_id: TX.into(),
        source_prefix_id: TOKEN.into(),
        source_identity: identity,
        destination_identity: destination,
        source_version: ImportVersion {
            generation: 7,
            snapshot_digest: "a".repeat(64),
            selection_digest: "b".repeat(64),
            capture_count: 0,
            capture_head: None,
            effective_image_digest: "c".repeat(64),
        },
        step_id: StepId::parse(TOKEN).unwrap(),
        root_tree_id: TreeId::parse(TOKEN).unwrap(),
        path_semantics: PathSemantics::PortableWindows,
        authority_data_digest: authority.digest().unwrap(),
        authority,
        observation_generation: 8,
        observation_snapshot_digest: "d".repeat(64),
        observation_selection_digest: "e".repeat(64),
        denied_path_count: paths.len() as u64,
        path_bytes,
        rows_digest,
        denied_paths: paths,
    }
}
fn snapshot(record: &TransitionDeniedCoverageRecord) -> ProjectionSnapshot {
    ProjectionSnapshot {
        schema_version: "layrs.projection-snapshot.v1".into(),
        base_step_id: record.step_id.clone(),
        base_root_tree_id: record.root_tree_id.clone(),
        observed_policy_set_id: ObjectId::parse(TOKEN).unwrap(),
        observed_authorization_epoch: 2,
        path_semantics: record.path_semantics,
        entries: vec![],
    }
}

#[test]
fn coverage_body_roundtrip_binds_exact_rows_and_every_reference_field() {
    let value = record(vec![path("secret.txt")]);
    let payload = value.encode().unwrap();
    let reference = value.reference().unwrap();
    assert!(codec::encode(&reference, 1024).unwrap().len() < 512);
    assert_eq!(
        TransitionDeniedCoverageRecord::read_body(&reference.id, &payload, &reference).unwrap(),
        value
    );
    // Independent literal framing: no production path-digest helper.
    let row = b"\"secret.txt\"";
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-provider-transition-denied-paths-data-v1\0");
    hash.update(&1u64.to_be_bytes());
    hash.update(&(row.len() as u64).to_be_bytes());
    hash.update(row);
    assert_eq!(value.rows_digest, format!("blake3:{}", hash.finalize()));
    assert_eq!(value.path_bytes, 10);
    assert!(TransitionDeniedCoverageRecord::read_body(TOKEN, &payload, &reference).is_err());
    for field in 0..4 {
        let mut wrong = reference.clone();
        match field {
            0 => wrong.id = TOKEN.into(),
            1 => wrong.count += 1,
            2 => wrong.path_bytes += 1,
            _ => wrong.rows_digest = TOKEN.into(),
        }
        assert!(TransitionDeniedCoverageRecord::read_body(&wrong.id, &payload, &wrong).is_err());
    }
    let mut changed = TransitionDeniedCoverageRecord::decode(&payload).unwrap();
    changed.transaction_id = "00000000000000000000000000000002".into();
    assert!(
        TransitionDeniedCoverageRecord::read_body(
            &reference.id,
            &changed.encode().unwrap(),
            &reference
        )
        .is_err()
    );
    changed = TransitionDeniedCoverageRecord::decode(&payload).unwrap();
    changed.source_version.effective_image_digest = "f".repeat(64);
    assert_ne!(changed.digest().unwrap(), reference.id);
    changed.denied_paths[0] = path("public.txt");
    assert!(changed.encode().is_err());
}

#[test]
fn coverage_codec_rejects_unknown_duplicate_missing_and_noncanonical_fields() {
    let payload = record(vec![]).encode().unwrap();
    let text = std::str::from_utf8(&payload).unwrap();
    let unknown = format!("{{\"unexpected\":0,{}", &text[1..]);
    assert!(TransitionDeniedCoverageRecord::decode(unknown.as_bytes()).is_err());
    let duplicate = format!("{{\"schemaVersion\":\"{SCHEMA}\",{}", &text[1..]);
    assert!(TransitionDeniedCoverageRecord::decode(duplicate.as_bytes()).is_err());
    let mut missing: serde_json::Value = serde_json::from_slice(&payload).unwrap();
    missing.as_object_mut().unwrap().remove("authority");
    assert!(
        TransitionDeniedCoverageRecord::decode(&layrs_core::scm::canonical_json(&missing).unwrap())
            .is_err()
    );
    let pretty =
        serde_json::to_vec_pretty(&serde_json::from_slice::<serde_json::Value>(&payload).unwrap())
            .unwrap();
    assert!(TransitionDeniedCoverageRecord::decode(&pretty).is_err());
    assert!(serde_json::from_slice::<ProjectionSnapshot>(&payload).is_err());
}

#[test]
fn coverage_decoder_stops_oversized_path_array_before_commitment_validation() {
    let empty = String::from_utf8(record(vec![]).encode().unwrap()).unwrap();
    let array = std::iter::repeat_n("\"a\"", MAX_PATHS + 1)
        .collect::<Vec<_>>()
        .join(",");
    let oversized = empty.replace("\"deniedPaths\":[]", &format!("\"deniedPaths\":[{array}]"));
    assert_ne!(oversized, empty);
    let error = TransitionDeniedCoverageRecord::decode(oversized.as_bytes()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("denied path/count exceeds bound"),
        "{error}"
    );
}

#[test]
fn coverage_negative_paths_reject_aliases_ancestors_and_metadata_names() {
    for names in [
        vec!["a", "a"],
        vec!["b", "a"],
        vec!["A", "a"],
        vec!["Dir/a", "dir/b"],
        vec!["closed", "closed/child"],
        vec![".git/private"],
        vec!["a/.layrs/data"],
        vec!["CON.txt"],
    ] {
        let paths: Vec<_> = names.into_iter().map(path).collect();
        assert!(validate_paths(&paths, PathSemantics::PortableWindows, None).is_err());
    }
    let siblings = vec![path("Dir/a"), path("Dir/b")];
    validate_paths(&siblings, PathSemantics::PortableWindows, None).unwrap();
    // The record is negative DATA; its existing namespace semantics can fence
    // aliases/parents without pretending to grant remote subtree access.
    let fences = ReservedPaths::new(["Dir/a"], PathSemantics::PortableWindows).unwrap();
    for denied in ["dir", "DIR/A", "Dir/a/child"] {
        assert!(fences.check_path(denied).is_err());
    }
    fences.check_path("Dir/b").unwrap();
}

#[test]
fn coverage_supports_all_100000_paths_at_exact_8mib_and_refuses_one_more_byte() {
    let width = MAX_PATH_BYTES as usize / MAX_PATHS;
    let remainder = MAX_PATH_BYTES as usize % MAX_PATHS;
    let paths: Vec<_> = (0..MAX_PATHS)
        .map(|index| {
            let length = width + usize::from(index < remainder);
            path(&format!("{index:06}-{}", "x".repeat(length - 7)))
        })
        .collect();
    let mut value = record(paths);
    assert_eq!(value.path_bytes, MAX_PATH_BYTES);
    assert_eq!(value.denied_path_count, MAX_PATHS as u64);
    let payload = value.encode().unwrap();
    assert!(payload.len() > 64 * 1024 && payload.len() < TransitionDeniedCoverageRecord::MAX_BYTES);
    let reference = value.reference().unwrap();
    assert_eq!(
        TransitionDeniedCoverageRecord::read_body(&reference.id, &payload, &reference).unwrap(),
        value
    );
    let last = value.denied_paths.last_mut().unwrap();
    *last = path(&format!("{}x", last.as_str()));
    assert!(value.encode().is_err());
    value.denied_paths.push(path("zzzzzz-extra"));
    assert!(value.encode().is_err());
}

#[test]
fn coverage_shares_count_and_bytes_with_positive_observation_before_clone() {
    let value = record(vec![]);
    let mut visible = snapshot(&value);
    visible.entries.push(ProjectionEntry {
        path: path("visible"),
        target: None,
        executable: false,
        writable: true,
        state: EntryState::Absent,
    });
    // Deliberately empty iterator: the count fence runs before path iteration
    // or cloning. No forged body is accepted by the record constructor.
    assert!(require_shared_namespace(&visible, MAX_PATHS, std::iter::empty(), None).is_err());
    let one = [path("denied")];
    require_shared_namespace(&visible, one.len(), one.iter(), None).unwrap();
    let batch: Vec<_> = (0..MAX_PATHS - 1)
        .map(|index| path(&format!("{index:06}-{}", "x".repeat(77))))
        .collect();
    assert!(require_shared_namespace(&visible, batch.len(), batch.iter(), None).is_err());
}

#[test]
fn coverage_derives_complete_denial_from_real_normalizer_and_refuses_other_prefix() {
    let (mut prefix, normalized) = test_data::observation();
    let value =
        TransitionDeniedCoverageRecord::from_observation(&prefix, TX, &normalized, None).unwrap();
    let derived = DerivedDeniedCoverage::from_observation(&prefix, TX, &normalized, None).unwrap();
    assert_eq!(derived.record(), &value);
    // Raw transport remains editable/decodable, but cannot change the sealed
    // writer input or be converted back into one without the observation.
    let mut transport = TransitionDeniedCoverageRecord::decode(&value.encode().unwrap()).unwrap();
    transport.source_prefix_id = TOKEN.into();
    assert!(transport.encode().is_ok());
    assert_ne!(transport, *derived.record());
    assert_eq!(value.denied_paths, vec![path("created.txt")]);
    assert_eq!(value.rows_digest, normalized.binding().denied_paths_digest);
    assert_eq!(
        value.authority_data_digest,
        normalized.binding().authority_data_digest
    );
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(
        DerivedDeniedCoverage::from_observation(&prefix, TX, &normalized, Some(&cancel)).is_err()
    );
    assert!(
        DerivedDeniedCoverage::from_observation(&prefix, "invalid", &normalized, None).is_err()
    );
    let cancelled =
        TransitionDeniedCoverageRecord::from_observation(&prefix, TX, &normalized, Some(&cancel))
            .unwrap_err();
    let crate::projection::ProjectionStoreError::ProviderContent(cause) = cancelled else {
        panic!("cancellation lost its typed provider cause");
    };
    assert!(matches!(
        *cause,
        crate::projection::provider::recovery::error::ProviderBootstrapCause::Vfs(
            layrs_vfs::Error::Cancelled
        )
    ));
    let ImportSource::Ordinary { version, .. } = &mut prefix.source else {
        unreachable!()
    };
    version.effective_image_digest = "b".repeat(64);
    assert!(DerivedDeniedCoverage::from_observation(&prefix, TX, &normalized, None).is_err());
    assert!(
        TransitionDeniedCoverageRecord::from_observation(&prefix, TX, &normalized, None).is_err()
    );
}
