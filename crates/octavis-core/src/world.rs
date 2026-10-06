use std::collections::HashMap;

use glam::IVec3;

use crate::block::{BlockId, BlockTable};
use crate::section::{SECTION_SIZE, Section};

const S: i32 = SECTION_SIZE as i32;

/// An unbounded (growable on all three axes) sparse grid of sections.
/// Missing sections read as air; they are allocated on first non-air write
/// and freed again by [`World::compact`] when they become entirely air.
#[derive(Clone, Debug, Default)]
pub struct World {
    pub blocks: BlockTable,
    sections: HashMap<IVec3, Section>,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    /// Section coordinate containing a world-space block position.
    pub fn section_pos(pos: IVec3) -> IVec3 {
        pos.div_euclid(IVec3::splat(S))
    }

    fn local(pos: IVec3) -> (usize, usize, usize) {
        let l = pos.rem_euclid(IVec3::splat(S));
        (l.x as usize, l.y as usize, l.z as usize)
    }

    pub fn get(&self, pos: IVec3) -> BlockId {
        match self.sections.get(&Self::section_pos(pos)) {
            Some(s) => {
                let (x, y, z) = Self::local(pos);
                s.get(x, y, z)
            }
            None => BlockId::AIR,
        }
    }

    /// Sets a block and returns the previous value.
    pub fn set(&mut self, pos: IVec3, block: BlockId) -> BlockId {
        let sp = Self::section_pos(pos);
        let (x, y, z) = Self::local(pos);
        match self.sections.get_mut(&sp) {
            Some(s) => s.set(x, y, z, block),
            None if block == BlockId::AIR => BlockId::AIR,
            None => {
                let mut s = Section::new_filled(BlockId::AIR);
                s.set(x, y, z, block);
                self.sections.insert(sp, s);
                BlockId::AIR
            }
        }
    }

    /// Prunes palettes and removes sections that are entirely air.
    pub fn compact(&mut self) {
        self.sections.retain(|_, s| !(s.compact() && s.palette()[0] == BlockId::AIR));
    }

    pub fn sections(&self) -> impl Iterator<Item = (IVec3, &Section)> {
        self.sections.iter().map(|(p, s)| (*p, s))
    }

    pub fn section(&self, pos: IVec3) -> Option<&Section> {
        self.sections.get(&pos)
    }

    pub fn section_count(&self) -> usize {
        self.sections.len()
    }

    /// Inclusive block-space bounds of all allocated sections, if any.
    pub fn bounds(&self) -> Option<(IVec3, IVec3)> {
        let mut it = self.sections.keys();
        let first = *it.next()?;
        let (lo, hi) = it.fold((first, first), |(lo, hi), &p| (lo.min(p), hi.max(p)));
        Some((lo * S, hi * S + IVec3::splat(S - 1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockState;

    #[test]
    fn grows_in_all_directions_including_negative() {
        let mut w = World::new();
        let stone = w.blocks.intern(BlockState::new("minecraft:stone"));
        let points = [
            IVec3::new(0, 0, 0),
            IVec3::new(-1, -1, -1),
            IVec3::new(-17, 300, 5),
            IVec3::new(100_000, -64, -100_000),
        ];
        for p in points {
            assert_eq!(w.get(p), BlockId::AIR);
            w.set(p, stone);
        }
        for p in points {
            assert_eq!(w.get(p), stone);
        }
        assert_eq!(w.get(IVec3::new(1, 0, 0)), BlockId::AIR);
        assert_eq!(w.section_count(), 4);
    }

    #[test]
    fn set_returns_old_and_air_write_allocates_nothing() {
        let mut w = World::new();
        let stone = w.blocks.intern(BlockState::new("minecraft:stone"));
        assert_eq!(w.set(IVec3::ZERO, BlockId::AIR), BlockId::AIR);
        assert_eq!(w.section_count(), 0);
        assert_eq!(w.set(IVec3::ZERO, stone), BlockId::AIR);
        assert_eq!(w.set(IVec3::ZERO, BlockId::AIR), stone);
    }

    #[test]
    fn compact_frees_empty_sections() {
        let mut w = World::new();
        let stone = w.blocks.intern(BlockState::new("minecraft:stone"));
        w.set(IVec3::new(5, 5, 5), stone);
        w.set(IVec3::new(5, 5, 5), BlockId::AIR);
        w.compact();
        assert_eq!(w.section_count(), 0);
        assert!(w.bounds().is_none());
    }

    #[test]
    fn interning_is_stable_and_property_order_independent() {
        let mut t = BlockTable::default();
        let a = t.intern(BlockState::new("s").with("a", "1").with("b", "2"));
        let b = t.intern(BlockState::new("s").with("b", "2").with("a", "1"));
        assert_eq!(a, b);
        assert_ne!(a, BlockId::AIR);
    }
}
