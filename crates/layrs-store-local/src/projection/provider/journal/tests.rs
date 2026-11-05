//! Pure supplied-stream tests. No SQLite, filesystem, process or native proof.
use super::*;
use std::error::Error;
use std::io::Cursor;

#[path = "fixture.rs"]
mod fixture;
use fixture::{Block, Fixture};

fn db(page_size: u32, pages: u64) -> DatabaseHeaderEvidence {
    DatabaseHeaderEvidence::from_observed(
        &fixture::database_header(page_size),
        u64::from(page_size) * pages,
    )
    .unwrap()
}

fn geometry() -> ReplayGeometry {
    ReplayGeometry::from_observed_device(4096, true)
}

fn inspect(bytes: &[u8], database: DatabaseHeaderEvidence) -> Result<JournalAdmission> {
    inspect_journal(
        &mut Cursor::new(bytes),
        bytes.len() as u64,
        database,
        geometry(),
        JournalLimits::default(),
    )
}

fn refused<T: fmt::Debug>(result: Result<T>) -> Refusal {
    match result {
        Err(JournalError::Refused(reason)) => reason,
        other => panic!("expected structural refusal, got {other:?}"),
    }
}

fn pair() -> Fixture {
    fixture::journal(
        512,
        512,
        4,
        vec![
            Block {
                seed: 0xffff_fff0,
                pages: vec![(2, vec![0x10; 512])],
            },
            Block {
                seed: 0x1020_3040,
                pages: vec![(3, vec![0x33; 512])],
            },
        ],
    )
}

#[test]
fn handwritten_single_record_and_complete_observation() {
    let bytes = fixture::literal_512();
    assert_eq!(bytes.len(), 1032);
    let admitted = inspect(&bytes, db(512, 2)).unwrap();
    assert_eq!(admitted.database, db(512, 2));
    assert_eq!(admitted.geometry, geometry());
    assert_eq!(admitted.journal_bytes, 1032);
    assert_eq!(
        (admitted.journal_page_size, admitted.journal_sector_size),
        (512, 512)
    );
    assert_eq!(
        (admitted.original_pages, admitted.original_bytes),
        (2, 1024)
    );
    assert_eq!(
        (admitted.headers, admitted.records, admitted.page_one_seen),
        (1, 1, false)
    );
    assert_eq!(
        admitted.consumed_byte_digest,
        *blake3::hash(&bytes).as_bytes()
    );
}

#[test]
fn complete_multi_header_uses_each_seed_and_hashes_all_padding() {
    let mut fixture = pair();
    assert_eq!(fixture.headers, [0, 1536]);
    assert_eq!(fixture.records, [512, 2048]);
    // First checksum wraps: 0xfffffff0 + 0x10 + 0x10 == 0x10.
    assert_eq!(&fixture.bytes[1028..1032], &[0, 0, 0, 0x10]);
    let admitted = inspect(&fixture.bytes, db(512, 4)).unwrap();
    assert_eq!((admitted.headers, admitted.records), (2, 2));
    assert_eq!(
        admitted.consumed_byte_digest,
        *blake3::hash(&fixture.bytes).as_bytes()
    );
    let original_digest = admitted.consumed_byte_digest;
    fixture.bytes[40] ^= 0x52; // unused first header bytes
    fixture.bytes[1200] ^= 0x73; // inter-header alignment bytes
    fixture.bytes[1600] ^= 0x41; // unused second header bytes
    let changed = inspect(&fixture.bytes, db(512, 4)).unwrap();
    assert_ne!(changed.consumed_byte_digest, original_digest);
    assert_eq!(
        changed.consumed_byte_digest,
        *blake3::hash(&fixture.bytes).as_bytes()
    );
    // Reusing the first seed for the second chunk cannot validate its records.
    fixture::put32(&mut fixture.bytes, 1536 + 12, 0xffff_fff0);
    assert!(matches!(
        refused(inspect(&fixture.bytes, db(512, 4))),
        Refusal::Checksum { offset: 2048, .. }
    ));
}

