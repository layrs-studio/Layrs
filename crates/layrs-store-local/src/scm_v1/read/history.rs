//! Reconstructible observation projection. SQLite insertion order is only a
//! local snapshot boundary, never a Step timestamp or a portable ordering.
use super::*;
use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservationSnapshot {
    pub space_id: SpaceId,
    pub watermark: i64,
    pub count: usize,
    pub digest: String,
}

pub struct ObservationProjection {
    pub snapshot: ObservationSnapshot,
    pub by_step: BTreeMap<StepId, Vec<StepObservation>>,
}

impl NativeScmReader<'_> {
    /// Rebuild from immutable records and validate every SQL projection. A
    /// resumed snapshot excludes even backdated observations appended later.
    /// Invalidated snapshots (including a rowid-renumbering maintenance) fail
    /// closed; callers must start a new traversal explicitly.
    pub fn observation_projection(
        &self,
        space: &SpaceId,
        snapshot: Option<&ObservationSnapshot>,
    ) -> Result<ObservationProjection> {
        if snapshot.is_some_and(|s| s.space_id != *space || s.watermark < 0 || s.count > 100_000) {
            return Err(corruption("invalid observation snapshot scope or bounds"));
        }
        // Release the connection guard before using the typed reader again.
        let (watermark, rows) = {
            let connection = self.connection()?;
            let watermark = match snapshot {
                Some(s) => s.watermark,
                None => connection
                    .query_row(
                        "SELECT coalesce(max(rowid),0) FROM step_observations WHERE space_id=?1",
                        [space.as_str()],
                        |r| r.get(0),
                    )
                    .map_err(sqlite_backend)?,
            };
            // Bound allocation before loading even IDs: damaged rows can carry
            // unbounded strings despite the typed identity contract.
            let (count, bytes): (i64, i64) = connection.query_row(
                "SELECT count(*),coalesce(sum(bytes),0) FROM ( \
                 SELECT length(CAST(payload_json AS BLOB))+length(CAST(observation_id AS BLOB)) AS bytes \
                 FROM step_observations WHERE space_id=?1 AND rowid<=?2 LIMIT 100001)",
                params![space.as_str(), watermark], |row| Ok((row.get(0)?,row.get(1)?)),
            ).map_err(sqlite_backend)?;
            if count > 100_000 || !(0..=64 * 1024 * 1024).contains(&bytes) {
                return Err(StoreError::AdmissionBudgetExceeded {
                    resource: "history observations",
                    limit: 100_000,
                });
            }
            let mut statement = connection.prepare(
                "SELECT rowid, observation_id, length(CAST(payload_json AS BLOB)) \
                 FROM step_observations WHERE space_id=?1 AND rowid<=?2 ORDER BY rowid LIMIT 100001",
            ).map_err(sqlite_backend)?;
            let rows = statement
                .query_map(params![space.as_str(), watermark], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .map_err(sqlite_backend)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(sqlite_backend)?;
            (watermark, rows)
        };
        if rows.len() > 100_000
            || rows
                .iter()
                .try_fold(0usize, |sum, (_, _, size)| {
                    usize::try_from(*size)
                        .ok()
                        .and_then(|size| sum.checked_add(size))
                })
                .is_none_or(|bytes| bytes > 64 * 1024 * 1024)
        {
            return Err(StoreError::AdmissionBudgetExceeded {
                resource: "history observations",
                limit: 100_000,
            });
        }
        let mut digest = blake3::Hasher::new();
        digest.update(b"layrs-local-observation-snapshot-v1\0");
        digest.update(space.as_str().as_bytes());
        let mut by_step: BTreeMap<StepId, Vec<StepObservation>> = BTreeMap::new();
        for (rowid, id, _) in &rows {
            let id = ObservationId::parse(id).map_err(|_| corruption("invalid observation id"))?;
            let record = self
                .get_step_observation(&id)?
                .ok_or_else(|| corruption("missing observation"))?;
            if record.id() != &id
                || record.space_id() != Some(space)
                || !self.object_admitted_to_space(
                    &StoreObjectId::Step(record.step_id().clone()),
                    space,
                )?
            {
                return Err(corruption("observation projection scope mismatch"));
            }
            let bytes = canonical_json(&record).map_err(|e| corruption(e.to_string()))?;
            digest.update(&rowid.to_le_bytes());
            digest.update(&(bytes.len() as u64).to_le_bytes());
            digest.update(&bytes);
            by_step
                .entry(record.step_id().clone())
                .or_default()
                .push(record);
        }
        let rebuilt = ObservationSnapshot {
            space_id: space.clone(),
            watermark,
            count: rows.len(),
            digest: digest.finalize().to_hex().to_string(),
        };
        if snapshot.is_some_and(|expected| expected != &rebuilt) {
            return Err(corruption(
                "observation snapshot changed; restart history query",
            ));
        }
        Ok(ObservationProjection {
            snapshot: rebuilt,
            by_step,
        })
    }
}
