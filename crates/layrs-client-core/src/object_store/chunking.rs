//! Content-defined chunk boundaries for the legacy object format.
use super::ChunkProfile;
pub(super) fn chunk_file(bytes: &[u8], profile: ChunkProfile) -> Vec<&[u8]> {
    if bytes.is_empty() || bytes.len() <= profile.min() {
        return vec![bytes];
    }

    let min = profile.min();
    let target = profile.target();
    let max = profile.max();
    let mask = (target.next_power_of_two() as u64) - 1;
    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut rolling = 0u64;

    for (index, byte) in bytes.iter().enumerate() {
        rolling = rolling.rotate_left(1).wrapping_add(gear_value(*byte));
        let len = index + 1 - start;
        let should_cut = len >= max || (len >= min && (rolling & mask) == 0);
        if should_cut {
            chunks.push(&bytes[start..index + 1]);
            start = index + 1;
            rolling = 0;
        }
    }

    if start < bytes.len() {
        chunks.push(&bytes[start..]);
    }
    chunks
}

fn gear_value(byte: u8) -> u64 {
    let mut value = (byte as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    value ^= value >> 33;
    value = value.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    value ^ (value >> 29)
}
