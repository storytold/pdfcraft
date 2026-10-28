//! Appearance Stream Synthesizer for PDF Annotations.
//! Complies with ISO 32000-1 §12.5.5 (Appearance Streams).

use std::fmt::Write;
use crate::markup_types::{AnnotationMarkup, LineEndingStyle};
use crate::rich_text::RichTextParser;

pub struct AppearanceStream {
    pub bbox: [f32; 4],
    pub stream_bytes: Vec<u8>,
}

pub struct AppearanceSynthesizer;

impl AppearanceSynthesizer {
    /// Synthesizes an appearance stream for any markup type.
    pub fn synthesize(markup: &AnnotationMarkup, bbox: [f32; 4], stroke_rgb: [f32; 3], line_width: f32) -> AppearanceStream {
        let mut stream = String::new();
        let w = bbox[2] - bbox[0];
        let h = bbox[3] - bbox[1];

        // Header and coordinate system isolation
        writeln!(&mut stream, "q").unwrap();
        writeln!(&mut stream, "{:.3} w", line_width.max(0.5)).unwrap();
        writeln!(&mut stream, "{:.3} {:.3} {:.3} RG", stroke_rgb[0], stroke_rgb[1], stroke_rgb[2]).unwrap();

        match markup {
            AnnotationMarkup::Highlight { quad_points } => {
                // Highlight semi-transparent yellow with Multiply blend mode
                writeln!(&mut stream, "/GS_Highlight gs").unwrap();
                writeln!(&mut stream, "1.000 0.950 0.000 rg").unwrap();
                if quad_points.len() >= 8 {
                    for chunk in quad_points.chunks_exact(8) {
                        // Quadrilateral points: (x1, y1) to (x4, y4)
                        let x0 = chunk[0] - bbox[0];
                        let y0 = chunk[1] - bbox[1];
                        let x1 = chunk[2] - bbox[0];
                        let y1 = chunk[3] - bbox[1];
                        let x2 = chunk[6] - bbox[0];
                        let y2 = chunk[7] - bbox[1];
                        let x3 = chunk[4] - bbox[0];
                        let y3 = chunk[5] - bbox[1];

                        writeln!(&mut stream, "{:.2} {:.2} m {:.2} {:.2} l {:.2} {:.2} l {:.2} {:.2} l h f", x0, y0, x1, y1, x2, y2, x3, y3).unwrap();
                    }
                } else {
                    writeln!(&mut stream, "0 0 {:.2} {:.2} re f", w, h).unwrap();
                }
            }
            AnnotationMarkup::Underline { quad_points } => {
                writeln!(&mut stream, "{:.3} {:.3} {:.3} RG", stroke_rgb[0], stroke_rgb[1], stroke_rgb[2]).unwrap();
                if quad_points.len() >= 8 {
                    for chunk in quad_points.chunks_exact(8) {
                        let x_start = chunk[4] - bbox[0];
                        let y_start = chunk[5] - bbox[1];
                        let x_end = chunk[6] - bbox[0];
                        let y_end = chunk[7] - bbox[1];
                        writeln!(&mut stream, "{:.2} {:.2} m {:.2} {:.2} l S", x_start, y_start, x_end, y_end).unwrap();
                    }
                } else {
                    writeln!(&mut stream, "0 1 m {:.2} 1 l S", w).unwrap();
                }
            }
            AnnotationMarkup::StrikeOut { quad_points } => {
                if quad_points.len() >= 8 {
                    for chunk in quad_points.chunks_exact(8) {
                        let mid_y0 = (chunk[1] + chunk[5]) / 2.0 - bbox[1];
                        let mid_y1 = (chunk[3] + chunk[7]) / 2.0 - bbox[1];
                        let x_start = chunk[0] - bbox[0];
                        let x_end = chunk[2] - bbox[0];
                        writeln!(&mut stream, "{:.2} {:.2} m {:.2} {:.2} l S", x_start, mid_y0, x_end, mid_y1).unwrap();
                    }
                } else {
                    writeln!(&mut stream, "0 {:.2} m {:.2} {:.2} l S", h / 2.0, w, h / 2.0).unwrap();
                }
            }
            AnnotationMarkup::FreeText { text, rich_text_xml } => {
                // Background and border
                writeln!(&mut stream, "1 1 1 rg 0 0 {:.2} {:.2} re B", w, h).unwrap();
                writeln!(&mut stream, "0 0 0 rg").unwrap();
                writeln!(&mut stream, "BT").unwrap();
                writeln!(&mut stream, "/Helv 10 Tf").unwrap();
                writeln!(&mut stream, "4 {:.2} Td", h - 12.0).unwrap();

                if let Some(xml) = rich_text_xml {
                    let spans = RichTextParser::parse(xml);
                    for span in spans {
                        writeln!(&mut stream, "({span_text}) Tj", span_text = Self::escape_pdf_str(&span.text)).unwrap();
                    }
                } else {
                    writeln!(&mut stream, "({text}) Tj", text = Self::escape_pdf_str(text)).unwrap();
                }
                writeln!(&mut stream, "ET").unwrap();
            }
            AnnotationMarkup::Line { start, end, start_style, end_style } => {
                let x0 = start.0 - bbox[0];
                let y0 = start.1 - bbox[1];
                let x1 = end.0 - bbox[0];
                let y1 = end.1 - bbox[1];
                writeln!(&mut stream, "{:.2} {:.2} m {:.2} {:.2} l S", x0, y0, x1, y1).unwrap();

                Self::render_line_ending(&mut stream, x0, y0, x1, y1, *start_style);
                Self::render_line_ending(&mut stream, x1, y1, x0, y0, *end_style);
            }
            AnnotationMarkup::Stamp { stamp_name } => {
                // Professional stamp design (e.g. APPROVED or CONFIDENTIAL)
                let is_approved = stamp_name.eq_ignore_ascii_case("approved");
                let color = if is_approved { "0.0 0.6 0.0" } else { "0.8 0.0 0.0" };
                writeln!(&mut stream, "{} RG {} rg", color, color).unwrap();
                writeln!(&mut stream, "2 w 2 2 {:.2} {:.2} re S", w - 4.0, h - 4.0).unwrap();
                writeln!(&mut stream, "BT /Helv-Bold 14 Tf 8 {:.2} Td ({}) Tj ET", h / 2.0 - 5.0, stamp_name.to_uppercase()).unwrap();
            }
            AnnotationMarkup::Ink { paths } => {
                for path in paths {
                    if let Some((first, rest)) = path.split_first() {
                        writeln!(&mut stream, "{:.2} {:.2} m", first.0 - bbox[0], first.1 - bbox[1]).unwrap();
                        for pt in rest {
                            writeln!(&mut stream, "{:.2} {:.2} l", pt.0 - bbox[0], pt.1 - bbox[1]).unwrap();
                        }
                        writeln!(&mut stream, "S").unwrap();
                    }
                }
            }
            AnnotationMarkup::Redact { overlay_text } => {
                // Permanent black redacted block
                writeln!(&mut stream, "0 0 0 rg 0 0 {:.2} {:.2} re f", w, h).unwrap();
                if let Some(text) = overlay_text {
                    writeln!(&mut stream, "1 1 1 rg BT /Helv-Bold 10 Tf 4 {:.2} Td ({}) Tj ET", h / 2.0 - 4.0, Self::escape_pdf_str(text)).unwrap();
                }
            }
            _ => {
                // Standard fallback rectangle outline
                writeln!(&mut stream, "0 0 {:.2} {:.2} re S", w, h).unwrap();
            }
        }

        writeln!(&mut stream, "Q").unwrap();

        AppearanceStream {
            bbox,
            stream_bytes: stream.into_bytes(),
        }
    }

