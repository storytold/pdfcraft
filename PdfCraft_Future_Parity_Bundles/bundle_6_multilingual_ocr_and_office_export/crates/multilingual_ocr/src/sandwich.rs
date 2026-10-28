//! Sandwich PDF Searchable Invisible Text Layer Generator (ISO 32000-1 §9.3.6).

use std::fmt::Write;

#[derive(Debug, Clone)]
pub struct RecognizedWord {
    pub text: String,
    pub bbox_pixels: [f32; 4], // [min_x, min_y, max_x, max_y]
    pub confidence: f32,       // 0.0 to 1.0
}

pub struct SandwichPdfGenerator;

impl SandwichPdfGenerator {
    /// Generates content stream combining the background scan image with an invisible text layer.
    pub fn generate_searchable_page_stream(
        image_xobject_name: &str,
        page_width_pts: f32,
        page_height_pts: f32,
        image_pixel_width: f32,
        image_pixel_height: f32,
        words: &[RecognizedWord],
    ) -> Vec<u8> {
        let mut stream = String::new();

        // 1. Draw scanned background image at full page dimensions
        writeln!(&mut stream, "q").unwrap();
        writeln!(&mut stream, "{:.2} 0 0 {:.2} 0 0 cm", page_width_pts, page_height_pts).unwrap();
        writeln!(&mut stream, "/{} Do", image_xobject_name).unwrap();
        writeln!(&mut stream, "Q").unwrap();

        // 2. Invisible text layer (3 Tr)
        let scale_x = page_width_pts / image_pixel_width.max(1.0);
        let scale_y = page_height_pts / image_pixel_height.max(1.0);

        writeln!(&mut stream, "BT").unwrap();
        writeln!(&mut stream, "3 Tr").unwrap(); // Invisible text rendering mode

        for word in words {
            let px_x0 = word.bbox_pixels[0];
            let px_y0 = word.bbox_pixels[1];
            let px_x1 = word.bbox_pixels[2];
            let px_y1 = word.bbox_pixels[3];

            // Convert scan pixel space (origin top-left) to PDF user space (origin bottom-left)
            let pdf_x = px_x0 * scale_x;
            let pdf_y = page_height_pts - (px_y1 * scale_y);
            let pdf_w = (px_x1 - px_x0) * scale_x;
            let pdf_h = (px_y1 - px_y0) * scale_y;

            let font_size = pdf_h.clamp(4.0, 72.0);

            writeln!(&mut stream, "/Helv {:.2} Tf", font_size).unwrap();
            writeln!(&mut stream, "1 0 0 1 {:.2} {:.2} Tm", pdf_x, pdf_y).unwrap();
            writeln!(&mut stream, "({}) Tj", Self::escape_pdf_str(&word.text)).unwrap();
        }

        writeln!(&mut stream, "ET").unwrap();
        stream.into_bytes()
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
    fn test_searchable_sandwich_generation() {
        let words = vec![
            RecognizedWord {
                text: "INVOICE".into(),
                bbox_pixels: [100.0, 100.0, 300.0, 150.0],
                confidence: 0.98,
            },
        ];

        let stream_bytes = SandwichPdfGenerator::generate_searchable_page_stream(
            "ScanImage1",
            612.0,
            792.0,
            1200.0,
            1600.0,
            &words,
        );

        let stream_str = String::from_utf8_lossy(&stream_bytes);
        assert!(stream_str.contains("/ScanImage1 Do"));
        assert!(stream_str.contains("3 Tr")); // Invisible text mode
        assert!(stream_str.contains("(INVOICE) Tj"));
    }
}
