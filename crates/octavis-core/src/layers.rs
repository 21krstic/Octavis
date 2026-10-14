//! Layers: stacked sparse grids, topmost non-air block wins.
//!
//! Each layer owns its cells. The editor keeps a *composite* [`World`]
//! (what is visible, and what tools, meshing and picking read); every
//! write goes through [`LayerStack::write`], which updates the layer and
//! the composite cell together. Structural changes (visibility, solo,
//! order, removal) call [`LayerStack::rebuild`] instead.
//!
//! A layer is either `Painted` (edited by tools) or `Generator` (its cells
//! are the cached result of a recipe kept by the app; tools cannot edit it,
//! so regenerating never destroys hand work).

use std::collections::HashSet;

use glam::IVec3;

use crate::{BlockId, Grid, World};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LayerId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerKind {
    Painted,
    Generator,
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub kind: LayerKind,
    pub cells: Grid,
}

/// Result of writing one cell into a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellWrite {
    /// The layer's previous value at the cell.
    pub old: BlockId,
    /// True if the visible (composite) block at the cell changed.
    pub composite_changed: bool,
}

#[derive(Clone, Debug)]
pub struct LayerStack {
    /// Bottom first.
    layers: Vec<Layer>,
    active: LayerId,
    solo: Option<LayerId>,
    next_id: u32,
}

impl LayerStack {
    /// A stack with one painted layer holding `cells`.
    pub fn single(name: impl Into<String>, cells: Grid) -> Self {
        let layer = Layer {
            id: LayerId(0),
            name: name.into(),
            visible: true,
            locked: false,
            kind: LayerKind::Painted,
            cells,
        };
        Self { active: layer.id, layers: vec![layer], solo: None, next_id: 1 }
    }

