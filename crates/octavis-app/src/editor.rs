//! Editing state and operations, independent of egui so it can be tested.

use std::collections::HashSet;

use glam::IVec3;
use octavis_core::{
    BlockId, Brush, Change, History, LayerCmd, LayerId, LayerKind, LayerStack, SECTION_SIZE,
    Selection, World,
};

/// Largest selection a drag may create. Keeps previews interactive and
/// stops a runaway drag from exhausting memory; revisit for big builds.
pub const MAX_SELECTION_CELLS: i64 = 500_000;
/// Radius whose ball holds about `MAX_SELECTION_CELLS` cells.
pub const MAX_SPHERE_RADIUS: f32 = 49.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectMode {
    Replace,
    Add,
    Subtract,
    Intersect,
}

/// Cells written by a live preview, with what they replaced, so the preview
/// can be reverted or turned into an undo step.
pub struct Backup {
    layer: LayerId,
    cells: Vec<(IVec3, BlockId)>,
}

impl Backup {
    pub fn len(&self) -> usize {
        self.cells.len()
    }
}

pub struct Editor {
    /// The composite of all shown layers: what is visible, and what tools,
    /// meshing and picking read. Written only through the layer stack.
    pub world: World,
    pub layers: LayerStack,
    pub history: History,
    pub selection: Selection,
    /// Sections whose mesh must be rebuilt before the next draw.
    dirty: HashSet<IVec3>,
    selection_dirty: bool,
}

impl Editor {
    /// An editor whose single layer, `layer_name`, holds `world`'s cells.
    pub fn new(world: World, layer_name: &str) -> Self {
        let dirty = world.sections().map(|(p, _)| p).collect();
        let layers = LayerStack::single(layer_name, (*world).clone());
        Self {
            world,
            layers,
            history: History::new(),
            selection: Selection::new(),
            dirty,
            selection_dirty: true,
        }
    }

    /// True if the selection changed since the last call.
    pub fn take_selection_dirty(&mut self) -> bool {
        std::mem::take(&mut self.selection_dirty)
    }

    pub fn take_dirty(&mut self) -> Vec<IVec3> {
        self.dirty.drain().collect()
    }

    /// Marks the section holding `pos`, plus any neighbour whose border
    /// faces depend on it.
    fn mark_dirty(&mut self, pos: IVec3) {
        let s = SECTION_SIZE as i32;
        let section = World::section_pos(pos);
        let local = pos - section * s;
        self.dirty.insert(section);
        for axis in 0..3 {
            let mut dir = IVec3::ZERO;
            if local[axis] == 0 {
                dir[axis] = -1;
            } else if local[axis] == s - 1 {
                dir[axis] = 1;
            } else {
                continue;
            }
            self.dirty.insert(section + dir);
        }
    }

    // ------------------------------------------------------------ cells

