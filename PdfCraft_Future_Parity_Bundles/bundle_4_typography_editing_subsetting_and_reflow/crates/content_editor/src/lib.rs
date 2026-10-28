//! PdfCraft Content Editor: AST delta patcher, marked content preservation,
//! vector path manipulation, and legal Bates numbering.
//!
//! Subsystem covering Milestone M7:
//! - [P0] Edit existing text in place
//! - [P1] Keep structure tags when editing (/MCID)
//! - [P1] Select, move, scale, recolor and delete vector paths
//! - [P1] Bates numbering and multi-file continuity

pub mod bates;
pub mod delta_patcher;
pub mod vector_editor;

pub use bates::{BatesConfig, BatesNumberer, BatesPosition};
pub use delta_patcher::ContentDeltaPatcher;
pub use vector_editor::{VectorElement, VectorPathEditor};
