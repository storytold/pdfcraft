//! Pure-Rust QR Code generator and vector/raster PDF embedder (ISO/IEC 18004).
//!
//! Generates QR codes with Reed-Solomon error correction and exports them as
//! razor-sharp PDF vector paths or high-resolution PNG images.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use rxing::Writer;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum QrError {
    #[error("the text can't be encoded as a QR code: {0}")]
    Encoding(String),
    #[error("failed to encode PNG: {0}")]
    PngEncoding(String),
}

/// A 2D boolean grid representing dark and light QR code modules.
#[derive(Clone, Debug, PartialEq)]
pub struct QrMatrix {
    /// Dimension in modules (e.g. 21 for v1, 25 for v2, etc.).
    pub size: usize,
    /// Row-major module data: true = dark/black, false = light/white.
    pub modules: Vec<bool>,
}

impl QrMatrix {
    pub fn new(size: usize) -> Self {
        Self { size, modules: vec![false; size.saturating_mul(size)] }
    }

    pub fn is_dark(&self, row: usize, col: usize) -> bool {
        if row < self.size && col < self.size {
            self.modules.get(row.saturating_mul(self.size).saturating_add(col)).copied().unwrap_or(false)
        } else {
            false
        }
    }

    pub fn set(&mut self, row: usize, col: usize, dark: bool) {
        if row < self.size
            && col < self.size
            && let Some(m) = self.modules.get_mut(row.saturating_mul(self.size).saturating_add(col))
        {
            *m = dark;
        }
    }

    /// Render QR matrix to PNG image bytes.
    pub fn to_png_bytes(&self, module_px: u32, border_modules: u32) -> Result<Vec<u8>, QrError> {
        let module_px = module_px.max(1);
        let border_modules = border_modules.max(1);
        let total_modules = (self.size as u32).saturating_add(border_modules.saturating_mul(2));
        let img_side = total_modules.saturating_mul(module_px);

        let mut rgba = vec![255u8; (img_side as usize).saturating_mul(img_side as usize).saturating_mul(4)];

        for r in 0..self.size {
            for c in 0..self.size {
                if self.is_dark(r, c) {
                    let start_x = ((c as u32).saturating_add(border_modules)).saturating_mul(module_px);
                    let start_y = ((r as u32).saturating_add(border_modules)).saturating_mul(module_px);

                    for py in start_y..start_y.saturating_add(module_px) {
                        for px in start_x..start_x.saturating_add(module_px) {
                            if px < img_side && py < img_side {
                                let idx = ((py as usize).saturating_mul(img_side as usize).saturating_add(px as usize)).saturating_mul(4);
                                if idx.saturating_add(3) < rgba.len() {
                                    rgba[idx] = 0;
                                    rgba[idx + 1] = 0;
                                    rgba[idx + 2] = 0;
                                    rgba[idx + 3] = 255;
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, img_side, img_side);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().map_err(|e| QrError::PngEncoding(e.to_string()))?;
            writer.write_image_data(&rgba).map_err(|e| QrError::PngEncoding(e.to_string()))?;
        }
        Ok(out)
    }

    /// Generate PDF vector path stream operators (`re` ... `f`) within `rect: [x, y, x + w, y + h]`.
    pub fn to_pdf_vector_ops(&self, rect: [f64; 4], border_modules: u32) -> String {
        let total_modules = (self.size as f64) + (border_modules as f64) * 2.0;
        let w = rect[2] - rect[0];
        let h = rect[3] - rect[1];
        let mod_w = w / total_modules.max(1.0);
        let mod_h = h / total_modules.max(1.0);

        let mut ops = String::with_capacity(self.size.saturating_mul(self.size).saturating_mul(24));
        ops.push_str("0 0 0 rg\n"); // Black fill

        for r in 0..self.size {
            for c in 0..self.size {
                if self.is_dark(r, c) {
                    let mx = rect[0] + ((c as f64) + (border_modules as f64)) * mod_w;
                    // PDF user space coordinates have y pointing upwards:
                    let my = rect[1] + ((self.size - 1 - r) as f64 + (border_modules as f64)) * mod_h;
                    use std::fmt::Write;
                    let _ = writeln!(ops, "{:.2} {:.2} {:.2} {:.2} re", mx, my, mod_w, mod_h);
                }
            }
        }
        ops.push_str("f\n");
        ops
    }
}

pub struct QrCode;

impl QrCode {
    /// Encode a text or URL payload into a `QrMatrix` using standard QR code encoding.
    pub fn encode_text(text: &str) -> Result<QrMatrix, QrError> {
        let writer = rxing::qrcode::QRCodeWriter;
        let matrix = writer.encode(text, &rxing::BarcodeFormat::QR_CODE, 0, 0).map_err(|e| QrError::Encoding(e.to_string()))?;

        let total_w = matrix.getWidth() as usize;
        // rxing adds a default 4-module quiet zone margin on each side (total 8 modules).
        // Extract the core matrix so `QrMatrix` has standard module dimensions.
        let core_size = total_w.saturating_sub(8);
        if core_size == 0 {
            return Err(QrError::Encoding("the encoder returned an empty matrix".into()));
        }

        let mut qr_matrix = QrMatrix::new(core_size);
        for r in 0..core_size {
            for c in 0..core_size {
                let dark = matrix.get((c + 4) as u32, (r + 4) as u32);
                qr_matrix.set(r, c, dark);
            }
        }

        Ok(qr_matrix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qr_encode_and_render_png() {
        let qr = QrCode::encode_text("https://example.com/craft").expect("encode QR");
        assert!(qr.size >= 21);
        let png = qr.to_png_bytes(4, 4).expect("to PNG");
        assert!(!png.is_empty());
        assert_eq!(&png[0..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn test_qr_pdf_vector_ops() {
        let qr = QrCode::encode_text("PDF-TEST").expect("encode QR");
        let ops = qr.to_pdf_vector_ops([50.0, 50.0, 150.0, 150.0], 4);
        assert!(ops.starts_with("0 0 0 rg\n"));
        assert!(ops.ends_with("f\n"));
        assert!(ops.contains("re\n"));
    }

    #[test]
    fn test_qr_decode_roundtrip() {
        let payload = "PDF-CRAFT-VERIFIED-QR-2026";
        let qr = QrCode::encode_text(payload).expect("encode QR");
        let png_bytes = qr.to_png_bytes(6, 4).expect("to png");

        let decoder = png::Decoder::new(std::io::Cursor::new(&png_bytes));
        let mut reader = decoder.read_info().expect("read png header");
        let mut buf = vec![0u8; reader.output_buffer_size().unwrap_or(500 * 500 * 4)];
        let info = reader.next_frame(&mut buf).expect("read png frame");
        let rgba = &buf[..info.buffer_size()];

        let gray = crate::barcode_scan::GrayImage::from_rgba(info.width as usize, info.height as usize, rgba).expect("gray");
        let results = rxing::helpers::detect_multiple_in_luma(gray.pixels, info.width, info.height).expect("detect");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].getText(), payload);
        assert_eq!(*results[0].getBarcodeFormat(), rxing::BarcodeFormat::QR_CODE);
    }
}
