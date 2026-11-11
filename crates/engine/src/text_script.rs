//! Superscript and subscript as Unicode characters (Edit a PDF ▸ Format text): `2` ↔ `²` and
//! `2` ↔ `₂`.
//!
//! The text keeps its font, size and baseline; its characters change to the forms Unicode encodes
//! for raised and lowered text, so the font must have those glyphs, and extracted text contains
//! them. Characters Unicode has no such form for are left as they are.
//!
//! A selection mixing plain and target-script characters is normalized: `H₂2` becomes `H₂₂`,
//! then `H22` on the next subscript toggle; superscript follows the same policy. Two toggles
//! restore uniformly plain or uniformly scripted input, but do not reconstruct an original
//! mixture. Restoring that exact prior text requires the caller's edit history (Undo).

use std::ops::Range;

/// Raised or lowered text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    Super,
    Sub,
}

/// Each character Unicode raises, and its superscript form.
const SUPERSCRIPT: &[(char, char)] = &[
    ('0', '⁰'),
    ('1', '¹'),
    ('2', '²'),
    ('3', '³'),
    ('4', '⁴'),
    ('5', '⁵'),
    ('6', '⁶'),
    ('7', '⁷'),
    ('8', '⁸'),
    ('9', '⁹'),
    ('+', '⁺'),
    ('−', '⁻'),
    ('=', '⁼'),
    ('(', '⁽'),
    (')', '⁾'),
    ('a', 'ᵃ'),
    ('b', 'ᵇ'),
    ('c', 'ᶜ'),
    ('d', 'ᵈ'),
    ('e', 'ᵉ'),
    ('f', 'ᶠ'),
    ('g', 'ᵍ'),
    ('h', 'ʰ'),
    ('i', 'ⁱ'),
    ('j', 'ʲ'),
    ('k', 'ᵏ'),
    ('l', 'ˡ'),
    ('m', 'ᵐ'),
    ('n', 'ⁿ'),
    ('o', 'ᵒ'),
    ('p', 'ᵖ'),
    ('r', 'ʳ'),
    ('s', 'ˢ'),
    ('t', 'ᵗ'),
    ('u', 'ᵘ'),
    ('v', 'ᵛ'),
    ('w', 'ʷ'),
    ('x', 'ˣ'),
    ('y', 'ʸ'),
    ('z', 'ᶻ'),
    ('A', 'ᴬ'),
    ('B', 'ᴮ'),
    ('D', 'ᴰ'),
    ('E', 'ᴱ'),
    ('G', 'ᴳ'),
    ('H', 'ᴴ'),
    ('I', 'ᴵ'),
    ('J', 'ᴶ'),
    ('K', 'ᴷ'),
    ('L', 'ᴸ'),
    ('M', 'ᴹ'),
    ('N', 'ᴺ'),
    ('O', 'ᴼ'),
    ('P', 'ᴾ'),
    ('R', 'ᴿ'),
    ('T', 'ᵀ'),
    ('U', 'ᵁ'),
    ('V', 'ⱽ'),
    ('W', 'ᵂ'),
];

/// Each character Unicode lowers, and its subscript form.
const SUBSCRIPT: &[(char, char)] = &[
    ('0', '₀'),
    ('1', '₁'),
    ('2', '₂'),
    ('3', '₃'),
    ('4', '₄'),
    ('5', '₅'),
    ('6', '₆'),
    ('7', '₇'),
    ('8', '₈'),
    ('9', '₉'),
    ('+', '₊'),
    ('−', '₋'),
    ('=', '₌'),
    ('(', '₍'),
    (')', '₎'),
    ('a', 'ₐ'),
    ('e', 'ₑ'),
    ('h', 'ₕ'),
    ('i', 'ᵢ'),
    ('j', 'ⱼ'),
    ('k', 'ₖ'),
    ('l', 'ₗ'),
    ('m', 'ₘ'),
    ('n', 'ₙ'),
    ('o', 'ₒ'),
    ('p', 'ₚ'),
    ('r', 'ᵣ'),
    ('s', 'ₛ'),
    ('t', 'ₜ'),
    ('u', 'ᵤ'),
    ('v', 'ᵥ'),
    ('x', 'ₓ'),
];

impl Script {
    fn pairs(self) -> &'static [(char, char)] {
        match self {
            Script::Super => SUPERSCRIPT,
            Script::Sub => SUBSCRIPT,
        }
    }

    /// `c` raised or lowered, or `None` when Unicode has no distinct reversible form for it.
    /// Mathematical minus U+2212 has a form; ASCII hyphen-minus stays unchanged to avoid a lossy collision.
    pub fn form(self, c: char) -> Option<char> {
        self.pairs().iter().find(|(plain, _)| *plain == c).map(|&(_, form)| form)
    }

    /// The character `c` is the raised or lowered form of, or `None` when it is no such form.
    pub fn plain(self, c: char) -> Option<char> {
        self.pairs().iter().find(|(_, form)| *form == c).map(|&(plain, _)| plain)
    }
}

/// `range` (UTF-8 bytes of `text`) clamped to it and widened to whole characters, or all of `text`
/// when there is none.
fn span(text: &str, range: Option<&Range<usize>>) -> Range<usize> {
    let Some(range) = range else { return 0..text.len() };
    let mut start = range.start.min(text.len());
    while !text.is_char_boundary(start) {
        start = start.saturating_sub(1);
    }
    let mut end = range.end.min(text.len());
    while !text.is_char_boundary(end) {
        end = end.saturating_add(1);
    }
    start..end
}

