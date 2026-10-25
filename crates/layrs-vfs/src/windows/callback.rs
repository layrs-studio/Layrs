use super::{
    api::{Api, check},
    raw,
};
use crate::{
    Cancellation, Catalog, Error, HydrationRange, Registration, SnapshotSource,
    catalog::read_checked,
};
use std::{
    collections::{BTreeMap, VecDeque},
    mem::{offset_of, size_of},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use windows_sys::Win32::{Foundation::*, Storage::CloudFilters::*};

const MAX_REQUESTS: usize = 8;

#[derive(Debug, Clone)]
pub struct CallbackFailure {
    pub transfer_key: i64,
    pub request_key: i64,
    pub message: String,
}
/// Completed callback evidence, bounded to the most recent 64 requests.
/// Windows may request a whole file even for a small application read.
#[derive(Debug, Clone)]
pub struct HydrationRequest {
    pub file_id: i64,
    pub file_size: i64,
    pub transfer_key: i64,
    pub request_key: i64,
    pub process_id: Option<u32>,
    pub required_offset: i64,
    pub required_length: i64,
    pub optional_offset: i64,
    pub optional_length: i64,
    pub transfer_start: u64,
    pub transfer_end: u64,
    pub source_bytes_read: u64,
    pub transferred_bytes: u64,
    pub completion_status: i32,
}
#[derive(Default)]
struct Pending {
    requests: BTreeMap<(i64, i64, i64, i64), Cancellation>,
    callbacks: usize,
    failures: VecDeque<CallbackFailure>,
    traces: VecDeque<HydrationRequest>,
}
pub(super) struct Context {
    pub api: Arc<Api>,
    pub source: Arc<dyn SnapshotSource>,
    catalog: Arc<Catalog>,
    // Retained until successful disconnect AND callback drain; on uncertain
    // teardown Session leaks this actual proof alongside source and DLL.
    _proof: Option<super::provider_root::ProviderRootGuard>,
    root_blob: Vec<u8>,
    stopping: AtomicBool,
    pending: Mutex<Pending>,
    drained: Condvar,
}
impl Context {
    pub fn new(
        api: Arc<Api>,
        source: Arc<dyn SnapshotSource>,
        catalog: Arc<Catalog>,
        registration: &Registration,
        proof: Option<super::provider_root::ProviderRootGuard>,
    ) -> Self {
        Self {
            api,
            source,
            catalog,
            _proof: proof,
            root_blob: registration.root_blob(true),
            stopping: AtomicBool::new(false),
            pending: Mutex::new(Pending::default()),
            drained: Condvar::new(),
        }
    }
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
        for cancel in self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .requests
            .values()
        {
            cancel.cancel();
        }
    }
    pub fn drain(&self) {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        while pending.callbacks != 0 || !pending.requests.is_empty() {
            pending = self
                .drained
                .wait(pending)
                .unwrap_or_else(|e| e.into_inner());
        }
    }
    pub fn failures(&self) -> Vec<CallbackFailure> {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .failures
            .iter()
            .cloned()
            .collect()
    }
    pub fn traces(&self) -> Vec<HydrationRequest> {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .traces
            .iter()
            .cloned()
            .collect()
    }
    fn record_trace(&self, trace: HydrationRequest) {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if pending.traces.len() == 64 {
            pending.traces.pop_front();
        }
        pending.traces.push_back(trace);
    }
    fn record(&self, info: &CF_CALLBACK_INFO, error: impl std::fmt::Display) {
        let message = catch_unwind(AssertUnwindSafe(|| error.to_string()))
            .unwrap_or_else(|_| "error formatter panicked".into());
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if pending.failures.len() == 32 {
            pending.failures.pop_front();
        }
        pending.failures.push_back(CallbackFailure {
            transfer_key: info.TransferKey,
            request_key: info.RequestKey,
            message: message.chars().take(512).collect(),
        });
    }
    fn begin(
        &self,
        info: &CF_CALLBACK_INFO,
        offset: i64,
        length: i64,
    ) -> crate::Result<Request<'_>> {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if self.stopping.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        let key = (info.TransferKey, info.RequestKey, offset, length);
        if pending.requests.len() >= MAX_REQUESTS || pending.requests.contains_key(&key) {
            return Err(Error::Busy);
        }
        let cancellation = Cancellation::default();
        pending.requests.insert(key, cancellation.clone());
        Ok(Request {
            context: self,
            key,
            cancellation,
        })
    }
}
struct Request<'a> {
    context: &'a Context,
    key: (i64, i64, i64, i64),
    cancellation: Cancellation,
}
struct CallbackGuard<'a>(&'a Context);
impl Drop for CallbackGuard<'_> {
    fn drop(&mut self) {
        self.0
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .callbacks -= 1;
        self.0.drained.notify_all();
    }
}
impl Drop for Request<'_> {
    fn drop(&mut self) {
        self.context
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .requests
            .remove(&self.key);
        self.context.drained.notify_all();
    }
}

