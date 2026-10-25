//! Supplied-byte test construction, independent of parser helpers. No I/O files.
use std::io::{self, Read, Seek, SeekFrom};

pub(super) const MAGIC: [u8; 8] = [0xd9, 0xd5, 5, 0xf9, 0x20, 0xa1, 0x63, 0xd7];

pub(super) fn database_header(page_size: u32) -> [u8; 100] {
    let mut header = [0; 100];
    header[..16].copy_from_slice(b"SQLite format 3\0");
    let encoded = if page_size == 65_536 {
        1
    } else {
        page_size as u16
    };
    header[16..18].copy_from_slice(&encoded.to_be_bytes());
    header[18] = 1;
    header[19] = 1;
    header[21..24].copy_from_slice(&[64, 32, 32]);
    header
}

pub(super) fn page_one(page_size: u32) -> Vec<u8> {
    let mut page = vec![0x25; page_size as usize];
    page[..100].copy_from_slice(&database_header(page_size));
    page
}

pub(super) fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

// Independent indexed-sum construction; the 512-byte literal control also has
// a handwritten checksum, so accepting only this encoder's outputs is not enough.
pub(super) fn record(seed: u32, number: u32, page: &[u8]) -> Vec<u8> {
    let sum = (1..=(page.len() - 1) / 200).fold(seed, |sum, step| {
        sum.wrapping_add(u32::from(page[page.len() - step * 200]))
    });
    let mut result = Vec::with_capacity(page.len() + 8);
    result.extend_from_slice(&number.to_be_bytes());
    result.extend_from_slice(page);
    result.extend_from_slice(&sum.to_be_bytes());
    result
}

pub(super) fn journal_header(
    page_size: u32,
    sector: u32,
    original: u32,
    count: u32,
    seed: u32,
) -> Vec<u8> {
    let mut result = vec![0xa3; sector as usize];
    result[..8].copy_from_slice(&MAGIC);
    put32(&mut result, 8, count);
    put32(&mut result, 12, seed);
    put32(&mut result, 16, original);
    put32(&mut result, 20, sector);
    put32(&mut result, 24, page_size);
    result
}

pub(super) struct Block {
    pub seed: u32,
    pub pages: Vec<(u32, Vec<u8>)>,
}

pub(super) struct Fixture {
    pub bytes: Vec<u8>,
    pub headers: Vec<usize>,
    pub records: Vec<usize>,
}

pub(super) fn journal(page_size: u32, sector: u32, original: u32, blocks: Vec<Block>) -> Fixture {
    let mut fixture = Fixture {
        bytes: Vec::new(),
        headers: Vec::new(),
        records: Vec::new(),
    };
    for block in blocks {
        while fixture.bytes.len() % sector as usize != 0 {
            fixture.bytes.push(0x6d);
        }
        fixture.headers.push(fixture.bytes.len());
        fixture.bytes.extend(journal_header(
            page_size,
            sector,
            original,
            block.pages.len() as u32,
            block.seed,
        ));
        for (number, page) in block.pages {
            assert_eq!(page.len(), page_size as usize);
            fixture.records.push(fixture.bytes.len());
            fixture.bytes.extend(record(block.seed, number, &page));
        }
    }
    fixture
}

/// A complete 1032-byte journal with independently handwritten header/checksum.
pub(super) fn literal_512() -> Vec<u8> {
    let mut bytes = vec![0xa5; 512];
    bytes[..28].copy_from_slice(&[
        0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7, 0, 0, 0, 1, // one record
        1, 2, 3, 4, // seed 0x01020304
        0, 0, 0, 2, // original pages
        0, 0, 2, 0, // sector 512
        0, 0, 2, 0, // page 512
    ]);
    bytes.extend_from_slice(&[0, 0, 0, 2]);
    bytes.extend_from_slice(&[0x11; 512]);
    // Only data[312] and data[112] contribute: seed + 0x11 + 0x11.
    bytes.extend_from_slice(&[1, 2, 3, 0x26]);
    bytes
}

pub(super) enum Segment {
    Bytes(Vec<u8>),
    Repeat { unit: Vec<u8>, count: u64 },
}

impl Segment {
    fn length(&self) -> u64 {
        match self {
            Self::Bytes(bytes) => bytes.len() as u64,
            Self::Repeat { unit, count } => unit.len() as u64 * count,
        }
    }
    fn unit(&self) -> &[u8] {
        match self {
            Self::Bytes(bytes) | Self::Repeat { unit: bytes, .. } => bytes,
        }
    }
}

/// A bounded-memory virtual byte stream, not a filesystem fixture. Repeated
/// records are deliberate parser inputs, not a claim that SQLite emits them.
pub(super) struct VirtualJournal {
    segments: Vec<Segment>,
    pub length: u64,
    position: u64,
    pub bytes_read: u64,
    pub largest_request: usize,
}

impl VirtualJournal {
    pub fn new(segments: Vec<Segment>) -> Self {
        let length = segments.iter().map(Segment::length).sum();
        Self {
            segments,
            length,
            position: 0,
            bytes_read: 0,
            largest_request: 0,
        }
    }

    pub fn exact_256_mib() -> Self {
        // 512 + 516156*520 = 268401632, then 32 alignment bytes,
        // then header 512 + 64*520 = 33792: total exactly 268435456.
        Self::new(vec![
            Segment::Bytes(journal_header(512, 512, 2, 516_156, 7)),
            Segment::Repeat {
                unit: record(7, 2, &[0; 512]),
                count: 516_156,
            },
            Segment::Bytes(vec![0x6d; 32]),
            Segment::Bytes(journal_header(512, 512, 2, 64, 13)),
            Segment::Repeat {
                unit: record(13, 2, &[0; 512]),
                count: 64,
            },
        ])
    }
}

impl Read for VirtualJournal {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.largest_request = self.largest_request.max(out.len());
        let count = out
            .len()
            .min(self.length.saturating_sub(self.position) as usize);
        let mut copied = 0;
        while copied < count {
            let mut start = 0;
            let segment = self
                .segments
                .iter()
                .find(|segment| {
                    if self.position < start + segment.length() {
                        true
                    } else {
                        start += segment.length();
                        false
                    }
                })
                .expect("position is inside virtual stream");
            let unit = segment.unit();
            let within = ((self.position - start) % unit.len() as u64) as usize;
            let n = (count - copied).min(unit.len() - within);
            out[copied..copied + n].copy_from_slice(&unit[within..within + n]);
            copied += n;
            self.position += n as u64;
        }
        self.bytes_read += copied as u64;
        Ok(copied)
    }
}

impl Seek for VirtualJournal {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let position = match position {
            SeekFrom::Start(value) => i128::from(value),
            SeekFrom::End(value) => i128::from(self.length) + i128::from(value),
            SeekFrom::Current(value) => i128::from(self.position) + i128::from(value),
        };
        self.position = u64::try_from(position).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "virtual seek out of range")
        })?;
        Ok(self.position)
    }
}