#[test]
fn initial_sector_fit_precedes_journal_sector_adoption() {
    let small = fixture::literal_512();
    let actual_device = ReplayGeometry::from_observed_device(4096, false);
    assert_eq!(
        refused(inspect_journal(
            &mut Cursor::new(&small),
            1032,
            db(512, 2),
            actual_device,
            JournalLimits::default()
        )),
        Refusal::InitialHeaderDoesNotFit {
            sector: 4096,
            bytes: 1032
        }
    );
    // Identical bytes fit an observed effective 512-byte sector.
    let _admission = inspect_journal(
        &mut Cursor::new(&small),
        1032,
        db(512, 2),
        geometry(),
        JournalLimits::default(),
    )
    .unwrap();
    // A longer complete journal also fits initial 4096, then adopts 512.
    let longer = fixture::journal(
        512,
        512,
        2,
        vec![Block {
            seed: 9,
            pages: vec![(2, vec![0; 512]); 7],
        }],
    );
    assert_eq!(longer.bytes.len(), 4152);
    let admitted = inspect_journal(
        &mut Cursor::new(&longer.bytes),
        4152,
        db(512, 2),
        actual_device,
        JournalLimits::default(),
    )
    .unwrap();
    assert_eq!(admitted.journal_sector_size, 512);
}

#[test]
fn observed_geometry_reproduces_psow_fallback_and_clamps_without_rounding() {
    for (raw, psow, expected) in [
        (4096, true, 512),
        (-1, false, 512),
        (0, false, 512),
        (31, false, 512),
        (32, false, 32),
        (33, false, 33),
        (4096, false, 4096),
        (65536, false, 65536),
        (65537, false, 65536),
        (i32::MAX, true, 512),
        (i32::MAX, false, 65536),
    ] {
        assert_eq!(
            ReplayGeometry::from_observed_device(raw, psow).effective_sector_size,
            expected
        );
    }
    // Data comparison only, not a live RO/RW-open test or file-pointer proof.
    assert_ne!(
        geometry(),
        ReplayGeometry::from_observed_device(4096, false)
    );
    assert_ne!(geometry(), ReplayGeometry::from_observed_device(512, false));
    assert_eq!(geometry(), ReplayGeometry::from_observed_device(4096, true));
}

#[test]
fn current_database_evidence_requires_exact_header_and_complete_nonzero_pages() {
    let header = fixture::database_header(512);
    assert_eq!(
        refused(DatabaseHeaderEvidence::from_observed(&header[..99], 1024)),
        Refusal::DatabaseHeaderLength { actual: 99 }
    );
    assert_eq!(
        refused(DatabaseHeaderEvidence::from_observed(&[0; 101], 1024)),
        Refusal::DatabaseHeaderLength { actual: 101 }
    );
    let mut bad = header;
    bad[0] ^= 1;
    assert_eq!(
        refused(DatabaseHeaderEvidence::from_observed(&bad, 1024)),
        Refusal::DatabaseMagic
    );
    for size in [0u16, 256, 513, 32769] {
        let mut bad = header;
        bad[16..18].copy_from_slice(&size.to_be_bytes());
        assert_eq!(
            refused(DatabaseHeaderEvidence::from_observed(&bad, 1024)),
            Refusal::DatabasePageSize {
                value: u32::from(size)
            }
        );
    }
    for (write, read) in [(0, 1), (2, 1), (1, 0), (1, 2), (2, 2)] {
        let mut bad = header;
        bad[18] = write;
        bad[19] = read;
        assert_eq!(
            refused(DatabaseHeaderEvidence::from_observed(&bad, 1024)),
            Refusal::DatabaseVersions { write, read }
        );
    }
    for bytes in [0, 1, 511, 513, 1023] {
        assert_eq!(
            refused(DatabaseHeaderEvidence::from_observed(&header, bytes)),
            Refusal::DatabaseExtent {
                bytes,
                page_size: 512
            }
        );
    }
    assert_eq!(db(512, 2).byte_length, 1024);
    // No physical database cap: only a supplied large length, no allocation.
    assert_eq!(db(512, 1 << 32).byte_length, 1 << 41);
}

#[test]
fn all_supported_page_sizes_include_database_64k_encoding() {
    for page_size in [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536] {
        let fixture = fixture::journal(
            page_size,
            512,
            2,
            vec![Block {
                seed: 55,
                pages: vec![(1, fixture::page_one(page_size))],
            }],
        );
        let admitted = inspect(&fixture.bytes, db(page_size, 2)).unwrap();
        assert!(admitted.page_one_seen);
        assert_eq!(admitted.journal_page_size, page_size);
        if page_size == 65536 {
            assert_eq!(&fixture.bytes[512 + 4 + 16..512 + 4 + 18], &[0, 1]);
            assert_eq!(&fixture.bytes[24..28], &[0, 1, 0, 0]);
        }
    }
}

