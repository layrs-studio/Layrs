use layrs_store_local::download_cache::DownloadCacheLease;
use std::{fs, io::Read, path::PathBuf};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "layrs-download-cache-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn checkpoints_are_private_persistent_immutable_and_quota_accounts_for_abandoned_records() {
    let fixture = Fixture::new();
    let path = fixture.0.join("cache");
    let binding = "a".repeat(64);
    let first = DownloadCacheLease::open(&path, &binding, 2048).unwrap();
    first.append(0, &vec![7; 1200]).unwrap();
    assert!(first.append(0, b"must not overwrite").is_err());
    drop(first);
    let reopened = DownloadCacheLease::open(&path, &binding, 2048).unwrap();
    let mut bytes = vec![];
    reopened
        .record(0)
        .unwrap()
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, vec![7; 1200]);
    assert!(
        reopened.append(1, &vec![8; 1200]).is_err(),
        "own active prefix counts toward quota"
    );
    drop(reopened);
    let other = DownloadCacheLease::open(&path, &"b".repeat(64), 2048).unwrap();
    other.append(0, &vec![9; 1200]).unwrap();
    assert!(
        !path.join(format!("entry-{binding}")).exists(),
        "only abandoned cache entry is evicted"
    );
    other.clear().unwrap();
    assert!(other.record(0).unwrap().is_none());
}
#[test]
fn foreign_cache_content_is_preserved_and_rejected() {
    let fixture = Fixture::new();
    let path = fixture.0.join("cache");
    let lease = DownloadCacheLease::open(&path, &"a".repeat(64), 4096).unwrap();
    let foreign = path
        .join(format!("entry-{}", "a".repeat(64)))
        .join("user-history.txt");
    fs::write(&foreign, b"keep exact").unwrap();
    assert!(lease.clear().is_err());
    assert_eq!(fs::read(foreign).unwrap(), b"keep exact");
}

#[cfg(windows)]
#[test]
fn windows_weakened_acl_is_refused_without_reading_or_removing_records() {
    let fixture = Fixture::new();
    let path = fixture.0.join("cache");
    let binding = "a".repeat(64);
    let lease = DownloadCacheLease::open(&path, &binding, 4096).unwrap();
    lease.append(0, b"private prefix").unwrap();
    drop(lease);
    let output = std::process::Command::new("icacls.exe")
        .arg(&path)
        .args(["/grant", "*S-1-1-0:(OI)(CI)R"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(DownloadCacheLease::open(&path, &binding, 4096).is_err());
    assert_eq!(
        fs::read(path.join(format!("entry-{binding}")).join("part-0000")).unwrap(),
        b"private prefix"
    );
}
#[cfg(unix)]
#[test]
fn unix_permissions_and_symlink_targets_are_never_weakened() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new();
    let path = fixture.0.join("cache");
    let lease = DownloadCacheLease::open(&path, &"a".repeat(64), 4096).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    drop(lease);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(DownloadCacheLease::open(&path, &"a".repeat(64), 4096).is_err());
    let link = fixture.0.join("link");
    symlink(&path, &link).unwrap();
    assert!(DownloadCacheLease::open(&link, &"a".repeat(64), 4096).is_err());
}
