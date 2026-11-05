use super::*;
use std::io::Cursor;

#[test]
fn ordinary_scan_hash_without_cancellation_keeps_file_identity_and_length_checks() {
    for bytes in [Vec::new(), vec![7; 64 * 1024 + 19]] {
        assert_eq!(
            hash_file(&mut Cursor::new(&bytes), bytes.len() as u64, None).unwrap(),
            FileId::from_bytes(&bytes)
        );
        assert!(hash_file(&mut Cursor::new(&bytes), bytes.len() as u64 + 1, None).is_err());
    }
    assert!(hash_file(&mut Cursor::new(b"larger than recorded"), 1, None).is_err());
}

#[cfg(windows)]
mod cancellation {
    use super::*;
    use crate::projection::provider::recovery::ProviderBootstrapCause as Cause;
    use layrs_vfs::Cancellation;

    struct CancelDuringRead {
        input: Cursor<Vec<u8>>,
        signal: Cancellation,
        reads: usize,
    }
    impl Read for CancelDuringRead {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            self.reads += 1;
            let count = self.input.read(output)?;
            self.signal.cancel();
            Ok(count)
        }
    }

    fn check(cancel: &Cancellation) -> Result<()> {
        cancel.check().map_err(Cause::from).map_err(Into::into)
    }

    fn assert_cancelled(result: Result<FileId>) {
        assert!(matches!(
            result,
            Err(ProjectionStoreError::ProviderContent(cause))
                if matches!(*cause, Cause::Vfs(layrs_vfs::Error::Cancelled))
        ));
    }

    #[test]
    fn scan_hash_cancellation_during_read_stops_before_next_chunk_and_covers_eof() {
        // Cancellation happens synchronously inside Read, so neither timing nor
        // a worker thread can turn this into an accidental pre-cancelled test.
        for length in [0, 128 * 1024] {
            let cancel = Cancellation::default();
            let mut input = CancelDuringRead {
                input: Cursor::new(vec![9; length]),
                signal: cancel.clone(),
                reads: 0,
            };
            let poll = || check(&cancel);
            assert_cancelled(hash_file(&mut input, length as u64, Some(&poll)));
            assert_eq!(input.reads, 1, "cancelled scan must not issue another read");
            assert_eq!(input.input.position(), length.min(64 * 1024) as u64);
        }
    }

    #[test]
    fn scan_hash_precancellation_refuses_before_reading_any_bytes() {
        struct NeverRead;
        impl Read for NeverRead {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                panic!("pre-cancelled scan reached its byte reader")
            }
        }
        let cancel = Cancellation::default();
        cancel.cancel();
        let poll = || check(&cancel);
        assert_cancelled(hash_file(&mut NeverRead, 1, Some(&poll)));
    }
}
