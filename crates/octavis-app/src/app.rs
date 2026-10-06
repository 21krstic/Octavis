use std::sync::Mutex;

use eframe::egui::{self, Key, PointerButton};
use eframe::egui_wgpu;
use glam::{IVec3, Vec2, Vec3};
use octavis_core::{BlockId, Connectivity, RayHit, Selection, World, raycast};
use octavis_mesh::Mesh;

use crate::camera::Camera;
use crate::editor::{Editor, MAX_SELECTION_CELLS, MAX_SPHERE_RADIUS, SelectMode};
use crate::picking::{ray_layer_cell, ray_plane_point};
use crate::render::{self, ViewportCallback};
use crate::{gizmo, scene};

const MAX_PICK_DISTANCE: f32 = 1000.0;
/// Stops a wand click on a huge connected mass from running away.
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
    /// Locked to one plane so a tap places one block and strokes paint flat.
    Stroke { last: Option<IVec3>, axis: usize, layer: i32 },
    Box { anchor: IVec3 },
    Sphere { center: IVec3 },
}

type Ray = (Vec3, Vec3);

pub struct OctavisApp {
    editor: Editor,
    camera: Camera,
    tool: Tool,
    block: BlockId,
    sel_mode: SelectMode,
    wand_diagonal: bool,
    show_axes: bool,
    drag: Option<Drag>,
    /// Selection preview while a box/sphere drag is in progress.
    preview: Option<Selection>,
    notice: Option<&'static str>,
    palette_dirty: bool,
    overlay_dirty: bool,
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
            wand_diagonal: true,
            show_axes: true,
            drag: None,
            preview: None,
            notice: None,
            palette_dirty: true,
            overlay_dirty: true,
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

    /// Meshes sets of cells as flat overlay faces, each layer with its marker id.
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

