//! Geometry-based regions and mixed-style rewriting of existing text.

use super::{
    BlockStyle, CraftFont, Dict, Document, EditError, HashMap, LineEdit, MAX_TYPE3_GLYPHS, Matrix, Metrics, Object, Op, PdfString, Pieces,
    RangeStyle, TextBlock, TextLine, Type3Glyph, coincident_ops, content_streams, fallback_face, japanese_glyph_from, needs_type3, no_fallback_font,
    no_glyph_for, page_dict, paragraph_separator, reveal_ops, rewritten_contents, same_base_font, same_font, source_encodes, source_family,
    source_program, source_show, source_space, source_width, splice, strip_subset_tag, type3_faces, type3_glyph, union, write_type3_glyphs,
};

#[path = "text_regions/grouping.rs"]
mod grouping;
#[path = "text_regions/styled.rs"]
mod styled;

pub(super) use grouping::{alignment_frame, group_blocks, group_reading_blocks, is_centred};
pub(super) use styled::rewrite_styled_block;
