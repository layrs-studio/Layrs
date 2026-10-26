use super::*;
use crate::projection::provider::{database_io::with_reference, owned_test as child};
use layrs_vfs::Cancellation;

#[test]
#[ignore = "owned audited SQLite child; exact family v1/v2 admission and unsupported-stage fences"]
fn schema_dispatch_is_exact_and_exact_stages_block_ordinary_access() {
    child::run(
        concat!(
            module_path!(),
            "::schema_dispatch_is_exact_and_exact_stages_block_ordinary_access"
        ),
        120,
        || {
            child::declare();
            with_reference(SQL, |c| {
                assert_eq!(require_catalog_schema(c).unwrap(), CatalogSchema::V1);
                assert_eq!(
                    provider_access::require_provider_catalog_schema(c, None)?,
                    CatalogSchema::V1
                );
                require_no_exact_installations(c, CatalogSchema::V1).unwrap();
                c.execute_batch("PRAGMA user_version=2")?;
                assert!(
                    require_catalog_schema(c).is_err(),
                    "version alone must never migrate a schema"
                );
                assert!(provider_access::require_provider_catalog_schema(c, None).is_err());
                c.execute_batch("PRAGMA user_version=1; PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                schema_v2::migrate(c, &Cancellation::default())?;
                assert_eq!(require_catalog_schema(c).unwrap(), CatalogSchema::V2);
                assert_eq!(
                    provider_access::require_provider_catalog_schema(c, None)?,
                    CatalogSchema::V2
                );
                require_no_exact_installations(c, CatalogSchema::V2).unwrap();
                c.execute_batch("CREATE INDEX foreign_family_index ON files(bytes)")?;
                assert!(require_catalog_schema(c).is_err());
                assert!(provider_access::require_provider_catalog_schema(c, None).is_err());
                c.execute_batch("DROP INDEX foreign_family_index")?;
                c.execute(
                    "INSERT INTO worktrees VALUES('owner','member','database',?1,'ready')",
                    [b"marker".as_slice()],
                )?;
                c.execute(
                    "INSERT INTO reservations VALUES('reservation','owner','operation',1024)",
                    [],
                )?;
                let id = format!("blake3:{}", "1".repeat(64));
                c.execute(
                    "INSERT INTO exact_installations VALUES('reservation',?1,?1,0,?2,NULL,0)",
                    params![id, b"intent".as_slice()],
                )?;
                assert!(
                    require_no_exact_installations(c, CatalogSchema::V2).is_err(),
                    "ordinary readers and GC must not ignore an exact stage"
                );
                assert!(provider_access::require_no_owner_exact_installations(
                    c,
                    CatalogSchema::V2,
                    "owner"
                )
                .is_err());
                provider_access::require_no_owner_exact_installations(
                    c,
                    CatalogSchema::V2,
                    "other-owner",
                )?;
                c.execute_batch("ROLLBACK")?;
                assert_eq!(require_catalog_schema(c).unwrap(), CatalogSchema::V1);
                Ok(())
            })
            .unwrap();
            with_reference("", |c| {
                c.execute_batch("BEGIN DEFERRED")?;
                assert!(
                    schema_v2::initialize_reference(c).is_err(),
                    "reference constructor cannot commit an existing caller transaction"
                );
                assert!(!c.is_autocommit());
                c.execute_batch("ROLLBACK")?;
                Ok(())
            })
            .unwrap();
        },
    );
}
