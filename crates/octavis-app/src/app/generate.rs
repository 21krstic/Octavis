//! Terrain generator panel.
//!
//! Terrain lives in its own *generator layer*: the layer's cells are the
//! cached result and the settings (the recipe) are stored in the layer, so
//! undo restores both together.
//!
//! Touch any setting while a generator layer is active and the terrain
//! *previews* live (a cheap skin-only version by default). The change stays
//! pending, with layers, tools, history and files locked, until you press
//! Apply (or Enter), which generates the full terrain and commits it as one
//! undo step, or Cancel (or Esc), which restores the old terrain and settings.
//!
//! While a change is pending the preview is written into the layer without
//! history and the old cells are kept, so it can be reverted or replaced.

use std::time::{Duration, Instant};

use eframe::egui;
use glam::IVec3;
use octavis_core::{BlockId, Grid, LayerId, LayerKind, Pattern};
use octavis_gen::{Detail, Terrain};

use super::OctavisApp;
use crate::editor::Backup;

/// Minimum time between preview recomputations while a slider is dragged.
const PREVIEW_INTERVAL: Duration = Duration::from_millis(150);

/// Generator options. Patterns are text until generation so half-typed
/// names never reach the block table.
#[derive(Clone, PartialEq, Debug)]
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

    /// The three block-name boxes' starting text.
    pub(super) fn texts(&self) -> [String; 3] {
        [self.surface.clone(), self.soil.clone(), self.fill.clone()]
    }

    /// The recipe as stored in a layer: one `key=value` per line.
    fn to_text(&self) -> String {
        format!(
            "terrain v1\nseed={}\ncentre_x={}\ncentre_z={}\nsize={}\ndepth={}\nuse_selection={}\nscale={}\noctaves={}\npersistence={}\nbase={}\namplitude={}\nsoil_depth={}\nsurface={}\nsoil={}\nfill={}\n",
            self.seed,
            self.centre_x,
            self.centre_z,
            self.size,
            self.depth,
            self.use_selection,
            self.scale,
            self.octaves,
            self.persistence,
            self.base,
            self.amplitude,
            self.soil_depth,
            self.surface,
            self.soil,
            self.fill
        )
    }

    /// Reads a recipe; unknown or missing keys keep their defaults.
    fn from_text(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        if lines.next()? != "terrain v1" {
            return None;
        }
        let mut s = Self::new();
        for line in lines {
            let Some((key, value)) = line.split_once('=') else { continue };
            match key {
                "seed" => s.seed = value.parse().ok()?,
                "centre_x" => s.centre_x = value.parse().ok()?,
                "centre_z" => s.centre_z = value.parse().ok()?,
                "size" => s.size = value.parse().ok()?,
                "depth" => s.depth = value.parse().ok()?,
                "use_selection" => s.use_selection = value.parse().ok()?,
                "scale" => s.scale = value.parse().ok()?,
                "octaves" => s.octaves = value.parse().ok()?,
                "persistence" => s.persistence = value.parse().ok()?,
                "base" => s.base = value.parse().ok()?,
                "amplitude" => s.amplitude = value.parse().ok()?,
                "soil_depth" => s.soil_depth = value.parse().ok()?,
                "surface" => s.surface = value.into(),
                "soil" => s.soil = value.into(),
                "fill" => s.fill = value.into(),
                _ => {}
            }
        }
        Some(s)
    }
}

/// A pending (uncommitted) terrain change.
pub(super) struct TerrainPreview {
    /// The generator layer being previewed into.
    layer: LayerId,
    /// Old values of every cell the preview wrote.
    backup: Option<Backup>,
    /// Settings the current preview shows.
    shown: TerrainSettings,
    last_update: Instant,
}