#[test]
fn supported_sector_extremes_and_multi_header_alignment() {
    for sector in [32, 64, 512, 4096, 65536] {
        let fixture = fixture::journal(
            512,
            sector,
            2,
            vec![
                Block {
                    seed: 3,
                    pages: vec![(2, vec![0; 512])],
                },
                Block {
                    seed: 4,
                    pages: vec![(2, vec![0; 512])],
                },
            ],
        );
        let admitted = inspect(&fixture.bytes, db(512, 2)).unwrap();
        assert_eq!(
            (admitted.headers, admitted.journal_sector_size),
            (2, sector)
        );
        assert_eq!(
            admitted.consumed_byte_digest,
            *blake3::hash(&fixture.bytes).as_bytes()
        );
    }
}

#[test]
fn invalid_journal_sizes_magic_and_database_mismatch_are_specific() {
    let healthy = fixture::literal_512();
    let _admission = inspect(&healthy, db(512, 2)).unwrap();
    let mut bad = healthy.clone();
    bad[0] = 0;
    assert_eq!(
        refused(inspect(&bad, db(512, 2))),
        Refusal::JournalMagic { offset: 0 }
    );
    for value in [0, 1, 256, 513, 65537, u32::MAX] {
        let mut bad = healthy.clone();
        fixture::put32(&mut bad, 24, value);
        assert_eq!(
            refused(inspect(&bad, db(512, 2))),
            Refusal::JournalPageSize { offset: 0, value }
        );
    }
    for value in [0, 1, 31, 33, 65537, u32::MAX] {
        let mut bad = healthy.clone();
        fixture::put32(&mut bad, 20, value);
        assert_eq!(
            refused(inspect(&bad, db(512, 2))),
            Refusal::JournalSectorSize { offset: 0, value }
        );
    }
    let mut bad = healthy.clone();
    fixture::put32(&mut bad, 24, 1024);
    assert_eq!(
        refused(inspect(&bad, db(512, 2))),
        Refusal::PageSizeMismatch {
            offset: 0,
            expected: 512,
            actual: 1024
        }
    );
    let mut bad = healthy;
    fixture::put32(&mut bad, 20, 2048);
    assert_eq!(
        refused(inspect(&bad, db(512, 2))),
        Refusal::HeaderDoesNotFit {
            offset: 0,
            sector: 2048
        }
    );
}

#[test]
fn later_header_original_and_geometry_fields_are_not_ignored() {
    let healthy = pair();
    let _admission = inspect(&healthy.bytes, db(512, 4)).unwrap();
    for (field, value, expected) in [
        (20, 1024, Refusal::HeaderGeometryChanged { offset: 1536 }),
        (16, 3, Refusal::OriginalPagesChanged { offset: 1536 }),
        (
            24,
            1024,
            Refusal::PageSizeMismatch {
                offset: 1536,
                expected: 512,
                actual: 1024,
            },
        ),
    ] {
        let mut bad = healthy.bytes.clone();
        fixture::put32(&mut bad, 1536 + field, value);
        assert_eq!(refused(inspect(&bad, db(512, 4))), expected);
    }
}

#[test]
fn zero_sentinel_and_extending_original_counts_refuse() {
    let healthy = fixture::literal_512();
    for value in [0, u32::MAX] {
        let mut bad = healthy.clone();
        fixture::put32(&mut bad, 8, value);
        assert_eq!(
            refused(inspect(&bad, db(512, 2))),
            Refusal::UnsupportedRecordCount { offset: 0, value }
        );
    }
    let mut bad = healthy.clone();
    fixture::put32(&mut bad, 16, 0);
    assert_eq!(
        refused(inspect(&bad, db(512, 2))),
        Refusal::ZeroOriginalPages { offset: 0 }
    );
    fixture::put32(&mut bad, 16, 3);
    assert_eq!(
        refused(inspect(&bad, db(512, 2))),
        Refusal::OriginalExtent {
            bytes: 1536,
            database_bytes: 1024
        }
    );
    // A smaller original extent is supported; this is not an equality rule.
    let _admission = inspect(&healthy, db(512, 3)).unwrap();
}

#[test]
fn declared_record_extents_and_total_counts_checked_before_record_reads() {
    let mut bad = fixture::literal_512();
    fixture::put32(&mut bad, 8, 2);
    assert_eq!(
        refused(inspect(&bad, db(512, 2))),
        Refusal::IncompleteRecords {
            offset: 0,
            required_end: 1552,
            bytes: 1032
        }
    );
    fixture::put32(&mut bad, 8, u32::MAX - 1);
    assert_eq!(
        refused(inspect(&bad, db(512, 2))),
        Refusal::RecordLimit {
            limit: 524288,
            attempted: u64::from(u32::MAX - 1)
        }
    );
    let mut later = pair();
    fixture::put32(&mut later.bytes, 1536 + 8, 0);
    assert_eq!(
        refused(inspect(&later.bytes, db(512, 4))),
        Refusal::UnsupportedRecordCount {
            offset: 1536,
            value: 0
        }
    );
}

