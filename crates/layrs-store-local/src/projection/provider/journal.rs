//! Test-only prerequisite: restricted, supplied-stream rollback-journal admission.
//!
//! Pinned protocol: libsqlite3-sys 0.30.1, SQLite 3.46.0, source id
//! 96c92aba00c8375bc32fafcdf12429c58bd8aabfcadab6683e35bbb9cdebf19e.
//! sqlite3.c: readSuperJournal 58408, journalHdrOffset 58464,
//! readJournalHdr 58688, pager_cksum 59340, pager_playback_one_page 59387,
//! sqlite3SectorSize/setSectorSize 59791/59825, pager_playback 59898.
//! Requalify this subset when the bundled parser changes. Unlike SQLite, this
//! module refuses zero/sentinel counts, incomplete tails and duplicate page 1.
//! The reserved-page calculation assumes the qualified default PENDING_BYTE;
//! a process that changes it through SQLite test controls is outside this profile.
//!
//! All inputs are unauthenticated observations supplied by the caller. There is
//! no filesystem, SQLite, native identity, lock, process-profile or capability
//! access here. The caller must separately qualify the observations and retain
//! the required bindings across any later action. A digest is observation data,
//! not a durable authority token or proof against a concurrently changing input.

use std::fmt;
use std::io::{self, Read, Seek, SeekFrom};

#[cfg(test)]
mod tests;

const JOURNAL_MAGIC: [u8; 8] = [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7];
const DATABASE_MAGIC: &[u8; 16] = b"SQLite format 3\0";
const HEADER_BYTES: usize = 28;
const MAX_PAGE_BYTES: u32 = 65_536;
const MAX_SECTOR_BYTES: u32 = 65_536;
const PENDING_BYTE: u64 = 0x4000_0000;
const DEFAULT_MAX_BYTES: u64 = 256 * 1024 * 1024;
const DEFAULT_MAX_RECORDS: u64 = 524_288;
const DEFAULT_MAX_HEADERS: u64 = 4_096;

type Result<T> = std::result::Result<T, JournalError>;

#[derive(Debug)]
pub(super) enum JournalError {
    Refused(Refusal),
    Io(io::Error),
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(reason) => write!(f, "restricted journal refused: {reason:?}"),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for JournalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Refused(_) => None,
        }
    }
}

