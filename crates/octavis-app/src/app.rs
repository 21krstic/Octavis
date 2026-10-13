use std::sync::Mutex;

use eframe::egui::{self, Key, PointerButton};
use eframe::egui_wgpu;
use glam::{IVec3, Vec2, Vec3};
use octavis_core::{
    BlockId, Brush, BrushMode, Connectivity, Mask, Pattern, RayHit, Selection, Shape, World,
    raycast, raycast_with,
};
use octavis_mesh::Mesh;

use crate::camera::Camera;
use crate::editor::{Editor, MAX_SELECTION_CELLS, MAX_SPHERE_RADIUS, SelectMode};
use crate::picking::{ray_layer_cell, ray_plane_point, trace_targets};
use crate::render::{self, ViewportCallback};
use crate::{gizmo, scene};

mod files;

const MAX_PICK_DISTANCE: f32 = 1000.0;
/// Stops a wand click on a huge connected mass from running away.
const WAND_LIMIT: usize = 200_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Pencil,
    Brush,
    SelectBox,
    SelectSphere,
    Wand,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShapeKind {
    Sphere,
    Cube,
    Cylinder,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModeKind {
    Paint,
    Splatter,
    Overlay,
    Smooth,
    Erode,
    Dilate,
    Erase,
}

impl ModeKind {
    const ALL: [ModeKind; 7] = [
        ModeKind::Paint,
        ModeKind::Splatter,
        ModeKind::Overlay,
        ModeKind::Smooth,
        ModeKind::Erode,
        ModeKind::Dilate,
        ModeKind::Erase,
    ];

    fn label(self) -> &'static str {
        match self {
            ModeKind::Paint => "Paint / replace",
            ModeKind::Splatter => "Splatter",
            ModeKind::Overlay => "Overlay (on top)",
            ModeKind::Smooth => "Smooth",
            ModeKind::Erode => "Erode",
            ModeKind::Dilate => "Dilate",
            ModeKind::Erase => "Erase",
        }
    }
}

/// Brush options as edited in the panel. The mask and pattern stay text
/// until a stroke starts, so half-typed names never reach the block table.
struct BrushSettings {
    shape: ShapeKind,
    radius: f32,
    half_height: i32,
    mode: ModeKind,
    density: f32,
    /// Passes for smooth / erode / dilate.
    iterations: u32,
    mask_text: String,
    /// Empty means "the block selected in the Block list".
    pattern_text: String,
}

impl BrushSettings {
    fn new() -> Self {
        Self {
            shape: ShapeKind::Sphere,
            radius: 3.0,
            half_height: 3,
            mode: ModeKind::Paint,
            density: 0.3,
            iterations: 1,
            mask_text: String::new(),
            pattern_text: String::new(),
        }
    }

    fn shape(&self) -> Shape {
        match self.shape {
            ShapeKind::Sphere => Shape::Sphere { radius: self.radius },
            ShapeKind::Cube => Shape::Cube { half: self.radius.round() as i32 },
            ShapeKind::Cylinder => {
                Shape::Cylinder { radius: self.radius, half_height: self.half_height }
            }
        }
    }

    /// Distance between dabs along a stroke: about half the brush size.
    fn dab_spacing(&self) -> f32 {
        (self.radius * 0.5).round().max(1.0)
    }
}

/// How a pencil stroke chooses cells while the button is held.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StrokeMode {
    /// Follows whatever surface is under the cursor each frame.
    Free,
    /// Locked to the plane of the face first clicked: a tap places exactly
    /// one block and strokes paint flat, but feel less direct than Free.
    PlaneLocked,
}

/// What the current left-button press is doing.
enum Drag {
    /// `plane` is `(axis, layer)` when the stroke is plane-locked.
    Stroke { last: Option<IVec3>, plane: Option<(usize, i32)> },
    Box { anchor: IVec3 },
    Sphere { center: IVec3 },
    /// `last` is the centre of the previous dab.
    Brush { last: IVec3, brush: Box<Brush>, spacing: f32 },
}

type Ray = (Vec3, Vec3);

