//! Test-only owned subprocess boundary. Root executes these recipes; source
//! authors do not run them. The opt-in asserts an audited disposable test host,
//! not a product activation or automatic-extension-registry proof.
use std::{
    os::windows::process::CommandExt,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Panic paths also terminate/reap this exact owned handle. Never kill by a
/// rediscovered PID or enumerate other processes.
pub(in crate::projection::provider) struct OwnedChild(std::process::Child);
impl OwnedChild {
    pub(in crate::projection::provider) fn new(child: std::process::Child) -> Self {
        Self(child)
    }
}
impl std::ops::Deref for OwnedChild {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for OwnedChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let kill = self.0.kill();
            let reap = self.0.wait();
            if kill.is_err() || reap.is_err() {
                eprintln!("UNQUALIFIED owned child cleanup: kill={kill:?}, reap={reap:?}");
            }
        }
    }
}

pub(in crate::projection::provider) fn run(name: &str, seconds: u64, body: impl FnOnce()) {
    assert_eq!(
        std::env::var("LAYRS_PROVIDER_SQLITE_TEST_HOST").as_deref(),
        Ok("audited-layrs-test-host-v1"),
        "UNQUALIFIED: root must explicitly opt this owned test process into its audited host prerequisite"
    );
    let name = name.strip_prefix("layrs_store_local::").unwrap_or(name);
    if std::env::var("LAYRS_PROVIDER_OWNED_CASE").as_deref() == Ok(name) {
        let nonce = std::env::var("LAYRS_PROVIDER_OWNED_NONCE").expect("owned child nonce");
        assert!(
            !nonce.is_empty()
                && nonce.len() <= 128
                && nonce.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
        );
        body();
        return;
    }
    let nonce = format!(
        "{:x}-{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let mut child = OwnedChild::new(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                name,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("LAYRS_PROVIDER_OWNED_CASE", name)
            .env("LAYRS_PROVIDER_OWNED_NONCE", nonce)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .creation_flags(0x08000000)
            .spawn()
            .expect("launch this test's owned hidden child"),
    );
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("query owned child") {
            assert!(status.success(), "owned case {name} exited {status}");
            return;
        }
        if started.elapsed() > Duration::from_secs(seconds) {
            let kill = child.kill();
            let reaped = child.wait();
            panic!(
                "owned case {name} exceeded {seconds}s; kill={kill:?}, reap={reaped:?}; retain all fixtures"
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub(in crate::projection::provider) fn declare() {
    super::super::declare_provider_sqlite_process_profile(
        super::super::ProviderSqliteProcessDeclaration::AuditedLayrsHostV1,
    )
    .expect("separately qualified owned test host/build candidate");
}
