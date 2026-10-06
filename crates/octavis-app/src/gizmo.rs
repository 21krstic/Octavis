//! 2D overlays that make orientation readable: world axes through the
//! origin (so negative space is identifiable) and a corner axis gizmo.
//! Drawn with egui's painter over the viewport, ignoring depth.

use eframe::egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, Vec2 as EVec2};
use glam::{Mat3, Mat4, Vec3, Vec4};

const AXES: [(Vec3, Color32, &str); 3] = [
    (Vec3::X, Color32::from_rgb(230, 70, 70), "X"),
    (Vec3::Y, Color32::from_rgb(90, 200, 90), "Y"),
    (Vec3::Z, Color32::from_rgb(80, 130, 240), "Z"),
];
const AXIS_LENGTH: f32 = 256.0;

/// Clips a world-space segment against the near plane and returns its
/// clip-space endpoints, or `None` if it is entirely behind the camera.
pub fn clip_segment(view_proj: &Mat4, a: Vec3, b: Vec3) -> Option<(Vec4, Vec4)> {
    let (mut ca, mut cb) = (*view_proj * a.extend(1.0), *view_proj * b.extend(1.0));
    // wgpu clip space: visible depth is z >= 0.
    match (ca.z >= 0.0, cb.z >= 0.0) {
        (false, false) => return None,
        (true, true) => {}
        (a_in, _) => {
            let t = ca.z / (ca.z - cb.z);
            let cut = ca + (cb - ca) * t;
            if a_in {
                cb = cut;
            } else {
                ca = cut;
            }
        }
    }
    Some((ca, cb))
}

fn to_screen(rect: Rect, c: Vec4) -> Pos2 {
    let ndc_x = c.x / c.w;
    let ndc_y = c.y / c.w;
    Pos2::new(
        rect.min.x + (ndc_x * 0.5 + 0.5) * rect.width(),
        rect.min.y + (0.5 - ndc_y * 0.5) * rect.height(),
    )
}

/// Lines through the world origin: bright for the positive half of each
/// axis, dim for the negative half.
pub fn draw_world_axes(painter: &Painter, rect: Rect, view_proj: &Mat4) {
    for (dir, color, _) in AXES {
        for (from, to, alpha) in [(Vec3::ZERO, dir * AXIS_LENGTH, 200u8), (-dir * AXIS_LENGTH, Vec3::ZERO, 70u8)] {
            if let Some((a, b)) = clip_segment(view_proj, from, to) {
                let c = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha);
                painter.line_segment([to_screen(rect, a), to_screen(rect, b)], Stroke::new(1.5, c));
            }
        }
    }
}

/// Small orientation widget in the viewport's bottom-left corner.
pub fn draw_corner_gizmo(painter: &Painter, rect: Rect, view: &Mat4) {
    let center = Pos2::new(rect.min.x + 70.0, rect.max.y - 70.0);
    let rot = Mat3::from_mat4(*view);
    let reach = 42.0;

    // Draw far axes first so near ones overlap them.
    let mut order: Vec<usize> = (0..3).collect();
    order.sort_by(|&i, &j| (rot * AXES[i].0).z.total_cmp(&(rot * AXES[j].0).z));

    for i in order {
        let (axis, color, label) = AXES[i];
        let v = rot * axis;
        let tip = center + EVec2::new(v.x, -v.y) * reach;
        let back = center - EVec2::new(v.x, -v.y) * reach;
        painter.circle_filled(back, 5.0, Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 90));
        painter.line_segment([center, tip], Stroke::new(2.0, color));
        painter.circle_filled(tip, 9.0, color);
        painter.text(tip, Align2::CENTER_CENTER, label, FontId::proportional(12.0), Color32::BLACK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp() -> Mat4 {
        // Camera at z=10 looking at the origin.
        let view = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO, Vec3::Y);
        Mat4::perspective_rh(1.0, 1.5, 0.1, 1000.0) * view
    }

    #[test]
    fn fully_visible_segment_is_unchanged() {
        let (a, b) = clip_segment(&vp(), Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)).unwrap();
        assert!(a.z >= 0.0 && b.z >= 0.0);
        assert!(a.x < b.x);
    }

    #[test]
    fn segment_behind_camera_is_dropped() {
        assert!(clip_segment(&vp(), Vec3::new(0.0, 0.0, 20.0), Vec3::new(1.0, 0.0, 30.0)).is_none());
    }

    #[test]
    fn segment_crossing_the_camera_plane_is_clipped_to_front() {
        // Runs from in front of the camera to far behind it.
        let (a, b) = clip_segment(&vp(), Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 50.0)).unwrap();
        assert!(a.z >= -1e-4 && b.z >= -1e-4);
        assert!(a.w > 0.0 && b.w > 0.0);
        // Same result regardless of endpoint order.
        let (c, d) = clip_segment(&vp(), Vec3::new(0.0, 0.0, 50.0), Vec3::new(0.0, 0.0, 0.0)).unwrap();
        assert!((c - b).length() < 1e-3 && (d - a).length() < 1e-3);
    }
}
