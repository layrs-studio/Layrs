#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ByteRange {
    pub start: u64,
    pub end_exclusive: u64,
}

impl ByteRange {
    pub fn new(start: u64, end_exclusive: u64) -> Option<Self> {
        (start <= end_exclusive).then_some(Self {
            start,
            end_exclusive,
        })
    }

    pub const fn len(self) -> u64 {
        self.end_exclusive - self.start
    }

    pub const fn is_empty(self) -> bool {
        self.start == self.end_exclusive
    }

    pub const fn is_full(self, total: u64) -> bool {
        self.start == 0 && self.end_exclusive == total
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestedRange {
    Inclusive { start: u64, end: u64 },
    From { start: u64 },
    Suffix { length: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RangeParseError {
    Invalid,
    Multiple,
}

pub fn parse_range_header(value: &str) -> Result<RequestedRange, RangeParseError> {
    let value = value
        .strip_prefix("bytes=")
        .ok_or(RangeParseError::Invalid)?;
    if value.contains(',') {
        return Err(RangeParseError::Multiple);
    }
    let (start, end) = value.split_once('-').ok_or(RangeParseError::Invalid)?;
    match (start.is_empty(), end.is_empty()) {
        (true, true) => Err(RangeParseError::Invalid),
        (true, false) => {
            let length = parse_decimal(end)?;
            (length > 0)
                .then_some(RequestedRange::Suffix { length })
                .ok_or(RangeParseError::Invalid)
        }
        (false, true) => Ok(RequestedRange::From {
            start: parse_decimal(start)?,
        }),
        (false, false) => {
            let start = parse_decimal(start)?;
            let end = parse_decimal(end)?;
            (start <= end)
                .then_some(RequestedRange::Inclusive { start, end })
                .ok_or(RangeParseError::Invalid)
        }
    }
}

pub fn resolve_range(requested: RequestedRange, total: u64) -> Option<ByteRange> {
    if total == 0 {
        return None;
    }
    match requested {
        RequestedRange::Inclusive { start, end } if start < total => {
            ByteRange::new(start, end.min(total - 1).checked_add(1)?)
        }
        RequestedRange::From { start } if start < total => ByteRange::new(start, total),
        RequestedRange::Suffix { length } => {
            ByteRange::new(total.saturating_sub(length.min(total)), total)
        }
        _ => None,
    }
}

fn parse_decimal(value: &str) -> Result<u64, RangeParseError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(RangeParseError::Invalid);
    }
    value.parse().map_err(|_| RangeParseError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_single_rfc_byte_ranges() {
        assert_eq!(
            parse_range_header("bytes=2-9"),
            Ok(RequestedRange::Inclusive { start: 2, end: 9 })
        );
        assert_eq!(
            parse_range_header("bytes=2-"),
            Ok(RequestedRange::From { start: 2 })
        );
        assert_eq!(
            parse_range_header("bytes=-5"),
            Ok(RequestedRange::Suffix { length: 5 })
        );
    }

    #[test]
    fn rejects_multiple_or_malformed_ranges() {
        assert_eq!(
            parse_range_header("bytes=0-1,4-5"),
            Err(RangeParseError::Multiple)
        );
        assert_eq!(
            parse_range_header("bytes=5-2"),
            Err(RangeParseError::Invalid)
        );
        assert_eq!(
            parse_range_header("items=0-2"),
            Err(RangeParseError::Invalid)
        );
        assert_eq!(
            parse_range_header("bytes=-0"),
            Err(RangeParseError::Invalid)
        );
    }

    #[test]
    fn resolves_and_clamps_ranges() {
        assert_eq!(
            resolve_range(RequestedRange::Inclusive { start: 2, end: 99 }, 10),
            ByteRange::new(2, 10)
        );
        assert_eq!(
            resolve_range(RequestedRange::Suffix { length: 3 }, 10),
            ByteRange::new(7, 10)
        );
        assert_eq!(resolve_range(RequestedRange::From { start: 10 }, 10), None);
    }
}
