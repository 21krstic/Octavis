//! Editing state and operations, independent of egui so it can be tested.

use std::collections::HashSet;

use glam::IVec3;
use octavis_core::{BlockId, History, SECTION_SIZE, Selection, World};

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
}

impl Editor {
    pub fn new(world: World) -> Self {
        let dirty = world.sections().map(|(p, _)| p).collect();
        Self { world, history: History::new(), selection: Selection::new(), dirty }
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

    /// Fills every selected cell as one undoable edit.
    pub fn fill_selection(&mut self, block: BlockId) {
        let cells: Vec<IVec3> = self.selection.iter().collect();
        self.begin_edit();
        for p in cells {
            self.set_block(p, block);
        }
        self.end_edit();
    }

    pub fn undo(&mut self) {
        for p in self.history.undo(&mut self.world) {
            self.mark_dirty(p);
        }
    }

    pub fn redo(&mut self) {
        for p in self.history.redo(&mut self.world) {
            self.mark_dirty(p);
        }
    }

    pub fn apply_selection(&mut self, mode: SelectMode, new: &Selection) {
        match mode {
            SelectMode::Replace => self.selection = new.clone(),
            SelectMode::Add => self.selection.union(new),
            SelectMode::Subtract => self.selection.subtract(new),
            SelectMode::Intersect => self.selection.intersect(new),
        }
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
}
