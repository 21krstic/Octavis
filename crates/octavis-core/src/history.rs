use std::collections::HashMap;

use glam::IVec3;

use crate::block::BlockId;
use crate::world::World;

/// One changed cell: what it was and what it became.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub pos: IVec3,
    pub old: BlockId,
    pub new: BlockId,
}

/// A user action (a brush stroke, a paste, a fill) as one sparse diff.
#[derive(Clone, Debug, Default)]
struct Edit {
    changes: Vec<Change>,
    index: HashMap<IVec3, usize>,
}

impl Edit {
    /// Records a change, folding repeated writes to a cell into one entry
    /// (first `old`, latest `new`) so undo restores the true original.
    fn record(&mut self, pos: IVec3, old: BlockId, new: BlockId) {
        match self.index.get(&pos) {
            Some(&i) => self.changes[i].new = new,
            None => {
                self.index.insert(pos, self.changes.len());
                self.changes.push(Change { pos, old, new });
            }
        }
    }

    fn is_noop(&self) -> bool {
        self.changes.iter().all(|c| c.old == c.new)
    }
}

/// Undo/redo via the command pattern. Edits go through [`History::set`]
/// between [`History::begin`] and [`History::commit`]; undo and redo return
/// the positions they touched so callers can invalidate meshes.
#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    open: Option<Edit>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts grouping writes into one undoable action. Any edit left open
    /// is committed first.
    pub fn begin(&mut self) {
        self.commit();
        self.open = Some(Edit::default());
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Writes a block, recording it if an edit is open. Returns the old value.
    pub fn set(&mut self, world: &mut World, pos: IVec3, block: BlockId) -> BlockId {
        let old = world.set(pos, block);
        if let Some(edit) = &mut self.open {
            edit.record(pos, old, block);
        }
        old
    }

    /// Closes the open edit. Empty or no-op edits are discarded. A real
    /// edit clears the redo stack.
    pub fn commit(&mut self) {
        if let Some(edit) = self.open.take() {
            if !edit.is_noop() {
                self.undo.push(edit);
                self.redo.clear();
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Reverts the latest edit; returns the positions that changed.
    pub fn undo(&mut self, world: &mut World) -> Vec<IVec3> {
        self.commit();
        let Some(edit) = self.undo.pop() else { return Vec::new() };
        let touched = apply(world, &edit, |c| c.old);
        self.redo.push(edit);
        touched
    }

    /// Re-applies the latest undone edit; returns the positions that changed.
    pub fn redo(&mut self, world: &mut World) -> Vec<IVec3> {
        self.commit();
        let Some(edit) = self.redo.pop() else { return Vec::new() };
        let touched = apply(world, &edit, |c| c.new);
        self.undo.push(edit);
        touched
    }
}

fn apply(world: &mut World, edit: &Edit, value: impl Fn(&Change) -> BlockId) -> Vec<IVec3> {
    edit.changes
        .iter()
        .filter(|c| c.old != c.new)
        .map(|c| {
            world.set(c.pos, value(c));
            c.pos
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockState;

    fn setup() -> (World, History, BlockId, BlockId) {
        let mut w = World::new();
        let a = w.blocks.intern(BlockState::new("a"));
        let b = w.blocks.intern(BlockState::new("b"));
        (w, History::new(), a, b)
    }

    #[test]
    fn undo_redo_roundtrip() {
        let (mut w, mut h, a, _) = setup();
        let p = IVec3::new(1, 2, 3);
        h.begin();
        h.set(&mut w, p, a);
        h.commit();
        assert_eq!(w.get(p), a);
        assert_eq!(h.undo(&mut w), vec![p]);
        assert_eq!(w.get(p), BlockId::AIR);
        assert_eq!(h.redo(&mut w), vec![p]);
        assert_eq!(w.get(p), a);
    }

    #[test]
    fn repeated_writes_undo_to_original() {
        let (mut w, mut h, a, b) = setup();
        let p = IVec3::ZERO;
        h.begin();
        h.set(&mut w, p, a);
        h.set(&mut w, p, b);
        h.set(&mut w, p, a);
        h.commit();
        h.undo(&mut w);
        assert_eq!(w.get(p), BlockId::AIR);
    }

    #[test]
    fn one_stroke_is_one_undo_step() {
        let (mut w, mut h, a, _) = setup();
        h.begin();
        for x in 0..50 {
            h.set(&mut w, IVec3::new(x, 0, 0), a);
        }
        h.commit();
        assert_eq!(h.undo(&mut w).len(), 50);
        assert!(!h.can_undo());
        assert_eq!(w.section_count(), 4); // x 0..50 spans 4 sections; they linger until compact()
        w.compact();
        assert_eq!(w.section_count(), 0);
    }

    #[test]
    fn new_edit_clears_redo() {
        let (mut w, mut h, a, b) = setup();
        h.begin();
        h.set(&mut w, IVec3::ZERO, a);
        h.commit();
        h.undo(&mut w);
        assert!(h.can_redo());
        h.begin();
        h.set(&mut w, IVec3::ZERO, b);
        h.commit();
        assert!(!h.can_redo());
    }

    #[test]
    fn noop_edits_are_discarded() {
        let (mut w, mut h, a, _) = setup();
        h.begin();
        h.set(&mut w, IVec3::ZERO, BlockId::AIR);
        h.commit();
        assert!(!h.can_undo());
        h.begin();
        h.set(&mut w, IVec3::ZERO, a);
        h.set(&mut w, IVec3::ZERO, BlockId::AIR);
        h.commit();
        assert!(!h.can_undo());
    }

    #[test]
    fn undo_with_nothing_is_empty() {
        let (mut w, mut h, _, _) = setup();
        assert!(h.undo(&mut w).is_empty());
        assert!(h.redo(&mut w).is_empty());
    }
}
