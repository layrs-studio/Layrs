//! A durable local request journal. Network authority is always revalidated by
//! the client/server; opaque JSON here is not an authorization capability.
mod storage;
use super::*;
use layrs_core::{
    projection::EntryState,
    scm::{TreeEntryTarget, canonical_json},
};
use rusqlite::params;

const MAX_BYTES: usize = 24 * 1024 * 1024;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingPublication {
    pub sequence: u64,
    pub projection_generation: u64,
    pub projection_digest: String,
    pub capture_digest: String,
    pub request_digest: String,
    pub request_bytes: Vec<u8>,
    pub receipt_bytes: Option<Vec<u8>>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletedPublication {
    pub sequence: u64,
    pub receipt_bytes: Vec<u8>,
    pub projection: VersionedProjection,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationPoint {
    RequestDurable,
    BeforeReceiptCommit,
    ReceiptDurable,
}
pub trait PublicationFaults {
    fn check(&self, point: PublicationPoint) -> io::Result<()>;
}
pub struct NoPublicationFaults;
impl PublicationFaults for NoPublicationFaults {
    fn check(&self, _: PublicationPoint) -> io::Result<()> {
        Ok(())
    }
}

impl ProjectionStore {
    pub fn completed_publication(&self, sequence: u64) -> Result<Option<CompletedPublication>> {
        let access = format::access(self, SessionMode::Shared)?;
        database::require_idle(&access.connection)?;
        let result =
            storage::completed_record(&access.connection, sequence)?.map(|(record, _)| record);
        access.verify()?;
        Ok(result)
    }
    pub fn begin_publication(
        &self,
        sequence: u64,
        expected_generation: u64,
        request_bytes: &[u8],
    ) -> Result<PendingPublication> {
        self.begin_publication_with_faults(
            sequence,
            expected_generation,
            request_bytes,
            &NoPublicationFaults,
        )
    }
    pub fn begin_publication_with_faults(
        &self,
        sequence: u64,
        expected_generation: u64,
        request_bytes: &[u8],
        faults: &dyn PublicationFaults,
    ) -> Result<PendingPublication> {
        let request_digest = storage::json_digest(b"layrs-projection-request-v1\0", request_bytes)?;
        let mut access = format::access(self, SessionMode::Exclusive)?;
        database::require_idle(&access.connection)?;
        let current = database::current(&access.connection)?;
        let patch = super::capture::storage::load(&access.connection, self, sequence)?;
        let pending = PendingPublication {
            sequence,
            projection_generation: expected_generation,
            projection_digest: current.digest.clone(),
            capture_digest: patch.digest.clone(),
            request_digest,
            request_bytes: request_bytes.to_vec(),
            receipt_bytes: None,
        };
        if let Some(existing) = storage::pending(&access.connection, self)? {
            let mut comparable = existing.clone();
            comparable.receipt_bytes = None;
            if comparable == pending {
                return Ok(existing);
            }
            return Err(invalid("a different publication is already pending"));
        }
        if current.generation != expected_generation
            || patch.projection_generation != current.generation
            || patch.projection_digest != current.digest
        {
            return Err(invalid("publication capture or projection is stale"));
        }
        if patch.changes.is_empty() {
            return Err(invalid("cannot publish an empty capture"));
        }
        let completed: bool = access.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM projection_publications WHERE sequence=?1)",
            [sequence],
            |r| r.get(0),
        )?;
        if completed {
            return Err(invalid("capture already has a completed publication"));
        }
        access.verify()?;
        let tx = access
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO projection_publication_pending VALUES(1,?1,?2,?3,?4,?5,?6,NULL,NULL)",
            params![
                sequence,
                expected_generation,
                pending.projection_digest,
                pending.capture_digest,
                pending.request_digest,
                request_bytes
            ],
        )?;
        tx.commit()?;
        faults.check(PublicationPoint::RequestDurable)?;
        access.verify()?;
        Ok(pending)
    }
    pub fn pending_publication(&self) -> Result<Option<PendingPublication>> {
        let access = format::access(self, SessionMode::Shared)?;
        database::require_idle(&access.connection)?;
        let result = storage::pending(&access.connection, self)?;
        access.verify()?;
        Ok(result)
    }
    /// Preserve a terminal receipt even if fetching its new projection fails.
    pub fn record_publication_receipt(
        &self,
        sequence: u64,
        receipt_bytes: &[u8],
    ) -> Result<PendingPublication> {
        let digest = storage::json_digest(b"layrs-projection-receipt-v1\0", receipt_bytes)?;
        let mut access = format::access(self, SessionMode::Exclusive)?;
        database::require_idle(&access.connection)?;
        let mut pending = storage::pending(&access.connection, self)?
            .filter(|p| p.sequence == sequence)
            .ok_or_else(|| invalid("matching publication is not pending"))?;
        if let Some(existing) = &pending.receipt_bytes {
            if existing != receipt_bytes {
                return Err(invalid("publication already has a different receipt"));
            }
            return Ok(pending);
        }
        access.verify()?;
        let tx = access
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("UPDATE projection_publication_pending SET receipt_digest=?1,receipt_bytes=?2 WHERE sequence=?3",params![digest,receipt_bytes,sequence])?;
        tx.commit()?;
        access.verify()?;
        pending.receipt_bytes = Some(receipt_bytes.to_vec());
        Ok(pending)
    }
    /// `next` is supplied only after the client verifies an Applied server
    /// receipt and its complete projected namespace. Rejection leaves the base.
    pub fn finish_publication(
        &self,
        sequence: u64,
        receipt_bytes: &[u8],
        next: Option<&ProjectionSnapshot>,
    ) -> Result<VersionedProjection> {
        self.finish_publication_with_faults(sequence, receipt_bytes, next, &NoPublicationFaults)
    }
    pub fn finish_publication_with_faults(
        &self,
        sequence: u64,
        receipt_bytes: &[u8],
        next: Option<&ProjectionSnapshot>,
        faults: &dyn PublicationFaults,
    ) -> Result<VersionedProjection> {
        let receipt_digest = storage::json_digest(b"layrs-projection-receipt-v1\0", receipt_bytes)?;
        let next_bytes = next.map(ProjectionSnapshot::canonical_bytes).transpose()?;
        let mut access = format::access(self, SessionMode::Exclusive)?;
        database::require_idle(&access.connection)?;
        if let Some(previous) = storage::completed(
            &access.connection,
            sequence,
            receipt_bytes,
            next_bytes.as_deref(),
        )? {
            access.verify()?;
            return Ok(previous);
        }
        let pending = storage::pending(&access.connection, self)?
            .filter(|p| p.sequence == sequence)
            .ok_or_else(|| invalid("matching publication is not pending"))?;
        if pending
            .receipt_bytes
            .as_deref()
            .is_some_and(|bytes| bytes != receipt_bytes)
        {
            return Err(invalid("publication finish differs from recorded receipt"));
        }
        let current = database::current(&access.connection)?;
        if current.generation != pending.projection_generation
            || current.digest != pending.projection_digest
        {
            return Err(invalid("pending publication projection changed"));
        }
        let patch = super::capture::storage::load(&access.connection, self, sequence)?;
        if let Some(next) = next {
            validate_applied(&current.snapshot, next, &patch)?;
        }
        let after = if let Some(next) = next {
            let generation = current
                .generation
                .checked_add(1)
                .filter(|n| *n <= i64::MAX as u64)
                .ok_or_else(|| invalid("projection generation exhausted"))?;
            VersionedProjection {
                generation,
                digest: database::digest(
                    generation,
                    Some(&current.digest),
                    next_bytes.as_ref().expect("next bytes present"),
                ),
                snapshot: next.clone(),
            }
        } else {
            current.clone()
        };
        access.verify()?;
        let tx = access
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(bytes) = &next_bytes {
            tx.execute(
                "INSERT INTO projection_snapshots VALUES(?1,?2,?3,?4)",
                params![after.generation, after.digest, current.digest, bytes],
            )?;
            tx.execute(
                "UPDATE projection_head SET generation=?1 WHERE singleton=1",
                [after.generation],
            )?;
        }
        tx.execute(
            "INSERT INTO projection_publications VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                sequence,
                pending.request_digest,
                receipt_digest,
                receipt_bytes,
                next_bytes,
                after.generation,
                after.digest,
                canonical_json(&after).map_err(invalid)?
            ],
        )?;
        tx.execute(
            "DELETE FROM projection_publication_pending WHERE singleton=1",
            [],
        )?;
        faults.check(PublicationPoint::BeforeReceiptCommit)?;
        tx.commit()?;
        faults.check(PublicationPoint::ReceiptDurable)?;
        access.verify()?;
        Ok(after)
    }
}

