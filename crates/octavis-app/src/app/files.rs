//! File menu: new scene, open / import / export `.schem`.
//!
//! Dialogs are modal (they block the UI thread), which is fine for these
//! one-shot operations.

use std::path::Path;

use eframe::egui;
use glam::IVec3;
use octavis_core::{BlockId, Selection, World};
use octavis_io::schem::{self, Schematic};

use super::OctavisApp;
use crate::editor::MAX_SELECTION_CELLS;
use crate::{render, scene};

#[derive(Clone, Copy)]
pub enum FileAction {
    New,
    Open,
    Import,
    Export,
}

/// `None` means the user cancelled.
type Outcome = Option<Result<String, String>>;

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn warnings_suffix(s: &Schematic) -> String {
    if s.warnings.is_empty() { String::new() } else { format!(" ({})", s.warnings.join("; ")) }
}

impl OctavisApp {
    pub(super) fn menu_bar(&mut self, ui: &mut egui::Ui) -> Option<FileAction> {
        let mut action = None;
        // Files can't be replaced or exported mid terrain / transform change.
        let pending = self.pending();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.add_enabled_ui(!pending, |ui| ui.menu_button("File", |ui| {
                for (a, label) in [
                    (FileAction::New, "New scene"),
                    (FileAction::Open, "Open .schem..."),
                    (FileAction::Import, "Import .schem into scene..."),
                    (FileAction::Export, "Export .schem (selection or everything)..."),
                ] {
                    if ui.button(label).clicked() {
                        action = Some(a);
                        ui.close();
                    }
                }
            }));
        });
        action
    }

    pub(super) fn run_file_action(&mut self, action: FileAction) {
        let outcome = match action {
            FileAction::New => self.new_scene(),
            FileAction::Open => self.open_schem(),
            FileAction::Import => self.import_schem(),
            FileAction::Export => self.export_schem(),
        };
        if outcome.is_some() {
            self.file_msg = outcome;
        }
    }

    /// Asks before a replacing operation throws away undoable work.
    fn confirm_discard(&self, what: &str) -> bool {
        !self.editor.history.can_undo()
            || rfd::MessageDialog::new()
                .set_title("Discard changes?")
                .set_description(format!("{what} replaces the current scene. Unsaved edits will be lost."))
                .set_buttons(rfd::MessageButtons::YesNo)
                .show()
                == rfd::MessageDialogResult::Yes
    }

    /// Call after the world or its block table was replaced or extended.
    fn after_blocks_changed(&mut self) -> String {
        self.palette_dirty = true;
        self.overlay_dirty = true;
        let n = self.editor.world.blocks.len();
        if self.editor.world.blocks.get(self.block).is_none() || self.block == BlockId::AIR {
            self.block = BlockId(1.min(n.saturating_sub(1)) as u32);
        }
        if n > render::MAX_BLOCK_IDS {
            format!(
                " Warning: {n} block types; only the first {} get a placeholder colour (the rest show magenta until texture packs exist).",
                render::MAX_BLOCK_IDS
            )
        } else {
            String::new()
        }
    }

    fn pick_and_read(&self) -> Option<Result<(String, Schematic), String>> {
        let path = rfd::FileDialog::new().add_filter("Sponge schematic", &["schem"]).pick_file()?;
        let name = file_name(&path);
        Some(
            std::fs::read(&path)
                .map_err(|e| format!("Could not read {name}: {e}"))
                .and_then(|bytes| schem::read_schem(&bytes).map_err(|e| format!("{name}: {e}")))
                .map(|s| (name, s)),
        )
    }

    fn new_scene(&mut self) -> Outcome {
        if !self.confirm_discard("A new scene") {
            return None;
        }
        self.editor.replace_world(scene::demo_world(), "Ground");
        self.sent_hover.clear();
        self.after_blocks_changed();
        Some(Ok("New scene".into()))
    }

    fn open_schem(&mut self) -> Outcome {
        let (name, s) = match self.pick_and_read()? {
            Ok(v) => v,
            Err(e) => return Some(Err(e)),
        };
        if !self.confirm_discard("Opening a file") {
            return None;
        }
        let mut world = World::new();
        let cells = s.place(&mut world.blocks, IVec3::ZERO);
        for &(p, b) in &cells {
            world.set(p, b);
        }
        world.compact();
        self.editor.replace_world(world, name.trim_end_matches(".schem"));
        self.sent_hover.clear();
        // Frame the whole build.
        self.camera.target = s.size.as_vec3() * 0.5;
        self.camera.distance = (s.size.max_element() as f32 * 1.5).clamp(10.0, 1500.0);
        let note = self.after_blocks_changed();
        Some(Ok(format!(
            "Opened {name}: {}x{}x{}, {} blocks{}.{note}",
            s.size.x,
            s.size.y,
            s.size.z,
            cells.len(),
            warnings_suffix(&s)
        )))
    }

    /// Imports a schematic as a new layer, centred on the camera target and
    /// standing on the highest visible block of that column. Being its own
    /// layer, it never overwrites anything. The result is selected; one undo step.
    fn import_schem(&mut self) -> Outcome {
        let (name, s) = match self.pick_and_read()? {
            Ok(v) => v,
            Err(e) => return Some(Err(e)),
        };
        let centre = self.camera.target.floor().as_ivec3();
        let (x, z) = (centre.x - s.size.x / 2, centre.z - s.size.z / 2);
        let ground = (-256..=1024)
            .rev()
            .find(|&y| self.editor.world.get(IVec3::new(centre.x, y, centre.z)) != BlockId::AIR)
            .map_or(0, |y| y + 1);
        let origin = IVec3::new(x, ground, z);

        let cells = s.place(&mut self.editor.world.blocks, origin);
        self.editor.add_layer_with(name.trim_end_matches(".schem"), octavis_core::LayerKind::Painted, None, &cells);
        if cells.len() as i64 <= MAX_SELECTION_CELLS {
            let sel = Selection::from_cells(cells.iter().map(|c| c.0));
            self.editor.apply_selection(crate::editor::SelectMode::Replace, &sel);
        }
        let note = self.after_blocks_changed();
        Some(Ok(format!(
            "Imported {name} at {}, {}, {}: {} blocks{}. Ctrl+Z undoes it.{note}",
            origin.x,
            origin.y,
            origin.z,
            cells.len(),
            warnings_suffix(&s)
        )))
    }

    /// Exports the selection if there is one, else every block in the scene.
    fn export_schem(&mut self) -> Outcome {
        let world = &self.editor.world;
        let sel = &self.editor.selection;
        let bounds = sel.bounds().or_else(|| world.content_bounds());
        let Some((min, max)) = bounds else {
            return Some(Err("Nothing to export: the scene is empty".into()));
        };
        let schematic = match Schematic::from_world(world, min, max, |p| sel.is_empty() || sel.contains(p)) {
            Ok(s) => s,
            Err(e) => return Some(Err(format!("Export failed: {e}"))),
        };
        let mut path = rfd::FileDialog::new()
            .add_filter("Sponge schematic", &["schem"])
            .set_file_name("build.schem")
            .save_file()?;
        if path.extension().is_none() {
            path.set_extension("schem");
        }
        let name = file_name(&path);
        let blocks = schematic.indices.iter().filter(|&&i| i != 0).count();
        let result = schem::write_schem(&schematic, schem::DEFAULT_DATA_VERSION)
            .map_err(|e| format!("Export failed: {e}"))
            .and_then(|bytes| {
                std::fs::write(&path, bytes).map_err(|e| format!("Could not write {name}: {e}"))
            });
        Some(result.map(|()| {
            let what = if sel.is_empty() { "scene" } else { "selection" };
            format!(
                "Exported {what} to {name}: {}x{}x{}, {blocks} blocks",
                schematic.size.x, schematic.size.y, schematic.size.z
            )
        }))
    }
}
