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

/// Cells on the straight line from `a` to `b`, both included, with no gaps
/// (consecutive cells differ by at most 1 on each axis). Used to bridge the
/// distance the cursor travels between two frames.
pub fn line_cells(a: IVec3, b: IVec3) -> Vec<IVec3> {
    let d = b - a;
    let n = d.abs().max_element();
    if n == 0 {
        return vec![a];
    }
    (0..=n)
        .map(|i| {
            if i == n {
                b
            } else {
                (a.as_vec3() + d.as_vec3() * (i as f32 / n as f32)).round().as_ivec3()
            }
        })
        .collect()
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
    fn line_has_endpoints_and_no_gaps() {
        for (a, b) in [
            (IVec3::new(0, 0, 0), IVec3::new(7, 2, -3)),
            (IVec3::new(-4, 5, 1), IVec3::new(-4, 5, 1)),
            (IVec3::new(3, 3, 3), IVec3::new(-6, -1, 8)),
        ] {
            let line = line_cells(a, b);
            assert_eq!(line[0], a);
            assert_eq!(*line.last().unwrap(), b);
            assert_eq!(line.len() as i32, (b - a).abs().max_element() + 1);
            for w in line.windows(2) {
                assert!((w[1] - w[0]).abs().max_element() <= 1, "gap in {line:?}");
            }
        }
    }

    #[test]
    fn plane_point_basic() {
        let p = ray_plane_point(Vec3::new(0.0, 0.0, 10.0), -Vec3::Z, Vec3::new(1.0, 2.0, 0.0), Vec3::Z, 100.0)
            .unwrap();
        assert!((p - Vec3::new(0.0, 0.0, 0.0)).length() < 1e-5);
        assert!(ray_plane_point(Vec3::ZERO, Vec3::X, Vec3::Z, Vec3::Z, 10.0).is_none());
    }
}
