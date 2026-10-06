use glam::{IVec3, Vec3};

use crate::block::BlockId;
use crate::world::World;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    /// The solid block that was hit.
    pub pos: IVec3,
    /// Unit axis normal of the face entered (points back toward the ray
    /// origin); `pos + normal` is the empty cell in front of it. Zero if the
    /// ray started inside a solid block.
    pub normal: IVec3,
    pub distance: f32,
}

/// Amanatides-Woo voxel traversal: first non-air block along the ray, within
/// `max_distance`. `dir` need not be normalized.
pub fn raycast(world: &World, origin: Vec3, dir: Vec3, max_distance: f32) -> Option<RayHit> {
    let dir = dir.try_normalize()?;
    let mut cell = origin.floor().as_ivec3();
    let step = IVec3::new(dir.x.signum() as i32, dir.y.signum() as i32, dir.z.signum() as i32);

    // Distance along the ray to the first boundary crossing, per axis.
    let boundary = |c: i32, s: i32| if s > 0 { (c + 1) as f32 } else { c as f32 };
    let t_max_axis = |o: f32, d: f32, c: i32, s: i32| {
        if d == 0.0 { f32::INFINITY } else { (boundary(c, s) - o) / d }
    };
    let mut t_max = Vec3::new(
        t_max_axis(origin.x, dir.x, cell.x, step.x),
        t_max_axis(origin.y, dir.y, cell.y, step.y),
        t_max_axis(origin.z, dir.z, cell.z, step.z),
    );
    let t_delta = Vec3::new(
        if dir.x == 0.0 { f32::INFINITY } else { (1.0 / dir.x).abs() },
        if dir.y == 0.0 { f32::INFINITY } else { (1.0 / dir.y).abs() },
        if dir.z == 0.0 { f32::INFINITY } else { (1.0 / dir.z).abs() },
    );

    if world.get(cell) != BlockId::AIR {
        return Some(RayHit { pos: cell, normal: IVec3::ZERO, distance: 0.0 });
    }
    loop {
        let (axis, t) = if t_max.x <= t_max.y && t_max.x <= t_max.z {
            (0, t_max.x)
        } else if t_max.y <= t_max.z {
            (1, t_max.y)
        } else {
            (2, t_max.z)
        };
        if t > max_distance {
            return None;
        }
        let mut normal = IVec3::ZERO;
        match axis {
            0 => {
                cell.x += step.x;
                t_max.x += t_delta.x;
                normal.x = -step.x;
            }
            1 => {
                cell.y += step.y;
                t_max.y += t_delta.y;
                normal.y = -step.y;
            }
            _ => {
                cell.z += step.z;
                t_max.z += t_delta.z;
                normal.z = -step.z;
            }
        }
        if world.get(cell) != BlockId::AIR {
            return Some(RayHit { pos: cell, normal, distance: t });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockState;

    fn world_with(blocks: &[IVec3]) -> World {
        let mut w = World::new();
        let s = w.blocks.intern(BlockState::new("stone"));
        for &b in blocks {
            w.set(b, s);
        }
        w
    }

    #[test]
    fn hits_first_block_and_reports_face() {
        let w = world_with(&[IVec3::new(5, 0, 0), IVec3::new(8, 0, 0)]);
        let hit = raycast(&w, Vec3::new(0.5, 0.5, 0.5), Vec3::X, 50.0).unwrap();
        assert_eq!(hit.pos, IVec3::new(5, 0, 0));
        assert_eq!(hit.normal, IVec3::NEG_X);
        assert!((hit.distance - 4.5).abs() < 1e-4);
    }

    #[test]
    fn works_in_negative_space_and_diagonally() {
        let w = world_with(&[IVec3::new(-4, -4, -4)]);
        let hit = raycast(&w, Vec3::new(0.5, 0.5, 0.5), Vec3::splat(-1.0), 50.0).unwrap();
        assert_eq!(hit.pos, IVec3::new(-4, -4, -4));
        assert_eq!(hit.normal.abs().element_sum(), 1);
    }

    #[test]
    fn misses_beyond_max_distance_and_in_empty_world() {
        let w = world_with(&[IVec3::new(20, 0, 0)]);
        assert!(raycast(&w, Vec3::new(0.5, 0.5, 0.5), Vec3::X, 10.0).is_none());
        assert!(raycast(&World::new(), Vec3::ZERO, Vec3::Y, 100.0).is_none());
        assert!(raycast(&w, Vec3::ZERO, Vec3::ZERO, 10.0).is_none());
    }

    #[test]
    fn start_inside_block_hits_immediately() {
        let w = world_with(&[IVec3::ZERO]);
        let hit = raycast(&w, Vec3::splat(0.5), Vec3::X, 5.0).unwrap();
        assert_eq!(hit.normal, IVec3::ZERO);
    }

    #[test]
    fn axis_aligned_ray_along_a_boundary_does_not_hang() {
        let w = world_with(&[IVec3::new(3, 0, 0)]);
        let hit = raycast(&w, Vec3::ZERO, Vec3::X, 10.0).unwrap();
        assert_eq!(hit.pos, IVec3::new(3, 0, 0));
        assert_eq!(hit.normal, IVec3::NEG_X);
    }
}