#[test]
fn illegal_reserved_and_out_of_range_page_numbers_refuse() {
    let healthy = fixture::literal_512();
    for (number, database, reason) in [
        (
            0,
            db(512, 2),
            Refusal::IllegalPageNumber {
                offset: 512,
                page: 0,
            },
        ),
        (
            3,
            db(512, 2),
            Refusal::PageOutOfRange {
                offset: 512,
                page: 3,
                original_pages: 2,
            },
        ),
        (
            2097153,
            db(512, 2097153),
            Refusal::IllegalPageNumber {
                offset: 512,
                page: 2097153,
            },
        ),
    ] {
        let mut bad = healthy.clone();
        fixture::put32(&mut bad, 512, number);
        if number == 2097153 {
            fixture::put32(&mut bad, 16, number);
        }
        assert_eq!(refused(inspect(&bad, database)), reason);
    }
}

#[test]
fn checksum_uses_exact_sample_indices_unsigned_bytes_and_wrapping_seed() {
    let mut healthy = fixture::literal_512();
    healthy[516 + 312] = 0xff;
    healthy[516 + 112] = 0x80;
    // Explicit independent arithmetic: 0x01020304 + 255 + 128.
    fixture::put32(&mut healthy, 1028, 0x0102_0483);
    let _admission = inspect(&healthy, db(512, 2)).unwrap();
    let before = inspect(&healthy, db(512, 2)).unwrap().consumed_byte_digest;
    healthy[516 + 113] ^= 0xff; // adjacent byte is NOT a checksum sample
    let after = inspect(&healthy, db(512, 2)).unwrap().consumed_byte_digest;
    assert_ne!(before, after); // still included in full-byte digest
    healthy[516 + 112] ^= 1;
    assert!(matches!(
        refused(inspect(&healthy, db(512, 2))),
        Refusal::Checksum { offset: 512, .. }
    ));
}

#[test]
fn page_one_header_checks_do_not_rely_on_the_sparse_checksum() {
    let page = fixture::page_one(512);
    let make = |page| {
        fixture::journal(
            512,
            512,
            2,
            vec![Block {
                seed: 1,
                pages: vec![(1, page)],
            }],
        )
    };
    let _admission = inspect(&make(page.clone()).bytes, db(512, 2)).unwrap();
    let mut bad = page.clone();
    bad[0] = 0;
    assert_eq!(
        refused(inspect(&make(bad).bytes, db(512, 2))),
        Refusal::PageOneMagic { offset: 512 }
    );
    let mut bad = page.clone();
    bad[16..18].copy_from_slice(&1024u16.to_be_bytes());
    assert_eq!(
        refused(inspect(&make(bad).bytes, db(512, 2))),
        Refusal::PageOnePageSize {
            offset: 512,
            expected: 512,
            actual: 1024
        }
    );
    for (write, read) in [(2, 2), (2, 1), (1, 2), (0, 1), (1, 3)] {
        let mut bad = page.clone();
        bad[18] = write;
        bad[19] = read;
        assert_eq!(
            refused(inspect(&make(bad).bytes, db(512, 2))),
            Refusal::PageOneVersions {
                offset: 512,
                write,
                read
            }
        );
    }
}

#[test]
fn later_chunk_wal_page_one_and_duplicate_page_one_refuse() {
    let make = |first, second| {
        fixture::journal(
            512,
            512,
            2,
            vec![
                Block {
                    seed: 3,
                    pages: vec![first],
                },
                Block {
                    seed: 8,
                    pages: vec![second],
                },
            ],
        )
    };
    let healthy = make((2, vec![0; 512]), (1, fixture::page_one(512)));
    assert!(inspect(&healthy.bytes, db(512, 2)).unwrap().page_one_seen);
    let mut wal = fixture::page_one(512);
    wal[18] = 2;
    wal[19] = 2;
    let bad = make((2, vec![0; 512]), (1, wal.clone()));
    assert_eq!(
        refused(inspect(&bad.bytes, db(512, 2))),
        Refusal::PageOneVersions {
            offset: 2048,
            write: 2,
            read: 2
        }
    );
    let duplicate = make((1, fixture::page_one(512)), (1, fixture::page_one(512)));
    assert_eq!(
        refused(inspect(&duplicate.bytes, db(512, 2))),
        Refusal::DuplicatePageOne { offset: 2048 }
    );
    let duplicate_wal = make((1, fixture::page_one(512)), (1, wal));
    // Validate EVERY occurrence, even before classifying a duplicate.
    assert_eq!(
        refused(inspect(&duplicate_wal.bytes, db(512, 2))),
        Refusal::PageOneVersions {
            offset: 2048,
            write: 2,
            read: 2
        }
    );
}

