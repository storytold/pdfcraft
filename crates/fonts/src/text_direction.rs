//! Paragraph base direction, independent of alignment and logical Unicode storage.
use unicode_bidi::{Direction, Level};

/// Bound the temporary Unicode levels and visual-unit inventory of a single row.
pub const MAX_DIRECTION_BYTES: usize = 262_144;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextDirection {
    #[default]
    Auto,
    LeftToRight,
    RightToLeft,
}

impl TextDirection {
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::LeftToRight => "LTR",
            Self::RightToLeft => "RTL",
        }
    }

    pub fn from_name(name: &[u8]) -> Self {
        match name {
            b"LTR" => Self::LeftToRight,
            b"RTL" => Self::RightToLeft,
            _ => Self::Auto,
        }
    }

    pub fn rtl(self, text: &str) -> bool {
        match self {
            Self::Auto => unicode_bidi::get_base_direction(text) == Direction::Rtl,
            Self::LeftToRight => false,
            Self::RightToLeft => true,
        }
    }

    pub fn level(self, text: &str) -> Level {
        if self.rtl(text) { Level::rtl() } else { Level::ltr() }
    }

    /// Visual order for an unshaped simple-font line. Complex-script callers must
    /// use source_shaping instead, which retains clusters and performs OpenType shaping.
    pub fn visual_plain(self, text: &str) -> Result<String, &'static str> {
        if text.len() > MAX_DIRECTION_BYTES {
            return Err("a directional text line exceeds 262144 UTF-8 bytes; shorten it before laying it out");
        }
        // Empty fields have no runs; unicode-bidi requires a nonempty visual-run range.
        if text.is_empty() {
            return Ok(String::new());
        }
        let info = unicode_bidi::ParagraphBidiInfo::new(text, Some(self.level(text)));
        let (levels, runs) = info.visual_runs(0..text.len());
        let mut out = String::with_capacity(text.len());
        for run in runs {
            let Some(part) = text.get(run.clone()) else { continue };
            if levels.get(run.start).is_some_and(Level::is_rtl) {
                // This helper only owns simple-font mirroring. Other neutral symbols may
                // need Unicode mirror pairs outside WinAnsi; leave those to OpenType shaping.
                if part.chars().any(|c| unicode_bidi::bidi_class(c) == unicode_bidi::BidiClass::ON && !crate::win_ansi_encodable(c)) {
                    return Err("directional text with non-WinAnsi symbols needs a complete font and OpenType shaping");
                }
                // L3: reversing visual units must not detach nonspacing marks.
                // Complex scripts still require the source-font shaping path.
                let mut units: Vec<String> = Vec::new();
                for c in part.chars() {
                    if unicode_bidi::bidi_class(c) == unicode_bidi::BidiClass::NSM
                        && let Some(unit) = units.last_mut()
                    {
                        unit.push(c);
                    } else {
                        units.push(c.to_string());
                    }
                }
                for unit in units.iter().rev() {
                    out.extend(unit.chars().map(|c| match c {
                        '(' => ')',
                        ')' => '(',
                        '[' => ']',
                        ']' => '[',
                        '{' => '}',
                        '}' => '{',
                        '<' => '>',
                        '>' => '<',
                        '«' => '»',
                        '»' => '«',
                        '‹' => '›',
                        '›' => '‹',
                        other => other,
                    }));
                }
            } else {
                out.push_str(part);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_directional_text_has_no_visual_runs() {
        for direction in [TextDirection::Auto, TextDirection::LeftToRight, TextDirection::RightToLeft] {
            assert_eq!(direction.visual_plain("").unwrap(), "");
        }
    }

    #[test]
    fn explicit_base_changes_visual_order_without_changing_logical_text() {
        let logical = "123 ABC";
        assert_eq!(TextDirection::Auto.visual_plain(logical).unwrap(), "123 ABC");
        assert_eq!(TextDirection::LeftToRight.visual_plain(logical).unwrap(), "123 ABC");
        assert_eq!(TextDirection::RightToLeft.visual_plain(logical).unwrap(), "ABC 123");
        assert_eq!(logical, "123 ABC");
        assert_eq!(TextDirection::from_name(b"RTL"), TextDirection::RightToLeft);
        assert_eq!(TextDirection::from_name(&[0xff]), TextDirection::Auto);
    }

    #[test]
    fn simple_visual_units_keep_nonspacing_marks_attached() {
        assert_eq!(TextDirection::RightToLeft.visual_plain("שָׁם").unwrap(), "םשָׁ");
        assert_eq!(TextDirection::LeftToRight.visual_plain("ABC e\u{301}").unwrap(), "ABC e\u{301}");
        assert!(TextDirection::RightToLeft.visual_plain(&"a".repeat(MAX_DIRECTION_BYTES.saturating_add(1))).is_err());
    }
}
