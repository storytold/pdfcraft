//! Structured OCR annotations: the text hierarchy glyph work is built on (ClearScan stage 1).
//!
//! The recognition engine reads a page into lines of characters, each with its own box; the
//! wrapper usually folds that into words, but ClearScan needs the finer grain:
//!
//! - [`CharBox`]es for slicing fused blobs (ink bleed joins "fi" into one component; the
//!   character boxes say where to cut),
//! - word boxes for matching components to letters,
//! - and line **metrics** — baseline, x-height, cap height, descent — for anchoring vector
//!   glyphs to the baseline when the synthesized font is placed.
//!
//! Boxes are in the pixel space of the image that was read, x right, y down, `[left, top,
//! right, bottom]`. Reading a [`crate::scan_prep::PreprocessedPage`] puts them in the
//! deskewed frame; `to_original` carries them back to the scan.
//!
//! The metrics are estimated from the geometry of the recognized words: the bottom of a word
//! without descenders sits on the baseline, the top of a lowercase-only word without
//! ascenders marks the x-height, capitals and digits mark the cap height, and descender words
//! measure the descent below the baseline. Medians make the estimates robust against a
//! misread word; each has a fallback for lines that offer no such word.

use ocrs::{TextChar, TextItem};

/// One recognized character and its box, in image pixels `[left, top, right, bottom]`.
#[derive(Clone, Debug, PartialEq)]
pub struct CharBox {
    pub char: char,
    pub rect: [f32; 4],
}

/// One recognized word: its UTF-8 text, its box (the union of its characters' boxes), and the
/// characters themselves in reading order.
#[derive(Clone, Debug, PartialEq)]
pub struct AnnotatedWord {
    pub text: String,
    pub rect: [f32; 4],
    pub chars: Vec<CharBox>,
}

/// A recognized line of words, in reading order, with its measured metrics. All metrics are
/// pixel offsets in the same frame as the boxes; `baseline` is a y coordinate, the rest are
/// heights above it (and `descent` below it).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AnnotatedLine {
    pub words: Vec<AnnotatedWord>,
    pub baseline: f32,
    pub x_height: f32,
    pub cap_height: f32,
    pub descent: f32,
}

impl AnnotatedLine {
    /// The line's text, words joined by single spaces.
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }

    /// The line's box: the union of its words' boxes, or an empty box for an empty line.
    pub fn rect(&self) -> [f32; 4] {
        self.words.iter().map(|w| w.rect).reduce(union).unwrap_or([0.0; 4])
    }
}

/// The union of two boxes.
fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

pub(crate) fn is_descender_char(c: char) -> bool {
    "gjpqyQ".contains(c)
}

/// A word's letters that reach below the baseline.
fn has_descender(word: &AnnotatedWord) -> bool {
    word.text.chars().any(is_descender_char)
}

/// A word's tall letters: capitals, digits and ascending lowercase, which reach about the
/// cap height (ascenders sit a little above it, so the median leans slightly high). Words
/// with any of them mark the cap-height level.
fn has_cap(word: &AnnotatedWord) -> bool {
    word.text.chars().any(|c| c.is_uppercase() || c.is_ascii_digit() || "bdfhkilt".contains(c) || (c.is_alphabetic() && !c.is_ascii()))
}

/// A word that is only x-height letters sits between baseline and x-height.
fn is_x_height_only(word: &AnnotatedWord) -> bool {
    let letters: Vec<char> = word.text.chars().filter(|c| c.is_alphabetic()).collect();
    !letters.is_empty() && letters.iter().all(|c| "acemnorsuvwxz".contains(*c))
}

/// Helvetica's x-height over cap height: the prior for a line that shows no small letters.
const X_OVER_CAP: f32 = 0.523 / 0.718;

/// The median of the finite values, or `None` when there are none. An even count averages
/// its two middle values.
fn median(mut values: Vec<f32>) -> Option<f32> {
    values.retain(|v| v.is_finite());
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    let mid = values.len() / 2;
    Some(if values.len() % 2 == 1 { values[mid] } else { (values[mid - 1] + values[mid]) / 2.0 })
}

