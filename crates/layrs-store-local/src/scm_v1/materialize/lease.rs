//! Cooperative lease renewal during bounded filesystem work. Expired or stolen
//! leases are never reacquired here: the pending journal must be recovered.
use super::*;

pub(super) const LEASE_MS: i64 = 300_000;
const RENEW_EVERY_MS: i64 = 30_000;

pub(super) struct LeaseKeeper<'a> {
    root: &'a Path,
    store_directory: &'a PinnedDirectory,
    metadata_directory: &'a PinnedDirectory,
    id: &'a RecoveryId,
    owner: &'a str,
    pub leases: Vec<ResourceLease>,
    faults: &'a dyn MaterializationFaults,
    last_ms: i64,
    /// Last implicit identity recheck by `tick()`; `force()` always rechecks.
    verified_at: Option<std::time::Instant>,
    /// The materialization holds the exclusive Store capability (council
    /// s10, Q1): nobody can take the Store, so leases are not renewed (a
    /// renewal would open the Store again); their expiry is still checked
    /// against the time the Store was taken, at the metadata commit.
    holding: bool,
}

/// Progress ticks run per scanned entry and chunk. Rebinding checks there are
/// throttled like `NativeStoreOpen::connection()`. Unconditional Store and
/// metadata checks remain before staging, in `force()` before metadata, in the
/// 30 s renewal's bound exclusive open and at the batch commit.
const TICK_IDENTITY_RECHECK: std::time::Duration = std::time::Duration::from_millis(250);
impl<'a> LeaseKeeper<'a> {
    pub fn new(
        root: &'a Path,
        store_directory: &'a PinnedDirectory,
        metadata_directory: &'a PinnedDirectory,
        id: &'a RecoveryId,
        owner: &'a str,
        leases: Vec<ResourceLease>,
        faults: &'a dyn MaterializationFaults,
    ) -> io::Result<Self> {
        let last_ms = leases
            .iter()
            .map(|lease| lease.expires_at_ms - LEASE_MS)
            .min()
            .ok_or_else(|| error("materialization has no resource leases"))?;
        Ok(Self {
            root,
            store_directory,
            metadata_directory,
            id,
            owner,
            leases,
            faults,
            last_ms,
            verified_at: None,
            holding: false,
        })
    }
    /// See `holding`.
    pub fn holding_store(mut self, holding: bool) -> Self {
        self.holding = holding;
        self
    }
    pub fn tick(&mut self) -> io::Result<()> {
        self.renew(false)
    }
    pub fn force(&mut self) -> io::Result<()> {
        self.renew(true)
    }
    fn renew(&mut self, force: bool) -> io::Result<()> {
        if force
            || self
                .verified_at
                .is_none_or(|at| at.elapsed() >= TICK_IDENTITY_RECHECK)
        {
            self.store_directory.verify_path_identity()?;
            self.metadata_directory.verify_path_identity()?;
            self.verified_at = Some(std::time::Instant::now());
        }
        let now = self.faults.lease_time_ms()?;
        // A backwards wall-clock correction does not expire an existing lease.
        // Avoid an O(paths²) renewal storm while time catches up.
        if !self.holding && (force || now.saturating_sub(self.last_ms) >= RENEW_EVERY_MS) {
            let mut store = open_exclusive(self.root)
                .map_err(error)?
                .with_filesystem_identity(&self.store_directory.identity_token())?
                .with_metadata_identity(&self.metadata_directory.identity_token())?;
            self.leases = NativeRecovery::new(&mut store)
                .map_err(error)?
                .heartbeat(self.id, self.owner, &self.leases, now, LEASE_MS)
                .map_err(error)?;
            self.last_ms = self
                .leases
                .iter()
                .map(|lease| lease.expires_at_ms - LEASE_MS)
                .min()
                .ok_or_else(|| error("materialization lost resource leases"))?;
        }
        Ok(())
    }
}
