//! Edit text on a scanned page, word by word: a text edit of an OCR phrase covers and redraws only
//! the words it changes, and the rest stay as the scan shows them (#3313). Each changed stretch is
//! rewritten as a phrase of its own, which is how a phrase beside others in one OCR layer already
//! rewrites. A deleted word leaves its gap: the words after it are pixels, and don't move.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;

use super::grouping::row_rect;
use super::styled::rewrite_styled_block;
use super::{BlockStyle, Document, EditError, LineEdit, TextBlock, TextLine, kept_words, word_lines};

/// How many times a replacement may be refitted to the room it has before the whole phrase is
/// redrawn instead. Each try rewrites a copy of the document.
const MAX_TRIES: usize = 16;

/// The narrowest a replacement is squeezed to stay in its word's place. Most replacements are
/// only a little wider than the word they replace, and taking over the next word pushes that one
/// along too, and so on down the line. Past this it would be hard to read, so it takes the word over.
const MIN_SQUEEZE: f64 = 0.75;

/// A stretch of the phrase's word runs that changes, the text that replaces it (empty when it is
/// deleted), and how much that text is squeezed across to fit.
#[derive(Debug, PartialEq)]
struct Change {
    runs: Range<usize>,
    /// Which of the draft's words replace them.
    words: Range<usize>,
    text: String,
    squeeze: Option<f64>,
}

/// The stretches `new` changes, given how many words each run holds and which word of `new` each
/// old word stays as ([`kept_words`]). A run in `redrawn` counts as changed, and new words with no
/// run of their own to replace add the run after them to it (or the last one, at the end).
fn changes(runs: &[usize], kept_as: &[Option<usize>], new: &[&str], redrawn: &mut BTreeSet<usize>) -> Vec<Change> {
    // Each pass either returns or adds a run to `redrawn`, so this ends.
    'plan: loop {
        let mut out = Vec::new();
        let (mut first_run, mut first_new, mut token) = (0usize, 0usize, 0usize);
        for (run, count) in runs.iter().enumerate() {
            let words = kept_as.get(token..token.saturating_add(*count)).unwrap_or_default();
            token = token.saturating_add(*count);
            let at = words.first().copied().flatten();
            let kept = !redrawn.contains(&run) && !words.is_empty() && words.iter().enumerate().all(|(i, w)| *w == at.map(|a| a.saturating_add(i)));
            let (true, Some(at)) = (kept, at) else { continue };
            if first_run == run && first_new < at {
                redrawn.insert(run);
                continue 'plan;
            }
            if first_run < run {
                out.push(Change {
                    runs: first_run..run,
                    words: first_new..at,
                    text: new.get(first_new..at).unwrap_or_default().join(" "),
                    squeeze: None,
                });
            }
            first_run = run.saturating_add(1);
            first_new = at.saturating_add(*count);
        }
        if first_run < runs.len() {
            out.push(Change {
                runs: first_run..runs.len(),
                words: first_new..new.len(),
                text: new.get(first_new..).unwrap_or_default().join(" "),
                squeeze: None,
            });
        } else if first_new < new.len()
            && let Some(last) = runs.len().checked_sub(1)
        {
            redrawn.insert(last);
            continue 'plan;
        }
        return out;
    }
}

/// Each word of `text` as a byte range.
fn word_ranges(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = None;
    for (at, c) in text.char_indices() {
        match (c.is_whitespace(), start) {
            (true, Some(from)) => {
                out.push(from..at);
                start = None;
            }
            (false, None) => start = Some(at),
            _ => {}
        }
    }
    if let Some(from) = start {
        out.push(from..text.len());
    }
    out
}

/// Whether `plan` can stand for the spacing in `draft`, whose words are `words`. Spaces beside a
/// change are nothing to draw: Backspacing over a word leaves one either side of where it was, and
/// the words around it stay where the scan has them. Anything else that isn't one space has to be
/// drawn, inside the new words or between two words nothing else changed, which only the whole
/// rewrite can do.
fn spacing_fits(draft: &str, words: &[Range<usize>], plan: &[Change]) -> bool {
    words.windows(2).enumerate().all(|(index, pair)| {
        let [before, after] = pair else { return true };
        let gap = draft.get(before.end..after.start).unwrap_or(" ");
        let boundary = index.saturating_add(1);
        gap == " " || (gap.chars().all(|c| c == ' ') && plan.iter().any(|change| change.words.start == boundary || change.words.end == boundary))
    })
}

