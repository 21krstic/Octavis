use std::sync::Mutex;

use eframe::egui_wgpu;
use glam::{IVec3, Vec3};
use octavis_core::World;

use crate::camera::Camera;
use crate::render::{self, ViewportCallback};
use crate::scene;

pub struct OctavisApp {
    world: World,
    camera: Camera,
    /// Sections whose GPU mesh must be rebuilt before the next draw.
    dirty: Vec<IVec3>,
    palette_dirty: bool,
}

impl OctavisApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let render_state = cc.wgpu_render_state.as_ref().expect("wgpu backend required");
        render::init(render_state);

        let world = scene::demo_world();
        let dirty = world.sections().map(|(p, _)| p).collect();
        Self { world, camera: Camera::new(Vec3::new(0.0, 4.0, 0.0)), dirty, palette_dirty: true }
    }

    fn take_uploads(&mut self) -> Vec<(IVec3, Option<octavis_mesh::Mesh>)> {
        self.dirty
            .drain(..)
            .map(|p| {
                let mesh = self.world.section(p).map(|_| octavis_mesh::mesh_section(&self.world, p));
                (p, mesh)
            })
            .collect()
    }
}

impl eframe::App for OctavisApp {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        eframe::egui::Frame::new().show(ui, |ui| {
            let (rect, response) =
                ui.allocate_exact_size(ui.available_size(), eframe::egui::Sense::click_and_drag());

            // Navigation: right-drag orbits, middle-drag (or shift+right) pans, wheel zooms.
            let delta = response.drag_delta();
            let shift = ui.input(|i| i.modifiers.shift);
            if response.dragged_by(eframe::egui::PointerButton::Middle)
                || (shift && response.dragged_by(eframe::egui::PointerButton::Secondary))
            {
                self.camera.pan(delta.x, delta.y);
            } else if response.dragged_by(eframe::egui::PointerButton::Secondary) {
                self.camera.orbit(delta.x, delta.y);
            }
            if response.hovered() {
                self.camera.zoom(ui.input(|i| i.smooth_scroll_delta.y));
            }

            let uploads = self.take_uploads();
            let palette = std::mem::take(&mut self.palette_dirty)
                .then(|| render::palette_colors(&self.world.blocks));
            let aspect = rect.width() / rect.height().max(1.0);
            ui.painter().add(egui_wgpu::Callback::new_paint_callback(
                rect,
                ViewportCallback {
                    view_proj: self.camera.view_proj(aspect),
                    palette: Mutex::new(palette),
                    uploads: Mutex::new(uploads),
                },
            ));
        });
    }
}
