use std::sync::Mutex;

use eframe::egui::{self, Key, PointerButton};
use eframe::egui_wgpu;
use glam::{IVec3, Vec2, Vec3};
use octavis_core::{BlockId, RayHit, Selection, World, raycast};
use octavis_mesh::Mesh;

use crate::camera::Camera;
use crate::editor::{Editor, SelectMode};
use crate::render::{self, ViewportCallback};
use crate::scene;

const MAX_PICK_DISTANCE: f32 = 1000.0;
/// Stops a wand click in open air from flooding forever.
const WAND_LIMIT: usize = 200_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Pencil,
    SelectBox,
    SelectSphere,
    Wand,
}

/// What the current left-button press is doing.
enum Drag {
    Stroke { last: Option<IVec3> },
    Box { anchor: IVec3 },
    Sphere { center: IVec3 },
}

pub struct OctavisApp {
    editor: Editor,
    camera: Camera,
    tool: Tool,
    block: BlockId,
    sel_mode: SelectMode,
    drag: Option<Drag>,
    /// Selection preview while a box/sphere drag is in progress.
    preview: Option<Selection>,
    palette_dirty: bool,
    selection_overlay_dirty: bool,
    /// Cell under the cursor last frame, for the status bar.
    hover: Option<IVec3>,
}

