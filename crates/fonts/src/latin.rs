//! Central European letters in the standard 14 fonts (#2098). WinAnsiEncoding has codes for
//! š ž Š Ž but not for č ć đ ł ő ę and the rest of Latin Extended-A, which the twelve text faces
//! nonetheless carry. Text that uses them is written with an `/Encoding` whose `/Differences` give
//! the letters it uses codes of their own ([`LatinEncoding`]), drawn from the viewer's standard
//! fonts (ISO 32000-2 §9.6.5), so nothing is embedded. The font also gets `/Widths` and a
//! `/ToUnicode` CMap ([`LatinEncoding::widths`], [`LatinEncoding::to_unicode`]): checked in a
//! widely used viewer (review on #3076), it took neither the widths nor the text from the glyph
//! names, so the letters were drawn on top of one another and copied without their accents.

use crate::std14::Std14;
use crate::std14_metrics::EXTRA_LETTERS;

/// Codes WinAnsiEncoding leaves undefined, given to the extra letters first: 0x7F and the five
/// gaps above 0x80, then the control codes but tab, line feed and carriage return.
const FREE_CODES: [u8; 35] = [
    0x7F, 0x81, 0x8D, 0x8F, 0x90, 0x9D, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x0B, 0x0C, 0x0E, 0x0F, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15,
    0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F, 0x00,
];

/// The standard glyph name of `c` when it is a letter the twelve text faces have beyond WinAnsi.
pub fn extra_letter(c: char) -> Option<&'static str> {
    extra(c).map(|(name, _)| name)
}

/// The extra letter whose standard glyph name is `name`.
pub(crate) fn extra_letter_named(name: &str) -> Option<char> {
    EXTRA_LETTERS.iter().find(|(_, n, _)| *n == name).map(|(c, ..)| *c)
}

fn extra(c: char) -> Option<(&'static str, &'static [u16; 12])> {
    // EXTRA_LETTERS is in code point order (`extra_letters_are_in_code_point_order`).
    let i = EXTRA_LETTERS.binary_search_by_key(&c, |(l, ..)| *l).ok()?;
    EXTRA_LETTERS.get(i).map(|(_, name, widths)| (*name, widths))
}

/// Whether the standard text fonts can draw `c`: in WinAnsi (a tab counts, as a space) or one of
/// the extra letters.
pub fn latin_drawable(c: char) -> bool {
    crate::win_ansi_encodable(c) || extra(c).is_some()
}

/// The first character of `s` the standard text fonts can't draw ([`latin_drawable`]).
pub fn first_non_latin(s: &str) -> Option<char> {
    s.chars().find(|c| !latin_drawable(*c))
}

impl Std14 {
    /// The advance of `text` at `size` points, written as [`LatinEncoding`] writes it: WinAnsi
    /// widths, and the standard widths of the extra letters. Anything else counts as the `?`
    /// [`crate::win_ansi`] writes for it.
    pub fn latin_text_width(self, text: &str, size: f64) -> f64 {
        let mut units = 0.0;
        for c in text.chars() {
            units += match (extra(c), self.text_face()) {
                (Some((_, widths)), Some(face)) => widths.get(face).copied().map_or(0.0, f64::from),
                _ => crate::win_ansi(&c.to_string()).iter().map(|&b| self.width(b)).sum(),
            };
        }
        units * size / 1000.0
    }

    /// The index of a text face among the twelve, in `BASE_FONTS` order; `None` for Symbol and
    /// ZapfDingbats.
    fn text_face(self) -> Option<usize> {
        let i = self as usize;
        (i < 12).then_some(i)
    }
}

/// An encoding for one text: WinAnsiEncoding, with the extra letters the text uses at codes of
/// their own. Free codes come first; past those, a code of a WinAnsi character the text doesn't
/// use. Built from the text alone, so the same text always gets the same encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LatinEncoding {
    /// The extra letters, in order of first use, with their codes and glyph names.
    letters: Vec<(char, u8, &'static str)>,
}

impl LatinEncoding {
    /// The encoding for `text`, or `None` when it needs none (all of it is WinAnsi) or can't
    /// have one (a character neither WinAnsi nor an extra letter, or more extra letters than
    /// codes to spare).
    pub fn for_text(text: &str) -> Option<Self> {
        Self::assign(text).ok().flatten()
    }