#[test]
fn every_single_chunk_truncation_has_an_explicit_refusal() {
    let bytes = fixture::literal_512();
    for cut in 0..bytes.len() {
        let expected = if cut == 8 {
            Refusal::SuperJournalTrailerSignature // prefix happens to end in magic
        } else if cut < 8 {
            Refusal::TooShortForFooterProbe { bytes: cut as u64 }
        } else if cut < 512 {
            Refusal::InitialHeaderDoesNotFit {
                sector: 512,
                bytes: cut as u64,
            }
        } else {
            Refusal::IncompleteRecords {
                offset: 0,
                required_end: 1032,
                bytes: cut as u64,
            }
        };
        assert_eq!(
            refused(inspect(&bytes[..cut], db(512, 2))),
            expected,
            "cut={cut}"
        );
    }
    let _admission = inspect(&bytes, db(512, 2)).unwrap();
}

#[test]
fn every_later_chunk_truncation_refuses_except_complete_record_eof() {
    let fixture = pair();
    let _admission = inspect(&fixture.bytes[..1032], db(512, 4)).unwrap();
    for cut in 1033..fixture.bytes.len() {
        let expected = if fixture.bytes[cut - 8..cut] == fixture::MAGIC {
            Refusal::SuperJournalTrailerSignature
        } else if cut < 2048 {
            Refusal::TrailingBytes {
                offset: 1032,
                bytes: (cut - 1032) as u64,
            }
        } else {
            Refusal::IncompleteRecords {
                offset: 1536,
                required_end: 2568,
                bytes: cut as u64,
            }
        };
        assert_eq!(
            refused(inspect(&fixture.bytes[..cut], db(512, 4))),
            expected,
            "cut={cut}"
        );
    }
}

#[test]
fn trailing_padding_partial_header_and_zero_header_never_admit() {
    let healthy = fixture::literal_512();
    for extra in [1, 4, 8, 503, 504, 505, 1015] {
        let mut bad = healthy.clone();
        bad.resize(bad.len() + extra, 0);
        assert_eq!(
            refused(inspect(&bad, db(512, 2))),
            Refusal::TrailingBytes {
                offset: 1032,
                bytes: extra as u64
            }
        );
    }
    let mut bad = healthy;
    bad.resize(2048, 0); // full next zero header, not a permissible stop marker
    assert_eq!(
        refused(inspect(&bad, db(512, 2))),
        Refusal::JournalMagic { offset: 1536 }
    );
}

#[test]
fn footer_signature_refuses_even_invalid_lengths_checksums_or_header() {
    for (name_length, checksum) in [(0u32, 0u32), (u32::MAX, 0), (3, 0), (3, 294)] {
        let mut bad = fixture::literal_512();
        bad[0] = 0; // footer rejection must precede body validation
        bad.extend_from_slice(b"abc");
        bad.extend_from_slice(&name_length.to_be_bytes());
        bad.extend_from_slice(&checksum.to_be_bytes());
        bad.extend_from_slice(&fixture::MAGIC);
        assert_eq!(
            refused(inspect(&bad, db(512, 2))),
            Refusal::SuperJournalTrailerSignature
        );
    }
    assert_eq!(
        refused(inspect(&fixture::MAGIC, db(512, 2))),
        Refusal::SuperJournalTrailerSignature
    );
}

