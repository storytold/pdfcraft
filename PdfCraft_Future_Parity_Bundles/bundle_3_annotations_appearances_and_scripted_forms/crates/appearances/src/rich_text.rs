//! Rich Text (/RC) XHTML Parser and Formatter per ISO 32000-1 §12.7.3.3.

#[derive(Debug, Clone)]
pub struct RichTextSpan {
    pub text: String,
    pub is_bold: bool,
    pub is_italic: bool,
    pub color_rgb: [f32; 3],
    pub font_size_pts: f32,
}

pub struct RichTextParser;

impl RichTextParser {
    /// Extracts text runs and inline styles from XHTML rich text string.
    pub fn parse(xml_str: &str) -> Vec<RichTextSpan> {
        let mut spans = Vec::new();
        let mut is_bold = false;
        let mut is_italic = false;
        let mut current_text = String::new();

        let mut chars = xml_str.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '<' {
                // Flush accumulated text
                if !current_text.is_empty() {
                    spans.push(RichTextSpan {
                        text: std::mem::take(&mut current_text),
                        is_bold,
                        is_italic,
                        color_rgb: [0.0, 0.0, 0.0],
                        font_size_pts: 10.0,
                    });
                }

                // Read tag
                let mut tag = String::new();
                for c in chars.by_ref() {
                    if c == '>' {
                        break;
                    }
                    tag.push(c);
                }

                let tag_lower = tag.to_lowercase();
                if tag_lower == "b" || tag_lower == "strong" {
                    is_bold = true;
                } else if tag_lower == "/b" || tag_lower == "/strong" {
                    is_bold = false;
                } else if tag_lower == "i" || tag_lower == "em" {
                    is_italic = true;
                } else if tag_lower == "/i" || tag_lower == "/em" {
                    is_italic = false;
                }
            } else {
                current_text.push(ch);
            }
        }

        if !current_text.is_empty() {
            spans.push(RichTextSpan {
                text: current_text,
                is_bold,
                is_italic,
                color_rgb: [0.0, 0.0, 0.0],
                font_size_pts: 10.0,
            });
        }

        spans
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rich_text_span_parsing() {
        let xml = "<body><p>Normal <b>Bold Text</b> <i>Italic</i></p></body>";
        let spans = RichTextParser::parse(xml);
        assert!(!spans.is_empty());
        let bold_span = spans.iter().find(|s| s.text.contains("Bold Text")).unwrap();
        assert!(bold_span.is_bold);
        assert!(!bold_span.is_italic);
    }
}