pub struct OctavisApp {
    editor: Editor,
    camera: Camera,
    tool: Tool,
    stroke_mode: StrokeMode,
    brush: BrushSettings,
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
    /// Cells last sent as the hover overlay, to skip identical re-uploads.
    sent_hover: Vec<IVec3>,
    block_filter: String,
    /// Viewport-relative cursor position at the end of the last stroke
    /// frame; the next frame's path starts here.
    last_cursor: Option<Vec2>,
    /// Outcome of the last file operation, shown in the status bar.
    file_msg: Option<Result<String, String>>,
    /// Bumped per stroke so random patterns and splatter differ between
    /// strokes but stay fixed within one.
    stroke_seed: u32,
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
            stroke_mode: StrokeMode::Free,
            brush: BrushSettings::new(),
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
            sent_hover: Vec::new(),
            block_filter: String::new(),
            last_cursor: None,
            file_msg: None,
            stroke_seed: 0,
        }
    }

    /// Builds the brush for a stroke from the panel text. `erase` (Ctrl)
    /// swaps the mode for Erase and ignores the pattern.
    fn build_brush(&mut self, erase: bool) -> Result<Brush, String> {
        let mask = Mask::parse(&self.brush.mask_text)?;
        let (mode, pattern) = if erase || self.brush.mode == ModeKind::Erase {
            (BrushMode::Erase, Pattern::Single(BlockId::AIR))
        } else {
            let pattern = if self.brush.pattern_text.trim().is_empty() {
                Pattern::Single(self.block)
            } else {
                let before = self.editor.world.blocks.len();
                let p = Pattern::parse(&self.brush.pattern_text, &mut self.editor.world.blocks)?;
                self.palette_dirty |= self.editor.world.blocks.len() != before;
                p
            };
            let mode = match self.brush.mode {
                ModeKind::Splatter => BrushMode::Splatter { density: self.brush.density },
                ModeKind::Overlay => BrushMode::Overlay,
                ModeKind::Smooth => BrushMode::Smooth { iterations: self.brush.iterations },
                ModeKind::Erode => BrushMode::Erode { iterations: self.brush.iterations },
                ModeKind::Dilate => BrushMode::Dilate { iterations: self.brush.iterations },
                ModeKind::Paint | ModeKind::Erase => BrushMode::Paint,
            };
            (mode, pattern)
        };
        self.stroke_seed = self.stroke_seed.wrapping_add(1);
        Ok(Brush { shape: self.brush.shape(), mask, pattern, mode, seed: self.stroke_seed })
    }

    fn brush_panel(&mut self, ui: &mut egui::Ui) {
        let b = &mut self.brush;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut b.shape, ShapeKind::Sphere, "Sphere");
            ui.selectable_value(&mut b.shape, ShapeKind::Cube, "Cube");
            ui.selectable_value(&mut b.shape, ShapeKind::Cylinder, "Cylinder");
        });
        ui.add(egui::Slider::new(&mut b.radius, 0.0..=16.0).step_by(0.5).text("Radius"));
        if b.shape == ShapeKind::Cylinder {
            ui.add(egui::Slider::new(&mut b.half_height, 0..=16).text("Half height"));
        }
        egui::ComboBox::from_label("Mode").selected_text(b.mode.label()).show_ui(ui, |ui| {
            for m in ModeKind::ALL {
                ui.selectable_value(&mut b.mode, m, m.label());
            }
        });
        if b.mode == ModeKind::Splatter {
            ui.add(egui::Slider::new(&mut b.density, 0.05..=1.0).text("Density"));
        }
        if matches!(b.mode, ModeKind::Smooth | ModeKind::Erode | ModeKind::Dilate) {
            ui.add(egui::Slider::new(&mut b.iterations, 1..=8).text("Passes"));
        }

        ui.label("Mask (empty = anywhere)");
        ui.add(
            egui::TextEdit::singleline(&mut b.mask_text)
                .hint_text("e.g. stone | dirt & y<10")
                .desired_width(f32::INFINITY),
        );
        match Mask::parse(&b.mask_text) {
            Err(e) => {
                ui.colored_label(egui::Color32::from_rgb(255, 110, 90), e);
            }
            Ok(m) => {
                if m.uses_selection() && self.editor.selection.is_empty() {
                    ui.colored_label(
                        egui::Color32::from_rgb(255, 170, 60),
                        "Nothing is selected, so this mask matches nothing",
                    );
                }
                let unknown = m.unknown_names(&self.editor.world.blocks);
                if !unknown.is_empty() {
                    ui.colored_label(
                        egui::Color32::from_rgb(255, 170, 60),
                        format!("Not in the scene yet: {}", unknown.join(", ")),
                    );
                }
            }
        }
        ui.small("air solid exposed surface selected y>=5 <block>  ! & | ( )");

        ui.label("Pattern (empty = selected block)");
        ui.add(
            egui::TextEdit::singleline(&mut b.pattern_text)
                .hint_text("e.g. 70%stone, 30%dirt")
                .desired_width(f32::INFINITY),
        );
        if !b.pattern_text.trim().is_empty() {
            match Pattern::validate(&b.pattern_text) {
                Err(e) => {
                    ui.colored_label(egui::Color32::from_rgb(255, 110, 90), e);
                }
                Ok(()) => {
                    let new = Pattern::unknown_names(&b.pattern_text, &self.editor.world.blocks);
                    if !new.is_empty() {
                        ui.colored_label(
                            egui::Color32::from_rgb(255, 170, 60),
                            format!("Creates new block type: {}", new.join(", ")),
                        );
                    }
                }
            }
        }
        ui.small("Ctrl = erase");

        ui.separator();
        let has_selection = !self.editor.selection.is_empty();
        let apply = ui
            .add_enabled(has_selection, egui::Button::new("Apply to selection"))
            .on_hover_text("Runs this mode, mask and pattern over every selected block (one undo step)")
            .clicked();
        if apply {
            match self.build_brush(false) {
                Ok(brush) => {
                    self.editor.apply_brush_to_selection(&brush);
                }
                Err(_) => self.notice = Some("Fix the brush mask or pattern first"),
            }
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
        ui.selectable_value(&mut self.tool, Tool::Brush, "Brush");
        ui.selectable_value(&mut self.tool, Tool::SelectBox, "Select box");
        ui.selectable_value(&mut self.tool, Tool::SelectSphere, "Select sphere");
        ui.selectable_value(&mut self.tool, Tool::Wand, "Wand (same block)");
        if self.tool == Tool::Pencil {
            ui.label("Stroke mode");
            ui.selectable_value(&mut self.stroke_mode, StrokeMode::Free, "Free");
            ui.selectable_value(&mut self.stroke_mode, StrokeMode::PlaneLocked, "Plane-locked");
        }
        if self.tool == Tool::Brush {
            ui.separator();
            self.brush_panel(ui);
        }
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
        ui.add(
            egui::TextEdit::singleline(&mut self.block_filter)
                .hint_text("filter blocks")
                .desired_width(f32::INFINITY),
        );
        let filter = self.block_filter.trim().to_ascii_lowercase();
        egui::ScrollArea::vertical().id_salt("blocks").max_height(220.0).show(ui, |ui| {
            for i in 1..self.editor.world.blocks.len() {
                let id = BlockId(i as u32);
                let name = &self.editor.world.blocks.get(id).unwrap().name;
                let short = name.trim_start_matches("minecraft:");
                if filter.is_empty() || short.contains(&filter) {
                    ui.selectable_value(&mut self.block, id, short);
                }
            }
        });

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
            match &self.file_msg {
                Some(Ok(m)) => {
                    ui.separator();
                    ui.colored_label(egui::Color32::from_rgb(120, 200, 120), m);
                }
                Some(Err(m)) => {
                    ui.separator();
                    ui.colored_label(egui::Color32::from_rgb(255, 110, 90), m);
                }
                None => {}
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
        let target = self.target_at(ray, ctrl);

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
                self.last_cursor = None;
            }
        }
        if down {
            if matches!(self.drag, Some(Drag::Stroke { .. } | Drag::Brush { .. })) {
                self.follow_cursor(ui, rect, ctrl);
            } else {
                self.continue_press(ray, target);
            }
        }
        if released {
            self.end_press(ctrl, shift);
            self.last_cursor = None;
        }

        (rect, target)
    }

    /// The cell the active tool aims at for a ray, given the current drag.
    fn target_at(&self, ray: Option<Ray>, ctrl: bool) -> Option<IVec3> {
        let hit = || ray.and_then(|(o, d)| raycast(&self.editor.world, o, d, MAX_PICK_DISTANCE));
        match &self.drag {
            Some(Drag::Stroke { plane: Some((axis, layer)), .. }) => {
                ray.and_then(|(o, d)| ray_layer_cell(o, d, *axis, *layer, MAX_PICK_DISTANCE))
            }
            // Box corners prefer a hit block, else the horizontal plane
            // through the anchor so the corner can be placed in open air.
            Some(Drag::Box { anchor }) => hit().map(|h| h.pos).or_else(|| {
                ray.and_then(|(o, d)| ray_layer_cell(o, d, 1, anchor.y, MAX_PICK_DISTANCE))
            }),
            Some(Drag::Sphere { .. }) => None,
            // Pick against the world as it was when the stroke began, so the
            // stroke's own changes never move its target (no digging
            // through the floor or building toward the camera).
            Some(Drag::Brush { .. }) => ray
                .and_then(|(o, d)| {
                    raycast_with(|p| self.editor.was_solid(p), o, d, MAX_PICK_DISTANCE)
                })
                .map(|h| h.pos),
            Some(Drag::Stroke { plane: None, .. }) | None => {
                hit().and_then(|h| self.hover_cell(&h, ctrl))
            }
        }
    }

    /// Advances a pencil or brush stroke along everything the cursor did
    /// since the last frame. Every mouse-move event is used, and the path
    /// between them is bisected until its targets leave no gaps (see
    /// [`trace_targets`]), so the result does not depend on frame rate.
    fn follow_cursor(&mut self, ui: &egui::Ui, rect: egui::Rect, ctrl: bool) {
        let size = Vec2::new(rect.width(), rect.height());
        let rel = |p: egui::Pos2| Vec2::new(p.x - rect.min.x, p.y - rect.min.y);
        let mut path: Vec<Vec2> = self.last_cursor.into_iter().collect();
        ui.input(|i| {
            for e in &i.events {
                if let egui::Event::PointerMoved(p) = e {
                    path.push(rel(*p));
                }
            }
            if let Some(p) = i.pointer.hover_pos() {
                if path.last() != Some(&rel(p)) {
                    path.push(rel(p));
                }
            }
        });

        // Neighbouring targets must be within one cell for a pencil, and
        // within a dab spacing for a brush.
        let max_gap = match &self.drag {
            Some(Drag::Brush { spacing, .. }) => (*spacing as i32).max(1),
            _ => 1,
        };
        let sample = |p: Vec2| self.target_at(Some(self.camera.ray(p, size)), ctrl);
        let mut targets = Vec::new();
        let mut prev: Option<(Vec2, Option<IVec3>)> = None;
        for &p in &path {
            let t = sample(p);
            match prev {
                Some((pp, pt)) => trace_targets(&sample, pp, pt, p, t, max_gap, &mut targets),
                // The first point is the previous frame's cursor, already applied.
                None if self.last_cursor.is_none() => targets.extend(t),
                None => {}
            }
            prev = Some((p, t));
        }
        self.last_cursor = path.last().copied().or(self.last_cursor);
        for t in targets {
            self.stroke_to(t, ctrl);
        }
    }

    /// Applies the pencil or brush at one target cell.
    fn stroke_to(&mut self, t: IVec3, ctrl: bool) {
        let block = if ctrl { BlockId::AIR } else { self.block };
        match &mut self.drag {
            Some(Drag::Stroke { last, .. }) => {
                if *last != Some(t) {
                    *last = Some(t);
                    self.editor.set_block(t, block);
                }
            }
            Some(Drag::Brush { last, brush, spacing }) => {
                if (t - *last).as_vec3().length() >= *spacing {
                    self.editor.apply_brush(brush, t);
                    *last = t;
                }
            }
            _ => {}
        }
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
                // Plane-locked strokes use the plane of the face that was clicked.
                let plane = (self.stroke_mode == StrokeMode::PlaneLocked).then(|| {
                    let axis = (0..3).find(|&a| hit.normal[a] != 0).unwrap_or(1);
                    (axis, target[axis])
                });
                self.editor.begin_edit();
                self.editor.set_block(target, if ctrl { BlockId::AIR } else { self.block });
                self.drag = Some(Drag::Stroke { last: Some(target), plane });
            }
            Tool::Brush => match self.build_brush(ctrl) {
                Ok(brush) => {
                    self.editor.begin_edit();
                    self.editor.apply_brush(&brush, hit.pos);
                    let spacing = self.brush.dab_spacing();
                    self.drag =
                        Some(Drag::Brush { last: hit.pos, brush: Box::new(brush), spacing });
                }
                Err(_) => self.notice = Some("Fix the brush mask or pattern first"),
            },
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

    fn continue_press(&mut self, ray: Option<Ray>, target: Option<IVec3>) {
        match &mut self.drag {
            // Strokes are advanced by `follow_cursor`.
            Some(Drag::Stroke { .. } | Drag::Brush { .. }) => {}
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
            Some(Drag::Stroke { .. } | Drag::Brush { .. }) => self.editor.end_edit(),
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
    /// Viewport background: a mid grey close to Blender's default.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.24, 0.24, 0.24, 1.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_shortcuts(ui.ctx());

        let mut action = None;
        egui::Panel::top("menu").show(ui, |ui| action = self.menu_bar(ui));
        if let Some(a) = action {
            if self.drag.is_none() {
                self.run_file_action(a);
            }
        }
        egui::Panel::left("tools").resizable(false).exact_size(210.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.tool_panel(ui));
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
            // The brush highlights its whole footprint; re-mesh only when it moves.
            let hover_cells: Vec<IVec3> = match (self.tool, hover) {
                (Tool::Brush, Some(h)) => self.brush.shape().cells(h),
                (_, h) => h.into_iter().collect(),
            };
            if hover_cells != self.sent_hover {
                overlay_uploads.push((
                    render::SLOT_HOVER,
                    Self::overlay_mesh(&[(&hover_cells, render::OVERLAY_HOVER)]),
                ));
                self.sent_hover = hover_cells;
            }

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
