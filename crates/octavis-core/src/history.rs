use glam::IVec3;

use crate::block::BlockId;
use crate::hash::CellMap;
use crate::layers::{Layer, LayerId, LayerKind, LayerStack};
use crate::selection::{Selection, SelectionDelta};
use crate::world::World;

/// One changed cell: which layer, what it was and what it became.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub layer: LayerId,
    pub pos: IVec3,
    pub old: BlockId,
    pub new: BlockId,
}

/// A reversible structural change to the layer stack. Applying it twice
/// returns to the start, so the same call serves undo and redo.
#[derive(Debug)]
pub enum LayerCmd {
    /// The layer is in the stack when `stored` is `None`; otherwise it is
    /// held here and `index` says where it goes back.
    Presence { id: LayerId, index: usize, stored: Option<Layer> },
    /// Swap two layers' positions.
    Swap { a: usize, b: usize },
    /// Swap a generator layer's recipe with `other` (the value that is not
    /// currently in the layer).
    Recipe { layer: LayerId, other: Option<String> },
    /// Swap a layer's kind (painted / generator) with `other`.
    Kind { layer: LayerId, other: LayerKind },
}

impl LayerCmd {
    /// A layer that was just added to the stack at `index`.
    pub fn added(id: LayerId, index: usize) -> Self {
        LayerCmd::Presence { id, index, stored: None }
    }

    /// A layer that was just removed from `index` (moved here, not copied).
    pub fn removed(index: usize, layer: Layer) -> Self {
        LayerCmd::Presence { id: layer.id, index, stored: Some(layer) }
    }

    pub fn swapped(a: usize, b: usize) -> Self {
        LayerCmd::Swap { a, b }
    }

    /// A generator layer's recipe was just replaced; `old` is what it was.
    pub fn recipe_changed(layer: LayerId, old: Option<String>) -> Self {
        LayerCmd::Recipe { layer, other: old }
    }

    /// A layer's kind was just changed; `old` is what it was.
    pub fn kind_changed(layer: LayerId, old: LayerKind) -> Self {
        LayerCmd::Kind { layer, other: old }
    }

    /// Flips the command. `touched` collects the cells whose visible block
    /// may change as a result.
    fn toggle(&mut self, stack: &mut LayerStack, touched: &mut Vec<IVec3>) {
        match self {
            LayerCmd::Presence { id, index, stored } => match stored.take() {
                Some(layer) => {
                    // A layer coming back (redo of an add, undo of a delete)
                    // becomes the one being worked on.
                    let id = layer.id;
                    touched.extend(layer.cells.solid_cells().into_iter().map(|c| c.0));
                    stack.insert(*index, layer);
                    stack.set_active(id);
                }
                None => {
                    if let Some((i, layer)) = stack.remove(*id) {
                        touched.extend(layer.cells.solid_cells().into_iter().map(|c| c.0));
                        *index = i;
                        *stored = Some(layer);
                    }
                }
            },
            LayerCmd::Swap { a, b } => {
                let ids: Vec<LayerId> =
                    [*a, *b].iter().filter_map(|&i| stack.layers().get(i)).map(|l| l.id).collect();
                touched.extend(stack.cells_of(&ids));
                stack.swap(*a, *b);
            }
            LayerCmd::Recipe { layer, other } => {
                if let Some(l) = stack.layer_mut(*layer) {
                    std::mem::swap(&mut l.recipe, other);
                }
            }
            LayerCmd::Kind { layer, other } => {
                if let Some(l) = stack.layer_mut(*layer) {
                    std::mem::swap(&mut l.kind, other);
                }
            }
        }
    }
}

/// A user action (a brush stroke, a paste, a fill, a layer operation) as one
/// sparse diff.
#[derive(Debug, Default)]
struct Edit {
    changes: Vec<Change>,
    index: CellMap<(LayerId, IVec3), usize>,
    /// Structural layer changes, in the order they happened.
    cmds: Vec<LayerCmd>,
    /// Set for selection-only edits (like paint.net, selecting is undoable).
    selection: Option<SelectionDelta>,
}

