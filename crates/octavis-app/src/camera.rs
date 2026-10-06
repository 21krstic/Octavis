use glam::{Mat4, Vec2, Vec3, Vec4};

/// Orbit camera around a target point (Blender-style navigation).
pub struct Camera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl Camera {
    pub fn new(target: Vec3) -> Self {
        Self { target, yaw: 0.8, pitch: 0.55, distance: 40.0 }
    }

    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + Vec3::new(cy * cp, sp, sy * cp) * self.distance
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let view = Mat4::look_at_rh(self.eye(), self.target, Vec3::Y);
        let proj = Mat4::perspective_rh(60f32.to_radians(), aspect.max(0.01), 0.1, 2000.0);
        proj * view
    }

    /// World-space ray through a point in the viewport. `pos` is in points
    /// relative to the viewport's top-left, `size` is the viewport size.
    pub fn ray(&self, pos: Vec2, size: Vec2) -> (Vec3, Vec3) {
        let ndc = Vec2::new(pos.x / size.x * 2.0 - 1.0, 1.0 - pos.y / size.y * 2.0);
        let inv = self.view_proj(size.x / size.y.max(1.0)).inverse();
        // Depth range is 0..1, so the far plane is z = 1.
        let far = inv * Vec4::new(ndc.x, ndc.y, 1.0, 1.0);
        let far = far.truncate() / far.w;
        let eye = self.eye();
        (eye, (far - eye).normalize())
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw += dx * 0.01;
        self.pitch = (self.pitch + dy * 0.01).clamp(-1.55, 1.55);
    }

    /// Pans in the view plane; speed scales with distance.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let forward = (self.target - self.eye()).normalize();
        let right = forward.cross(Vec3::Y).normalize();
        let up = right.cross(forward);
        let k = self.distance * 0.0015;
        self.target += (-right * dx + up * dy) * k;
    }

    pub fn zoom(&mut self, scroll: f32) {
        self.distance = (self.distance * (-scroll * 0.002).exp()).clamp(1.0, 1500.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centre_ray_points_at_target() {
        let cam = Camera::new(Vec3::new(3.0, 4.0, 5.0));
        let (origin, dir) = cam.ray(Vec2::new(400.0, 300.0), Vec2::new(800.0, 600.0));
        let to_target = (cam.target - origin).normalize();
        assert!(dir.dot(to_target) > 0.9999, "{dir} vs {to_target}");
    }

    #[test]
    fn right_side_of_screen_is_right_of_view() {
        let cam = Camera::new(Vec3::ZERO);
        let (_, left) = cam.ray(Vec2::new(100.0, 300.0), Vec2::new(800.0, 600.0));
        let (_, right) = cam.ray(Vec2::new(700.0, 300.0), Vec2::new(800.0, 600.0));
        let forward = (cam.target - cam.eye()).normalize();
        let screen_right = forward.cross(Vec3::Y);
        assert!(right.dot(screen_right) > left.dot(screen_right));
    }
}
