//! Terrain generator panel, with a live preview.
//!
//! The preview writes the terrain into the world without recording
//! history and keeps the old values, so it can be reverted (Cancel),
//! recomputed when a setting changes, or committed as one undo step
//! (Apply). Editing tools are locked while it is active, so nothing else
//! can touch the previewed cells.

use std::time::{Duration, Instant};

use eframe::egui;
use glam::IVec3;
use octavis_core::{BlockId, Pattern};
use octavis_gen::Terrain;

use super::OctavisApp;

/// Minimum time between preview recomputations while a slider is dragged.
const PREVIEW_INTERVAL: Duration = Duration::from_millis(150);

/// Generator options as edited in the panel. Patterns stay text until
/// generation so half-typed names never reach the block table.
#[derive(Clone, PartialEq)]
pub(super) struct TerrainSettings {
    seed: u32,
    /// Manual area: a square of `size` centred on (`centre_x`, `centre_z`),
    /// rewritten from `base - depth` up to `base + amplitude`. Independent
    /// of the camera.
    centre_x: i32,
    centre_z: i32,
    size: i32,
    depth: i32,
    /// Fit to the selection (its bounds, and only its cells) when one exists.
    use_selection: bool,
    scale: f32,
    octaves: u32,
    persistence: f32,
    base: i32,
    amplitude: f32,
    soil_depth: i32,
    surface: String,
    soil: String,
    fill: String,
}

impl TerrainSettings {
    pub(super) fn new() -> Self {
        Self {
            seed: 1,
            centre_x: 0,
            centre_z: 0,
            size: 128,
            depth: 12,
            use_selection: true,
            scale: 64.0,
            octaves: 4,
            persistence: 0.5,
            base: 0,
            amplitude: 20.0,
            soil_depth: 3,
            surface: "grass_block".into(),
            soil: "dirt".into(),
            fill: "stone".into(),
        }
    }
}

/// A running preview.
pub(super) struct TerrainPreview {
    /// Old values of every cell the preview wrote.
    backup: Vec<(IVec3, BlockId)>,
    /// Settings the current preview was generated from.
    shown: TerrainSettings,
    last_update: Instant,
}

impl OctavisApp {
    /// Editing tools are locked while a terrain preview is showing.
    pub(super) fn locked(&self) -> bool {
        self.terrain_preview.is_some()
    }

    /// True if the terrain is clipped to the selection.
    fn terrain_uses_selection(&self) -> bool {
        self.terrain.use_selection && !self.editor.selection.is_empty()
    }

    /// The inclusive box terrain is generated into.
    fn terrain_area(&self) -> (IVec3, IVec3) {
        if self.terrain_uses_selection() {
            if let Some(bounds) = self.editor.selection.bounds() {
                return bounds;
            }
        }
        let t = &self.terrain;
        let min = IVec3::new(t.centre_x - t.size / 2, t.base - t.depth, t.centre_z - t.size / 2);
        let max = IVec3::new(min.x + t.size - 1, t.base + t.amplitude.ceil() as i32, min.z + t.size - 1);
        (min, max)
    }

    pub(super) fn terrain_panel(&mut self, ui: &mut egui::Ui) {
        let (min, max) = self.terrain_area();
        let has_selection = !self.editor.selection.is_empty();
        let previewing = self.locked();
        let camera_cell = self.camera.target.floor().as_ivec3();
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

        ui.separator();
        ui.add_enabled(has_selection, egui::Checkbox::new(&mut t.use_selection, "Fit to selection"));
        if !(t.use_selection && has_selection) {
            ui.horizontal(|ui| {
                ui.label("Centre X");
                ui.add(egui::DragValue::new(&mut t.centre_x));
                ui.label("Z");
                ui.add(egui::DragValue::new(&mut t.centre_z));
            });
            if ui.button("Centre on camera target").clicked() {
                t.centre_x = camera_cell.x;
                t.centre_z = camera_cell.z;
            }
            ui.add(egui::Slider::new(&mut t.size, 16..=256).text("Area size"));
            ui.add(egui::Slider::new(&mut t.depth, 1..=64).text("Depth below base"));
        }
        let size = max - min + IVec3::ONE;
        ui.small(format!("Area: x {}..{}, z {}..{}, y {}..{} ({}x{}x{})", min.x, max.x, min.z, max.z, min.y, max.y, size.x, size.y, size.z));

        ui.separator();
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

        ui.separator();
        if previewing {
            ui.colored_label(egui::Color32::from_rgb(255, 200, 80), "Previewing. Tools are locked until you apply or cancel.");
            ui.horizontal(|ui| {
                if ui.button("Apply").on_hover_text("Keep the terrain (one undo step)").clicked() {
                    self.apply_terrain_preview();
                }
                if ui.button("Cancel (Esc)").clicked() {
                    self.cancel_terrain_preview();
                }
            });
        } else {
            ui.horizontal(|ui| {
                if ui.button("Preview").on_hover_text("Show the terrain live while you adjust settings").clicked() {
                    self.start_terrain_preview();
                }
                if ui.button("Generate").on_hover_text("Replaces everything in the area; one undo step").clicked() {
                    self.file_msg = Some(self.generate_terrain());
                }
            });
        }
    }

