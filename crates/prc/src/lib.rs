//! A reader for PRC, the 3D model format a PDF `/3D` stream of `/Subtype /PRC` carries
//! (ISO 32000-2 §13.6.3, ISO 14739-1).
//!
//! Layer L0, standalone: this crate depends on no other PdfCraft crate. It reads the
//! uncompressed container, inflates each section, and decodes the bit-level primitives and
//! the schema a newer producer uses to describe fields this reader does not know; it decodes
//! tessellations and walks the product tree into placed, coloured meshes
//! ([`PrcFile::placements`], [`PrcFile::model_tree`]).
//!
//! Clause numbers in `[WD x.y]` refer to the PRC working draft, ISO/TC 171/SC 2 N570.
//!
//! Every read is bounded: inflation by [`MAX_INFLATED_BYTES`] (or a caller's limit), decoded
//! array elements by [`MAX_DECODED_ELEMENTS`] and schema-program steps by [`MAX_SCHEMA_STEPS`]
//! per reading pass, schema programs by a depth ceiling. Damaged data is an error, never a panic.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod acof;
mod arrays;
mod bits;
mod container;
mod entity;
mod error;
mod geometry;
mod schema;
mod tess;
#[cfg(test)]
mod testw;
mod tree;

pub use bits::{BitReader, MAX_DECODED_ELEMENTS, MAX_SCHEMA_STEPS};
pub use container::{FileStructure, MAX_FILE_STRUCTURES, MAX_INFLATED_BYTES, PRC_READER_VERSION, PrcFile, PrcHeader, SectionKind, UniqueId};
pub use error::PrcError;
pub use geometry::Matrix;
pub use schema::Schema;
pub use tess::{Tessellation, TriangleMesh, TriangleStyle};
pub use tree::{EntityOverrides, MAX_PLACEMENTS, ModelNode, NameSource, Placement, Rgba, StyleAlpha};
