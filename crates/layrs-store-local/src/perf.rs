//! Opt-in diagnostic spans (`LAYRS_PERF=1`). Inclusive wall time and call
//! counts per named span, reported once by the host process. Disabled by
//! default: one relaxed bool check per span, no allocation, no I/O.
//!
//! Leaf filesystem spans (`fs.*`) and Store openings (`store.open*`) are
//! also attributed to the innermost enclosing non-`fs.` span as
//! `name@parent`, to locate hot callers.
//!
//! With `LAYRS_PERF_TRACE=<file>` as well, every span of at least 0.1 ms is
//! also kept as an event (thread, depth, start and end from the first span)
//! and appended to that file as one JSON line, to see what runs in parallel
//! and what is on the critical path.
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU32, Ordering},
    },
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
    static THREAD: Cell<u32> = const { Cell::new(0) };
}

fn tracing() -> bool {
    static TRACING: OnceLock<bool> = OnceLock::new();
    *TRACING.get_or_init(|| std::env::var_os("LAYRS_PERF_TRACE").is_some_and(|v| !v.is_empty()))
}

/// (name, thread, depth, start, end), in microseconds from the first span.
type Event = (&'static str, u32, u32, u64, u64);

fn events() -> &'static Mutex<Vec<Event>> {
    static EVENTS: OnceLock<Mutex<Vec<Event>>> = OnceLock::new();
    EVENTS.get_or_init(|| Mutex::new(Vec::new()))
}

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn thread_number() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    THREAD.with(|thread| {
        if thread.get() == 0 {
            thread.set(NEXT.fetch_add(1, Ordering::Relaxed));
        }
        thread.get()
    })
}

/// Records the guard's lifetime under `name` when diagnostics are enabled.
#[must_use = "a span measures until it is dropped"]
pub struct Span(Option<(&'static str, Instant, Option<&'static str>, u32)>);

impl Drop for Span {
    fn drop(&mut self) {
        if let Some((name, start, parent, depth)) = self.0.take() {
            let elapsed = start.elapsed().as_nanos();
            if tracing() && elapsed >= 100_000 {
                let begin = start.saturating_duration_since(epoch()).as_micros() as u64;
                let event = (
                    name,
                    thread_number(),
                    depth,
                    begin,
                    begin + (elapsed / 1_000) as u64,
                );
                if let Ok(mut events) = events().lock() {
                    events.push(event);
                }
            }
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
    if tracing() {
        epoch();
    }
    let (parent, depth) = if name.starts_with("fs.") {
        STACK.with(|stack| {
            let stack = stack.borrow();
            (stack.last().copied(), stack.len() as u32)
        })
    } else {
        STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            let parent = name
                .starts_with("store.open")
                .then(|| stack.last().copied())
                .flatten();
            let depth = stack.len() as u32;
            stack.push(name);
            (parent, depth)
        })
    };
    Span(Some((name, Instant::now(), parent, depth)))
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
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
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
    if let (Some(path), Ok(events)) = (std::env::var_os("LAYRS_PERF_TRACE"), events().lock()) {
        use std::io::Write;
        let body = events
            .iter()
            .map(|(name, thread, depth, start, end)| {
                format!("[\"{name}\",{thread},{depth},{start},{end}]")
            })
            .collect::<Vec<_>>()
            .join(",");
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(
                file,
                "{{\"label\":{},\"pid\":{},\"events\":[{body}]}}",
                serde_json::Value::String(label.to_owned()),
                std::process::id()
            );
        }
    }
}

/// Forgets every span and count recorded so far (per-request reports).
pub fn reset() {
    if let Ok(mut map) = spans().lock() {
        map.clear();
    }
    if let Ok(mut events) = events().lock() {
        events.clear();
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
