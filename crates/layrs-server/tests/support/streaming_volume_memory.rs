//! OS counters for this test process (HTTPS server AND blocking native clients).
use serde::Serialize;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub supported: bool,
    pub resident_bytes: u64,
    pub private_bytes: Option<u64>,
    pub process_lifetime_peak_resident_bytes: u64,
    pub samples: u64,
}
impl Sample {
    fn include(&mut self, next: Self) {
        self.supported |= next.supported;
        self.resident_bytes = self.resident_bytes.max(next.resident_bytes);
        self.private_bytes = match (self.private_bytes, next.private_bytes) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        self.process_lifetime_peak_resident_bytes = self
            .process_lifetime_peak_resident_bytes
            .max(next.process_lifetime_peak_resident_bytes);
        self.samples += u64::from(next.supported);
    }
}
pub struct Monitor {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<Sample>>,
}
impl Monitor {
    pub fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let signal = stop.clone();
        let worker = thread::spawn(move || {
            let mut sample = Sample::default();
            loop {
                sample.include(current());
                if signal.load(Ordering::Acquire) {
                    return sample;
                }
                thread::sleep(Duration::from_millis(50));
            }
        });
        Self {
            stop,
            worker: Some(worker),
        }
    }
    pub fn finish(mut self) -> Sample {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap_or_default()
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(windows)]
fn current() -> Sample {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        size: u32,
        page_faults: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged: usize,
        quota_paged: usize,
        quota_peak_nonpaged: usize,
        quota_nonpaged: usize,
        pagefile: usize,
        peak_pagefile: usize,
        private: usize,
    }
    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut core::ffi::c_void,
            counters: *mut Counters,
            size: u32,
        ) -> i32;
    }
    let size = std::mem::size_of::<Counters>() as u32;
    let mut value = Counters {
        size,
        ..Default::default()
    };
    // The documented current-process pseudo handle needs no close; the buffer
    // has exactly PROCESS_MEMORY_COUNTERS_EX layout and remains live for the call.
    if unsafe { GetProcessMemoryInfo(-1isize as *mut _, &mut value, size) } == 0 {
        return Sample::default();
    }
    Sample {
        supported: true,
        resident_bytes: value.working_set as u64,
        private_bytes: Some(value.private as u64),
        process_lifetime_peak_resident_bytes: value.peak_working_set as u64,
        samples: 0,
    }
}

#[cfg(target_os = "linux")]
fn current() -> Sample {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return Sample::default();
    };
    let value = |prefix: &str| {
        status.lines().find_map(|line| {
            line.strip_prefix(prefix)?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()?
                .checked_mul(1024)
        })
    };
    match (value("VmRSS:"), value("VmHWM:")) {
        (Some(resident_bytes), Some(peak)) => Sample {
            supported: true,
            resident_bytes,
            private_bytes: None,
            process_lifetime_peak_resident_bytes: peak,
            samples: 0,
        },
        _ => Sample::default(),
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn current() -> Sample {
    Sample::default()
}