    /// Why tools cannot edit right now (active layer locked, hidden or a
    /// generator result), if so.
    pub fn edit_blocked(&self) -> Option<&'static str> {
        self.layers.why_not_editable(self.layers.active())
    }

    /// Writes into one layer as part of the open edit (or untracked if
    /// none), with no permission check.
    fn write(&mut self, layer: LayerId, pos: IVec3, block: BlockId) {
        if self.history.set(&mut self.layers, &mut self.world, layer, pos, block) {
            self.mark_dirty(pos);
        }
    }

    /// Writes a block into the active layer as part of the open edit.
    /// Ignored if the active layer is not editable.
    pub fn set_block(&mut self, pos: IVec3, block: BlockId) {
        let layer = self.layers.active();
        if self.layers.editable(layer) {
            self.write(layer, pos, block);
        }
    }

    pub fn begin_edit(&mut self) {
        self.history.begin();
    }

    pub fn end_edit(&mut self) {
        self.history.commit();
    }

    /// Applies one brush dab as part of the open edit.
    pub fn apply_brush(&mut self, brush: &Brush, center: IVec3) {
        for (p, b) in brush.plan(&self.world, &self.selection, center) {
            self.set_block(p, b);
        }
    }

    /// Runs the brush's mode, mask and pattern over every selected cell as
    /// one undoable edit. Returns how many cells changed.
    pub fn apply_brush_to_selection(&mut self, brush: &Brush) -> usize {
        let cells: Vec<IVec3> = self.selection.iter().collect();
        let changes = brush.plan_cells(&self.world, &self.selection, &cells);
        self.begin_edit();
        for &(p, b) in &changes {
            self.set_block(p, b);
        }
        self.end_edit();
        changes.len()
    }

    /// Whether `pos` was visibly solid before the open edit (stroke) began.
    /// Strokes pick against this so their own changes do not move the target.
    pub fn was_solid(&self, pos: IVec3) -> bool {
        let active = self.layers.active();
        match self.history.original(active, pos) {
            Some(orig) => self.layers.composite_with(pos, active, orig) != BlockId::AIR,
            None => self.world.get(pos) != BlockId::AIR,
        }
    }

    /// Fills every selected cell as one undoable edit.
    pub fn fill_selection(&mut self, block: BlockId) {
        let cells: Vec<IVec3> = self.selection.iter().collect();
        self.begin_edit();
        for p in cells {
            self.set_block(p, block);
        }
        self.end_edit();
    }

    /// Writes cells into a specific layer (a generator's own layer, say) as
    /// one undoable edit, ignoring the layer's lock state.
    pub fn paste_to(&mut self, layer: LayerId, cells: &[(IVec3, BlockId)]) {
        self.begin_edit();
        for &(p, b) in cells {
            self.write(layer, p, b);
        }
        self.end_edit();
    }

    /// Writes cells into a layer without recording history (for a live
    /// preview) and returns the old values so [`Editor::restore`] or
    /// [`Editor::commit_applied`] can finish the job.
    pub fn apply_untracked(&mut self, layer: LayerId, cells: &[(IVec3, BlockId)]) -> Backup {
        debug_assert!(!self.history.is_open(), "untracked write inside an open edit");
        let mut old = Vec::with_capacity(cells.len());
        for &(p, b) in cells {
            old.push((p, self.layers.layer(layer).map_or(BlockId::AIR, |l| l.cells.get(p))));
            self.write(layer, p, b);
        }
        Backup { layer, cells: old }
    }

    /// Undoes [`Editor::apply_untracked`].
    pub fn restore(&mut self, backup: Backup) {
        for (p, old) in backup.cells.into_iter().rev() {
            self.write(backup.layer, p, old);
        }
    }

    /// Turns an [`Editor::apply_untracked`] write into one undo step.
    pub fn commit_applied(&mut self, backup: Backup) {
        let layer = backup.layer;
        let changes: Vec<Change> = backup
            .cells
            .into_iter()
            .map(|(pos, old)| Change {
                layer,
                pos,
                old,
                new: self.layers.layer(layer).map_or(BlockId::AIR, |l| l.cells.get(pos)),
            })
            .collect();
        self.history.record_applied(changes);
    }

    // ----------------------------------------------------------- layers

    /// Runs `f` inside the open edit, or inside a fresh one that is
    /// committed afterwards.
    fn in_edit<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let was_open = self.history.is_open();
        if !was_open {
            self.history.begin();
        }
        let r = f(self);
        if !was_open {
            self.history.commit();
        }
        r
    }

    /// Recomputes the composite from the layers and schedules a remesh.
    pub fn rebuild(&mut self) {
        let touched = self.layers.rebuild(&mut self.world);
        self.dirty.extend(touched);
    }

    /// Adds an empty layer above the active one and makes it active.
    pub fn add_layer(&mut self, name: &str, kind: LayerKind) -> LayerId {
        self.in_edit(|ed| {
            let layer = ed.layers.new_layer(name, kind);
            let id = layer.id;
            let index = ed.layers.index_of(ed.layers.active()).map_or(ed.layers.layers().len(), |i| i + 1);
            ed.layers.insert(index, layer);
            ed.layers.set_active(id);
            ed.history.push_cmd(LayerCmd::added(id, index));
            id
        })
    }

    /// Adds a layer holding `cells` as one undo step.
    pub fn add_layer_with(&mut self, name: &str, kind: LayerKind, cells: &[(IVec3, BlockId)]) -> LayerId {
        self.in_edit(|ed| {
            let id = ed.add_layer(name, kind);
            for &(p, b) in cells {
                ed.write(id, p, b);
            }
            id
        })
    }

    /// Removes a layer (the last one cannot be removed).
    pub fn delete_layer(&mut self, id: LayerId) -> bool {
        let removed = self.in_edit(|ed| {
            let Some((index, layer)) = ed.layers.remove(id) else { return false };
            ed.history.push_cmd(LayerCmd::removed(index, layer));
            true
        });
        if removed {
            self.rebuild();
        }
        removed
    }

    /// Moves a layer one step up (towards the top) or down.
    pub fn move_layer(&mut self, id: LayerId, up: bool) -> bool {
        let Some(i) = self.layers.index_of(id) else { return false };
        let j = if up { i + 1 } else { i.wrapping_sub(1) };
        if j >= self.layers.layers().len() {
            return false;
        }
        self.in_edit(|ed| {
            ed.layers.swap(i, j);
            ed.history.push_cmd(LayerCmd::swapped(i, j));
        });
        self.rebuild();
        true
    }

    /// Merges a layer into the one below it. Generator layers cannot be
    /// merged into (their cells are a regenerable result).
    pub fn merge_down(&mut self, id: LayerId) -> Result<(), &'static str> {
        let (lower, changes) = self.layers.merge_plan(id).ok_or("There is no layer below")?;
        if !self.layers.editable(lower) {
            return Err("The layer below is locked, hidden or a generator layer");
        }
        self.in_edit(|ed| {
            for (pos, _, new) in changes {
                ed.write(lower, pos, new);
            }
            if let Some((index, layer)) = ed.layers.remove(id) {
                ed.history.push_cmd(LayerCmd::removed(index, layer));
            }
        });
        self.rebuild();
        Ok(())
    }

    /// Visibility is view state: not undoable.
    pub fn set_visible(&mut self, id: LayerId, visible: bool) {
        if let Some(l) = self.layers.layer_mut(id) {
            l.visible = visible;
            self.rebuild();
        }
    }

    pub fn set_solo(&mut self, solo: Option<LayerId>) {
        self.layers.set_solo(solo);
        self.rebuild();
    }

    pub fn set_locked(&mut self, id: LayerId, locked: bool) {
        if let Some(l) = self.layers.layer_mut(id) {
            l.locked = locked;
        }
    }

    pub fn rename_layer(&mut self, id: LayerId, name: &str) {
        if let Some(l) = self.layers.layer_mut(id) {
            l.name = name.to_string();
        }
    }

    /// Turns a generator layer into an ordinary painted one.
    pub fn bake_layer(&mut self, id: LayerId) {
        if let Some(l) = self.layers.layer_mut(id) {
            l.kind = LayerKind::Painted;
        }
    }

    /// Swaps in a whole new world as a single layer, discarding history and
    /// selection, and schedules a remesh of every old and new section.
    pub fn replace_world(&mut self, world: World, layer_name: &str) {
        let mut dirty: HashSet<IVec3> = self.world.sections().map(|(p, _)| p).collect();
        dirty.extend(world.sections().map(|(p, _)| p));
        self.layers = LayerStack::single(layer_name, (*world).clone());
        self.world = world;
        self.history = History::new();
        self.selection = Selection::new();
        self.dirty = dirty;
        self.selection_dirty = true;
    }

    // ------------------------------------------------------ undo / redo

    pub fn undo(&mut self) {
        let u = self.history.undo(&mut self.layers, &mut self.world, &mut self.selection);
        self.after_undo(u);
    }

    pub fn redo(&mut self) {
        let u = self.history.redo(&mut self.layers, &mut self.world, &mut self.selection);
        self.after_undo(u);
    }

    fn after_undo(&mut self, u: octavis_core::Undone) {
        for p in u.cells {
            self.mark_dirty(p);
        }
        if u.structure_changed {
            self.rebuild();
        }
        self.selection_dirty |= u.selection_changed;
    }

    // -------------------------------------------------------- selection

    /// Combines `new` into the selection per `mode` as one undoable step.
    pub fn apply_selection(&mut self, mode: SelectMode, new: &Selection) {
        let mut result = self.selection.clone();
        match mode {
            SelectMode::Replace => result = new.clone(),
            SelectMode::Add => result.union(new),
            SelectMode::Subtract => result.subtract(new),
            SelectMode::Intersect => result.intersect(new),
        }
        let delta = self.selection.delta_to(&result);
        if delta.is_empty() {
            return;
        }
        self.selection = result;
        self.history.record_selection(delta);
        self.selection_dirty = true;
    }

    pub fn clear_selection(&mut self) {
        self.apply_selection(SelectMode::Replace, &Selection::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octavis_core::BlockState;

    fn editor() -> (Editor, BlockId) {
        let mut w = World::new();
        let stone = w.blocks.intern(BlockState::new("stone"));
        (Editor::new(w, "Ground"), stone)
    }

    #[test]
    fn interior_edit_dirties_one_section() {
        let (mut e, stone) = editor();
        e.set_block(IVec3::new(5, 5, 5), stone);
        assert_eq!(e.take_dirty(), vec![IVec3::ZERO]);
        assert!(e.take_dirty().is_empty());
    }

    #[test]
    fn border_edit_dirties_neighbours_including_negative() {
        let (mut e, stone) = editor();
        e.set_block(IVec3::new(0, 5, 15), stone); // -x and +z borders
        let mut d = e.take_dirty();
        d.sort_by_key(|p| p.to_array());
        let mut expect = vec![IVec3::ZERO, IVec3::NEG_X, IVec3::Z];
        expect.sort_by_key(|p| p.to_array());
        assert_eq!(d, expect);
    }

    #[test]
    fn unchanged_write_is_not_dirty() {
        let (mut e, _) = editor();
        e.set_block(IVec3::ONE, BlockId::AIR);
        assert!(e.take_dirty().is_empty());
    }

    #[test]
    fn fill_selection_is_one_undo_step() {
        let (mut e, stone) = editor();
        e.selection = Selection::cuboid(IVec3::ZERO, IVec3::new(3, 3, 3));
        e.fill_selection(stone);
        assert_eq!(e.world.get(IVec3::new(2, 2, 2)), stone);
        e.undo();
        assert_eq!(e.world.get(IVec3::new(2, 2, 2)), BlockId::AIR);
        e.redo();
        assert_eq!(e.world.get(IVec3::new(3, 3, 3)), stone);
    }

    #[test]
    fn undo_marks_sections_dirty() {
        let (mut e, stone) = editor();
        e.begin_edit();
        e.set_block(IVec3::new(20, 1, 1), stone);
        e.end_edit();
        e.take_dirty();
        e.undo();
        assert_eq!(e.take_dirty(), vec![IVec3::X]);
    }

    #[test]
    fn a_brush_stroke_is_one_undo_step() {
        use octavis_core::{BrushMode, Mask, Pattern, Shape};
        let (mut e, stone) = editor();
        let brush = Brush {
            shape: Shape::Sphere { radius: 2.0 },
            mask: Mask::Any,
            pattern: Pattern::Single(stone),
            mode: BrushMode::Paint,
            seed: 0,
        };
        e.begin_edit();
        e.apply_brush(&brush, IVec3::new(0, 0, 0));
        e.apply_brush(&brush, IVec3::new(10, 0, 0)); // second dab, other sections
        e.end_edit();
        assert_eq!(e.world.get(IVec3::new(10, 1, 0)), stone);
        assert!(!e.take_dirty().is_empty());
        e.undo();
        assert_eq!(e.world.get(IVec3::new(10, 1, 0)), BlockId::AIR);
        assert_eq!(e.world.get(IVec3::ZERO), BlockId::AIR);
        assert!(!e.history.can_undo());
    }

    #[test]
    fn paste_to_is_one_undo_step() {
        let (mut e, stone) = editor();
        let layer = e.layers.active();
        e.paste_to(layer, &[(IVec3::new(1, 1, 1), stone), (IVec3::new(40, 0, 0), stone)]);
        assert_eq!(e.world.get(IVec3::new(40, 0, 0)), stone);
        e.undo();
        assert_eq!(e.world.get(IVec3::new(1, 1, 1)), BlockId::AIR);
        assert!(!e.history.can_undo());
    }

    #[test]
    fn untracked_preview_can_be_restored_or_committed() {
        let (mut e, stone) = editor();
        let layer = e.layers.active();
        e.set_block(IVec3::ZERO, stone);
        e.take_dirty();
        let plan = [(IVec3::ZERO, BlockId::AIR), (IVec3::new(2, 0, 0), stone)];
        let backup = e.apply_untracked(layer, &plan);
        assert_eq!(e.world.get(IVec3::new(2, 0, 0)), stone);
        assert!(!e.take_dirty().is_empty() && !e.history.can_undo());
        e.restore(backup);
        assert_eq!(e.world.get(IVec3::ZERO), stone);
        assert_eq!(e.world.get(IVec3::new(2, 0, 0)), BlockId::AIR);

        let backup = e.apply_untracked(layer, &plan);
        e.commit_applied(backup);
        assert!(e.history.can_undo());
        e.undo();
        assert_eq!(e.world.get(IVec3::ZERO), stone);
        assert_eq!(e.world.get(IVec3::new(2, 0, 0)), BlockId::AIR);
        e.redo();
        assert_eq!(e.world.get(IVec3::new(2, 0, 0)), stone);
    }

    #[test]
    fn replacing_the_world_remeshes_old_and_new_sections() {
        let (mut e, stone) = editor();
        e.set_block(IVec3::new(100, 0, 0), stone);
        e.take_dirty();
        e.selection = Selection::cuboid(IVec3::ZERO, IVec3::ONE);
        let mut fresh = World::new();
        let id = fresh.blocks.intern(octavis_core::BlockState::new("stone"));
        fresh.set(IVec3::new(-20, 0, 0), id);
        e.replace_world(fresh, "Opened");
        let mut d = e.take_dirty();
        d.sort_by_key(|p| p.x);
        assert_eq!(d, vec![IVec3::new(-2, 0, 0), IVec3::new(6, 0, 0)]);
        assert!(e.selection.is_empty() && !e.history.can_undo());
        assert_eq!(e.layers.layers().len(), 1);
        assert_eq!(e.layers.layers()[0].name, "Opened");
        assert_eq!(e.layers.layers()[0].cells.get(IVec3::new(-20, 0, 0)), id);
    }

    #[test]
    fn stroke_picking_ignores_the_strokes_own_changes() {
        let (mut e, stone) = editor();
        e.set_block(IVec3::ZERO, stone);
        e.begin_edit();
        e.set_block(IVec3::ZERO, BlockId::AIR); // carved during the stroke
        e.set_block(IVec3::X, stone); // built during the stroke
        assert!(e.was_solid(IVec3::ZERO));
        assert!(!e.was_solid(IVec3::X));
        e.end_edit();
    }

    #[test]
    fn stroke_picking_still_sees_other_layers() {
        let (mut e, stone) = editor();
        e.set_block(IVec3::ZERO, stone); // ground layer
        let top = e.add_layer("top", LayerKind::Painted);
        assert_eq!(e.layers.active(), top);
        e.begin_edit();
        e.set_block(IVec3::ZERO, stone);
        e.set_block(IVec3::X, stone);
        // The block below keeps (0,0,0) solid; (1,0,0) only exists since the stroke began.
        assert!(e.was_solid(IVec3::ZERO));
        assert!(!e.was_solid(IVec3::X));
        e.end_edit();
    }

    #[test]
    fn brush_applies_to_the_selection_only() {
        use octavis_core::{BrushMode, Mask, Pattern, Shape};
        let (mut e, stone) = editor();
        e.selection = Selection::cuboid(IVec3::ZERO, IVec3::new(2, 0, 0));
        let brush = Brush {
            shape: Shape::Sphere { radius: 1.0 },
            mask: Mask::Any,
            pattern: Pattern::Single(stone),
            mode: BrushMode::Paint,
            seed: 0,
        };
        assert_eq!(e.apply_brush_to_selection(&brush), 3);
        assert_eq!(e.world.get(IVec3::new(3, 0, 0)), BlockId::AIR);
        e.undo();
        assert_eq!(e.world.get(IVec3::new(1, 0, 0)), BlockId::AIR);
    }

    #[test]
    fn selection_modes() {
        let (mut e, _) = editor();
        let a = Selection::cuboid(IVec3::ZERO, IVec3::new(3, 0, 0));
        let b = Selection::cuboid(IVec3::new(2, 0, 0), IVec3::new(5, 0, 0));
        e.apply_selection(SelectMode::Replace, &a);
        assert_eq!(e.selection.len(), 4);
        e.apply_selection(SelectMode::Add, &b);
        assert_eq!(e.selection.len(), 6);
        e.apply_selection(SelectMode::Subtract, &a);
        assert_eq!(e.selection.len(), 2);
        e.apply_selection(SelectMode::Intersect, &b);
        assert_eq!(e.selection.len(), 2);
        e.apply_selection(SelectMode::Replace, &Selection::new());
        assert!(e.selection.is_empty());
    }

    #[test]
    fn selection_changes_undo_and_flag_overlay() {
        let (mut e, _) = editor();
        e.take_selection_dirty();
        e.apply_selection(SelectMode::Replace, &Selection::cuboid(IVec3::ZERO, IVec3::ONE));
        assert!(e.take_selection_dirty());
        e.clear_selection();
        assert!(e.selection.is_empty());
        e.undo();
        assert_eq!(e.selection.len(), 8);
        assert!(e.take_selection_dirty());
        e.undo();
        assert!(e.selection.is_empty());
        e.redo();
        assert_eq!(e.selection.len(), 8);
    }

    #[test]
    fn selecting_nothing_new_records_no_history() {
        let (mut e, _) = editor();
        e.apply_selection(SelectMode::Add, &Selection::new());
        e.clear_selection();
        assert!(!e.history.can_undo());
    }

    // ------------------------------------------------------------ layers

    #[test]
    fn edits_go_to_the_active_layer_and_show_through_layers_below() {
        let (mut e, stone) = editor();
        e.set_block(IVec3::ZERO, stone);
        let top = e.add_layer("top", LayerKind::Painted);
        e.set_block(IVec3::X, stone);
        assert_eq!(e.layers.layer(top).unwrap().cells.get(IVec3::X), stone);
        assert_eq!(e.layers.layers()[0].cells.get(IVec3::X), BlockId::AIR);
        assert_eq!(e.world.get(IVec3::ZERO), stone, "lower layer still visible");
        // Erasing in the top layer cannot remove the lower layer's block.
        e.set_block(IVec3::ZERO, BlockId::AIR);
        assert_eq!(e.world.get(IVec3::ZERO), stone);
    }

    #[test]
    fn locked_hidden_and_generator_layers_reject_tool_writes() {
        let (mut e, stone) = editor();
        let id = e.layers.active();
        e.set_locked(id, true);
        assert!(e.edit_blocked().is_some());
        e.set_block(IVec3::ZERO, stone);
        assert_eq!(e.world.get(IVec3::ZERO), BlockId::AIR);
        e.set_locked(id, false);
        e.set_visible(id, false);
        assert!(e.edit_blocked().is_some());
        e.set_visible(id, true);
        e.bake_layer(id);
        let generated = e.add_layer("terrain", LayerKind::Generator);
        assert!(e.edit_blocked().is_some());
        // The generator itself can still write its own layer.
        e.paste_to(generated, &[(IVec3::ONE, stone)]);
        assert_eq!(e.world.get(IVec3::ONE), stone);
        e.bake_layer(generated);
        assert!(e.edit_blocked().is_none());
    }

    #[test]
    fn visibility_hides_a_layer_from_the_composite() {
        let (mut e, stone) = editor();
        e.set_block(IVec3::ZERO, stone);
        let id = e.layers.active();
        e.add_layer("top", LayerKind::Painted);
        e.set_visible(id, false);
        assert_eq!(e.world.get(IVec3::ZERO), BlockId::AIR);
        assert!(e.take_dirty().contains(&IVec3::ZERO));
        e.set_visible(id, true);
        assert_eq!(e.world.get(IVec3::ZERO), stone);
    }

    #[test]
    fn layer_add_delete_move_and_merge_are_undoable() {
        let (mut e, stone) = editor();
        let dirt = e.world.blocks.intern(octavis_core::BlockState::new("dirt"));
        let bottom = e.layers.active();
        e.set_block(IVec3::ZERO, stone);
        let top = e.add_layer("top", LayerKind::Painted);
        e.set_block(IVec3::ZERO, dirt);
        e.set_block(IVec3::X, dirt);
        assert_eq!(e.world.get(IVec3::ZERO), dirt);

        // Move top below: stone now wins.
        assert!(e.move_layer(top, false));
        assert_eq!(e.world.get(IVec3::ZERO), stone);
        e.undo();
        assert_eq!(e.world.get(IVec3::ZERO), dirt);
        assert!(!e.move_layer(top, true), "already on top");

        // Merge top into bottom: one layer, dirt on top of stone.
        e.merge_down(top).unwrap();
        assert_eq!(e.layers.layers().len(), 1);
        assert_eq!(e.world.get(IVec3::ZERO), dirt);
        assert_eq!(e.world.get(IVec3::X), dirt);
        assert_eq!(e.layers.layer(bottom).unwrap().cells.get(IVec3::X), dirt);
        e.undo();
        assert_eq!(e.layers.layers().len(), 2);
        assert_eq!(e.layers.layer(bottom).unwrap().cells.get(IVec3::X), BlockId::AIR);
        assert_eq!(e.world.get(IVec3::ZERO), dirt);
        e.redo();
        assert_eq!(e.layers.layers().len(), 1);

        // Delete the merged layer? It is the last one: refused.
        assert!(!e.delete_layer(bottom));
        e.undo();
        assert!(e.delete_layer(top));
        assert_eq!(e.world.get(IVec3::X), BlockId::AIR);
        e.undo();
        assert_eq!(e.world.get(IVec3::X), dirt);
        assert_eq!(e.layers.layers().len(), 2);
    }

    #[test]
    fn merging_into_a_protected_layer_is_refused() {
        let (mut e, _) = editor();
        let bottom = e.layers.active();
        let top = e.add_layer("top", LayerKind::Painted);
        e.set_locked(bottom, true);
        assert!(e.merge_down(top).is_err());
        assert!(e.merge_down(bottom).is_err(), "nothing below the bottom layer");
        assert_eq!(e.layers.layers().len(), 2);
    }

    #[test]
    fn adding_a_layer_with_content_is_one_undo_step() {
        let (mut e, stone) = editor();
        let id = e.add_layer_with("imported", LayerKind::Painted, &[(IVec3::new(3, 3, 3), stone)]);
        assert_eq!(e.layers.active(), id);
        assert_eq!(e.world.get(IVec3::new(3, 3, 3)), stone);
        e.undo();
        assert_eq!(e.layers.layers().len(), 1);
        assert_eq!(e.world.get(IVec3::new(3, 3, 3)), BlockId::AIR);
        assert!(!e.history.can_undo());
        e.redo();
        assert_eq!(e.world.get(IVec3::new(3, 3, 3)), stone);
        assert_eq!(e.layers.active(), id);
    }
}
