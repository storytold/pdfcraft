//! AcroForm Appearance Stream Regenerator with Auto-Size, Comb, and Password Masking.

use std::fmt::Write;
use crate::field_types::FormField;

pub struct FieldAppearanceGenerator;

impl FieldAppearanceGenerator {
    /// Generates compliant `/AP << /N stream >>` for an updated form field value.
    pub fn generate_normal_appearance(field: &FormField) -> Vec<u8> {
        let mut stream = String::new();
        let w = field.bbox[2] - field.bbox[0];
        let h = field.bbox[3] - field.bbox[1];

        writeln!(&mut stream, "q").unwrap();
        // Border and clipping
        writeln!(&mut stream, "0 0 {:.2} {:.2} re W n", w, h).unwrap();

        // Determine display string (mask if password)
        let display_text = if field.is_password() {
            "\u{2022}".repeat(field.value.len())
        } else {
            field.value.clone()
        };

        if field.is_comb() && field.max_len.is_some() {
            // Comb field formatting: N equal cells
            Self::generate_comb_appearance(&mut stream, &display_text, field.max_len.unwrap(), w, h);
        } else if field.is_multiline() {
            // Multiline text formatting
            Self::generate_multiline_appearance(&mut stream, &display_text, w, h);
        } else {
            // Single line text with dynamic auto-sizing
            Self::generate_single_line_appearance(&mut stream, &display_text, w, h);
        }

        writeln!(&mut stream, "Q").unwrap();
        stream.into_bytes()
    }

    fn generate_single_line_appearance(stream: &mut String, text: &str, width: f32, height: f32) {
        // Auto-size font height calculation: height * 0.7 clamped between 6pt and 36pt
        let font_size = (height * 0.7).clamp(6.0, 36.0);
        let baseline_y = (height - font_size) / 2.0 + 2.0;

        writeln!(stream, "BT").unwrap();
        writeln!(stream, "/Helv {:.2} Tf 0 g", font_size).unwrap();
        writeln!(stream, "2.00 {:.2} Td", baseline_y).unwrap();
        writeln!(stream, "({}) Tj", Self::escape_pdf_str(text)).unwrap();
        writeln!(stream, "ET").unwrap();
    }

    fn generate_comb_appearance(stream: &mut String, text: &str, max_len: usize, width: f32, height: f32) {
        if max_len == 0 {
            return;
        }
        let cell_width = width / max_len as f32;
        let font_size = (height * 0.65).clamp(8.0, 24.0);
        let baseline_y = (height - font_size) / 2.0 + 2.0;

        writeln!(stream, "BT").unwrap();
        writeln!(stream, "/Helv {:.2} Tf 0 g", font_size).unwrap();

        for (idx, ch) in text.chars().take(max_len).enumerate() {
            let cell_center_x = (idx as f32 + 0.5) * cell_width;
            let approx_char_half_w = font_size * 0.25;
            let char_x = (cell_center_x - approx_char_half_w).max(0.0);

            writeln!(stream, "1 0 0 1 {:.2} {:.2} Tm ({}) Tj", char_x, baseline_y, ch).unwrap();
        }

        writeln!(stream, "ET").unwrap();
    }

    fn generate_multiline_appearance(stream: &mut String, text: &str, width: f32, height: f32) {
        let font_size = 10.0f32;
        let leading = 12.0f32;
        let mut curr_y = height - leading;

        writeln!(stream, "BT").unwrap();
        writeln!(stream, "/Helv {:.2} Tf 0 g", font_size).unwrap();
        writeln!(stream, "2.00 {:.2} Td", curr_y).unwrap();
        writeln!(stream, "{:.2} TL", leading).unwrap();

        for line in text.lines() {
            if curr_y < font_size {
                break; // Stop when bottom reached
            }
            writeln!(stream, "({}) '", Self::escape_pdf_str(line)).unwrap();
            curr_y -= leading;
        }

        writeln!(stream, "ET").unwrap();
    }

    fn escape_pdf_str(s: &str) -> String {
        let mut out = String::new();
        for c in s.chars() {
            if c == '(' || c == ')' || c == '\\' {
                out.push('\\');
            }
            out.push(c);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field_types::{FLAG_COMB, FLAG_PASSWORD, FormFieldType};

    #[test]
    fn test_comb_appearance_generation() {
        let field = FormField {
            name: "TaxID".into(),
            field_type: FormFieldType::Text,
            flags: FLAG_COMB,
            value: "12345".into(),
            default_value: None,
            max_len: Some(5),
            bbox: [0.0, 0.0, 100.0, 20.0],
            default_appearance: "/Helv 0 Tf 0 g".into(),
        };

        let stream_bytes = FieldAppearanceGenerator::generate_normal_appearance(&field);
        let stream_str = String::from_utf8_lossy(&stream_bytes);
        assert!(stream_str.contains("Tm (1) Tj"));
        assert!(stream_str.contains("Tm (5) Tj"));
    }

    #[test]
    fn test_password_appearance_masked() {
        let field = FormField {
            name: "Pass".into(),
            field_type: FormFieldType::Text,
            flags: FLAG_PASSWORD,
            value: "Secret".into(),
            default_value: None,
            max_len: None,
            bbox: [0.0, 0.0, 100.0, 20.0],
            default_appearance: "/Helv 10 Tf 0 g".into(),
        };

        let stream_bytes = FieldAppearanceGenerator::generate_normal_appearance(&field);
        let stream_str = String::from_utf8_lossy(&stream_bytes);
        assert!(!stream_str.contains("Secret"));
        assert!(stream_str.contains("\u{2022}"));
    }
}
