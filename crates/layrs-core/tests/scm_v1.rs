use layrs_core::scm::*;

fn object_id(hex: char) -> String {
    format!("blake3:{}", hex.to_string().repeat(64))
}

#[test]
fn domain_separated_chunk_and_file_match_goldens() {
    let chunk = Chunk::new(b"hello".to_vec()).unwrap();
    assert_eq!(
        chunk.id().as_str(),
        "blake3:4eb1f72d41aa4b778090debd8e33feded93a1a98fc76bfad36fea32d94a5ce80"
    );
    assert_eq!(chunk.identity_bytes(), b"layrs-chunk-v1\0hello");
    assert_eq!(
        FileId::from_bytes(b"hello").as_str(),
        "blake3:cf2b3005225d995b905536bff0fb60d3486f4043ca86bf44da845052c6a4cb7d"
    );
    assert_ne!(chunk.id().as_str(), FileId::from_bytes(b"hello").as_str());
}

#[test]
fn step_canonical_payload_sorts_before_hash_and_matches_golden() {
    let parents = vec![
        StepId::parse(object_id('b')).unwrap(),
        StepId::parse(object_id('a')).unwrap(),
    ];
    let root = TreeId::parse(object_id('c')).unwrap();
    let step = Step::new(parents, root).unwrap();
    assert_eq!(
        step.id().as_str(),
        "blake3:a1b5c9dfe69fcaad375f6f59e7e20eb9ddc93e1275997a0a2d58ba76b3e12833"
    );
    let expected = format!(
        "layrs-step-v1\0parent-count\t2\nparent\t{}\nparent\t{}\nroot-tree\t{}\n",
        object_id('a'),
        object_id('b'),
        object_id('c')
    );
    assert_eq!(step.identity_bytes(), expected.as_bytes());
    assert!(
        Step::new(
            vec![StepId::parse(object_id('a')).unwrap(); 2],
            TreeId::parse(object_id('c')).unwrap()
        )
        .is_err()
    );
}

#[test]
fn tree_sorts_utf8_names_rejects_duplicates_and_matches_golden() {
    let entries = vec![
        TreeEntry::new(
            RepoPathComponent::parse("z.txt").unwrap(),
            TreeEntryTarget::file(FileId::parse(object_id('1')).unwrap()),
            false,
        ),
        TreeEntry::new(
            RepoPathComponent::parse("a").unwrap(),
            TreeEntryTarget::tree(TreeId::parse(object_id('2')).unwrap()),
            false,
        ),
    ];
    let tree = Tree::new(entries).unwrap();
    assert_eq!(
        tree.id().as_str(),
        "blake3:e74dfaab4bbed9d499cb372665f58eed3bd71211e3a810b9e2b72744ec5ed42a"
    );
    assert_eq!(tree.entries()[0].name().as_str(), "a");
    let duplicate = TreeEntry::new(
        RepoPathComponent::parse("a").unwrap(),
        TreeEntryTarget::file(FileId::parse(object_id('3')).unwrap()),
        false,
    );
    assert!(Tree::new(vec![tree.entries()[0].clone(), duplicate]).is_err());
}