/// The phrase's stretch `indices` of `lines`, as a paragraph of its own. It keeps the whole
/// phrase's size, so what is redrawn is set alike wherever in the phrase it is.
fn stretch(phrase: &TextBlock, lines: &[TextLine], indices: &[usize]) -> Result<TextBlock, EditError> {
    let first = indices.first().and_then(|i| lines.get(*i)).ok_or_else(|| EditError::Invalid("the phrase has no such words".into()))?;
    let rect = row_rect(indices, lines).ok_or_else(|| EditError::Invalid("the phrase has no such words".into()))?;
    let mut text = String::new();
    let mut spans = Vec::with_capacity(indices.len());
    for index in indices {
        let Some(line) = lines.get(*index) else { continue };
        if !text.is_empty() && !text.ends_with(char::is_whitespace) && !line.text.starts_with(char::is_whitespace) {
            text.push(' ');
        }
        let start = text.len();
        text.push_str(&line.text);
        spans.push((start..text.len(), *index));
    }
    Ok(TextBlock {
        reflow_text: text.clone(),
        editor_text: text.clone(),
        text,
        spans,
        rect,
        base_font: first.base_font.clone(),
        size: phrase.size,
        bold: first.bold,
        italic: first.italic,
        color: first.color,
        rotation: first.rotation(),
        lines: indices.to_vec(),
        rows: vec![indices.to_vec()],
        invisible: true,
    })
}

fn key(line: &TextLine) -> (&str, [i64; 4]) {
    // Saturating casts: a non-finite edge only makes the comparison below fail, safely.
    (line.text.as_str(), line.rect.map(|v| (v * 100.0).round() as i64))
}

/// How far right a rewrite of the runs `indices` drew (`None` when it drew nothing), or an error
/// when it moved or lost any other text on the page: text positioned after the end of the text
/// before it, say, depends on the words that went.
fn drawn_right(before: &[TextLine], indices: &[usize], after: &[TextLine]) -> Result<Option<f64>, EditError> {
    let mut unmatched: HashMap<_, usize> = HashMap::new();
    for line in before {
        let count = unmatched.entry(key(line)).or_default();
        *count = count.saturating_add(1);
    }
    let mut drawn = Vec::new();
    for line in after {
        match unmatched.get_mut(&key(line)).filter(|count| **count > 0) {
            Some(count) => *count -= 1,
            None => drawn.push(line),
        }
    }
    // Visible text beside what was drawn reads as one line with it. It is still where it was.
    let joined = |line: &TextLine| {
        let [x0, y0, x1, y1] = line.rect;
        drawn.iter().any(|d| {
            d.text.contains(line.text.trim()) && d.rect[0] <= x0 + 0.01 && d.rect[1] <= y0 + 0.01 && d.rect[2] >= x1 - 0.01 && d.rect[3] >= y1 - 0.01
        })
    };
    let moved = before
        .iter()
        .enumerate()
        .any(|(index, line)| !indices.contains(&index) && unmatched.get(&key(line)).is_some_and(|c| *c > 0) && !joined(line));
    if moved {
        return Err(EditError::Invalid("rewriting these words alone would move the text around them".into()));
    }
    Ok(drawn.iter().map(|line| line.rect[2]).reduce(f64::max))
}

