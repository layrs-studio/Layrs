//! A fetch plan is immutable after finalization. Resolve its entire bounded
//! snapshot once before persisting it; pages are projections of that snapshot.
use super::*;
use async_trait::async_trait;
use layrs_core::scm::SpaceId;
use std::{collections::BTreeMap, sync::Mutex};

pub async fn build_fetch_snapshot_page(
    availability: &dyn RootedFetchAvailability,
    capabilities: &CapabilityCodec,
    limits: PlanLimits,
    context: &PlanContext,
    claims: Vec<FetchClaim>,
    page_cursor: Option<&str>,
    requested_max_objects: usize,
) -> V3StoreResult<(FetchPlan, FetchPlan)> {
    let cache = SnapshotAvailability {
        source: availability,
        entries: Mutex::new(SnapshotEntries::default()),
        max_objects: limits.max_page_objects,
    };
    let builder = FetchPlanBuilder::new(&cache, capabilities, limits);
    // Verify a supplied cursor before doing any rooted resolution. The cache
    // makes page/full-snapshot construction share each exact claim decision.
    let page = builder
        .build(context, claims.clone(), page_cursor, requested_max_objects)
        .await?;
    let snapshot = if page_cursor.is_none() && !page.has_more {
        page.clone()
    } else {
        builder
            .build(context, claims, None, limits.max_page_objects)
            .await?
    };
    if snapshot.has_more {
        return Err(V3StoreError::LimitExceeded {
            resource: "complete fetch snapshot objects",
            limit: limits.max_page_objects as u64,
        });
    }
    Ok((snapshot, page))
}

#[derive(Default)]
struct SnapshotEntries {
    values: BTreeMap<Vec<u8>, ClaimResolution>,
    objects: usize,
}
struct SnapshotAvailability<'a> {
    source: &'a dyn RootedFetchAvailability,
    entries: Mutex<SnapshotEntries>,
    max_objects: usize,
}
#[async_trait]
impl RootedFetchAvailability for SnapshotAvailability<'_> {
    async fn resolve(&self, space: &SpaceId, claim: &FetchClaim) -> V3StoreResult<ClaimResolution> {
        let key = serde_json::to_vec(claim).map_err(|e| V3StoreError::Backend(e.to_string()))?;
        if let Some(value) = self
            .entries
            .lock()
            .map_err(|_| V3StoreError::Backend("fetch snapshot lock poisoned".into()))?
            .values
            .get(&key)
            .cloned()
        {
            return Ok(value);
        }
        let value = self.source.resolve(space, claim).await?;
        if matches!(&value,ClaimResolution::Available{descriptors,..} if descriptors.len()>self.max_objects)
        {
            return Err(V3StoreError::LimitExceeded {
                resource: "single fetch claim expansion",
                limit: self.max_objects as u64,
            });
        }
        let cost = match &value {
            ClaimResolution::Available { descriptors, .. } => descriptors.len().max(1),
            ClaimResolution::Denied(_) => 1,
        };
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| V3StoreError::Backend("fetch snapshot lock poisoned".into()))?;
        if entries
            .objects
            .checked_add(cost)
            .is_none_or(|count| count > self.max_objects)
        {
            return Err(V3StoreError::LimitExceeded {
                resource: "complete fetch snapshot objects",
                limit: self.max_objects as u64,
            });
        }
        entries.objects += cost;
        entries.values.insert(key, value.clone());
        Ok(value)
    }
}
