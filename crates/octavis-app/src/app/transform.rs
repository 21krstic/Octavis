//! Transform tool: move, rotate, scale and flip the selected blocks of the
//! active layer, or the whole layer.
//!
//! Starting a transform lifts the source blocks into a buffer. While it is
//! pending the result is previewed in the layer without history (like the
//! terrain preview) and layers, tools and files are locked. Apply (Enter)
//! commits it as one undo step together with the moved selection; Cancel
//! (Esc) puts everything back.

use std::time::{Duration, Instant};

use eframe::egui::{self, Key};
use glam::{IVec3, Vec3};
use octavis_core::{BlockId, CellMap, LayerId, Selection, Transform};

use super::{MAX_PICK_DISTANCE, OctavisApp, Ray};
use crate::editor::{Backup, Editor, MAX_SELECTION_CELLS};
use crate::gizmo;
use crate::picking::ray_plane_point;

/// Minimum time between preview recomputations while values change.
const PREVIEW_INTERVAL: Duration = Duration::from_millis(100);
/// Largest box a resampled (scaled or freely rotated) result may span.
const MAX_RESULT_VOLUME: u64 = 16_000_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Selection,
    Layer,
}

/// A left-button drag moving the transform.
struct MoveDrag {
    /// The gizmo arrow being dragged, or `None` for the ground plane.
    axis: Option<usize>,
    start_offset: IVec3,
    /// Gizmo origin when the drag began.
    origin: Vec3,
    /// Where the drag began: on the plane, or (in `x`) along the axis.
    start: Vec3,
}

pub(super) struct TransformSession {
    layer: LayerId,
    source: Source,
    /// The lifted (non-air) blocks.
    cells: Vec<(IVec3, BlockId)>,
    /// The selection they came from (selection source only).
    selection: Option<Selection>,
    pivot: IVec3,
    transform: Transform,
    /// Keep the original blocks.
    copy: bool,
    uniform_scale: bool,
    /// What the preview in the layer currently shows.
    shown: Option<(Transform, bool)>,
    backup: Option<Backup>,
    last_update: Instant,
    /// The selection as it will be after applying, for the overlay.
    moved_selection: Option<Selection>,
    error: Option<String>,
    drag: Option<MoveDrag>,
    hovered_axis: Option<usize>,
}

impl TransformSession {
    /// The selection to draw while transforming, if it differs from the
    /// editor's.
    pub(super) fn shown_selection(&self) -> Option<&Selection> {
        self.moved_selection.as_ref()
    }

    fn gizmo_origin(&self) -> Vec3 {
        (self.pivot + self.transform.offset).as_vec3() + Vec3::splat(0.5)
    }
}

/// Cell changes that turn the layer (as it is without the preview) into the
/// transformed result, and the selection after it.
fn plan(s: &TransformSession, editor: &mut Editor) -> Result<(Vec<(IVec3, BlockId)>, Option<Selection>), String> {
    let t = s.transform;
    let dest = t.apply(&s.cells, s.pivot, &mut editor.world.blocks, MAX_RESULT_VOLUME)?;
    let layer = editor.layers.layer(s.layer).ok_or("The layer being transformed is gone")?;
    // Moving clears the source; air in the buffer is not part of it, so the
    // result never overwrites with air.
    let cleared: &[(IVec3, BlockId)] = if s.copy { &[] } else { &s.cells };
    let cells = merge_writes(cleared, &dest, |p| layer.cells.get(p));
    let moved = match &s.selection {
        Some(sel) => {
            let positions: Vec<(IVec3, ())> = sel.iter().map(|p| (p, ())).collect();
            let moved = t.map_cells(&positions, s.pivot, MAX_RESULT_VOLUME)?;
            // Too big to keep as a selection: it is dropped on apply.
            Some(if moved.len() as i64 > MAX_SELECTION_CELLS {
                Selection::new()
            } else {
                Selection::from_cells(moved.into_iter().map(|c| c.0))
            })
        }
        None => None,
    };
    Ok((cells, moved))
}

/// Largest box `merge_writes` indexes densely.
const DENSE_MERGE_LIMIT: i64 = 32_000_000;

