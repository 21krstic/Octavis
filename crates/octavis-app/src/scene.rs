use glam::IVec3;
use octavis_core::{BlockState, World};

/// Throwaway demo content so the viewport has something to show until
/// real tools and file import exist.
pub fn demo_world() -> World {
    let mut w = World::new();
    let grass = w.blocks.intern(BlockState::new("minecraft:grass_block"));
    let dirt = w.blocks.intern(BlockState::new("minecraft:dirt"));
    let stone = w.blocks.intern(BlockState::new("minecraft:stone"));
    let planks = w.blocks.intern(BlockState::new("minecraft:oak_planks"));

    for x in -24..24 {
        for z in -24..24 {
            w.set(IVec3::new(x, 0, z), grass);
            w.set(IVec3::new(x, -1, z), dirt);
            w.set(IVec3::new(x, -2, z), stone);
        }
    }
    // A sphere and a few pillars crossing section borders.
    let c = IVec3::new(0, 8, 0);
    for p in cube(c, 7) {
        if (p - c).as_vec3().length() <= 6.5 {
            w.set(p, stone);
        }
    }
    for i in 0..4 {
        for y in 1..12 {
            w.set(IVec3::new(-15 + i * 10, y, 14), planks);
        }
    }
    w.compact();
    w
}

fn cube(center: IVec3, r: i32) -> impl Iterator<Item = IVec3> {
    (-r..=r).flat_map(move |x| {
        (-r..=r).flat_map(move |y| (-r..=r).map(move |z| center + IVec3::new(x, y, z)))
    })
}
