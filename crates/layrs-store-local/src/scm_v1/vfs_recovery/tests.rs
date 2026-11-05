use super::*;

#[test]
fn export_verification_checks_cancellation_between_bounded_blocks() {
    let bytes = vec![0x63; 128 * 1024];
    let mut input = std::io::Cursor::new(&bytes);
    let mut output = Vec::new();
    let mut checks = 0;
    let error = copy_export_checked(&mut input, &mut output, &mut || {
        checks += 1;
        if checks > 1 {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    assert_eq!(output, bytes[..64 * 1024]);
    assert_eq!(input.position(), 64 * 1024);
}

fn capture_batch(id: &WorktreeId) -> WriteBatch {
    WriteBatch::new(
        layrs_store::scm::TransactionId::new("transaction:journal-boundary").unwrap(),
        vec![],
        vec![],
        vec![],
        vec![StoreMutation::WorktreeActiveOperation(
            layrs_store::scm::WorktreeActiveOperationCas::new(
                id.clone(),
                OperationId::parse("operation:before").unwrap(),
                OperationId::parse("operation:after").unwrap(),
            )
            .unwrap(),
        )],
    )
    .unwrap()
}

fn capture_intent(id: &WorktreeId) -> VfsCaptureIntent {
    let batch = capture_batch(id);
    VfsCaptureIntent {
        transaction_id: batch.transaction_id().to_string(),
        request_digest: batch.request_digest().to_string(),
        operation_id: OperationId::parse("operation:after").unwrap(),
        step_id: StepId::parse(format!("blake3:{}", "0".repeat(64))).unwrap(),
        receipt: ApplyBatchReceipt::new(&batch),
    }
}

fn activation_intent(id: &WorktreeId) -> VfsActivationIntent {
    let capture = capture_intent(id);
    VfsActivationIntent {
        transaction_id: capture.transaction_id,
        request_digest: capture.request_digest,
        operation_id: capture.operation_id,
        step_id: capture.step_id,
        expected_registration_operation_id: OperationId::parse("operation:before").unwrap(),
        receipt: capture.receipt,
    }
}

#[test]
fn production_capacity_reserves_ack_and_allows_only_one_legacy_terminal_event() {
    let intent = capture_intent(&WorktreeId::parse("worktree:capacity").unwrap());
    let prepared = VfsEvent::CapturePrepared {
        intent: intent.clone(),
    };
    let resolved = VfsEvent::CaptureResolved {
        transaction_id: intent.transaction_id,
        accepted: true,
    };
    assert!(require_append_capacity(65_534, MAX_RECORDS, &prepared).is_ok());
    assert!(require_append_capacity(65_535, MAX_RECORDS, &prepared).is_err());
    assert!(require_append_capacity(65_535, MAX_RECORDS, &resolved).is_ok());
    assert!(require_append_capacity(65_536, MAX_RECORDS, &resolved).is_ok());
    assert!(
        require_append_capacity(
            65_536,
            MAX_RECORDS,
            &VfsEvent::CaptureResolved {
                transaction_id: "absent".into(),
                accepted: false,
            }
        )
        .is_err()
    );
    assert!(require_append_capacity(65_536, MAX_RECORDS, &VfsEvent::Ready).is_err());
    assert!(require_append_capacity(65_537, MAX_RECORDS, &resolved).is_err());
    let activation = activation_intent(&WorktreeId::parse("worktree:capacity").unwrap());
    let prepared = VfsEvent::ActivationPrepared {
        intent: activation.clone(),
    };
    assert!(require_append_capacity(65_534, MAX_RECORDS, &prepared).is_ok());
    assert!(require_append_capacity(65_535, MAX_RECORDS, &prepared).is_err());
    for accepted in [false, true] {
        let resolved = VfsEvent::ActivationResolved {
            transaction_id: activation.transaction_id.clone(),
            accepted,
        };
        assert!(require_append_capacity(65_535, MAX_RECORDS, &resolved).is_ok());
        assert!(require_append_capacity(65_536, MAX_RECORDS, &resolved).is_err());
        assert!(require_replay_capacity(65_536, MAX_RECORDS, &resolved).is_err());
    }
}

#[test]
fn activation_replay_requires_exact_receipt_and_never_infers_legacy_baseline() {
    let root = std::env::temp_dir().join(format!(
        "layrs-vfs-activation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let store_root = root.join("store");
    let store_id = StoreId::parse("store:activation").unwrap();
    let worktree_id = WorktreeId::parse("worktree:activation").unwrap();
    drop(super::super::init(&store_root, store_id.clone()).unwrap());
    let identity = PinnedDirectory::open(&store_root).unwrap().identity_token();
    let metadata = PinnedDirectory::open(&store_root.join("metadata"))
        .unwrap()
        .identity_token();
    let open = |create| {
        VfsRecovery::acquire_bounded(
            &store_root,
            &identity,
            &metadata,
            &store_id,
            &worktree_id,
            create,
            12,
        )
    };
    let mut journal = open(true).unwrap();
    for event in [
        VfsEvent::Reserved { locator: vec![1] },
        VfsEvent::Registered,
        VfsEvent::ProviderPrepared { ticket: vec![2] },
        VfsEvent::ProviderPopulated,
        VfsEvent::ControlPrepared { control: vec![3] },
        VfsEvent::LocatorPublished { control: vec![3] },
    ] {
        journal.append(event, &NoVfsJournalFaults).unwrap();
    }
    let legacy = transition(Some(journal.state()), &VfsEvent::Ready).unwrap();
    assert!(legacy.provider_capture_baseline().is_none());
    let batch = capture_batch(&worktree_id);
    let expected = activation_intent(&worktree_id);
    assert!(
        journal
            .prepare_activation(
                &batch,
                &expected.step_id,
                &OperationId::parse("operation:foreign").unwrap(),
                &NoVfsJournalFaults
            )
            .is_err()
    );
    assert_eq!(journal.next_sequence, 6);
    journal.record_limit = 7;
    assert!(
        journal
            .prepare_activation(
                &batch,
                &expected.step_id,
                &expected.expected_registration_operation_id,
                &NoVfsJournalFaults
            )
            .is_err()
    );
    assert_eq!(journal.next_sequence, 6);
    journal.record_limit = 12;
    journal
        .prepare_activation(
            &batch,
            &expected.step_id,
            &expected.expected_registration_operation_id,
            &NoVfsJournalFaults,
        )
        .unwrap();
    drop(journal); // Before acceptance: replay may only return to the original registration.
    let mut journal = open(false).unwrap();
    assert_eq!(
        journal.resolve_activation(&NoVfsJournalFaults).unwrap(),
        Some(false)
    );
    assert_eq!(journal.state().phase, VfsPhase::LocatorPublished);
    assert!(journal.state().provider_capture_baseline().is_none());
    journal
        .prepare_activation(
            &batch,
            &expected.step_id,
            &expected.expected_registration_operation_id,
            &NoVfsJournalFaults,
        )
        .unwrap();
    let committed = super::super::open_exclusive(&store_root).unwrap();
    let connection = committed.begin_write_connection().unwrap();
    connection.execute("INSERT INTO applied_transactions(transaction_id,request_digest,receipt_json,applied_at_ms) VALUES(?1,?2,?3,0)", rusqlite::params![
        expected.transaction_id, expected.request_digest,
        String::from_utf8(layrs_core::scm::canonical_json(&expected.receipt).unwrap()).unwrap(),
    ]).unwrap();
    drop(connection);
    drop(committed);
    drop(journal); // After acceptance, before acknowledgment: exact Operation survives.
    let mut journal = open(false).unwrap();
    assert_eq!(
        journal.resolve_activation(&NoVfsJournalFaults).unwrap(),
        Some(true)
    );
    assert_eq!(
        journal.state().provider_capture_baseline(),
        Some((&expected.operation_id, &expected.step_id))
    );
    let sequence = journal.next_sequence;
    assert_eq!(
        journal.resolve_activation(&NoVfsJournalFaults).unwrap(),
        None
    );
    assert_eq!(journal.next_sequence, sequence);
    drop(journal);
    let journal = open(false).unwrap();
    assert_eq!(
        journal.state().provider_capture_baseline(),
        Some((&expected.operation_id, &expected.step_id))
    );
    drop(journal);
    let committed = super::super::open_exclusive(&store_root).unwrap();
    let connection = committed.begin_write_connection().unwrap();
    assert_ne!(
        expected.request_digest,
        format!("blake3:{}", "0".repeat(64))
    );
    connection
        .execute(
            "UPDATE applied_transactions SET request_digest=?1 WHERE transaction_id=?2",
            rusqlite::params![
                format!("blake3:{}", "0".repeat(64)),
                expected.transaction_id,
            ],
        )
        .unwrap();
    drop(connection);
    drop(committed);
    assert!(
        open(false).is_err(),
        "acknowledged activation still requires the exact receipt on reopen"
    );
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_capture_below_capacity_requires_receipt_for_replayed_lineage() {
    let root = std::env::temp_dir().join(format!(
        "layrs-vfs-legacy-receipt-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let store_root = root.join("store");
    let store_id = StoreId::parse("store:legacy-receipt").unwrap();
    let worktree_id = WorktreeId::parse("worktree:legacy-receipt").unwrap();
    drop(super::super::init(&store_root, store_id.clone()).unwrap());
    let identity = PinnedDirectory::open(&store_root).unwrap().identity_token();
    let metadata = PinnedDirectory::open(&store_root.join("metadata"))
        .unwrap()
        .identity_token();
    let open = |create| {
        VfsRecovery::acquire_bounded(
            &store_root,
            &identity,
            &metadata,
            &store_id,
            &worktree_id,
            create,
            12,
        )
    };
    let mut journal = open(true).unwrap();
    for event in [
        VfsEvent::Reserved { locator: vec![1] },
        VfsEvent::Registered,
        VfsEvent::ProviderPrepared { ticket: vec![2] },
        VfsEvent::ProviderPopulated,
        VfsEvent::ControlPrepared { control: vec![3] },
        VfsEvent::LocatorPublished { control: vec![3] },
    ] {
        journal.append(event, &NoVfsJournalFaults).unwrap();
    }
    assert!(
        journal
            .append(VfsEvent::Ready, &NoVfsJournalFaults)
            .is_err()
    );
    journal
        .append_checked(VfsEvent::Ready, &NoVfsJournalFaults)
        .unwrap();
    assert!(journal.state().provider_capture_baseline().is_none());
    let expected = capture_intent(&worktree_id);
    journal
        .prepare_capture(
            &capture_batch(&worktree_id),
            &expected.step_id,
            &NoVfsJournalFaults,
        )
        .unwrap();
    assert!(journal.state().provider_capture_baseline().is_none());
    let committed = super::super::open_exclusive(&store_root).unwrap();
    let connection = committed.begin_write_connection().unwrap();
    connection.execute("INSERT INTO applied_transactions(transaction_id,request_digest,receipt_json,applied_at_ms) VALUES(?1,?2,?3,0)", rusqlite::params![
        expected.transaction_id, expected.request_digest,
        String::from_utf8(layrs_core::scm::canonical_json(&expected.receipt).unwrap()).unwrap(),
    ]).unwrap();
    drop(connection);
    drop(committed);
    assert_eq!(
        journal.resolve_capture(&NoVfsJournalFaults).unwrap(),
        Some(true)
    );
    assert_eq!(journal.next_sequence, 9);
    drop(journal);
    let journal = open(false).unwrap();
    assert!(journal.state().activation.is_none());
    assert_eq!(
        journal.state().provider_capture_baseline(),
        Some((&expected.operation_id, &expected.step_id))
    );
    drop(journal);
    let committed = super::super::open_exclusive(&store_root).unwrap();
    let connection = committed.begin_write_connection().unwrap();
    assert_ne!(
        expected.request_digest,
        format!("blake3:{}", "0".repeat(64))
    );
    connection
        .execute(
            "UPDATE applied_transactions SET request_digest=?1 WHERE transaction_id=?2",
            rusqlite::params![
                format!("blake3:{}", "0".repeat(64)),
                expected.transaction_id
            ],
        )
        .unwrap();
    drop(connection);
    drop(committed);
    assert!(
        open(false).is_err(),
        "legacy capture below capacity needs its exact accepted receipt"
    );
    let committed = super::super::open_exclusive(&store_root).unwrap();
    let connection = committed.begin_write_connection().unwrap();
    connection
        .execute(
            "UPDATE applied_transactions SET request_digest=?1 WHERE transaction_id=?2",
            rusqlite::params![expected.request_digest, expected.transaction_id],
        )
        .unwrap();
    drop(connection);
    drop(committed);
    assert_eq!(
        open(false).unwrap().state().provider_capture_baseline(),
        Some((&expected.operation_id, &expected.step_id))
    );
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn exhausted_legacy_pending_journal_reopens_and_resolves_exact_receipt_once() {
    let root = std::env::temp_dir().join(format!(
        "layrs-vfs-terminal-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let store_root = root.join("store");
    let store_id = StoreId::parse("store:vfs-terminal").unwrap();
    let worktree_id = WorktreeId::parse("worktree:vfs-terminal").unwrap();
    drop(super::super::init(&store_root, store_id.clone()).unwrap());
    let identity = PinnedDirectory::open(&store_root).unwrap().identity_token();
    let metadata = PinnedDirectory::open(&store_root.join("metadata"))
        .unwrap()
        .identity_token();
    // Same parser/writer at a smaller physical limit: seven creation records
    // leave exactly one slot, which is insufficient for a new capture.
    let open = |create| {
        VfsRecovery::acquire_bounded(
            &store_root,
            &identity,
            &metadata,
            &store_id,
            &worktree_id,
            create,
            8,
        )
    };
    let mut journal = open(true).unwrap();
    for event in [
        VfsEvent::Reserved { locator: vec![1] },
        VfsEvent::Registered,
        VfsEvent::ProviderPrepared { ticket: vec![2] },
        VfsEvent::ProviderPopulated,
        VfsEvent::ControlPrepared { control: vec![3] },
        VfsEvent::LocatorPublished { control: vec![3] },
        VfsEvent::Ready,
    ] {
        if matches!(event, VfsEvent::Ready) {
            assert!(journal.append(event.clone(), &NoVfsJournalFaults).is_err());
            // Test-only old-producer fixture; public writers cannot emit Ready.
            journal.append_checked(event, &NoVfsJournalFaults).unwrap();
        } else {
            journal.append(event, &NoVfsJournalFaults).unwrap();
        }
    }
    let intent = capture_intent(&worktree_id);
    assert!(
        journal
            .prepare_capture(
                &capture_batch(&worktree_id),
                &intent.step_id,
                &NoVfsJournalFaults
            )
            .is_err()
    );
    assert_eq!(journal.next_sequence, 7);
    assert!(journal.state().pending_capture.is_none());
    // Reproduce an old writer's durable final-slot intent without weakening
    // the current append path. All later replay/resolution uses real methods.
    let record = Record {
        schema: SCHEMA.into(),
        sequence: 7,
        previous_digest: journal.previous_digest.clone(),
        store_id: store_id.clone(),
        worktree_id: worktree_id.clone(),
        event: VfsEvent::CapturePrepared {
            intent: intent.clone(),
        },
    };
    journal
        .publish(
            "00000000007.json",
            &serde_json::to_vec(&record).unwrap(),
            &NoVfsJournalFaults,
        )
        .unwrap();
    drop(journal);
    let mut journal = open(false).unwrap();
    assert_eq!(journal.next_sequence, 8);
    assert_eq!(
        journal.resolve_capture(&NoVfsJournalFaults).unwrap(),
        Some(false)
    );
    assert_eq!(journal.next_sequence, 8);
    assert!(journal.state().pending_capture.is_some());
    assert!(journal.state().last_accepted.is_none());
    assert!(
        journal
            .ensure_capture_capacity()
            .unwrap_err()
            .to_string()
            .contains("recovery export")
    );
    assert!(
        journal
            .append(VfsEvent::Ready, &NoVfsJournalFaults)
            .is_err()
    );
    journal
        .publish_endpoint("00000000-0000-0000-0000-000000000001", b"recovery endpoint")
        .unwrap();
    drop(journal);
    let mut journal = open(false).unwrap();
    assert_eq!(
        journal.resolve_capture(&NoVfsJournalFaults).unwrap(),
        Some(false)
    );
    assert!(journal.state().pending_capture.is_some());
    // Exhaustion never prevents the independent verified-copy export path
    // or changes unaccepted dirty bytes. Native tests separately cover the
    // provider checkpoint feeding this same export writer.
    let dirty = b"unaccepted dirty bytes must survive a full journal";
    let dirty_path = root.join("dirty.txt");
    std::fs::write(&dirty_path, dirty).unwrap();
    let mut export =
        VfsExportWriter::create(&root, "00000000-0000-0000-0000-000000000003").unwrap();
    export
        .write_file(
            &layrs_core::scm::RepoPath::parse("dirty.txt").unwrap(),
            &layrs_core::scm::FileId::from_bytes(dirty),
            dirty.len() as u64,
            &mut |output| output.write_all(&std::fs::read(&dirty_path)?),
        )
        .unwrap();
    let exported = export.finish(br#"{"complete":true}"#).unwrap();
    assert_eq!(std::fs::read(exported.join("dirty.txt")).unwrap(), dirty);
    assert_eq!(std::fs::read(&dirty_path).unwrap(), dirty);
    // Emulate the independently committed acceptance receipt. An altered
    // receipt must refuse resolution and leave the pending intent untouched.
    let committed = super::super::open_exclusive(&store_root).unwrap();
    let connection = committed.begin_write_connection().unwrap();
    let mut bad_receipt = intent.receipt.clone();
    let mut bad_value = serde_json::to_value(&bad_receipt).unwrap();
    bad_value["mutationCount"] = serde_json::json!(99);
    bad_receipt = serde_json::from_value(bad_value).unwrap();
    connection.execute("INSERT INTO applied_transactions(transaction_id,request_digest,receipt_json,applied_at_ms) VALUES(?1,?2,?3,0)", rusqlite::params![
        intent.transaction_id, intent.request_digest,
        String::from_utf8(layrs_core::scm::canonical_json(&bad_receipt).unwrap()).unwrap(),
    ]).unwrap();
    drop(connection);
    drop(committed);
    assert!(journal.resolve_capture(&NoVfsJournalFaults).is_err());
    assert_eq!(journal.next_sequence, 8);
    assert!(journal.state().pending_capture.is_some());
    let committed = super::super::open_exclusive(&store_root).unwrap();
    let connection = committed.begin_write_connection().unwrap();
    connection
        .execute(
            "UPDATE applied_transactions SET receipt_json=?1 WHERE transaction_id=?2",
            rusqlite::params![
                String::from_utf8(layrs_core::scm::canonical_json(&intent.receipt).unwrap())
                    .unwrap(),
                intent.transaction_id,
            ],
        )
        .unwrap();
    drop(connection);
    drop(committed);
    assert_eq!(
        journal.resolve_capture(&NoVfsJournalFaults).unwrap(),
        Some(true)
    );
    assert_eq!(journal.next_sequence, 9);
    assert!(
        journal
            .append(VfsEvent::Ready, &NoVfsJournalFaults)
            .is_err()
    );
    assert!(
        journal
            .prepare_capture(
                &capture_batch(&worktree_id),
                &intent.step_id,
                &NoVfsJournalFaults
            )
            .is_err()
    );
    // Endpoint renewal is not an event and must remain available to recovery.
    journal
        .publish_endpoint("00000000-0000-0000-0000-000000000002", b"endpoint")
        .unwrap();
    drop(journal);
    let mut reopened = open(false).unwrap();
    assert_eq!(reopened.next_sequence, 9);
    assert!(reopened.state().pending_capture.is_none());
    assert_eq!(
        reopened
            .state()
            .last_accepted
            .as_ref()
            .unwrap()
            .operation_id,
        intent.operation_id
    );
    assert_eq!(reopened.resolve_capture(&NoVfsJournalFaults).unwrap(), None);
    drop(reopened);
    // Replay of the exceptional terminal slot rechecks the committed receipt,
    // rather than trusting only a self-consistent journal digest chain.
    let committed = super::super::open_exclusive(&store_root).unwrap();
    let connection = committed.begin_write_connection().unwrap();
    assert_ne!(intent.request_digest, format!("blake3:{}", "0".repeat(64)));
    connection
        .execute(
            "UPDATE applied_transactions SET request_digest=?1 WHERE transaction_id=?2",
            rusqlite::params![format!("blake3:{}", "0".repeat(64)), intent.transaction_id,],
        )
        .unwrap();
    drop(connection);
    drop(committed);
    assert!(open(false).is_err());
    assert!(
        VfsRecovery::acquire_bounded(
            &store_root,
            "foreign-root",
            &metadata,
            &store_id,
            &worktree_id,
            false,
            8
        )
        .is_err()
    );
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn journal_context_preserves_native_error_kind_and_source() {
    let error = Err::<(), _>(io::Error::from_raw_os_error(5))
        .journal_context("reopen private container")
        .unwrap_err();
    assert_eq!(error.kind(), io::Error::from_raw_os_error(5).kind());
    assert!(error.to_string().contains("reopen private container"));
    let context = error
        .get_ref()
        .unwrap()
        .downcast_ref::<JournalIoError>()
        .unwrap();
    assert_eq!(context.source.raw_os_error(), Some(5));
}

#[test]
fn metadata_read_waits_for_typed_writer_contention_and_rechecks_identity() {
    let root = std::env::temp_dir().join(format!(
        "layrs-vfs-contention-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let store_root = root.join("store");
    let store_id = StoreId::parse("store:vfs-contention-test").unwrap();
    drop(super::super::init(&store_root, store_id.clone()).unwrap());
    let identity = PinnedDirectory::open(&store_root).unwrap().identity_token();
    let metadata = PinnedDirectory::open(&store_root.join("metadata"))
        .unwrap()
        .identity_token();
    let writer = super::super::open_exclusive(&store_root).unwrap();
    let (contended_tx, contended_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let (reader_root, reader_identity, reader_metadata, reader_store_id) =
            (&store_root, &identity, &metadata, &store_id);
        let reader = scope.spawn(move || {
            let store =
                open_bound_store_wait(reader_root, reader_identity, reader_metadata, &mut || {
                    contended_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                })
                .unwrap();
            assert_eq!(store.store_id(), reader_store_id);
            store.verify_filesystem_identity().unwrap();
        });
        // This handshake fires only after the actual typed failed open.
        contended_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(writer);
        release_tx.send(()).unwrap();
        reader.join().unwrap();
    });
    let mut retried = false;
    assert!(
        open_bound_store_wait(&store_root, "wrong-root", &metadata, &mut || {
            retried = true;
        })
        .is_err()
    );
    assert!(!retried, "identity failures must not be retried");
    assert!(
        open_bound_store_wait(&store_root, &identity, "wrong-metadata", &mut || {
            retried = true;
        })
        .is_err()
    );
    assert!(!retried);
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn creation_cannot_skip_population_or_publish_ready_early() {
    assert!(transition(None, &VfsEvent::Ready).is_err());
    let mut state = transition(None, &VfsEvent::Reserved { locator: vec![1] }).unwrap();
    assert!(transition(Some(&state), &VfsEvent::Ready).is_err());
    for event in [
        VfsEvent::Registered,
        VfsEvent::ProviderPrepared { ticket: vec![2] },
        VfsEvent::ProviderPopulated,
        VfsEvent::ControlPrepared { control: vec![3] },
        VfsEvent::LocatorPublished { control: vec![3] },
        VfsEvent::Ready,
    ] {
        state = transition(Some(&state), &event).unwrap();
        if state.phase == VfsPhase::ControlPrepared {
            assert!(
                transition(
                    Some(&state),
                    &VfsEvent::LocatorPublished { control: vec![4] }
                )
                .is_err(),
                "publication must retain the prepared native identity"
            );
        }
    }
    assert_eq!(state.phase, VfsPhase::Ready);
    assert_eq!(state.ticket, Some(vec![2]));
    assert!(transition(Some(&state), &VfsEvent::ProviderPopulated).is_err());
    assert!(
        transition(
            Some(&state),
            &VfsEvent::CaptureResolved {
                transaction_id: "foreign".into(),
                accepted: true
            }
        )
        .is_err()
    );
}

#[test]
fn short_staged_record_never_becomes_authority_and_host_lease_is_exclusive() {
    struct Short;
    impl VfsJournalFaults for Short {
        fn write_staged(&self, output: &mut dyn Write, bytes: &[u8]) -> io::Result<()> {
            output.write_all(&bytes[..bytes.len() / 2])?;
            Err(io::Error::other("injected short journal write"))
        }
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("layrs-vfs-journal-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let store_root = root.join("store");
    let store_id = StoreId::parse(format!("store:journal-{nonce}")).unwrap();
    let worktree_id = WorktreeId::parse(format!("worktree:journal-{nonce}")).unwrap();
    drop(super::super::init(&store_root, store_id.clone()).unwrap());
    let identity = PinnedDirectory::open(&store_root).unwrap().identity_token();
    let metadata = PinnedDirectory::open(&store_root.join("metadata"))
        .unwrap()
        .identity_token();
    assert!(
        VfsRecovery::create(
            &store_root,
            &identity,
            &metadata,
            &store_id,
            &worktree_id,
            vec![1, 2],
            &Short
        )
        .is_err()
    );
    assert!(VfsRecovery::open(&store_root, &identity, &metadata, &store_id, &worktree_id).is_err());
    let journal = VfsRecovery::create(
        &store_root,
        &identity,
        &metadata,
        &store_id,
        &worktree_id,
        vec![3, 4],
        &NoVfsJournalFaults,
    )
    .unwrap();
    assert!(VfsRecovery::open(&store_root, &identity, &metadata, &store_id, &worktree_id).is_err());
    drop(journal);
    let mut journal =
        VfsRecovery::open(&store_root, &identity, &metadata, &store_id, &worktree_id).unwrap();
    assert_eq!(journal.state().locator, vec![3, 4]);
    journal
        .append(VfsEvent::Registered, &NoVfsJournalFaults)
        .unwrap();
    let endpoint_id = "00000000-0000-0000-0000-000000000001";
    journal
        .publish_endpoint(endpoint_id, b"private endpoint test")
        .unwrap();
    assert_eq!(
        VfsRecovery::read_endpoint(
            &store_root,
            &identity,
            &metadata,
            &store_id,
            &worktree_id,
            endpoint_id,
        )
        .unwrap(),
        b"private endpoint test"
    );
    assert!(
        journal
            .append(
                VfsEvent::CaptureResolved {
                    transaction_id: "forged".into(),
                    accepted: true
                },
                &NoVfsJournalFaults
            )
            .is_err()
    );
    drop(journal);
    let record = store_root
        .join("metadata/vfs-v1")
        .join(
            blake3::hash(worktree_id.as_str().as_bytes())
                .to_hex()
                .as_str(),
        )
        .join("00000000000.json");
    std::fs::write(record, b"broken journal").unwrap();
    assert!(VfsRecovery::open(&store_root, &identity, &metadata, &store_id, &worktree_id).is_err());
    // Only this test's unique disposable root is removed; never a provider root.
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    std::fs::remove_dir_all(root).unwrap();
}