#[test]
fn exact_small_budgets_and_one_over_have_healthy_stream_controls() {
    let fixture = pair();
    let length = fixture.bytes.len() as u64;
    let exact = JournalLimits {
        max_bytes: length,
        max_records: 2,
        max_headers: 2,
    };
    let _admission = inspect_journal(
        &mut Cursor::new(&fixture.bytes),
        length,
        db(512, 4),
        geometry(),
        exact,
    )
    .unwrap();
    for (limits, expected) in [
        (
            JournalLimits {
                max_bytes: length - 1,
                ..exact
            },
            Refusal::ByteLimit {
                limit: length - 1,
                actual: length,
            },
        ),
        (
            JournalLimits {
                max_records: 1,
                ..exact
            },
            Refusal::RecordLimit {
                limit: 1,
                attempted: 2,
            },
        ),
        (
            JournalLimits {
                max_headers: 1,
                ..exact
            },
            Refusal::HeaderLimit {
                limit: 1,
                attempted: 2,
            },
        ),
    ] {
        assert_eq!(
            refused(inspect_journal(
                &mut Cursor::new(&fixture.bytes),
                length,
                db(512, 4),
                geometry(),
                limits
            )),
            expected
        );
    }
}

#[test]
fn default_header_limit_is_exercised_by_actual_streams() {
    let fixture = fixture::journal(
        512,
        512,
        2,
        (0..4097)
            .map(|seed| Block {
                seed,
                pages: vec![(2, vec![0; 512])],
            })
            .collect(),
    );
    // Approximately 6 MiB each parse. The shorter prefix ends at a record,
    // excluding the next alignment/header bytes rather than blessing a tail.
    let end_4096 = fixture.records[4095] + 520;
    assert_eq!(
        inspect(&fixture.bytes[..end_4096], db(512, 2))
            .unwrap()
            .headers,
        4096
    );
    assert_eq!(
        refused(inspect(&fixture.bytes, db(512, 2))),
        Refusal::HeaderLimit {
            limit: 4096,
            attempted: 4097
        }
    );
}

#[test]
fn hard_profile_limits_cannot_be_widened_and_zero_limits_refuse() {
    let bytes = fixture::literal_512();
    let defaults = JournalLimits::default();
    for limits in [
        JournalLimits {
            max_bytes: DEFAULT_MAX_BYTES + 1,
            ..defaults
        },
        JournalLimits {
            max_records: DEFAULT_MAX_RECORDS + 1,
            ..defaults
        },
        JournalLimits {
            max_headers: DEFAULT_MAX_HEADERS + 1,
            ..defaults
        },
    ] {
        assert_eq!(
            refused(inspect_journal(
                &mut Cursor::new(&bytes),
                1032,
                db(512, 2),
                geometry(),
                limits
            )),
            Refusal::LimitsExceedProfile
        );
    }
    assert_eq!(
        refused(inspect_journal(
            &mut Cursor::new(&bytes),
            1032,
            db(512, 2),
            geometry(),
            JournalLimits {
                max_headers: 0,
                ..defaults
            }
        )),
        Refusal::HeaderLimit {
            limit: 0,
            attempted: 1
        }
    );
    assert_eq!(
        refused(inspect_journal(
            &mut Cursor::new(&bytes),
            1032,
            db(512, 2),
            geometry(),
            JournalLimits {
                max_records: 0,
                ..defaults
            }
        )),
        Refusal::RecordLimit {
            limit: 0,
            attempted: 1
        }
    );
}

#[test]
fn counter_only_default_record_limit_and_checked_arithmetic_overflow() {
    // 524288 actual 512-byte records already exceed 256 MiB. These are
    // explicitly counter-only checks, not a claim of admitted stream capacity.
    assert_eq!(count_records(524287, 1, 524288).unwrap(), 524288);
    assert_eq!(
        refused(count_records(524288, 1, 524288)),
        Refusal::RecordLimit {
            limit: 524288,
            attempted: 524289
        }
    );
    for result in [
        add(u64::MAX, 1),
        multiply(u64::MAX, 2),
        align(u64::MAX, 512),
        count_records(u64::MAX, 1, u64::MAX),
        count_headers(u64::MAX, 1, u64::MAX),
    ] {
        assert_eq!(refused(result), Refusal::ArithmeticOverflow);
    }
    assert_eq!(align(0, 512).unwrap(), 0);
    assert_eq!(align(512, 512).unwrap(), 512);
    assert_eq!(align(1032, 512).unwrap(), 1536);
}

struct ObservedStream {
    inner: Cursor<Vec<u8>>,
    reads: Vec<(u64, usize)>,
    seeks: usize,
    max_read: usize,
    fail_read: Option<usize>,
    fail_seek: Option<usize>,
    short_at: Option<u64>,
    short_only_body: bool,
    wrong_seek: bool,
    grow_at_eof: bool,
    report_growth_at_end: bool,
}

