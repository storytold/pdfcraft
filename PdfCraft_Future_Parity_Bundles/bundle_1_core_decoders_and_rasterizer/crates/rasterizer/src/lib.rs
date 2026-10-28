//! PdfCraft Rasterizer: 2D vector graphics rasterizer, display list compositor,
//! blend modes, shadings, and Type 3 font interpreter for PDF.
//!
//! Subsystem covering Milestone M2:
//! - [P0] Page rasterizer (text, vectors, images)
//! - [P0] Shadings, gradients, and tiling patterns
//! - [P0] Blend modes, soft masks, and transparency groups
//! - [P0] Type 3 fonts
//! - [P0] CID fonts and CMaps (CJK without ToUnicode)

pub mod blend_modes;
pub mod shadings;
pub mod graphics_state;
pub mod type3_fonts;
pub mod cjk_cmap;
pub mod renderer;

pub use blend_modes::{BlendMode, PixelBuffer};
pub use shadings::{AxialShading, ColorStop, RadialShading, ShadingRenderer, TilingPattern};
pub use graphics_state::{GraphicsState, Matrix2D, PathSegment, VectorPath};
pub use type3_fonts::{Type3Font, Type3Glyph};
pub use cjk_cmap::{CjkCMapRegistry, CjkOrdering};
pub use renderer::{PageRasterizer, RenderError};
