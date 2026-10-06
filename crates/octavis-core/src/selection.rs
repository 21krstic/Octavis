use std::collections::{HashSet, VecDeque};

use glam::IVec3;

use crate::block::BlockId;
use crate::world::World;

/// A set of selected cells with boolean combination. Stored sparsely, so
/// it works for any shape; fine at schematic scale.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    cells: HashSet<IVec3>,
}

impl Selection {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inclusive box between two corners, in any order.
    pub fn cuboid(a: IVec3, b: IVec3) -> Self {
        let (lo, hi) = (a.min(b), a.max(b));
        let mut cells = HashSet::with_capacity(((hi - lo + IVec3::ONE).element_product()) as usize);
        for x in lo.x..=hi.x {
            for y in lo.y..=hi.y {
                for z in lo.z..=hi.z {
                    cells.insert(IVec3::new(x, y, z));
                }
            }
        }
        Self { cells }
    }

    /// Cells whose centre lies within `radius` of the centre cell.
    pub fn sphere(center: IVec3, radius: f32) -> Self {
        let r = radius.ceil() as i32;
        let mut cells = HashSet::new();
        for x in -r..=r {
            for y in -r..=r {
                for z in -r..=r {
                    let d = IVec3::new(x, y, z);
                    if (d.as_vec3().length()) <= radius {
                        cells.insert(center + d);
                    }
                }
            }
        }
        Self { cells }
    }

    /// 6-connected flood fill of cells holding the same block as `start`
    /// (air counts as a block, so it selects enclosed pockets), stopping at
    /// `limit` cells so a click in open air cannot run away.
    pub fn wand(world: &World, start: IVec3, limit: usize) -> Self {
        let target = world.get(start);
        let mut cells = HashSet::new();
        let mut queue = VecDeque::from([start]);
        cells.insert(start);
        while let Some(p) = queue.pop_front() {
            if cells.len() >= limit {
                break;
            }
            for d in [IVec3::X, IVec3::NEG_X, IVec3::Y, IVec3::NEG_Y, IVec3::Z, IVec3::NEG_Z] {
                let n = p + d;
                if !cells.contains(&n) && world.get(n) == target {
                    cells.insert(n);
                    queue.push_back(n);
                }
            }
        }
        Self { cells }
    }

    pub fn union(&mut self, other: &Selection) {
        self.cells.extend(other.cells.iter().copied());
    }

    pub fn subtract(&mut self, other: &Selection) {
        self.cells.retain(|c| !other.cells.contains(c));
    }

    pub fn intersect(&mut self, other: &Selection) {
        self.cells.retain(|c| other.cells.contains(c));
    }

    pub fn clear(&mut self) {
        self.cells.clear();
    }

    pub fn contains(&self, p: IVec3) -> bool {
        self.cells.contains(&p)
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = IVec3> + '_ {
        self.cells.iter().copied()
    }

    /// Inclusive bounding box, if non-empty.
    pub fn bounds(&self) -> Option<(IVec3, IVec3)> {
        let mut it = self.cells.iter();
        let first = *it.next()?;
        Some(it.fold((first, first), |(lo, hi), &p| (lo.min(p), hi.max(p))))
    }

    /// True if the selected cells are all air (used to warn on empty copies).
    pub fn is_all_air(&self, world: &World) -> bool {
        self.cells.iter().all(|&p| world.get(p) == BlockId::AIR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockState;

    #[test]
    fn cuboid_any_corner_order_and_size() {
        let s = Selection::cuboid(IVec3::new(2, 2, 2), IVec3::new(-1, 0, 0));
        assert_eq!(s.len(), 4 * 3 * 3);
        assert_eq!(s.bounds(), Some((IVec3::new(-1, 0, 0), IVec3::new(2, 2, 2))));
        assert!(s.contains(IVec3::ZERO));
        assert!(!s.contains(IVec3::new(3, 0, 0)));
    }

    #[test]
    fn boolean_ops() {
        let a = Selection::cuboid(IVec3::ZERO, IVec3::new(3, 0, 0)); // x 0..=3
        let b = Selection::cuboid(IVec3::new(2, 0, 0), IVec3::new(5, 0, 0)); // x 2..=5

        let mut u = a.clone();
        u.union(&b);
        assert_eq!(u.len(), 6);

        let mut d = a.clone();
        d.subtract(&b);
        assert_eq!(d.len(), 2);
        assert!(d.contains(IVec3::new(1, 0, 0)) && !d.contains(IVec3::new(2, 0, 0)));

        let mut i = a.clone();
        i.intersect(&b);
        assert_eq!(i.len(), 2);
        assert!(i.contains(IVec3::new(2, 0, 0)) && i.contains(IVec3::new(3, 0, 0)));
    }

    #[test]
    fn sphere_is_symmetric_and_roughly_right() {
        let s = Selection::sphere(IVec3::new(10, 10, 10), 4.0);
        assert!(s.contains(IVec3::new(14, 10, 10)) && s.contains(IVec3::new(6, 10, 10)));
        assert!(!s.contains(IVec3::new(15, 10, 10)));
        assert!(!s.contains(IVec3::new(13, 13, 13))); // length 5.19 > 4
        // Volume of a radius-4 ball is ~268; lattice count should be close.
        assert!((240..300).contains(&s.len()), "{}", s.len());
    }

    #[test]
    fn wand_selects_connected_same_block_only() {
        let mut w = World::new();
        let stone = w.blocks.intern(BlockState::new("stone"));
        let dirt = w.blocks.intern(BlockState::new("dirt"));
        for x in 0..3 {
            w.set(IVec3::new(x, 0, 0), stone);
        }
        w.set(IVec3::new(1, 1, 0), dirt);
        w.set(IVec3::new(10, 0, 0), stone); // disconnected

        let s = Selection::wand(&w, IVec3::ZERO, 1000);
        assert_eq!(s.len(), 3);
        assert!(!s.contains(IVec3::new(10, 0, 0)));
        assert!(!s.contains(IVec3::new(1, 1, 0)));
    }

    #[test]
    fn wand_respects_limit_in_open_air() {
        let w = World::new();
        let s = Selection::wand(&w, IVec3::ZERO, 100);
        assert!(s.len() >= 100 && s.len() < 200);
    }
}
