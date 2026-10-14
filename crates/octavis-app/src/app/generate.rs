//! Terrain generator panel, with a live preview.
//!
//! Terrain lives in its own *generator layer*: the layer's cells are the
//! cached result and the settings (the recipe) are kept in
//! `OctavisApp::recipes`. Changing settings and regenerating rewrites only
//! that layer, so layers below (and your hand work) are untouched.
//!
//! The preview writes the terrain into the generator layer without
//! recording history and keeps the old values, so it can be reverted
//! (Cancel), recomputed when a setting changes, or committed as one undo
//! step (Apply). Editing tools are locked while it is active, so nothing
//! else can touch the previewed cells.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui;
use glam::IVec3;
use octavis_core::{BlockId, Grid, LayerId, LayerKind, Pattern};
use octavis_gen::{Detail, Terrain};

use super::OctavisApp;
use crate::editor::Backup;

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
    /// Preview only the visible skin of the terrain (much faster); Apply
    /// always generates the full result.
    low_preview: bool,
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
            low_preview: true,
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

/// Settings of every generator layer, by layer.
pub(super) type Recipes = HashMap<LayerId, TerrainSettings>;

/// A running preview.
pub(super) struct TerrainPreview {
    /// The generator layer being previewed into.
    layer: LayerId,
    /// Old values of every cell the preview wrote.
    backup: Option<Backup>,
    /// Settings the current preview was generated from.
    shown: TerrainSettings,
    last_update: Instant,
}

impl OctavisApp {
    /// Editing tools are locked while a terrain preview is showing.
    pub(super) fn locked(&self) -> bool {
        self.terrain_preview.is_some()
    }

    /// The active layer, if it is a generator layer.
    fn active_generator(&self) -> Option<LayerId> {
        let id = self.editor.layers.active();
        self.editor.layers.layer(id).filter(|l| l.kind == LayerKind::Generator).map(|l| l.id)
    }

