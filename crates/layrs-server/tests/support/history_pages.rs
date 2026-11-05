use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use layrs_store_server::v3::{StepHistoryRequest, StepHistoryResponse};

pub async fn page(
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    space: &SpaceId,
    anchor: &StepId,
    cursor: Option<String>,
    limit: u16,
) -> StepHistoryResponse {
    facade
        .step_history(
            principal,
            space,
            StepHistoryRequest {
                filter: Default::default(),
                path: None,
                anchor: anchor.clone(),
                cursor,
                limit,
            },
        )
        .await
        .unwrap()
}
pub async fn verify(
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    space: &SpaceId,
    anchor: &StepId,
    first: &StepId,
    cursor: String,
) {
    // The other branch was published between the two page reads.
    let second = page(facade, principal, space, anchor, Some(cursor.clone()), 1).await;
    assert_eq!(
        second.steps.iter().map(Step::id).collect::<Vec<_>>(),
        vec![first]
    );
    assert!(second.next.is_none());
    let replay = page(facade, principal, space, anchor, Some(cursor.clone()), 1).await;
    assert_eq!(
        serde_jcs::to_vec(&second).unwrap(),
        serde_jcs::to_vec(&replay).unwrap()
    );
    let whole = page(facade, principal, space, anchor, None, 100).await;
    assert_eq!(
        whole.steps.iter().map(Step::id).collect::<Vec<_>>(),
        vec![anchor, first]
    );
    // One path is deleted on this branch; the immutable root still records its
    // original creation. A new publication between pages cannot alter either.
    let path = layrs_core::scm::RepoPath::parse("docs/readme.txt").unwrap();
    let selected = facade
        .step_history(
            principal,
            space,
            StepHistoryRequest {
                filter: Default::default(),
                anchor: anchor.clone(),
                cursor: None,
                limit: 1,
                path: Some(path.clone()),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        selected.steps.iter().map(Step::id).collect::<Vec<_>>(),
        vec![anchor]
    );
    assert_eq!(selected.path, Some(path.clone()));
    let continuation = selected.next.unwrap();
    let last = facade
        .step_history(
            principal,
            space,
            StepHistoryRequest {
                filter: Default::default(),
                anchor: anchor.clone(),
                cursor: Some(continuation.clone()),
                limit: 1,
                path: Some(path.clone()),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        last.steps.iter().map(Step::id).collect::<Vec<_>>(),
        vec![first]
    );
    assert!(last.next.is_none());
    assert!(
        facade
            .step_history(
                principal,
                space,
                StepHistoryRequest {
                    filter: Default::default(),
                    anchor: anchor.clone(),
                    cursor: Some(continuation),
                    limit: 1,
                    path: None,
                }
            )
            .await
            .is_err(),
        "a cursor must retain the exact path filter"
    );
    for (query, expected) in [("missing.txt", vec![]), ("link", vec![anchor])] {
        let filtered = facade
            .step_history(
                principal,
                space,
                StepHistoryRequest {
                    filter: Default::default(),
                    anchor: anchor.clone(),
                    cursor: None,
                    limit: 100,
                    path: Some(layrs_core::scm::RepoPath::parse(query).unwrap()),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            filtered.steps.iter().map(Step::id).collect::<Vec<_>>(),
            expected
        );
        assert!(filtered.next.is_none());
    }
    for limit in [0, 101] {
        assert!(
            facade
                .step_history(
                    principal,
                    space,
                    StepHistoryRequest {
                        filter: Default::default(),
                        path: None,
                        anchor: anchor.clone(),
                        cursor: None,
                        limit
                    }
                )
                .await
                .is_err()
        );
    }
    let cursor_value: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&cursor).unwrap()).unwrap();
    for (key, value) in [
        ("version", serde_json::json!(3)),
        ("space", serde_json::json!("space:another")),
        ("principal", serde_json::json!("principal:another")),
        ("epoch", serde_json::json!(0)),
        ("offset", serde_json::json!(100001)),
        ("anchor", serde_json::json!(first)),
    ] {
        let mut forged = cursor_value.clone();
        forged[key] = value;
        let cursor = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&forged).unwrap());
        assert!(
            facade
                .step_history(
                    principal,
                    space,
                    StepHistoryRequest {
                        filter: Default::default(),
                        path: None,
                        anchor: anchor.clone(),
                        cursor: Some(cursor),
                        limit: 1
                    }
                )
                .await
                .is_err(),
            "forged {key}"
        );
    }
    for cursor in ["%%%".to_owned(), "a".repeat(1025)] {
        assert!(
            facade
                .step_history(
                    principal,
                    space,
                    StepHistoryRequest {
                        filter: Default::default(),
                        path: None,
                        anchor: anchor.clone(),
                        cursor: Some(cursor),
                        limit: 1
                    }
                )
                .await
                .is_err()
        );
    }
    // Possession of an ID or a cursor does not bypass Space admission or identity.
    let unknown = Step::new(
        vec![anchor.clone()],
        Tree::new(vec![]).unwrap().id().clone(),
    )
    .unwrap();
    assert!(
        facade
            .step_history(
                principal,
                space,
                StepHistoryRequest {
                    filter: Default::default(),
                    path: None,
                    anchor: unknown.id().clone(),
                    cursor: None,
                    limit: 1
                }
            )
            .await
            .is_err()
    );
    let outsider = AuthenticatedPrincipal {
        principal_id: PrincipalId::parse("principal:outside-history").unwrap(),
    };
    assert!(
        facade
            .step_history(
                &outsider,
                space,
                StepHistoryRequest {
                    filter: Default::default(),
                    path: None,
                    anchor: anchor.clone(),
                    cursor: Some(cursor),
                    limit: 1
                }
            )
            .await
            .is_err()
    );
}
