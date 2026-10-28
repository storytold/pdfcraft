//! Font Subset Expander, Glyph Metrics Injector, and ToUnicode Synchronizer.

use std::collections::HashMap;

pub struct FontSubsetExpander {
    pub font_name: String,
    pub glyph_widths: HashMap<u16, u16>,      // Glyph ID -> Advance width (1/1000 units)
    pub unicode_map: HashMap<u16, char>,      // Glyph ID -> Unicode char
    pub char_to_glyph: HashMap<char, u16>,
    next_available_gid: u16,
}

impl FontSubsetExpander {
    pub fn new(font_name: impl Into<String>) -> Self {
        let mut expander = Self {
            font_name: font_name.into(),
            glyph_widths: HashMap::new(),
            unicode_map: HashMap::new(),
            char_to_glyph: HashMap::new(),
            next_available_gid: 1, // GID 0 is always .notdef
        };
        // GID 0 (.notdef)
        expander.glyph_widths.insert(0, 500);
        expander
    }

    /// Registers existing subset character with advance width.
    pub fn register_existing(&mut self, gid: u16, ch: char, width: u16) {
        self.glyph_widths.insert(gid, width);
        self.unicode_map.insert(gid, ch);
        self.char_to_glyph.insert(ch, gid);
        if gid >= self.next_available_gid {
            self.next_available_gid = gid + 1;
        }
    }

    /// Resolves character to GID; if character was not in original subset,
    /// allocates a new GID and expands the subset!
    pub fn resolve_or_allocate_glyph(&mut self, ch: char) -> (u16, u16) {
        if let Some(&gid) = self.char_to_glyph.get(&ch) {
            let width = self.glyph_widths.get(&gid).copied().unwrap_or(500);
            return (gid, width);
        }

        // Allocate new GID in subset
        let new_gid = self.next_available_gid;
        self.next_available_gid += 1;

        // Default width estimation: 600 for Latin/alphanumeric, 1000 for CJK fullwidth
        let approx_width = if (ch as u32) >= 0x2E80 { 1000 } else { 600 };

        self.glyph_widths.insert(new_gid, approx_width);
        self.unicode_map.insert(new_gid, ch);
        self.char_to_glyph.insert(ch, new_gid);

        (new_gid, approx_width)
    }

    /// Generates updated `/Widths` array for PDF Font dictionary.
    pub fn generate_pdf_widths_array(&self, first_char: u16, last_char: u16) -> Vec<u16> {
        let mut widths = Vec::new();
        for code in first_char..=last_char {
            let w = self.glyph_widths.get(&code).copied().unwrap_or(0);
            widths.push(w);
        }
        widths
    }

    /// Generates updated `/ToUnicode` CMap stream.
    pub fn generate_to_unicode_cmap(&self) -> String {
        let mut cmap = String::new();
        cmap.push_str("/CIDInit /ProcSet findresource begin\n");
        cmap.push_str("12 dict begin\nbegincmap\n");
        cmap.push_str("/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n");
        cmap.push_str("/CMapName /PdfCraft-ToUnicode def\n");
        cmap.push_str("/CMapType 2 def\n");
        cmap.push_str("1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n");

        if !self.unicode_map.is_empty() {
            cmap.push_str(&format!("{} begincidchar\n", self.unicode_map.len()));
            for (&gid, &ch) in &self.unicode_map {
                let u_val = ch as u32;
                cmap.push_str(&format!("<{:04X}> <{:04X}>\n", gid, u_val));
            }
            cmap.push_str("endcidchar\n");
        }

        cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
        cmap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_font_subset_expansion_and_cmap_generation() {
        let mut expander = FontSubsetExpander::new("BAAAAA+TimesNewRomanPSMT");
        // Existing subset only had 'A' and 'B'
        expander.register_existing(1, 'A', 650);
        expander.register_existing(2, 'B', 700);

        // User types 'C' in in-place edit mode
        let (gid_c, width_c) = expander.resolve_or_allocate_glyph('C');
        assert_eq!(gid_c, 3);
        assert!(width_c > 0);

        let cmap = expander.generate_to_unicode_cmap();
        assert!(cmap.contains("<0003> <0043>")); // GID 3 maps to 'C' (0x0043)
    }
}