impl OctavisApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let render_state = cc.wgpu_render_state.as_ref().expect("wgpu backend required");
        render::init(render_state);

        let world = scene::demo_world();
        let block = world.blocks.len().checked_sub(2).map_or(BlockId(1), |i| BlockId(i as u32));
        Self {
            editor: Editor::new(world),
            camera: Camera::new(Vec3::new(0.0, 4.0, 0.0)),
            tool: Tool::Pencil,
            block,
            sel_mode: SelectMode::Replace,
            drag: None,
            preview: None,
            palette_dirty: true,
            selection_overlay_dirty: true,
            hover: None,
        }
    }

    fn mesh_uploads(&mut self) -> Vec<(IVec3, Option<Mesh>)> {
        let dirty = self.editor.take_dirty();
        let world = &self.editor.world;
        dirty
            .into_iter()
            .map(|p| (p, world.section(p).map(|_| octavis_mesh::mesh_section(world, p))))
            .collect()
    }

    /// Meshes a set of cells as flat overlay faces with a marker id.
    fn overlay_mesh(layers: &[(&[IVec3], u32)]) -> Mesh {
        let mut w = World::new();
        for (cells, marker) in layers {
            for &c in *cells {
                w.set(c, BlockId(*marker));
            }
        }
        let mut mesh = Mesh::default();
        for (p, _) in w.sections() {
            let m = octavis_mesh::mesh_section(&w, p);
            let base = mesh.vertices.len() as u32;
            mesh.vertices.extend(m.vertices);
            mesh.indices.extend(m.indices.iter().map(|i| base + i));
        }
        mesh
    }

    /// The cell a tool acts on for a given hit.
    fn target_cell(&self, hit: &RayHit, remove: bool) -> Option<IVec3> {
        match self.tool {
            Tool::Pencil if remove => Some(hit.pos),
            Tool::Pencil => (hit.normal != IVec3::ZERO).then_some(hit.pos + hit.normal),
            _ => Some(hit.pos),
        }
    }

    fn shape_selection(drag: &Drag, current: IVec3) -> Selection {
        match *drag {
            Drag::Box { anchor } => Selection::cuboid(anchor, current),
            Drag::Sphere { center } => Selection::sphere(center, (current - center).as_vec3().length()),
            Drag::Stroke { .. } => Selection::new(),
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let (undo, redo, deselect) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            (
                cmd && !i.modifiers.shift && i.key_pressed(Key::Z),
                cmd && (i.key_pressed(Key::Y) || (i.modifiers.shift && i.key_pressed(Key::Z))),
                i.key_pressed(Key::Escape),
            )
        });
        if undo {
            self.editor.undo();
        }
        if redo {
            self.editor.redo();
        }
        if deselect && !self.editor.selection.is_empty() {
            self.editor.selection.clear();
            self.selection_overlay_dirty = true;
        }
        if undo || redo {
            self.selection_overlay_dirty = true;
        }
    }

    fn tool_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Tools");
        ui.selectable_value(&mut self.tool, Tool::Pencil, "Pencil (place / Ctrl: break)");
        ui.selectable_value(&mut self.tool, Tool::SelectBox, "Select box");
        ui.selectable_value(&mut self.tool, Tool::SelectSphere, "Select sphere");
        ui.selectable_value(&mut self.tool, Tool::Wand, "Wand (same block)");

        ui.separator();
        ui.label("Selection mode (Shift = add, Ctrl = subtract)");
        for (mode, label) in [
            (SelectMode::Replace, "Replace"),
            (SelectMode::Add, "Add"),
            (SelectMode::Subtract, "Subtract"),
            (SelectMode::Intersect, "Intersect"),
        ] {
            ui.selectable_value(&mut self.sel_mode, mode, label);
        }

        ui.separator();
        ui.heading("Block");
        for i in 1..self.editor.world.blocks.len() {
            let id = BlockId(i as u32);
            let name = &self.editor.world.blocks.get(id).unwrap().name;
            ui.selectable_value(&mut self.block, id, name.trim_start_matches("minecraft:"));
        }

        ui.separator();
        ui.heading("Selection");
        let has_sel = !self.editor.selection.is_empty();
        ui.add_enabled_ui(has_sel, |ui| {
            if ui.button("Fill with block").clicked() {
                self.editor.fill_selection(self.block);
            }
            if ui.button("Delete blocks").clicked() {
                self.editor.fill_selection(BlockId::AIR);
            }
            if ui.button("Deselect (Esc)").clicked() {
                self.editor.selection.clear();
                self.selection_overlay_dirty = true;
            }
        });

        ui.separator();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(self.editor.history.can_undo(), |ui| {
                if ui.button("Undo").clicked() {
                    self.editor.undo();
                }
            });
            ui.add_enabled_ui(self.editor.history.can_redo(), |ui| {
                if ui.button("Redo").clicked() {
                    self.editor.redo();
                }
            });
        });
        ui.small("Ctrl+Z / Ctrl+Y");
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            match self.hover {
                Some(p) => ui.label(format!("Cell: {}, {}, {}", p.x, p.y, p.z)),
                None => ui.label("Cell: -"),
            };
            ui.separator();
            let sel = &self.editor.selection;
            match sel.bounds() {
                Some((lo, hi)) => {
                    let size = hi - lo + IVec3::ONE;
                    ui.label(format!("Selected: {} blocks ({}x{}x{})", sel.len(), size.x, size.y, size.z))
                }
                None => ui.label("Selected: none"),
            };
        });
    }

    /// Handles input in the viewport; returns its rect and the cell under the cursor.
    fn viewport(&mut self, ui: &mut egui::Ui) -> (egui::Rect, Option<IVec3>) {
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());

        // Navigation: right-drag orbits, middle-drag (or shift+right) pans, wheel zooms.
        let delta = response.drag_delta();
        let shift = ui.input(|i| i.modifiers.shift);
        let ctrl = ui.input(|i| i.modifiers.command);
        if response.dragged_by(PointerButton::Middle)
            || (shift && response.dragged_by(PointerButton::Secondary))
        {
            self.camera.pan(delta.x, delta.y);
        } else if response.dragged_by(PointerButton::Secondary) {
            self.camera.orbit(delta.x, delta.y);
        }
        if response.hovered() {
            self.camera.zoom(ui.input(|i| i.smooth_scroll_delta.y));
        }

        // Pick under the cursor. Keep picking while a drag is held even if
        // the cursor strays outside the viewport.
        let hit = ui.input(|i| i.pointer.hover_pos()).and_then(|p| {
            let rel = p - rect.min;
            let size = rect.size();
            if !(rect.contains(p) || self.drag.is_some()) {
                return None;
            }
            let (origin, dir) = self.camera.ray(Vec2::new(rel.x, rel.y), Vec2::new(size.x, size.y));
            raycast(&self.editor.world, origin, dir, MAX_PICK_DISTANCE)
        });
        let target = hit.as_ref().and_then(|h| self.target_cell(h, ctrl));

        let (pressed, down, released) = ui.input(|i| {
            (i.pointer.button_pressed(PointerButton::Primary), i.pointer.primary_down(), i.pointer.primary_released())
        });

        if pressed && response.hovered() && self.drag.is_none() {
            if let (Some(h), Some(t)) = (hit.as_ref(), target) {
                self.begin_press(h, t, ctrl, shift);
            }
        }
        if down {
            if let Some(t) = target {
                self.continue_press(t, ctrl);
            }
        }
        if released {
            self.end_press(ctrl, shift);
        }

        (rect, target)
    }

    fn begin_press(&mut self, hit: &RayHit, target: IVec3, ctrl: bool, shift: bool) {
        match self.tool {
            Tool::Pencil => {
                self.editor.begin_edit();
                let block = if ctrl { BlockId::AIR } else { self.block };
                self.editor.set_block(target, block);
                self.drag = Some(Drag::Stroke { last: Some(target) });
            }
            Tool::SelectBox => self.drag = Some(Drag::Box { anchor: hit.pos }),
            Tool::SelectSphere => self.drag = Some(Drag::Sphere { center: hit.pos }),
            Tool::Wand => {
                let sel = Selection::wand(&self.editor.world, hit.pos, WAND_LIMIT);
                let mode = self.effective_mode(ctrl, shift);
                self.editor.apply_selection(mode, &sel);
                self.selection_overlay_dirty = true;
            }
        }
    }

    fn continue_press(&mut self, target: IVec3, ctrl: bool) {
        let block = if ctrl { BlockId::AIR } else { self.block };
        match &mut self.drag {
            Some(Drag::Stroke { last }) => {
                if *last != Some(target) {
                    *last = Some(target);
                    self.editor.set_block(target, block);
                }
            }
            Some(d @ (Drag::Box { .. } | Drag::Sphere { .. })) => {
                self.preview = Some(Self::shape_selection(d, target));
                self.selection_overlay_dirty = true;
            }
            None => {}
        }
    }

    fn end_press(&mut self, ctrl: bool, shift: bool) {
        match self.drag.take() {
            Some(Drag::Stroke { .. }) => self.editor.end_edit(),
            Some(Drag::Box { .. } | Drag::Sphere { .. }) => {
                if let Some(p) = self.preview.take() {
                    let mode = self.effective_mode(ctrl, shift);
                    self.editor.apply_selection(mode, &p);
                }
                self.selection_overlay_dirty = true;
            }
            None => {}
        }
    }

    fn effective_mode(&self, ctrl: bool, shift: bool) -> SelectMode {
        if ctrl {
            SelectMode::Subtract
        } else if shift {
            SelectMode::Add
        } else {
            self.sel_mode
        }
    }
}