    /// The first extra letter of `text` left without a code: more extra letters than the codes
    /// the text leaves to spare, so it would be written as `?` (review on #3076).
    pub fn first_without_code(text: &str) -> Option<char> {
        Self::assign(text).err()
    }

    /// Codes for the extra letters of `text`: `Ok(None)` when it needs or can have none, `Err`
    /// with the first letter left without a code.
    fn assign(text: &str) -> Result<Option<Self>, char> {
        // Lines are laid out apart, so their separators aren't text the encoding has to show
        // (review on #3076: `č\nć` got no codes and both letters were written as `?`).
        if text.split('\n').find_map(first_non_latin).is_some() {
            return Ok(None);
        }
        let mut wanted: Vec<(char, &'static str)> = Vec::new();
        for c in text.chars() {
            if let Some((name, _)) = extra(c)
                && !wanted.iter().any(|(w, _)| *w == c)
            {
                wanted.push((c, name));
            }
        }
        if wanted.is_empty() {
            return Ok(None);
        }
        let used: std::collections::HashSet<u8> = text.chars().filter_map(crate::win_ansi_byte).collect();
        let defined = |b: u8| b >= 0xA0 || crate::encodings::WIN_ANSI.get(usize::from(b)).is_some_and(|u| *u != 0);
        let spare = (0x80..=0xFFu8).filter(|b| !used.contains(b) && defined(*b));
        let mut codes = FREE_CODES.iter().copied().chain(spare);
        let mut letters = Vec::with_capacity(wanted.len());
        for (c, name) in wanted {
            letters.push((c, codes.next().ok_or(c)?, name));
        }
        Ok(Some(Self { letters }))
    }

    /// `s` in this encoding; characters it can't write become `?`, as [`crate::win_ansi`] does.
    pub fn encode(&self, s: &str) -> Vec<u8> {
        s.chars()
            .map(|c| match self.letters.iter().find(|(l, ..)| *l == c) {
                Some(&(_, code, _)) => code,
                None if c == '\t' => b' ',
                None => crate::win_ansi_byte(c).unwrap_or(b'?'),
            })
            .collect()
    }

    /// The `/Widths` of codes 0 to 255 (`/FirstChar 0 /LastChar 255`) in `font`: each extra
    /// letter's Core14 width at its code, the WinAnsi widths elsewhere. A viewer that doesn't
    /// take widths from the `/Differences` glyph names (a widely used one doesn't) would otherwise
    /// give the letters' codes no advance.
    pub fn widths(&self, font: Std14) -> Vec<f64> {
        (0..=255u8)
            .map(|code| match self.letters.iter().find(|&&(_, at, _)| at == code) {
                Some(&(c, ..)) => match (extra(c), font.text_face()) {
                    (Some((_, widths)), Some(face)) => widths.get(face).copied().map_or(0.0, f64::from),
                    _ => 0.0,
                },
                None => font.width(code),
            })
            .collect()
    }

    /// A `/ToUnicode` CMap for this encoding: each extra letter's code to the letter, every
    /// other code WinAnsi defines to its character. Text is read from it before the glyph
    /// names (ISO 32000-2 §9.10.2).
    pub fn to_unicode(&self) -> String {
        let mut pairs: Vec<(u8, char)> = Vec::new();
        for code in 0..=255u8 {
            let c = match self.letters.iter().find(|&&(_, at, _)| at == code) {
                Some(&(c, ..)) => Some(c),
                None => crate::encodings::WIN_ANSI.get(usize::from(code)).copied().filter(|u| *u != 0).and_then(char::from_u32),
            };
            if let Some(c) = c {
                pairs.push((code, c));
            }
        }
        let mut out = String::from(
            "/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def
             /CMapName /Adobe-Identity-UCS def
/CMapType 2 def
1 begincodespacerange
<00> <FF>
endcodespacerange
",
        );
        // At most 100 mappings per block (ISO 32000-2 §9.10.3).
        for block in pairs.chunks(100) {
            out.push_str(&format!(
                "{} beginbfchar
",
                block.len()
            ));
            for &(code, c) in block {
                let mut units = [0u16; 2];
                let hex: String = c.encode_utf16(&mut units).iter().map(|u| format!("{u:04X}")).collect();
                out.push_str(&format!(
                    "<{code:02X}> <{hex}>
"
                ));
            }
            out.push_str(
                "endbfchar
",
            );
        }
        out.push_str(
            "endcmap
CMapName currentdict /CMap defineresource pop
end
end
",
        );
        out
    }

    /// The `/Differences` array: each extra letter's code and glyph name, in code order.
    pub fn differences(&self) -> Vec<(u8, &'static str)> {
        let mut d: Vec<(u8, &'static str)> = self.letters.iter().map(|&(_, code, name)| (code, name)).collect();
        d.sort_unstable();
        d
    }

    /// A short tag naming this encoding, for a font resource name: the same differences give
    /// the same tag, so items with the same letters share one font entry and different ones
    /// never collide.
    pub fn tag(&self) -> String {
        // FNV-1a over the differences: stable across builds and platforms.
        let mut h: u32 = 0x811C_9DC5;
        for (code, name) in self.differences() {
            for b in std::iter::once(code).chain(name.bytes()).chain(std::iter::once(b' ')) {
                h ^= u32::from(b);
                h = h.wrapping_mul(0x0100_0193);
            }
        }
        format!("{h:08x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `extra` looks letters up by binary search.
    #[test]
    fn extra_letters_are_in_code_point_order() {
        assert!(EXTRA_LETTERS.windows(2).all(|w| w[0].0 < w[1].0));
        for (c, name, _) in &EXTRA_LETTERS {
            assert_eq!(extra_letter(*c), Some(*name));
        }
        assert_eq!(extra_letter('a'), None);
    }

    /// Review on #3076: a widely used viewer took neither the widths nor the text from the glyph
    /// names, so the font carries both by code. Each extra letter's code has its Core14 width and
    /// maps to the letter; the other codes keep their WinAnsi width and character.
    #[test]
    fn the_encoding_gives_widths_and_text_by_code() {
        let e = LatinEncoding::for_text("čćđ Đakovo").expect("an encoding");
        let code = |c: char| e.encode(&c.to_string())[0];
        let w = e.widths(Std14::Helvetica);
        assert_eq!(w.len(), 256);
        assert_eq!(w[usize::from(code('č'))], 500.0, "Helvetica ccaron");
        assert_eq!(w[usize::from(code('Đ'))], 722.0, "Helvetica Dcroat");
        assert_eq!(w[usize::from(b'a')], 556.0, "WinAnsi a");
        assert_eq!(e.widths(Std14::TimesRoman)[usize::from(code('č'))], 444.0, "Times-Roman ccaron");
        let cmap = e.to_unicode();
        assert!(cmap.contains(&format!("<{:02X}> <010D>", code('č'))), "{cmap}");
        assert!(cmap.contains(&format!("<{:02X}> <0110>", code('Đ'))));
        assert!(cmap.contains("<61> <0061>") && cmap.contains("<80> <20AC>"), "WinAnsi a and the euro");
        // No block over 100 mappings (ISO 32000-2 9.10.3).
        for block in cmap.split("beginbfchar").skip(1) {
            assert!(block.lines().filter(|l| l.starts_with('<')).count() <= 100);
        }
    }

    #[test]
    fn croatian_letters_get_codes_of_their_own() {
        let e = LatinEncoding::for_text("čćžšđ Đakovo").expect("an encoding");
        // ž and š are WinAnsi; č ć đ Đ are not.
        assert_eq!(e.differences(), vec![(0x7F, "ccaron"), (0x81, "cacute"), (0x8D, "dcroat"), (0x8F, "Dcroat")]);
        assert_eq!(e.encode("čćžšđ Đ"), vec![0x7F, 0x81, 0x9E, 0x9A, 0x8D, b' ', 0x8F]);
        assert_eq!(LatinEncoding::for_text("čć"), LatinEncoding::for_text("ččć"), "the same letters, the same encoding");
        assert_ne!(e.tag(), LatinEncoding::for_text("čć").expect("an encoding").tag());
        // The same codes for other letters: another tag, so the two never share a font entry.
        let tag = |t: &str| LatinEncoding::for_text(t).expect("an encoding").tag();
        assert_ne!(tag("čć"), tag("łő"));
    }

    #[test]
    fn plain_and_undrawable_text_get_no_encoding() {
        assert_eq!(LatinEncoding::for_text("Zagreb šuma"), None, "all WinAnsi");
        assert_eq!(LatinEncoding::for_text("č ș"), None, "ș has a comma accent, not one of the extra letters yet");
        assert_eq!(first_non_latin("čćđ ș"), Some('ș'));
        assert_eq!(first_non_latin("Łódź, Kraków, Győr, Brașov"), Some('ș'));
        assert_eq!(first_non_latin("Łódź, Kraków, Győr, Plzeň, Ďáblice"), None);
    }

    /// More extra letters than free codes: the rest take the codes of WinAnsi characters the
    /// text doesn't use, never one it does.
    #[test]
    fn past_the_free_codes_unused_winansi_codes_are_taken() {
        let letters: String = EXTRA_LETTERS.iter().map(|(c, ..)| *c).collect();
        // € has the first code above the free ones (0x80): it stays €'s.
        let text = format!("{letters} € é");
        let e = LatinEncoding::for_text(&text).expect("an encoding");
        let codes: Vec<u8> = e.differences().iter().map(|(c, _)| *c).collect();
        assert_eq!(codes.len(), EXTRA_LETTERS.len());
        assert!(!codes.contains(&0x80) && !codes.contains(&0xE9), "€ and é are used, so their codes aren't taken");
        assert!(!codes.contains(&b' ') && !codes.iter().any(|c| (0x20..0x7F).contains(c)), "ASCII stays as it is");
        let unique: std::collections::HashSet<u8> = codes.iter().copied().collect();
        assert_eq!(unique.len(), codes.len());
        let encoded = e.encode(&text);
        assert_eq!(encoded.get(encoded.len().saturating_sub(3)..), Some(&[0x80, b' ', 0xE9][..]));
    }

    /// Every extra letter, with all of WinAnsi above 0x80 used too: the codes run out, there's
    /// no encoding, and the letter named is the first left without a code. Every letter before it
    /// has one; with it, none can be made.
    #[test]
    fn a_text_that_uses_up_the_codes_names_the_letter_left_without_one() {
        let letters: Vec<char> = EXTRA_LETTERS.iter().map(|(c, ..)| *c).collect();
        let high: String = ('\u{A1}'..='\u{FF}').chain("€‚ƒ„…†‡ˆ‰Š‹ŒŽ‘’“”•–—˜™š›œžŸ".chars()).collect();
        let text = |n: usize| format!("{}{high}", letters.iter().take(n).collect::<String>());
        let all = text(letters.len());
        assert_eq!(LatinEncoding::for_text(&all), None);
        let c = LatinEncoding::first_without_code(&all).expect("a letter left without a code");
        let at = letters.iter().position(|l| *l == c).unwrap();
        assert!(LatinEncoding::for_text(&text(at)).is_some(), "the letters before {c} all have codes");
        assert_eq!(LatinEncoding::first_without_code(&text(at)), None);
        assert_eq!(LatinEncoding::for_text(&text(at + 1)), None);
        assert_eq!(LatinEncoding::first_without_code(&text(at + 1)), Some(c));
        // Not drawable at all, or plain WinAnsi: nothing to name.
        assert_eq!(LatinEncoding::first_without_code("č ș"), None);
        assert_eq!(LatinEncoding::first_without_code("Zagreb"), None);
    }

    /// Widths from the published Core14 metrics, typed in here by hand.
    #[test]
    fn extra_letters_have_the_standard_widths() {
        assert_eq!(Std14::Helvetica.latin_text_width("č", 1000.0), 500.0);
        assert_eq!(Std14::Helvetica.latin_text_width("ď", 1000.0), 643.0);
        assert_eq!(Std14::TimesRoman.latin_text_width("Ł", 1000.0), 611.0);
        assert_eq!(Std14::HelveticaBold.latin_text_width("ő", 1000.0), 611.0);
        assert_eq!(Std14::Courier.latin_text_width("đĐ", 10.0), 12.0);
        // WinAnsi text measures as before.
        assert_eq!(Std14::Helvetica.latin_text_width("Aš", 12.0), Std14::Helvetica.text_width("Aš", 12.0));
    }
}
