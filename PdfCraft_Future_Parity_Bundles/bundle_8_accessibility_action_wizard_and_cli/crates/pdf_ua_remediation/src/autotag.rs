//! Structural Auto-Tagging Classifier and Artifact Marker.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardStructureTag {
    Document,
    Part,
    Art,
    Sect,
    Div,
    H1,
    H2,
    H3,
    P,
    Table,
    TR,
    TH,
    TD,
    Figure,
    Artifact,
}

#[derive(Debug, Clone)]
pub struct ContentBlock {
    pub text: String,
    pub font_size: f32,
    pub is_bold: bool,
    pub bbox: [f32; 4],
    pub is_running_header_or_footer: bool,
}

pub struct AutoTagClassifier;

impl AutoTagClassifier {
    /// Classifies an un-tagged content block into appropriate structure tag.
    pub fn classify_block(block: &ContentBlock, base_font_size: f32) -> StandardStructureTag {
        if block.is_running_header_or_footer {
            return StandardStructureTag::Artifact;
        }

        if block.font_size >= base_font_size * 1.6 && block.is_bold {
            StandardStructureTag::H1
        } else if block.font_size >= base_font_size * 1.3 && block.is_bold {
            StandardStructureTag::H2
        } else if block.font_size >= base_font_size * 1.15 {
            StandardStructureTag::H3
        } else {
            StandardStructureTag::P
        }
    }

    /// Wraps content stream segment in `/Artifact` tag.
    pub fn mark_as_artifact(stream_segment: &str) -> String {
        format!("/Artifact BMC\n{}\nEMC\n", stream_segment)
    }
}
