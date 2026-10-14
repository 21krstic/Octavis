//! Heightfield terrain: fractal noise decides each column's surface height,
//! and three patterns say what the surface, the soil beneath it and the
//! deeper fill are made of.

use glam::IVec3;
use octavis_core::{BlockId, Pattern, World};

use crate::noise::Perlin2;

/// Most cells a single generation may cover; keeps the undo entry and the
/// change list a sane size.
pub const MAX_CELLS: i64 = 4_000_000;

#[derive(Clone, Debug)]
pub struct Terrain {
    pub seed: u32,
    /// Feature size in blocks: larger means broader hills.
    pub scale: f32,
    pub octaves: u32,
    /// Weight of each finer octave relative to the previous one.
    pub persistence: f32,
    /// Average surface height (y of the topmost block).
    pub base: i32,
    /// How far the surface may rise or fall from `base`.
    pub amplitude: f32,
    /// Thickness of the soil layer under the surface block.
    pub soil_depth: i32,
    pub surface: Pattern,
    pub soil: Pattern,
    pub fill: Pattern,
}

impl Terrain {
    /// Surface height (y of the topmost solid block) of the column at x, z.
    pub fn height(&self, noise: &Perlin2, x: i32, z: i32) -> i32 {
        let s = self.scale.max(0.01) as f64;
        let n = noise.fbm((x as f64 + 0.5) / s, (z as f64 + 0.5) / s, self.octaves, self.persistence as f64);
        self.base + (n * self.amplitude as f64).round() as i32
    }

    /// Block wanted at `y` in a column whose surface is at `h`.
    fn block_at(&self, p: IVec3, h: i32) -> BlockId {
        if p.y > h {
            BlockId::AIR
        } else if p.y == h {
            self.surface.pick(p, self.seed)
        } else if p.y > h - 1 - self.soil_depth {
            self.soil.pick(p, self.seed)
        } else {
            self.fill.pick(p, self.seed)
        }
    }

