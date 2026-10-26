//! Exact product executable supplied by ROOT's retained artifact audit. No
//! process profile is inherited: the fresh CLI must declare its own profile.
use super::*;
use layrs_core::scm::canonical_json;
use std::{
    fs::{self, OpenOptions},
    os::windows::{fs::OpenOptionsExt, process::CommandExt},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn witness(f: &Fixture) -> [(String, u64, String); 2] {
    [
        f.family.root().join("metadata/catalog.sqlite3"),
        f.metadata.join("projection.sqlite3"),
    ]
    .map(|path| crate::projection::provider::database_io::tests::support::witness(&path))
}

struct OwnedChild(std::process::Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn invoke(
    exe: &Path,
    f: &Fixture,
    ticket: &Path,
    name: &str,
    json: bool,
) -> (i32, serde_json::Value) {
    let output = f.outer.join(format!("cli-{name}.out"));
    let errors = f.outer.join(format!("cli-{name}.err"));
    let mut command = Command::new(exe);
    if json {
        command.arg("--json");
    }
    command
        .args(["partial", "provider", "status", "--ticket"])
        .arg(ticket)
        .current_dir(&f.outer)
        .stdin(Stdio::null())
        .stdout(
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&output)
                .unwrap(),
        )
        .stderr(
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&errors)
                .unwrap(),
        )
        .creation_flags(0x08000000);
    let mut child = OwnedChild(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "owned product CLI timed out");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(fs::metadata(&output).unwrap().len() <= 16 * 1024);
    assert!(fs::metadata(&errors).unwrap().len() <= 16 * 1024);
    let value = serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
    (status.code().unwrap(), value)
}

#[test]
#[ignore = "ROOT must release the exact retained product EXE and libtest host before this owned recipe"]
fn audited_product_status_reopens_and_refuses_foreign_ticket_without_writes() {
    child::run(
        concat!(
            module_path!(),
            "::audited_product_status_reopens_and_refuses_foreign_ticket_without_writes"
        ),
        180,
        || {
            child::declare();
            hooks::reset();
            let exe = PathBuf::from(
                std::env::var_os("LAYRS_QUALIFIED_PROVIDER_CLI")
                    .expect("retained audited product executable"),
            );
            assert!(exe.is_absolute());
            assert_eq!(exe.file_name().unwrap(), "layrs.exe");
            // The release wrapper verifies the retained SHA-256 before this test.
            // Hold a read-only sharing handle while every exact child runs.
            let _executable_pin = OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(&exe)
                .unwrap();
            let (f, active, _, _, binding) = capture_family::active_holder_with_private();
            let expected_catalog = active.catalog_id().to_owned();
            let expected_digest = active.digest().to_owned();
            drop(active);
            let mut ticket = serde_json::json!({
                "schemaVersion": "layrs.partial-provider-status-ticket.v1",
                "familyRoot": f.family.root(), "worktreeId": f.worktree,
                "transactionId": fixture::TX, "expectedBinding": binding,
            });
            let path = f.outer.join("provider-ticket.json");
            fs::write(&path, canonical_json(&ticket).unwrap()).unwrap();
            let before = witness(&f);
            let (code, result) = invoke(&exe, &f, &path, "json", true);
            assert_eq!(code, 0, "{result}");
            assert_eq!(result["ok"], true);
            assert_eq!(result["data"]["state"], "active");
            assert_eq!(result["data"]["root"], serde_json::json!(f.checkout));
            assert_eq!(result["data"]["catalogId"], expected_catalog);
            assert_eq!(result["data"]["generation"], 1);
            assert_eq!(result["data"]["digest"], expected_digest);
            assert_eq!(witness(&f), before);
            let (code, human) = invoke(&exe, &f, &path, "human", false);
            assert_eq!(code, 0);
            assert_eq!(human, result["data"]);
            assert_eq!(witness(&f), before);
            ticket["transactionId"] = serde_json::json!("foreign-transaction");
            fs::write(&path, canonical_json(&ticket).unwrap()).unwrap();
            let (code, refused) = invoke(&exe, &f, &path, "foreign", true);
            assert_ne!(code, 0);
            assert_eq!(refused["ok"], false);
            assert!(
                refused["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("ticket_binding_inconsistent")
            );
            assert_eq!(witness(&f), before);
            ticket["transactionId"] = serde_json::json!(fixture::TX);
            ticket["expectedBinding"]["recovery"]["familyMarkerDigest"] =
                serde_json::json!(format!("blake3:{}", "0".repeat(64)));
            fs::write(&path, canonical_json(&ticket).unwrap()).unwrap();
            let (code, refused) = invoke(&exe, &f, &path, "binding", true);
            assert_ne!(code, 0);
            assert_eq!(refused["ok"], false);
            let message = refused["error"]["message"].as_str().unwrap();
            assert!(message.contains("needs_repair"), "{message}");
            assert!(
                message.contains("expected bootstrap binding changed"),
                "{message}"
            );
            assert_eq!(witness(&f), before);

            let preparing = Fixture::new();
            let published = preparing.published();
            let binding = serde_json::to_value(&published.held.binding).unwrap();
            let outcome = published.prepare_provider([0x64; 16]);
            assert!(matches!(
                &outcome,
                super::super::preparation::ProviderPreparationOutcome::Prepared(_)
            ));
            drop(outcome);
            let ticket = serde_json::json!({
                "schemaVersion": "layrs.partial-provider-status-ticket.v1",
                "familyRoot": preparing.family.root(), "worktreeId": preparing.worktree,
                "transactionId": fixture::TX, "expectedBinding": binding,
            });
            let path = preparing.outer.join("preparing-ticket.json");
            fs::write(&path, canonical_json(&ticket).unwrap()).unwrap();
            let before = witness(&preparing);
            let (code, refused) = invoke(&exe, &preparing, &path, "preparing", true);
            assert_ne!(code, 0);
            assert_eq!(refused["ok"], false);
            let message = refused["error"]["message"].as_str().unwrap();
            assert!(message.contains("needs_repair"), "{message}");
            assert!(
                message.contains("database=Some(PrivateMetadata)"),
                "{message}"
            );
            assert!(
                message.contains("requires exactly one projection_snapshots row"),
                "{message}"
            );
            assert_eq!(witness(&preparing), before);
        },
    );
}