fn operation(info: &CF_CALLBACK_INFO, kind: CF_OPERATION_TYPE) -> CF_OPERATION_INFO {
    CF_OPERATION_INFO {
        StructSize: size_of::<CF_OPERATION_INFO>() as u32,
        Type: kind,
        ConnectionKey: info.ConnectionKey,
        TransferKey: info.TransferKey,
        RequestKey: info.RequestKey,
        CorrelationVector: info.CorrelationVector,
        SyncStatus: std::ptr::null(),
    }
}

fn transfer(
    context: &Context,
    info: &CF_CALLBACK_INFO,
    offset: i64,
    length: i64,
    bytes: Option<&[u8]>,
    status: i32,
) -> crate::Result<()> {
    let op = operation(info, CF_OPERATION_TYPE_TRANSFER_DATA);
    let mut params = CF_OPERATION_PARAMETERS {
        ParamSize: (offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
            + size_of::<CF_OPERATION_PARAMETERS_0_0>()) as u32,
        Anonymous: CF_OPERATION_PARAMETERS_0 {
            TransferData: CF_OPERATION_PARAMETERS_0_0 {
                Flags: CF_OPERATION_TRANSFER_DATA_FLAG_NONE,
                CompletionStatus: status,
                Buffer: bytes.map_or(std::ptr::null(), |b| b.as_ptr().cast()),
                Offset: offset,
                Length: length,
            },
        },
    };
    // Input buffers, callback request identity and correlation vector remain
    // alive throughout this synchronous call. No callback pointers are queued.
    check("CfExecute(TRANSFER_DATA)", unsafe {
        (context.api.execute)(&op, &mut params)
    })
}
fn failure_status(error: &Error) -> i32 {
    match error {
        Error::MissingBytes { .. } => STATUS_CLOUD_FILE_UNSUCCESSFUL,
        Error::Cancelled => STATUS_CLOUD_FILE_REQUEST_CANCELED,
        Error::InvalidRange | Error::IdentityMismatch => STATUS_CLOUD_FILE_INVALID_REQUEST,
        Error::Busy => STATUS_CLOUD_FILE_INSUFFICIENT_RESOURCES,
        Error::Unsupported(_) => STATUS_CLOUD_FILE_NOT_SUPPORTED,
        _ => STATUS_CLOUD_FILE_UNSUCCESSFUL,
    }
}

// Windows guarantees valid, call-scoped buffers. Length checks bound every
// opaque identity read; no Rust panic is allowed to cross the system ABI.
unsafe fn context<'a>(
    info: &'a CF_CALLBACK_INFO,
) -> Option<(&'a Context, &'a CF_CALLBACK_INFO, CallbackGuard<'a>)> {
    if info.CallbackContext.is_null() {
        return None;
    }
    let context = unsafe { &*info.CallbackContext.cast::<Context>() };
    context
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .callbacks += 1;
    Some((context, info, CallbackGuard(context)))
}
unsafe fn identity<'a>(data: *const std::ffi::c_void, length: u32) -> crate::Result<&'a [u8]> {
    if data.is_null() || length == 0 || length > 4096 {
        return Err(Error::IdentityMismatch);
    }
    Ok(unsafe { std::slice::from_raw_parts(data.cast(), length as usize) })
}