impl eframe::App for OctavisApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_shortcuts(ui.ctx());

        egui::Panel::left("tools").resizable(false).exact_size(210.0).show(ui, |ui| {
            self.tool_panel(ui);
        });

        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));

        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let (rect, hover) = self.viewport(ui);
            self.hover = hover;

            let uploads = self.mesh_uploads();
            let palette = std::mem::take(&mut self.palette_dirty)
                .then(|| render::palette_colors(&self.editor.world.blocks));

            let mut overlay_uploads = Vec::new();
            if std::mem::take(&mut self.selection_overlay_dirty) {
                let sel: Vec<IVec3> = self.editor.selection.iter().collect();
                let prev: Vec<IVec3> =
                    self.preview.as_ref().map(|p| p.iter().collect()).unwrap_or_default();
                overlay_uploads.push((
                    render::SLOT_SELECTION,
                    Self::overlay_mesh(&[
                        (&sel, render::OVERLAY_SELECTION),
                        (&prev, render::OVERLAY_PREVIEW),
                    ]),
                ));
            }
            let hover_cells: Vec<IVec3> = hover.into_iter().collect();
            overlay_uploads
                .push((render::SLOT_HOVER, Self::overlay_mesh(&[(&hover_cells, render::OVERLAY_HOVER)])));

            let aspect = rect.width() / rect.height().max(1.0);
            ui.painter().add(egui_wgpu::Callback::new_paint_callback(
                rect,
                ViewportCallback {
                    view_proj: self.camera.view_proj(aspect),
                    palette: Mutex::new(palette),
                    uploads: Mutex::new(uploads),
                    overlay_uploads: Mutex::new(overlay_uploads),
                },
            ));
        });
    }
}
