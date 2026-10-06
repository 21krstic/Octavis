//! Ray helpers for tools that need a position in space rather than a hit
//! on an existing block.

use glam::{IVec3, Vec3};

/// Cell where a ray crosses the mid-plane of a one-cell-thick layer
/// (`layer` along `axis`). Used to lock a pencil stroke to one plane so
/// strokes paint flat and a tap places exactly one block.
pub fn ray_layer_cell(
    origin: Vec3,
    dir: Vec3,
    axis: usize,
    layer: i32,
    max_distance: f32,
) -> Option<IVec3> {
    let d = dir[axis];
    if d.abs() < 1e-6 {
        return None;
    }
    let t = (layer as f32 + 0.5 - origin[axis]) / d;
    if !(0.0..=max_distance).contains(&t) {
        return None;
    }
    let mut cell = (origin + dir * t).floor().as_ivec3();
    cell[axis] = layer;
    Some(cell)
}

/// Point where a ray crosses the plane through `point` with `normal`.
pub fn ray_plane_point(
    origin: Vec3,
    dir: Vec3,
    point: Vec3,
    normal: Vec3,
    max_distance: f32,
) -> Option<Vec3> {
    let denom = dir.dot(normal);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (point - origin).dot(normal) / denom;
    (0.0..=max_distance).contains(&t).then(|| origin + dir * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_cell_is_stable_for_a_still_cursor() {
        // Looking down at the floor from above: layer y=0 is hit at one cell.
        let o = Vec3::new(2.3, 10.0, 4.7);
        let d = Vec3::new(0.0, -1.0, 0.0);
        let c = ray_layer_cell(o, d, 1, 0, 100.0).unwrap();
        assert_eq!(c, IVec3::new(2, 0, 4));
    }

    #[test]
    fn layer_cell_works_in_negative_space_and_obliquely() {
        let o = Vec3::new(-5.5, 8.0, -3.2);
        let d = Vec3::new(0.3, -1.0, 0.1).normalize();
        let c = ray_layer_cell(o, d, 1, 2, 100.0).unwrap();
        assert_eq!(c.y, 2);
        let t = (2.5 - 8.0) / d.y;
        let p = o + d * t;
        assert_eq!((c.x, c.z), (p.x.floor() as i32, p.z.floor() as i32));
    }

    #[test]
    fn layer_cell_rejects_parallel_behind_and_far() {
        let o = Vec3::new(0.5, 5.0, 0.5);
        assert!(ray_layer_cell(o, Vec3::X, 1, 0, 100.0).is_none()); // parallel
        assert!(ray_layer_cell(o, Vec3::Y, 1, 0, 100.0).is_none()); // plane behind
        assert!(ray_layer_cell(o, -Vec3::Y, 1, 0, 2.0).is_none()); // too far
    }

    #[test]
    fn plane_point_basic() {
        let p = ray_plane_point(Vec3::new(0.0, 0.0, 10.0), -Vec3::Z, Vec3::new(1.0, 2.0, 0.0), Vec3::Z, 100.0)
            .unwrap();
        assert!((p - Vec3::new(0.0, 0.0, 0.0)).length() < 1e-5);
        assert!(ray_plane_point(Vec3::ZERO, Vec3::X, Vec3::Z, Vec3::Z, 10.0).is_none());
    }
}
