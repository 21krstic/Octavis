//! Core voxel engine. Depends on nothing but math: no rendering, UI, or
//! Minecraft-version knowledge. Everything else consumes this crate.

pub mod block;
pub mod section;
pub mod world;

pub use block::{BlockId, BlockState, BlockTable};
pub use section::{SECTION_SIZE, SECTION_VOLUME, Section};
pub use world::World;
