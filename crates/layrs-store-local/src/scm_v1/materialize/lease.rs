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
}
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
        })
    }
    pub fn tick(&mut self) -> io::Result<()> {
        self.renew(false)
    }
    pub fn force(&mut self) -> io::Result<()> {
        self.renew(true)
    }
    fn renew(&mut self, force: bool) -> io::Result<()> {
        self.store_directory.verify_path_identity()?;
        self.metadata_directory.verify_path_identity()?;
        let now = self.faults.lease_time_ms()?;
        // A backwards wall-clock correction does not expire an existing lease.
        // Avoid an O(paths²) renewal storm while time catches up.
        if force || now.saturating_sub(self.last_ms) >= RENEW_EVERY_MS {
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
