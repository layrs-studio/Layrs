//! Process-local cooperative cancellation. Registrations contain no credential.
//! A single atomic transition decides whether cancellation or commit wins.
use super::{remote::RemoteResult, NativeRemoteError};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Mutex, OnceLock,
    },
};
const ACTIVE: u8 = 0;
const CANCELLED: u8 = 1;
const COMMITTING: u8 = 2;
const FINISHED: u8 = 3;

#[derive(Clone, Default)]
pub struct NativeTransferControl(Arc<AtomicU8>);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeTransferCancelStatus {
    CancelRequested,
    AlreadyRequested,
    TooLate,
    NotFound,
}
impl NativeTransferControl {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn request_cancel(&self) -> NativeTransferCancelStatus {
        match self
            .0
            .compare_exchange(ACTIVE, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => NativeTransferCancelStatus::CancelRequested,
            Err(CANCELLED) => NativeTransferCancelStatus::AlreadyRequested,
            Err(_) => NativeTransferCancelStatus::TooLate,
        }
    }
    pub(super) fn check(&self) -> RemoteResult<()> {
        if self.0.load(Ordering::Acquire) == CANCELLED {
            Err(NativeRemoteError::Cancelled)
        } else {
            Ok(())
        }
    }
    pub(super) fn begin_commit(&self) -> RemoteResult<()> {
        match self
            .0
            .compare_exchange(ACTIVE, COMMITTING, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => Ok(()),
            Err(CANCELLED) => Err(NativeRemoteError::Cancelled),
            Err(_) => Err(NativeRemoteError::Invalid(
                "transfer control already consumed",
            )),
        }
    }
    pub(super) fn finish(&self) {
        self.0.store(FINISHED, Ordering::Release);
    }
}

#[derive(PartialEq, Eq)]
struct Scope {
    origin: String,
    root: PathBuf,
    session: [u8; 32],
}
impl Scope {
    fn new(origin: &str, root: &Path, token: &str) -> RemoteResult<Self> {
        let mut hash = blake3::Hasher::new();
        hash.update(b"layrs.transfer-session.v1\0");
        hash.update(token.as_bytes());
        Ok(Self {
            origin: origin.into(),
            root: root.canonicalize()?,
            session: *hash.finalize().as_bytes(),
        })
    }
}
struct Entry {
    scope: Scope,
    control: NativeTransferControl,
}
static REGISTRY: OnceLock<Mutex<BTreeMap<String, Entry>>> = OnceLock::new();
fn registry() -> &'static Mutex<BTreeMap<String, Entry>> {
    REGISTRY.get_or_init(Default::default)
}
fn validate_id(id: &str) -> RemoteResult<()> {
    if !(16..=128).contains(&id.len())
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(NativeRemoteError::Invalid("invalid transfer identifier"));
    }
    Ok(())
}
pub(super) struct Registration {
    id: String,
    pub control: NativeTransferControl,
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.control.finish();
        if let Ok(mut entries) = registry().lock() {
            if entries
                .get(&self.id)
                .is_some_and(|e| Arc::ptr_eq(&e.control.0, &self.control.0))
            {
                entries.remove(&self.id);
            }
        }
    }
}
pub(super) fn register(
    id: &str,
    origin: &str,
    root: &Path,
    token: &str,
) -> RemoteResult<Registration> {
    validate_id(id)?;
    let scope = Scope::new(origin, root, token)?;
    let mut entries = registry()
        .lock()
        .map_err(|_| NativeRemoteError::Invalid("transfer registry unavailable"))?;
    if entries.len() >= 16 {
        return Err(NativeRemoteError::Invalid("too many active transfers"));
    }
    if entries.contains_key(id) {
        return Err(NativeRemoteError::Invalid(
            "transfer identifier already active",
        ));
    }
    let control = NativeTransferControl::new();
    entries.insert(
        id.into(),
        Entry {
            scope,
            control: control.clone(),
        },
    );
    Ok(Registration {
        id: id.into(),
        control,
    })
}
pub(super) fn cancel(
    id: &str,
    origin: &str,
    root: &Path,
    token: &str,
) -> RemoteResult<NativeTransferCancelStatus> {
    validate_id(id)?;
    let scope = Scope::new(origin, root, token)?;
    let entries = registry()
        .lock()
        .map_err(|_| NativeRemoteError::Invalid("transfer registry unavailable"))?;
    Ok(entries
        .get(id)
        .filter(|e| e.scope == scope)
        .map_or(NativeTransferCancelStatus::NotFound, |e| {
            e.control.request_cancel()
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_and_commit_have_exactly_one_winner() {
        for _ in 0..100 {
            let control = NativeTransferControl::new();
            let cancel = control.clone();
            let handle = std::thread::spawn(move || cancel.request_cancel());
            let committed = control.begin_commit().is_ok();
            let cancelled = handle.join().unwrap();
            assert_eq!(committed, cancelled == NativeTransferCancelStatus::TooLate);
        }
    }
    #[test]
    fn registration_is_scoped_and_removed_on_drop() {
        let root = std::env::temp_dir();
        let id = format!("scope-{}", uuid::Uuid::new_v4());
        let registration = register(&id, "https://a/", &root, "test-session-one").unwrap();
        assert_eq!(
            cancel(
                &id,
                "https://a/",
                &std::env::current_dir().unwrap(),
                "test-session-one"
            )
            .unwrap(),
            NativeTransferCancelStatus::NotFound
        );
        assert!(
            register(&id, "https://a/", &root, "test-session-one").is_err(),
            "concurrent duplicate transfer IDs are refused"
        );
        assert_eq!(
            cancel(&id, "https://b/", &root, "test-session-one").unwrap(),
            NativeTransferCancelStatus::NotFound
        );
        assert_eq!(
            cancel(&id, "https://a/", &root, "test-session-two").unwrap(),
            NativeTransferCancelStatus::NotFound
        );
        assert_eq!(
            cancel(&id, "https://a/", &root, "test-session-one").unwrap(),
            NativeTransferCancelStatus::CancelRequested
        );
        assert!(matches!(
            registration.control.begin_commit(),
            Err(NativeRemoteError::Cancelled)
        ));
        drop(registration);
        assert_eq!(
            cancel(&id, "https://a/", &root, "test-session-one").unwrap(),
            NativeTransferCancelStatus::NotFound
        );
    }
    #[test]
    fn once_commit_wins_cancel_cannot_claim_to_stop_it() {
        let control = NativeTransferControl::new();
        control.begin_commit().unwrap();
        assert_eq!(
            control.request_cancel(),
            NativeTransferCancelStatus::TooLate
        );
        control.finish();
        assert_eq!(
            control.request_cancel(),
            NativeTransferCancelStatus::TooLate
        );
    }
}