/// The writes that leave every `cleared` cell air and every `dest` cell its
/// block (dest wins where both apply), minus those `current` already has.
/// Uses a dense array over the affected box when it is small enough: a
/// whole-layer move touches about a million cells per preview.
fn merge_writes(
    cleared: &[(IVec3, BlockId)],
    dest: &[(IVec3, BlockId)],
    current: impl Fn(IVec3) -> BlockId,
) -> Vec<(IVec3, BlockId)> {
    let all = || cleared.iter().chain(dest).map(|c| c.0);
    let Some(first) = all().next() else { return Vec::new() };
    let (lo, hi) = all().fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p)));
    let size = hi - lo + IVec3::ONE;
    let mut out = Vec::new();
    if size.as_i64vec3().element_product() <= DENSE_MERGE_LIMIT {
        const UNSET: u32 = u32::MAX;
        let slot = |p: IVec3| {
            let r = p - lo;
            (r.x + size.x * (r.y + size.y * r.z)) as usize
        };
        let mut wanted = vec![UNSET; size.as_i64vec3().element_product() as usize];
        for &(p, _) in cleared {
            wanted[slot(p)] = BlockId::AIR.0;
        }
        for &(p, b) in dest {
            wanted[slot(p)] = b.0;
        }
        for p in all() {
            let w = std::mem::replace(&mut wanted[slot(p)], UNSET);
            if w != UNSET && current(p).0 != w {
                out.push((p, BlockId(w)));
            }
        }
    } else {
        let mut wanted: CellMap<IVec3, BlockId> = cleared.iter().map(|&(p, _)| (p, BlockId::AIR)).collect();
        wanted.extend(dest.iter().copied());
        out.extend(wanted.into_iter().filter(|&(p, b)| current(p) != b));
    }
    out
}

/// Closest point to a ray on the line through `origin` along `axis`, as a
/// distance along the axis. `None` when the ray is nearly parallel to it.
fn axis_param(ray: Ray, origin: Vec3, axis: Vec3) -> Option<f32> {
    let (o, d) = ray;
    let w = origin - o;
    let b = axis.dot(d);
    let denom = 1.0 - b * b;
    (denom > 1e-4).then(|| (b * d.dot(w) - axis.dot(w)) / denom)
}

const AXIS_DIRS: [Vec3; 3] = [Vec3::X, Vec3::Y, Vec3::Z];

