//! Real filesystem ENOSPC, isolated by scripts/test-linux-disk-full.mjs.
#![cfg(target_os = "linux")]
use bytes::Bytes;
use futures_util::{StreamExt, stream};
use layrs_core::scm::{Chunk, ObjectId};
use layrs_store_server::v3::*;
use layrs_sync::v3::{Compression, ObjectDescriptor, ObjectKind};
use std::{
    fs::{self, File},
    io::Write,
    path::Path,
};

fn descriptor(bytes: &[u8]) -> ObjectDescriptor {
    let chunk = Chunk::new(bytes.to_vec()).unwrap();
    ObjectDescriptor {
        kind: ObjectKind::Chunk,
        object_id: ObjectId::parse(chunk.id().as_str()).unwrap(),
        canonical_byte_len: bytes.len() as u64,
        encoded_byte_len: bytes.len() as u64,
        compression: Compression::Identity,
    }
}
fn body(bytes: &[u8]) -> CasByteStream {
    Box::pin(stream::iter(
        bytes
            .chunks(16 * 1024)
            .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
            .collect::<Vec<_>>(),
    ))
}
async fn contents(cas: &FsServerCas, object: &ObjectDescriptor) -> Vec<u8> {
    let mut stream = cas.verified_stream(object).await.unwrap();
    let mut result = vec![];
    while let Some(bytes) = stream.next().await {
        result.extend(bytes.unwrap());
    }
    result
}

#[tokio::test]
#[ignore = "requires the disposable 64 MiB tmpfs Docker recipe"]
async fn filesystem_server_cas_refuses_enospc_without_installing_partial_bytes() {
    let started = std::time::Instant::now();
    assert_eq!(
        std::env::var("LAYRS_TEST_TMPFS_ROOT").unwrap(),
        "/layrs-full"
    );
    let root = Path::new("/layrs-full");
    assert!(fs::symlink_metadata(root).unwrap().is_dir());
    assert_eq!(fs::canonicalize(root).unwrap(), root);
    assert!(
        fs::read_to_string("/proc/self/mountinfo")
            .unwrap()
            .lines()
            .any(|line| {
                let Some((left, right)) = line.split_once(" - ") else {
                    return false;
                };
                left.split_whitespace().nth(4) == Some("/layrs-full")
                    && right.split_whitespace().next() == Some("tmpfs")
                    && right
                        .split_whitespace()
                        .last()
                        .unwrap()
                        .split(',')
                        .any(|option| option == "size=65536k")
            }),
        "exact disposable 64 MiB tmpfs required"
    );
    let fixture = root.join("server-cas-enospc");
    fs::create_dir(&fixture).unwrap();
    let cas = FsServerCas::open(fixture.join("cas"), CasLimits::default())
        .await
        .unwrap();
    let old = b"immutable original";
    let old_object = descriptor(old);
    cas.put_if_absent(&old_object, body(old)).await.unwrap();
    let next = vec![0x41; 1024 * 1024];
    let new_object = descriptor(&next);
    let pressure_path = fixture.join("pressure.bin");
    let mut pressure = File::create_new(&pressure_path).unwrap();
    loop {
        match pressure.write_all(&[0x5a; 64 * 1024]) {
            Ok(()) => assert!(pressure.metadata().unwrap().len() <= 64 * 1024 * 1024),
            Err(error) => {
                assert_eq!(error.raw_os_error(), Some(28));
                break;
            }
        }
    }
    let error = cas
        .put_if_absent(&new_object, body(&next))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("No space left on device"),
        "{error}"
    );
    assert!(cas.head(&new_object).await.unwrap().is_none());
    assert_eq!(contents(&cas, &old_object).await, old);
    drop(pressure);
    fs::remove_file(&pressure_path).unwrap();
    assert_eq!(
        cas.put_if_absent(&new_object, body(&next)).await.unwrap(),
        PutOutcome::Installed
    );
    assert_eq!(contents(&cas, &new_object).await, next);
    assert_eq!(contents(&cas, &old_object).await, old);
    drop(cas);
    assert_eq!(fixture.parent(), Some(root));
    fs::remove_dir_all(fixture).unwrap();
    eprintln!(
        "Actual Linux server CAS ENOSPC: partial object absent, existing bytes intact, verified retry passed in {} ms",
        started.elapsed().as_millis()
    );
}
