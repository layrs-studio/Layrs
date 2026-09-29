use axum::{body::Body, extract::Request};
use bytes::BytesMut;
use futures_util::StreamExt;
use serde::de::DeserializeOwned;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use super::{
    error::V3HttpError,
    state::{UploadStream, V3ServiceError},
};

pub async fn limited_json<T: DeserializeOwned>(
    request: Request,
    limit: usize,
) -> Result<T, V3HttpError> {
    let mut stream = request.into_body().into_data_stream();
    let mut bytes = BytesMut::new();
    while let Some(item) = stream.next().await {
        let chunk = item.map_err(|_| V3HttpError::from(V3ServiceError::BodyReadFailure))?;
        let next = bytes
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| V3HttpError::payload_too_large(limit as u64))?;
        if next > limit {
            return Err(V3HttpError::payload_too_large(limit as u64));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| V3HttpError::invalid_request("invalid_json"))
}

#[derive(Clone, Default)]
pub struct UploadProgress(Arc<UploadProgressInner>);

#[derive(Default)]
struct UploadProgressInner {
    bytes_seen: AtomicU64,
    complete: AtomicBool,
}

impl UploadProgress {
    pub fn bytes_seen(&self) -> u64 {
        self.0.bytes_seen.load(Ordering::Acquire)
    }

    pub fn is_complete(&self) -> bool {
        self.0.complete.load(Ordering::Acquire)
    }
}

pub fn limited_upload_stream(
    body: Body,
    global_limit: u64,
    declared_length: u64,
) -> (UploadStream, UploadProgress) {
    let stream = body.into_data_stream();
    let progress = UploadProgress::default();
    let observer = progress.clone();
    let limited =
        futures_util::stream::try_unfold((Box::pin(stream), 0_u64), move |(mut stream, used)| {
            let observer = observer.clone();
            async move {
                match stream.next().await {
                    Some(Ok(bytes)) => {
                        let next = used.checked_add(bytes.len() as u64).ok_or(
                            V3ServiceError::PayloadTooLarge {
                                limit: global_limit,
                            },
                        )?;
                        observer.0.bytes_seen.store(next, Ordering::Release);
                        if next > global_limit {
                            return Err(V3ServiceError::PayloadTooLarge {
                                limit: global_limit,
                            });
                        }
                        if next > declared_length {
                            return Err(V3ServiceError::LengthMismatch);
                        }
                        Ok(Some((bytes, (stream, next))))
                    }
                    Some(Err(_)) => Err(V3ServiceError::BodyReadFailure),
                    None => {
                        observer.0.complete.store(true, Ordering::Release);
                        Ok(None)
                    }
                }
            }
        });
    (Box::pin(limited), progress)
}
