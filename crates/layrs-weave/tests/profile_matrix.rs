#![cfg(any(windows, all(target_os = "linux", target_arch = "x86_64")))]
//! Same fixed fixture digest on Windows and Linux; receipt host policy digests
//! deliberately differ because they also pin each platform's worker executable.
use layrs_lens_sdk::{
    LensReconcileLabelsContract, LensReconcileRequest, LensReconcileSideContract as Side,
    sha256_digest,
};
use layrs_weave::{
    reconciliation::*,
    wasm_host::{self, SandboxLimits, WasmHost},
};
use serde_json::json;
type Fixture<'a> = (
    &'a str,
    Option<&'a [u8]>,
    Option<&'a [u8]>,
    Option<&'a [u8]>,
);

#[test]
fn supported_text_code_profiles_preserve_every_variant_and_match_cross_os_golden() {
    let executable = std::path::Path::new(env!("CARGO_BIN_EXE_layrs-lens-worker"));
    let pin = sha256_digest(&std::fs::read(executable).unwrap());
    let host = WasmHost::open(executable, &pin, SandboxLimits::default()).unwrap();
    let policy = FileMergePolicy {
        unsupported: FailureAction::StructuralFallback,
        quota: FailureAction::Reject,
        limits: FileMergeLimits::default(),
    };
    let mut results = Vec::new();
    let fixtures: [Fixture<'_>; 6] = [
        (
            "crlf_no_final",
            Some("é\r\nmiddle\r\nend".as_bytes()),
            Some("É\r\nmiddle\r\nend".as_bytes()),
            Some("é\r\nmiddle\r\nEND".as_bytes()),
        ),
        (
            "edit_conflict",
            Some(b"base\nbase\n"),
            Some(b"left\nbase\n"),
            Some(b"right\nbase\n"),
        ),
        ("add_add", None, Some(b""), Some(b"added")),
        ("delete_modify", Some(b"before"), None, Some(b"changed")),
        ("binary", Some(b"\0\xff"), Some(b"\0\xfe"), Some(b"\0\xfd")),
        ("invalid_utf8", Some(b"\xff"), Some(b"\xfe"), Some(b"\xfd")),
    ];
    for lens in [NativeLens::Text, NativeLens::Code] {
        for (name, base, left, right) in fixtures {
            let side = |bytes: Option<&[u8]>| {
                bytes.map_or_else(Side::absent, |b| Side::present(b, Some(sha256_digest(b))))
            };
            let mut request = LensReconcileRequest::new(
                native_manifest(lens).pin().unwrap(),
                if lens == NativeLens::Text {
                    "é.txt"
                } else {
                    "é.ts"
                },
                None,
                side(base),
                side(left),
                side(right),
                LensReconcileLabelsContract {
                    existing: "A".into(),
                    incoming: "B".into(),
                },
            );
            let expected = reconcile_file(&request, policy).unwrap();
            request.lens = wasm_host::manifest(lens).pin().unwrap();
            let result = host.reconcile(&request, policy).unwrap();
            assert_eq!(result.outcome, expected.outcome, "{lens:?}/{name}");
            for (choice, bytes) in [
                (FileValueChoice::Base, base),
                (FileValueChoice::Existing, left),
                (FileValueChoice::Incoming, right),
            ] {
                let chosen = result.choose_value(choice);
                assert_eq!(chosen.exists, bytes.is_some());
                assert_eq!(chosen.bytes, bytes.unwrap_or_default());
            }
            results.push(json!({"lens":request.lens,"fixture":name,"outcome":result.outcome,"base":result.choose_value(FileValueChoice::Base),"existing":result.choose_value(FileValueChoice::Existing),"incoming":result.choose_value(FileValueChoice::Incoming)}));
        }
    }
    let bytes = layrs_core::scm::canonical_json(&json!(results)).unwrap();
    assert_eq!(
        sha256_digest(&bytes),
        "sha256:73a2110ef2073f565b96cdbb69a760ae988b94836746a1ee1d5855924900f125"
    );
}
