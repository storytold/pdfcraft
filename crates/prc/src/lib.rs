//! A reader and a mesh writer for PRC, the 3D model format a PDF `/3D` stream of `/Subtype /PRC`
//! carries (ISO 32000-2 §13.6.3, ISO 14739-1), and a glTF 2.0 importer that feeds the writer.
//!
//! Layer L0, standalone: this crate depends on no other PdfCraft crate. It reads the
//! uncompressed container, inflates each section, and decodes the bit-level primitives and
//! the schema a newer producer uses to describe fields this reader does not know; it decodes
//! tessellations and walks the product tree into placed, coloured meshes
//! ([`PrcFile::placements`], [`PrcFile::model_tree`]).
//!
//! [`write_prc`] writes a triangle-mesh scene ([`MeshScene`]: named nodes, shared meshes, colours,
//! PNG/JPEG base-colour textures) as a single-file-structure stream; [`read_gltf`] reads a `.gltf`
//! or `.glb` file into one, noting what it drops. Neither opens files: the caller resolves external
//! URIs.
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
mod gltf;
mod schema;
mod tess;
#[cfg(test)]
mod testw;
mod tree;
mod write;

pub use bits::{BitReader, MAX_DECODED_ELEMENTS, MAX_SCHEMA_STEPS};
pub use container::{FileStructure, MAX_FILE_STRUCTURES, MAX_INFLATED_BYTES, PRC_READER_VERSION, PrcFile, PrcHeader, SectionKind, UniqueId};
pub use error::PrcError;
pub use geometry::Matrix;
pub use gltf::{GltfColours, GltfError, GltfImport, GltfOptions, GltfUpAxis, MAX_GLTF_VALUES, MAX_IMAGE_BYTES, MAX_JSON_VALUES, read_gltf};
pub use schema::Schema;
pub use tess::{Tessellation, TriangleMesh, TriangleStyle};
pub use tree::{EntityOverrides, MAX_PLACEMENTS, ModelNode, NameSource, Placement, Rgba, StyleAlpha};
pub use write::{
    MAX_DEPTH, MeshScene, PrcWriteError, PrcWriteReport, SceneMaterial, SceneMesh, SceneNode, ScenePrimitive, SceneTexture, TextureWrap, write_prc,
};
