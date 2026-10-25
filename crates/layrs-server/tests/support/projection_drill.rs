use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
#[path = "projection_selection_drill.rs"]
mod selection;

pub(super) fn verify(
    read: &Http<'_>,
    remote: &NativeRemote,
    base: &SpaceBootstrap,
    step: &StepId,
) -> ProjectionPageRequest {
    let mut request = ProjectionPageRequest {
        step_id: step.clone(),
        expected_policy_set_id: base.current_policy_set_id.clone(),
        expected_authorization_epoch: base.authorization_epoch,
        cursor: None,
        limit: 1,
    };
    let mut entries = std::collections::BTreeMap::new();
    let mut offset = 0;
    let mut saved = None;
    loop {
        let page = remote.projection_page(&read.space, &request).unwrap();
        assert_eq!(page.principal_id, base.principal_id);
        assert_eq!(page.offset, offset);
        assert_eq!(page.step_id, *step);
        assert!(!page.entries.is_empty());
        offset += page.entries.len() as u32;
        for entry in page.entries {
            assert!(!entry.writable, "v2 restricted remains read-only");
            assert!(entries.insert(entry.path.clone(), entry).is_none());
        }
        let Some(cursor) = page.next_cursor else {
            break;
        };
        request.cursor = Some(cursor);
        saved.get_or_insert(request.clone());
        assert!(offset < 100, "fixture page loop must terminate");
    }
    let allowed = entries
        .get(&RepoPath::parse("public/allowed.txt").unwrap())
        .unwrap();
    let blocked = entries
        .get(&RepoPath::parse("public/blocked.txt").unwrap())
        .unwrap();
    let private = entries
        .get(&RepoPath::parse("private.txt").unwrap())
        .unwrap();
    assert!(allowed.content_readable);
    assert!(!blocked.content_readable);
    assert!(!private.content_readable);
    assert_eq!(
        allowed.target, private.target,
        "same FileId, different rights"
    );
    selection::clone_and_expand(read, remote, step);
    let saved = saved.unwrap();
    let mut forged = saved.clone();
    let mut cursor: Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(forged.cursor.as_ref().unwrap())
            .unwrap(),
    )
    .unwrap();
    cursor["principalId"] = json!("principal:other-member");
    forged.cursor = Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor).unwrap()));
    assert!(
        !read
            .call(
                "POST",
                &format!("/v3/spaces/{}/projection", read.space),
                serde_json::to_value(forged).unwrap()
            )
            .status()
            .is_success()
    );
    let mut invalid_limit = saved.clone();
    invalid_limit.limit = 201;
    assert!(
        !read
            .call(
                "POST",
                &format!("/v3/spaces/{}/projection", read.space),
                serde_json::to_value(invalid_limit).unwrap()
            )
            .status()
            .is_success()
    );
    saved
}

pub(super) fn selected_publication(
    remote: &NativeRemote,
    read: &Http<'_>,
    layer: &layrs_core::scm::LayerId,
) {
    selection::publication(remote, read, layer);
}

pub(super) fn stale(read: &Http<'_>, request: ProjectionPageRequest) {
    assert_eq!(
        read.call(
            "POST",
            &format!("/v3/spaces/{}/projection", read.space),
            serde_json::to_value(request).unwrap()
        )
        .status()
        .as_u16(),
        409
    );
}