/// Rewrite each change of `plan` in `doc`, last first: a rewrite goes where its words were or at
/// the end of the page, so the words before it keep their indexes (an error if they don't). Gives
/// how far right each drew.
fn apply(
    doc: &mut Document,
    page: usize,
    phrase: &TextBlock,
    members: &[usize],
    plan: &[Change],
    style: &BlockStyle,
    max_glyphs: usize,
) -> Result<(Vec<Option<f64>>, Option<String>), EditError> {
    // The stretch starts where its words did. Its own box would centre a word that happens to
    // sit in the middle of the page.
    let mut style = BlockStyle { align: Some(crate::added::Align::Left), changed_words_only: false, ..style.clone() };
    let mut rights = vec![None; plan.len()];
    let mut substituted = None;
    let original = word_lines(doc, page)?;
    for (at, change) in plan.iter().enumerate().rev() {
        let indices = members.get(change.runs.clone()).ok_or_else(|| EditError::Invalid("the phrase has no such words".into()))?;
        let before = word_lines(doc, page)?;
        // The words up to the next that stays must still be where they were. A rewrite also takes
        // away copies of its words, and a copy earlier on the page moves these up the list.
        let up_to = change.runs.end.saturating_add(1).min(members.len());
        let same = |index: &usize| before.get(*index).map(key) == original.get(*index).map(key);
        if !members.get(..up_to).unwrap_or_default().iter().all(same) {
            return Err(EditError::Invalid("rewriting these words alone would lose track of the words before them".into()));
        }
        let part = stretch(phrase, &before, indices)?;
        // Relative to the scale the words have, which a layer may have narrowed already.
        let own_scale = indices.first().and_then(|i| before.get(*i)).map_or(1.0, |line| line.origin.state.scale);
        style.scale = change.squeeze.map(|squeeze| squeeze * own_scale * 100.0);
        // The words on either side stay, with all their ink.
        let edge = |run: Option<usize>| run.and_then(|run| members.get(run)).and_then(|i| before.get(*i)).map(|line| line.rect);
        style.cover_between = Some([
            edge(change.runs.start.checked_sub(1)).map_or(f64::NEG_INFINITY, |rect| rect[2]),
            edge(Some(change.runs.end)).map_or(f64::INFINITY, |rect| rect[0]),
        ]);
        let edit = rewrite_styled_block(doc, page, &part, &before, Some(&change.text), &style, max_glyphs)?;
        substituted = substituted.or(edit.substituted);
        let right = drawn_right(&before, indices, &word_lines(doc, page)?)?;
        if right.is_none() && !change.text.is_empty() {
            return Err(EditError::Invalid("the new words were not drawn".into()));
        }
        if let Some(slot) = rights.get_mut(at) {
            *slot = right;
        }
    }
    Ok((rights, substituted))
}

/// Whether a change that drew as far as `right` needs another try. It should end by the middle of
/// the gap before the word after it, so the two don't touch: within [`MIN_SQUEEZE`] it is squeezed
/// to, and otherwise it takes that word over.
fn make_room(change: &Change, right: f64, boxes: &[[f64; 4]], squeezes: &mut BTreeMap<usize, f64>, redrawn: &mut BTreeSet<usize>) -> bool {
    let (Some(first), Some(last), Some(next)) =
        (boxes.get(change.runs.start), change.runs.end.checked_sub(1).and_then(|run| boxes.get(run)), boxes.get(change.runs.end))
    else {
        return false;
    };
    let limit = (last[2] + next[0]) / 2.0;
    // Widths scale in proportion, so one squeeze lands within this; the rest is rounding.
    if right <= limit + 0.25 {
        return false;
    }
    let squeeze = change.squeeze.unwrap_or(1.0) * (limit - first[0]) / (right - first[0]);
    if squeeze >= MIN_SQUEEZE {
        squeezes.insert(change.runs.start, squeeze);
    } else {
        squeezes.remove(&change.runs.start);
        redrawn.insert(change.runs.end);
    }
    true
}

