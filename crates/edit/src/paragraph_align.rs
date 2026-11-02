//! Alignment paragraph by paragraph. A paragraph is the text between two hard line breaks
//! (`\n`): a line typed with Enter in added text, or a source row of an edited paragraph. Each can
//! have an alignment of its own; `None` (or no entry) follows the box's or paragraph's alignment.

use std::ops::RangeInclusive;

use crate::added::Align;

/// How many paragraphs `text` has (an empty text has one).
pub fn count(text: &str) -> usize {
    text.bytes().filter(|b| *b == b'\n').count().saturating_add(1)
}

/// The paragraph the byte position `at` of `text` is in.
fn paragraph_at(text: &str, at: usize) -> usize {
    let bytes = text.as_bytes();
    bytes.get(..at.min(bytes.len())).unwrap_or_default().iter().filter(|b| **b == b'\n').count()
}

/// The paragraphs a selection (byte range) of `text` touches. An empty selection is the caret's
/// paragraph; a selection ending just after a line break doesn't reach the next paragraph.
pub fn selected(text: &str, range: std::ops::Range<usize>) -> RangeInclusive<usize> {
    let first = paragraph_at(text, range.start);
    let last = if range.end > range.start { paragraph_at(text, range.end - 1) } else { first };
    first..=last.max(first)
}

/// Paragraph `i`'s alignment: its own, or `base`.
pub fn align_of(aligns: &[Option<Align>], i: usize, base: Align) -> Align {
    aligns.get(i).copied().flatten().unwrap_or(base)
}

/// Give paragraphs `which` of a text with `count` paragraphs the alignment `align`; the others
/// keep theirs. With a `base`, entries equal to it become `None` ([`tidy`]), so equal alignments
/// compare equal however they were set; without one (the base is only guessed, as for an edited
/// paragraph's own rows) every entry set stays. Trailing `None`s are dropped either way.
pub fn set(aligns: &[Option<Align>], count: usize, which: RangeInclusive<usize>, align: Align, base: Option<Align>) -> Vec<Option<Align>> {
    let mut out: Vec<Option<Align>> = (0..count).map(|i| aligns.get(i).copied().flatten()).collect();
    for i in which {
        if let Some(a) = out.get_mut(i) {
            *a = Some(align);
        }
    }
    match base {
        Some(base) => tidy(out, base),
        None => trimmed(out),
    }
}

/// `aligns` without entries that only repeat `base`, and without trailing `None`s.
pub fn tidy(mut aligns: Vec<Option<Align>>, base: Align) -> Vec<Option<Align>> {
    for a in &mut aligns {
        if *a == Some(base) {
            *a = None;
        }
    }
    trimmed(aligns)
}

fn trimmed(mut aligns: Vec<Option<Align>>) -> Vec<Option<Align>> {
    while aligns.last() == Some(&None) {
        aligns.pop();
    }
    aligns
}

/// Carry the alignments of `old`'s paragraphs over to `new`: the paragraphs before and after
/// the change keep theirs, and the paragraphs the change made take the alignment of the first
/// paragraph it changed, or for lines only inserted, of the paragraph before them (so Enter in or
/// after a centred paragraph makes centred ones).
pub fn remap(old: &str, new: &str, aligns: &[Option<Align>]) -> Vec<Option<Align>> {
    remap_at(old, new, aligns, None)
}

