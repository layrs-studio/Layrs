use layrs_steps::{DebouncePolicy, StepCapturePlan, StepSummary, StepTrigger};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonConfig {
    pub workspace_root: PathBuf,
    pub poll_interval: Duration,
    pub debounce: DebouncePolicy,
    pub auto_capture: bool,
}

impl DaemonConfig {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            poll_interval: Duration::from_secs(2),
            debounce: DebouncePolicy::default(),
            auto_capture: true,
        }
    }

    pub fn with_poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval;
        self
    }

    pub fn with_debounce(mut self, debounce: DebouncePolicy) -> Self {
        self.debounce = debounce;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewEvent {
    pub path: PathBuf,
    pub kind: ViewEventKind,
}

impl ViewEvent {
    pub fn changed(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            kind: ViewEventKind::Changed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewEventKind {
    Created,
    Changed,
    Deleted,
}

pub trait ViewWatcher {
    fn next_events(&mut self) -> io::Result<Vec<ViewEvent>>;
}

pub trait CaptureSink {
    fn capture(&mut self, plan: StepCapturePlan) -> io::Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureLoopReport {
    pub polled_events: usize,
    pub captured_steps: usize,
}

pub struct CaptureLoop<W, S> {
    config: DaemonConfig,
    watcher: W,
    sink: S,
}

impl<W, S> CaptureLoop<W, S>
where
    W: ViewWatcher,
    S: CaptureSink,
{
    pub fn new(config: DaemonConfig, watcher: W, sink: S) -> Self {
        Self {
            config,
            watcher,
            sink,
        }
    }

    pub fn config(&self) -> &DaemonConfig {
        &self.config
    }

    pub fn run_once(&mut self) -> Result<CaptureLoopReport, DaemonError> {
        let events = self.watcher.next_events()?;
        let polled_events = events.len();

        if !self.config.auto_capture || events.is_empty() {
            return Ok(CaptureLoopReport {
                polled_events,
                captured_steps: 0,
            });
        }

        let plan = build_capture_plan(&self.config, &events);
        self.sink.capture(plan)?;

        Ok(CaptureLoopReport {
            polled_events,
            captured_steps: 1,
        })
    }

    pub fn into_parts(self) -> (DaemonConfig, W, S) {
        (self.config, self.watcher, self.sink)
    }
}

pub fn build_capture_plan(config: &DaemonConfig, events: &[ViewEvent]) -> StepCapturePlan {
    let first_path = events
        .first()
        .map(|event| event.path.clone())
        .unwrap_or_else(|| config.workspace_root.clone());

    let mut summary = StepSummary::titled("Auto snapshot");
    for event in events {
        summary.add_changed_path(relative_to_workspace(&config.workspace_root, &event.path));
    }
    summary.record_artifact();

    StepCapturePlan::new(StepTrigger::ViewChanged {
        path: first_path,
        event_count: events.len(),
    })
    .with_summary(summary)
    .with_debounce(config.debounce)
}

fn relative_to_workspace(workspace_root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(workspace_root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}

#[derive(Debug)]
pub enum DaemonError {
    Io(io::Error),
}

impl fmt::Display for DaemonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for DaemonError {}

impl From<io::Error> for DaemonError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeWatcher {
        events: Vec<ViewEvent>,
    }

    impl ViewWatcher for FakeWatcher {
        fn next_events(&mut self) -> io::Result<Vec<ViewEvent>> {
            Ok(std::mem::take(&mut self.events))
        }
    }

    #[derive(Default)]
    struct FakeSink {
        plans: Vec<StepCapturePlan>,
    }

    impl CaptureSink for FakeSink {
        fn capture(&mut self, plan: StepCapturePlan) -> io::Result<()> {
            self.plans.push(plan);
            Ok(())
        }
    }

    #[test]
    fn run_once_captures_one_plan_for_a_batch() {
        let config = DaemonConfig::new(PathBuf::from("workspace"));
        let watcher = FakeWatcher {
            events: vec![
                ViewEvent::changed(PathBuf::from("workspace/src/lib.rs")),
                ViewEvent::changed(PathBuf::from("workspace/src/main.rs")),
            ],
        };
        let sink = FakeSink::default();
        let mut capture_loop = CaptureLoop::new(config, watcher, sink);

        let report = capture_loop.run_once().expect("run once");
        let (_config, _watcher, sink) = capture_loop.into_parts();

        assert_eq!(report.polled_events, 2);
        assert_eq!(report.captured_steps, 1);
        assert_eq!(sink.plans.len(), 1);
        assert_eq!(sink.plans[0].summary.changed_paths.len(), 2);
    }

    #[test]
    fn auto_capture_can_be_disabled() {
        let mut config = DaemonConfig::new(PathBuf::from("workspace"));
        config.auto_capture = false;

        let watcher = FakeWatcher {
            events: vec![ViewEvent::changed("workspace/file.txt")],
        };
        let sink = FakeSink::default();
        let mut capture_loop = CaptureLoop::new(config, watcher, sink);

        let report = capture_loop.run_once().expect("run once");
        let (_config, _watcher, sink) = capture_loop.into_parts();

        assert_eq!(report.polled_events, 1);
        assert_eq!(report.captured_steps, 0);
        assert!(sink.plans.is_empty());
    }
}
