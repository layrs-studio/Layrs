use crate::{Error, Result};

pub const READ_BUFFER_SIZE: usize = 256 * 1024;
const ALIGNMENT: u64 = 4096;

pub fn checked_range(size: u64, offset: u64, length: u64) -> Result<std::ops::Range<u64>> {
    let end = offset.checked_add(length).ok_or(Error::InvalidRange)?;
    if size > i64::MAX as u64 || end > size {
        return Err(Error::InvalidRange);
    }
    Ok(offset..end)
}

/// Aligned transfer chunks; the final chunk may end exactly at EOF.
#[derive(Debug, Clone)]
pub struct HydrationRange {
    next: u64,
    end: u64,
}
impl HydrationRange {
    pub(crate) fn bounds(&self) -> std::ops::Range<u64> {
        self.next..self.end
    }
    pub fn new(size: u64, offset: i64, length: i64) -> Result<Self> {
        if offset < 0 || length == 0 || length < -1 {
            return Err(Error::InvalidRange);
        }
        let length = if length == -1 {
            size.checked_sub(offset as u64)
                .filter(|length| *length != 0)
                .ok_or(Error::InvalidRange)?
        } else {
            length as u64
        };
        let requested = checked_range(size, offset as u64, length)?;
        let next = requested.start / ALIGNMENT * ALIGNMENT;
        let end = requested
            .end
            .checked_add(ALIGNMENT - 1)
            .ok_or(Error::InvalidRange)?
            / ALIGNMENT
            * ALIGNMENT;
        Ok(Self {
            next,
            end: end.min(size),
        })
    }
}
impl Iterator for HydrationRange {
    type Item = std::ops::Range<u64>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.next == self.end {
            return None;
        }
        let end = self
            .next
            .saturating_add(READ_BUFFER_SIZE as u64)
            .min(self.end);
        let result = self.next..end;
        self.next = end;
        Some(result)
    }
}
