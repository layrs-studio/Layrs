use super::*;
use reqwest::{header::*, StatusCode};

#[test]
fn resumed_response_must_bind_exact_range_and_single_framing() {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_LENGTH, "5".parse().unwrap());
    headers.insert(CONTENT_RANGE, "bytes 4-8/9".parse().unwrap());
    assert!(validate_framing(StatusCode::PARTIAL_CONTENT, &headers, true, 4, 9, 9).is_ok());
    assert!(validate_framing(StatusCode::OK, &headers, true, 4, 9, 9).is_err());
    for value in ["bytes 0-4/9", "bytes 4-8/10", "bytes 4-9/9", "bytes */9"] {
        headers.insert(CONTENT_RANGE, value.parse().unwrap());
        assert!(validate_framing(StatusCode::PARTIAL_CONTENT, &headers, true, 4, 9, 9).is_err());
    }
    headers.insert(CONTENT_RANGE, "bytes 4-8/9".parse().unwrap());
    headers.append(CONTENT_LENGTH, "5".parse().unwrap());
    assert!(validate_framing(StatusCode::PARTIAL_CONTENT, &headers, true, 4, 9, 9).is_err());
    headers.insert(CONTENT_LENGTH, "5".parse().unwrap());
    headers.append(CONTENT_ENCODING, "identity".parse().unwrap());
    headers.append(CONTENT_ENCODING, "gzip".parse().unwrap());
    assert!(validate_framing(StatusCode::PARTIAL_CONTENT, &headers, true, 4, 9, 9).is_err());
    headers.remove(CONTENT_ENCODING);
    headers.remove(CONTENT_RANGE);
    assert!(validate_framing(StatusCode::OK, &headers, false, 0, 5, 5).is_ok());
}

#[test]
fn disk_spool_preserves_prefix_and_bounds_each_read_without_a_file_sized_buffer() {
    struct Generated {
        remaining: usize,
        largest_read: usize,
        interrupt: bool,
    }
    impl Read for Generated {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.largest_read = self.largest_read.max(buffer.len());
            if self.remaining == 0 && self.interrupt {
                return Err(std::io::Error::other("cut"));
            }
            let count = self.remaining.min(buffer.len());
            buffer[..count].fill(0xa7);
            self.remaining -= count;
            Ok(count)
        }
    }
    let mut spool = temporary_file().unwrap();
    let mut offset = 0;
    let mut first = Generated {
        remaining: 75_001,
        largest_read: 0,
        interrupt: true,
    };
    assert!(matches!(
        copy_segment(&mut first, &mut spool, &mut offset, 8 * 1024 * 1024),
        Err(NativeRemoteError::Transport)
    ));
    assert_eq!(offset, 75_001);
    assert_eq!(spool.metadata().unwrap().len(), offset);
    let mut suffix = Generated {
        remaining: 8 * 1024 * 1024 - offset as usize,
        largest_read: 0,
        interrupt: false,
    };
    copy_segment(&mut suffix, &mut spool, &mut offset, 8 * 1024 * 1024).unwrap();
    assert!(first.largest_read <= 16 * 1024 && suffix.largest_read <= 16 * 1024);
    spool.seek(SeekFrom::Start(0)).unwrap();
    let mut block = [0u8; 8192];
    let mut total = 0;
    loop {
        let count = spool.read(&mut block).unwrap();
        if count == 0 {
            break;
        };
        assert!(block[..count].iter().all(|&byte| byte == 0xa7));
        total += count;
    }
    assert_eq!(total, 8 * 1024 * 1024);
    assert!(copy_segment(&mut &b"excess"[..], &mut spool, &mut offset, total as u64).is_err());
}
