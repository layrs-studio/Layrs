//! Real HTTPS transfer across process death/recreation; credentials remain in
//! the child process environment, never in the durable cache or diagnostic text.
use layrs_client_core::native_scm::{NativeRemote, NativeWorkspace};
use layrs_core::scm::SpaceId;
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

fn client(origin: &str, token: &str, ca: &str, cache: &Path) -> NativeRemote {
    NativeRemote::with_trusted_ca(origin, token, ca.as_bytes())
        .unwrap()
        .with_download_cache(cache)
}
fn records(cache: &Path) -> Vec<PathBuf> {
    let mut paths = vec![];
    for entry in std::fs::read_dir(cache).unwrap() {
        let entry = entry.unwrap();
        if !entry.file_name().to_string_lossy().starts_with("entry-") {
            continue;
        }
        for part in std::fs::read_dir(entry.path()).unwrap() {
            let part = part.unwrap();
            if part.file_name().to_string_lossy().starts_with("part-") {
                paths.push(part.path());
            }
        }
    }
    paths.sort();
    paths
}
#[allow(clippy::too_many_arguments)]
pub fn run(
    root: &Path,
    origin: &str,
    token: &str,
    ca: &str,
    space: &SpaceId,
    interrupted: &AtomicUsize,
    resumed: &AtomicUsize,
) {
    let cache = root.join("durable-download-cache");
    let target = root.join("durable-download-clone");
    std::fs::create_dir(&target).unwrap();
    interrupted.store(4, Ordering::SeqCst);
    assert!(
        client(origin, token, ca, &cache)
            .clone_into(&target, space, "Restart")
            .is_err()
    );
    assert_eq!(interrupted.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::fs::read_dir(&target).unwrap().count(),
        0,
        "no local Store before closure"
    );
    let saved = records(&cache);
    assert!(!saved.is_empty());
    let before = saved
        .iter()
        .map(|p| std::fs::read(p).unwrap())
        .collect::<Vec<_>>();
    assert!(before.iter().all(|bytes| {
        !bytes
            .windows(token.len())
            .any(|part| part == token.as_bytes())
    }));
    assert!(
        client(origin, "invalid-bearer-token", ca, &cache)
            .clone_into(&target, space, "Unauthorized")
            .is_err()
    );
    assert_eq!(
        before,
        saved
            .iter()
            .map(|p| std::fs::read(p).unwrap())
            .collect::<Vec<_>>()
    );
    let start = resumed.load(Ordering::SeqCst);
    // A distinct OS process gets a fresh bootstrap, plan and capabilities. No
    // in-memory NativeRemote, spool handle or token is inherited by the cache.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "durable_download_child",
            "--ignored",
            "--nocapture",
        ])
        .env("LAYRS_DURABLE_CHILD_ROOT", &target)
        .env("LAYRS_DURABLE_CHILD_CACHE", &cache)
        .env("LAYRS_DURABLE_CHILD_ORIGIN", origin)
        .env("LAYRS_DURABLE_CHILD_TOKEN", token)
        .env("LAYRS_DURABLE_CHILD_CA", ca)
        .env("LAYRS_DURABLE_CHILD_SPACE", space.as_str())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "durable download child failed (output intentionally omitted)"
    );
    assert!(
        resumed.load(Ordering::SeqCst) > start,
        "new process must reuse durable prefix"
    );
    assert!(NativeWorkspace::open(&target).is_ok());
    assert!(records(&cache).is_empty());

    let corrupt_target = root.join("durable-corrupt-clone");
    std::fs::create_dir(&corrupt_target).unwrap();
    interrupted.store(4, Ordering::SeqCst);
    assert!(
        client(origin, token, ca, &cache)
            .clone_into(&corrupt_target, space, "Corrupt")
            .is_err()
    );
    let saved = records(&cache);
    let mut bytes = std::fs::read(&saved[0]).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    std::fs::write(&saved[0], bytes).unwrap();
    assert!(
        client(origin, token, ca, &cache)
            .clone_into(&corrupt_target, space, "Corrupt")
            .is_err()
    );
    assert_eq!(std::fs::read_dir(&corrupt_target).unwrap().count(), 0);
    client(origin, token, ca, &cache)
        .clone_into(&corrupt_target, space, "Fresh retry")
        .unwrap();

    let epoch_target = root.join("durable-epoch-clone");
    std::fs::create_dir(&epoch_target).unwrap();
    interrupted.store(4, Ordering::SeqCst);
    assert!(
        client(origin, token, ca, &cache)
            .clone_into(&epoch_target, space, "Old epoch")
            .is_err()
    );
    let saved = records(&cache);
    assert!(!saved.is_empty());
    let http = reqwest::blocking::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
        .build()
        .unwrap();
    let account:serde_json::Value=http.post(format!("{origin}/v3/auth/accounts")).bearer_auth(token)
        .json(&serde_json::json!({"login":"durable-observer@example.invalid","password":"durable fixture observer password","displayName":"Durable observer"}))
        .send().unwrap().error_for_status().unwrap().json().unwrap();
    http.post(format!("{origin}/v3/spaces/{space}/members"))
        .bearer_auth(token)
        .json(&serde_json::json!({"principalId":account["principalId"],"role":"auditor"}))
        .send()
        .unwrap()
        .error_for_status()
        .unwrap();
    let start = resumed.load(Ordering::SeqCst);
    client(origin, token, ca, &cache)
        .clone_into(&epoch_target, space, "New epoch")
        .unwrap();
    assert_eq!(
        start,
        resumed.load(Ordering::SeqCst),
        "new epoch must not consume old authority prefix"
    );
    assert!(
        saved.iter().all(|p| p.exists()),
        "old authority remains bounded abandoned cache, never reused"
    );
    eprintln!(
        "Durable TLS spool: new OS process resumes exact prefix; revoked bearer, corruption, epoch isolation and no premature local Store passed"
    );
}

pub fn child() {
    if std::env::var_os("LAYRS_DURABLE_CHILD_ROOT").is_none() {
        return;
    }
    let value = |name| std::env::var(name).expect("missing durable child configuration");
    let root = value("LAYRS_DURABLE_CHILD_ROOT");
    let cache = value("LAYRS_DURABLE_CHILD_CACHE");
    let remote = client(
        &value("LAYRS_DURABLE_CHILD_ORIGIN"),
        &value("LAYRS_DURABLE_CHILD_TOKEN"),
        &value("LAYRS_DURABLE_CHILD_CA"),
        Path::new(&cache),
    );
    let space = SpaceId::parse(value("LAYRS_DURABLE_CHILD_SPACE")).unwrap();
    assert!(
        remote.clone_into(root, &space, "After restart").is_ok(),
        "authorized restart clone failed"
    );
}
