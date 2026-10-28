//! All 18 Standard ISO 32000-1 §12.5.6 Annotation Types and Styling Geometry.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEndingStyle {
    None,
    Square,
    Circle,
    Diamond,
    OpenArrow,
    ClosedArrow,
    Butt,
    ROpenArrow,
    RClosedArrow,
    Slash,
}

impl LineEndingStyle {
    pub fn from_pdf_name(name: &str) -> Self {
        match name {
            "/Square" => Self::Square,
            "/Circle" => Self::Circle,
            "/Diamond" => Self::Diamond,
            "/OpenArrow" => Self::OpenArrow,
            "/ClosedArrow" => Self::ClosedArrow,
            "/Butt" => Self::Butt,
            "/ROpenArrow" => Self::ROpenArrow,
            "/RClosedArrow" => Self::RClosedArrow,
            "/Slash" => Self::Slash,
            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AnnotationMarkup {
    Text { icon: String }, // "Comment", "Key", "Note", "Help", "NewParagraph", "Paragraph", "Insert"
    Highlight { quad_points: Vec<f32> },
    Underline { quad_points: Vec<f32> },
    StrikeOut { quad_points: Vec<f32> },
    Squiggly { quad_points: Vec<f32> },
    FreeText { text: String, rich_text_xml: Option<String> },
    Line { start: (f32, f32), end: (f32, f32), start_style: LineEndingStyle, end_style: LineEndingStyle },
    Square { fill_color: Option<[f32; 3]> },
    Circle { fill_color: Option<[f32; 3]> },
    PolyLine { vertices: Vec<(f32, f32)> },
    Polygon { vertices: Vec<(f32, f32)>, fill_color: Option<[f32; 3]> },
    Caret { proposal_text: Option<String> },
    Stamp { stamp_name: String }, // "Approved", "Confidential", "Draft", "Final", "Expired"
    Ink { paths: Vec<Vec<(f32, f32)>> },
    Popup,
    FileAttachment { file_name: String },
    Sound,
    Redact { overlay_text: Option<String> },
}