#[test]
fn repository_view_symlink_and_tombstone_match_goldens() {
    let view = RepositoryView::new(
        SpaceId::parse("space:golden").unwrap(),
        vec![],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    assert_eq!(
        view.id().as_str(),
        "blake3:2208e8e832cba3c0985e1102dc5bf68c09b7df432d888f0467e3f6fb988a717f"
    );
    assert_eq!(
        String::from_utf8(view.identity_bytes().unwrap()).unwrap(),
        "layrs-repository-view-v1\0{\"layerHeads\":[],\"markerIds\":[],\"releaseIds\":[],\"schemaVersion\":\"layrs.repository-view.v1\",\"spaceId\":\"space:golden\",\"worktrees\":[]}"
    );

    let symlink = Symlink::new("../shared/config.toml").unwrap();
    assert_eq!(
        symlink.id().as_str(),
        "blake3:757ce1bd12694eba55ef7e7e6f0d507cb4a6478b55a9b8f0cb8ab3f1688ba2e3"
    );
    let tombstone = Tombstone::new(Some(ObjectId::parse(object_id('a')).unwrap())).unwrap();
    assert_eq!(
        tombstone.id().as_str(),
        "blake3:94098ace9530a52aa515c3db11db98eb2b90cc2f886cbed44f13aeb97e2f1ad3"
    );
}

#[test]
fn one_file_id_can_have_two_verified_manifests() {
    let chunk = Chunk::new(b"hello".to_vec()).unwrap();
    let single =
        FileManifest::from_chunks(ChunkingDescriptor::single(), std::slice::from_ref(&chunk))
            .unwrap();
    let fixed = FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(1_048_576).unwrap(),
        std::slice::from_ref(&chunk),
    )
    .unwrap();
    assert_eq!(single.file_id(), fixed.file_id());
    assert_ne!(single.id(), fixed.id());
    assert_eq!(
        single.id().as_str(),
        "blake3:8285c740fc07c66592c95622b313067642398e6c0d8e69f5c2e5eed3129e3321"
    );
    assert_eq!(
        fixed.id().as_str(),
        "blake3:454a8cae3c27fcf366a6242d2d16a5259474ec36c76cb97366a8e2a780c6569a"
    );
    assert_eq!(
        single.reconstruct(|_| Some(b"hello".to_vec())).unwrap(),
        b"hello"
    );
}

#[test]
fn absent_is_not_an_empty_file() {
    let empty = FileManifest::empty(ChunkingDescriptor::single()).unwrap();
    let absent = FileState::Absent;
    let present = FileState::Present(empty.file_id().clone());
    assert_ne!(absent, present);
    assert_eq!(empty.byte_length(), 0);
    assert!(empty.chunks().is_empty());
}

#[test]
fn ids_paths_nfc_and_chunking_are_strict() {
    assert!(StepId::parse(object_id('a')).is_ok());
    assert!(StepId::parse(format!("blake3:{}", "A".repeat(64))).is_err());
    assert!(StepId::parse("blake3:abc").is_err());
    assert!(SpaceId::parse("layer:not-space").is_err());
    assert!(RepoPath::parse("src/lib.rs").is_ok());
    assert!(RepoPath::parse("assets/e\u{301}.txt").is_err());
    assert!(RepoPath::parse("a/../b").is_err());
    assert!(RepoPathComponent::parse("a".repeat(256)).is_err());
    assert!(ChunkingDescriptor::fast_cdc(1024, 512, 4096).is_err());
}

#[test]
fn decode_revalidates_claimed_ids_unknown_fields_and_required_nullables() {
    let bad_chunk = r#"{"schemaVersion":"layrs.chunk.v1","chunkId":"blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","byteLength":5,"content":{"encoding":"base64","bytes":"aGVsbG8="}}"#;
    assert!(serde_json::from_str::<Chunk>(bad_chunk).is_err());

    let tombstone = Tombstone::new(None).unwrap();
    let encoded = serde_json::to_string(&tombstone).unwrap();
    assert!(serde_json::from_str::<Tombstone>(&encoded).is_ok());
    let mut missing: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    let removed = missing.as_object_mut().unwrap().remove("previousObjectId");
    assert!(removed.is_some(), "{encoded}");
    assert!(serde_json::from_value::<Tombstone>(missing).is_err());

    let extra = format!(
        r#"{{"schemaVersion":"layrs.step.v1","stepId":"{}","parentStepIds":[],"rootTreeId":"{}","extra":true}}"#,
        object_id('a'),
        object_id('b')
    );
    assert!(serde_json::from_str::<Step>(&extra).is_err());
}