impl AnnotatedLine {
    /// Measure a line's metrics from its words' geometry. Words without descenders sit on the
    /// baseline; x-height-only words and tall words mark the heights above it; descender
    /// words mark the depth below it.
    pub fn measure(words: Vec<AnnotatedWord>) -> AnnotatedLine {
        let bottoms = |filter: fn(&AnnotatedWord) -> bool| words.iter().filter(|w| filter(w)).map(|w| w.rect[3]).collect::<Vec<_>>();
        let tops = |filter: fn(&AnnotatedWord) -> bool| words.iter().filter(|w| filter(w)).map(|w| w.rect[1]).collect::<Vec<_>>();
        let char_bottoms = || {
            words
                .iter()
                .flat_map(|w| &w.chars)
                .filter(|c| c.char.is_alphanumeric() && !is_descender_char(c.char))
                .map(|c| c.rect[3])
                .collect::<Vec<_>>()
        };
        // The baseline: where the words (or, when every word has a descender, the non-descender
        // characters) that sit on it end.
        let baseline = median(bottoms(|w| !has_descender(w))).or_else(|| median(char_bottoms())).or_else(|| median(bottoms(|_| true))).unwrap_or(0.0);
        // Heights above it, from the tops of the words that reach each height.
        let cap_height = median(tops(has_cap)).map_or(0.0, |top| (baseline - top).max(0.0));
        let x_height = median(tops(is_x_height_only)).map_or(0.0, |top| (baseline - top).max(0.0));
        // The descent: how far the deepest descender goes.
        let descent = words.iter().filter(|w| has_descender(w)).map(|w| (w.rect[3] - baseline).max(0.0)).fold(0.0_f32, f32::max);
        // Complete whatever the line did not show, through the Helvetica ratio.
        let (x_height, cap_height) = if x_height <= 0.0 && cap_height <= 0.0 {
            let height = median(words.iter().map(|w| w.rect[3] - w.rect[1]).collect::<Vec<_>>()).unwrap_or(0.0).max(0.0);
            (height * X_OVER_CAP, height)
        } else if x_height <= 0.0 {
            (cap_height * X_OVER_CAP, cap_height)
        } else if cap_height <= 0.0 {
            (x_height, x_height / X_OVER_CAP)
        } else {
            (x_height, cap_height)
        };
        AnnotatedLine { words, baseline, x_height, cap_height, descent }
    }
}

/// The words of one recognized ocrs line: characters grouped between spaces, each word's box
/// the union of its characters' boxes. Space characters are separators, not items.
#[cfg(test)]
fn words_from_chars(chars: &[TextChar]) -> Vec<AnnotatedWord> {
    words_from_chars_clipped(chars, None)
}

fn words_from_chars_clipped(chars: &[TextChar], before: Option<rten_imageproc::RectF>) -> Vec<AnnotatedWord> {
    chars
        .split(|c| c.char == ' ')
        .filter(|group| !group.is_empty())
        .map(|group| {
            let chars = group
                .iter()
                .map(|c| CharBox {
                    char: c.char,
                    rect: crate::ocrs::clip([c.rect.left() as f32, c.rect.top() as f32, c.rect.right() as f32, c.rect.bottom() as f32], before),
                })
                .collect::<Vec<_>>();
            let rect = chars.iter().map(|c| c.rect).reduce(union).unwrap_or([0.0; 4]);
            let text = chars.iter().map(|c| c.char).collect();
            AnnotatedWord { text, rect, chars }
        })
        .filter(|w| !w.text.trim().is_empty())
        .collect()
}

