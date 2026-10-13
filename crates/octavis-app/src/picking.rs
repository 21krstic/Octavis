//! Ray helpers for tools that need a position in space rather than a hit
//! on an existing block.

use glam::{IVec3, Vec2, Vec3};

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

/// Deepest bisection of one screen segment (2^9 samples at most).
const MAX_TRACE_DEPTH: u32 = 9;

/// Targets along the straight screen-space path from `p0` (whose target is
/// `t0`) to `p1` (target `t1`), excluding `p0`'s, appended to `out`.
///
/// The path is bisected until neighbouring targets are within `max_gap`
/// cells on every axis, so a fast cursor leaves no gaps and every sample
/// lands on a surface actually under the cursor (a line drawn between two
/// 3D cells could cut through solid volumes). The result depends only on
/// the path, not on how often the cursor was sampled. Bisection also ends
/// at pixel size, at depth jumps (silhouette edges) and where there is no
/// target (sky).
pub fn trace_targets(
    sample: &dyn Fn(Vec2) -> Option<IVec3>,
    p0: Vec2,
    t0: Option<IVec3>,
    p1: Vec2,
    t1: Option<IVec3>,
    max_gap: i32,
    out: &mut Vec<IVec3>,
) {
    bisect(sample, p0, t0, p1, t1, max_gap, 0, out);
}

#[allow(clippy::too_many_arguments)]
fn bisect(
    sample: &dyn Fn(Vec2) -> Option<IVec3>,
    p0: Vec2,
    t0: Option<IVec3>,
    p1: Vec2,
    t1: Option<IVec3>,
    max_gap: i32,
    depth: u32,
    out: &mut Vec<IVec3>,
) {
    let close = matches!((t0, t1), (Some(a), Some(b)) if (a - b).abs().max_element() <= max_gap);
    if close || depth >= MAX_TRACE_DEPTH || t0.is_none() || t1.is_none() || p0.distance(p1) < 1.0 {
        out.extend(t1);
        return;
    }
    let pm = (p0 + p1) * 0.5;
    let tm = sample(pm);
    bisect(sample, p0, t0, pm, tm, max_gap, depth + 1, out);
    bisect(sample, pm, tm, p1, t1, max_gap, depth + 1, out);
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

    /// A flat floor seen from above: 3 pixels per cell along x.
    fn floor(p: Vec2) -> Option<IVec3> {
        Some(IVec3::new((p.x / 3.0).floor() as i32, 0, 0))
    }

    #[test]
    fn trace_leaves_no_gaps_however_fast_the_cursor_moves() {
        for gap in [1, 4] {
            let (p0, p1) = (Vec2::new(0.0, 5.0), Vec2::new(900.0, 5.0));
            let mut out = Vec::new();
            trace_targets(&floor, p0, floor(p0), p1, floor(p1), gap, &mut out);
            assert_eq!(*out.last().unwrap(), floor(p1).unwrap());
            let mut prev = floor(p0).unwrap();
            for &c in &out {
                assert!((c - prev).abs().max_element() <= gap, "gap {prev} -> {c}");
                prev = c;
            }
        }
    }

    #[test]
    fn trace_does_not_depend_on_sampling_rate() {
        // Many short hops and one long hop visit the same cells.
        let cells = |hops: &[f32]| {
            let mut out = Vec::new();
            for w in hops.windows(2) {
                let (a, b) = (Vec2::new(w[0], 0.0), Vec2::new(w[1], 0.0));
                trace_targets(&floor, a, floor(a), b, floor(b), 1, &mut out);
            }
            out.sort_by_key(|c| c.to_array());
            out.dedup();
            out
        };
        let fine: Vec<f32> = (0..=100).map(|i| i as f32 * 9.0).collect();
        assert_eq!(cells(&[0.0, 900.0]), cells(&fine));
    }

    #[test]
    fn trace_terminates_at_depth_jumps_and_sky() {
        // A cliff: cells jump from x=0 to x=1000 at pixel 50.
        let cliff = |p: Vec2| Some(IVec3::new(if p.x < 50.0 { 0 } else { 1000 }, 0, 0));
        let mut out = Vec::new();
        let (p0, p1) = (Vec2::ZERO, Vec2::new(100.0, 0.0));
        trace_targets(&cliff, p0, cliff(p0), p1, cliff(p1), 1, &mut out);
        assert!(out.len() < 1000 && out.contains(&IVec3::new(1000, 0, 0)));
        let sky = |_: Vec2| None;
        let mut none = Vec::new();
        trace_targets(&sky, p0, None, p1, None, 1, &mut none);
        assert!(none.is_empty());
    }

    #[test]
    fn plane_point_basic() {
        let p = ray_plane_point(Vec3::new(0.0, 0.0, 10.0), -Vec3::Z, Vec3::new(1.0, 2.0, 0.0), Vec3::Z, 100.0)
            .unwrap();
        assert!((p - Vec3::new(0.0, 0.0, 0.0)).length() < 1e-5);
        assert!(ray_plane_point(Vec3::ZERO, Vec3::X, Vec3::Z, Vec3::Z, 10.0).is_none());
    }
}