unsafe extern "system" fn fetch(
    info: *const CF_CALLBACK_INFO,
    params: *const CF_CALLBACK_PARAMETERS,
) {
    let Some(parsed_info) = (unsafe { raw::callback_info(info) }) else {
        return;
    };
    let Some((context, info, _inflight)) = (unsafe { context(&parsed_info) }) else {
        return;
    };
    let Some(fetch) = (unsafe { raw::fetch(params) }) else {
        return;
    };
    let mut trace = HydrationRequest {
        file_id: info.FileId,
        file_size: info.FileSize,
        transfer_key: info.TransferKey,
        request_key: info.RequestKey,
        process_id: unsafe { raw::process_id(info.ProcessInfo) },
        required_offset: fetch.required_offset,
        required_length: fetch.required_length,
        optional_offset: fetch.optional_offset,
        optional_length: fetch.optional_length,
        transfer_start: 0,
        transfer_end: 0,
        source_bytes_read: 0,
        transferred_bytes: 0,
        completion_status: STATUS_SUCCESS,
    };
    let result = catch_unwind(AssertUnwindSafe(|| -> crate::Result<()> {
        let request = context.begin(info, fetch.required_offset, fetch.required_length)?;
        if unsafe { identity(info.SyncRootIdentity, info.SyncRootIdentityLength)? }
            != context.root_blob
        {
            return Err(Error::IdentityMismatch);
        }
        let entry = context
            .catalog
            .resolve_identity(unsafe { identity(info.FileIdentity, info.FileIdentityLength)? })?;
        // A partial file may have private writes; hydration is still from its
        // immutable identity. Size changes require a future reconciliation path.
        if info.FileSize != entry.size as i64 {
            return Err(Error::Unsupported("hydration after local size change"));
        }
        let ranges = HydrationRange::new(entry.size, fetch.required_offset, fetch.required_length)?;
        trace.transfer_start = ranges.bounds().start;
        trace.transfer_end = ranges.bounds().end;
        let mut buffer = vec![0u8; crate::READ_BUFFER_SIZE];
        for range in ranges {
            let length = (range.end - range.start) as usize;
            read_checked(
                context.source.as_ref(),
                entry,
                range.start,
                &mut buffer[..length],
                &request.cancellation,
            )?;
            trace.source_bytes_read += length as u64;
            transfer(
                context,
                info,
                range.start as i64,
                length as i64,
                Some(&buffer[..length]),
                STATUS_SUCCESS,
            )?;
            trace.transferred_bytes += length as u64;
        }
        Ok(())
    }));
    let error = match result {
        Ok(Ok(())) => {
            context.record_trace(trace);
            return;
        }
        Ok(Err(e)) => e,
        Err(_) => Error::Unsupported("snapshot source panicked"),
    };
    trace.completion_status = failure_status(&error);
    context.record_trace(trace);
    context.record(info, &error);
    // Fail the entire requested interval, aligned exactly as successful data.
    // Never substitute bytes after a source error (even a partially filled read).
    if let Ok(ranges) = HydrationRange::new(
        info.FileSize.max(0) as u64,
        fetch.required_offset,
        fetch.required_length,
    ) {
        let range = ranges.bounds();
        if range.start < range.end {
            if let Err(e) = transfer(
                context,
                info,
                range.start as i64,
                (range.end - range.start) as i64,
                None,
                failure_status(&error),
            ) {
                context.record(info, e);
            }
        }
    }
}
unsafe extern "system" fn cancel(
    info: *const CF_CALLBACK_INFO,
    params: *const CF_CALLBACK_PARAMETERS,
) {
    let Some(parsed_info) = (unsafe { raw::callback_info(info) }) else {
        return;
    };
    if let Some((context, info, _inflight)) = unsafe { context(&parsed_info) } {
        let Some(cancel_range) = (unsafe { raw::cancel(params) }) else {
            return;
        };
        for ((transfer, request, offset, length), cancel) in &context
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .requests
        {
            if *transfer == info.TransferKey
                && *request == info.RequestKey
                && covers(
                    cancel_range.offset,
                    cancel_range.length,
                    *offset,
                    *length,
                    info.FileSize,
                )
            {
                cancel.cancel();
            }
        }
        // With PROGRESSIVE, last-handle closure may cancel background work.
        // A subset must not cancel still-required bytes in the same fetch or a
        // different random-access fetch. We conservatively finish such a fetch
        // in bounded chunks; this may do extra IO for the cancelled subset.
        // Fully covered requests stop cooperatively, including shutdown.
    }
}
fn covers(cancel_start: i64, cancel_length: i64, start: i64, length: i64, size: i64) -> bool {
    if size < 0
        || start < 0
        || start >= size
        || cancel_start < 0
        || cancel_start >= size
        || length == 0
        || length < -1
        || cancel_length == 0
        || cancel_length < -1
    {
        return false;
    }
    let end = if length == -1 {
        Some(size)
    } else {
        start.checked_add(length)
    };
    let cancel_end = if cancel_length == -1 {
        Some(size)
    } else {
        cancel_start.checked_add(cancel_length)
    };
    cancel_start >= 0
        && cancel_start <= start
        && matches!((end, cancel_end), (Some(end), Some(cancel_end)) if cancel_end >= end)
}