/// Whether the characters of `text` in `range` (UTF-8 bytes; all of it when `None`) are already in
/// `script`: at least one of them is, and none of the others could be. Characters Unicode has no
/// form for (a space, a `q`) don't count toward that state.
pub fn is_in(text: &str, range: Option<&Range<usize>>, script: Script) -> bool {
    let target = text.get(span(text, range)).unwrap_or_default();
    target.chars().any(|c| script.plain(c).is_some()) && target.chars().all(|c| script.plain(c).is_some() || script.form(c).is_none())
}

/// Superscript or Subscript on the characters of `text` in `range` (UTF-8 bytes; all of it when
/// `None`): back to plain characters when they are already in `script` ([`is_in`]), and otherwise
/// each character that has a form takes it. Returns the new text and the range the converted
/// characters now take, or `None` when nothing changes.
///
/// Mixed plain/scripted input normalizes to the target script and then to plain text; for
/// example, `H₂2` → `H₂₂` → `H22`. This stateless conversion does not remember the original
/// mixture. Use edit history to restore it, rather than treating a second toggle as Undo.
pub fn toggle(text: &str, range: Option<&Range<usize>>, script: Script) -> Option<(String, Range<usize>)> {
    let span = span(text, range);
    let target = text.get(span.clone()).filter(|t| !t.is_empty())?;
    let converted: String = if is_in(text, Some(&span), script) {
        target.chars().map(|c| script.plain(c).unwrap_or(c)).collect()
    } else {
        target.chars().map(|c| script.form(c).unwrap_or(c)).collect()
    };
    if converted == target {
        return None;
    }
    let mut out = String::with_capacity(text.len().saturating_add(converted.len()));
    out.push_str(text.get(..span.start).unwrap_or_default());
    out.push_str(&converted);
    out.push_str(text.get(span.end..).unwrap_or_default());
    let end = span.start.saturating_add(converted.len());
    Some((out, span.start..end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_form_toggles_back_to_its_character() {
        for script in [Script::Super, Script::Sub] {
            for &(plain, form) in script.pairs() {
                let s = plain.to_string();
                let (raised, range) = toggle(&s, None, script).unwrap();
                assert_eq!(raised, form.to_string());
                assert_eq!(range, 0..raised.len());
                assert_eq!(toggle(&raised, None, script).unwrap().0, s, "{script:?} {plain}");
            }
        }
        assert_eq!(toggle("-", None, Script::Super), None);
        assert_eq!(toggle("-", None, Script::Sub), None);
    }

    #[test]
    fn review_script_round_trips_preserve_both_minus_characters() {
        for script in [Script::Super, Script::Sub] {
            let text = "−2 -3";
            let once = toggle(text, None, script).unwrap().0;
            assert_eq!(toggle(&once, None, script).unwrap().0, text);
        }
    }

    #[test]
    fn text_with_characters_unicode_cannot_raise_toggles_back() {
        // q has no superscript form and Q no subscript one; the space neither.
        for (script, text, toggled) in [(Script::Super, "x2 q", "ˣ² q"), (Script::Sub, "CO2 Q", "CO₂ Q")] {
            let (once, _) = toggle(text, None, script).unwrap();
            assert_eq!(once, toggled);
            assert!(is_in(&once, None, script));
            assert_eq!(toggle(&once, None, script).unwrap().0, text, "{script:?}");
        }
        // Nothing to raise: nothing changes.
        assert_eq!(toggle("q Q", None, Script::Super), None);
        assert!(!is_in("q Q", None, Script::Super));
    }

    #[test]
    fn a_range_converts_only_its_characters_and_follows_them() {
        let (text, range) = toggle("E = mc2", Some(&(6..7)), Script::Super).unwrap();
        assert_eq!((text.as_str(), range.clone()), ("E = mc²", 6..8));
        assert_eq!(toggle(&text, Some(&range), Script::Super).unwrap(), ("E = mc2".to_string(), 6..7));
        // A mixed range takes the form throughout.
        assert_eq!(toggle("H₂2", None, Script::Sub).unwrap().0, "H₂₂");
        assert!(!is_in("H₂2", None, Script::Sub));
    }

    #[test]
    fn mixed_plain_and_scripted_text_normalizes_instead_of_round_tripping() {
        for (script, mixed, uniform, plain) in [(Script::Super, "Q²2", "Q²²", "Q22"), (Script::Sub, "H₂2", "H₂₂", "H22")] {
            assert!(!is_in(mixed, None, script));
            let (once, range) = toggle(mixed, None, script).unwrap();
            assert_eq!(once, uniform);
            assert_eq!(range, 0..uniform.len());
            assert!(is_in(&once, None, script));
            let (twice, range) = toggle(&once, None, script).unwrap();
            assert_eq!(twice, plain);
            assert_eq!(range, 0..plain.len());
            assert_ne!(twice, mixed, "a second toggle normalizes the original mixture");
            assert_eq!(toggle(&twice, None, script).unwrap().0, uniform);
        }
    }

    #[test]
    fn hostile_ranges_are_clamped_to_whole_characters() {
        // Byte 1 is inside ²: widened to the whole character on each side.
        assert_eq!(toggle("²x", Some(&(1..1)), Script::Super).unwrap(), ("2x".to_string(), 0..1));
        assert_eq!(toggle("²x", Some(&(2..2)), Script::Super), None);
        assert_eq!(toggle("x2", Some(&(1..usize::MAX)), Script::Super).unwrap(), ("x²".to_string(), 1..3));
        assert_eq!(toggle("x2", Some(&(5..9)), Script::Super), None);
        #[allow(clippy::reversed_empty_ranges)]
        let backwards = 2..1;
        assert_eq!(toggle("x2", Some(&backwards), Script::Super), None);
        assert_eq!(toggle("", None, Script::Sub), None);
        assert!(!is_in("", None, Script::Sub));
    }
}
