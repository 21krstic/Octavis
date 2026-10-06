use crate::block::BlockId;

pub const SECTION_SIZE: usize = 16;
pub const SECTION_VOLUME: usize = SECTION_SIZE * SECTION_SIZE * SECTION_SIZE;

/// A 16^3 cube of blocks, palette-compressed like Minecraft's own chunk
/// sections: a local palette of distinct ids plus bit-packed indices.
/// Indices never straddle a `u64` (matches the 1.16+ on-disk layout).
/// A uniform section has a one-entry palette and no index data at all.
#[derive(Clone, Debug)]
pub struct Section {
    palette: Vec<BlockId>,
    bits: u32,
    data: Vec<u64>,
}

impl Section {
    pub fn new_filled(block: BlockId) -> Self {
        Self { palette: vec![block], bits: 0, data: Vec::new() }
    }

    pub fn is_uniform(&self) -> bool {
        self.bits == 0
    }

    /// Distinct ids the palette knows about. May include ids no longer
    /// present in the section; call [`Section::compact`] to prune them.
    pub fn palette(&self) -> &[BlockId] {
        &self.palette
    }

    /// Local coordinates must each be in `0..16`.
    pub fn get(&self, x: usize, y: usize, z: usize) -> BlockId {
        self.palette[self.index_at(Self::linear(x, y, z))]
    }

    /// Sets a block and returns the previous value.
    pub fn set(&mut self, x: usize, y: usize, z: usize, block: BlockId) -> BlockId {
        let i = Self::linear(x, y, z);
        let old = self.palette[self.index_at(i)];
        if old == block {
            return old;
        }
        let idx = match self.palette.iter().position(|&b| b == block) {
            Some(p) => p,
            None => self.add_to_palette(block),
        };
        self.write_index(i, idx);
        old
    }

    /// Drops unused palette entries and shrinks the bit width to fit.
    /// Returns true if the whole section is now a single block.
    pub fn compact(&mut self) -> bool {
        let mut used = vec![false; self.palette.len()];
        for i in 0..SECTION_VOLUME {
            used[self.index_at(i)] = true;
        }
        if used.iter().any(|u| !u) {
            let mut remap = vec![0usize; self.palette.len()];
            let mut new_palette = Vec::new();
            for (old, &u) in used.iter().enumerate() {
                if u {
                    remap[old] = new_palette.len();
                    new_palette.push(self.palette[old]);
                }
            }
            let indices: Vec<usize> =
                (0..SECTION_VOLUME).map(|i| remap[self.index_at(i)]).collect();
            self.palette = new_palette;
            self.rebuild(&indices);
        }
        self.is_uniform()
    }

    fn linear(x: usize, y: usize, z: usize) -> usize {
        debug_assert!(x < SECTION_SIZE && y < SECTION_SIZE && z < SECTION_SIZE);
        (y * SECTION_SIZE + z) * SECTION_SIZE + x
    }

    fn index_at(&self, i: usize) -> usize {
        if self.bits == 0 {
            return 0;
        }
        let per_long = 64 / self.bits as usize;
        let word = self.data[i / per_long];
        let shift = (i % per_long) as u32 * self.bits;
        ((word >> shift) & ((1u64 << self.bits) - 1)) as usize
    }

    fn write_index(&mut self, i: usize, idx: usize) {
        let per_long = 64 / self.bits as usize;
        let shift = (i % per_long) as u32 * self.bits;
        let mask = ((1u64 << self.bits) - 1) << shift;
        let word = &mut self.data[i / per_long];
        *word = (*word & !mask) | ((idx as u64) << shift);
    }

    fn add_to_palette(&mut self, block: BlockId) -> usize {
        // Read existing indices at the old width before widening.
        let indices: Vec<usize> = (0..SECTION_VOLUME).map(|i| self.index_at(i)).collect();
        self.palette.push(block);
        if bits_for(self.palette.len()) != self.bits {
            self.rebuild(&indices);
        }
        self.palette.len() - 1
    }

    /// Re-packs `indices` using the bit width implied by the palette size.
    fn rebuild(&mut self, indices: &[usize]) {
        self.bits = bits_for(self.palette.len());
        if self.bits == 0 {
            self.data = Vec::new();
            return;
        }
        let per_long = 64 / self.bits as usize;
        self.data = vec![0; SECTION_VOLUME.div_ceil(per_long)];
        for (i, &idx) in indices.iter().enumerate() {
            self.write_index(i, idx);
        }
    }
}

/// Bits per index for a palette of `n` entries; 0 for a single entry.
/// Minecraft uses at least 4 bits for multi-entry block palettes; we don't
/// need to, since I/O re-packs on export.
fn bits_for(n: usize) -> u32 {
    if n <= 1 { 0 } else { usize::BITS - (n - 1).leading_zeros() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(i: usize) -> (usize, usize, usize) {
        (i % 16, i / 256, (i / 16) % 16)
    }

    #[test]
    fn uniform_has_no_data() {
        let s = Section::new_filled(BlockId::AIR);
        assert!(s.is_uniform());
        assert_eq!(s.get(3, 4, 5), BlockId::AIR);
    }

    #[test]
    fn set_returns_previous_value() {
        let mut s = Section::new_filled(BlockId::AIR);
        assert_eq!(s.set(1, 2, 3, BlockId(5)), BlockId::AIR);
        assert_eq!(s.set(1, 2, 3, BlockId(9)), BlockId(5));
        assert_eq!(s.get(1, 2, 3), BlockId(9));
        assert_eq!(s.get(0, 0, 0), BlockId::AIR);
    }

    #[test]
    fn exact_roundtrip_across_palette_growth() {
        let mut s = Section::new_filled(BlockId::AIR);
        let mut expect = vec![BlockId::AIR; SECTION_VOLUME];
        // 23 distinct ids pass through bit widths 1..=5, repacking each time.
        for i in 0..SECTION_VOLUME {
            let id = BlockId((i.wrapping_mul(2654435761) % 23) as u32);
            let (x, y, z) = at(i);
            s.set(x, y, z, id);
            expect[i] = id;
        }
        for i in 0..SECTION_VOLUME {
            let (x, y, z) = at(i);
            assert_eq!(s.get(x, y, z), expect[i], "mismatch at {i}");
        }
    }

    #[test]
    fn compact_shrinks_to_uniform() {
        let mut s = Section::new_filled(BlockId::AIR);
        for i in 0..SECTION_VOLUME {
            let (x, y, z) = at(i);
            s.set(x, y, z, BlockId((i % 9) as u32));
        }
        for i in 0..SECTION_VOLUME {
            let (x, y, z) = at(i);
            s.set(x, y, z, BlockId(7));
        }
        assert!(s.compact());
        assert_eq!(s.palette(), &[BlockId(7)]);
        assert_eq!(s.get(1, 2, 3), BlockId(7));
    }

    #[test]
    fn bit_widths() {
        assert_eq!(bits_for(1), 0);
        assert_eq!(bits_for(2), 1);
        assert_eq!(bits_for(3), 2);
        assert_eq!(bits_for(4), 2);
        assert_eq!(bits_for(5), 3);
        assert_eq!(bits_for(256), 8);
    }
}
