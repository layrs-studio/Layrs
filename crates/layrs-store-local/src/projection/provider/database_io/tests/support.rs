use crate::secure_fs::PinnedDirectory;
use std::{fs, path::PathBuf};

pub(in crate::projection::provider) fn owned_root(label: &str) -> PathBuf {
    assert!(
        label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    );
    let parent = PathBuf::from(
        std::env::var_os("LAYRS_VFS_TEST_PARENT")
            .expect("UNQUALIFIED: absolute disposable LAYRS_VFS_TEST_PARENT required"),
    );
    assert!(parent.is_absolute());
    let parent_pin = PinnedDirectory::open(&parent).expect("existing no-follow disposable parent");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = parent.join(format!(
        "layrs-provider-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&root).expect("exclusive owned fixture root");
    let preflight = layrs_vfs::ControlRootPreflight::open(&root)
        .expect("ordinary fresh root outside sync roots");
    let canonical = preflight.root().to_path_buf();
    parent_pin.verify_path_identity().unwrap();
    create(
        &canonical.join("fixture-owner.txt"),
        format!("{label}:{}:{nonce}", std::process::id()).as_bytes(),
    );
    println!("RETAINED_PROVIDER_FIXTURE={}", canonical.display());
    canonical
}

pub(in crate::projection::provider) fn create(path: &std::path::Path, bytes: &[u8]) {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

pub(in crate::projection::provider) fn witness(path: &std::path::Path) -> (String, u64, String) {
    let file = crate::layout::open_read_no_follow(path).unwrap();
    let identity = crate::projection::file::identity(&file).unwrap();
    let len = file.metadata().unwrap().len();
    assert!(len <= 16 * 1024 * 1024, "bounded fixture witness");
    let bytes = fs::read(path).unwrap();
    (identity, len, blake3::hash(&bytes).to_hex().to_string())
}

/// Real unprivileged NTFS junction, using the already qualified constant-command
/// transport. This is test setup only; no native allocator/ACL bypass is copied.
pub(super) fn junction(root: &std::path::Path, link: &std::path::Path, target: &std::path::Path) {
    use super::super::super::process_profile::tests::child::OwnedChild;
    use std::{
        os::windows::{fs::MetadataExt, process::CommandExt},
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    for p in [root, link, target] {
        assert!(p.is_absolute() && p.starts_with(root));
        assert!(
            !p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        );
        let s = p.to_str().expect("Unicode owned junction path");
        assert!(
            s.len() <= 4096
                && !s.chars().any(|c| c.is_control()
                    || matches!(c, '"' | '%' | '!' | '^' | '&' | '|' | '<' | '>'))
        );
    }
    assert!(target.is_dir());
    assert_eq!(
        fs::symlink_metadata(link).unwrap_err().raw_os_error(),
        Some(2)
    );
    let stdout = root.join("junction.stdout.log");
    let stderr = root.join("junction.stderr.log");
    let output = |p: &std::path::Path| {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(p)
            .unwrap()
    };
    let system_root = PathBuf::from(std::env::var_os("SystemRoot").expect("Windows SystemRoot"));
    assert!(system_root.is_absolute());
    let mut child = OwnedChild::new(
        Command::new(system_root.join("System32").join("cmd.exe"))
            .args(["/d", "/v:off", "/c"])
            .raw_arg(
                "mklink /J \"%LAYRS_PROVIDER_JUNCTION_LINK%\" \"%LAYRS_PROVIDER_JUNCTION_TARGET%\"",
            )
            .env(
                "LAYRS_PROVIDER_JUNCTION_LINK",
                link.to_str().unwrap().replace('/', "\\"),
            )
            .env(
                "LAYRS_PROVIDER_JUNCTION_TARGET",
                target.to_str().unwrap().replace('/', "\\"),
            )
            .current_dir(root)
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(output(&stdout))
            .stderr(output(&stderr))
            .spawn()
            .unwrap(),
    );
    let start = Instant::now();
    loop {
        for p in [&stdout, &stderr] {
            assert!(
                fs::metadata(p).unwrap().len() <= 65536,
                "bounded junction output"
            );
        }
        if let Some(status) = child.try_wait().unwrap() {
            assert!(
                status.success(),
                "UNQUALIFIED junction transport: {status}; retained logs in {}",
                root.display()
            );
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "UNQUALIFIED junction timeout; owned child reaped on unwind"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_ne!(
        fs::symlink_metadata(link).unwrap().file_attributes() & 0x400,
        0,
        "actual reparse point required"
    );
}
