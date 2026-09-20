//! Evidence for the explicitly unimplemented offline GC precondition.
//! SQL fences cannot currently establish filesystem writer exclusion.
use bytes::Bytes;
use futures_util::{StreamExt, stream};
use layrs_core::scm::{Chunk, ObjectId};
use layrs_store_server::v3::{CasLimits, FsServerCas, PutOutcome, ServerCas};
use layrs_sync::v3::{Compression, ObjectDescriptor, ObjectKind};
use sqlx::PgPool;
use std::{path::Path, time::Duration};

pub async fn prove_sql_fence_does_not_exclude_cas(pool: &PgPool, parent: &Path) {
    // The caller owns this newly-created test root. Nothing is removed here;
    // its ordinary fixture cleanup removes this child after the audit recipe.
    let root = parent.join("exclusion-proof");
    std::fs::create_dir(&root).unwrap();
    let first = FsServerCas::open(&root, CasLimits::default())
        .await
        .unwrap();
    let second = FsServerCas::open(&root, CasLimits::default())
        .await
        .unwrap();
    let mut fence = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE spaces, canonical_objects, object_leases IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *fence)
        .await
        .unwrap();
    let bytes = b"uncatalogued CAS preparation while catalogue is exclusively locked";
    let chunk = Chunk::new(bytes.to_vec()).unwrap();
    let descriptor = ObjectDescriptor {
        kind: ObjectKind::Chunk,
        object_id: ObjectId::parse(chunk.id().as_str()).unwrap(),
        canonical_byte_len: bytes.len() as u64,
        encoded_byte_len: bytes.len() as u64,
        compression: Compression::Identity,
    };
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        second.put_if_absent(
            &descriptor,
            Box::pin(stream::once(async { Ok(Bytes::from_static(bytes)) })),
        ),
    )
    .await
    .expect("CAS preparation is independent of SQL locks in the current contract")
    .unwrap();
    assert_eq!(outcome, PutOutcome::Installed);
    let mut contents = first.verified_stream(&descriptor).await.unwrap();
    let mut actual = Vec::new();
    while let Some(chunk) = contents.next().await {
        actual.extend(chunk.unwrap());
    }
    assert_eq!(actual, bytes);
    let catalogued: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM canonical_objects WHERE object_id=$1)")
            .bind(descriptor.object_id.as_str())
            .fetch_one(&mut *fence)
            .await
            .unwrap();
    assert!(
        !catalogued,
        "the valid preparation has not been admitted yet"
    );
    fence.rollback().await.unwrap();
    eprintln!(
        "GC precondition evidence: exclusive SQL table fences do not exclude CAS preparation; no deletion attempted"
    );
}
