use std::collections::{BTreeMap, HashMap};

/// Interned handle to a [`BlockState`] within one [`BlockTable`].
/// `BlockId::AIR` is always id 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub u32);

impl BlockId {
    pub const AIR: BlockId = BlockId(0);
}

/// A namespaced block state, e.g. `minecraft:oak_stairs[facing=north]`.
/// Properties are sorted so equal states compare and hash equal.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlockState {
    pub name: String,
    pub properties: BTreeMap<String, String>,
}

impl BlockState {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), properties: BTreeMap::new() }
    }

    pub fn with(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.properties.insert(key.into(), value.into());
        self
    }
}

/// Interner mapping block states to compact ids. Section palettes store
/// these ids, so format-specific names never leak into voxel storage.
#[derive(Clone, Debug)]
pub struct BlockTable {
    states: Vec<BlockState>,
    lookup: HashMap<BlockState, BlockId>,
}

impl Default for BlockTable {
    fn default() -> Self {
        let air = BlockState::new("minecraft:air");
        Self { lookup: HashMap::from([(air.clone(), BlockId::AIR)]), states: vec![air] }
    }
}

impl BlockTable {
    pub fn intern(&mut self, state: BlockState) -> BlockId {
        if let Some(&id) = self.lookup.get(&state) {
            return id;
        }
        let id = BlockId(self.states.len() as u32);
        self.lookup.insert(state.clone(), id);
        self.states.push(state);
        id
    }

    pub fn get(&self, id: BlockId) -> Option<&BlockState> {
        self.states.get(id.0 as usize)
    }

    /// Number of interned states (always at least 1: air).
    pub fn len(&self) -> usize {
        self.states.len()
    }
}
