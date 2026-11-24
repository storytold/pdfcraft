//! Script and font-slot classification.
//!
//! DrawingML gives every run three typefaces: `a:latin`, `a:ea` (East Asian) and `a:cs` (complex
//! script: Arabic, Hebrew, Thai, Indic…). Which one a character uses follows its Unicode script
//! (UAX #24). Characters with no script of their own (Common and Inherited: spaces, digits,
//! punctuation, combining marks) take the script and slot of the text around them, so a space or
//! a parenthesis between two Arabic words is shaped with them.

/// An ISO 15924 script tag, e.g. `*b"Arab"`.
pub type ScriptTag = [u8; 4];

pub const ARABIC: ScriptTag = *b"Arab";
pub const HEBREW: ScriptTag = *b"Hebr";
pub const LATIN: ScriptTag = *b"Latn";

/// The DrawingML typeface slot a character is drawn with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FontSlot {
    /// `a:latin`.
    #[default]
    Latin,
    /// `a:ea`.
    EastAsian,
    /// `a:cs`.
    ComplexScript,
}

/// Inclusive code point ranges and their script (a subset of `Scripts.txt` covering the scripts
/// the slot decision and shaping need; everything else is Latin-slot or Common).
const RANGES: &[(u32, u32, ScriptTag)] = &[
    (0x0041, 0x005A, *b"Latn"),
    (0x0061, 0x007A, *b"Latn"),
    (0x00AA, 0x00AA, *b"Latn"),
    (0x00BA, 0x00BA, *b"Latn"),
    (0x00C0, 0x00D6, *b"Latn"),
    (0x00D8, 0x00F6, *b"Latn"),
    (0x00F8, 0x024F, *b"Latn"),
    (0x0370, 0x03FF, *b"Grek"),
    (0x0400, 0x052F, *b"Cyrl"),
    (0x0530, 0x058F, *b"Armn"),
    (0x0591, 0x05F4, *b"Hebr"),
    // Arabic block, minus the Common/Inherited code points handled in `script_of`.
    (0x0600, 0x06FF, *b"Arab"),
    (0x0700, 0x074F, *b"Syrc"),
    (0x0750, 0x077F, *b"Arab"),
    (0x0780, 0x07BF, *b"Thaa"),
    (0x07C0, 0x07FF, *b"Nkoo"),
    (0x0870, 0x08FF, *b"Arab"),
    (0x0900, 0x097F, *b"Deva"),
    (0x0980, 0x09FF, *b"Beng"),
    (0x0A00, 0x0A7F, *b"Guru"),
    (0x0A80, 0x0AFF, *b"Gujr"),
    (0x0B00, 0x0B7F, *b"Orya"),
    (0x0B80, 0x0BFF, *b"Taml"),
    (0x0C00, 0x0C7F, *b"Telu"),
    (0x0C80, 0x0CFF, *b"Knda"),
    (0x0D00, 0x0D7F, *b"Mlym"),
    (0x0D80, 0x0DFF, *b"Sinh"),
    (0x0E00, 0x0E7F, *b"Thai"),
    (0x0E80, 0x0EFF, *b"Laoo"),
    (0x0F00, 0x0FFF, *b"Tibt"),
    (0x1000, 0x109F, *b"Mymr"),
    (0x10A0, 0x10FF, *b"Geor"),
    (0x1100, 0x11FF, *b"Hang"),
    (0x1200, 0x139F, *b"Ethi"),
    (0x1780, 0x17FF, *b"Khmr"),
    (0x1E00, 0x1EFF, *b"Latn"),
    (0x1F00, 0x1FFF, *b"Grek"),
    (0x2C60, 0x2C7F, *b"Latn"),
    (0x2E80, 0x2FDF, *b"Hani"),
    (0x3040, 0x309F, *b"Hira"),
    (0x30A0, 0x30FF, *b"Kana"),
    (0x3100, 0x312F, *b"Bopo"),
    (0x3130, 0x318F, *b"Hang"),
    (0x31F0, 0x31FF, *b"Kana"),
    (0x3400, 0x4DBF, *b"Hani"),
    (0x4E00, 0x9FFF, *b"Hani"),
    (0xA720, 0xA7FF, *b"Latn"),
    (0xAC00, 0xD7AF, *b"Hang"),
    (0xF900, 0xFAFF, *b"Hani"),
    (0xFB00, 0xFB06, *b"Latn"),
    (0xFB1D, 0xFB4F, *b"Hebr"),
    (0xFB50, 0xFDFF, *b"Arab"),
    (0xFE70, 0xFEFF, *b"Arab"),
    (0xFF21, 0xFF3A, *b"Latn"),
    (0xFF41, 0xFF5A, *b"Latn"),
    (0xFF66, 0xFF9F, *b"Kana"),
    (0x10E60, 0x10E7F, *b"Arab"),
    (0x1EE00, 0x1EEFF, *b"Arab"),
    (0x20000, 0x323AF, *b"Hani"),
];