pub(crate) fn from_text_lines(lines: Vec<(ocrs::TextLine, Option<rten_imageproc::RectF>)>) -> Vec<AnnotatedLine> {
    lines
        .into_iter()
        .map(|(line, before)| AnnotatedLine::measure(words_from_chars_clipped(line.chars(), before)))
        .filter(|l| !l.words.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ocrs::TextLine;
    use rten_imageproc::Rect;

    /// A line of "text" where each character is a `width`-wide box; tops and bottoms come
    /// from per-character overrides so tests can shape words like real letters.
    fn char_rect(index: usize, char_width: i32, top: i32, bottom: i32) -> Rect {
        Rect::from_tlhw(top, (index as i32) * char_width, bottom - top, char_width)
    }

    fn text_line(chars: &[(char, i32, i32)]) -> TextLine {
        let items = chars.iter().enumerate().map(|(i, &(c, top, bottom))| TextChar { char: c, rect: char_rect(i, 10, top, bottom) }).collect();
        TextLine::new(items)
    }

    #[test]
    fn characters_group_into_words_at_spaces() {
        // "hi yo": h and i ascend, y descends, o is x-height.
        let line = text_line(&[('h', 0, 20), ('i', 0, 20), (' ', 0, 20), ('y', 6, 22), ('o', 6, 20)]);
        let words = words_from_chars(line.chars());
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].text, "hi");
        assert_eq!(words[0].chars.len(), 2);
        assert_eq!(words[0].chars[0].char, 'h');
        // The word's box is the union of its characters'.
        assert_eq!(words[0].rect, [0.0, 0.0, 20.0, 20.0]);
        assert_eq!(words[1].text, "yo");
        assert_eq!(words[1].rect, [30.0, 6.0, 50.0, 22.0]);
        // Leading and trailing spaces change nothing.
        let padded = text_line(&[(' ', 0, 20), ('h', 0, 20), (' ', 0, 20), (' ', 0, 20)]);
        assert_eq!(words_from_chars(padded.chars()).len(), 1);
    }

    #[test]
    fn a_line_of_only_spaces_has_no_words() {
        let line = text_line(&[(' ', 0, 20), (' ', 0, 20)]);
        assert!(words_from_chars(line.chars()).is_empty());
    }

    /// "DT sun gap": the baseline is the bottom of DT and sun (no descenders), not of gap;
    /// the x-height is sun's top, the cap height DT's, the descent gap's tail.
    #[test]
    fn metrics_follow_the_letters() {
        // Baseline at 100: DT spans 30–100, sun 55–100, gap 55–125.
        let words = vec![word("DT", 0.0, 30.0, 100.0), word("sun", 40.0, 55.0, 100.0), word("gap", 80.0, 55.0, 125.0)];
        let line = AnnotatedLine::measure(words);
        assert_eq!(line.baseline, 100.0);
        assert_eq!(line.x_height, 45.0);
        assert_eq!(line.cap_height, 70.0);
        assert_eq!(line.descent, 25.0);
        assert_eq!(line.text(), "DT sun gap");
        assert_eq!(line.rect(), [0.0, 30.0, 110.0, 125.0]);
    }

    /// Every word descends: the baseline falls back to the median of all bottoms.
    #[test]
    fn a_line_of_only_descenders_falls_back_to_its_bottoms() {
        let words = vec![word("gyp", 0.0, 60.0, 120.0), word("q", 50.0, 60.0, 130.0)];
        let line = AnnotatedLine::measure(words);
        assert_eq!(line.baseline, 125.0, "the median of the bottoms");
        assert_eq!(line.descent, 5.0);
    }

    #[test]
    fn a_line_where_every_word_has_a_descender_uses_non_descender_char_bottoms() {
        let words = vec![
            AnnotatedWord {
                text: "gap".into(),
                rect: [0.0, 55.0, 30.0, 125.0],
                chars: vec![
                    CharBox { char: 'g', rect: [0.0, 55.0, 10.0, 125.0] },
                    CharBox { char: 'a', rect: [10.0, 55.0, 20.0, 100.0] },
                    CharBox { char: 'p', rect: [20.0, 55.0, 30.0, 125.0] },
                ],
            },
            AnnotatedWord {
                text: "dog".into(),
                rect: [40.0, 30.0, 70.0, 125.0],
                chars: vec![
                    CharBox { char: 'd', rect: [40.0, 30.0, 50.0, 100.0] },
                    CharBox { char: 'o', rect: [50.0, 55.0, 60.0, 100.0] },
                    CharBox { char: 'g', rect: [60.0, 55.0, 70.0, 125.0] },
                ],
            },
        ];
        let line = AnnotatedLine::measure(words);
        assert_eq!(line.baseline, 100.0, "non-descender chars anchor the baseline");
        assert_eq!(line.descent, 25.0);
    }

    /// No lowercase: the x-height is inferred from the cap height through the ratio.
    #[test]
    fn a_line_without_small_letters_infers_the_x_height() {
        let line = AnnotatedLine::measure(vec![word("DT4", 0.0, 28.2, 100.0)]);
        assert_eq!(line.baseline, 100.0);
        assert!((line.cap_height - 71.8).abs() < 0.01, "{}", line.cap_height);
        assert!((line.x_height - 71.8 * X_OVER_CAP).abs() < 0.01, "{}", line.x_height);
        assert_eq!(line.descent, 0.0);
    }

    /// No capitals: the cap height is inferred from the x-height.
    #[test]
    fn a_line_without_tall_letters_infers_the_cap_height() {
        let line = AnnotatedLine::measure(vec![word("sun", 0.0, 40.0, 100.0)]);
        assert_eq!(line.x_height, 60.0);
        assert!((line.cap_height - 60.0 / X_OVER_CAP).abs() < 0.01, "{}", line.cap_height);
    }

    /// One misplaced word must not move the median baseline.
    #[test]
    fn a_misread_word_does_not_move_the_median_baseline() {
        let words = vec![
            word("one", 0.0, 50.0, 100.0),
            word("two", 30.0, 50.0, 100.0),
            word("iii", 60.0, 50.0, 100.0),
            word("ivv", 90.0, 50.0, 130.0), // a misread descender where none belongs
        ];
        assert_eq!(AnnotatedLine::measure(words).baseline, 100.0);
    }

    #[test]
    fn an_empty_line_measures_to_zero() {
        let line = AnnotatedLine::measure(Vec::new());
        assert_eq!((line.baseline, line.x_height, line.cap_height, line.descent), (0.0, 0.0, 0.0, 0.0));
        assert_eq!(line.rect(), [0.0; 4]);
        assert_eq!(line.text(), "");
    }

    /// A word with a per-character box for each character, spanning `(left, top)` to
    /// `(left + n·10, bottom)`.
    fn word(text: &str, left: f32, top: f32, bottom: f32) -> AnnotatedWord {
        let chars = text
            .chars()
            .enumerate()
            .map(|(i, c)| CharBox { char: c, rect: [left + i as f32 * 10.0, top, left + (i as f32 + 1.0) * 10.0, bottom] })
            .collect::<Vec<_>>();
        let n = chars.len() as f32;
        AnnotatedWord { text: text.into(), rect: [left, top, left + n * 10.0, bottom], chars }
    }

    #[test]
    fn the_median_averages_an_even_count_and_skips_non_finite() {
        assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(vec![4.0, 1.0, 3.0, 2.0]), Some(2.5));
        assert_eq!(median(vec![f32::NAN, 1.0]), Some(1.0));
        assert_eq!(median(vec![f32::NAN]), None);
        assert_eq!(median(Vec::new()), None);
    }
}
