fn numbered_lines(prefix: &str, count: usize) -> String {
    let mut text = String::new();
    for index in 0..count {
        text.push_str(prefix);
        text.push('-');
        text.push_str(&index.to_string());
        text.push('\n');
    }
    text
}

fn directory_file_size(path: &Path, extension: Option<&str>) -> u64 {
    if !path.exists() {
        return 0;
    }
    fs::read_dir(path)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && extension.is_none_or(|extension| {
                    path.extension().and_then(|value| value.to_str()) == Some(extension)
                })
        })
        .filter_map(|path| fs::metadata(path).ok().map(|metadata| metadata.len()))
        .sum()
}

fn directory_file_count(path: &Path, extension: Option<&str>) -> usize {
    if !path.exists() {
        return 0;
    }
    fs::read_dir(path)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && extension.is_none_or(|extension| {
                    path.extension().and_then(|value| value.to_str()) == Some(extension)
                })
        })
        .count()
}

// Each historical test runs in a child process: APPDATA changes must never
// race another test or leak into the main test process/user configuration.
fn isolated_legacy_test(name: &str) -> bool {
    let full_name = format!("access_registry::tests::{name}");
    if std::env::var("LAYRS_LEGACY_UNIT_CHILD").as_deref() == Ok(full_name.as_str()) {
        return true;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &full_name, "--nocapture", "--test-threads=1"])
        .env("LAYRS_LEGACY_UNIT_CHILD", &full_name)
        .output()
        .expect("isolated legacy unit test");
    assert!(
        output.status.success(),
        "{full_name} failed in isolated subprocess:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

fn unique_test_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = env::temp_dir().join(format!("layrs-{name}-{}-{nonce}", std::process::id()));
    fs::create_dir(&path).expect("fresh unique test directory");
    fs::canonicalize(path).expect("canonical test root")
}
