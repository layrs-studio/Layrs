use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

struct Checkout(PathBuf);
impl Checkout {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "layrs-cli-rename-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn command(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_layrs"))
            .current_dir(&self.0)
            .arg("--json")
            .args(args)
            .env_remove("LAYRS_LENS_WORKER")
            .env_remove("LAYRS_LENS_WORKER_SHA256")
            .output()
            .unwrap()
    }
    fn run(&self, args: &[&str]) -> Value {
        let out = self.command(args);
        assert!(
            out.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"].clone()
    }
}
impl Drop for Checkout {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[test]
fn cli_rename_refuses_single_file_reads_and_resolves_all_coupled_paths() {
    let checkout = Checkout::new();
    fs::write(checkout.0.join("old"), b"same").unwrap();
    checkout.run(&["init", "Rename"]);
    checkout.run(&["layer", "create", "topic"]);
    checkout.run(&["layer", "use", "topic"]);
    fs::rename(checkout.0.join("old"), checkout.0.join("new")).unwrap();
    let source = checkout.run(&["step"])["stepId"]
        .as_str()
        .unwrap()
        .to_owned();
    checkout.run(&["layer", "use", "main"]);
    fs::write(checkout.0.join("old"), b"changed").unwrap();
    checkout.run(&["step"]);
    assert_eq!(
        checkout.run(&["weave", "topic", "--target", "main"])["weave"]["materialized"],
        false
    );
    let conflicts = checkout.run(&["weave", "conflicts"]);
    assert_eq!(conflicts["conflicts"].as_array().unwrap().len(), 1);
    let conflict = &conflicts["conflicts"][0];
    let id = conflict["conflictId"].as_str().unwrap();
    assert_eq!(conflict["document"]["kind"], "rename");
    assert_eq!(
        conflict["document"]["placements"].as_array().unwrap().len(),
        2
    );
    let term = conflict["document"]["sides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["provenance"]["stepId"] == source)
        .unwrap()["termId"]
        .as_str()
        .unwrap();
    let replacement = Checkout::new();
    let edited = replacement.0.join("edited");
    fs::write(&edited, b"manual").unwrap();
    let before = checkout.run(&["timeline"]);
    for args in [
        vec!["conflict-variant", id, term, "--layer", "main"],
        vec![
            "conflict-variant",
            id,
            term,
            "--against",
            term,
            "--layer",
            "main",
        ],
        vec![
            "resolve-file",
            id,
            edited.to_str().unwrap(),
            "--layer",
            "main",
        ],
    ] {
        assert!(!checkout.command(&args).status.success());
    }
    assert_eq!(checkout.run(&["timeline"]), before);
    assert_eq!(fs::read(checkout.0.join("old")).unwrap(), b"changed");
    assert!(!checkout.0.join("new").exists());
    let preview = checkout.run(&["resolve-term", id, term, "--layer", "main", "--preview"]);
    assert!(preview["conflicts"].as_array().unwrap().is_empty());
    assert_eq!(checkout.run(&["timeline"]), before);
    let resolved = checkout.run(&["resolve-term", id, term, "--layer", "main"]);
    assert_eq!(resolved["resolution"]["materialized"], true);
    assert!(!checkout.0.join("old").exists());
    assert_eq!(fs::read(checkout.0.join("new")).unwrap(), b"same");
    checkout.run(&["undo"]);
    assert_eq!(
        checkout.run(&["weave", "conflicts"])["conflicts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(fs::read(checkout.0.join("old")).unwrap(), b"changed");
    assert!(!checkout.0.join("new").exists());
}
