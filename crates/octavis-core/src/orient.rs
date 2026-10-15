//! How block states turn when blocks are rotated or mirrored: `facing`,
//! `axis`, sign/banner `rotation`, connection sides (fences, walls, panes),
//! stair and door handedness, top/bottom halves and rail shapes.
//!
//! These are Minecraft's block-state property names (stable since 1.13).
//! Until a block registry exists this works from the property names alone,
//! so a turn a block cannot take (stairs facing up) leaves that property as
//! it was.

use std::f64::consts::PI;

use glam::{DMat3, DVec3, IVec3};

use crate::block::BlockState;

/// A rotation or mirror that maps every axis onto an axis (a signed
/// permutation): what 90-degree turns and flips do to directions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AxisMap {
    /// Where +X, +Y and +Z end up.
    cols: [IVec3; 3],
}

impl AxisMap {
    pub const IDENTITY: AxisMap = AxisMap { cols: [IVec3::X, IVec3::Y, IVec3::Z] };

    /// `m` itself, if it is a signed permutation (up to rounding error).
    pub fn exact(m: DMat3) -> Option<AxisMap> {
        let mut cols = [IVec3::ZERO; 3];
        let mut used = [false; 3];
        for (j, col) in cols.iter_mut().enumerate() {
            let c = m.col(j);
            let r = c.round();
            if (c - r).abs().max_element() > 1e-9 || r.abs().element_sum() != 1.0 {
                return None;
            }
            let axis = (0..3).find(|&i| r[i] != 0.0)?;
            if std::mem::replace(&mut used[axis], true) {
                return None;
            }
            *col = r.as_ivec3();
        }
        Some(AxisMap { cols })
    }

    /// The signed permutation closest to `m` (a rotation, possibly mirrored
    /// or scaled). Block states turn by this for arbitrary angles.
    pub fn nearest(m: DMat3) -> AxisMap {
        let mut entries: Vec<(f64, usize, usize)> =
            (0..3).flat_map(|j| (0..3).map(move |i| (m.col(j)[i].abs(), i, j))).collect();
        entries.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut cols = [IVec3::ZERO; 3];
        let (mut row_used, mut col_used) = ([false; 3], [false; 3]);
        for (_, i, j) in entries {
            if row_used[i] || col_used[j] {
                continue;
            }
            row_used[i] = true;
            col_used[j] = true;
            cols[j][i] = if m.col(j)[i] < 0.0 { -1 } else { 1 };
        }
        AxisMap { cols }
    }

    pub fn apply(&self, v: IVec3) -> IVec3 {
        self.cols[0] * v.x + self.cols[1] * v.y + self.cols[2] * v.z
    }

    fn apply_f(&self, v: DVec3) -> DVec3 {
        self.cols[0].as_dvec3() * v.x + self.cols[1].as_dvec3() * v.y + self.cols[2].as_dvec3() * v.z
    }
}

const DIRS: [(&str, IVec3); 6] = [
    ("north", IVec3::NEG_Z),
    ("south", IVec3::Z),
    ("east", IVec3::X),
    ("west", IVec3::NEG_X),
    ("up", IVec3::Y),
    ("down", IVec3::NEG_Y),
];

const RAIL_SHAPES: [&str; 6] = ["north_south", "east_west", "south_east", "south_west", "north_west", "north_east"];

fn dir_vec(name: &str) -> Option<IVec3> {
    DIRS.iter().find(|d| d.0 == name).map(|d| d.1)
}

fn dir_name(v: IVec3) -> Option<&'static str> {
    DIRS.iter().find(|d| d.1 == v).map(|d| d.0)
}

/// Swaps a property between the two values of any listed pair.
fn swap_values(state: &mut BlockState, key: &str, pairs: &[(&str, &str)]) {
    let Some(v) = state.properties.get_mut(key) else { return };
    for (a, b) in pairs {
        if v == a {
            *v = b.to_string();
            return;
        }
        if v == b {
            *v = a.to_string();
            return;
        }
    }
}

