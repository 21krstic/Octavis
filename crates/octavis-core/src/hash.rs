//! A fast hasher for cell and section coordinates.
//!
//! The standard hasher (SipHash) resists hash flooding, which matters for
//! untrusted keys, not for coordinates. This one is several times faster on
//! small integer keys, and deterministic.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

#[derive(Clone, Copy, Default)]
pub struct CellHasher(u64);

impl CellHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

impl Hasher for CellHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(i as u64);
    }

    #[inline]
    fn write_i32(&mut self, i: i32) {
        self.add(i as u32 as u64);
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }

    /// Murmur3's finaliser, so every input bit reaches the low bits the
    /// table picks buckets with.
    #[inline]
    fn finish(&self) -> u64 {
        let mut h = self.0;
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^ (h >> 33)
    }
}

pub type CellBuildHasher = BuildHasherDefault<CellHasher>;
/// A `HashMap` keyed by coordinates (or other small integers).
pub type CellMap<K, V> = HashMap<K, V, CellBuildHasher>;
/// A `HashSet` of coordinates (or other small integers).
pub type CellSet<K> = HashSet<K, CellBuildHasher>;

#[cfg(test)]
mod tests {
    use super::*;
    use glam::IVec3;
    use std::hash::{BuildHasher, Hash};

    #[test]
    fn neighbouring_cells_spread_over_buckets() {
        // Low bits pick the bucket: a 32^3 block of cells should use most
        // of 2^15 buckets, not pile into a few.
        let build = CellBuildHasher::default();
        let mut used = CellSet::default();
        for x in 0..32 {
            for y in 0..32 {
                for z in 0..32 {
                    let mut h = build.build_hasher();
                    IVec3::new(x, y, z).hash(&mut h);
                    used.insert(h.finish() & 0x7fff);
                }
            }
        }
        assert!(used.len() > 18_000, "{} of 32768 buckets", used.len());
    }
}