impl ObservedStream {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            inner: Cursor::new(bytes),
            reads: Vec::new(),
            seeks: 0,
            max_read: usize::MAX,
            fail_read: None,
            fail_seek: None,
            short_at: None,
            short_only_body: false,
            wrong_seek: false,
            grow_at_eof: false,
            report_growth_at_end: false,
        }
    }
}

impl Read for ObservedStream {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.reads.push((self.inner.position(), out.len()));
        if self.fail_read == Some(self.reads.len()) {
            return Err(io::Error::from_raw_os_error(1234));
        }
        let short_at = if self.short_only_body && self.reads.len() == 1 {
            None
        } else {
            self.short_at
        };
        if short_at.is_some_and(|end| self.inner.position() >= end) {
            return Ok(0);
        }
        if self.grow_at_eof && self.inner.position() == self.inner.get_ref().len() as u64 {
            self.grow_at_eof = false;
            self.inner.get_mut().push(0x71);
        }
        let mut count = out.len().min(self.max_read);
        if let Some(end) = short_at {
            count = count.min(end.saturating_sub(self.inner.position()) as usize);
        }
        self.inner.read(&mut out[..count])
    }
}

impl Seek for ObservedStream {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.seeks += 1;
        if self.fail_seek == Some(self.seeks) {
            return Err(io::Error::from_raw_os_error(5678));
        }
        let result = self.inner.seek(position)?;
        if self.wrong_seek && matches!(position, SeekFrom::Start(_)) {
            return Ok(result + 1);
        }
        if self.report_growth_at_end && self.seeks == 4 {
            return Ok(result + 1);
        }
        Ok(result)
    }
}

#[test]
fn footer_is_first_read_and_signature_does_not_inspect_foreign_bytes() {
    let mut bytes = fixture::literal_512();
    bytes.extend_from_slice(b"not-a-path-to-open");
    bytes.extend_from_slice(&fixture::MAGIC);
    let length = bytes.len() as u64;
    let mut stream = ObservedStream::new(bytes);
    assert_eq!(
        refused(inspect_journal(
            &mut stream,
            length,
            db(512, 2),
            geometry(),
            JournalLimits::default()
        )),
        Refusal::SuperJournalTrailerSignature
    );
    assert_eq!(stream.reads, [(length - 8, 8)]);
    // The parser has only Read+Seek: no pathname or external-access callback.
}

#[test]
fn original_read_and_seek_errors_are_retained_in_source_chain() {
    for (read, seek, raw) in [
        (Some(1), None, 1234),
        (Some(4), None, 1234),
        (None, Some(1), 5678),
        (None, Some(3), 5678),
        (None, Some(4), 5678),
    ] {
        let mut stream = ObservedStream::new(fixture::literal_512());
        stream.fail_read = read;
        stream.fail_seek = seek;
        let error = inspect_journal(
            &mut stream,
            1032,
            db(512, 2),
            geometry(),
            JournalLimits::default(),
        )
        .unwrap_err();
        let source = error.source().unwrap().downcast_ref::<io::Error>().unwrap();
        assert_eq!(source.raw_os_error(), Some(raw));
        assert!(matches!(error, JournalError::Io(_)));
    }
}

#[test]
fn short_reads_are_completed_but_premature_eof_is_original_io_failure() {
    let mut stream = ObservedStream::new(fixture::literal_512());
    stream.max_read = 3;
    let admitted = inspect_journal(
        &mut stream,
        1032,
        db(512, 2),
        geometry(),
        JournalLimits::default(),
    )
    .unwrap();
    assert_eq!(
        admitted.consumed_byte_digest,
        *blake3::hash(stream.inner.get_ref()).as_bytes()
    );
    // Footer read fails without a structural "tail is harmless" conversion.
    let mut short = ObservedStream::new(fixture::literal_512());
    short.short_at = Some(1030);
    let error = inspect_journal(
        &mut short,
        1032,
        db(512, 2),
        geometry(),
        JournalLimits::default(),
    )
    .unwrap_err();
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .kind(),
        io::ErrorKind::UnexpectedEof
    );
    let mut body_short = ObservedStream::new(fixture::literal_512());
    body_short.short_at = Some(700);
    body_short.short_only_body = true;
    let error = inspect_journal(
        &mut body_short,
        1032,
        db(512, 2),
        geometry(),
        JournalLimits::default(),
    )
    .unwrap_err();
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .kind(),
        io::ErrorKind::UnexpectedEof
    );
    assert_eq!(body_short.inner.position(), 700);
}