impl From<io::Error> for JournalError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<Refusal> for JournalError {
    fn from(reason: Refusal) -> Self {
        Self::Refused(reason)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    DatabaseHeaderLength {
        actual: usize,
    },
    DatabaseMagic,
    DatabasePageSize {
        value: u32,
    },
    DatabaseVersions {
        write: u8,
        read: u8,
    },
    DatabaseExtent {
        bytes: u64,
        page_size: u32,
    },
    LimitsExceedProfile,
    ByteLimit {
        limit: u64,
        actual: u64,
    },
    RecordLimit {
        limit: u64,
        attempted: u64,
    },
    HeaderLimit {
        limit: u64,
        attempted: u64,
    },
    LengthChanged {
        expected: u64,
        actual: u64,
    },
    SeekPosition {
        expected: u64,
        actual: u64,
    },
    TooShortForFooterProbe {
        bytes: u64,
    },
    SuperJournalTrailerSignature,
    InitialHeaderDoesNotFit {
        sector: u32,
        bytes: u64,
    },
    HeaderDoesNotFit {
        offset: u64,
        sector: u32,
    },
    JournalMagic {
        offset: u64,
    },
    JournalPageSize {
        offset: u64,
        value: u32,
    },
    JournalSectorSize {
        offset: u64,
        value: u32,
    },
    PageSizeMismatch {
        offset: u64,
        expected: u32,
        actual: u32,
    },
    HeaderGeometryChanged {
        offset: u64,
    },
    ZeroOriginalPages {
        offset: u64,
    },
    OriginalExtent {
        bytes: u64,
        database_bytes: u64,
    },
    OriginalPagesChanged {
        offset: u64,
    },
    UnsupportedRecordCount {
        offset: u64,
        value: u32,
    },
    IncompleteRecords {
        offset: u64,
        required_end: u64,
        bytes: u64,
    },
    IllegalPageNumber {
        offset: u64,
        page: u32,
    },
    PageOutOfRange {
        offset: u64,
        page: u32,
        original_pages: u32,
    },
    Checksum {
        offset: u64,
        expected: u32,
        actual: u32,
    },
    PageOneMagic {
        offset: u64,
    },
    PageOnePageSize {
        offset: u64,
        expected: u32,
        actual: u32,
    },
    PageOneVersions {
        offset: u64,
        write: u8,
        read: u8,
    },
    DuplicatePageOne {
        offset: u64,
    },
    TrailingBytes {
        offset: u64,
        bytes: u64,
    },
    ArithmeticOverflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DatabaseHeaderEvidence {
    byte_length: u64,
    page_size: u32,
}

impl DatabaseHeaderEvidence {
    /// Validate supplied header bytes and length; this does not observe a file.
    pub(super) fn from_observed(header: &[u8], byte_length: u64) -> Result<Self> {
        if header.len() != 100 {
            return Err(Refusal::DatabaseHeaderLength {
                actual: header.len(),
            }
            .into());
        }
        if &header[..16] != DATABASE_MAGIC {
            return Err(Refusal::DatabaseMagic.into());
        }
        let page_size = database_page_size(header);
        if !valid_page_size(page_size) {
            return Err(Refusal::DatabasePageSize { value: page_size }.into());
        }
        if header[18] != 1 || header[19] != 1 {
            return Err(Refusal::DatabaseVersions {
                write: header[18],
                read: header[19],
            }
            .into());
        }
        if byte_length == 0 || byte_length % u64::from(page_size) != 0 {
            return Err(Refusal::DatabaseExtent {
                bytes: byte_length,
                page_size,
            }
            .into());
        }
        Ok(Self {
            byte_length,
            page_size,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ReplayGeometry {
    observed_sector_size: i32,
    observed_powersafe_overwrite: bool,
    effective_sector_size: u32,
}

impl ReplayGeometry {
    /// Pure reproduction of setSectorSize for a non-temporary database. The
    /// caller supplies observations; this function neither obtains nor trusts
    /// SQLite file pointers. Equality compares the complete supplied tuple.
    pub(super) fn from_observed_device(sector_size: i32, powersafe_overwrite: bool) -> Self {
        let effective_sector_size = if powersafe_overwrite || sector_size < 32 {
            512
        } else if sector_size > MAX_SECTOR_BYTES as i32 {
            MAX_SECTOR_BYTES
        } else {
            sector_size as u32
        };
        Self {
            observed_sector_size: sector_size,
            observed_powersafe_overwrite: powersafe_overwrite,
            effective_sector_size,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct JournalLimits {
    max_bytes: u64,
    max_records: u64,
    max_headers: u64,
}

impl Default for JournalLimits {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_BYTES,
            max_records: DEFAULT_MAX_RECORDS,
            max_headers: DEFAULT_MAX_HEADERS,
        }
    }
}

impl JournalLimits {
    /// Callers may tighten, but cannot silently widen, the restricted profile.
    fn validate(self) -> Result<()> {
        if self.max_bytes > DEFAULT_MAX_BYTES
            || self.max_records > DEFAULT_MAX_RECORDS
            || self.max_headers > DEFAULT_MAX_HEADERS
        {
            return Err(Refusal::LimitsExceedProfile.into());
        }
        Ok(())
    }
}

/// Intentionally neither Clone nor serialized. Only supplied-stream data;
/// native evidence, operation ownership and any RW decision remain elsewhere.
#[derive(Debug)]
#[must_use]
pub(super) struct JournalAdmission {
    database: DatabaseHeaderEvidence,
    geometry: ReplayGeometry,
    journal_bytes: u64,
    journal_page_size: u32,
    journal_sector_size: u32,
    original_pages: u32,
    original_bytes: u64,
    headers: u64,
    records: u64,
    page_one_seen: bool,
    consumed_byte_digest: [u8; 32],
}

pub(super) fn inspect_journal<R: Read + Seek>(
    reader: &mut R,
    journal_length: u64,
    database: DatabaseHeaderEvidence,
    geometry: ReplayGeometry,
    limits: JournalLimits,
) -> Result<JournalAdmission> {
    limits.validate()?;
    if journal_length > limits.max_bytes {
        return Err(Refusal::ByteLimit {
            limit: limits.max_bytes,
            actual: journal_length,
        }
        .into());
    }
    check_length(reader, journal_length)?;
    if journal_length < 8 {
        return Err(Refusal::TooShortForFooterProbe {
            bytes: journal_length,
        }
        .into());
    }

    // readSuperJournal examines physical EOF independently of replay records.
    // Reject even malformed/colliding trailer signatures; never interpret a
    // filename, checksum or path supplied by that trailer. This is the FIRST
    // read, before header adoption or any record processing.
    seek_to(reader, journal_length - 8)?;
    let mut footer = [0; 8];
    reader.read_exact(&mut footer)?;
    if footer == JOURNAL_MAGIC {
        return Err(Refusal::SuperJournalTrailerSignature.into());
    }
    if journal_length < u64::from(geometry.effective_sector_size) {
        return Err(Refusal::InitialHeaderDoesNotFit {
            sector: geometry.effective_sector_size,
            bytes: journal_length,
        }
        .into());
    }
    seek_to(reader, 0)?;

    let mut digest = blake3::Hasher::new();
    let mut offset = 0u64;
    let mut headers = 0u64;
    let mut records = 0u64;
    let mut sector = geometry.effective_sector_size;
    let mut original_pages = 0;
    let mut original_bytes = 0;
    let mut page_one_seen = false;
    // At most two 64 KiB working allocations; no journal/record collection.
    let mut scratch = vec![0; MAX_SECTOR_BYTES as usize];
    let mut page = vec![0; database.page_size as usize];

    loop {
        let header_offset = offset;
        headers = count_headers(headers, 1, limits.max_headers)?;
        // The initial fit uses connection geometry. Subsequent fits use the
        // first header's sector, exactly as readJournalHdr/journalHdrOffset.
        if add(offset, u64::from(sector))? > journal_length {
            return Err(Refusal::HeaderDoesNotFit { offset, sector }.into());
        }
        let mut header = [0; HEADER_BYTES];
        read_hashed(reader, &mut digest, &mut offset, &mut header)?;
        if header[..8] != JOURNAL_MAGIC {
            return Err(Refusal::JournalMagic {
                offset: header_offset,
            }
            .into());
        }
        let n_rec = be32(&header[8..12]);
        let seed = be32(&header[12..16]);
        let header_pages = be32(&header[16..20]);
        let header_sector = be32(&header[20..24]);
        let header_page_size = be32(&header[24..28]);
        if !valid_page_size(header_page_size) {
            return Err(Refusal::JournalPageSize {
                offset: header_offset,
                value: header_page_size,
            }
            .into());
        }
        if !(32..=MAX_SECTOR_BYTES).contains(&header_sector) || !header_sector.is_power_of_two() {
            return Err(Refusal::JournalSectorSize {
                offset: header_offset,
                value: header_sector,
            }
            .into());
        }
        if header_page_size != database.page_size {
            return Err(Refusal::PageSizeMismatch {
                offset: header_offset,
                expected: database.page_size,
                actual: header_page_size,
            }
            .into());
        }
        if header_pages == 0 {
            return Err(Refusal::ZeroOriginalPages {
                offset: header_offset,
            }
            .into());
        }
        if n_rec == 0 || n_rec == u32::MAX {
            return Err(Refusal::UnsupportedRecordCount {
                offset: header_offset,
                value: n_rec,
            }
            .into());
        }
        if headers == 1 {
            sector = header_sector;
            original_pages = header_pages;
            original_bytes = multiply(u64::from(header_pages), u64::from(header_page_size))?;
            if original_bytes > database.byte_length {
                return Err(Refusal::OriginalExtent {
                    bytes: original_bytes,
                    database_bytes: database.byte_length,
                }
                .into());
            }
        } else {
            // SQLite only adopts sector/page size from the first header. We
            // additionally reject inconsistent later fields instead of ignoring
            // them; every header still supplies its own checksum seed.
            if header_sector != sector {
                return Err(Refusal::HeaderGeometryChanged {
                    offset: header_offset,
                }
                .into());
            }
            if header_pages != original_pages {
                return Err(Refusal::OriginalPagesChanged {
                    offset: header_offset,
                }
                .into());
            }
        }
        let header_end = add(header_offset, u64::from(sector))?;
        if header_end > journal_length {
            return Err(Refusal::HeaderDoesNotFit {
                offset: header_offset,
                sector,
            }
            .into());
        }
        records = count_records(records, u64::from(n_rec), limits.max_records)?;
        let record_bytes = multiply(u64::from(n_rec), add(u64::from(database.page_size), 8)?)?;
        let records_end = add(header_end, record_bytes)?;
        if records_end > journal_length {
            return Err(Refusal::IncompleteRecords {
                offset: header_offset,
                required_end: records_end,
                bytes: journal_length,
            }
            .into());
        }
        consume_padding(reader, &mut digest, &mut offset, header_end, &mut scratch)?;

        for _ in 0..n_rec {
            let record_offset = offset;
            let mut field = [0; 4];
            read_hashed(reader, &mut digest, &mut offset, &mut field)?;
            let page_number = be32(&field);
            let reserved_page = PENDING_BYTE / u64::from(database.page_size) + 1;
            if page_number == 0 || u64::from(page_number) == reserved_page {
                return Err(Refusal::IllegalPageNumber {
                    offset: record_offset,
                    page: page_number,
                }
                .into());
            }
            if page_number > original_pages {
                return Err(Refusal::PageOutOfRange {
                    offset: record_offset,
                    page: page_number,
                    original_pages,
                }
                .into());
            }
            read_hashed(reader, &mut digest, &mut offset, &mut page)?;
            read_hashed(reader, &mut digest, &mut offset, &mut field)?;
            let actual = be32(&field);
            let expected = checksum(seed, &page);
            if actual != expected {
                return Err(Refusal::Checksum {
                    offset: record_offset,
                    expected,
                    actual,
                }
                .into());
            }
            if page_number == 1 {
                validate_page_one(&page, database.page_size, record_offset)?;
                if page_one_seen {
                    return Err(Refusal::DuplicatePageOne {
                        offset: record_offset,
                    }
                    .into());
                }
                page_one_seen = true;
            }
        }
        debug_assert_eq!(offset, records_end);
        if offset == journal_length {
            // Check both stream EOF and reported length again. This detects
            // observable drift, not atomicity against a changing supplied stream.
            let mut extra = [0; 1];
            if reader.read(&mut extra)? != 0 {
                return Err(Refusal::TrailingBytes { offset, bytes: 1 }.into());
            }
            check_length(reader, journal_length)?;
            return Ok(JournalAdmission {
                database,
                geometry,
                journal_bytes: journal_length,
                journal_page_size: database.page_size,
                journal_sector_size: sector,
                original_pages,
                original_bytes,
                headers,
                records,
                page_one_seen,
                consumed_byte_digest: *digest.finalize().as_bytes(),
            });
        }

        let next_header = align(offset, u64::from(sector))?;
        if add(next_header, u64::from(sector))? > journal_length {
            return Err(Refusal::TrailingBytes {
                offset,
                bytes: journal_length - offset,
            }
            .into());
        }
        consume_padding(reader, &mut digest, &mut offset, next_header, &mut scratch)?;
    }
}

fn validate_page_one(page: &[u8], expected_size: u32, offset: u64) -> Result<()> {
    if &page[..16] != DATABASE_MAGIC {
        return Err(Refusal::PageOneMagic { offset }.into());
    }
    let actual = database_page_size(page);
    if actual != expected_size {
        return Err(Refusal::PageOnePageSize {
            offset,
            expected: expected_size,
            actual,
        }
        .into());
    }
    if page[18] != 1 || page[19] != 1 {
        return Err(Refusal::PageOneVersions {
            offset,
            write: page[18],
            read: page[19],
        }
        .into());
    }
    Ok(())
}

fn valid_page_size(value: u32) -> bool {
    (512..=MAX_PAGE_BYTES).contains(&value) && value.is_power_of_two()
}

fn database_page_size(header: &[u8]) -> u32 {
    let encoded = u16::from_be_bytes([header[16], header[17]]);
    if encoded == 1 {
        65_536
    } else {
        u32::from(encoded)
    }
}

fn checksum(seed: u32, page: &[u8]) -> u32 {
    let mut value = seed;
    let mut index = page.len() - 200;
    while index > 0 {
        value = value.wrapping_add(u32::from(page[index]));
        index = index.saturating_sub(200);
    }
    value
}

fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b)
        .ok_or_else(|| Refusal::ArithmeticOverflow.into())
}

fn multiply(a: u64, b: u64) -> Result<u64> {
    a.checked_mul(b)
        .ok_or_else(|| Refusal::ArithmeticOverflow.into())
}

fn align(offset: u64, sector: u64) -> Result<u64> {
    if offset == 0 {
        return Ok(0);
    }
    multiply(add((offset - 1) / sector, 1)?, sector)
}

fn count_records(current: u64, more: u64, limit: u64) -> Result<u64> {
    let attempted = add(current, more)?;
    if attempted > limit {
        Err(Refusal::RecordLimit { limit, attempted }.into())
    } else {
        Ok(attempted)
    }
}

fn count_headers(current: u64, more: u64, limit: u64) -> Result<u64> {
    let attempted = add(current, more)?;
    if attempted > limit {
        Err(Refusal::HeaderLimit { limit, attempted }.into())
    } else {
        Ok(attempted)
    }
}

fn check_length<R: Seek>(reader: &mut R, expected: u64) -> Result<()> {
    let actual = reader.seek(SeekFrom::End(0))?;
    if actual != expected {
        return Err(Refusal::LengthChanged { expected, actual }.into());
    }
    Ok(())
}

fn seek_to<R: Seek>(reader: &mut R, expected: u64) -> Result<()> {
    let actual = reader.seek(SeekFrom::Start(expected))?;
    if actual != expected {
        return Err(Refusal::SeekPosition { expected, actual }.into());
    }
    Ok(())
}

fn read_hashed<R: Read>(
    reader: &mut R,
    digest: &mut blake3::Hasher,
    offset: &mut u64,
    bytes: &mut [u8],
) -> Result<()> {
    let end = add(*offset, bytes.len() as u64)?;
    reader.read_exact(bytes)?;
    digest.update(bytes);
    *offset = end;
    Ok(())
}

fn consume_padding<R: Read>(
    reader: &mut R,
    digest: &mut blake3::Hasher,
    offset: &mut u64,
    end: u64,
    scratch: &mut [u8],
) -> Result<()> {
    while *offset < end {
        let amount = (end - *offset).min(scratch.len() as u64) as usize;
        read_hashed(reader, digest, offset, &mut scratch[..amount])?;
    }
    Ok(())
}
