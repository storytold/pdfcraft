//! Clean-room ISO 32000-1 §9.6.5 Type 3 Font Interpreter.
//! Executes `/CharProcs` streams using `d0` (colored) and `d1` (uncolored) glyph operators.

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Type3Glyph {
    pub name: String,
    pub width: (f32, f32),
    pub is_colored: bool,
    pub bbox: Option<[f32; 4]>,
    pub stream_ops: Vec<u8>,
}

pub struct Type3Font {
    pub font_matrix: [f32; 6],
    pub font_bbox: [f32; 4],
    pub char_procs: HashMap<String, Type3Glyph>,
    pub encoding: HashMap<u8, String>,
}

impl Type3Font {
    pub fn new(font_matrix: [f32; 6], font_bbox: [f32; 4]) -> Self {
        Self {
            font_matrix,
            font_bbox,
            char_procs: HashMap::new(),
            encoding: HashMap::new(),
        }
    }

    pub fn register_glyph(&mut self, char_code: u8, glyph_name: String, stream: &[u8]) {
        let (width, is_colored, bbox) = Self::parse_glyph_header(stream);
        let glyph = Type3Glyph {
            name: glyph_name.clone(),
            width,
            is_colored,
            bbox,
            stream_ops: stream.to_vec(),
        };
        self.char_procs.insert(glyph_name.clone(), glyph);
        self.encoding.insert(char_code, glyph_name);
    }

    /// Parses initial `d0` or `d1` operator from charproc stream.
    fn parse_glyph_header(stream: &[u8]) -> ((f32, f32), bool, Option<[f32; 4]>) {
        let text = String::from_utf8_lossy(stream);
        let tokens: Vec<&str> = text.split_whitespace().collect();

        for i in 0..tokens.len() {
            if tokens[i] == "d0" && i >= 2 {
                let wx = tokens[i - 2].parse::<f32>().unwrap_or(0.0);
                let wy = tokens[i - 1].parse::<f32>().unwrap_or(0.0);
                return ((wx, wy), true, None);
            } else if tokens[i] == "d1" && i >= 6 {
                let wx = tokens[i - 6].parse::<f32>().unwrap_or(0.0);
                let wy = tokens[i - 5].parse::<f32>().unwrap_or(0.0);
                let llx = tokens[i - 4].parse::<f32>().unwrap_or(0.0);
                let lly = tokens[i - 3].parse::<f32>().unwrap_or(0.0);
                let urx = tokens[i - 2].parse::<f32>().unwrap_or(0.0);
                let ury = tokens[i - 1].parse::<f32>().unwrap_or(0.0);
                return ((wx, wy), false, Some([llx, lly, urx, ury]));
            }
        }

        ((1000.0, 0.0), false, None)
    }

    pub fn get_glyph_for_char(&self, char_code: u8) -> Option<&Type3Glyph> {
        let glyph_name = self.encoding.get(&char_code)?;
        self.char_procs.get(glyph_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type3_d0_colored_glyph_parsing() {
        let mut font = Type3Font::new([0.001, 0.0, 0.0, 0.001, 0.0, 0.0], [0.0, 0.0, 1000.0, 1000.0]);
        let charproc = b"500 0 d0 1 0 0 rg 0 0 500 500 re f";
        font.register_glyph(b'A', "glyphA".into(), charproc);

        let glyph = font.get_glyph_for_char(b'A').unwrap();
        assert_eq!(glyph.width, (500.0, 0.0));
        assert!(glyph.is_colored);
        assert!(glyph.bbox.is_none());
    }

    #[test]
    fn test_type3_d1_uncolored_glyph_parsing() {
        let mut font = Type3Font::new([0.001, 0.0, 0.0, 0.001, 0.0, 0.0], [0.0, 0.0, 1000.0, 1000.0]);
        let charproc = b"600 0 0 0 600 800 d1 0 0 600 800 re f";
        font.register_glyph(b'B', "glyphB".into(), charproc);

        let glyph = font.get_glyph_for_char(b'B').unwrap();
        assert_eq!(glyph.width, (600.0, 0.0));
        assert!(!glyph.is_colored);
        assert_eq!(glyph.bbox, Some([0.0, 0.0, 600.0, 800.0]));
    }
}
