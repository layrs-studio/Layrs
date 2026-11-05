use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let repo = manifest
        .parent()
        .and_then(|path| path.parent())
        .and_then(|path| path.parent())
        .expect("desktop is inside repository");
    let profile = env::var("PROFILE").expect("Cargo profile");
    let executable = if cfg!(windows) {
        "layrs-lens-worker.exe"
    } else {
        "layrs-lens-worker"
    };
    let source = repo.join("target").join(&profile).join(executable);
    println!("cargo:rerun-if-changed={}", source.display());
    let bytes = fs::read(&source).unwrap_or_else(|error| {
        panic!(
        "trusted Lens worker is missing at {} ({error}); build layrs-weave worker before Desktop",
        source.display()
    )
    });
    let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
    let resources = manifest.join("resources");
    fs::create_dir_all(&resources).expect("create Desktop resources directory");
    let bundled = resources.join("layrs-lens-worker.exe");
    fs::copy(&source, &bundled)
        .expect("bundle exact trusted Lens worker image and executable permissions");
    println!("cargo:rustc-env=LAYRS_BUNDLED_WORKER_SHA256={digest}");
    println!(
        "cargo:rustc-env=LAYRS_DEBUG_WORKER_PATH={}",
        source.display()
    );
    tauri_build::build();
}