/// A rail shape after a horizontal turn or mirror.
fn rail_shape(shape: &str, m: &AxisMap) -> Option<String> {
    let turn = |d: &str| {
        let w = m.apply(dir_vec(d)?);
        (w.y == 0).then(|| dir_name(w)).flatten()
    };
    if let Some(d) = shape.strip_prefix("ascending_") {
        return Some(format!("ascending_{}", turn(d)?));
    }
    let (a, b) = shape.split_once('_')?;
    let (a, b) = (turn(a)?, turn(b)?);
    RAIL_SHAPES.iter().find(|s| **s == format!("{a}_{b}") || **s == format!("{b}_{a}")).map(|s| s.to_string())
}

/// `state` after its block is turned by `m`.
pub fn transform_state(state: &BlockState, m: &AxisMap) -> BlockState {
    if *m == AxisMap::IDENTITY || state.properties.is_empty() {
        return state.clone();
    }
    let props = &state.properties;
    let mut out = state.clone();

    // A horizontal facing that would point up or down is left alone: most
    // such blocks (stairs, doors) cannot face vertically.
    if let Some(v) = props.get("facing").and_then(|f| dir_vec(f)) {
        let w = m.apply(v);
        if v.y != 0 || w.y == 0 {
            out.properties.insert("facing".into(), dir_name(w).unwrap_or("north").into());
        }
    }
    if let Some(i) = props.get("axis").and_then(|a| ["x", "y", "z"].iter().position(|n| n == a)) {
        let mut u = IVec3::ZERO;
        u[i] = 1;
        let w = m.apply(u).abs();
        if let Some(j) = (0..3).find(|&k| w[k] == 1) {
            out.properties.insert("axis".into(), ["x", "y", "z"][j].into());
        }
    }
    // Connection sides move with the block, if every side they move to
    // exists on it (a fence has no up/down sides to turn onto).
    let present: Vec<(&str, IVec3)> = DIRS.iter().copied().filter(|(n, _)| props.contains_key(*n)).collect();
    let moved: Option<Vec<&str>> = present.iter().map(|(_, v)| dir_name(m.apply(*v))).collect();
    if let Some(moved) = moved.filter(|ns| ns.iter().all(|n| props.contains_key(*n))) {
        for (n, _) in &present {
            out.properties.remove(*n);
        }
        for ((n, _), to) in present.iter().zip(moved) {
            out.properties.insert(to.into(), props[*n].clone());
        }
    }

    // The rest only makes sense while up stays vertical.
    let y = m.apply(IVec3::Y);
    if y.x != 0 || y.z != 0 {
        return out;
    }
    if y.y < 0 {
        swap_values(&mut out, "half", &[("top", "bottom"), ("upper", "lower")]);
        swap_values(&mut out, "type", &[("top", "bottom")]);
        swap_values(&mut out, "face", &[("floor", "ceiling")]);
        swap_values(&mut out, "attachment", &[("floor", "ceiling")]);
        swap_values(&mut out, "vertical_direction", &[("up", "down")]);
    }
    let (ex, ez) = (m.apply(IVec3::X), m.apply(IVec3::Z));
    let mirrored = ex.x * ez.z - ex.z * ez.x < 0;
    if let Some(r) = props.get("rotation").and_then(|r| r.parse::<i32>().ok()) {
        // 16 steps: 0 = south, 4 = west, 8 = north, 12 = east.
        let a = r as f64 * PI / 8.0;
        let w = m.apply_f(DVec3::new(-a.sin(), 0.0, a.cos()));
        let r = ((-w.x).atan2(w.z) / (PI / 8.0)).round() as i32;
        out.properties.insert("rotation".into(), r.rem_euclid(16).to_string());
    }
    if let Some(shape) = props.get("shape") {
        if let Some(s) = rail_shape(shape, m) {
            out.properties.insert("shape".into(), s);
        } else if mirrored {
            let s = if shape.contains("left") {
                shape.replace("left", "right")
            } else {
                shape.replace("right", "left")
            };
            out.properties.insert("shape".into(), s);
        }
    }
    if mirrored {
        swap_values(&mut out, "hinge", &[("left", "right")]);
        swap_values(&mut out, "type", &[("left", "right")]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A quarter turn about Y, counter-clockwise seen from above
    /// (east -> north).
    fn turn_y() -> AxisMap {
        AxisMap { cols: [IVec3::NEG_Z, IVec3::Y, IVec3::X] }
    }

    fn mirror_x() -> AxisMap {
        AxisMap { cols: [IVec3::NEG_X, IVec3::Y, IVec3::Z] }
    }

    fn flip_y() -> AxisMap {
        AxisMap { cols: [IVec3::X, IVec3::NEG_Y, IVec3::Z] }
    }

    fn prop(s: &BlockState, k: &str) -> String {
        s.properties[k].clone()
    }

    #[test]
    fn exact_and_nearest_axis_maps() {
        assert_eq!(AxisMap::exact(DMat3::IDENTITY), Some(AxisMap::IDENTITY));
        assert!(AxisMap::exact(DMat3::from_rotation_y(0.3)).is_none());
        assert!(AxisMap::exact(DMat3::from_diagonal(DVec3::new(2.0, 1.0, 1.0))).is_none());
        // 80 degrees is nearest to a quarter turn.
        let near = AxisMap::nearest(DMat3::from_rotation_y(80f64.to_radians()));
        assert_eq!(near, turn_y());
    }

    #[test]
    fn stairs_turn_and_mirror() {
        let s = BlockState::new("minecraft:oak_stairs")
            .with("facing", "north")
            .with("half", "bottom")
            .with("shape", "inner_left");
        let t = transform_state(&s, &turn_y());
        assert_eq!((prop(&t, "facing"), prop(&t, "shape")), ("west".into(), "inner_left".into()));
        let m = transform_state(&s, &mirror_x());
        assert_eq!((prop(&m, "facing"), prop(&m, "shape")), ("north".into(), "inner_right".into()));
        let f = transform_state(&s, &flip_y());
        assert_eq!((prop(&f, "half"), prop(&f, "shape")), ("top".into(), "inner_left".into()));
        // Four turns are the identity.
        let mut r = s.clone();
        for _ in 0..4 {
            r = transform_state(&r, &turn_y());
        }
        assert_eq!(r, s);
    }

    #[test]
    fn axis_sides_rotation_and_rails() {
        let log = BlockState::new("log").with("axis", "x");
        assert_eq!(prop(&transform_state(&log, &turn_y()), "axis"), "z");
        let fence = BlockState::new("fence").with("north", "true").with("east", "false").with("south", "false").with("west", "false");
        let t = transform_state(&fence, &turn_y());
        assert_eq!((prop(&t, "west"), prop(&t, "north")), ("true".into(), "false".into()));
        // Sides that would turn onto missing up/down keys stay put.
        let roll = AxisMap { cols: [IVec3::X, IVec3::Z, IVec3::NEG_Y] };
        assert_eq!(transform_state(&fence, &roll), fence);
        let sign = BlockState::new("sign").with("rotation", "4"); // west
        assert_eq!(prop(&transform_state(&sign, &mirror_x()), "rotation"), "12");
        assert_eq!(prop(&transform_state(&sign, &turn_y()), "rotation"), "0"); // west -> south
        let rail = BlockState::new("rail").with("shape", "north_east");
        assert_eq!(prop(&transform_state(&rail, &turn_y()), "shape"), "north_west");
        let up = BlockState::new("rail").with("shape", "ascending_east");
        assert_eq!(prop(&transform_state(&up, &mirror_x()), "shape"), "ascending_west");
    }

    #[test]
    fn vertical_facing_and_halves() {
        let piston = BlockState::new("piston").with("facing", "up");
        assert_eq!(prop(&transform_state(&piston, &flip_y()), "facing"), "down");
        let door = BlockState::new("door").with("half", "upper").with("hinge", "left").with("facing", "east");
        let m = transform_state(&door, &mirror_x());
        assert_eq!((prop(&m, "hinge"), prop(&m, "facing")), ("right".into(), "west".into()));
        assert_eq!(prop(&transform_state(&door, &flip_y()), "half"), "lower");
        // Stairs cannot face up: a roll leaves facing alone.
        let stairs = BlockState::new("stairs").with("facing", "north");
        let roll = AxisMap { cols: [IVec3::X, IVec3::Z, IVec3::NEG_Y] };
        assert_eq!(prop(&transform_state(&stairs, &roll), "facing"), "north");
    }
}