/// Rewrite only the words `draft` changes in the invisible phrase `phrase`, or `None` when this
/// edit is not one that can (it changes the formatting or the lines, the phrase is turned or
/// right-to-left, or its words can't be told apart or kept in place): the caller then rewrites the
/// whole phrase. Nothing changes in `doc` unless the whole edit works.
pub(crate) fn rewrite_changed_words(
    doc: &mut Document,
    page: usize,
    phrase: &TextBlock,
    joined: &[TextLine],
    draft: &str,
    style: &BlockStyle,
    max_glyphs: usize,
) -> Option<LineEdit> {
    let right_to_left = |text: &str| text.chars().any(crate::bidi::is_rtl);
    if !style.changed_words_only
        || !phrase.invisible
        || phrase.rows.len() != 1
        || draft.contains(['\n', '\r'])
        || right_to_left(draft)
        || right_to_left(&phrase.text)
    {
        return None;
    }
    let lines = word_lines(doc, page).ok()?;
    let phrase_ops: BTreeSet<usize> = phrase.lines.iter().filter_map(|i| joined.get(*i)).flat_map(|line| line.ops.iter().copied()).collect();
    let members: Vec<usize> =
        lines.iter().enumerate().filter(|(_, line)| line.ops.first().is_some_and(|op| phrase_ops.contains(op))).map(|(index, _)| index).collect();
    if members.iter().filter_map(|i| lines.get(*i)).any(|line| !line.upright() || line.visual_order) {
        return None;
    }
    let runs: Vec<Vec<&str>> = members.iter().filter_map(|i| lines.get(*i)).map(|line| line.text.split_whitespace().collect()).collect();
    let old: Vec<&str> = runs.iter().flatten().copied().collect();
    let new: Vec<&str> = draft.split_whitespace().collect();
    // Words the layer doesn't show apart (a run per letter, say) can't be kept apart.
    if !phrase.editor_text.split_whitespace().eq(old.iter().copied()) {
        return None;
    }
    // A single word that changes has nothing to keep. With the same words, the plan is empty.
    if old.len() < 2 && old != new {
        return None;
    }
    let kept_as = kept_words(&old, &new)?;
    let counts: Vec<usize> = runs.iter().map(Vec::len).collect();
    let boxes: Vec<[f64; 4]> = members.iter().filter_map(|i| lines.get(*i)).map(|line| line.rect).collect();
    let mut redrawn = BTreeSet::new();
    let mut squeezes = BTreeMap::new();
    for _ in 0..MAX_TRIES {
        let mut plan = changes(&counts, &kept_as, &new, &mut redrawn);
        if !spacing_fits(draft, &word_ranges(draft), &plan) {
            return None;
        }
        for change in &mut plan {
            change.squeeze = squeezes.get(&change.runs.start).copied();
        }
        let mut trial = doc.clone();
        let (rights, substituted) = apply(&mut trial, page, phrase, &members, &plan, style, max_glyphs).ok()?;
        let mut again = false;
        for (change, right) in plan.iter().zip(&rights) {
            if let Some(right) = right {
                again |= make_room(change, *right, &boxes, &mut squeezes, &mut redrawn);
            }
        }
        if !again {
            *doc = trial;
            return Some(LineEdit { substituted });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(old: &[&[&str]], new: &str) -> Vec<(Range<usize>, String)> {
        let flat: Vec<&str> = old.iter().flat_map(|run| run.iter().copied()).collect();
        let new: Vec<&str> = new.split_whitespace().collect();
        let counts: Vec<usize> = old.iter().map(|run| run.len()).collect();
        changes(&counts, &kept_words(&flat, &new).unwrap(), &new, &mut BTreeSet::new()).into_iter().map(|c| (c.runs, c.text)).collect()
    }

    #[test]
    fn only_the_runs_a_draft_changes_are_planned() {
        let abc: &[&[&str]] = &[&["a"], &["b"], &["c"]];
        assert_eq!(plan(abc, "a b c"), []);
        assert_eq!(plan(abc, "a c"), [(1..2, String::new())], "a deletion");
        assert_eq!(plan(abc, "a x c"), [(1..2, "x".to_owned())], "a replacement");
        assert_eq!(plan(abc, "x b y"), [(0..1, "x".to_owned()), (2..3, "y".to_owned())], "two apart");
        assert_eq!(plan(abc, "x y c"), [(0..2, "x y".to_owned())], "two together");
        assert_eq!(plan(abc, ""), [(0..3, String::new())], "everything");
    }

    #[test]
    fn new_words_with_no_run_to_replace_take_a_neighbour() {
        let abc: &[&[&str]] = &[&["a"], &["b"], &["c"]];
        assert_eq!(plan(abc, "x a b c"), [(0..1, "x a".to_owned())], "at the start");
        assert_eq!(plan(abc, "a x b c"), [(1..2, "x b".to_owned())], "in the middle");
        assert_eq!(plan(abc, "a b c x"), [(2..3, "c x".to_owned())], "at the end");
    }

    #[test]
    fn a_run_of_several_words_is_kept_only_whole() {
        let runs: &[&[&str]] = &[&["a", "b"], &["c"]];
        assert_eq!(plan(runs, "a b x"), [(1..2, "x".to_owned())]);
        assert_eq!(plan(runs, "a x c"), [(0..1, "a x".to_owned())], "one of its words changed");
        assert_eq!(plan(runs, "a x b c"), [(0..1, "a x b".to_owned())], "a word went in between its words");
    }

    #[test]
    fn a_drafts_words_are_found_by_their_bytes() {
        assert!(word_ranges("").is_empty() && word_ranges("  ").is_empty());
        assert_eq!(word_ranges("ab  é c "), [0..2, 4..6, 7..8]);
    }

    /// The plan for `draft` against the words a, b and c, and whether it stands for its spacing.
    fn fits(draft: &str) -> bool {
        let (old, new): (Vec<&str>, Vec<&str>) = (vec!["a", "b", "c"], draft.split_whitespace().collect());
        let plan = changes(&[1, 1, 1], &kept_words(&old, &new).unwrap(), &new, &mut BTreeSet::new());
        spacing_fits(draft, &word_ranges(draft), &plan)
    }

    #[test]
    fn spaces_beside_a_change_are_nothing_to_draw() {
        assert!(fits("a b c"));
        assert!(fits("a b c "), "after the words");
        assert!(fits("  a b c"), "before them");
        assert!(fits("a  c"), "either side of a deleted word");
        assert!(fits("  b c"), "where a first word went");
        assert!(fits("a   x  c"), "either side of a replacement");
    }

    #[test]
    fn spacing_that_has_to_be_drawn_doesnt_fit() {
        assert!(!fits("a  b c"), "between words nothing else changed");
        assert!(!fits("x  y c"), "inside the new words");
        assert!(!fits("a\u{3000}x c"), "another kind of space, even beside a change");
        assert!(!fits("a\u{a0}x c"));
    }

    /// Word boxes 10 wide, with a gap of 4 between them.
    const BOXES: [[f64; 4]; 3] = [[0.0, 0.0, 10.0, 10.0], [14.0, 0.0, 24.0, 10.0], [28.0, 0.0, 38.0, 10.0]];

    fn room_for(right: f64) -> (bool, BTreeMap<usize, f64>, BTreeSet<usize>) {
        let (mut squeezes, mut redrawn) = (BTreeMap::new(), BTreeSet::new());
        let change = Change { runs: 0..1, words: 0..1, text: "x".to_owned(), squeeze: None };
        (make_room(&change, right, &BOXES, &mut squeezes, &mut redrawn), squeezes, redrawn)
    }

    #[test]
    fn a_word_that_fits_is_left_alone() {
        assert_eq!(room_for(12.0), (false, BTreeMap::new(), BTreeSet::new()), "it ends in the gap's first half");
        let last = Change { runs: 2..3, words: 2..3, text: "x".to_owned(), squeeze: None };
        assert!(!make_room(&last, 900.0, &BOXES, &mut BTreeMap::new(), &mut BTreeSet::new()), "the last word has the page to grow into");
    }

    #[test]
    fn a_word_a_little_too_wide_is_squeezed_to_the_middle_of_the_gap() {
        let (again, squeezes, redrawn) = room_for(15.0);
        assert!(again && redrawn.is_empty());
        assert_eq!(squeezes.get(&0).copied(), Some(12.0 / 15.0));
    }

    #[test]
    fn a_word_far_too_wide_takes_over_the_next_one() {
        let (again, squeezes, redrawn) = room_for(30.0);
        assert!(again && squeezes.is_empty());
        assert_eq!(redrawn, BTreeSet::from([1]));
    }

    #[test]
    fn a_redrawn_run_joins_the_change_beside_it() {
        let (counts, old, new) = ([1, 1, 1], ["a", "b", "c"], ["x", "b", "c"]);
        let mut redrawn = BTreeSet::from([1]);
        let planned = changes(&counts, &kept_words(&old, &new).unwrap(), &new, &mut redrawn);
        assert_eq!(planned, [Change { runs: 0..2, words: 0..2, text: "x b".to_owned(), squeeze: None }]);
    }
}