#[test]
fn metadata_is_sorted_and_duplicate_keys_are_rejected() {
    let metadata = Metadata::new(vec![
        MetadataEntry::new("z", "1").unwrap(),
        MetadataEntry::new("a", "2").unwrap(),
    ])
    .unwrap();
    assert_eq!(metadata.entries()[0].key(), "a");
    assert!(
        Metadata::new(vec![
            MetadataEntry::new("a", "1").unwrap(),
            MetadataEntry::new("a", "2").unwrap()
        ])
        .is_err()
    );
}

#[test]
fn jcs_orders_object_keys_as_utf16() {
    let mut object = serde_json::Map::new();
    object.insert("\u{e000}".to_owned(), serde_json::Value::from(1));
    object.insert("\u{1f600}".to_owned(), serde_json::Value::from(2));
    let encoded =
        String::from_utf8(canonical_json(&serde_json::Value::Object(object)).unwrap()).unwrap();
    assert_eq!(encoded, "{\"😀\":2,\"\u{e000}\":1}");
}

struct Resolver {
    has_manifest: bool,
    ancestor_pair: Option<(StepId, StepId)>,
    denied: Option<&'static str>,
}

impl Resolver {
    fn allows(&self, capability: &'static str) -> bool {
        self.denied != Some(capability)
    }
}

impl AdmissionResolver for Resolver {
    fn chunk_bytes(&self, _: &ChunkId) -> Option<Vec<u8>> {
        Some(b"hello".to_vec())
    }
    fn tree_exists(&self, _: &TreeId) -> bool {
        self.allows("tree")
    }
    fn tree_admitted_to_space(&self, _: &TreeId, _: &SpaceId) -> bool {
        self.allows("tree_space")
    }
    fn conflict_exists(&self, _: &ConflictId) -> bool {
        true
    }
    fn symlink_exists(&self, _: &SymlinkId) -> bool {
        true
    }
    fn tombstone_exists(&self, _: &TombstoneId) -> bool {
        true
    }
    fn file_has_verified_manifest(&self, _: &FileId) -> bool {
        self.has_manifest
    }
    fn step_exists(&self, _: &StepId) -> bool {
        self.allows("step")
    }
    fn step_admitted_to_space(&self, _: &StepId, _: &SpaceId) -> bool {
        self.allows("step_space")
    }
    fn is_step_ancestor(&self, ancestor: &StepId, descendant: &StepId) -> bool {
        self.ancestor_pair
            .as_ref()
            .is_some_and(|pair| &pair.0 == ancestor && &pair.1 == descendant)
    }
    fn layer_in_space(&self, _: &LayerId, _: &SpaceId) -> bool {
        self.allows("layer_space")
    }
    fn marker_in_space(&self, _: &MarkerId, _: &SpaceId) -> bool {
        true
    }
    fn release_in_space(&self, _: &ReleaseId, _: &SpaceId) -> bool {
        self.allows("release_space")
    }
    fn worktree_in_space(&self, _: &WorktreeId, _: &SpaceId) -> bool {
        self.allows("worktree_space")
    }
    fn worktree_exists(&self, _: &WorktreeId) -> bool {
        self.allows("worktree")
    }
    fn operation_in_space(&self, _: &OperationId, _: &SpaceId) -> bool {
        self.allows("operation_space")
    }
    fn operation_exists(&self, _: &OperationId) -> bool {
        self.allows("operation")
    }
    fn operation_reaches_step(&self, _: &OperationId, _: &StepId) -> bool {
        self.allows("operation_step")
    }
    fn repository_view_in_space(&self, _: &RepositoryViewId, _: &SpaceId) -> bool {
        self.allows("view_space")
    }
    fn space_exists(&self, _: &SpaceId) -> bool {
        self.allows("space")
    }
    fn store_exists(&self, _: &StoreId) -> bool {
        self.allows("store")
    }
    fn proof_in_space(&self, _: &ProofId, _: &SpaceId) -> bool {
        self.allows("proof_space")
    }
    fn actor_exists(&self, _: &ActorId) -> bool {
        self.allows("actor")
    }
    fn device_exists(&self, _: &DeviceId) -> bool {
        self.allows("device")
    }
}

