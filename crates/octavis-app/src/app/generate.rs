//! Terrain generator panel.

use eframe::egui;
use glam::IVec3;
use octavis_core::Pattern;
use octavis_gen::Terrain;

use super::OctavisApp;

/// Generator options as edited in the panel. Patterns stay text until
/// generation so half-typed names never reach the block table.
pub(super) struct TerrainSettings {
    seed: u32,
    /// Side of the square generated around the camera target (no selection).
    size: i32,
    scale: f32,
    octaves: u32,
    persistence: f32,
    base: i32,
    amplitude: f32,
    /// How far below `base` columns are rewritten (no selection).
    depth: i32,
    soil_depth: i32,
    surface: String,
    soil: String,
    fill: String,
}

impl TerrainSettings {
    pub(super) fn new() -> Self {
        Self {
            seed: 1,
            size: 128,
            scale: 64.0,
            octaves: 4,
            persistence: 0.5,
            base: 0,
            amplitude: 20.0,
            depth: 12,
            soil_depth: 3,
            surface: "grass_block".into(),
            soil: "dirt".into(),
            fill: "stone".into(),
        }
    }
}

impl OctavisApp {
    /// The box terrain is generated into: the selection's bounds, else a
    /// square around the camera target sized by the settings.
    fn terrain_area(&self) -> (IVec3, IVec3) {
        if let Some(bounds) = self.editor.selection.bounds() {
            return bounds;
        }
        let t = &self.terrain;
        let centre = self.camera.target.floor().as_ivec3();
        let min = IVec3::new(centre.x - t.size / 2, t.base - t.depth, centre.z - t.size / 2);
        let max = IVec3::new(min.x + t.size - 1, t.base + t.amplitude.ceil() as i32, min.z + t.size - 1);
        (min, max)
    }

    pub(super) fn terrain_panel(&mut self, ui: &mut egui::Ui) {
        let (min, max) = self.terrain_area();
        let has_selection = !self.editor.selection.is_empty();
        let t = &mut self.terrain;
        ui.horizontal(|ui| {
            ui.label("Seed");
            ui.add(egui::DragValue::new(&mut t.seed));
            if ui.button("Random").clicked() {
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(1, |d| d.subsec_nanos() ^ d.as_secs() as u32);
                t.seed = nanos.wrapping_mul(2_654_435_761) % 1_000_000;
            }
        });
        ui.add(egui::Slider::new(&mut t.scale, 4.0..=256.0).logarithmic(true).text("Scale"));
        ui.add(egui::Slider::new(&mut t.octaves, 1..=8).text("Octaves"));
        ui.add(egui::Slider::new(&mut t.persistence, 0.1..=0.9).text("Roughness"));
        ui.add(egui::Slider::new(&mut t.base, -64..=256).text("Base height"));
        ui.add(egui::Slider::new(&mut t.amplitude, 0.0..=64.0).text("Amplitude"));
        ui.add(egui::Slider::new(&mut t.soil_depth, 0..=8).text("Soil depth"));
        if !has_selection {
            ui.add(egui::Slider::new(&mut t.size, 16..=256).text("Area size"));
            ui.add(egui::Slider::new(&mut t.depth, 1..=64).text("Depth below base"));
        }

        for (label, text) in [
            ("Surface block(s)", &mut t.surface),
            ("Soil block(s)", &mut t.soil),
            ("Deep block(s)", &mut t.fill),
        ] {
            ui.label(label);
            ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
        }
        let texts = [t.surface.clone(), t.soil.clone(), t.fill.clone()];
        for text in &texts {
            if let Err(e) = Pattern::validate(text) {
                ui.colored_label(egui::Color32::from_rgb(255, 110, 90), format!("{text}: {e}"));
            } else {
                let new = Pattern::unknown_names(text, &self.editor.world.blocks);
                if !new.is_empty() {
                    ui.colored_label(
                        egui::Color32::from_rgb(255, 170, 60),
                        format!("Creates new block type: {}", new.join(", ")),
                    );
                }
            }
        }

        let size = max - min + IVec3::ONE;
        ui.small(if has_selection {
            format!("Area: selection, {}x{}x{}", size.x, size.y, size.z)
        } else {
            format!("Area: around the camera target, {}x{}x{}", size.x, size.y, size.z)
        });
        if ui.button("Generate terrain").on_hover_text("Replaces everything in the area; one undo step").clicked() {
            self.file_msg = Some(self.generate_terrain());
        }
    }

    fn generate_terrain(&mut self) -> Result<String, String> {
        let (min, max) = self.terrain_area();
        let t = &self.terrain;
        let (seed, scale, octaves, persistence, base, amplitude, soil_depth) =
            (t.seed, t.scale, t.octaves, t.persistence, t.base, t.amplitude, t.soil_depth);
        let texts = [t.surface.clone(), t.soil.clone(), t.fill.clone()];

        let before = self.editor.world.blocks.len();
        let mut patterns = Vec::new();
        for text in &texts {
            patterns.push(Pattern::parse(text, &mut self.editor.world.blocks).map_err(|e| format!("{text}: {e}"))?);
        }
        if self.editor.world.blocks.len() != before {
            self.palette_dirty = true;
        }
        let [surface, soil, fill]: [Pattern; 3] = patterns.try_into().map_err(|_| "pattern error")?;
        let terrain =
            Terrain { seed, scale, octaves, persistence, base, amplitude, soil_depth, surface, soil, fill };

        let plan = terrain.plan(&self.editor.world, min, max)?;
        if plan.is_empty() {
            return Ok("Terrain already matches; nothing changed".into());
        }
        self.editor.paste(&plan);
        let size = max - min + IVec3::ONE;
        Ok(format!(
            "Generated terrain (seed {seed}): {} blocks changed in {}x{}x{}. Ctrl+Z undoes it.",
            plan.len(),
            size.x,
            size.y,
            size.z
        ))
    }
}
