//! Moving, rotating, scaling and flipping a set of cells about a pivot.
//!
//! Quarter turns and flips at unit scale move every cell exactly. Other
//! angles and scales resample: each destination cell takes the source cell
//! its centre maps back to (nearest neighbour), so results have no holes.

use glam::{DMat3, DVec3, IVec3};

use crate::block::{BlockId, BlockTable};
use crate::hash::CellMap;
use crate::orient::{AxisMap, transform_state};

/// Sources with bounds up to this many cells are indexed densely.
const DENSE_INDEX_LIMIT: i64 = 8_000_000;

/// Rotation first, then scale and flip along the world axes, all about the
/// centre of the pivot cell, then the offset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub offset: IVec3,
    /// Degrees about X, Y and Z, applied in that order. Positive turns are
    /// counter-clockwise looking from the positive end of the axis.
    pub rotation: [f64; 3],
    pub scale: [f64; 3],
    pub flip: [bool; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// Rotation about one axis; quarter turns are exact.
fn axis_rotation(axis: usize, degrees: f64) -> DMat3 {
    let quarters = degrees / 90.0;
    let (s, c) = if (quarters - quarters.round()).abs() < 1e-9 {
        [(0.0, 1.0), (1.0, 0.0), (0.0, -1.0), (-1.0, 0.0)][quarters.round().rem_euclid(4.0) as usize]
    } else {
        degrees.to_radians().sin_cos()
    };
    match axis {
        0 => DMat3::from_cols(DVec3::X, DVec3::new(0.0, c, s), DVec3::new(0.0, -s, c)),
        1 => DMat3::from_cols(DVec3::new(c, 0.0, -s), DVec3::Y, DVec3::new(s, 0.0, c)),
        _ => DMat3::from_cols(DVec3::new(c, s, 0.0), DVec3::new(-s, c, 0.0), DVec3::Z),
    }
}

impl Transform {
    pub const IDENTITY: Transform =
        Transform { offset: IVec3::ZERO, rotation: [0.0; 3], scale: [1.0; 3], flip: [false; 3] };

    /// The cell whose centre a box `lo..=hi` turns about: its middle cell.
    pub fn pivot_of(lo: IVec3, hi: IVec3) -> IVec3 {
        (lo + hi).div_euclid(IVec3::splat(2))
    }

    fn rotation_matrix(&self) -> DMat3 {
        axis_rotation(2, self.rotation[2]) * axis_rotation(1, self.rotation[1]) * axis_rotation(0, self.rotation[0])
    }

    fn flips(&self) -> DVec3 {
        DVec3::from_array(self.flip.map(|f| if f { -1.0 } else { 1.0 }))
    }

    /// The linear part: flip and scale after rotation.
    pub fn matrix(&self) -> DMat3 {
        DMat3::from_diagonal(self.flips() * DVec3::from_array(self.scale)) * self.rotation_matrix()
    }

    /// How directions turn, ignoring scale (for block states).
    pub fn orientation(&self) -> AxisMap {
        AxisMap::nearest(DMat3::from_diagonal(self.flips()) * self.rotation_matrix())
    }

    /// Moves `cells` (any payload). `max_volume` caps the box a resampled
    /// result may span.
    pub fn map_cells<T: Copy>(
        &self,
        cells: &[(IVec3, T)],
        pivot: IVec3,
        max_volume: u64,
    ) -> Result<Vec<(IVec3, T)>, String> {
        let m = self.matrix();
        if let Some(exact) = AxisMap::exact(m) {
            let shift = pivot + self.offset;
            return Ok(cells.iter().map(|&(p, v)| (exact.apply(p - pivot) + shift, v)).collect());
        }
        let Some((lo, hi)) = bounds(cells) else { return Ok(Vec::new()) };
        if m.determinant().abs() < 1e-12 {
            return Err("Scale must be above zero".into());
        }
        let inv = m.inverse();
        let centre = pivot.as_dvec3() + DVec3::splat(0.5);
        let offset = self.offset.as_dvec3();

        // The destination box: the source box's corners, transformed.
        let (mut dmin, mut dmax) = (DVec3::INFINITY, DVec3::NEG_INFINITY);
        for i in 0..8 {
            let corner = DVec3::new(
                if i & 1 == 0 { lo.x } else { hi.x + 1 } as f64,
                if i & 2 == 0 { lo.y } else { hi.y + 1 } as f64,
                if i & 4 == 0 { lo.z } else { hi.z + 1 } as f64,
            );
            let q = m * (corner - centre) + centre + offset;
            dmin = dmin.min(q);
            dmax = dmax.max(q);
        }
        let from = dmin.floor().as_ivec3();
        let to = dmax.ceil().as_ivec3() - IVec3::ONE;
        let size = (to - from + IVec3::ONE).max(IVec3::ZERO).as_i64vec3();
        let volume = (size.x * size.y * size.z) as u64;
        if volume > max_volume {
            return Err(format!("The result would span {volume} cells (limit {max_volume})"));
        }

        let index = Index::new(cells, lo, hi);
        // A tiny bias sends centres that land exactly on a cell boundary
        // (even scale factors) the same way every time.
        let bias = DVec3::splat(1e-7);
        let mut out = Vec::new();
        for z in from.z..=to.z {
            for y in from.y..=to.y {
                for x in from.x..=to.x {
                    let d = DVec3::new(x as f64, y as f64, z as f64) + DVec3::splat(0.5);
                    let s = inv * (d - centre - offset) + centre + bias;
                    if let Some(i) = index.get(s.floor().as_ivec3()) {
                        out.push((IVec3::new(x, y, z), cells[i].1));
                    }
                }
            }
        }
        Ok(out)
    }

    /// Moves blocks: positions as [`Transform::map_cells`], and block states
    /// (facing, axis, ...) turned by the nearest quarter turns, interning
    /// new states into `blocks`.
    pub fn apply(
        &self,
        cells: &[(IVec3, BlockId)],
        pivot: IVec3,
        blocks: &mut BlockTable,
        max_volume: u64,
    ) -> Result<Vec<(IVec3, BlockId)>, String> {
        let mut out = self.map_cells(cells, pivot, max_volume)?;
        let turn = self.orientation();
        if turn != AxisMap::IDENTITY {
            let mut turned: CellMap<BlockId, BlockId> = CellMap::default();
            for (_, b) in &mut out {
                *b = *turned.entry(*b).or_insert_with(|| match blocks.get(*b) {
                    Some(state) => {
                        let new = transform_state(state, &turn);
                        blocks.intern(new)
                    }
                    None => *b,
                });
            }
        }
        Ok(out)
    }
}

fn bounds<T>(cells: &[(IVec3, T)]) -> Option<(IVec3, IVec3)> {
    let first = cells.first()?.0;
    Some(cells.iter().fold((first, first), |(lo, hi), c| (lo.min(c.0), hi.max(c.0))))
}

/// Source cell lookup: a dense array over the bounds when small enough.
enum Index {
    Dense { lo: IVec3, size: IVec3, slots: Vec<u32> },
    Sparse(CellMap<IVec3, u32>),
}

impl Index {
    fn new<T>(cells: &[(IVec3, T)], lo: IVec3, hi: IVec3) -> Self {
        let size = hi - lo + IVec3::ONE;
        let volume = size.as_i64vec3().element_product();
        if volume <= DENSE_INDEX_LIMIT {
            let mut slots = vec![u32::MAX; volume as usize];
            for (i, (p, _)) in cells.iter().enumerate() {
                let r = *p - lo;
                slots[(r.x + size.x * (r.y + size.y * r.z)) as usize] = i as u32;
            }
            Index::Dense { lo, size, slots }
        } else {
            Index::Sparse(cells.iter().enumerate().map(|(i, (p, _))| (*p, i as u32)).collect())
        }
    }

    fn get(&self, p: IVec3) -> Option<usize> {
        match self {
            Index::Dense { lo, size, slots } => {
                let r = p - *lo;
                if r.cmplt(IVec3::ZERO).any() || r.cmpge(*size).any() {
                    return None;
                }
                let i = slots[(r.x + size.x * (r.y + size.y * r.z)) as usize];
                (i != u32::MAX).then_some(i as usize)
            }
            Index::Sparse(map) => map.get(&p).map(|&i| i as usize),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockState;

    fn positions(cells: &[(IVec3, ())]) -> Vec<IVec3> {
        let mut v: Vec<IVec3> = cells.iter().map(|c| c.0).collect();
        v.sort_by_key(|p| p.to_array());
        v
    }

    fn cube(lo: IVec3, hi: IVec3) -> Vec<(IVec3, ())> {
        let mut v = Vec::new();
        for z in lo.z..=hi.z {
            for y in lo.y..=hi.y {
                for x in lo.x..=hi.x {
                    v.push((IVec3::new(x, y, z), ()));
                }
            }
        }
        v
    }

    #[test]
    fn offset_moves_exactly() {
        let t = Transform { offset: IVec3::new(3, -1, 2), ..Transform::IDENTITY };
        let out = t.map_cells(&[(IVec3::ONE, ())], IVec3::ZERO, 0).unwrap();
        assert_eq!(out[0].0, IVec3::new(4, 0, 3));
    }

    #[test]
    fn quarter_turns_are_exact_and_cycle() {
        let pivot = IVec3::new(10, 0, 10);
        let start = vec![(pivot + IVec3::X, ()), (pivot + IVec3::new(2, 1, 0), ())];
        let t = Transform { rotation: [0.0, 90.0, 0.0], ..Transform::IDENTITY };
        let once = t.map_cells(&start, pivot, 0).unwrap();
        // East of the pivot turns to north of it (counter-clockwise from above).
        assert_eq!(once[0].0, pivot + IVec3::NEG_Z);
        let mut cells = start.clone();
        for _ in 0..4 {
            cells = t.map_cells(&cells, pivot, 0).unwrap();
        }
        assert_eq!(cells, start);
        // 450 degrees is a quarter turn too, still exact (volume cap 0).
        let t = Transform { rotation: [0.0, 450.0, 0.0], ..Transform::IDENTITY };
        assert_eq!(t.map_cells(&start, pivot, 0).unwrap(), once);
    }

    #[test]
    fn flips_mirror_about_the_pivot_cell() {
        let t = Transform { flip: [true, false, false], ..Transform::IDENTITY };
        let out = t.map_cells(&[(IVec3::new(7, 0, 0), ())], IVec3::new(5, 0, 0), 0).unwrap();
        assert_eq!(out[0].0, IVec3::new(3, 0, 0));
    }

    #[test]
    fn scaling_resamples_without_holes() {
        let two = Transform { scale: [2.0; 3], ..Transform::IDENTITY };
        let src = cube(IVec3::ZERO, IVec3::ONE);
        let pivot = Transform::pivot_of(IVec3::ZERO, IVec3::ONE);
        let big = two.map_cells(&src, pivot, 1000).unwrap();
        assert_eq!(big.len(), 64);
        let (lo, hi) = bounds(&big).unwrap();
        assert_eq!(hi - lo, IVec3::splat(3));

        let half = Transform { scale: [0.5; 3], ..Transform::IDENTITY };
        let src = cube(IVec3::ZERO, IVec3::splat(3));
        let small = half.map_cells(&src, Transform::pivot_of(IVec3::ZERO, IVec3::splat(3)), 1000).unwrap();
        assert_eq!(small.len(), 8);
        // Too big a result is refused.
        assert!(two.map_cells(&src, IVec3::ZERO, 10).is_err());
    }

    #[test]
    fn free_rotation_keeps_roughly_the_same_volume() {
        let src = cube(IVec3::ZERO, IVec3::new(19, 0, 19));
        let t = Transform { rotation: [0.0, 45.0, 0.0], ..Transform::IDENTITY };
        let out = t.map_cells(&src, IVec3::new(10, 0, 10), 100_000).unwrap();
        assert!((340..=460).contains(&out.len()), "{}", out.len());
        // Deterministic.
        assert_eq!(positions(&out), positions(&t.map_cells(&src, IVec3::new(10, 0, 10), 100_000).unwrap()));
    }

    #[test]
    fn dense_and_sparse_indexes_agree() {
        let cells = vec![(IVec3::ZERO, ()), (IVec3::new(3, -2, 1), ()), (IVec3::splat(5000), ())];
        let (lo, hi) = bounds(&cells).unwrap();
        let sparse = Index::new(&cells, lo, hi);
        assert!(matches!(sparse, Index::Sparse(_)));
        let dense = Index::new(&cells[..2], lo, IVec3::new(3, 0, 1));
        assert!(matches!(dense, Index::Dense { .. }));
        for (i, (p, _)) in cells[..2].iter().enumerate() {
            assert_eq!((sparse.get(*p), dense.get(*p)), (Some(i), Some(i)));
        }
        assert_eq!((sparse.get(IVec3::ONE), dense.get(IVec3::ONE)), (None, None));
        assert_eq!(dense.get(IVec3::splat(-9)), None);
        // A huge result box is refused before any work.
        let t = Transform { rotation: [0.0, 10.0, 0.0], ..Transform::IDENTITY };
        assert!(t.map_cells(&cells, IVec3::ZERO, 1000).is_err());
    }

    #[test]
    fn block_states_turn_with_the_blocks() {
        let mut table = BlockTable::default();
        let stairs = table.intern(BlockState::new("minecraft:oak_stairs").with("facing", "north"));
        let t = Transform { rotation: [0.0, 90.0, 0.0], ..Transform::IDENTITY };
        let out = t.apply(&[(IVec3::ZERO, stairs)], IVec3::ZERO, &mut table, 0).unwrap();
        let state = table.get(out[0].1).unwrap();
        assert_eq!(state.properties["facing"], "west");
    }
}
