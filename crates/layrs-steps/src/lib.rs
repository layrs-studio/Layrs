use std::path::PathBuf;
use std::time::Duration;

pub const DEFAULT_DEBOUNCE_QUIET_MS: u64 = 750;
pub const DEFAULT_DEBOUNCE_MAX_WINDOW_MS: u64 = 5_000;
pub const DEFAULT_DEBOUNCE_MAX_EVENTS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepCapturePlan {
    pub workspace_id: Option<String>,
    pub space_id: Option<String>,
    pub layer_id: Option<String>,
    pub trigger: StepTrigger,
    pub summary: StepSummary,
    pub debounce: DebouncePolicy,
}

impl StepCapturePlan {
    pub fn new(trigger: StepTrigger) -> Self {
        Self {
            workspace_id: None,
            space_id: None,
            layer_id: None,
            trigger,
            summary: StepSummary::default(),
            debounce: DebouncePolicy::default(),
        }
    }

    pub fn with_scope(
        mut self,
        workspace_id: impl Into<String>,
        space_id: impl Into<String>,
        layer_id: impl Into<String>,
    ) -> Self {
        self.workspace_id = Some(workspace_id.into());
        self.space_id = Some(space_id.into());
        self.layer_id = Some(layer_id.into());
        self
    }

    pub fn with_summary(mut self, summary: StepSummary) -> Self {
        self.summary = summary;
        self
    }

    pub fn with_debounce(mut self, debounce: DebouncePolicy) -> Self {
        self.debounce = debounce;
        self
    }

    pub fn is_scoped(&self) -> bool {
        self.workspace_id.is_some() && self.space_id.is_some() && self.layer_id.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepTrigger {
    Manual {
        actor: Option<String>,
    },
    ViewChanged {
        path: PathBuf,
        event_count: usize,
    },
    Scheduled {
        schedule_id: String,
    },
    External {
        source: String,
        correlation_id: Option<String>,
    },
    DaemonTick,
}

impl StepTrigger {
    pub fn manual() -> Self {
        Self::Manual { actor: None }
    }

    pub fn view_changed(path: impl Into<PathBuf>) -> Self {
        Self::ViewChanged {
            path: path.into(),
            event_count: 1,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Manual { .. } => "manual",
            Self::ViewChanged { .. } => "view_changed",
            Self::Scheduled { .. } => "scheduled",
            Self::External { .. } => "external",
            Self::DaemonTick => "daemon_tick",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepSummary {
    pub title: String,
    pub detail: Option<String>,
    pub changed_paths: Vec<PathBuf>,
    pub artifact_count: u32,
    pub proof_count: u32,
    pub status: StepSummaryStatus,
}

impl Default for StepSummary {
    fn default() -> Self {
        Self {
            title: String::new(),
            detail: None,
            changed_paths: Vec::new(),
            artifact_count: 0,
            proof_count: 0,
            status: StepSummaryStatus::Planned,
        }
    }
}

impl StepSummary {
    pub fn titled(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn add_changed_path(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        if !self.changed_paths.iter().any(|existing| existing == &path) {
            self.changed_paths.push(path);
        }
    }

    pub fn record_artifact(&mut self) {
        self.artifact_count += 1;
    }

    pub fn record_proof(&mut self) {
        self.proof_count += 1;
    }

    pub fn mark_captured(&mut self) {
        self.status = StepSummaryStatus::Captured;
    }

    pub fn is_empty(&self) -> bool {
        self.title.is_empty()
            && self.detail.is_none()
            && self.changed_paths.is_empty()
            && self.artifact_count == 0
            && self.proof_count == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepSummaryStatus {
    Planned,
    Captured,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DebouncePolicy {
    pub quiet_period: Duration,
    pub max_window: Duration,
    pub max_events: usize,
    pub merge: DebounceMerge,
}

impl Default for DebouncePolicy {
    fn default() -> Self {
        Self {
            quiet_period: Duration::from_millis(DEFAULT_DEBOUNCE_QUIET_MS),
            max_window: Duration::from_millis(DEFAULT_DEBOUNCE_MAX_WINDOW_MS),
            max_events: DEFAULT_DEBOUNCE_MAX_EVENTS,
            merge: DebounceMerge::MergeChangedPaths,
        }
    }
}

impl DebouncePolicy {
    pub fn disabled() -> Self {
        Self {
            quiet_period: Duration::ZERO,
            max_window: Duration::ZERO,
            max_events: 1,
            merge: DebounceMerge::CaptureEveryEvent,
        }
    }

    pub fn should_capture(&self, window: &DebounceWindow, now: Duration) -> bool {
        if self.max_events > 0 && window.event_count >= self.max_events {
            return true;
        }

        let since_last = now.saturating_sub(window.last_event_at);
        if since_last >= self.quiet_period {
            return true;
        }

        let since_first = now.saturating_sub(window.first_event_at);
        since_first >= self.max_window
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebounceMerge {
    CaptureEveryEvent,
    MergeChangedPaths,
    KeepLatest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebounceWindow {
    pub first_event_at: Duration,
    pub last_event_at: Duration,
    pub event_count: usize,
}

impl DebounceWindow {
    pub fn new(first_event_at: Duration) -> Self {
        Self {
            first_event_at,
            last_event_at: first_event_at,
            event_count: 1,
        }
    }

    pub fn observe(&mut self, event_at: Duration) {
        if event_at < self.first_event_at {
            self.first_event_at = event_at;
        }

        if event_at >= self.last_event_at {
            self.last_event_at = event_at;
        }

        self.event_count += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_deduplicates_changed_paths() {
        let mut summary = StepSummary::titled("snapshot");
        summary.add_changed_path("src/lib.rs");
        summary.add_changed_path("src/lib.rs");
        summary.record_artifact();

        assert_eq!(summary.changed_paths, vec![PathBuf::from("src/lib.rs")]);
        assert_eq!(summary.artifact_count, 1);
        assert!(!summary.is_empty());
    }

    #[test]
    fn debounce_waits_for_quiet_period() {
        let policy = DebouncePolicy {
            quiet_period: Duration::from_millis(100),
            max_window: Duration::from_millis(1_000),
            max_events: 10,
            merge: DebounceMerge::MergeChangedPaths,
        };
        let window = DebounceWindow {
            first_event_at: Duration::from_millis(0),
            last_event_at: Duration::from_millis(50),
            event_count: 2,
        };

        assert!(!policy.should_capture(&window, Duration::from_millis(120)));
        assert!(policy.should_capture(&window, Duration::from_millis(150)));
    }

    #[test]
    fn capture_plan_scope_is_explicit() {
        let plan = StepCapturePlan::new(StepTrigger::manual())
            .with_scope("workspace", "space", "layer")
            .with_summary(StepSummary::titled("manual snapshot"));

        assert!(plan.is_scoped());
        assert_eq!(plan.trigger.label(), "manual");
        assert_eq!(plan.summary.title, "manual snapshot");
    }
}