/// Code points inside the Arabic blocks whose script is Common or Inherited: the Arabic comma,
/// semicolon and question mark, tatweel, the harakat (combining vowel marks) and the BOM-like
/// U+FEFF. They take the script of the text around them.
fn arabic_common(cp: u32) -> bool {
    matches!(cp, 0x060C | 0x061B | 0x061F | 0x0640 | 0x064B..=0x0655 | 0x0670 | 0xFEFF | 0xFD3E | 0xFD3F)
}

/// The script of `c`, or `None` for Common and Inherited characters (spaces, digits,
/// punctuation, combining marks), which take the script of their neighbours.
pub fn script_of(c: char) -> Option<ScriptTag> {
    let cp = c as u32;
    if arabic_common(cp) {
        return None;
    }
    // Ranges are sorted and disjoint: binary search on the start.
    let i = RANGES.partition_point(|(lo, _, _)| *lo <= cp);
    let (lo, hi, tag) = RANGES.get(i.checked_sub(1)?)?;
    (cp >= *lo && cp <= *hi).then_some(*tag)
}

/// The slot a script is drawn with.
pub fn slot_of_script(s: ScriptTag) -> FontSlot {
    match &s {
        b"Hani" | b"Hira" | b"Kana" | b"Hang" | b"Bopo" => FontSlot::EastAsian,
        b"Arab" | b"Hebr" | b"Syrc" | b"Thaa" | b"Nkoo" | b"Deva" | b"Beng" | b"Guru" | b"Gujr" | b"Orya" | b"Taml" | b"Telu" | b"Knda" | b"Mlym"
        | b"Sinh" | b"Thai" | b"Laoo" | b"Tibt" | b"Mymr" | b"Khmr" => FontSlot::ComplexScript,
        _ => FontSlot::Latin,
    }
}

/// The slot of `c` on its own: `None` for characters that take their neighbours' slot. CJK
/// punctuation and full-width forms are East Asian although their script is Common.
pub fn slot_of(c: char) -> Option<FontSlot> {
    if let Some(s) = script_of(c) {
        return Some(slot_of_script(s));
    }
    let cp = c as u32;
    if (0x3000..=0x303F).contains(&cp) || (0xFF00..=0xFFEF).contains(&cp) || (0x3200..=0x33FF).contains(&cp) {
        return Some(FontSlot::EastAsian);
    }
    None
}

/// A default BCP 47 language for a script (shaping picks `locl` forms by language).
pub fn default_language(s: ScriptTag) -> Option<&'static str> {
    Some(match &s {
        b"Arab" => "ar",
        b"Hebr" => "he",
        b"Syrc" => "syr",
        b"Thaa" => "dv",
        b"Thai" => "th",
        b"Deva" => "hi",
        _ => return None,
    })
}

/// Resolve every character's script and slot, giving Common/Inherited characters the value of the
/// preceding character with one (or, at the start, the following one; Latin when there is none).
pub fn resolve(chars: &[char]) -> Vec<(Option<ScriptTag>, FontSlot)> {
    let own: Vec<(Option<ScriptTag>, Option<FontSlot>)> = chars.iter().map(|c| (script_of(*c), slot_of(*c))).collect();
    let first = own.iter().find_map(|(s, sl)| sl.map(|sl| (*s, sl))).unwrap_or((None, FontSlot::Latin));
    let mut prev = first;
    own.into_iter()
        .map(|(s, sl)| {
            if let Some(sl) = sl {
                prev = (s, sl);
            }
            prev
        })
        .collect()
}

