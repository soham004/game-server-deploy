//! `CurseForge`'s file fingerprint: `MurmurHash2` (32-bit, seed 1) of the file with its whitespace
//! bytes left out. It is how `CurseForge` recognises a file it has been given.

/// The bytes `CurseForge` leaves out: tab, line feed, carriage return, space.
const WHITESPACE: [u8; 4] = [9, 10, 13, 32];
const SEED: u32 = 1;

/// The fingerprint of a file's contents.
pub fn fingerprint(bytes: &[u8]) -> u32 {
    let kept: Vec<u8> = bytes
        .iter()
        .copied()
        .filter(|byte| !WHITESPACE.contains(byte))
        .collect();
    murmur2(&kept, SEED)
}

fn murmur2(data: &[u8], seed: u32) -> u32 {
    const M: u32 = 0x5bd1_e995;
    const R: u32 = 24;
    // MurmurHash2 is defined over a 32-bit length; only the low bits of a longer one count.
    #[allow(clippy::cast_possible_truncation)]
    let mut hash = seed ^ data.len() as u32;
    let (chunks, tail) = data.as_chunks::<4>();
    for chunk in chunks {
        let mut k = u32::from_le_bytes(*chunk);
        k = k.wrapping_mul(M);
        k ^= k >> R;
        k = k.wrapping_mul(M);
        hash = hash.wrapping_mul(M) ^ k;
    }
    if !tail.is_empty() {
        for (position, byte) in tail.iter().enumerate() {
            hash ^= u32::from(*byte) << (8 * position);
        }
        hash = hash.wrapping_mul(M);
    }
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(M);
    hash ^ (hash >> 15)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values from another implementation of the same algorithm: Apache Kafka's `UtilsTest`
    /// (its partitioner uses `MurmurHash2` with this seed, and lists the results as signed).
    #[test]
    fn matches_a_published_implementation() {
        let seed = 0x9747_b28c;
        for (input, expected) in [
            (&b"21"[..], -973_932_308_i32),
            (b"foobar", -790_332_482),
            (b"a-little-bit-long-string", -985_981_536),
            (b"a-little-bit-longer-string", -1_486_304_829),
            (b"lkjh234lh9fiuh90y23oiuhsafujhadof229phr9h19h89h8", -58_897_971),
            (b"abc", 479_470_107),
        ] {
            assert_eq!(
                murmur2(input, seed).cast_signed(),
                expected,
                "{}",
                String::from_utf8_lossy(input)
            );
        }
        assert_eq!(murmur2(b"", 0), 0);
    }

    #[test]
    fn whitespace_does_not_count() {
        assert_eq!(fingerprint(b"a b\tc\r\nd"), fingerprint(b"abcd"));
        assert_ne!(fingerprint(b"abcd"), fingerprint(b"abce"));
    }

    #[test]
    fn every_tail_length_counts() {
        let all: Vec<u32> = (0..8).map(|length| fingerprint(&b"abcdefgh"[..length])).collect();
        for (position, value) in all.iter().enumerate() {
            assert!(!all[..position].contains(value), "{all:?}");
        }
    }
}
