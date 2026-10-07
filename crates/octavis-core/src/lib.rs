//! Core voxel engine. Depends on nothing but math: no rendering, UI, or
//! Minecraft-version knowledge. Everything else consumes this crate.

pub mod block;
pub mod brush;
pub mod history;
pub mod raycast;
pub mod section;
pub mod selection;
pub mod world;

pub use block::{BlockId, BlockState, BlockTable};
pub use brush::{Brush, BrushMode, Cmp, Mask, Pattern, Shape};
pub use history::{Change, History, Undone};
pub use raycast::{RayHit, raycast};
pub use section::{SECTION_SIZE, SECTION_VOLUME, Section};
pub use selection::{Connectivity, Selection, SelectionDelta};
pub use world::World;