    /// Changes needed to carve this terrain into the inclusive box
    /// `min..=max`: every column is rewritten over the box's full height
    /// (air above the surface, so regenerating replaces old terrain). Only
    /// cells that actually change are returned. Cells for which `include`
    /// is false are left alone (heights stay absolute, so a masked region
    /// matches the same area generated unmasked).
    pub fn plan(
        &self,
        world: &World,
        min: IVec3,
        max: IVec3,
        include: &dyn Fn(IVec3) -> bool,
    ) -> Result<Vec<(IVec3, BlockId)>, String> {
        let size = max - min + IVec3::ONE;
        if size.min_element() < 1 {
            return Err("empty area".into());
        }
        let cells = size.x as i64 * size.y as i64 * size.z as i64;
        if cells > MAX_CELLS {
            return Err(format!("area has {cells} blocks; the limit is {MAX_CELLS}"));
        }
        let noise = Perlin2::new(self.seed);
        let mut out = Vec::new();
        for z in min.z..=max.z {
            for x in min.x..=max.x {
                let h = self.height(&noise, x, z);
                for y in min.y..=max.y {
                    let p = IVec3::new(x, y, z);
                    let want = self.block_at(p, h);
                    if include(p) && world.get(p) != want {
                        out.push((p, want));
                    }
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octavis_core::BlockState;

    fn terrain(world: &mut World) -> Terrain {
        let mut id = |n: &str| world.blocks.intern(BlockState::new(format!("minecraft:{n}")));
        Terrain {
            seed: 11,
            scale: 24.0,
            octaves: 4,
            persistence: 0.5,
            base: 10,
            amplitude: 8.0,
            soil_depth: 3,
            surface: Pattern::Single(id("grass_block")),
            soil: Pattern::Single(id("dirt")),
            fill: Pattern::Single(id("stone")),
        }
    }

    fn apply(world: &mut World, plan: &[(IVec3, BlockId)]) {
        for &(p, b) in plan {
            world.set(p, b);
        }
    }

    #[test]
    fn columns_are_layered_and_heights_stay_in_range() {
        let mut w = World::new();
        let t = terrain(&mut w);
        let (min, max) = (IVec3::new(-20, -5, -20), IVec3::new(20, 30, 20));
        let plan = t.plan(&w, min, max, &|_| true).unwrap();
        apply(&mut w, &plan);
        let [grass, dirt, stone] = ["grass_block", "dirt", "stone"]
            .map(|n| w.blocks.intern(BlockState::new(format!("minecraft:{n}"))));
        let noise = Perlin2::new(t.seed);
        let mut heights = std::collections::BTreeSet::new();
        for x in -20..=20 {
            for z in -20..=20 {
                let h = t.height(&noise, x, z);
                assert!((2..=18).contains(&h), "height {h} outside base±amplitude");
                heights.insert(h);
                assert_eq!(w.get(IVec3::new(x, h, z)), grass);
                assert_eq!(w.get(IVec3::new(x, h + 1, z)), BlockId::AIR);
                assert_eq!(w.get(IVec3::new(x, h - 1, z)), dirt);
                assert_eq!(w.get(IVec3::new(x, h - 3, z)), dirt);
                assert_eq!(w.get(IVec3::new(x, h - 4, z)), stone);
                assert_eq!(w.get(IVec3::new(x, min.y, z)), stone);
            }
        }
        assert!(heights.len() > 4, "terrain should not be flat");
    }

    #[test]
    fn same_seed_same_terrain_and_regeneration_changes_nothing() {
        let mut w = World::new();
        let t = terrain(&mut w);
        let (min, max) = (IVec3::new(0, 0, 0), IVec3::new(15, 25, 15));
        let first = t.plan(&w, min, max, &|_| true).unwrap();
        assert_eq!(first, t.plan(&w, min, max, &|_| true).unwrap());
        apply(&mut w, &first);
        assert!(t.plan(&w, min, max, &|_| true).unwrap().is_empty());
        let other = Terrain { seed: 12, ..t.clone() };
        assert!(!other.plan(&w, min, max, &|_| true).unwrap().is_empty());
    }

    #[test]
    fn regenerating_clears_the_old_terrain_and_ignores_cells_outside() {
        let mut w = World::new();
        let t = terrain(&mut w);
        let (min, max) = (IVec3::new(0, 0, 0), IVec3::new(7, 25, 7));
        let first = t.plan(&w, min, max, &|_| true).unwrap();
        apply(&mut w, &first);
        // A tall column left over from earlier edits, and a block outside the box.
        let junk = w.blocks.intern(BlockState::new("minecraft:glass"));
        w.set(IVec3::new(3, 24, 3), junk);
        w.set(IVec3::new(3, 40, 3), junk);
        let flat = Terrain { amplitude: 0.0, ..t };
        let second = flat.plan(&w, min, max, &|_| true).unwrap();
        apply(&mut w, &second);
        assert_eq!(w.get(IVec3::new(3, 24, 3)), BlockId::AIR);
        assert_eq!(w.get(IVec3::new(3, 40, 3)), junk);
    }

    #[test]
    fn weighted_patterns_and_limits() {
        let mut w = World::new();
        let mut t = terrain(&mut w);
        let moss = w.blocks.intern(BlockState::new("minecraft:moss_block"));
        t.surface = Pattern::Weighted(vec![(t_block(&w, "grass_block"), 1.0), (moss, 1.0)]);
        let plan = t.plan(&w, IVec3::ZERO, IVec3::new(31, 20, 31), &|_| true).unwrap();
        let tops = plan.iter().filter(|&&(p, b)| b == moss && p.y > 0).count();
        assert!(tops > 100, "expected a mix on the surface, got {tops}");
        assert!(t.plan(&w, IVec3::ZERO, IVec3::new(5000, 5000, 5000), &|_| true).is_err());
        assert!(t.plan(&w, IVec3::ONE, IVec3::ZERO, &|_| true).is_err());
    }

    #[test]
    fn include_mask_limits_changes_to_chosen_cells() {
        let mut w = World::new();
        let t = terrain(&mut w);
        let (min, max) = (IVec3::new(0, 0, 0), IVec3::new(15, 25, 15));
        // An L-shaped region: its bounding box is the whole 16x16 area.
        let in_l = |p: IVec3| p.x < 4 || p.z < 4;
        let plan = t.plan(&w, min, max, &in_l).unwrap();
        assert!(!plan.is_empty() && plan.iter().all(|&(p, _)| in_l(p)));
        // Heights are absolute, so masked terrain matches unmasked terrain.
        let full = t.plan(&w, min, max, &|_| true).unwrap();
        let expected: Vec<_> = full.into_iter().filter(|&(p, _)| in_l(p)).collect();
        assert_eq!(plan, expected);
    }

    fn t_block(w: &World, n: &str) -> BlockId {
        (1..w.blocks.len() as u32)
            .map(BlockId)
            .find(|&b| w.blocks.get(b).is_some_and(|s| s.name == format!("minecraft:{n}")))
            .unwrap()
    }
}
