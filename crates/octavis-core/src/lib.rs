//! Core voxel engine. Depends on nothing but math: no rendering, UI, or
//! Minecraft-version knowledge. Everything else consumes this crate.

pub mod block;
pub mod brush;
pub mod hash;
pub mod history;
pub mod layers;
pub mod orient;
pub mod raycast;
pub mod section;
pub mod selection;
pub mod transform;
pub mod world;

pub use block::{BlockId, BlockState, BlockTable};
pub use brush::{Brush, BrushMode, Cmp, Mask, Pattern, Shape};
pub use hash::{CellMap, CellSet};
pub use history::{Change, History, LayerCmd, Undone};
pub use layers::{CellWrite, Layer, LayerId, LayerKind, LayerStack};
pub use raycast::{RayHit, raycast, raycast_with};
pub use orient::AxisMap;
pub use section::{SECTION_SIZE, SECTION_VOLUME, Section};
pub use selection::{Connectivity, Selection, SelectionDelta};
pub use transform::Transform;
pub use world::{Grid, World};
