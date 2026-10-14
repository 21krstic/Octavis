//! Layers panel.

use eframe::egui;
use octavis_core::{LayerId, LayerKind};

use super::OctavisApp;

impl OctavisApp {
    pub(super) fn layers_panel(&mut self, ui: &mut egui::Ui) {
        let active = self.editor.layers.active();
        let solo = self.editor.layers.solo();
        // Top layer first, like most editors.
        let rows: Vec<(LayerId, String, bool, bool, LayerKind)> = self
            .editor
            .layers
            .layers()
            .iter()
            .rev()
            .map(|l| (l.id, l.name.clone(), l.visible, l.locked, l.kind))
            .collect();

        for (id, name, visible, locked, kind) in rows {
            ui.horizontal(|ui| {
                let mut v = visible;
                if ui.checkbox(&mut v, "").on_hover_text("Visible").changed() {
                    self.editor.set_visible(id, v);
                }
                if ui.selectable_label(locked, "L").on_hover_text("Lock (no edits)").clicked() {
                    self.editor.set_locked(id, !locked);
                }
                if ui.selectable_label(solo == Some(id), "S").on_hover_text("Solo (show only this layer)").clicked() {
                    self.editor.set_solo(if solo == Some(id) { None } else { Some(id) });
                }
                let label = match kind {
                    LayerKind::Painted => name,
                    LayerKind::Generator => format!("{name} (generator)"),
                };
                if ui.selectable_label(id == active, label).clicked() {
                    self.editor.layers.set_active(id);
                }
            });
        }

        // Rename the active layer.
        if let Some(layer) = self.editor.layers.layer(active) {
            let mut name = layer.name.clone();
            if ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY)).changed() {
                self.editor.rename_layer(active, &name);
            }
        }

        ui.horizontal_wrapped(|ui| {
            if ui.button("New").on_hover_text("Empty layer above the active one").clicked() {
                let n = self.editor.layers.layers().len() + 1;
                self.editor.add_layer(&format!("Layer {n}"), LayerKind::Painted);
            }
            if ui.button("Delete").clicked() && !self.editor.delete_layer(active) {
                self.file_msg = Some(Err("The last layer cannot be deleted".into()));
            }
            if ui.button("Up").clicked() {
                self.editor.move_layer(active, true);
            }
            if ui.button("Down").clicked() {
                self.editor.move_layer(active, false);
            }
            if ui.button("Merge down").on_hover_text("Merge into the layer below").clicked() {
                if let Err(e) = self.editor.merge_down(active) {
                    self.file_msg = Some(Err(e.into()));
                }
            }
            let generator = self.editor.layers.layer(active).is_some_and(|l| l.kind == LayerKind::Generator);
            if generator
                && ui
                    .button("Bake")
                    .on_hover_text("Turn the generated blocks into an ordinary editable layer")
                    .clicked()
            {
                self.editor.bake_layer(active);
            }
        });
        ui.small("Top layer wins. Tools edit the active layer. Visibility, lock and names are not undoable.");
    }
}
