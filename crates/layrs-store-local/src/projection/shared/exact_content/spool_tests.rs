use super::*;

#[test]
fn bounded_sink_rejects_tail_short_write_and_cancellation() {
    let cancel = Cancellation::default();
    let mut bytes = Vec::new();
    let mut sink = PieceSink {
        output: &mut bytes,
        remaining: 3,
        hash: blake3::Hasher::new(),
        cancel: &cancel,
        cancelled: false,
    };
    sink.hash.update(b"layrs-chunk-v1\0");
    sink.write_all(b"abc").unwrap();
    assert_eq!(sink.remaining, 0);
    assert!(sink.write_all(b"tail").is_err());
    assert_eq!(
        format!("blake3:{}", sink.hash.finalize()),
        layrs_core::scm::Chunk::new(b"abc".to_vec())
            .unwrap()
            .id()
            .as_str()
    );
    drop(sink);
    assert_eq!(bytes, b"abc");
    struct Short;
    impl Write for Short {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Ok(0)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut output = Short;
    let mut sink = PieceSink {
        output: &mut output,
        remaining: 3,
        hash: blake3::Hasher::new(),
        cancel: &cancel,
        cancelled: false,
    };
    assert_eq!(
        sink.write_all(b"abc").unwrap_err().kind(),
        io::ErrorKind::WriteZero
    );
    assert_eq!(sink.remaining, 3);
    cancel.cancel();
    assert_eq!(
        sink.write_all(b"abc").unwrap_err().kind(),
        io::ErrorKind::Other
    );
    assert!(sink.cancelled);
}