    fn render_line_ending(stream: &mut String, x: f32, y: f32, other_x: f32, other_y: f32, style: LineEndingStyle) {
        if style == LineEndingStyle::None {
            return;
        }
        let angle = (other_y - y).atan2(other_x - x);
        let arrow_len = 6.0f32;

        match style {
            LineEndingStyle::OpenArrow | LineEndingStyle::ClosedArrow => {
                let a1 = angle + std::f32::consts::PI * 0.85;
                let a2 = angle - std::f32::consts::PI * 0.85;
                let p1_x = x + arrow_len * a1.cos();
                let p1_y = y + arrow_len * a1.sin();
                let p2_x = x + arrow_len * a2.cos();
                let p2_y = y + arrow_len * a2.sin();

                if style == LineEndingStyle::ClosedArrow {
                    writeln!(stream, "{:.2} {:.2} m {:.2} {:.2} l {:.2} {:.2} l h b", p1_x, p1_y, x, y, p2_x, p2_y).unwrap();
                } else {
                    writeln!(stream, "{:.2} {:.2} m {:.2} {:.2} l {:.2} {:.2} l S", p1_x, p1_y, x, y, p2_x, p2_y).unwrap();
                }
            }
            LineEndingStyle::Circle => {
                let r = 3.0f32;
                writeln!(stream, "{:.2} {:.2} {:.2} 0 360 arc f", x, y, r).unwrap();
            }
            LineEndingStyle::Square => {
                let s = 4.0f32;
                writeln!(stream, "{:.2} {:.2} {:.2} {:.2} re f", x - s / 2.0, y - s / 2.0, s, s).unwrap();
            }
            _ => {}
        }
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

    #[test]
    fn test_highlight_appearance_stream_generation() {
        let markup = AnnotationMarkup::Highlight { quad_points: vec![10.0, 10.0, 50.0, 10.0, 10.0, 20.0, 50.0, 20.0] };
        let app = AppearanceSynthesizer::synthesize(&markup, [10.0, 10.0, 50.0, 20.0], [1.0, 1.0, 0.0], 1.0);
        let stream_str = String::from_utf8_lossy(&app.stream_bytes);
        assert!(stream_str.contains("/GS_Highlight gs"));
        assert!(stream_str.contains("rg"));
    }

    #[test]
    fn test_stamp_appearance_generation() {
        let markup = AnnotationMarkup::Stamp { stamp_name: "Approved".into() };
        let app = AppearanceSynthesizer::synthesize(&markup, [0.0, 0.0, 120.0, 40.0], [0.0, 0.6, 0.0], 2.0);
        let stream_str = String::from_utf8_lossy(&app.stream_bytes);
        assert!(stream_str.contains("APPROVED"));
    }
}
