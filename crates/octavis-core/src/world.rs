use std::ops::{Deref, DerefMut};

use glam::IVec3;

use crate::block::{BlockId, BlockTable};
use crate::hash::CellMap;
use crate::section::{SECTION_SIZE, Section};

const S: i32 = SECTION_SIZE as i32;

/// An unbounded (growable on all three axes) sparse grid of sections.
/// Missing sections read as air; they are allocated on first non-air write
/// and freed again by [`Grid::compact`] when they become entirely air.
///
/// A grid holds only cells; the block ids in it mean something relative to
/// a [`BlockTable`] (see [`World`]). Layers each own a grid and share one
/// table.
#[derive(Clone, Debug, Default)]
pub struct Grid {
    sections: CellMap<IVec3, Section>,
}

impl Grid {
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

    /// Removes every cell.
    pub fn clear(&mut self) {
        self.sections.clear();
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

    /// Every non-air cell with its block.
    pub fn solid_cells(&self) -> Vec<(IVec3, BlockId)> {
        let mut out = Vec::new();
        for (sp, section) in &self.sections {
            if section.palette().iter().all(|&b| b == BlockId::AIR) {
                continue;
            }
            for z in 0..SECTION_SIZE {
                for y in 0..SECTION_SIZE {
                    for x in 0..SECTION_SIZE {
                        let b = section.get(x, y, z);
                        if b != BlockId::AIR {
                            out.push((*sp * S + IVec3::new(x as i32, y as i32, z as i32), b));
                        }
                    }
                }
            }
        }
        out
    }

    /// Inclusive bounds of the non-air blocks, if there are any. Exact,
    /// unlike [`Grid::bounds`], which is section-aligned.
    pub fn content_bounds(&self) -> Option<(IVec3, IVec3)> {
        let mut acc: Option<(IVec3, IVec3)> = None;
        for (sp, section) in &self.sections {
            if section.palette().iter().all(|&b| b == BlockId::AIR) {
                continue;
            }
            for z in 0..SECTION_SIZE {
                for y in 0..SECTION_SIZE {
                    for x in 0..SECTION_SIZE {
                        if section.get(x, y, z) != BlockId::AIR {
                            let p = *sp * S + IVec3::new(x as i32, y as i32, z as i32);
                            acc = Some(acc.map_or((p, p), |(lo, hi)| (lo.min(p), hi.max(p))));
                        }
                    }
                }
            }
        }
        acc
    }

    /// Inclusive block-space bounds of all allocated sections, if any.
    pub fn bounds(&self) -> Option<(IVec3, IVec3)> {
        let mut it = self.sections.keys();
        let first = *it.next()?;
        let (lo, hi) = it.fold((first, first), |(lo, hi), &p| (lo.min(p), hi.max(p)));
        Some((lo * S, hi * S + IVec3::splat(S - 1)))
    }
}

/// A grid plus the table that gives its block ids meaning. The editor keeps
/// one of these as the *composite* of all visible layers.
#[derive(Clone, Debug, Default)]
pub struct World {
    pub blocks: BlockTable,
    grid: Grid,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    /// Section coordinate containing a world-space block position.
    pub fn section_pos(pos: IVec3) -> IVec3 {
        Grid::section_pos(pos)
    }

    /// A world over an existing grid of cells.
    pub fn from_grid(blocks: BlockTable, grid: Grid) -> Self {
        Self { blocks, grid }
    }
}

impl Deref for World {
    type Target = Grid;
    fn deref(&self) -> &Grid {
        &self.grid
    }
}

impl DerefMut for World {
    fn deref_mut(&mut self) -> &mut Grid {
        &mut self.grid
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockState;

    #[test]
    fn content_bounds_are_exact() {
        let mut w = World::new();
        assert_eq!(w.content_bounds(), None);
        let stone = w.blocks.intern(BlockState::new("minecraft:stone"));
        w.set(IVec3::new(-3, 20, 5), stone);
        w.set(IVec3::new(40, -7, 5), stone);
        assert_eq!(w.content_bounds(), Some((IVec3::new(-3, -7, 5), IVec3::new(40, 20, 5))));
        w.set(IVec3::new(40, -7, 5), BlockId::AIR);
        assert_eq!(w.content_bounds(), Some((IVec3::new(-3, 20, 5), IVec3::new(-3, 20, 5))));
    }

    #[test]
    fn solid_cells_lists_every_block() {
        let mut g = Grid::new();
        g.set(IVec3::new(-1, 0, 0), BlockId(3));
        g.set(IVec3::new(20, 5, -40), BlockId(4));
        let mut cells = g.solid_cells();
        cells.sort_by_key(|c| c.0.x);
        assert_eq!(cells, vec![(IVec3::new(-1, 0, 0), BlockId(3)), (IVec3::new(20, 5, -40), BlockId(4))]);
        g.clear();
        assert!(g.solid_cells().is_empty());
    }

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
