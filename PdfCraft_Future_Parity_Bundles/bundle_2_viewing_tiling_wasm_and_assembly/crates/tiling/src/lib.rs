//! PdfCraft Tiling: Deep zoom multi-scale pyramid tile cache and viewport culler.
//!
//! Subsystem covering Milestone M3:
//! - [P0] Tiled rendering keeps deep zoom sharp (up to 6400%)

pub mod tile_coord;
pub mod pyramid;
pub mod viewport;

pub use tile_coord::{Rect2D, TileCoordinate};
pub use pyramid::{RenderedTile, TilePyramidCache};
pub use viewport::ViewportCuller;