    fn effective_mode(&self, ctrl: bool, shift: bool) -> SelectMode {
        if ctrl {
            SelectMode::Subtract
        } else if shift {
            SelectMode::Add
        } else {
            self.sel_mode
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
        // Don't rewind history underneath an in-progress press.
        if self.drag.is_some() {
            return;
        }
        if undo {
            self.editor.undo();
        }
        if redo {
            self.editor.redo();
        }
        if deselect {
            self.editor.clear_selection();
        }
    }

    fn tool_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Tools");
        ui.selectable_value(&mut self.tool, Tool::Pencil, "Pencil (place / Ctrl: break)");
        ui.selectable_value(&mut self.tool, Tool::SelectBox, "Select box");
        ui.selectable_value(&mut self.tool, Tool::SelectSphere, "Select sphere");
        ui.selectable_value(&mut self.tool, Tool::Wand, "Wand (same block)");
        if self.tool == Tool::Wand {
            ui.checkbox(&mut self.wand_diagonal, "Include diagonals");
        }

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
                self.editor.clear_selection();
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

        ui.separator();
        ui.checkbox(&mut self.show_axes, "Show axes");
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
            if let Some(p) = &self.preview {
                ui.separator();
                ui.label(format!("Preview: {} blocks", p.len()));
            }
            if let Some(n) = self.notice {
                ui.separator();
                ui.colored_label(egui::Color32::from_rgb(255, 170, 60), n);
            }
        });
    }

    /// Handles input in the viewport; returns its rect and the cell to highlight.
    fn viewport(&mut self, ui: &mut egui::Ui) -> (egui::Rect, Option<IVec3>) {
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());

        // Navigation: right-drag orbits, middle-drag (or shift+right) pans, wheel zooms.
        let delta = response.drag_delta();
        let (shift, ctrl) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
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

        // Ray under the cursor. While a press is held it keeps working even
        // if the cursor leaves the viewport.
        let ray: Option<Ray> = ui.input(|i| i.pointer.hover_pos()).and_then(|p| {
            if !(rect.contains(p) || self.drag.is_some()) {
                return None;
            }
            let rel = p - rect.min;
            Some(self.camera.ray(Vec2::new(rel.x, rel.y), Vec2::new(rect.width(), rect.height())))
        });
        let hit = ray.and_then(|(o, d)| raycast(&self.editor.world, o, d, MAX_PICK_DISTANCE));

        let target = match &self.drag {
            Some(Drag::Stroke { axis, layer, .. }) => {
                ray.and_then(|(o, d)| ray_layer_cell(o, d, *axis, *layer, MAX_PICK_DISTANCE))
            }
            // Box corners prefer a hit block, else the horizontal plane
            // through the anchor so the corner can be placed in open air.
            Some(Drag::Box { anchor }) => hit.map(|h| h.pos).or_else(|| {
                ray.and_then(|(o, d)| ray_layer_cell(o, d, 1, anchor.y, MAX_PICK_DISTANCE))
            }),
            Some(Drag::Sphere { .. }) => None,
            None => hit.and_then(|h| self.hover_cell(&h, ctrl)),
        };

        let (pressed, down, released) = ui.input(|i| {
            (
                i.pointer.button_pressed(PointerButton::Primary),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
            )
        });
        self.notice = None;
        if pressed && response.hovered() && self.drag.is_none() {
            if let Some(h) = hit {
                self.begin_press(&h, ctrl, shift);
            }
        }
        if down {
            self.continue_press(ray, target, ctrl);
        }
        if released {
            self.end_press(ctrl, shift);
        }

        (rect, target)
    }

    /// The cell a tool would act on, for hover highlighting before a press.
    fn hover_cell(&self, hit: &RayHit, remove: bool) -> Option<IVec3> {
        match self.tool {
            Tool::Pencil if remove => Some(hit.pos),
            Tool::Pencil => (hit.normal != IVec3::ZERO).then_some(hit.pos + hit.normal),
            _ => Some(hit.pos),
        }
    }

    fn begin_press(&mut self, hit: &RayHit, ctrl: bool, shift: bool) {
        match self.tool {
            Tool::Pencil => {
                let Some(target) = self.hover_cell(hit, ctrl) else { return };
                // Lock to the plane of the face that was clicked.
                let axis = (0..3).find(|&a| hit.normal[a] != 0).unwrap_or(1);
                self.editor.begin_edit();
                self.editor.set_block(target, if ctrl { BlockId::AIR } else { self.block });
                self.drag = Some(Drag::Stroke { last: Some(target), axis, layer: target[axis] });
            }
            Tool::SelectBox => self.drag = Some(Drag::Box { anchor: hit.pos }),
            Tool::SelectSphere => self.drag = Some(Drag::Sphere { center: hit.pos }),
            Tool::Wand => {
                let connectivity =
                    if self.wand_diagonal { Connectivity::Full } else { Connectivity::Face };
                let sel = Selection::wand(&self.editor.world, hit.pos, WAND_LIMIT, connectivity);
                let mode = self.effective_mode(ctrl, shift);
                self.editor.apply_selection(mode, &sel);
            }
        }
    }

    fn continue_press(&mut self, ray: Option<Ray>, target: Option<IVec3>, ctrl: bool) {
        let block = if ctrl { BlockId::AIR } else { self.block };
        match &mut self.drag {
            Some(Drag::Stroke { last, .. }) => {
                if let Some(t) = target {
                    if *last != Some(t) {
                        *last = Some(t);
                        self.editor.set_block(t, block);
                    }
                }
            }
            Some(Drag::Box { anchor }) => {
                let Some(corner) = target else { return };
                let extent = (*anchor - corner).abs() + IVec3::ONE;
                let volume = extent.x as i64 * extent.y as i64 * extent.z as i64;
                self.preview = if volume > MAX_SELECTION_CELLS {
                    self.notice = Some("Box too large (limit 500,000 blocks)");
                    Some(Selection::new())
                } else {
                    Some(Selection::cuboid(*anchor, corner))
                };
                self.overlay_dirty = true;
            }
            Some(Drag::Sphere { center }) => {
                let Some((origin, dir)) = ray else { return };
                // The sphere's surface follows the cursor on a camera-facing
                // plane through its centre, wherever the cursor is.
                let c = center.as_vec3() + Vec3::splat(0.5);
                let Some(p) = ray_plane_point(origin, dir, c, self.camera.forward(), MAX_PICK_DISTANCE)
                else {
                    return;
                };
                let mut radius = (p - c).length();
                if radius > MAX_SPHERE_RADIUS {
                    radius = MAX_SPHERE_RADIUS;
                    self.notice = Some("Sphere at maximum radius (49)");
                }
                self.preview = Some(Selection::sphere(*center, radius));
                self.overlay_dirty = true;
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
                self.overlay_dirty = true;
            }
            None => {}
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
            if self.editor.take_selection_dirty() | std::mem::take(&mut self.overlay_dirty) {
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
            let view_proj = self.camera.view_proj(aspect);
            ui.painter().add(egui_wgpu::Callback::new_paint_callback(
                rect,
                ViewportCallback {
                    view_proj,
                    palette: Mutex::new(palette),
                    uploads: Mutex::new(uploads),
                    overlay_uploads: Mutex::new(overlay_uploads),
                },
            ));

            if self.show_axes {
                let painter = ui.painter_at(rect);
                gizmo::draw_world_axes(&painter, rect, &view_proj);
                gizmo::draw_corner_gizmo(&painter, rect, &self.camera.view());
            }
        });
    }
}