    /// Loads a generator layer's recipe into the panel when it becomes active.
    pub(super) fn sync_recipe(&mut self) {
        let active = self.editor.layers.active();
        if active != self.last_active {
            self.last_active = active;
            if let Some(recipe) = self.recipes.get(&active) {
                self.terrain = recipe.clone();
            }
        }
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
        let generator = self.active_generator();
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
        ui.small(format!(
            "Area: x {}..{}, z {}..{}, y {}..{} ({}x{}x{})",
            min.x, max.x, min.z, max.z, min.y, max.y, size.x, size.y, size.z
        ));

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
        let Some(layer) = generator else {
            ui.small("Terrain is generated into its own layer, so it never overwrites your work.");
            if ui.button("New terrain layer").on_hover_text("Creates a generator layer and generates into it").clicked() {
                self.file_msg = Some(self.create_terrain_layer());
            }
            return;
        };
        let name = self.editor.layers.layer(layer).map(|l| l.name.clone()).unwrap_or_default();
        ui.label(format!("Generator layer: {name}"));
        ui.checkbox(&mut self.terrain.low_preview, "Low-quality preview")
            .on_hover_text("Preview only the visible skin of the terrain: much faster. Apply still generates everything.");
        if previewing {
            ui.colored_label(
                egui::Color32::from_rgb(255, 200, 80),
                "Previewing. Tools are locked until you apply or cancel.",
            );
            ui.horizontal(|ui| {
                if ui.button("Apply (Enter)").on_hover_text("Keep the terrain (one undo step)").clicked() {
                    self.apply_terrain_preview();
                }
                if ui.button("Cancel (Esc)").clicked() {
                    self.cancel_terrain_preview();
                }
            });
        } else {
            ui.horizontal(|ui| {
                if ui.button("Preview").on_hover_text("Show the terrain live while you adjust settings").clicked() {
                    self.start_terrain_preview(layer);
                }
                if ui
                    .button("Regenerate")
                    .on_hover_text("Rewrites this layer with the current settings; one undo step")
                    .clicked()
                {
                    self.file_msg = Some(self.regenerate_terrain(layer));
                }
            });
            if ui
                .button("New terrain layer")
                .on_hover_text("Another generator layer, using the current settings")
                .clicked()
            {
                self.file_msg = Some(self.create_terrain_layer());
            }
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

    /// What the current settings generate, as non-air cells.
    fn fresh_terrain(&mut self, detail: Detail) -> Result<Vec<(IVec3, BlockId)>, String> {
        let terrain = self.build_terrain()?;
        let (min, max) = self.terrain_area();
        let empty = Grid::new();
        if self.terrain_uses_selection() {
            let sel = &self.editor.selection;
            terrain.plan_detail(&empty, min, max, &|p| sel.contains(p), detail)
        } else {
            terrain.plan_detail(&empty, min, max, &|_| true, detail)
        }
    }

    /// Changes that turn `layer`'s cells into the terrain the current
    /// settings describe (the whole layer is replaced, nothing else).
    fn layer_plan(&mut self, layer: LayerId, detail: Detail) -> Result<Vec<(IVec3, BlockId)>, String> {
        let fresh = self.fresh_terrain(detail)?;
        Ok(self.editor.layers.layer(layer).map_or_else(Vec::new, |l| replace_cells(&l.cells, fresh)))
    }

    /// Creates a generator layer from the panel's settings, as one undo step.
    fn create_terrain_layer(&mut self) -> Result<String, String> {
        let cells = self.fresh_terrain(Detail::Full)?;
        let name = format!("Terrain (seed {})", self.terrain.seed);
        let id = self.editor.add_layer_with(&name, LayerKind::Generator, &cells);
        self.recipes.insert(id, self.terrain.clone());
        self.last_active = id;
        Ok(format!("Created \"{name}\": {} blocks. Adjust the settings, then Preview or Regenerate.", cells.len()))
    }

    /// One-shot regeneration into a generator layer: one undo step.
    fn regenerate_terrain(&mut self, layer: LayerId) -> Result<String, String> {
        let plan = self.layer_plan(layer, Detail::Full)?;
        self.recipes.insert(layer, self.terrain.clone());
        if plan.is_empty() {
            return Ok("Terrain already matches; nothing changed".into());
        }
        self.editor.paste_to(layer, &plan);
        Ok(format!("Regenerated terrain (seed {}): {} blocks changed. Ctrl+Z undoes it.", self.terrain.seed, plan.len()))
    }

    pub(super) fn start_terrain_preview(&mut self, layer: LayerId) {
        if self.terrain_preview.is_some() {
            return;
        }
        self.editor.end_edit();
        self.terrain_preview = Some(TerrainPreview {
            layer,
            backup: None,
            shown: self.terrain.clone(),
            last_update: Instant::now(),
        });
        self.refresh_terrain_preview();
    }

    /// Recomputes the preview from the current settings.
    fn refresh_terrain_preview(&mut self) {
        let Some(preview) = self.terrain_preview.as_mut() else { return };
        let layer = preview.layer;
        if let Some(old) = preview.backup.take() {
            self.editor.restore(old);
        }
        let detail = if self.terrain.low_preview { Detail::Shell } else { Detail::Full };
        let plan = self.layer_plan(layer, detail);
        let settings = self.terrain.clone();
        let backup = match plan {
            Ok(plan) => {
                self.file_msg = None;
                Some(self.editor.apply_untracked(layer, &plan))
            }
            Err(e) => {
                self.file_msg = Some(Err(e));
                None
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
            if let Some(backup) = preview.backup {
                self.editor.restore(backup);
            }
        }
    }

    fn apply_terrain_preview(&mut self) {
        let Some(preview) = self.terrain_preview.take() else { return };
        let (layer, seed) = (preview.layer, self.terrain.seed);
        self.recipes.insert(layer, self.terrain.clone());
        if self.terrain.low_preview || preview.shown != self.terrain || preview.backup.is_none() {
            // The preview is only a skin, or is stale: generate the real thing.
            if let Some(backup) = preview.backup {
                self.editor.restore(backup);
            }
            self.file_msg = Some(match self.layer_plan(layer, Detail::Full) {
                Ok(plan) => {
                    self.editor.paste_to(layer, &plan);
                    Ok(format!("Applied terrain (seed {seed}): {} blocks changed. Ctrl+Z undoes it.", plan.len()))
                }
                Err(e) => Err(e),
            });
        } else if let Some(backup) = preview.backup {
            let n = backup.len();
            self.editor.commit_applied(backup);
            self.file_msg = Some(Ok(format!("Applied terrain (seed {seed}): {n} blocks changed. Ctrl+Z undoes it.")));
        }
    }

    /// Enter applies a running preview (unless a text box is being edited).
    pub(super) fn terrain_preview_shortcut(&mut self, ctx: &egui::Context) {
        if self.locked() && !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.apply_terrain_preview();
        }
    }
}

/// Changes that turn `current` into exactly `fresh` (a list of non-air
/// cells): stale cells become air, differing cells are rewritten, matching
/// cells are left alone.
fn replace_cells(current: &Grid, fresh: Vec<(IVec3, BlockId)>) -> Vec<(IVec3, BlockId)> {
    let wanted: std::collections::HashSet<IVec3> = fresh.iter().map(|c| c.0).collect();
    let mut plan: Vec<(IVec3, BlockId)> = current
        .solid_cells()
        .into_iter()
        .filter(|(pos, _)| !wanted.contains(pos))
        .map(|(pos, _)| (pos, BlockId::AIR))
        .collect();
    plan.extend(fresh.into_iter().filter(|&(p, b)| current.get(p) != b));
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_a_layer_removes_stale_cells_and_keeps_matching_ones() {
        let (a, b) = (BlockId(1), BlockId(2));
        let mut layer = Grid::new();
        layer.set(IVec3::new(0, 0, 0), a); // unchanged
        layer.set(IVec3::new(1, 0, 0), a); // changes to b
        layer.set(IVec3::new(50, 0, 0), a); // stale: removed
        let fresh = vec![(IVec3::new(0, 0, 0), a), (IVec3::new(1, 0, 0), b), (IVec3::new(2, 0, 0), b)];
        let mut plan = replace_cells(&layer, fresh);
        plan.sort_by_key(|c| c.0.x);
        assert_eq!(
            plan,
            vec![(IVec3::new(1, 0, 0), b), (IVec3::new(2, 0, 0), b), (IVec3::new(50, 0, 0), BlockId::AIR)]
        );
        // Applying the plan makes the layer match exactly.
        for (p, blk) in plan {
            layer.set(p, blk);
        }
        assert!(replace_cells(&layer, vec![(IVec3::ZERO, a), (IVec3::X, b), (IVec3::new(2, 0, 0), b)]).is_empty());
    }
}
