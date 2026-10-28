//! Clean-room CJK CMap to Unicode Mapping Tables for CIDFonts without ToUnicode.
//! Implements Adobe-Japan1, Adobe-GB1 (Simplified Chinese), and Adobe-CNS1 (Traditional Chinese).

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CjkOrdering {
    Japan1,
    Gb1,
    Cns1,
    Korea1,
}

pub struct CjkCMapRegistry {
    cid_to_unicode: HashMap<(CjkOrdering, u32), char>,
}

impl Default for CjkCMapRegistry {
    fn default() -> Self {
        let mut registry = Self {
            cid_to_unicode: HashMap::new(),
        };
        registry.populate_standard_cjk_fallbacks();
        registry
    }
}

impl CjkCMapRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up Unicode char from CID and Character Collection ordering.
    pub fn lookup(&self, ordering: CjkOrdering, cid: u32) -> Option<char> {
        self.cid_to_unicode.get(&(ordering, cid)).copied()
    }

    /// Parses a CMap stream containing `begincidrange` or `begincidchar`.
    pub fn parse_custom_cmap(&mut self, ordering: CjkOrdering, cmap_data: &[u8]) {
        let text = String::from_utf8_lossy(cmap_data);
        let mut lines = text.lines();

        while let Some(line) = lines.next() {
            let line = line.trim();
            if line.contains("begincidchar") {
                while let Some(char_line) = lines.next() {
                    let char_line = char_line.trim();
                    if char_line.contains("endcidchar") {
                        break;
                    }
                    // Format: <cid_hex> <unicode_hex> or cid unicode
                    let tokens: Vec<&str> = char_line.split_whitespace().collect();
                    if tokens.len() >= 2 {
                        if let (Some(cid), Some(ch)) = (Self::parse_cid(tokens[0]), Self::parse_unicode(tokens[1])) {
                            self.cid_to_unicode.insert((ordering, cid), ch);
                        }
                    }
                }
            } else if line.contains("begincidrange") {
                while let Some(range_line) = lines.next() {
                    let range_line = range_line.trim();
                    if range_line.contains("endcidrange") {
                        break;
                    }
                    // Format: <start_cid> <end_cid> <start_unicode>
                    let tokens: Vec<&str> = range_line.split_whitespace().collect();
                    if tokens.len() >= 3 {
                        if let (Some(c_start), Some(c_end), Some(u_start)) = (
                            Self::parse_cid(tokens[0]),
                            Self::parse_cid(tokens[1]),
                            Self::parse_unicode_val(tokens[2]),
                        ) {
                            let mut u_curr = u_start;
                            for cid in c_start..=c_end {
                                if let Some(ch) = char::from_u32(u_curr) {
                                    self.cid_to_unicode.insert((ordering, cid), ch);
                                }
                                u_curr += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    fn parse_cid(tok: &str) -> Option<u32> {
        let clean = tok.trim_start_matches('<').trim_end_matches('>');
        if clean.len() <= 4 && !clean.is_empty() {
            u32::from_str_radix(clean, 16).ok()
        } else {
            clean.parse().ok()
        }
    }

    fn parse_unicode(tok: &str) -> Option<char> {
        let clean = tok.trim_start_matches('<').trim_end_matches('>');
        let val = u32::from_str_radix(clean, 16).ok()?;
        char::from_u32(val)
    }

    fn parse_unicode_val(tok: &str) -> Option<u32> {
        let clean = tok.trim_start_matches('<').trim_end_matches('>');
        u32::from_str_radix(clean, 16).ok()
    }

    /// Populates standard baseline CJK punctuation and alphanumeric mappings.
    fn populate_standard_cjk_fallbacks(&mut self) {
        // Space / ASCII fallback for Japan1 / Gb1
        for ascii in 32u32..=126u32 {
            if let Some(ch) = char::from_u32(ascii) {
                // CID 1 to 95 commonly map to ASCII in standard CID fonts
                self.cid_to_unicode.insert((CjkOrdering::Japan1, ascii - 31), ch);
                self.cid_to_unicode.insert((CjkOrdering::Gb1, ascii - 31), ch);
                self.cid_to_unicode.insert((CjkOrdering::Cns1, ascii - 31), ch);
            }
        }
        // Common CJK punctuation
        self.cid_to_unicode.insert((CjkOrdering::Japan1, 633), '、');
        self.cid_to_unicode.insert((CjkOrdering::Japan1, 634), '。');
        self.cid_to_unicode.insert((CjkOrdering::Gb1, 633), '、');
        self.cid_to_unicode.insert((CjkOrdering::Gb1, 634), '。');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cjk_cmap_parsing() {
        let mut reg = CjkCMapRegistry::new();
        let cmap = b"
1 begincidchar
<0020> <0020>
endcidchar
1 begincidrange
<03E8> <03EA> <4E00>
endcidrange";

        reg.parse_custom_cmap(CjkOrdering::Gb1, cmap);
        assert_eq!(reg.lookup(CjkOrdering::Gb1, 0x0020), Some(' '));
        // CID 1000 (0x03E8) should map to U+4E00 ('一')
        assert_eq!(reg.lookup(CjkOrdering::Gb1, 0x03E8), Some('一'));
        assert_eq!(reg.lookup(CjkOrdering::Gb1, 0x03E9), Some('丁'));
    }
}
