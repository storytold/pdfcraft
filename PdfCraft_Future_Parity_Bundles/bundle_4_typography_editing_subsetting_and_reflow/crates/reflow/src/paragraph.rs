//! Paragraph Reflow, Word Wrapping, and Bullet List Hanging Indentation.

use crate::bidi::{BidiClassifier, TextDirection};

#[derive(Debug, Clone)]
pub struct ReflowedLine {
    pub text: String,
    pub x_offset: f32,
    pub y_offset: f32,
    pub is_bullet: bool,
}

pub struct ParagraphReflowEngine;

impl ParagraphReflowEngine {
    /// Reflows paragraph text into wrapped lines within the specified bounding box width.
    pub fn reflow_text(
        text: &str,
        max_width: f32,
        font_size: f32,
        line_height: f32,
    ) -> Vec<ReflowedLine> {
        let mut lines = Vec::new();
        let avg_char_w = font_size * 0.5; // Average proportional font character width
        let mut current_y = 0.0f32;

        for raw_line in text.lines() {
            let trimmed = raw_line.trim_start();
            let is_bullet = trimmed.starts_with('•') || trimmed.starts_with('-') || trimmed.starts_with('*');
            let bullet_indent = if is_bullet { font_size * 1.5 } else { 0.0 };

            let words: Vec<&str> = raw_line.split_whitespace().collect();
            if words.is_empty() {
                current_y += line_height;
                continue;
            }

            let mut current_line_words: Vec<&str> = Vec::new();
            let mut current_line_w = 0.0f32;
            let mut is_first_line_of_para = true;

            for word in words {
                let word_w = word.len() as f32 * avg_char_w;
                let available_w = if is_first_line_of_para { max_width } else { max_width - bullet_indent };

                if !current_line_words.is_empty() && (current_line_w + word_w + avg_char_w > available_w) {
                    // Flush line
                    let line_str = current_line_words.join(" ");
                    let x_offset = if is_first_line_of_para { 0.0 } else { bullet_indent };
                    lines.push(ReflowedLine {
                        text: line_str,
                        x_offset,
                        y_offset: current_y,
                        is_bullet: is_first_line_of_para && is_bullet,
                    });
                    current_y += line_height;
                    current_line_words.clear();
                    current_line_w = 0.0;
                    is_first_line_of_para = false;
                }

                current_line_words.push(word);
                current_line_w += word_w + avg_char_w;
            }

            if !current_line_words.is_empty() {
                let line_str = current_line_words.join(" ");
                let x_offset = if is_first_line_of_para { 0.0 } else { bullet_indent };
                lines.push(ReflowedLine {
                    text: line_str,
                    x_offset,
                    y_offset: current_y,
                    is_bullet: is_first_line_of_para && is_bullet,
                });
                current_y += line_height;
            }
        }

        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paragraph_reflow_wrapping() {
        let text = "This is a long sentence that should definitely be wrapped across multiple lines when width is small.";
        // max width 150pt with 10pt font (approx 5pt per char -> ~30 chars per line)
        let lines = ParagraphReflowEngine::reflow_text(text, 150.0, 10.0, 14.0);
        assert!(lines.len() >= 3);
    }

    #[test]
    fn test_bullet_list_hanging_indent() {
        let text = "• First bullet point with a long explanation that will spill onto the second line.";
        let lines = ParagraphReflowEngine::reflow_text(text, 150.0, 10.0, 14.0);
        assert!(lines.len() >= 2);
        assert_eq!(lines[0].is_bullet, true);
        assert_eq!(lines[0].x_offset, 0.0);
        assert!(lines[1].x_offset > 0.0); // Hanging indent applied
    }
}
