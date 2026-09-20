//! Recovery state transitions within the single native writer.
use super::*;
impl<'store> NativeRecovery<'store> {
    pub fn new(store: &'store mut NativeStoreExclusive) -> RecoveryResult<Self> {
        Self::with_fault_injector(store, Arc::new(NoFaults))
    }

    pub fn with_fault_injector(
        store: &'store mut NativeStoreExclusive,
        faults: Arc<dyn RecoveryFaultInjector>,
    ) -> RecoveryResult<Self> {
        let connection = store.begin_write_connection()?;
        let cas = NativeCas::open_exclusive(store)?;
        Ok(Self {
            connection,
            cas,
            faults,
        })
    }

    pub fn create(&mut self, plan: &RecoveryPlan) -> RecoveryResult<MutationOutcome> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<(String, String)> = transaction
            .query_row(
                "SELECT recovery_operation_id, request_digest FROM recovery_operations \
                 WHERE idempotency_key = ?1 OR recovery_operation_id = ?2",
                params![plan.idempotency_key, plan.id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((id, digest)) = existing {
            if id == plan.id.as_str() && digest == plan.request_digest {
                transaction.commit()?;
                self.hit(RecoveryFaultPoint::AfterCreateCommit)?;
                return Ok(MutationOutcome::Replayed);
            }
            return Err(RecoveryError::ReplayMismatch {
                idempotency_key: plan.idempotency_key.clone(),
            });
        }
        transaction.execute(
            "INSERT INTO recovery_operations(
                recovery_operation_id, idempotency_key, request_digest, worktree_id,
                operation_id, recovery_type, state, payload_json, evidence_json,
                attempt_count, owner_token, last_error, created_at_ms, updated_at_ms,
                completed_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, '{}', 0, NULL, NULL, ?8, ?8, NULL)",
            params![
                plan.id.as_str(),
                plan.idempotency_key,
                plan.request_digest,
                plan.worktree_id.as_str(),
                plan.operation_id.as_ref().map(OperationId::as_str),
                plan.recovery_type.as_str(),
                plan.payload_json,
                plan.created_at_ms,
            ],
        )?;
        for path in &plan.paths {
            transaction.execute(
                "INSERT INTO recovery_paths(
                    recovery_operation_id, path, path_role, expected_object_id,
                    evidence_json, state
                 ) VALUES (?1, ?2, ?3, ?4, '{}', 'pending')",
                params![
                    plan.id.as_str(),
                    path.path,
                    path.role.as_str(),
                    path.expected_object_id,
                ],
            )?;
        }
        transaction.commit()?;
        self.hit(RecoveryFaultPoint::AfterCreateCommit)?;
        Ok(MutationOutcome::Applied)
    }

    pub fn transition(
        &mut self,
        id: &RecoveryId,
        transition: RecoveryTransition,
        now_ms: i64,
    ) -> RecoveryResult<MutationOutcome> {
        match transition {
            RecoveryTransition::Prepare(preparation) => self.prepare(id, &preparation, now_ms),
            RecoveryTransition::Complete {
                owner_token,
                expected_leases,
                evidence,
                path_results,
            } => self.complete(
                id,
                &owner_token,
                &expected_leases,
                evidence,
                &path_results,
                now_ms,
            ),
            RecoveryTransition::Fail {
                owner_token,
                expected_leases,
                evidence,
                error,
                path_results,
            } => self.fail(
                id,
                &owner_token,
                &expected_leases,
                evidence,
                error,
                &path_results,
                now_ms,
            ),
        }
    }

    pub fn prepare(
        &mut self,
        id: &RecoveryId,
        preparation: &RecoveryPreparation,
        updated_at_ms: i64,
    ) -> RecoveryResult<MutationOutcome> {
        valid_time("updated_at_ms", updated_at_ms)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = load_record(&transaction, id)?;
        let updated_at_ms = ordered_time(&transaction, &current, updated_at_ms)?;
        if current.state == RecoveryState::Prepared {
            if canonical_text(&current.evidence)? == preparation.evidence_json
                && path_evidence_matches(&current.paths, &preparation.path_evidence)
            {
                transaction.commit()?;
                self.hit(RecoveryFaultPoint::AfterPrepareCommit)?;
                return Ok(MutationOutcome::Replayed);
            }
            return Err(RecoveryError::InvalidTransition {
                from: current.state,
                to: RecoveryState::Prepared,
            });
        }
        if current.state == RecoveryState::Failed {
            let evidence: Value = serde_json::from_str(&preparation.evidence_json)
                .map_err(|error| RecoveryError::Corruption(error.to_string()))?;
            if evidence.get("retryable").and_then(Value::as_bool) != Some(true) {
                return Err(RecoveryError::MissingEvidence(id.clone()));
            }
        } else if current.state != RecoveryState::Pending {
            return Err(RecoveryError::InvalidTransition {
                from: current.state,
                to: RecoveryState::Prepared,
            });
        }
        require_exact_path_evidence(&current.paths, &preparation.path_evidence, id)?;
        for evidence in &preparation.path_evidence {
            transaction.execute(
                "UPDATE recovery_paths SET evidence_json = ?1, state = 'verified' \
                 WHERE recovery_operation_id = ?2 AND path = ?3 AND path_role = ?4",
                params![
                    evidence.canonical_json,
                    id.as_str(),
                    evidence.path,
                    evidence.role.as_str(),
                ],
            )?;
        }
        transaction.execute(
            "UPDATE recovery_operations SET state = 'prepared', evidence_json = ?1,
             owner_token = NULL, last_error = NULL, updated_at_ms = ?2,
             completed_at_ms = NULL WHERE recovery_operation_id = ?3",
            params![preparation.evidence_json, updated_at_ms, id.as_str()],
        )?;
        transaction.commit()?;
        self.hit(RecoveryFaultPoint::AfterPrepareCommit)?;
        Ok(MutationOutcome::Applied)
    }

    pub fn claim(
        &mut self,
        id: &RecoveryId,
        owner_token: impl Into<String>,
        now_ms: i64,
        lease_duration_ms: i64,
    ) -> RecoveryResult<ClaimOutcome> {
        let owner_token = validated_token("owner_token", owner_token.into())?;
        valid_time("now_ms", now_ms)?;
        if lease_duration_ms <= 0 {
            return Err(RecoveryError::InvalidInput {
                field: "lease_duration_ms",
                reason: "must be positive".into(),
            });
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = load_record(&transaction, id)?;
        let now_ms = ordered_time(&transaction, &current, now_ms)?;
        let expires_at_ms =
            now_ms
                .checked_add(lease_duration_ms)
                .ok_or_else(|| RecoveryError::InvalidInput {
                    field: "lease_duration_ms",
                    reason: "expiry overflows SQLite INTEGER".into(),
                })?;
        if current.state == RecoveryState::Applying
            && current.owner_token.as_deref() == Some(owner_token.as_str())
            && leases_are_active(&transaction, &current, &owner_token, now_ms)?
        {
            let leases = load_owner_leases(&transaction, &current, &owner_token)?;
            transaction.commit()?;
            self.hit(RecoveryFaultPoint::AfterClaimCommit)?;
            return Ok(ClaimOutcome {
                outcome: MutationOutcome::Replayed,
                leases,
            });
        }
        if !matches!(
            current.state,
            RecoveryState::Prepared | RecoveryState::Applying
        ) {
            return Err(RecoveryError::InvalidTransition {
                from: current.state,
                to: RecoveryState::Applying,
            });
        }
        let resources = resource_keys(&current);
        let mut leases = Vec::new();
        for resource_key in resources {
            let existing: Option<(String, String, u64, i64, Option<i64>)> = transaction
                .query_row(
                    "SELECT recovery_operation_id, owner_token, generation, expires_at_ms, released_at_ms FROM materialization_leases \
                     WHERE worktree_id = ?1 AND resource_key = ?2",
                    params![current.worktree_id.as_str(), resource_key],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .optional()?;
            let generation = match existing {
                Some((recovery_id, owner, _, expiry, None))
                    if (recovery_id != id.as_str() || owner != owner_token) && expiry > now_ms =>
                {
                    return Err(RecoveryError::LeaseFenced {
                        worktree_id: current.worktree_id.clone(),
                        resource_key,
                    });
                }
                Some((_, _, generation, _, _)) => generation.checked_add(1).ok_or_else(|| {
                    RecoveryError::Corruption("materialization lease generation overflow".into())
                })?,
                None => 1,
            };
            transaction.execute(
                "INSERT INTO materialization_leases(
                    worktree_id, resource_key, recovery_operation_id, owner_token, generation,
                    acquired_at_ms, heartbeat_at_ms, expires_at_ms, released_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, NULL)
                 ON CONFLICT(worktree_id, resource_key) DO UPDATE SET
                    recovery_operation_id = excluded.recovery_operation_id,
                    owner_token = excluded.owner_token,
                    generation = excluded.generation,
                    acquired_at_ms = excluded.acquired_at_ms,
                    heartbeat_at_ms = excluded.heartbeat_at_ms,
                    expires_at_ms = excluded.expires_at_ms,
                    released_at_ms = NULL",
                params![
                    current.worktree_id.as_str(),
                    resource_key,
                    id.as_str(),
                    owner_token,
                    generation,
                    now_ms,
                    expires_at_ms,
                ],
            )?;
            leases.push(ResourceLease {
                resource_key,
                generation,
                expires_at_ms,
            });
        }
        transaction.execute(
            "UPDATE recovery_operations SET state = 'applying', owner_token = ?1,
             attempt_count = attempt_count + 1, updated_at_ms = ?2
             WHERE recovery_operation_id = ?3",
            params![owner_token, now_ms, id.as_str()],
        )?;
        transaction.commit()?;
        self.hit(RecoveryFaultPoint::AfterClaimCommit)?;
        Ok(ClaimOutcome {
            outcome: MutationOutcome::Applied,
            leases,
        })
    }

    pub fn complete(
        &mut self,
        id: &RecoveryId,
        owner_token: &str,
        expected_leases: &[ResourceLease],
        evidence: Value,
        path_results: &[RecoveryPathResult],
        now_ms: i64,
    ) -> RecoveryResult<MutationOutcome> {
        self.terminal(
            id,
            owner_token,
            expected_leases,
            evidence,
            None,
            path_results,
            now_ms,
        )
    }

    /// Renews every resource lease only when the caller still owns the exact
    /// fencing generations returned by [`Self::claim`].
    pub fn heartbeat(
        &mut self,
        id: &RecoveryId,
        owner_token: &str,
        expected: &[ResourceLease],
        now_ms: i64,
        lease_duration_ms: i64,
    ) -> RecoveryResult<Vec<ResourceLease>> {
        let owner_token = validated_token("owner_token", owner_token.to_owned())?;
        valid_time("now_ms", now_ms)?;
        if lease_duration_ms <= 0 {
            return Err(RecoveryError::InvalidInput {
                field: "lease_duration_ms",
                reason: "must be positive".into(),
            });
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = load_record(&transaction, id)?;
        let now_ms = ordered_time(&transaction, &current, now_ms)?;
        let expires_at_ms =
            now_ms
                .checked_add(lease_duration_ms)
                .ok_or_else(|| RecoveryError::InvalidInput {
                    field: "lease_duration_ms",
                    reason: "expiry overflows SQLite INTEGER".into(),
                })?;
        if current.state != RecoveryState::Applying
            || current.owner_token.as_deref() != Some(owner_token.as_str())
        {
            return Err(RecoveryError::OwnerMismatch(id.clone()));
        }
        let expected_map: BTreeMap<_, _> = expected
            .iter()
            .map(|lease| (lease.resource_key.as_str(), lease.generation))
            .collect();
        let resources = resource_keys(&current);
        if expected_map.len() != expected.len()
            || expected_map.len() != resources.len()
            || resources
                .iter()
                .any(|resource| !expected_map.contains_key(resource.as_str()))
        {
            return Err(RecoveryError::OwnerMismatch(id.clone()));
        }
        let mut renewed = Vec::new();
        for resource in resources {
            let generation = expected_map[resource.as_str()];
            let changed = transaction.execute(
                "UPDATE materialization_leases SET heartbeat_at_ms = ?1, expires_at_ms = ?2
                 WHERE worktree_id = ?3 AND resource_key = ?4
                   AND recovery_operation_id = ?5 AND owner_token = ?6
                   AND generation = ?7 AND released_at_ms IS NULL AND expires_at_ms > ?1",
                params![
                    now_ms,
                    expires_at_ms,
                    current.worktree_id.as_str(),
                    resource,
                    id.as_str(),
                    owner_token,
                    generation,
                ],
            )?;
            if changed != 1 {
                return Err(RecoveryError::LeaseFenced {
                    worktree_id: current.worktree_id.clone(),
                    resource_key: resource,
                });
            }
            renewed.push(ResourceLease {
                resource_key: resource,
                generation,
                expires_at_ms,
            });
        }
        transaction.commit()?;
        Ok(renewed)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn fail(
        &mut self,
        id: &RecoveryId,
        owner_token: &str,
        expected_leases: &[ResourceLease],
        evidence: Value,
        error: impl Into<String>,
        path_results: &[RecoveryPathResult],
        now_ms: i64,
    ) -> RecoveryResult<MutationOutcome> {
        let error = error.into();
        if error.is_empty() {
            return Err(RecoveryError::InvalidInput {
                field: "error",
                reason: "must not be empty".into(),
            });
        }
        self.terminal(
            id,
            owner_token,
            expected_leases,
            evidence,
            Some(error),
            path_results,
            now_ms,
        )
    }

    pub fn list_pending(&self) -> RecoveryResult<Vec<RecoveryRecord>> {
        let connection = &self.connection;
        let mut statement = connection.prepare(
            "SELECT recovery_operation_id FROM recovery_operations \
             WHERE state <> 'completed' ORDER BY created_at_ms, recovery_operation_id",
        )?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| load_record(connection, &RecoveryId(id)))
            .collect()
    }

    pub fn recover(
        &mut self,
        owner_prefix: &str,
        now_ms: i64,
        lease_duration_ms: i64,
        handler: &mut dyn RecoveryHandler,
    ) -> RecoveryResult<RecoveryReport> {
        let owner_prefix = validated_token("owner_prefix", owner_prefix.to_owned())?;
        let records = self.list_pending()?;
        let mut report = RecoveryReport::default();
        for mut record in records {
            if record.state == RecoveryState::Pending {
                report.skipped += 1;
                report.finding(RecoveryFinding {
                    recovery_id: record.id,
                    code: RecoveryFindingCode::MissingEvidence,
                    message: "pending record has not been prepared with durable evidence".into(),
                });
                continue;
            }
            if record.state == RecoveryState::Failed
                && record.evidence.get("retryable").and_then(Value::as_bool) != Some(true)
            {
                report.skipped += 1;
                report.finding(RecoveryFinding {
                    recovery_id: record.id,
                    code: RecoveryFindingCode::FailedNotRetryable,
                    message: "failed record lacks retryable=true evidence".into(),
                });
                continue;
            }
            if record.state == RecoveryState::Failed {
                let path_evidence = record
                    .paths
                    .iter()
                    .map(|path| {
                        PathEvidence::new(path.path.clone(), path.role, path.evidence.clone())
                    })
                    .collect::<RecoveryResult<Vec<_>>>()?;
                let preparation = RecoveryPreparation::new(record.evidence.clone(), path_evidence)?;
                self.prepare(&record.id, &preparation, now_ms)?;
            }
            let owner = format!("{owner_prefix}:{}", record.id.as_str());
            let claim = match self.claim(&record.id, &owner, now_ms, lease_duration_ms) {
                Ok(claim) => claim,
                Err(RecoveryError::LeaseFenced { .. }) => {
                    report.skipped += 1;
                    report.finding(RecoveryFinding {
                        recovery_id: record.id,
                        code: RecoveryFindingCode::LeaseFenced,
                        message: "another live claimant owns a required resource".into(),
                    });
                    continue;
                }
                Err(error) => return Err(error),
            };
            record = load_record(&self.connection, &record.id)?;
            self.hit(RecoveryFaultPoint::BeforeHandler)?;
            let decision = handler.recover(&record);
            self.hit(RecoveryFaultPoint::AfterHandler)?;
            match decision {
                Ok(RecoveryDecision::Complete {
                    evidence,
                    path_results,
                }) => {
                    self.complete(
                        &record.id,
                        &owner,
                        &claim.leases,
                        evidence,
                        &path_results,
                        now_ms,
                    )?;
                    report.completed += 1;
                }
                Ok(RecoveryDecision::Fail {
                    evidence,
                    error,
                    path_results,
                }) => {
                    self.fail(
                        &record.id,
                        &owner,
                        &claim.leases,
                        evidence,
                        error,
                        &path_results,
                        now_ms,
                    )?;
                    report.failed += 1;
                }
                Ok(RecoveryDecision::RecoverCasStaging { mut evidence }) => {
                    if record.recovery_type != RecoveryType::CasStagingCleanup {
                        return Err(RecoveryError::Handler(
                            "CAS staging action requested for another recovery type".into(),
                        ));
                    }
                    self.hit(RecoveryFaultPoint::BeforeCasStagingRecovery)?;
                    let removed = self.cas.recover_staging()?;
                    self.hit(RecoveryFaultPoint::AfterCasStagingRecovery)?;
                    let object =
                        evidence
                            .as_object_mut()
                            .ok_or_else(|| RecoveryError::InvalidInput {
                                field: "evidence",
                                reason: "CAS recovery evidence must be an object".into(),
                            })?;
                    object.insert("removedStagingObjects".into(), removed.into());
                    self.complete(&record.id, &owner, &claim.leases, evidence, &[], now_ms)?;
                    report.completed += 1;
                }
                Err(error) => {
                    // An error is not evidence of a per-path terminal outcome.
                    // Leave the journal Applying and fenced until expiry so a
                    // later claimant must inspect durable state before retrying.
                    report.failed = report.failed.saturating_add(1);
                    report.finding(RecoveryFinding {
                        recovery_id: record.id,
                        code: RecoveryFindingCode::HandlerFailed,
                        message: bounded_finding_message(error.to_string()),
                    });
                }
            }
        }
        Ok(report)
    }

    #[allow(clippy::too_many_arguments)]
    fn terminal(
        &mut self,
        id: &RecoveryId,
        owner_token: &str,
        expected_leases: &[ResourceLease],
        evidence: Value,
        error: Option<String>,
        path_results: &[RecoveryPathResult],
        now_ms: i64,
    ) -> RecoveryResult<MutationOutcome> {
        let owner_token = validated_token("owner_token", owner_token.to_owned())?;
        valid_time("now_ms", now_ms)?;
        let evidence_json = required_evidence(evidence)?;
        let target = if error.is_some() {
            RecoveryState::Failed
        } else {
            RecoveryState::Completed
        };
        self.hit(RecoveryFaultPoint::BeforeTerminalCommit)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = load_record(&transaction, id)?;
        let now_ms = ordered_time(&transaction, &current, now_ms)?;
        require_exact_path_results(&current.paths, path_results, id)?;
        if current.state == target {
            let current_evidence = canonical_text(&current.evidence)?;
            if current_evidence == evidence_json
                && current.last_error == error
                && path_results_match(&current.paths, path_results)
            {
                transaction.commit()?;
                self.hit(RecoveryFaultPoint::AfterTerminalCommit)?;
                return Ok(MutationOutcome::Replayed);
            }
        }
        if current.state != RecoveryState::Applying {
            return Err(RecoveryError::InvalidTransition {
                from: current.state,
                to: target,
            });
        }
        if current.owner_token.as_deref() != Some(owner_token.as_str()) {
            return Err(RecoveryError::OwnerMismatch(id.clone()));
        }
        release_leases(
            &transaction,
            &current,
            &owner_token,
            expected_leases,
            now_ms,
        )?;
        persist_path_results(&transaction, id, path_results)?;
        let changed = transaction.execute(
            "UPDATE recovery_operations SET state = ?1, evidence_json = ?2,
             owner_token = NULL, last_error = ?3, updated_at_ms = ?4,
             completed_at_ms = ?5 WHERE recovery_operation_id = ?6
             AND state = 'applying' AND owner_token = ?7",
            params![
                target.as_str(),
                evidence_json,
                error,
                now_ms,
                (target == RecoveryState::Completed).then_some(now_ms),
                id.as_str(),
                owner_token,
            ],
        )?;
        if changed != 1 {
            return Err(RecoveryError::OwnerMismatch(id.clone()));
        }
        transaction.commit()?;
        self.hit(RecoveryFaultPoint::AfterTerminalCommit)?;
        Ok(MutationOutcome::Applied)
    }

    fn hit(&self, point: RecoveryFaultPoint) -> RecoveryResult<()> {
        self.faults
            .check(point)
            .map_err(|message| RecoveryError::InjectedFault { point, message })
    }
}