#[cfg(test)]
mod cancellation_tests {
    use super::covers;

    #[test]
    fn cancellation_does_not_discard_still_required_random_access_bytes() {
        // Background and foreground fetches may overlap or arrive out of order.
        assert!(!covers(8192, 4096, 4096, 8192, 16384));
        assert!(!covers(4096, 4096, 8192, 4096, 16384));
        assert!(covers(4096, 8192, 8192, 4096, 16384));
        assert!(covers(8192, -1, 8192, 4096, 16384));
        assert!(covers(8192, -1, 8192, -1, 16384));
        assert!(!covers(12288, -1, 8192, -1, 16384));
    }

    #[test]
    fn malformed_cancellation_cannot_cancel_a_request() {
        for (offset, length) in [(-1, 4096), (4096, 0), (4096, -2), (4096, i64::MAX)] {
            assert!(!covers(offset, length, 4096, 4096, 16384));
        }
        assert!(!covers(0, -1, 0, 1, -1));
        assert!(!covers(0, -1, 16384, -1, 16384));
    }
}
unsafe extern "system" fn rename(info: *const CF_CALLBACK_INFO, _: *const CF_CALLBACK_PARAMETERS) {
    let Some(parsed_info) = (unsafe { raw::callback_info(info) }) else {
        return;
    };
    if let Some((context, info, _inflight)) = unsafe { context(&parsed_info) } {
        let op = operation(info, CF_OPERATION_TYPE_ACK_RENAME);
        let mut params = CF_OPERATION_PARAMETERS {
            ParamSize: (offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
                + size_of::<CF_OPERATION_PARAMETERS_0_6>()) as u32,
            Anonymous: CF_OPERATION_PARAMETERS_0 {
                AckRename: CF_OPERATION_PARAMETERS_0_6 {
                    Flags: CF_OPERATION_ACK_RENAME_FLAG_NONE,
                    CompletionStatus: STATUS_CLOUD_FILE_NOT_SUPPORTED,
                },
            },
        };
        if let Err(error) = check("CfExecute(ACK_RENAME)", unsafe {
            (context.api.execute)(&op, &mut params)
        }) {
            context.record(info, error);
        }
    }
}
unsafe extern "system" fn dehydrate(
    info: *const CF_CALLBACK_INFO,
    _: *const CF_CALLBACK_PARAMETERS,
) {
    let Some(parsed_info) = (unsafe { raw::callback_info(info) }) else {
        return;
    };
    if let Some((context, info, _inflight)) = unsafe { context(&parsed_info) } {
        let op = operation(info, CF_OPERATION_TYPE_ACK_DEHYDRATE);
        let mut params = CF_OPERATION_PARAMETERS {
            ParamSize: (offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
                + size_of::<CF_OPERATION_PARAMETERS_0_5>()) as u32,
            Anonymous: CF_OPERATION_PARAMETERS_0 {
                AckDehydrate: CF_OPERATION_PARAMETERS_0_5 {
                    Flags: CF_OPERATION_ACK_DEHYDRATE_FLAG_NONE,
                    CompletionStatus: STATUS_CLOUD_FILE_NOT_SUPPORTED,
                    ..Default::default()
                },
            },
        };
        if let Err(error) = check("CfExecute(ACK_DEHYDRATE)", unsafe {
            (context.api.execute)(&op, &mut params)
        }) {
            context.record(info, error);
        }
    }
}
pub(super) fn table() -> [CF_CALLBACK_REGISTRATION; 5] {
    [
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_FETCH_DATA,
            Callback: Some(fetch),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_CANCEL_FETCH_DATA,
            Callback: Some(cancel),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NOTIFY_RENAME,
            Callback: Some(rename),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NOTIFY_DEHYDRATE,
            Callback: Some(dehydrate),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NONE,
            Callback: None,
        },
    ]
}
