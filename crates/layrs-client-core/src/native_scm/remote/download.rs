//! Bounded ranged downloads. No native Store capability survives a network call.
use super::*;
use std::io::{Seek, SeekFrom};
use std::time::Instant;

const SEGMENT_BYTES: u64 = 1024 * 1024;
const MAX_FAILURES: usize = 3;
const OBJECT_DEADLINE: Duration = Duration::from_secs(120);

impl NativeRemote {
    pub(super) fn download_object(
        &self,
        cap: &DownloadCapability,
        mut renew: impl FnMut(Duration) -> RemoteResult<DownloadCapability>,
    ) -> RemoteResult<CanonicalObject> {
        let mut cap = cap.clone();
        self.transfer_check()?;
        let total = cap.descriptor.encoded_byte_len;
        if total > MAX_OBJECT || cap.descriptor.canonical_byte_len > MAX_OBJECT {
            return Err(NativeRemoteError::Invalid("download exceeds limits"));
        }
        let mut spool = temporary_file()?;
        let mut resume = self.resume_download(&cap, &mut spool)?;
        let started = Instant::now();
        let mut offset = resume.as_ref().map_or(0, |r| r.offset);
        // Even a complete cached prefix needs a fresh authorized response.
        // Never treat local checkpoint data as an authorization capability.
        if offset == total && total != 0 {
            offset -= 1;
            spool.seek(SeekFrom::Start(offset))?;
        }
        let mut failures = 0;
        let mut renewals = 0;
        loop {
            self.transfer_check()?;
            let ranged = cap.accept_ranges && total != 0;
            let end = if ranged {
                (offset + SEGMENT_BYTES).min(total)
            } else {
                total
            };
            let remaining = OBJECT_DEADLINE
                .checked_sub(started.elapsed())
                .ok_or(NativeRemoteError::Transport)?;
            let mut request = self
                .http
                .get(self.transfer_url(&cap.download_url)?)
                .bearer_auth(&cap.token)
                .timeout(remaining.min(Duration::from_secs(30)));
            if ranged {
                request = request.header(
                    reqwest::header::RANGE,
                    format!("bytes={offset}-{}", end - 1),
                );
            }
            let received = match request.send() {
                Ok(response) => {
                    let mut response = match checked(response) {
                        Ok(response) => response,
                        Err(error)
                            if renewals < 2 && download_refresh::expired_download(&error, &cap) =>
                        {
                            self.transfer_check()?;
                            let remaining = OBJECT_DEADLINE
                                .checked_sub(started.elapsed())
                                .filter(|duration| !duration.is_zero())
                                .ok_or(NativeRemoteError::Transport)?;
                            let next = renew(remaining)?;
                            if next.descriptor != cap.descriptor
                                || next.claim != cap.claim
                                || next.claim_digest != cap.claim_digest
                                || next.principal_id != cap.principal_id
                                || next.space_id != cap.space_id
                                || next.authorization_epoch != cap.authorization_epoch
                                || next.accept_ranges != cap.accept_ranges
                            {
                                return Err(NativeRemoteError::Invalid(
                                    "download renewal changed capability binding",
                                ));
                            }
                            cap = next;
                            if let Some(resume) = &mut resume {
                                resume.renewed(&cap);
                            }
                            renewals += 1;
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    validate_response(&response, ranged, offset, end, total)?;
                    copy_segment_controlled(
                        &mut response,
                        &mut spool,
                        &mut offset,
                        end,
                        self.transfer.as_ref(),
                    )
                }
                Err(_) => Err(NativeRemoteError::Transport),
            };
            // Save verified-length progress even when a connection or process
            // is about to stop. A malformed response never becomes a checkpoint.
            if !(received.is_ok() && offset == total)
                && matches!(
                    &received,
                    Ok(()) | Err(NativeRemoteError::Transport) | Err(NativeRemoteError::Cancelled)
                )
            {
                if let Some(resume) = &mut resume {
                    resume.save(&mut spool, offset)?;
                }
            }
            match self.transfer_result(received) {
                Ok(()) if offset == total => break,
                Ok(()) => (),
                Err(NativeRemoteError::Transport) if failures < MAX_FAILURES => {
                    failures += 1;
                    // A peer which did not authorize ranges must restart the
                    // object; it may never append a full response to a prefix.
                    if !ranged {
                        spool.set_len(0)?;
                        spool.seek(SeekFrom::Start(0))?;
                        offset = 0;
                    }
                    if offset == total {
                        // All bytes arrived but EOF was not authenticated by
                        // the HTTP stack. Recheck the last byte on a new range.
                        offset = offset.saturating_sub(1);
                        spool.set_len(offset)?;
                        spool.seek(SeekFrom::Start(offset))?;
                    }
                }
                Err(error) => return Err(error),
            }
        }
        self.transfer_check()?;
        spool.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        let limit = cap.descriptor.canonical_byte_len + 1;
        match cap.descriptor.compression {
            Compression::Identity => {
                spool.take(limit).read_to_end(&mut bytes)?;
            }
            Compression::Zstd => {
                zstd::stream::read::Decoder::new(spool)?
                    .take(limit)
                    .read_to_end(&mut bytes)?;
            }
        }
        if let Some(resume) = resume {
            resume.clear()?;
        }
        if bytes.len() as u64 != cap.descriptor.canonical_byte_len {
            return Err(NativeRemoteError::Invalid("decoded length mismatch"));
        }
        let object = decode(cap.descriptor.kind, &bytes)?;
        if object.object_id().as_str() != cap.descriptor.object_id.as_str() {
            return Err(NativeRemoteError::Invalid(
                "download content identity mismatch",
            ));
        }
        Ok(object)
    }
}

fn validate_response(
    response: &Response,
    ranged: bool,
    start: u64,
    end: u64,
    total: u64,
) -> RemoteResult<()> {
    validate_framing(
        response.status(),
        response.headers(),
        ranged,
        start,
        end,
        total,
    )
}

fn validate_framing(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    ranged: bool,
    start: u64,
    end: u64,
    total: u64,
) -> RemoteResult<()> {
    let expected_status = if ranged {
        reqwest::StatusCode::PARTIAL_CONTENT
    } else {
        reqwest::StatusCode::OK
    };
    let one = |name| {
        let mut values = headers.get_all(name).iter();
        let value = values.next().and_then(|v| v.to_str().ok());
        if values.next().is_some() {
            None
        } else {
            value
        }
    };
    if status != expected_status
        || one(reqwest::header::CONTENT_LENGTH).and_then(|v| v.parse::<u64>().ok())
            != Some(end - start)
        || (headers.contains_key(reqwest::header::CONTENT_ENCODING)
            && one(reqwest::header::CONTENT_ENCODING) != Some("identity"))
    {
        return Err(NativeRemoteError::Invalid(
            "download response framing mismatch",
        ));
    }
    if ranged {
        let expected = format!("bytes {start}-{}/{total}", end - 1);
        if one(reqwest::header::CONTENT_RANGE) != Some(expected.as_str()) {
            return Err(NativeRemoteError::Invalid(
                "download content range mismatch",
            ));
        }
    } else if headers.contains_key(reqwest::header::CONTENT_RANGE) {
        return Err(NativeRemoteError::Invalid("unexpected download range"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "download_tests.rs"]
mod tests;

#[cfg(test)]
fn copy_segment(
    response: &mut impl Read,
    spool: &mut File,
    offset: &mut u64,
    end: u64,
) -> RemoteResult<()> {
    copy_segment_controlled(response, spool, offset, end, None)
}
fn copy_segment_controlled(
    response: &mut impl Read,
    spool: &mut File,
    offset: &mut u64,
    end: u64,
    control: Option<&NativeTransferControl>,
) -> RemoteResult<()> {
    let mut buffer = [0u8; 16 * 1024];
    loop {
        if let Some(control) = control {
            control.check()?;
        }
        let limit = (end - *offset + 1).min(buffer.len() as u64) as usize;
        let count = response
            .read(&mut buffer[..limit])
            .map_err(|_| NativeRemoteError::Transport)?;
        if count == 0 {
            return if *offset == end {
                Ok(())
            } else {
                Err(NativeRemoteError::Transport)
            };
        }
        if count as u64 > end - *offset {
            return Err(NativeRemoteError::Invalid(
                "download exceeds declared segment",
            ));
        }
        spool.write_all(&buffer[..count])?;
        *offset += count as u64;
    }
}

/// An anonymous/private spool, removed on close (also on normal early return).
/// It carries object bytes only, never credentials or capability tokens.
pub(super) fn temporary_file() -> RemoteResult<File> {
    Ok(layrs_store_local::private_spool_file()?)
}
