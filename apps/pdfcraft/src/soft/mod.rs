//! PdfCraft drawn on the processor, for Remote Desktop sessions without a graphics processor.
//!
//! On such a machine (an Azure Virtual Desktop host, say) wgpu still starts: Direct3D 12 offers
//! WARP, a software adapter. But a GPU renderer draws the whole window for every frame and
//! presents it whole, and Remote Desktop then encodes and sends the whole window for every frame.
//! Scrolling a page there was far slower than in viewers that paint with GDI, which tell the
//! system the rectangles they changed.
//!
//! This renderer does what those do:
//!
//! - [`damage`] finds the parts of the window that differ from the last frame;
//! - [`raster`] draws only those, on the processor, copying page tiles row by row;
//! - [`painter`] keeps the window's pixels between frames and presents only those rectangles;
//! - [`runner`] owns the window and the event loop, as eframe does for the GPU renderers, and
//!   paces the frames.
//!
//! `PDFCRAFT_RENDERER=software` chooses it anywhere on Windows; unset, it is used in a Remote
//! Desktop session on a machine with no graphics processor (`draws_in_software` in `main.rs`).
//! Paint callbacks, which need a GPU, are skipped: PdfCraft has none.

mod damage;
mod painter;
mod raster;
mod runner;
mod storage;

pub(crate) use runner::{Setup, run};
