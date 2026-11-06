//! Headless CPU rendering of PRC 3D models (ISO 14739-1) to an RGBA image.
//!
//! Layer L3: depends only on `pdfcraft-prc`. [`assemble`] turns a parsed [`pdfcraft_prc::PrcFile`]
//! into world-space meshes; [`render`] draws them from a [`Camera`] with one depth-tested sample per
//! pixel centre and a light at the camera; [`pick`] and [`PickIndex`] find the triangle under an
//! image position. Single-threaded and GUI-free, so it runs unchanged on wasm32.
//! The image size is capped by [`MAX_RENDER_PIXELS`], the drawing work by [`MAX_RENDER_WORK`] and the
//! assembled model by [`MAX_ASSEMBLED_TRIANGLES`] and [`MAX_ASSEMBLED_VERTICES`]; bad input is an
//! error, never a panic.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod assemble;
mod pick;
mod pick_index;
mod raster;
mod render;
mod vec3;

pub use assemble::{MAX_ASSEMBLED_TRIANGLES, MAX_ASSEMBLED_VERTICES, WorldMesh, assemble, place};
pub use pick::{Hit, Ray, pick};
pub use pick_index::PickIndex;
pub use render::{Bounds, Camera, Image, Lighting, MAX_RENDER_PIXELS, MAX_RENDER_WORK, Projection, RenderError, RenderOptions, render};