    /// Builds the generator from the settings, interning pattern blocks.
    fn build_terrain(&mut self) -> Result<Terrain, String> {
        let t = self.terrain.clone();
        let before = self.editor.world.blocks.len();
        let mut parse = |text: &str| {
            Pattern::parse(text, &mut self.editor.world.blocks).map_err(|e| format!("{text}: {e}"))
        };
        let (surface, soil, fill) = (parse(&t.surface)?, parse(&t.soil)?, parse(&t.fill)?);
        if self.editor.world.blocks.len() != before {
            self.palette_dirty = true;
        }
        Ok(Terrain {
            seed: t.seed,
            scale: t.scale,
            octaves: t.octaves,
            persistence: t.persistence,
            base: t.base,
            amplitude: t.amplitude,
            soil_depth: t.soil_depth,
            surface,
            soil,
            fill,
        })
    }

    /// Changes the current settings would make to the world as it is now.
    fn plan_terrain(&mut self) -> Result<Vec<(IVec3, BlockId)>, String> {
        let terrain = self.build_terrain()?;
        let (min, max) = self.terrain_area();
        if self.terrain_uses_selection() {
            let sel = &self.editor.selection;
            terrain.plan(&self.editor.world, min, max, &|p| sel.contains(p))
        } else {
            terrain.plan(&self.editor.world, min, max, &|_| true)
        }
    }

    /// One-shot generation: one undo step.
    fn generate_terrain(&mut self) -> Result<String, String> {
        let plan = self.plan_terrain()?;
        if plan.is_empty() {
            return Ok("Terrain already matches; nothing changed".into());
        }
        self.editor.paste(&plan);
        Ok(format!("Generated terrain (seed {}): {} blocks changed. Ctrl+Z undoes it.", self.terrain.seed, plan.len()))
    }

    pub(super) fn start_terrain_preview(&mut self) {
        if self.terrain_preview.is_some() {
            return;
        }
        // Open edits (a half-finished stroke cannot exist here, but be safe).
        self.editor.end_edit();
        self.terrain_preview = Some(TerrainPreview {
            backup: Vec::new(),
            shown: self.terrain.clone(),
            last_update: Instant::now(),
        });
        self.refresh_terrain_preview();
    }

    /// Recomputes the preview from the current settings.
    fn refresh_terrain_preview(&mut self) {
        let Some(preview) = self.terrain_preview.as_mut() else { return };
        let old = std::mem::take(&mut preview.backup);
        self.editor.restore(old);
        let plan = self.plan_terrain();
        let settings = self.terrain.clone();
        let backup = match plan {
            Ok(plan) => {
                self.file_msg = None;
                self.editor.apply_untracked(&plan)
            }
            Err(e) => {
                self.file_msg = Some(Err(e));
                Vec::new()
            }
        };
        if let Some(preview) = self.terrain_preview.as_mut() {
            preview.backup = backup;
            preview.shown = settings;
            preview.last_update = Instant::now();
        }
    }

    /// Call every frame: refreshes the preview when settings changed, at
    /// most every [`PREVIEW_INTERVAL`] so dragging a slider stays responsive.
    pub(super) fn update_terrain_preview(&mut self, ctx: &egui::Context) {
        let Some(preview) = &self.terrain_preview else { return };
        if preview.shown == self.terrain {
            return;
        }
        let wait = PREVIEW_INTERVAL.saturating_sub(preview.last_update.elapsed());
        if wait.is_zero() {
            self.refresh_terrain_preview();
        } else {
            ctx.request_repaint_after(wait);
        }
    }

    pub(super) fn cancel_terrain_preview(&mut self) {
        if let Some(preview) = self.terrain_preview.take() {
            self.editor.restore(preview.backup);
        }
    }

    fn apply_terrain_preview(&mut self) {
        // Make sure what is shown matches the settings, then keep it.
        if self.terrain_preview.as_ref().is_some_and(|p| p.shown != self.terrain) {
            self.refresh_terrain_preview();
        }
        if let Some(preview) = self.terrain_preview.take() {
            let n = preview.backup.len();
            self.editor.commit_applied(preview.backup);
            self.file_msg = Some(Ok(format!("Applied terrain (seed {}): {n} blocks changed. Ctrl+Z undoes it.", self.terrain.seed)));
        }
    }
}