/// Arabic letters that join to the following letter (joining type D, dual-joining): a kashida
/// (tatweel) may be inserted after them. Right-joining (alef, dal, reh, waw…) and non-joining
/// (hamza) letters are excluded.
pub fn joins_following(c: char) -> bool {
    let cp = c as u32;
    let letter = matches!(cp, 0x0620..=0x064A | 0x066E..=0x06D3 | 0x06FA..=0x06FC | 0x06FF | 0x0750..=0x077F | 0x08A0..=0x08C9);
    letter
        && !matches!(
            cp,
            0x0621..=0x0625 | 0x0627 | 0x0629 | 0x062F..=0x0632 | 0x0648 | 0x0671..=0x0673 | 0x0675..=0x0677 | 0x0688..=0x0699 | 0x06C0
                | 0x06C3..=0x06CB | 0x06CD | 0x06CF | 0x06D2 | 0x06D3 | 0x0759..=0x075B | 0x076B | 0x076C | 0x0771 | 0x0773 | 0x0774
                | 0x0778 | 0x0779 | 0x08AA..=0x08AC | 0x08AE | 0x08B1 | 0x08B2 | 0x08B9
        )
}

/// An Arabic letter that joins to the preceding letter (joining type D or R).
pub fn joins_preceding(c: char) -> bool {
    let cp = c as u32;
    matches!(cp, 0x0622..=0x064A | 0x066E..=0x06D3 | 0x06FA..=0x06FC | 0x06FF | 0x0750..=0x077F | 0x08A0..=0x08C9) && cp != 0x0621
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_and_slots() {
        assert_eq!(script_of('س'), Some(ARABIC));
        assert_eq!(script_of('ש'), Some(HEBREW));
        assert_eq!(script_of('R'), Some(LATIN));
        assert_eq!(script_of('2'), None);
        assert_eq!(script_of('('), None);
        // Harakat and tatweel are Inherited/Common.
        assert_eq!(script_of('\u{064E}'), None);
        assert_eq!(script_of('\u{0640}'), None);
        assert_eq!(slot_of('س'), Some(FontSlot::ComplexScript));
        assert_eq!(slot_of('日'), Some(FontSlot::EastAsian));
        assert_eq!(slot_of('、'), Some(FontSlot::EastAsian));
        assert_eq!(slot_of('a'), Some(FontSlot::Latin));
        assert_eq!(slot_of(' '), None);
        assert_eq!(script_of('\u{10FFFF}'), None);
        assert_eq!(script_of('\0'), None);
    }

    #[test]
    fn neutrals_take_the_surrounding_slot() {
        let chars: Vec<char> = "سنة 2024 (Retrait) فقط".chars().collect();
        let r = resolve(&chars);
        // "سنة 2024 (" follows Arabic: complex script.
        for (i, c) in chars.iter().enumerate().take(10) {
            assert_eq!(r[i].1, FontSlot::ComplexScript, "{c:?}");
        }
        // "Retrait)" is Latin; the space after ")" stays Latin; "فقط" is complex script.
        assert_eq!(r[10].1, FontSlot::Latin);
        assert_eq!(r[17].1, FontSlot::Latin);
        assert_eq!(r[19], (Some(ARABIC), FontSlot::ComplexScript));
        // Leading neutrals take the first strong character's slot.
        let r = resolve(&['(', 'س']);
        assert_eq!(r[0].1, FontSlot::ComplexScript);
        assert!(resolve(&[]).is_empty());
        assert_eq!(resolve(&['1'])[0], (None, FontSlot::Latin));
    }

    #[test]
    fn joining_classes() {
        assert!(joins_following('س'));
        assert!(joins_following('ن'));
        assert!(!joins_following('ا'));
        assert!(!joins_following('د'));
        assert!(!joins_following('ء'));
        assert!(joins_preceding('ا'));
        assert!(!joins_preceding('ء'));
        assert!(!joins_following('a'));
    }

    #[test]
    fn ranges_are_sorted_and_disjoint() {
        for w in RANGES.windows(2) {
            assert!(w[0].1 < w[1].0, "{:x?} {:x?}", w[0], w[1]);
        }
    }
}