#[test]
fn contextual_admission_checks_closure_and_antichains() {
    let file_id = FileId::from_bytes(b"hello");
    let tree = Tree::new(vec![TreeEntry::new(
        RepoPathComponent::parse("file.txt").unwrap(),
        TreeEntryTarget::file(file_id),
        false,
    )])
    .unwrap();
    let resolver = Resolver {
        has_manifest: false,
        ancestor_pair: None,
        denied: None,
    };
    assert!(validate_tree_admission(&tree, &resolver).is_err());

    let first = StepId::parse(object_id('a')).unwrap();
    let second = StepId::parse(object_id('b')).unwrap();
    let heads = LayerHeadSet::new(
        LayerId::parse("layer:main").unwrap(),
        vec![first.clone(), second.clone()],
    )
    .unwrap();
    let view = RepositoryView::new(
        SpaceId::parse("space:test").unwrap(),
        vec![heads],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    let resolver = Resolver {
        has_manifest: true,
        ancestor_pair: Some((first, second)),
        denied: None,
    };
    let error = validate_repository_view_admission(&view, &resolver).unwrap_err();
    assert_eq!(error.code(), "layer_heads_not_antichain");
}

fn accepted_resolver() -> Resolver {
    Resolver {
        has_manifest: true,
        ancestor_pair: None,
        denied: None,
    }
}

fn timestamp() -> Timestamp {
    Timestamp::parse("2026-07-19T12:34:56.123+02:00").unwrap()
}

#[test]
fn contract_error_codes_are_stable() {
    let parent = StepId::parse(object_id('a')).unwrap();
    let error = Step::new(
        vec![parent.clone(), parent],
        TreeId::parse(object_id('b')).unwrap(),
    )
    .unwrap_err();
    assert_eq!(error.code(), "duplicate_step_parent");

    let gap = FileManifest::new(
        FileId::parse(object_id('c')).unwrap(),
        2,
        ChunkingDescriptor::single(),
        vec![FileChunk::new(ChunkId::parse(object_id('d')).unwrap(), 1, 2).unwrap()],
    )
    .unwrap_err();
    assert_eq!(gap.code(), "file_manifest_chunks_not_contiguous");

    let fast_cdc = ChunkingDescriptor::fast_cdc(1024, 512, 4096).unwrap_err();
    assert_eq!(fast_cdc.code(), "fastcdc_parameter_order");
    assert_eq!(
        RepoPath::parse("..").unwrap_err().code(),
        "invalid_repo_path"
    );
}

#[test]
fn timestamp_is_real_rfc3339_and_preserves_input() {
    let value = "2024-02-29T23:59:59.123456789+02:30";
    assert_eq!(Timestamp::parse(value).unwrap().as_str(), value);
    assert!(Timestamp::parse("2023-02-29T12:00:00Z").is_err());
    assert!(Timestamp::parse("2024-01-01T24:00:00Z").is_err());
    assert!(Timestamp::parse("2024-13-01T12:00:00Z").is_err());
}

#[test]
fn symlink_limit_counts_unicode_scalars() {
    assert!(Symlink::new("é".repeat(4096)).is_ok());
    assert!(Symlink::new("é".repeat(4097)).is_err());
}

#[test]
fn repository_view_requires_materialized_tree_in_same_space() {
    let state = WorktreeViewState::new(
        WorktreeId::parse("worktree:test").unwrap(),
        None,
        None,
        Some(TreeId::parse(object_id('a')).unwrap()),
        WorktreeStatus::Ready,
    );
    let view = RepositoryView::new(
        SpaceId::parse("space:test").unwrap(),
        vec![],
        vec![],
        vec![],
        vec![state],
    )
    .unwrap();
    let resolver = Resolver {
        denied: Some("tree_space"),
        ..accepted_resolver()
    };
    assert!(validate_repository_view_admission(&view, &resolver).is_err());
}

#[test]
fn operation_rewrite_steps_must_be_admitted_to_operation_space() {
    let rewrite = RewriteTarget::new(
        StepId::parse(object_id('a')).unwrap(),
        vec![StepId::parse(object_id('b')).unwrap()],
    )
    .unwrap();
    let operation = Operation::new(
        OperationId::parse("operation:rewrite").unwrap(),
        SpaceId::parse("space:test").unwrap(),
        vec![OperationId::parse("operation:parent").unwrap()],
        RepositoryViewId::parse(object_id('c')).unwrap(),
        OperationKind::RewriteSteps,
        "rewrite",
        vec![rewrite],
        None,
        None,
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let resolver = Resolver {
        denied: Some("step_space"),
        ..accepted_resolver()
    };
    assert!(validate_operation_admission(&operation, &resolver).is_err());
}

#[test]
fn business_object_admissions_enforce_cross_space_references() {
    let space = SpaceId::parse("space:test").unwrap();
    let layer = Layer::new(
        LayerId::parse("layer:child").unwrap(),
        space.clone(),
        "child",
        LayerKind::Base,
        LayerStatus::Active,
        vec![LayerId::parse("layer:parent").unwrap()],
        LayerSettings::new(true, 100).unwrap(),
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    assert!(
        validate_layer_admission(
            &layer,
            &Resolver {
                denied: Some("layer_space"),
                ..accepted_resolver()
            }
        )
        .is_err()
    );

    let observation = StepObservation::new(
        ObservationId::parse("observation:test").unwrap(),
        StepId::parse(object_id('a')).unwrap(),
        Some(space.clone()),
        Some(OperationId::parse("operation:test").unwrap()),
        None,
        None,
        None,
        timestamp(),
        CaptureReason::Received,
        Metadata::empty(),
    );
    assert!(
        validate_step_observation_admission(
            &observation,
            &Resolver {
                denied: Some("operation_space"),
                ..accepted_resolver()
            }
        )
        .is_err()
    );
    let without_space = StepObservation::new(
        ObservationId::parse("observation:detached").unwrap(),
        StepId::parse(object_id('a')).unwrap(),
        None,
        Some(OperationId::parse("operation:test").unwrap()),
        None,
        None,
        None,
        timestamp(),
        CaptureReason::Received,
        Metadata::empty(),
    );
    assert!(
        validate_step_observation_admission(
            &without_space,
            &Resolver {
                denied: Some("operation_space"),
                ..accepted_resolver()
            }
        )
        .is_ok()
    );

    let proof = ProofId::parse("proof:test").unwrap();
    let marker = Marker::new(
        MarkerId::parse("marker:test").unwrap(),
        space.clone(),
        StepId::parse(object_id('a')).unwrap(),
        MarkerKind::Checkpoint,
        "checkpoint",
        "",
        "",
        None,
        timestamp(),
        vec![proof.clone()],
        Metadata::empty(),
    )
    .unwrap();
    assert!(
        validate_marker_admission(
            &marker,
            &Resolver {
                denied: Some("proof_space"),
                ..accepted_resolver()
            }
        )
        .is_err()
    );

    let release = Release::new(
        ReleaseId::parse("release:test").unwrap(),
        space.clone(),
        StepId::parse(object_id('a')).unwrap(),
        "1.0.0",
        "",
        "",
        None,
        timestamp(),
        vec![proof],
        Metadata::empty(),
    )
    .unwrap();
    assert!(
        validate_release_admission(
            &release,
            &Resolver {
                denied: Some("step_space"),
                ..accepted_resolver()
            }
        )
        .is_err()
    );

    let publication = ReleasePublication::new(
        ReleasePublicationId::parse("release-publication:test").unwrap(),
        ReleaseId::parse("release:test").unwrap(),
        space,
        None,
        None,
        None,
        timestamp(),
        Metadata::empty(),
    );
    assert!(
        validate_release_publication_admission(
            &publication,
            &Resolver {
                denied: Some("release_space"),
                ..accepted_resolver()
            }
        )
        .is_err()
    );
}