impl OctavisApp {
    /// The active layer and its recipe, if it is a generator layer.
    fn active_generator(&self) -> Option<(LayerId, String)> {
        let l = self.editor.layers.layer(self.editor.layers.active())?;
        if l.kind != LayerKind::Generator {
            return None;
        }
        l.recipe.clone().map(|r| (l.id, r))
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

    /// Replaces the panel's settings (and the text boxes) with a recipe.
    fn load_terrain_settings(&mut self, settings: TerrainSettings) {
        self.terrain_text = settings.texts();
        self.terrain = settings;
    }

    pub(super) fn terrain_panel(&mut self, ui: &mut egui::Ui) {
        let (min, max) = self.terrain_area();
        let has_selection = !self.editor.selection.is_empty();
        let generator = self.active_generator();
        let camera_cell = self.camera.target.floor().as_ivec3();
        let pending = self.terrain_preview.is_some();
        // Always drawn (greyed out when idle) so the panel never shifts under
        // a slider that is being dragged.
        ui.horizontal(|ui| {
            if ui.add_enabled(pending, egui::Button::new("Apply")).on_hover_text("Generate the full terrain and keep it (Enter)").clicked() {
                self.commit_terrain_preview();
            }
            if ui.add_enabled(pending, egui::Button::new("Cancel")).on_hover_text("Go back to the previous terrain and settings (Esc)").clicked() {
                self.cancel_terrain_preview();
            }
            if pending {
                ui.colored_label(egui::Color32::from_rgb(255, 170, 60), "Pending: layers and tools locked");
            }
        });
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
        // Block names are edited in separate buffers and only reach the
        // settings once they are complete: every name already in the scene
        // applies at once, a brand-new name when you press Enter or leave
        // the box. Otherwise typing "moss" would create "m", "mo", "mos".
        for (i, label) in ["Surface block(s)", "Soil block(s)", "Deep block(s)"].into_iter().enumerate() {
            ui.label(label);
            let response =
                ui.add(egui::TextEdit::singleline(&mut self.terrain_text[i]).desired_width(f32::INFINITY));
            let text = self.terrain_text[i].clone();
            match Pattern::validate(&text) {
                Err(e) => {
                    ui.colored_label(egui::Color32::from_rgb(255, 110, 90), e);
                }
                Ok(()) => {
                    let new = Pattern::unknown_names(&text, &self.editor.world.blocks);
                    if !new.is_empty() {
                        ui.colored_label(
                            egui::Color32::from_rgb(255, 170, 60),
                            format!("Creates new block type: {}", new.join(", ")),
                        );
                    }
                    if new.is_empty() || response.lost_focus() {
                        let field = [&mut self.terrain.surface, &mut self.terrain.soil, &mut self.terrain.fill];
                        *field[i] = text;
                    }
                }
            }
        }

        ui.separator();
        ui.checkbox(&mut self.fast_preview, "Fast preview (skin only)").on_hover_text(
            "While you adjust settings, show only the visible skin of the terrain. The full terrain is generated when you apply.",
        );
        match generator {
            Some((layer, _)) => {
                let name = self.editor.layers.layer(layer).map(|l| l.name.clone()).unwrap_or_default();
                ui.label(format!("Generator layer: {name}"));
                ui.small("Changes preview live. Apply (Enter) keeps them, Cancel (Esc) goes back.");
                if ui
                    .add_enabled(!pending, egui::Button::new("New terrain layer"))
                    .on_hover_text("Another generator layer, using these settings")
                    .clicked()
                {
                    self.file_msg = Some(self.create_terrain_layer());
                }
            }
            None => {
                ui.small("Terrain is generated into its own layer, so it never overwrites your work.");
                if ui
                    .add_enabled(!pending, egui::Button::new("New terrain layer"))
                    .on_hover_text("Creates a generator layer and generates into it")
                    .clicked()
                {
                    self.file_msg = Some(self.create_terrain_layer());
                }
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
        // A pending change belonged to the old layer; drop it but keep the
        // panel's settings, which are what the new layer should get.
        if let Some(preview) = self.terrain_preview.take() {
            if let Some(backup) = preview.backup {
                self.editor.restore(backup);
            }
        }
        let cells = self.fresh_terrain(Detail::Full)?;
        let name = format!("Terrain (seed {})", self.terrain.seed);
        let recipe = self.terrain.to_text();
        self.editor.add_layer_with(&name, LayerKind::Generator, Some(recipe.clone()), &cells);
        self.synced_recipe = recipe;
        Ok(format!("Created \"{name}\": {} blocks. Change any setting to adjust it.", cells.len()))
    }

    /// Call every frame. Keeps the panel and the active generator layer in
    /// step: loads a layer's recipe when it becomes active (or after undo),
    /// starts a preview when the settings are edited and refreshes it while
    /// they keep changing. Committing is up to the user (Apply / Enter).
    pub(super) fn update_terrain(&mut self, ctx: &egui::Context) {
        if self.terrain_preview.is_some() {
            self.update_pending_terrain(ctx);
            return;
        }
        let Some((layer, recipe)) = self.active_generator() else { return };
        if recipe != self.synced_recipe {
            // A different layer, or undo/redo: show that layer's recipe.
            if let Some(settings) = TerrainSettings::from_text(&recipe) {
                self.load_terrain_settings(settings);
            }
            self.synced_recipe = recipe;
        } else if self.terrain.to_text() != recipe {
            self.terrain_preview = Some(TerrainPreview {
                layer,
                backup: None,
                shown: self.terrain.clone(),
                last_update: Instant::now() - PREVIEW_INTERVAL,
            });
            self.refresh_terrain_preview();
        }
    }

    fn update_pending_terrain(&mut self, ctx: &egui::Context) {
        let Some(preview) = self.terrain_preview.as_ref() else { return };
        if preview.shown != self.terrain {
            // Still being edited: refresh at a sane rate.
            let wait = PREVIEW_INTERVAL.saturating_sub(preview.last_update.elapsed());
            if wait.is_zero() {
                self.refresh_terrain_preview();
            } else {
                ctx.request_repaint_after(wait);
            }
        }
    }

    /// Recomputes the preview from the current settings.
    fn refresh_terrain_preview(&mut self) {
        let Some(preview) = self.terrain_preview.as_mut() else { return };
        let layer = preview.layer;
        if let Some(old) = preview.backup.take() {
            self.editor.restore(old);
        }
        let detail = if self.fast_preview { Detail::Shell } else { Detail::Full };
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

    /// Shows the committed recipe of `layer` in the panel again.
    fn reload_committed_recipe(&mut self, layer: LayerId) {
        let recipe = self.editor.layers.layer(layer).and_then(|l| l.recipe.clone());
        if let Some(settings) = recipe.as_deref().and_then(TerrainSettings::from_text) {
            self.load_terrain_settings(settings);
        }
    }

    /// Discards a pending change and shows the layer's committed recipe again.
    pub(super) fn cancel_terrain_preview(&mut self) {
        let Some(preview) = self.terrain_preview.take() else { return };
        if let Some(backup) = preview.backup {
            self.editor.restore(backup);
        }
        self.reload_committed_recipe(preview.layer);
    }

    /// Generates the full terrain for the pending change and commits it
    /// (cells and recipe together) as one undo step.
    pub(super) fn commit_terrain_preview(&mut self) {
        let Some(preview) = self.terrain_preview.take() else { return };
        let layer = preview.layer;
        if let Some(backup) = preview.backup {
            self.editor.restore(backup);
        }
        // Use what is in the panel now, even if the last refresh lagged behind.
        match self.layer_plan(layer, Detail::Full) {
            Ok(plan) => {
                let recipe = self.terrain.to_text();
                self.editor.regenerate_layer(layer, &plan, recipe.clone());
                self.synced_recipe = recipe;
                self.file_msg = Some(Ok(format!(
                    "Terrain updated (seed {}): {} blocks changed. Ctrl+Z undoes it.",
                    self.terrain.seed,
                    plan.len()
                )));
            }
            Err(e) => {
                self.file_msg = Some(Err(e));
                self.reload_committed_recipe(layer);
            }
        }
    }

    /// Enter applies and Esc cancels a pending terrain change (unless a
    /// text box is being edited; Enter there finishes the block name first).
    pub(super) fn terrain_preview_shortcut(&mut self, ctx: &egui::Context) {
        if self.terrain_preview.is_none() || ctx.egui_wants_keyboard_input() {
            return;
        }
        let (enter, esc) = ctx.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if enter {
            self.commit_terrain_preview();
        } else if esc {
            self.cancel_terrain_preview();
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

    #[test]
    fn recipe_text_roundtrips_and_tolerates_unknown_keys() {
        let mut s = TerrainSettings::new();
        s.seed = 4242;
        s.scale = 33.5;
        s.use_selection = false;
        s.surface = "70%moss_block, 30%grass_block".into();
        s.centre_x = -17;
        let text = s.to_text();
        assert_eq!(TerrainSettings::from_text(&text), Some(s.clone()));
        let extended = format!("{text}future_key=whatever\nnot a pair\n");
        assert_eq!(TerrainSettings::from_text(&extended), Some(s));
        assert!(TerrainSettings::from_text("something else").is_none());
        assert!(TerrainSettings::from_text("terrain v1\nseed=notanumber\n").is_none());
    }
}
