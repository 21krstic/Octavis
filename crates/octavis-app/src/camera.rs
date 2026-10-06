use glam::{Mat4, Vec3};

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
