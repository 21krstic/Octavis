//! Editing state and operations, independent of egui so it can be tested.

use std::collections::HashSet;

use glam::IVec3;
use octavis_core::{BlockId, Brush, History, SECTION_SIZE, Selection, World};

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

pub struct Editor {
    pub world: World,
    pub history: History,
    pub selection: Selection,
    /// Sections whose mesh must be rebuilt before the next draw.
    dirty: HashSet<IVec3>,
    selection_dirty: bool,
}

impl Editor {
    pub fn new(world: World) -> Self {
        let dirty = world.sections().map(|(p, _)| p).collect();
        Self { world, history: History::new(), selection: Selection::new(), dirty, selection_dirty: true }
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

    /// Writes a block as part of the open edit (or untracked if none).
    pub fn set_block(&mut self, pos: IVec3, block: BlockId) {
        if self.history.set(&mut self.world, pos, block) != block {
            self.mark_dirty(pos);
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

    /// Whether `pos` was solid before the open edit (stroke) began.
    /// Strokes pick against this so their own changes do not move the target.
    pub fn was_solid(&self, pos: IVec3) -> bool {
        self.history.original(pos).unwrap_or_else(|| self.world.get(pos)) != BlockId::AIR
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

    /// Writes cells (for example a pasted schematic) as one undoable edit.
    pub fn paste(&mut self, cells: &[(IVec3, BlockId)]) {
        self.begin_edit();
        for &(p, b) in cells {
            self.set_block(p, b);
        }
        self.end_edit();
    }

    /// Swaps in a whole new world, discarding history and selection, and
    /// schedules a remesh of every old and new section.
    pub fn replace_world(&mut self, world: World) {
        let mut dirty: HashSet<IVec3> = self.world.sections().map(|(p, _)| p).collect();
        dirty.extend(world.sections().map(|(p, _)| p));
        self.world = world;
        self.history = History::new();
        self.selection = Selection::new();
        self.dirty = dirty;
        self.selection_dirty = true;
    }

    pub fn undo(&mut self) {
        let u = self.history.undo(&mut self.world, &mut self.selection);
        self.after_undo(u);
    }

    pub fn redo(&mut self) {
        let u = self.history.redo(&mut self.world, &mut self.selection);
        self.after_undo(u);
    }

    fn after_undo(&mut self, u: octavis_core::Undone) {
        for p in u.cells {
            self.mark_dirty(p);
        }
        self.selection_dirty |= u.selection_changed;
    }

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
        (Editor::new(w), stone)
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
    fn paste_is_one_undo_step() {
        let (mut e, stone) = editor();
        e.paste(&[(IVec3::new(1, 1, 1), stone), (IVec3::new(40, 0, 0), stone)]);
        assert_eq!(e.world.get(IVec3::new(40, 0, 0)), stone);
        e.undo();
        assert_eq!(e.world.get(IVec3::new(1, 1, 1)), BlockId::AIR);
        assert!(!e.history.can_undo());
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
        e.replace_world(fresh);
        let mut d = e.take_dirty();
        d.sort_by_key(|p| p.x);
        assert_eq!(d, vec![IVec3::new(-2, 0, 0), IVec3::new(6, 0, 0)]);
        assert!(e.selection.is_empty() && !e.history.can_undo());
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
}