    /// Layers, bottom first.
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }

    pub fn index_of(&self, id: LayerId) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    pub fn active(&self) -> LayerId {
        self.active
    }

    pub fn set_active(&mut self, id: LayerId) {
        if self.layer(id).is_some() {
            self.active = id;
        }
    }

    pub fn solo(&self) -> Option<LayerId> {
        self.solo
    }

    pub fn set_solo(&mut self, solo: Option<LayerId>) {
        self.solo = solo.filter(|&id| self.layer(id).is_some());
    }

    /// True if the layer contributes to the composite.
    pub fn shown(&self, layer: &Layer) -> bool {
        match self.solo {
            Some(s) => layer.id == s,
            None => layer.visible,
        }
    }

    /// True if tools may write to the layer: it exists, is shown, unlocked
    /// and painted (not a generator result).
    pub fn editable(&self, id: LayerId) -> bool {
        self.layer(id).is_some_and(|l| self.shown(l) && !l.locked && l.kind == LayerKind::Painted)
    }

    /// Why the layer cannot be edited, for the UI.
    pub fn why_not_editable(&self, id: LayerId) -> Option<&'static str> {
        let l = self.layer(id)?;
        if l.kind == LayerKind::Generator {
            Some("Generator layers are read-only; bake the layer to edit it")
        } else if l.locked {
            Some("The active layer is locked")
        } else if !self.shown(l) {
            Some("The active layer is hidden")
        } else {
            None
        }
    }

    /// The visible block at `pos`: topmost shown layer with a non-air cell.
    pub fn composite_at(&self, pos: IVec3) -> BlockId {
        for layer in self.layers.iter().rev() {
            if self.shown(layer) {
                let b = layer.cells.get(pos);
                if b != BlockId::AIR {
                    return b;
                }
            }
        }
        BlockId::AIR
    }

    /// The visible block at `pos` as if layer `id` held `value` there.
    /// Lets a stroke keep picking against the scene as it was before it
    /// began.
    pub fn composite_with(&self, pos: IVec3, id: LayerId, value: BlockId) -> BlockId {
        for layer in self.layers.iter().rev() {
            if self.shown(layer) {
                let b = if layer.id == id { value } else { layer.cells.get(pos) };
                if b != BlockId::AIR {
                    return b;
                }
            }
        }
        BlockId::AIR
    }

    /// Writes one cell into a layer and keeps the composite in step.
    /// `None` if the layer does not exist.
    pub fn write(&mut self, world: &mut World, id: LayerId, pos: IVec3, block: BlockId) -> Option<CellWrite> {
        let layer = self.layers.iter_mut().find(|l| l.id == id)?;
        let old = layer.cells.set(pos, block);
        if old == block {
            return Some(CellWrite { old, composite_changed: false });
        }
        let visible = self.composite_at(pos);
        let composite_changed = world.get(pos) != visible;
        if composite_changed {
            world.set(pos, visible);
        }
        Some(CellWrite { old, composite_changed })
    }

    /// Creates a layer (not yet inserted) with a fresh id.
    pub fn new_layer(&mut self, name: impl Into<String>, kind: LayerKind) -> Layer {
        let id = LayerId(self.next_id);
        self.next_id += 1;
        Layer { id, name: name.into(), visible: true, locked: false, kind, cells: Grid::new() }
    }

    /// Inserts a layer at `index` (bottom = 0), clamped to the stack.
    pub fn insert(&mut self, index: usize, layer: Layer) {
        self.next_id = self.next_id.max(layer.id.0 + 1);
        let at = index.min(self.layers.len());
        self.layers.insert(at, layer);
    }

    /// Removes a layer and returns it with its index. The last remaining
    /// layer cannot be removed. The active layer and solo are kept valid.
    pub fn remove(&mut self, id: LayerId) -> Option<(usize, Layer)> {
        if self.layers.len() <= 1 {
            return None;
        }
        let index = self.index_of(id)?;
        let layer = self.layers.remove(index);
        if self.active == id {
            self.active = self.layers[index.saturating_sub(1).min(self.layers.len() - 1)].id;
        }
        if self.solo == Some(id) {
            self.solo = None;
        }
        Some((index, layer))
    }

    /// Re-validates the active layer after structural undo/redo.
    pub fn fix_active(&mut self) {
        if self.layer(self.active).is_none() {
            self.active = self.layers.last().map_or(LayerId(0), |l| l.id);
        }
        if let Some(s) = self.solo {
            if self.layer(s).is_none() {
                self.solo = None;
            }
        }
    }

    /// Swaps two layers' positions.
    pub fn swap(&mut self, a: usize, b: usize) {
        if a < self.layers.len() && b < self.layers.len() {
            self.layers.swap(a, b);
        }
    }

    /// Recomputes the whole composite. Returns the section positions that
    /// may have changed (old and new), for remeshing.
    pub fn rebuild(&self, world: &mut World) -> Vec<IVec3> {
        let mut touched: HashSet<IVec3> = world.sections().map(|(p, _)| p).collect();
        world.clear();
        for layer in &self.layers {
            if self.shown(layer) {
                for (pos, b) in layer.cells.solid_cells() {
                    world.set(pos, b);
                }
            }
        }
        touched.extend(world.sections().map(|(p, _)| p));
        touched.into_iter().collect()
    }

    /// What merging `upper` into the layer below would change there:
    /// `(lower layer, [(pos, old, new)])`. `None` if there is no layer below.
    pub fn merge_plan(&self, upper: LayerId) -> Option<(LayerId, Vec<(IVec3, BlockId, BlockId)>)> {
        let i = self.index_of(upper)?;
        let lower = self.layers.get(i.checked_sub(1)?)?;
        let changes = self.layers[i]
            .cells
            .solid_cells()
            .into_iter()
            .filter_map(|(pos, new)| {
                let old = lower.cells.get(pos);
                (old != new).then_some((pos, old, new))
            })
            .collect();
        Some((lower.id, changes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: BlockId = BlockId(1);
    const B: BlockId = BlockId(2);

    fn stack() -> (LayerStack, World, LayerId, LayerId) {
        let mut s = LayerStack::single("bottom", Grid::new());
        let bottom = s.active();
        let top = s.new_layer("top", LayerKind::Painted);
        let top_id = top.id;
        s.insert(1, top);
        (s, World::new(), bottom, top_id)
    }

    #[test]
    fn topmost_non_air_wins_and_erasing_reveals_below() {
        let (mut s, mut w, bottom, top) = stack();
        let p = IVec3::new(3, -2, 5);
        s.write(&mut w, bottom, p, A);
        assert_eq!(w.get(p), A);
        let r = s.write(&mut w, top, p, B).unwrap();
        assert!(r.composite_changed && w.get(p) == B);
        // Writing the same value again changes nothing.
        assert!(!s.write(&mut w, top, p, B).unwrap().composite_changed);
        // Erasing the top cell reveals the layer underneath.
        s.write(&mut w, top, p, BlockId::AIR);
        assert_eq!(w.get(p), A);
        // A write below an opaque cell does not change what is visible.
        s.write(&mut w, top, p, B);
        assert!(!s.write(&mut w, bottom, p, B).unwrap().composite_changed);
    }

    #[test]
    fn hiding_and_solo_change_the_composite_after_rebuild() {
        let (mut s, mut w, bottom, top) = stack();
        s.write(&mut w, bottom, IVec3::ZERO, A);
        s.write(&mut w, top, IVec3::X, B);
        s.layer_mut(top).unwrap().visible = false;
        let touched = s.rebuild(&mut w);
        assert!(!touched.is_empty());
        assert_eq!((w.get(IVec3::ZERO), w.get(IVec3::X)), (A, BlockId::AIR));
        s.layer_mut(top).unwrap().visible = true;
        s.set_solo(Some(top));
        s.rebuild(&mut w);
        assert_eq!((w.get(IVec3::ZERO), w.get(IVec3::X)), (BlockId::AIR, B));
        assert!(!s.editable(bottom) && s.editable(top));
        s.set_solo(None);
        s.rebuild(&mut w);
        assert_eq!((w.get(IVec3::ZERO), w.get(IVec3::X)), (A, B));
    }

    #[test]
    fn editability_rules() {
        let (mut s, _, bottom, top) = stack();
        assert!(s.editable(top));
        s.layer_mut(top).unwrap().locked = true;
        assert!(!s.editable(top) && s.why_not_editable(top).is_some());
        s.layer_mut(top).unwrap().locked = false;
        s.layer_mut(top).unwrap().kind = LayerKind::Generator;
        assert!(!s.editable(top));
        s.layer_mut(bottom).unwrap().visible = false;
        assert!(!s.editable(bottom));
        assert_eq!(s.why_not_editable(LayerId(99)), None);
    }

    #[test]
    fn remove_keeps_the_active_layer_valid_and_protects_the_last_layer() {
        let (mut s, _, bottom, top) = stack();
        s.set_active(top);
        s.set_solo(Some(top));
        let (index, layer) = s.remove(top).unwrap();
        assert_eq!(index, 1);
        assert_eq!(s.active(), bottom);
        assert_eq!(s.solo(), None);
        assert!(s.remove(bottom).is_none(), "the last layer stays");
        s.insert(index, layer);
        assert_eq!(s.layers().len(), 2);
        assert_eq!(s.index_of(top), Some(1));
    }

    #[test]
    fn swap_reorders_priority() {
        let (mut s, mut w, bottom, top) = stack();
        s.write(&mut w, bottom, IVec3::ZERO, A);
        s.write(&mut w, top, IVec3::ZERO, B);
        assert_eq!(w.get(IVec3::ZERO), B);
        s.swap(0, 1);
        s.rebuild(&mut w);
        assert_eq!(w.get(IVec3::ZERO), A);
    }

    #[test]
    fn merge_plan_lists_changes_on_the_lower_layer() {
        let (mut s, mut w, bottom, top) = stack();
        s.write(&mut w, bottom, IVec3::ZERO, A);
        s.write(&mut w, bottom, IVec3::X, A);
        s.write(&mut w, top, IVec3::ZERO, B);
        s.write(&mut w, top, IVec3::X, A); // same as below: no change
        s.write(&mut w, top, IVec3::Y, B);
        let (lower, mut changes) = s.merge_plan(top).unwrap();
        assert_eq!(lower, bottom);
        changes.sort_by_key(|c| c.0.to_array());
        assert_eq!(changes, vec![(IVec3::ZERO, A, B), (IVec3::Y, BlockId::AIR, B)]);
        assert!(s.merge_plan(bottom).is_none());
    }

    #[test]
    fn rebuild_matches_incremental_writes() {
        let (mut s, mut w, bottom, top) = stack();
        for i in 0..40 {
            let p = IVec3::new(i * 3 - 50, i % 7, -i);
            s.write(&mut w, if i % 2 == 0 { bottom } else { top }, p, if i % 3 == 0 { A } else { B });
        }
        let incremental = w.solid_cells();
        let mut fresh = World::new();
        s.rebuild(&mut fresh);
        let mut a = incremental;
        let mut b = fresh.solid_cells();
        a.sort_by_key(|c| c.0.to_array());
        b.sort_by_key(|c| c.0.to_array());
        assert_eq!(a, b);
    }
}