/// [`remap`] for an edit that left the caret at byte `caret` of `new`. Repeated paragraphs can
/// make where the edit was ambiguous ("A\nA" to "A" deletes either one); the caret, at the edit's
/// end, says which. It only chooses between readings that fit the texts, so a caret away from the
/// edit never makes it an edit of other paragraphs.
pub fn remap_at(old: &str, new: &str, aligns: &[Option<Align>], caret: Option<usize>) -> Vec<Option<Align>> {
    if aligns.is_empty() || old == new {
        return aligns.to_vec();
    }
    let (o, n): (Vec<&str>, Vec<&str>) = (old.split('\n').collect(), new.split('\n').collect());
    let shortest = o.len().min(n.len());
    let same_start = o.iter().zip(&n).take_while(|(a, b)| a == b).count().min(shortest);
    let same_end = o.iter().rev().zip(n.iter().rev()).take_while(|(a, b)| a == b).count().min(shortest);
    let fewest_kept = shortest - same_start;
    let (prefix, suffix) = match caret {
        Some(at) if fewest_kept <= same_end => {
            // The edit's paragraphs of `new` end before the caret's paragraph when the caret starts
            // one, and with it otherwise.
            let starts = at == 0 || at.checked_sub(1).and_then(|i| new.as_bytes().get(i)) == Some(&b'\n');
            let end = paragraph_at(new, at).saturating_add(usize::from(!starts));
            let suffix = n.len().saturating_sub(end).clamp(fewest_kept, same_end);
            (shortest - suffix, suffix)
        }
        _ => (same_start, same_end.min(shortest - same_start)),
    };
    let at = |i: usize| aligns.get(i).copied().flatten();
    let inserted_only = o.len() - prefix - suffix == 0;
    let changed = if inserted_only && prefix > 0 { at(prefix - 1) } else { at(prefix) };
    let mut out: Vec<Option<Align>> = (0..prefix).map(at).collect();
    out.extend(std::iter::repeat_n(changed, n.len() - prefix - suffix));
    out.extend((o.len() - suffix..o.len()).map(at));
    trimmed(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Align::*;

    #[test]
    fn a_selection_names_the_paragraphs_it_touches() {
        let text = "One\nTwo\nThree";
        assert_eq!(count(text), 3);
        assert_eq!(count(""), 1);
        assert_eq!(selected(text, 0..0), 0..=0, "the caret's paragraph");
        assert_eq!(selected(text, 5..6), 1..=1);
        assert_eq!(selected(text, 2..9), 0..=2);
        assert_eq!(selected(text, 4..8), 1..=1, "a whole line with its break stays on that line");
        assert_eq!(selected(text, 0..999), 0..=2, "past the end is clamped");
        // Not a character boundary: still counted by bytes, never a panic.
        assert_eq!(selected("é\nb", 1..1), 0..=0);
    }

    #[test]
    fn setting_one_paragraph_keeps_the_others_and_tidies() {
        let one = set(&[], 3, 1..=1, Center, Some(Left));
        assert_eq!(one, [None, Some(Center)]);
        assert_eq!(align_of(&one, 0, Left), Left);
        assert_eq!(align_of(&one, 1, Left), Center);
        assert_eq!(align_of(&one, 9, Right), Right);
        assert_eq!(set(&one, 3, 1..=1, Left, Some(Left)), [], "back to the box's alignment is no entry at all");
        assert_eq!(set(&one, 3, 2..=7, Right, Some(Left)), [None, Some(Center), Some(Right)], "paragraphs past the end are ignored");
        assert_eq!(set(&[Some(Right); 9], 2, 0..=0, Center, Some(Left)), [Some(Center), Some(Right)], "entries past the text are dropped");
        assert_eq!(set(&[], 3, 0..=0, Left, None), [Some(Left)], "without a base, an explicit entry stays");
    }

    #[test]
    fn alignments_follow_their_paragraphs_through_an_edit() {
        let aligns = [Some(Center), None, Some(Right)];
        assert_eq!(remap("A\nB\nC", "A\nB\nC", &aligns), aligns);
        // A line inserted before the last keeps the last right-aligned.
        assert_eq!(remap("A\nB\nC", "A\nB\nnew\nC", &aligns), [Some(Center), None, None, Some(Right)]);
        // Enter inside the centred first paragraph: both halves are centred.
        assert_eq!(remap("Ab\nB\nC", "A\nb\nB\nC", &aligns), [Some(Center), Some(Center), None, Some(Right)]);
        // The middle paragraph deleted.
        assert_eq!(remap("A\nB\nC", "A\nC", &aligns), [Some(Center), Some(Right)]);
        // Everything replaced takes the first changed paragraph's alignment.
        assert_eq!(remap("A\nB", "x", &[None, Some(Right)]), []);
        assert_eq!(remap("A\nA", "A", &[Some(Center), Some(Right)]), [Some(Center)]);
        assert_eq!(remap("A", "A\nA", &[Some(Center)]), [Some(Center), Some(Center)]);
    }

    /// Repeated rows make the edit's place ambiguous: "A\nA" to "A" deletes either row. The caret,
    /// where the edit ended, settles which (CodeRabbit on 648dd428).
    #[test]
    fn the_caret_says_which_of_two_same_rows_an_edit_took() {
        let aligns = [Some(Center), Some(Right)];
        assert_eq!(remap_at("A\nA", "A", &aligns, Some(0)), [Some(Right)], "the first row and its break deleted");
        assert_eq!(remap_at("A\nA", "A", &aligns, Some(1)), [Some(Center)], "the break and the second row deleted");
        assert_eq!(remap_at("A\nA", "A", &aligns, None), [Some(Center)], "no caret: as before");
        // Enter at the start of the first of two same rows: the new row is before it.
        assert_eq!(remap_at("A\nA", "\nA\nA", &aligns, Some(1)), [Some(Center), Some(Center), Some(Right)]);
        // Enter at the end of the second: the new row follows it.
        assert_eq!(remap_at("A\nA", "A\nA\n", &aligns, Some(4)), [Some(Center), Some(Right), Some(Right)]);
        // A caret away from the edit never makes it an edit of other rows.
        assert_eq!(remap_at("A\nB\nC", "A\nB\nCx", &[Some(Center), Some(Right), None], Some(0)), [Some(Center), Some(Right)]);
        // A caret past the end, or not on a character boundary, is no panic.
        assert_eq!(remap_at("A\nA", "A", &aligns, Some(99)), [Some(Center)]);
        assert_eq!(remap_at("é\né", "é", &aligns, Some(1)), [Some(Center)]);
    }
}