impl Edit {
    /// Records a change, folding repeated writes to a cell into one entry
    /// (first `old`, latest `new`) so undo restores the true original.
    fn record(&mut self, layer: LayerId, pos: IVec3, old: BlockId, new: BlockId) {
        match self.index.get(&(layer, pos)) {
            Some(&i) => self.changes[i].new = new,
            None => {
                self.index.insert((layer, pos), self.changes.len());
                self.changes.push(Change { layer, pos, old, new });
            }
        }
    }

    fn is_noop(&self) -> bool {
        self.selection.is_none() && self.cmds.is_empty() && self.changes.iter().all(|c| c.old == c.new)
    }
}

/// What an undo or redo touched, so callers can refresh meshes and overlays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Undone {
    /// Block positions whose visible block changed.
    pub cells: Vec<IVec3>,
    pub selection_changed: bool,
    /// The layer stack changed shape.
    pub structure_changed: bool,
    /// Cells whose visible block may have changed because of that; recompute
    /// the composite there.
    pub restacked: Vec<IVec3>,
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

    /// Writes a block into `layer`, recording it if an edit is open.
    /// Returns true if the visible block at `pos` changed.
    pub fn set(
        &mut self,
        stack: &mut LayerStack,
        world: &mut World,
        layer: LayerId,
        pos: IVec3,
        block: BlockId,
    ) -> bool {
        let Some(write) = stack.write(world, layer, pos, block) else { return false };
        if let Some(edit) = &mut self.open {
            edit.record(layer, pos, write.old, block);
        }
        write.composite_changed
    }

    /// Value `pos` had in `layer` before the open edit touched it, or
    /// `None` if the open edit has not changed it (or there is no open edit).
    pub fn original(&self, layer: LayerId, pos: IVec3) -> Option<BlockId> {
        let edit = self.open.as_ref()?;
        edit.index.get(&(layer, pos)).map(|&i| edit.changes[i].old)
    }

    /// Adds a structural layer change (already applied) to the open edit.
    /// Does nothing if no edit is open.
    pub fn push_cmd(&mut self, cmd: LayerCmd) {
        if let Some(edit) = &mut self.open {
            edit.cmds.push(cmd);
        }
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

    /// Records changes that were already written (outside any open edit)
    /// as one undo step.
    pub fn record_applied(&mut self, changes: impl IntoIterator<Item = Change>) {
        self.commit();
        let mut edit = Edit::default();
        for c in changes {
            edit.record(c.layer, c.pos, c.old, c.new);
        }
        if !edit.is_noop() {
            self.undo.push(edit);
            self.redo.clear();
        }
    }

    /// Adds an already-applied selection change to the open edit, so cells
    /// and selection undo together. Does nothing if no edit is open.
    pub fn push_selection(&mut self, delta: SelectionDelta) {
        if let Some(edit) = &mut self.open {
            debug_assert!(edit.selection.is_none(), "one selection change per edit");
            if !delta.is_empty() {
                edit.selection = Some(delta);
            }
        }
    }

    /// Records an already-applied selection change as its own undo step.
    pub fn record_selection(&mut self, delta: SelectionDelta) {
        self.commit();
        if delta.is_empty() {
            return;
        }
        self.undo.push(Edit { selection: Some(delta), ..Edit::default() });
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Reverts the latest edit: blocks, layer structure or selection.
    pub fn undo(&mut self, stack: &mut LayerStack, world: &mut World, selection: &mut Selection) -> Undone {
        self.commit();
        let Some(mut edit) = self.undo.pop() else { return Undone::default() };
        let cells = apply(stack, world, &edit, |c| c.old);
        let mut restacked = Vec::new();
        for cmd in edit.cmds.iter_mut().rev() {
            cmd.toggle(stack, &mut restacked);
        }
        stack.fix_active();
        let selection_changed = edit.selection.as_ref().is_some_and(|d| {
            selection.revert_delta(d);
            true
        });
        let structure_changed = !edit.cmds.is_empty();
        self.redo.push(edit);
        Undone { cells, selection_changed, structure_changed, restacked }
    }

    /// Re-applies the latest undone edit.
    pub fn redo(&mut self, stack: &mut LayerStack, world: &mut World, selection: &mut Selection) -> Undone {
        self.commit();
        let Some(mut edit) = self.redo.pop() else { return Undone::default() };
        let mut restacked = Vec::new();
        for cmd in edit.cmds.iter_mut() {
            cmd.toggle(stack, &mut restacked);
        }
        stack.fix_active();
        let cells = apply(stack, world, &edit, |c| c.new);
        let selection_changed = edit.selection.as_ref().is_some_and(|d| {
            selection.apply_delta(d);
            true
        });
        let structure_changed = !edit.cmds.is_empty();
        self.undo.push(edit);
        Undone { cells, selection_changed, structure_changed, restacked }
    }
}

/// Writes one side of every change; returns where the visible block changed.
fn apply(
    stack: &mut LayerStack,
    world: &mut World,
    edit: &Edit,
    value: impl Fn(&Change) -> BlockId,
) -> Vec<IVec3> {
    edit.changes
        .iter()
        .filter(|c| c.old != c.new)
        .filter_map(|c| {
            let w = stack.write(world, c.layer, c.pos, value(c))?;
            w.composite_changed.then_some(c.pos)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockState;
    use crate::layers::LayerKind;

    struct Fixture {
        w: World,
        s: LayerStack,
        h: History,
        layer: LayerId,
        a: BlockId,
        b: BlockId,
    }

    impl Fixture {
        fn new() -> Self {
            let mut w = World::new();
            let a = w.blocks.intern(BlockState::new("a"));
            let b = w.blocks.intern(BlockState::new("b"));
            let s = LayerStack::single("base", crate::Grid::new());
            let layer = s.active();
            Self { w, s, h: History::new(), layer, a, b }
        }

        fn set(&mut self, pos: IVec3, block: BlockId) {
            self.h.set(&mut self.s, &mut self.w, self.layer, pos, block);
        }

        fn undo(&mut self, sel: &mut Selection) -> Undone {
            self.h.undo(&mut self.s, &mut self.w, sel)
        }

        fn redo(&mut self, sel: &mut Selection) -> Undone {
            self.h.redo(&mut self.s, &mut self.w, sel)
        }
    }

    #[test]
    fn undo_redo_roundtrip() {
        let mut f = Fixture::new();
        let mut sel = Selection::new();
        let p = IVec3::new(1, 2, 3);
        f.h.begin();
        f.set(p, f.a);
        f.h.commit();
        assert_eq!(f.w.get(p), f.a);
        assert_eq!(f.undo(&mut sel).cells, vec![p]);
        assert_eq!(f.w.get(p), BlockId::AIR);
        assert_eq!(f.redo(&mut sel).cells, vec![p]);
        assert_eq!(f.w.get(p), f.a);
    }

    #[test]
    fn repeated_writes_undo_to_original() {
        let mut f = Fixture::new();
        let mut sel = Selection::new();
        f.h.begin();
        f.set(IVec3::ZERO, f.a);
        f.set(IVec3::ZERO, f.b);
        f.set(IVec3::ZERO, f.a);
        f.h.commit();
        f.undo(&mut sel);
        assert_eq!(f.w.get(IVec3::ZERO), BlockId::AIR);
    }

    #[test]
    fn one_stroke_is_one_undo_step() {
        let mut f = Fixture::new();
        let mut sel = Selection::new();
        f.h.begin();
        for x in 0..50 {
            f.set(IVec3::new(x, 0, 0), f.a);
        }
        f.h.commit();
        assert_eq!(f.undo(&mut sel).cells.len(), 50);
        assert!(!f.h.can_undo());
        assert_eq!(f.w.section_count(), 4); // x 0..50 spans 4 sections; they linger until compact()
        f.w.compact();
        assert_eq!(f.w.section_count(), 0);
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut f = Fixture::new();
        let mut sel = Selection::new();
        f.h.begin();
        f.set(IVec3::ZERO, f.a);
        f.h.commit();
        f.undo(&mut sel);
        assert!(f.h.can_redo());
        f.h.begin();
        f.set(IVec3::ZERO, f.b);
        f.h.commit();
        assert!(!f.h.can_redo());
    }

    #[test]
    fn noop_edits_are_discarded() {
        let mut f = Fixture::new();
        f.h.begin();
        f.set(IVec3::ZERO, BlockId::AIR);
        f.h.commit();
        assert!(!f.h.can_undo());
        f.h.begin();
        f.set(IVec3::ZERO, f.a);
        f.set(IVec3::ZERO, BlockId::AIR);
        f.h.commit();
        assert!(!f.h.can_undo());
    }

    #[test]
    fn undo_with_nothing_is_empty() {
        let mut f = Fixture::new();
        let mut sel = Selection::new();
        assert!(f.undo(&mut sel).cells.is_empty());
        assert!(f.redo(&mut sel).cells.is_empty());
    }

    #[test]
    fn selection_changes_are_undoable_steps() {
        let mut f = Fixture::new();
        let mut sel = Selection::new();
        let target = Selection::cuboid(IVec3::ZERO, IVec3::new(2, 0, 0));

        f.h.begin();
        f.set(IVec3::new(9, 9, 9), f.a);
        f.h.commit();

        f.h.record_selection(sel.delta_to(&target));
        sel = target.clone();

        let u = f.undo(&mut sel);
        assert!(u.selection_changed && u.cells.is_empty());
        assert!(sel.is_empty());
        assert_eq!(f.w.get(IVec3::new(9, 9, 9)), f.a, "block edit untouched by selection undo");

        let u = f.redo(&mut sel);
        assert!(u.selection_changed);
        assert_eq!(sel, target);

        f.undo(&mut sel);
        let u = f.undo(&mut sel);
        assert!(!u.selection_changed);
        assert_eq!(f.w.get(IVec3::new(9, 9, 9)), BlockId::AIR);
    }

    #[test]
    fn empty_selection_delta_is_not_recorded() {
        let mut h = History::new();
        h.record_selection(SelectionDelta::default());
        assert!(!h.can_undo());
    }

    #[test]
    fn edits_on_different_layers_undo_independently_per_layer() {
        let mut f = Fixture::new();
        let mut sel = Selection::new();
        let upper = f.s.new_layer("upper", LayerKind::Painted);
        let upper_id = upper.id;
        f.s.insert(1, upper);
        let p = IVec3::ZERO;
        f.h.begin();
        f.set(p, f.a);
        f.h.set(&mut f.s, &mut f.w, upper_id, p, f.b);
        f.h.commit();
        assert_eq!(f.w.get(p), f.b);
        f.undo(&mut sel);
        assert_eq!(f.w.get(p), BlockId::AIR);
        assert_eq!(f.s.layer(upper_id).unwrap().cells.get(p), BlockId::AIR);
        f.redo(&mut sel);
        assert_eq!(f.w.get(p), f.b);
        assert_eq!(f.s.layer(f.layer).unwrap().cells.get(p), f.a);
    }

    #[test]
    fn adding_a_layer_with_content_is_one_undoable_step() {
        let mut f = Fixture::new();
        let mut sel = Selection::new();
        f.h.begin();
        let layer = f.s.new_layer("imported", LayerKind::Painted);
        let id = layer.id;
        f.s.insert(1, layer);
        f.h.push_cmd(LayerCmd::added(id, 1));
        f.h.set(&mut f.s, &mut f.w, id, IVec3::new(4, 4, 4), f.a);
        f.h.commit();
        assert_eq!(f.s.layers().len(), 2);

        let u = f.undo(&mut sel);
        assert!(u.structure_changed);
        assert_eq!(f.s.layers().len(), 1);
        f.s.rebuild(&mut f.w);
        assert_eq!(f.w.get(IVec3::new(4, 4, 4)), BlockId::AIR);

        let u = f.redo(&mut sel);
        assert!(u.structure_changed);
        assert_eq!(f.s.layers().len(), 2);
        f.s.rebuild(&mut f.w);
        assert_eq!(f.w.get(IVec3::new(4, 4, 4)), f.a);
        assert_eq!(f.s.layer(id).unwrap().cells.get(IVec3::new(4, 4, 4)), f.a);
    }

    #[test]
    fn deleting_a_layer_moves_it_into_history_and_back() {
        let mut f = Fixture::new();
        let mut sel = Selection::new();
        let mut upper = f.s.new_layer("upper", LayerKind::Painted);
        upper.cells.set(IVec3::ONE, f.a);
        let id = upper.id;
        f.s.insert(1, upper);
        f.s.rebuild(&mut f.w);

        let (index, layer) = f.s.remove(id).unwrap();
        f.h.begin();
        f.h.push_cmd(LayerCmd::removed(index, layer));
        f.h.commit();
        f.s.rebuild(&mut f.w);
        assert_eq!(f.w.get(IVec3::ONE), BlockId::AIR);

        f.undo(&mut sel);
        f.s.rebuild(&mut f.w);
        assert_eq!(f.w.get(IVec3::ONE), f.a);
        assert_eq!(f.s.index_of(id), Some(1));
        f.redo(&mut sel);
        assert!(f.s.layer(id).is_none());
    }

    #[test]
    fn recipe_changes_undo_and_redo_with_the_cells() {
        let mut f = Fixture::new();
        let mut sel = Selection::new();
        f.s.layer_mut(f.layer).unwrap().recipe = Some("v1".into());
        f.h.begin();
        f.set(IVec3::ZERO, f.a);
        let old = f.s.layer_mut(f.layer).unwrap().recipe.replace("v2".into());
        f.h.push_cmd(LayerCmd::recipe_changed(f.layer, old));
        f.h.commit();
        let recipe = |f: &Fixture| f.s.layer(f.layer).unwrap().recipe.clone();
        f.undo(&mut sel);
        assert_eq!((recipe(&f), f.w.get(IVec3::ZERO)), (Some("v1".into()), BlockId::AIR));
        f.redo(&mut sel);
        assert_eq!((recipe(&f), f.w.get(IVec3::ZERO)), (Some("v2".into()), f.a));
    }

    #[test]
    fn kind_changes_undo_and_redo() {
        let mut f = Fixture::new();
        let mut sel = Selection::new();
        f.s.layer_mut(f.layer).unwrap().kind = LayerKind::Generator;
        f.h.begin();
        f.s.layer_mut(f.layer).unwrap().kind = LayerKind::Painted;
        f.h.push_cmd(LayerCmd::kind_changed(f.layer, LayerKind::Generator));
        f.h.commit();
        let kind = |f: &Fixture| f.s.layer(f.layer).unwrap().kind;
        f.undo(&mut sel);
        assert_eq!(kind(&f), LayerKind::Generator);
        f.redo(&mut sel);
        assert_eq!(kind(&f), LayerKind::Painted);
    }

    #[test]
    fn swaps_undo_and_redo() {
        let mut f = Fixture::new();
        let mut sel = Selection::new();
        let upper = f.s.new_layer("upper", LayerKind::Painted);
        let id = upper.id;
        f.s.insert(1, upper);
        f.s.swap(0, 1);
        f.h.begin();
        f.h.push_cmd(LayerCmd::swapped(0, 1));
        f.h.commit();
        assert_eq!(f.s.index_of(id), Some(0));
        f.undo(&mut sel);
        assert_eq!(f.s.index_of(id), Some(1));
        f.redo(&mut sel);
        assert_eq!(f.s.index_of(id), Some(0));
    }
}
