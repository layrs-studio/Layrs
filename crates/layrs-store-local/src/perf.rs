//! Opt-in diagnostic spans (`LAYRS_PERF=1`). Inclusive wall time and call
//! counts per named span, reported once by the host process. Disabled by
//! default: one relaxed bool check per span, no allocation, no I/O.
//!
//! Leaf filesystem spans (`fs.*`) are also attributed to the innermost
//! enclosing non-`fs.` span as `fs.name@parent`, to locate hot callers.
use std::{
    cell::RefCell,
    collections::BTreeMap,
    sync::{Mutex, OnceLock},
    time::Instant,
};

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("LAYRS_PERF").is_some_and(|v| !v.is_empty()))
}

fn spans() -> &'static Mutex<BTreeMap<String, (u64, u128)>> {
    static SPANS: OnceLock<Mutex<BTreeMap<String, (u64, u128)>>> = OnceLock::new();
    SPANS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

thread_local! {
    static STACK: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

/// Records the guard's lifetime under `name` when diagnostics are enabled.
#[must_use = "a span measures until it is dropped"]
pub struct Span(Option<(&'static str, Instant, Option<&'static str>)>);

impl Drop for Span {
    fn drop(&mut self) {
        if let Some((name, start, parent)) = self.0.take() {
            let elapsed = start.elapsed().as_nanos();
            if !name.starts_with("fs.") {
                STACK.with(|stack| {
                    let mut stack = stack.borrow_mut();
                    if let Some(index) = stack.iter().rposition(|n| *n == name) {
                        stack.remove(index);
                    }
                });
            }
            if let Ok(mut map) = spans().lock() {
                let entry = map.entry(name.to_owned()).or_insert((0, 0));
                entry.0 += 1;
                entry.1 += elapsed;
                if let Some(parent) = parent {
                    let entry = map.entry(format!("{name}@{parent}")).or_insert((0, 0));
                    entry.0 += 1;
                    entry.1 += elapsed;
                }
            }
        }
    }
}

pub fn span(name: &'static str) -> Span {
    if !enabled() {
        return Span(None);
    }
    let parent = if name.starts_with("fs.") {
        STACK.with(|stack| stack.borrow().last().copied())
    } else {
        STACK.with(|stack| stack.borrow_mut().push(name));
        None
    };
    Span(Some((name, Instant::now(), parent)))
}

/// Count an event without timing it.
pub fn count(name: &'static str, n: u64) {
    if enabled() {
        if let Ok(mut map) = spans().lock() {
            map.entry(name.to_owned()).or_insert((0, 0)).0 += n;
        }
    }
}

/// `{"span": {"calls": n, "ms": t}}` sorted by name, or None when disabled.
/// Writes the report, if enabled, as one JSON line labelled with `label` and
/// the process id: appended to the file named by `LAYRS_PERF_OUT` when set
/// (so detached processes such as VFS hosts can be profiled), else stderr.
pub fn emit(label: &str) {
    let Some(report) = report() else { return };
    match std::env::var_os("LAYRS_PERF_OUT") {
        Some(path) => {
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(
                    file,
                    "{{\"label\":{},\"pid\":{},{}",
                    serde_json::Value::String(label.to_owned()),
                    std::process::id(),
                    &report[1..]
                );
            }
        }
        None => eprintln!("{report}"),
    }
}

/// Forgets every span and count recorded so far (per-request reports).
pub fn reset() {
    if let Ok(mut map) = spans().lock() {
        map.clear();
    }
}

pub fn report() -> Option<String> {
    if !enabled() {
        return None;
    }
    let map = spans().lock().ok()?;
    let body = map
        .iter()
        .map(|(name, (calls, nanos))| {
            format!(
                "\"{name}\":{{\"calls\":{calls},\"ms\":{:.3}}}",
                *nanos as f64 / 1_000_000.0
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    Some(format!("{{\"layrsPerf\":{{{body}}}}}"))
}
