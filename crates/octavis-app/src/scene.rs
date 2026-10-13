use glam::IVec3;
use octavis_core::{BlockState, World};

/// Half the platform's side length; the default platform is 480x480.
const HALF: i32 = 240;

/// Default starting scene: a flat three-layer platform, so tools have
/// something to work on until file import exists.
pub fn demo_world() -> World {
    let mut w = World::new();
    let grass = w.blocks.intern(BlockState::new("minecraft:grass_block"));
    let dirt = w.blocks.intern(BlockState::new("minecraft:dirt"));
    let stone = w.blocks.intern(BlockState::new("minecraft:stone"));
    // Interned so they appear in the block picker.
    w.blocks.intern(BlockState::new("minecraft:oak_planks"));
    w.blocks.intern(BlockState::new("minecraft:sand"));
    w.blocks.intern(BlockState::new("minecraft:bricks"));

    for x in -HALF..HALF {
        for z in -HALF..HALF {
            w.set(IVec3::new(x, 0, z), grass);
            w.set(IVec3::new(x, -1, z), dirt);
            w.set(IVec3::new(x, -2, z), stone);
        }
    }
    w.compact();
    w
}