fn validate_applied(
    before: &ProjectionSnapshot,
    after: &ProjectionSnapshot,
    patch: &CapturedPatch,
) -> Result<()> {
    after.validate()?;
    let mut expected = before.clone();
    expected.base_step_id = after.base_step_id.clone();
    expected.base_root_tree_id = after.base_root_tree_id.clone();
    for change in &patch.changes {
        let index = expected
            .entries
            .binary_search_by(|e| e.path.cmp(&change.path))
            .map_err(|_| invalid("published capture path disappeared"))?;
        let entry = &mut expected.entries[index];
        match &change.replacement {
            Some(file) => {
                entry.target = Some(TreeEntryTarget::file(file.file_id.clone()));
                entry.executable = file.executable;
                entry.state = EntryState::Materialized;
            }
            None => {
                entry.target = None;
                entry.executable = false;
                entry.state = EntryState::Absent;
            }
        }
    }
    // Tree IDs on retained structural entries may change with descendant patches;
    // paths, states, modes and observed permissions still must remain exact.
    if after.entries.len() != expected.entries.len() {
        return Err(invalid("publication changed the observed namespace"));
    }
    for (old, new) in expected.entries.iter_mut().zip(&after.entries) {
        if matches!(old.target, Some(TreeEntryTarget::Tree { .. }))
            && matches!(new.target, Some(TreeEntryTarget::Tree { .. }))
        {
            old.target = new.target.clone();
        }
    }
    if expected != *after {
        return Err(invalid(
            "publication snapshot changes uncaptured or protected entries",
        ));
    }
    Ok(())
}