#[test]
fn length_seek_and_observable_end_drift_refuse() {
    let bytes = fixture::literal_512();
    assert_eq!(
        refused(inspect_journal(
            &mut Cursor::new(&bytes),
            1031,
            db(512, 2),
            geometry(),
            JournalLimits::default()
        )),
        Refusal::LengthChanged {
            expected: 1031,
            actual: 1032
        }
    );
    let mut wrong = ObservedStream::new(bytes.clone());
    wrong.wrong_seek = true;
    assert_eq!(
        refused(inspect_journal(
            &mut wrong,
            1032,
            db(512, 2),
            geometry(),
            JournalLimits::default()
        )),
        Refusal::SeekPosition {
            expected: 1024,
            actual: 1025
        }
    );
    let mut growth = ObservedStream::new(bytes.clone());
    growth.grow_at_eof = true;
    assert_eq!(
        refused(inspect_journal(
            &mut growth,
            1032,
            db(512, 2),
            geometry(),
            JournalLimits::default()
        )),
        Refusal::TrailingBytes {
            offset: 1032,
            bytes: 1
        }
    );
    let mut changed_length = ObservedStream::new(bytes);
    changed_length.report_growth_at_end = true;
    assert_eq!(
        refused(inspect_journal(
            &mut changed_length,
            1032,
            db(512, 2),
            geometry(),
            JournalLimits::default()
        )),
        Refusal::LengthChanged {
            expected: 1032,
            actual: 1033
        }
    );
}

#[test]
fn reads_are_bounded_and_body_bytes_are_consumed_once_in_order() {
    let fixture = fixture::journal(
        65536,
        65536,
        2,
        vec![Block {
            seed: 8,
            pages: vec![(1, fixture::page_one(65536))],
        }],
    );
    let length = fixture.bytes.len() as u64;
    let mut stream = ObservedStream::new(fixture.bytes);
    let _admission = inspect_journal(
        &mut stream,
        length,
        db(65536, 2),
        geometry(),
        JournalLimits::default(),
    )
    .unwrap();
    assert_eq!(stream.reads[0], (length - 8, 8));
    assert!(stream.reads.iter().all(|(_, size)| *size <= 65536));
    let mut expected = 0;
    for &(offset, count) in &stream.reads[1..stream.reads.len() - 1] {
        assert_eq!(offset, expected);
        expected += count as u64;
    }
    assert_eq!(expected, length);
    assert_eq!(stream.reads.last(), Some(&(length, 1)));
    assert_eq!(stream.seeks, 4);
}

#[test]
fn oversized_declared_stream_refuses_before_any_read_or_seek() {
    let mut stream = ObservedStream::new(Vec::new());
    assert_eq!(
        refused(inspect_journal(
            &mut stream,
            DEFAULT_MAX_BYTES + 1,
            db(512, 2),
            geometry(),
            JournalLimits::default()
        )),
        Refusal::ByteLimit {
            limit: DEFAULT_MAX_BYTES,
            actual: DEFAULT_MAX_BYTES + 1
        }
    );
    assert!(stream.reads.is_empty());
    assert_eq!(stream.seeks, 0);
}

#[test]
#[ignore = "pure resource qualification: streams exactly 256 MiB twice; bounded memory, no filesystem/native calls; run serially"]
fn resource_exact_256_mib_virtual_stream_and_one_over() {
    let mut stream = fixture::VirtualJournal::exact_256_mib();
    assert_eq!(stream.length, 268_435_456);
    let length = stream.length;
    let admitted = inspect_journal(
        &mut stream,
        length,
        db(512, 2),
        geometry(),
        JournalLimits::default(),
    )
    .unwrap();
    assert_eq!((admitted.headers, admitted.records), (2, 516_220));
    assert_eq!(stream.bytes_read, length + 8);
    assert!(stream.largest_request <= 65536);
    // Independently hash the whole virtual byte sequence in large reads.
    stream.seek(SeekFrom::Start(0)).unwrap();
    let mut hash = blake3::Hasher::new();
    let mut buffer = vec![0; 65536];
    loop {
        let read = stream.read(&mut buffer).unwrap();
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    assert_eq!(admitted.consumed_byte_digest, *hash.finalize().as_bytes());
    let before = stream.bytes_read;
    assert_eq!(
        refused(inspect_journal(
            &mut stream,
            length + 1,
            db(512, 2),
            geometry(),
            JournalLimits::default()
        )),
        Refusal::ByteLimit {
            limit: length,
            actual: length + 1
        }
    );
    assert_eq!(stream.bytes_read, before);
}
