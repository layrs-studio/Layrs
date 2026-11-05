use bytes::Bytes;
use futures_util::{StreamExt, stream};
use layrs_store_server::v3::*;
use layrs_sync::v3::ObjectDescriptor;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn body(bytes: &[u8]) -> CasByteStream {
    Box::pin(stream::iter(vec![Ok(Bytes::copy_from_slice(bytes))]))
}

pub async fn check(cas: &dyn ServerCas, descriptor: &ObjectDescriptor, bytes: &[u8]) {
    assert_eq!(
        cas.put_if_absent(descriptor, body(bytes)).await.unwrap(),
        PutOutcome::Installed
    );
    let exhausted = Arc::new(AtomicBool::new(false));
    let observed = exhausted.clone();
    let input = Bytes::copy_from_slice(bytes);
    let stream = stream::unfold(Some(input), move |state| {
        let observed = observed.clone();
        async move {
            match state {
                Some(bytes) => Some((Ok(bytes), None)),
                None => {
                    observed.store(true, Ordering::SeqCst);
                    None
                }
            }
        }
    });
    assert_eq!(
        cas.put_if_absent(descriptor, Box::pin(stream))
            .await
            .unwrap(),
        PutOutcome::AlreadyPresent
    );
    assert!(
        exhausted.load(Ordering::SeqCst),
        "existing object still requires consuming the body through EOF"
    );
    for wrong in [
        Vec::new(),
        bytes[..bytes.len() - 1].to_vec(),
        vec![b'x'; bytes.len()],
        [bytes, b"extra"].concat(),
    ] {
        assert!(
            cas.put_if_absent(descriptor, body(&wrong)).await.is_err(),
            "empty, truncated, wrong-content or oversized retry cannot prove possession"
        );
    }
    let failing = stream::iter(vec![
        Ok(Bytes::copy_from_slice(bytes)),
        Err(V3StoreError::Backend("simulated late read failure".into())),
    ]);
    assert!(
        cas.put_if_absent(descriptor, Box::pin(failing))
            .await
            .is_err(),
        "a read failure after the declared bytes cannot be silently ignored"
    );
    let mut stored = cas.verified_stream(descriptor).await.unwrap();
    let mut restored = Vec::new();
    while let Some(chunk) = stored.next().await {
        restored.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(
        restored, bytes,
        "failed retries preserve the immutable stored object"
    );
}