impl OctavisApp {
    pub(super) fn transform_panel(&mut self, ui: &mut egui::Ui) {
        if self.transform.is_none() {
            let has_selection = !self.editor.selection.is_empty();
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(has_selection, egui::Button::new("Transform selection"))
                    .on_hover_text("Move, rotate, scale or flip the selected blocks of the active layer (G)")
                    .clicked()
                {
                    self.start_transform(Source::Selection);
                }
                if ui.button("Transform layer").on_hover_text("Move, rotate, scale or flip the whole active layer").clicked() {
                    self.start_transform(Source::Layer);
                }
            });
            ui.small("G: transform the selection (or the active layer if nothing is selected).");
            return;
        }

        let mut apply = false;
        let mut cancel = false;
        let Some(s) = self.transform.as_mut() else { return };
        ui.horizontal(|ui| {
            apply = ui.button("Apply").on_hover_text("Keep the result (Enter)").clicked();
            cancel = ui.button("Cancel").on_hover_text("Put everything back (Esc)").clicked();
            ui.colored_label(egui::Color32::from_rgb(255, 170, 60), "Layers and tools locked");
        });
        let what = match s.source {
            Source::Selection => "selected blocks",
            Source::Layer => "blocks (whole layer)",
        };
        ui.small(format!(
            "{} {what}, pivot {}, {}, {}",
            s.cells.len(),
            s.pivot.x,
            s.pivot.y,
            s.pivot.z
        ));

        let t = &mut s.transform;
        ui.label("Move");
        ui.horizontal(|ui| {
            for (i, name) in ["X", "Y", "Z"].into_iter().enumerate() {
                ui.label(name);
                ui.add(egui::DragValue::new(&mut t.offset[i]).speed(0.2));
            }
        });
        ui.label("Rotate (degrees)").on_hover_text("Positive turns counter-clockwise, looking from the positive end of the axis");
        for (i, name) in ["X", "Y", "Z"].into_iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(name);
                ui.add(egui::DragValue::new(&mut t.rotation[i]).speed(1.0).range(0.0..=360.0).max_decimals(1).suffix("°"));
                if ui.button("-90").clicked() {
                    t.rotation[i] = (t.rotation[i] - 90.0).rem_euclid(360.0);
                }
                if ui.button("+90").clicked() {
                    t.rotation[i] = (t.rotation[i] + 90.0).rem_euclid(360.0);
                }
            });
        }
        ui.horizontal(|ui| {
            ui.label("Scale");
            ui.checkbox(&mut s.uniform_scale, "Uniform");
        });
        ui.horizontal(|ui| {
            if s.uniform_scale {
                let mut v = t.scale[0];
                if ui.add(egui::DragValue::new(&mut v).speed(0.01).range(0.1..=8.0).max_decimals(2)).changed() {
                    t.scale = [v; 3];
                }
            } else {
                for (i, name) in ["X", "Y", "Z"].into_iter().enumerate() {
                    ui.label(name);
                    ui.add(egui::DragValue::new(&mut t.scale[i]).speed(0.01).range(0.1..=8.0).max_decimals(2));
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Flip");
            for (i, name) in ["X", "Y", "Z"].into_iter().enumerate() {
                ui.toggle_value(&mut t.flip[i], name);
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut s.copy, "Keep original (copy)");
            if ui.button("Reset").on_hover_text("Back to no change").clicked() {
                s.transform = Transform::IDENTITY;
            }
        });
        if let Some(e) = &s.error {
            ui.colored_label(egui::Color32::from_rgb(255, 110, 90), e);
        }
        ui.small("Drag in the viewport to slide on the ground, or drag an arrow to move along it. Arrow keys and PgUp/PgDn nudge, R turns 90° about Y (Shift+R back). Rotation and scale are about the pivot; other angles and scales resample.");

        if apply {
            self.apply_transform();
        } else if cancel {
            self.cancel_transform();
        }
    }

    fn start_transform(&mut self, source: Source) {
        if self.transform.is_some() || self.terrain_preview.is_some() || !self.edit_allowed() {
            return;
        }
        let id = self.editor.layers.active();
        let Some(layer) = self.editor.layers.layer(id) else { return };
        let (cells, selection, bounds) = match source {
            Source::Selection => {
                let sel = self.editor.selection.clone();
                let cells: Vec<(IVec3, BlockId)> = sel
                    .iter()
                    .filter_map(|p| {
                        let b = layer.cells.get(p);
                        (b != BlockId::AIR).then_some((p, b))
                    })
                    .collect();
                let bounds = sel.bounds();
                (cells, Some(sel), bounds)
            }
            Source::Layer => (layer.cells.solid_cells(), None, layer.cells.content_bounds()),
        };
        let (Some((lo, hi)), false) = (bounds, cells.is_empty()) else {
            self.file_msg = Some(Err(match source {
                Source::Selection => format!("No blocks of the active layer \"{}\" are selected", layer.name),
                Source::Layer => format!("The active layer \"{}\" is empty", layer.name),
            }));
            return;
        };
        self.transform = Some(TransformSession {
            layer: id,
            source,
            cells,
            selection,
            pivot: Transform::pivot_of(lo, hi),
            transform: Transform::IDENTITY,
            copy: false,
            uniform_scale: true,
            shown: Some((Transform::IDENTITY, false)),
            backup: None,
            last_update: Instant::now(),
            moved_selection: None,
            error: None,
            drag: None,
            hovered_axis: None,
        });
        self.file_msg = None;
    }

    /// Call every frame: refreshes the preview when the transform changed.
    pub(super) fn update_transform(&mut self, ctx: &egui::Context) {
        let Some(s) = self.transform.as_ref() else { return };
        if s.shown == Some((s.transform, s.copy)) {
            return;
        }
        let wait = PREVIEW_INTERVAL.saturating_sub(s.last_update.elapsed());
        if !wait.is_zero() {
            ctx.request_repaint_after(wait);
            return;
        }
        let Some(mut s) = self.transform.take() else { return };
        if let Some(b) = s.backup.take() {
            self.editor.restore(b);
        }
        let before = self.editor.world.blocks.len();
        match plan(&s, &mut self.editor) {
            Ok((cells, moved)) => {
                s.backup = Some(self.editor.apply_untracked(s.layer, &cells));
                s.moved_selection = moved;
                s.error = None;
            }
            Err(e) => {
                s.moved_selection = None;
                s.error = Some(e);
            }
        }
        self.palette_dirty |= self.editor.world.blocks.len() != before;
        // Only a selection transform moves the drawn selection.
        self.overlay_dirty |= s.selection.is_some();
        s.shown = Some((s.transform, s.copy));
        s.last_update = Instant::now();
        self.transform = Some(s);
    }

    /// Commits the pending transform as one undo step.
    pub(super) fn apply_transform(&mut self) {
        let Some(mut s) = self.transform.take() else { return };
        if let Some(b) = s.backup.take() {
            self.editor.restore(b);
        }
        let before = self.editor.world.blocks.len();
        match plan(&s, &mut self.editor) {
            Ok((cells, moved)) => {
                let dropped = moved.as_ref().is_some_and(|m| m.is_empty());
                self.editor.commit_cells(s.layer, &cells, moved);
                self.file_msg = Some(Ok(format!(
                    "Transformed {} blocks.{} Ctrl+Z undoes it.",
                    s.cells.len(),
                    if dropped { " The result was too large to keep selected." } else { "" }
                )));
            }
            Err(e) => {
                // Keep the session open so the values can be fixed.
                s.error = Some(e);
                s.shown = None;
                self.transform = Some(s);
            }
        }
        self.palette_dirty |= self.editor.world.blocks.len() != before;
        self.overlay_dirty = true;
    }

    /// Drops the pending transform and restores the layer.
    pub(super) fn cancel_transform(&mut self) {
        let Some(s) = self.transform.take() else { return };
        if let Some(b) = s.backup {
            self.editor.restore(b);
        }
        self.overlay_dirty = true;
    }

    /// Gizmo origin and arrow length in world units (a fixed share of the
    /// view, so the arrows keep their screen size).
    pub(super) fn transform_gizmo(&self) -> Option<(Vec3, f32, Option<usize>)> {
        let s = self.transform.as_ref()?;
        let origin = s.gizmo_origin();
        let length = self.camera.eye().distance(origin) * 0.12;
        let active = s.drag.as_ref().map_or(s.hovered_axis, |d| d.axis);
        Some((origin, length, active))
    }

    /// Viewport input while transforming: drag an arrow to move along its
    /// axis, drag anywhere else to slide on the horizontal plane.
    pub(super) fn transform_viewport(&mut self, ui: &egui::Ui, rect: egui::Rect, ray: Option<Ray>, hovered: bool) {
        let Some((origin, length, _)) = self.transform_gizmo() else { return };
        let view_proj = self.camera.view_proj(rect.width() / rect.height().max(1.0));
        let (pressed, down, cursor) =
            ui.input(|i| (i.pointer.button_pressed(egui::PointerButton::Primary), i.pointer.primary_down(), i.pointer.hover_pos()));
        let Some(s) = self.transform.as_mut() else { return };

        if s.drag.is_none() {
            s.hovered_axis = cursor.and_then(|c| gizmo::pick_move_gizmo(rect, &view_proj, origin, length, c));
        }
        if pressed && hovered && s.drag.is_none() {
            if let Some((o, d)) = ray {
                let start = match s.hovered_axis {
                    Some(a) => axis_param((o, d), origin, AXIS_DIRS[a]).map(|p| Vec3::new(p, 0.0, 0.0)),
                    None => ray_plane_point(o, d, origin, Vec3::Y, MAX_PICK_DISTANCE),
                };
                if let Some(start) = start {
                    s.drag = Some(MoveDrag { axis: s.hovered_axis, start_offset: s.transform.offset, origin, start });
                }
            }
        }
        if !down {
            s.drag = None;
            return;
        }
        let (Some(drag), Some(ray)) = (&s.drag, ray) else { return };
        let mut offset = drag.start_offset;
        match drag.axis {
            Some(a) => {
                let Some(p) = axis_param(ray, drag.origin, AXIS_DIRS[a]) else { return };
                offset[a] += (p - drag.start.x).round() as i32;
            }
            None => {
                let Some(p) = ray_plane_point(ray.0, ray.1, drag.origin, Vec3::Y, MAX_PICK_DISTANCE) else { return };
                let moved = (p - drag.start).round().as_ivec3();
                offset.x += moved.x;
                offset.z += moved.z;
            }
        }
        s.transform.offset = offset;
    }

    /// Keys while transforming (unless a text box has focus): Enter applies,
    /// Esc cancels, arrows and PgUp/PgDn nudge, R turns about Y. G starts a
    /// transform when none is pending.
    pub(super) fn transform_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if self.transform.is_none() {
            let start = ctx.input(|i| i.key_pressed(Key::G) && !i.modifiers.any());
            if start && self.drag.is_none() {
                let source = if self.editor.selection.is_empty() { Source::Layer } else { Source::Selection };
                self.start_transform(source);
            }
            return;
        }
        let (enter, esc, shift) =
            ctx.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape), i.modifiers.shift));
        if enter {
            self.apply_transform();
            return;
        }
        if esc {
            self.cancel_transform();
            return;
        }
        // Arrow keys move along the horizontal axes nearest the view.
        let f = self.camera.forward();
        let forward = if f.x.abs() > f.z.abs() {
            IVec3::new(f.x.signum() as i32, 0, 0)
        } else {
            IVec3::new(0, 0, f.z.signum() as i32)
        };
        let right = IVec3::new(-forward.z, 0, forward.x);
        let nudge = ctx.input(|i| {
            [
                (Key::ArrowUp, forward),
                (Key::ArrowDown, -forward),
                (Key::ArrowRight, right),
                (Key::ArrowLeft, -right),
                (Key::PageUp, IVec3::Y),
                (Key::PageDown, IVec3::NEG_Y),
            ]
            .into_iter()
            .filter(|(k, _)| i.key_pressed(*k))
            .map(|(_, d)| d)
            .sum::<IVec3>()
        });
        let turn = ctx.input(|i| i.key_pressed(Key::R));
        let Some(s) = self.transform.as_mut() else { return };
        s.transform.offset += nudge;
        if turn {
            let step = if shift { -90.0 } else { 90.0 };
            s.transform.rotation[1] = (s.transform.rotation[1] + step).rem_euclid(360.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octavis_core::{BlockState, World};

    fn session(e: &Editor, source: Source, transform: Transform, copy: bool) -> TransformSession {
        let layer = e.layers.active();
        let cells = e.layers.layer(layer).unwrap().cells.solid_cells();
        let (selection, bounds) = match source {
            Source::Selection => (Some(e.selection.clone()), e.selection.bounds().unwrap()),
            Source::Layer => (None, e.layers.layer(layer).unwrap().cells.content_bounds().unwrap()),
        };
        TransformSession {
            layer,
            source,
            cells,
            selection,
            pivot: Transform::pivot_of(bounds.0, bounds.1),
            transform,
            copy,
            uniform_scale: true,
            shown: None,
            backup: None,
            last_update: Instant::now(),
            moved_selection: None,
            error: None,
            drag: None,
            hovered_axis: None,
        }
    }

    fn editor() -> (Editor, BlockId, BlockId) {
        let mut w = World::new();
        let stone = w.blocks.intern(BlockState::new("stone"));
        let dirt = w.blocks.intern(BlockState::new("dirt"));
        (Editor::new(w, "Ground"), stone, dirt)
    }

    #[test]
    fn move_clears_the_source_and_copy_keeps_it() {
        let (mut e, stone, dirt) = editor();
        e.set_block(IVec3::ZERO, stone);
        e.set_block(IVec3::new(2, 0, 0), dirt); // lies where the block moves to
        e.selection = Selection::from_cells([IVec3::ZERO, IVec3::Y]);
        let shift = Transform { offset: IVec3::new(2, 0, 0), ..Transform::IDENTITY };

        let s = session(&e, Source::Selection, shift, false);
        let (cells, moved) = plan(&s, &mut e).unwrap();
        let layer = s.layer;
        e.commit_cells(layer, &cells, moved);
        assert_eq!(e.world.get(IVec3::ZERO), BlockId::AIR);
        assert_eq!(e.world.get(IVec3::new(2, 0, 0)), stone);
        // The selection (air cell included) moved along.
        assert!(e.selection.contains(IVec3::new(2, 1, 0)) && !e.selection.contains(IVec3::ZERO));
        e.undo();
        assert_eq!((e.world.get(IVec3::ZERO), e.world.get(IVec3::new(2, 0, 0))), (stone, dirt));
        assert!(e.selection.contains(IVec3::ZERO));

        let s = session(&e, Source::Selection, shift, true);
        let (cells, _) = plan(&s, &mut e).unwrap();
        e.commit_cells(layer, &cells, None);
        assert_eq!((e.world.get(IVec3::ZERO), e.world.get(IVec3::new(2, 0, 0))), (stone, stone));
    }

    #[test]
    fn merge_writes_lets_dest_win_and_skips_unchanged_cells() {
        let (a, b) = (BlockId(1), BlockId(2));
        let current = |p: IVec3| if p == IVec3::ZERO || p == IVec3::X { a } else { BlockId::AIR };
        // A near cell uses the dense path, a far one the sparse path.
        for far in [IVec3::new(3, 0, 0), IVec3::splat(400)] {
            let cleared = [(IVec3::ZERO, a), (IVec3::X, a)];
            let dest = [(IVec3::X, a), (far, b)];
            let mut out = merge_writes(&cleared, &dest, current);
            out.sort_by_key(|c| c.0.to_array());
            assert_eq!(out, vec![(IVec3::ZERO, BlockId::AIR), (far, b)]);
        }
    }

    #[test]
    fn layer_transform_rotates_everything_in_the_layer() {
        let (mut e, stone, _) = editor();
        for x in 0..5 {
            e.set_block(IVec3::new(x, 0, 0), stone);
        }
        let quarter = Transform { rotation: [0.0, 90.0, 0.0], ..Transform::IDENTITY };
        let s = session(&e, Source::Layer, quarter, false);
        let (cells, moved) = plan(&s, &mut e).unwrap();
        assert!(moved.is_none());
        e.commit_cells(s.layer, &cells, None);
        // A row along X through pivot (2,0,0) becomes a row along Z.
        let solid: Vec<IVec3> = e.layers.layer(s.layer).unwrap().cells.solid_cells().into_iter().map(|c| c.0).collect();
        assert_eq!(solid.len(), 5);
        assert!(solid.iter().all(|p| p.x == 2 && p.y == 0 && (-2..=2).contains(&p.z)));
    }

    #[test]
    fn preview_and_restore_leave_the_layer_unchanged() {
        let (mut e, stone, _) = editor();
        e.set_block(IVec3::ZERO, stone);
        let before = e.layers.layer(e.layers.active()).unwrap().cells.solid_cells();
        let s = session(&e, Source::Layer, Transform { offset: IVec3::X, ..Transform::IDENTITY }, false);
        let (cells, _) = plan(&s, &mut e).unwrap();
        let backup = e.apply_untracked(s.layer, &cells);
        assert_eq!(e.world.get(IVec3::X), stone);
        e.restore(backup);
        assert_eq!(e.layers.layer(s.layer).unwrap().cells.solid_cells(), before);
        assert!(!e.history.can_undo()); // the preview never reaches history
    }

    /// Run with `cargo test --release -p octavis-app timing_ -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing_moving_the_default_ground_layer() {
        let mut e = Editor::new(crate::scene::demo_world(), "Ground");
        for offset in [IVec3::X, IVec3::Y, IVec3::X, IVec3::Y] {
            let s = session(&e, Source::Layer, Transform { offset, ..Transform::IDENTITY }, false);
            let t0 = Instant::now();
            let (cells, _) = plan(&s, &mut e).unwrap();
            let t1 = Instant::now();
            let backup = e.apply_untracked(s.layer, &cells);
            let t2 = Instant::now();
            e.restore(backup);
            let t3 = Instant::now();
            e.take_dirty();
            println!(
                "offset {offset}: {} cells, {} changes; plan {:?}, apply {:?}, restore {:?}",
                s.cells.len(),
                cells.len(),
                t1 - t0,
                t2 - t1,
                t3 - t2
            );
        }
    }
}
