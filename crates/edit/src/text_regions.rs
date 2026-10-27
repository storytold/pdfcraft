//! Geometry-based regions and mixed-style rewriting of existing text.

use super::*;

#[path = "text_regions/grouping.rs"]
mod grouping;
#[path = "text_regions/styled.rs"]
mod styled;

pub(super) use grouping::{group_blocks, group_reading_blocks, is_centred};
pub(super) use styled::rewrite_styled_block;
