use super::{Reading, Severity, StorageMonitorError};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageThresholds {
    pub quota_warning_percent: u8,
    pub quota_critical_percent: u8,
    pub volume_warning_percent: u8,
    pub volume_critical_percent: u8,
    pub database_warning_bytes: Option<u64>,
    pub database_critical_bytes: Option<u64>,
}
impl Default for StorageThresholds {
    fn default() -> Self {
        Self {
            quota_warning_percent: 90,
            quota_critical_percent: 100,
            volume_warning_percent: 90,
            volume_critical_percent: 95,
            database_warning_bytes: None,
            database_critical_bytes: None,
        }
    }
}
impl StorageThresholds {
    pub fn from_env(env: &BTreeMap<String, String>) -> Result<Self, StorageMonitorError> {
        let mut value = Self::default();
        for (name, target) in [
            (
                "LAYRS_STORAGE_QUOTA_WARN_PERCENT",
                &mut value.quota_warning_percent,
            ),
            (
                "LAYRS_STORAGE_QUOTA_CRITICAL_PERCENT",
                &mut value.quota_critical_percent,
            ),
            (
                "LAYRS_STORAGE_VOLUME_WARN_PERCENT",
                &mut value.volume_warning_percent,
            ),
            (
                "LAYRS_STORAGE_VOLUME_CRITICAL_PERCENT",
                &mut value.volume_critical_percent,
            ),
        ] {
            if let Some(text) = env.get(name) {
                *target = text
                    .parse()
                    .map_err(|_| StorageMonitorError::InvalidThresholds)?;
            }
        }
        let bytes = |name| {
            env.get(name)
                .map(|s| {
                    s.parse::<u64>()
                        .map_err(|_| StorageMonitorError::InvalidThresholds)
                })
                .transpose()
        };
        value.database_warning_bytes = bytes("LAYRS_STORAGE_DATABASE_WARN_BYTES")?;
        value.database_critical_bytes = bytes("LAYRS_STORAGE_DATABASE_CRITICAL_BYTES")?;
        value.validate()
    }
    pub fn validate(self) -> Result<Self, StorageMonitorError> {
        for (warn, critical) in [
            (self.quota_warning_percent, self.quota_critical_percent),
            (self.volume_warning_percent, self.volume_critical_percent),
        ] {
            if warn == 0 || warn >= critical || critical > 100 {
                return Err(StorageMonitorError::InvalidThresholds);
            }
        }
        if self.database_warning_bytes == Some(0)
            || self.database_critical_bytes == Some(0)
            || matches!((self.database_warning_bytes,self.database_critical_bytes),(Some(w),Some(c)) if w>=c)
        {
            return Err(StorageMonitorError::InvalidThresholds);
        }
        Ok(self)
    }
    pub(super) fn quota(
        self,
        resource: &'static str,
        unit: &'static str,
        used: u64,
        limit: Option<u64>,
    ) -> Reading {
        let at = |percent| limit.map(|n| percent_at(n, percent));
        reading(
            resource,
            unit,
            used,
            at(self.quota_warning_percent),
            at(self.quota_critical_percent),
        )
    }
    pub(super) fn volume(self, total: u64, available: u64) -> Reading {
        reading(
            "cas_volume_unavailable",
            "bytes",
            total - available,
            Some(percent_at(total, self.volume_warning_percent)),
            Some(percent_at(total, self.volume_critical_percent)),
        )
    }
    pub(super) fn database(self, bytes: u64) -> Reading {
        reading(
            "postgres_database",
            "bytes",
            bytes,
            self.database_warning_bytes,
            self.database_critical_bytes,
        )
    }
}
fn percent_at(total: u64, percent: u8) -> u64 {
    ((u128::from(total) * u128::from(percent)).div_ceil(100)) as u64
}
fn reading(
    resource: &'static str,
    unit: &'static str,
    used: u64,
    warning_at: Option<u64>,
    critical_at: Option<u64>,
) -> Reading {
    let severity = if critical_at.is_some_and(|n| used >= n) {
        Severity::Critical
    } else if warning_at.is_some_and(|n| used >= n) {
        Severity::Warning
    } else {
        Severity::Ok
    };
    Reading {
        resource,
        unit,
        used,
        warning_at,
        critical_at,
        monitored: warning_at.is_some() || critical_at.is_some(),
        severity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundaries_are_exact_and_overflow_safe() {
        assert_eq!(
            (
                Severity::Ok.exit_code(),
                Severity::Warning.exit_code(),
                Severity::Critical.exit_code()
            ),
            (0, 1, 2)
        );
        let limits = StorageThresholds::default();
        for (used, status) in [
            (89, Severity::Ok),
            (90, Severity::Warning),
            (99, Severity::Warning),
            (100, Severity::Critical),
            (101, Severity::Critical),
        ] {
            assert_eq!(
                limits.quota("quota", "bytes", used, Some(100)).severity,
                status
            );
        }
        assert_eq!(
            limits.quota("quota", "bytes", 0, Some(1)).severity,
            Severity::Ok
        );
        assert_eq!(
            limits.quota("quota", "bytes", 1, Some(1)).severity,
            Severity::Critical
        );
        assert_eq!(
            limits
                .quota("quota", "bytes", u64::MAX, Some(u64::MAX))
                .severity,
            Severity::Critical
        );
        assert!(!limits.quota("quota", "bytes", 100, None).monitored);
        assert_eq!(limits.volume(100, 10).severity, Severity::Warning);
        assert_eq!(limits.volume(100, 5).severity, Severity::Critical);
        assert!(!limits.database(100).monitored);
    }
    #[test]
    fn invalid_thresholds_fail_closed_and_database_is_independent() {
        for value in ["0", "100", "-1", "invalid"] {
            let env = BTreeMap::from([("LAYRS_STORAGE_QUOTA_WARN_PERCENT".into(), value.into())]);
            assert!(StorageThresholds::from_env(&env).is_err());
        }
        let env = BTreeMap::from([
            ("LAYRS_STORAGE_DATABASE_WARN_BYTES".into(), "10".into()),
            ("LAYRS_STORAGE_DATABASE_CRITICAL_BYTES".into(), "20".into()),
        ]);
        let thresholds = StorageThresholds::from_env(&env).unwrap();
        assert_eq!(thresholds.database(10).severity, Severity::Warning);
        assert_eq!(thresholds.database(20).severity, Severity::Critical);
        assert_eq!(
            thresholds.quota("quota", "bytes", 0, Some(100)).severity,
            Severity::Ok
        );
    }
}
