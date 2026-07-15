use super::*;

#[test]
fn code_and_image_reconcile_are_structured_unsupported() {
    for (lens_id, path, media_type) in [
        (layrs_lens_code::CODE_LENS_ID, "src/main.rs", "text/rust"),
        (layrs_lens_image::IMAGE_LENS_ID, "image.png", "image/png"),
    ] {
        let row = conflict_row(lens_id, path);
        let result = reconcile_weave_conflict(
            &row,
            &side(b"base", media_type),
            &side(b"ours", media_type),
            &side(b"theirs", media_type),
        );

        assert_eq!(result.status, LensReconcileResultStatus::Unsupported);
        assert!(result.file_methods.is_empty());
        assert!(lens_resolution_methods(lens_id).file.is_empty());
        let error = ensure_resolution_method_allowed(
            lens_id,
            "file",
            path,
            ResolutionMethod::Existing,
            &[],
        )
        .expect_err("unsupported Lens must fail closed");
        assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error.code, "lens_reconcile_unsupported");
    }
}

#[test]
fn text_and_raw_dispatch_to_their_own_contracts() {
    let text_row = conflict_row(layrs_lens_text::TEXT_LENS_ID, "notes/readme.txt");
    let text = reconcile_weave_conflict(
        &text_row,
        &side(b"base\n", "text/plain"),
        &side(b"base\n", "text/plain"),
        &side(b"incoming\n", "text/plain"),
    );
    assert_eq!(text.status, LensReconcileResultStatus::AutoResolved);
    assert_eq!(text.resolved.expect("text result").bytes, b"incoming\n");
    assert!(
        lens_resolution_methods(layrs_lens_text::TEXT_LENS_ID)
            .file
            .is_empty()
    );
    assert_eq!(
        lens_resolution_methods(layrs_lens_text::TEXT_LENS_ID).block,
        layrs_lens_text::resolution_methods().block
    );

    let raw_row = conflict_row(layrs_lens_raw::RAW_LENS_ID, "archive.bin");
    let raw = reconcile_weave_conflict(
        &raw_row,
        &side(b"base", "application/octet-stream"),
        &side(b"base", "application/octet-stream"),
        &side(b"incoming", "application/octet-stream"),
    );
    assert_eq!(raw.status, LensReconcileResultStatus::AutoResolved);
    assert_eq!(raw.resolved.expect("raw result").bytes, b"incoming");
    assert_eq!(
        lens_resolution_methods(layrs_lens_raw::RAW_LENS_ID).file,
        layrs_lens_raw::resolution_methods().file
    );
}

fn conflict_row(lens_id: &str, path: &str) -> WeaveConflictRow {
    WeaveConflictRow {
        conflict_id: "conflict-1".to_string(),
        logical_path: path.to_string(),
        lens_id: lens_id.to_string(),
        status: "open".to_string(),
        message: String::new(),
        base_file_object_id: None,
        ours_file_object_id: None,
        theirs_file_object_id: None,
        resolved_file_object_id: None,
        source_layer_id: "source".to_string(),
        target_layer_id: "target".to_string(),
    }
}

fn side(bytes: &[u8], media_type: &str) -> WeaveConflictSide {
    WeaveConflictSide {
        exists: true,
        digest: None,
        media_type: Some(media_type.to_string()),
        bytes: bytes.to_vec(),
    }
}
