use super::*;

#[test]
fn ordinary_and_registration_reads_enforce_the_same_expected_tokens() {
    let root =
        std::env::temp_dir().join(format!("layrs-registration-read-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let workspace = crate::native_scm::NativeWorkspace::initialize(&root, "Read bindings").unwrap();
    let client = workspace.client();
    let reads: [fn(&NativeClient) -> Result<layrs_store_local::scm_v1::NativeStoreOpen>; 2] = [
        NativeClient::open_store,
        NativeClient::open_store_for_registration,
    ];
    for read in reads {
        let store = read(client).unwrap();
        assert_eq!(store.store_id(), &client.store_id);
        store.verify_filesystem_identity().unwrap();
        drop(store);
        // These are wrong expected-token tests, not filesystem replacement tests.
        for field in 0..3 {
            let mut wrong = client.clone();
            match field {
                0 => wrong.store_identity = "fs:v1:wrong".into(),
                1 => wrong.metadata_identity = "fs:v1:wrong".into(),
                _ => wrong.store_id = layrs_core::scm::StoreId::parse("store:other").unwrap(),
            }
            let error = read(&wrong).unwrap_err();
            if field == 2 {
                assert!(
                    matches!(
                        error,
                        NativeClientError::InvalidState("Store identity changed")
                    ),
                    "{error:?}"
                );
            } else {
                assert!(
                    matches!(error, NativeClientError::Filesystem(_)),
                    "{error:?}"
                );
                assert!(
                    error
                        .to_string()
                        .contains("Store filesystem identity changed"),
                    "{error}"
                );
            }
        }
    }
    drop(workspace);
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    std::fs::remove_dir_all(root).unwrap();
}
