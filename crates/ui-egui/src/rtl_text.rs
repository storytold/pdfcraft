//! Typing right-to-left text (Arabic, Hebrew) in a text box.
//!
//! egui shapes each stretch of one script with that script's direction, so the letters of an
//! Arabic word join and run right to left, but it lays the stretches out left to right in typed
//! order and keeps the caret in typed order too. [`layout`] lays each line out in display order
//! (the Unicode bidirectional algorithm, as [`crate::bidi::visual`] does for one-line labels) and
//! gives the caret its place for every character; [`pointer`] puts the caret where a click or drag
//! lands. Text without right-to-left characters is laid out exactly as egui's own text box does.

use std::sync::Arc;

use egui::text::{CCursor, CCursorRange, LayoutJob};
use egui::{Color32, FontId, Galley, Id, Ui};
use unicode_bidi::{BidiInfo, Level};

use crate::bidi::{is_rtl_script, mirrored};

/// The galley for `text` in a multiline text box, laid out like egui's default with `font` and
/// `color`, with right-to-left lines in display order. `align` places lines that read right to
/// left (left, centre, right); `None` sets them flush right, as right-to-left paragraphs are.
pub fn layout(ui: &Ui, text: &str, font: &FontId, color: Color32, wrap_width: f32, align: Option<egui::Align>) -> Arc<Galley> {
    let line_height = ui.fonts_mut(|f| f.row_height(font)) + ui.spacing().extra_text_line_spacing;
    if !text.chars().any(is_rtl_script) {
        let mut job = LayoutJob::simple(text.to_owned(), font.clone(), color, wrap_width);
        // As egui's text box: trailing spaces stay while typing.
        job.keep_trailing_whitespace = true;
        for section in &mut job.sections {
            section.format.line_height = Some(line_height);
        }
        return ui.fonts_mut(|f| f.layout_job(job));
    }
    let format = egui::TextFormat { line_height: Some(line_height), ..egui::TextFormat::simple(font.clone(), color) };
    let chars: Vec<char> = text.chars().collect();
    let lines = ui.fonts_mut(|f| break_lines(&chars, wrap_width, |s| f.layout_no_wrap(s.to_owned(), font.clone(), color).size().x));
    // Each line in the order it is drawn, every character its own section: egui then shapes no
    // right-to-left run itself (it would add a glyph per letter, and the caret would drift) and
    // draws one glyph per character. A space where a line wraps becomes a line break, so the text
    // keeps its length and every character its index.
    let mut job = LayoutJob { keep_trailing_whitespace: true, ..LayoutJob::default() };
    job.wrap.max_width = f32::INFINITY;
    let mut orders = Vec::with_capacity(lines.len());
    for (k, line) in lines.iter().enumerate() {
        let piece: Vec<char> = chars.get(line.clone()).unwrap_or_default().to_vec();
        let (drawn, order) = display_order(&piece);
        for c in drawn {
            // Pushed, not appended: `append` merges sections of one format into one run.
            let start = job.text.len();
            job.text.push(c);
            let byte_range = egui::text::ByteIndex(start)..egui::text::ByteIndex(job.text.len());
            job.sections.push(egui::text::LayoutSection { leading_space: 0.0, byte_range, format: format.clone() });
        }
        orders.push(order);
        if k + 1 < lines.len() {
            job.append("\n", 0.0, format.clone());
        }
    }
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let mut galley = Arc::unwrap_or_clone(galley);
    // The rows are the lines (nothing wraps): give each character its caret position, in typed
    // order, and place right-to-left lines.
    if galley.rows.len() == orders.len() {
        for (placed, (order, rtl_line)) in galley.rows.iter_mut().zip(&orders) {
            if placed.row.glyphs.len() != order.len() {
                continue;
            }
            let width = placed.row.size.x;
            let row = Arc::make_mut(&mut placed.row);
            let drawn = std::mem::take(&mut row.glyphs);
            let mut glyphs = drawn.clone();
            for (glyph, (logical, rtl)) in drawn.iter().zip(order) {
                let Some(slot) = glyphs.get_mut(*logical) else { continue };
                let mut g = *glyph;
                // The caret before a right-to-left character is at its right edge.
                if *rtl {
                    g.pos.x = glyph.max_x();
                }
                *slot = g;
            }
            // The caret after the line's last character: left of it when it runs right to left.
            if let Some((i, (_, rtl))) = order.iter().enumerate().max_by_key(|(_, (logical, _))| *logical)
                && let Some(glyph) = drawn.get(i)
            {
                row.size.x = if *rtl { glyph.pos.x } else { glyph.max_x() };
            }
            row.glyphs = glyphs;
            if *rtl_line && wrap_width.is_finite() {
                let free = (wrap_width - width).max(0.0);
                placed.pos.x += match align.unwrap_or(egui::Align::RIGHT) {
                    egui::Align::Min => 0.0,
                    egui::Align::Center => free / 2.0,
                    egui::Align::Max => free,
                };
            }
        }
    }
    if wrap_width.is_finite() && orders.iter().any(|(_, rtl)| *rtl) {
        galley.rect.max.x = galley.rect.max.x.max(galley.rect.min.x + wrap_width);
    }
    // The galley's text is the typed text, as the box's other code expects.
    let job = Arc::make_mut(&mut galley.job);
    if job.text.len() == text.len() {
        job.text = text.to_owned();
    }
    Arc::new(galley)
}

/// Line ranges (character indices) of `chars` wrapped to `width`: paragraphs end at `\n`, lines
/// break at spaces, a word wider than the line stays whole.
fn break_lines(chars: &[char], width: f32, mut measure: impl FnMut(&str) -> f32) -> Vec<std::ops::Range<usize>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, c) in chars.iter().enumerate().chain(std::iter::once((chars.len(), &'\n'))) {
        if *c != '\n' {
            continue;
        }
        // One paragraph: start..i.
        let mut line = start;
        let mut last_space = None;
        for j in start..i {
            if chars.get(j) == Some(&' ') {
                let candidate: String = chars.get(line..j).unwrap_or_default().iter().collect();
                if measure(&candidate) > width
                    && let Some(space) = last_space
                {
                    lines.push(line..space);
                    line = space + 1;
                }
                last_space = Some(j);
            }
        }
        let rest: String = chars.get(line..i).unwrap_or_default().iter().collect();
        if measure(&rest) > width
            && let Some(space) = last_space.filter(|s| *s >= line)
        {
            lines.push(line..space);
            line = space + 1;
        }
        lines.push(line..i);
        start = i + 1;
    }
    lines
}

/// For each character drawn, left to right: the character it stands for (its index in the
/// line) and whether it runs right to left; and whether the line itself reads right to left.
type Order = (Vec<(usize, bool)>, bool);

/// One line (typed order) as drawn: in display order (the Unicode bidirectional algorithm), each
/// Arabic letter in the form it takes between its neighbours (its presentation form: initial,
/// medial, final or isolated), brackets of right-to-left runs mirrored.
fn display_order(line: &[char]) -> (Vec<char>, Order) {
    let text: String = line.iter().collect();
    let shaped: Vec<char> = (0..line.len()).map(|i| joining_form(line, i)).collect();
    // unicode-bidi indexes the first level of a line, so an empty one would panic.
    if text.is_empty() {
        return (Vec::new(), (Vec::new(), false));
    }
    let info = BidiInfo::new(&text, None);
    let rtl_line = info.paragraphs.first().is_some_and(|p| p.level.is_rtl());
    let byte_to_char: Vec<usize> = {
        let mut v = vec![0; text.len() + 1];
        for (n, (b, _)) in text.char_indices().enumerate() {
            if let Some(slot) = v.get_mut(b) {
                *slot = n;
            }
        }
        v
    };
    let mut drawn = Vec::with_capacity(line.len());
    let mut order = Vec::with_capacity(line.len());
    for para in &info.paragraphs {
        let (levels, runs) = info.visual_runs(para, para.range.clone());
        for run in runs {
            let rtl = levels.get(run.start).is_some_and(Level::is_rtl);
            let Some(piece) = text.get(run.clone()) else { continue };
            let mut indices: Vec<usize> = piece.char_indices().map(|(b, _)| byte_to_char.get(run.start + b).copied().unwrap_or(0)).collect();
            if rtl {
                indices.reverse();
            }
            for i in indices {
                let c = shaped.get(i).copied().unwrap_or(' ');
                drawn.push(if rtl { mirrored(c) } else { c });
                order.push((i, rtl));
            }
        }
    }
    (drawn, (order, rtl_line))
}

/// Letters with presentation forms: the letter, its first form (isolated, then final, initial
/// and medial) and how many forms it has (2: it joins only to the letter before it).
const FORMS: &[(char, u32, u32)] = &[
    ('\u{0622}', 0xFE81, 2),
    ('\u{0623}', 0xFE83, 2),
    ('\u{0624}', 0xFE85, 2),
    ('\u{0625}', 0xFE87, 2),
    ('\u{0626}', 0xFE89, 4),
    ('\u{0627}', 0xFE8D, 2),
    ('\u{0628}', 0xFE8F, 4),
    ('\u{0629}', 0xFE93, 2),
    ('\u{062A}', 0xFE95, 4),
    ('\u{062B}', 0xFE99, 4),
    ('\u{062C}', 0xFE9D, 4),
    ('\u{062D}', 0xFEA1, 4),
    ('\u{062E}', 0xFEA5, 4),
    ('\u{062F}', 0xFEA9, 2),
    ('\u{0630}', 0xFEAB, 2),
    ('\u{0631}', 0xFEAD, 2),
    ('\u{0632}', 0xFEAF, 2),
    ('\u{0633}', 0xFEB1, 4),
    ('\u{0634}', 0xFEB5, 4),
    ('\u{0635}', 0xFEB9, 4),
    ('\u{0636}', 0xFEBD, 4),
    ('\u{0637}', 0xFEC1, 4),
    ('\u{0638}', 0xFEC5, 4),
    ('\u{0639}', 0xFEC9, 4),
    ('\u{063A}', 0xFECD, 4),
    ('\u{0641}', 0xFED1, 4),
    ('\u{0642}', 0xFED5, 4),
    ('\u{0643}', 0xFED9, 4),
    ('\u{0644}', 0xFEDD, 4),
    ('\u{0645}', 0xFEE1, 4),
    ('\u{0646}', 0xFEE5, 4),
    ('\u{0647}', 0xFEE9, 4),
    ('\u{0648}', 0xFEED, 2),
    ('\u{0649}', 0xFEEF, 2),
    ('\u{064A}', 0xFEF1, 4),
    ('\u{067E}', 0xFB56, 4),
    ('\u{0686}', 0xFB7A, 4),
    ('\u{0698}', 0xFB8A, 2),
    ('\u{06A4}', 0xFB6A, 4),
    ('\u{06A9}', 0xFB8E, 4),
    ('\u{06AF}', 0xFB92, 4),
    ('\u{06CC}', 0xFBFC, 4),
];

/// How a character joins: `Some(true)` both ways, `Some(false)` only to the character before it,
/// `None` not at all.
fn joins(c: char) -> Option<bool> {
    if c == '\u{0640}' {
        return Some(true);
    }
    FORMS.iter().find(|(l, _, _)| *l == c).map(|(_, _, n)| *n == 4)
}

/// The isolated lam-alef ligature for an alef after a lam (the final form follows it).
fn lam_alef(c: char) -> Option<u32> {
    match c {
        '\u{0622}' => Some(0xFEF5),
        '\u{0623}' => Some(0xFEF7),
        '\u{0625}' => Some(0xFEF9),
        '\u{0627}' => Some(0xFEFB),
        _ => None,
    }
}

/// Marks sit on their letter and don't break joining.
fn transparent(c: char) -> bool {
    matches!(u32::from(c), 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06DC | 0x06DF..=0x06E4 | 0x06E7..=0x06E8 | 0x06EA..=0x06ED)
}

/// Character `i` of `line` in the presentation form its neighbours give it (unchanged when it
/// has none).
fn joining_form(line: &[char], i: usize) -> char {
    let Some(&c) = line.get(i) else { return ' ' };
    let Some((_, first, count)) = FORMS.iter().find(|(l, _, _)| *l == c) else { return c };
    let before = line.get(..i).unwrap_or_default().iter().rev().find(|c| !transparent(**c));
    let after = line.get(i + 1..).unwrap_or_default().iter().find(|c| !transparent(**c));
    let to_prev = before.is_some_and(|b| joins(*b) == Some(true));
    // Lam then alef is one letter, lam-alef, drawn for the lam; the alef takes no room.
    if c == '\u{0644}'
        && let Some(ligature) = after.and_then(|a| lam_alef(*a))
    {
        return char::from_u32(ligature + u32::from(to_prev)).unwrap_or(c);
    }
    if lam_alef(c).is_some() && before == Some(&'\u{0644}') {
        return '\u{200B}';
    }
    let to_next = *count == 4 && after.is_some_and(|a| joins(*a).is_some());
    let form = match (to_prev, to_next) {
        (false, false) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (true, true) => 3,
    };
    char::from_u32(first + form.min(count - 1)).unwrap_or(c)
}

/// Before a text box laid out by [`layout`] is shown: egui paints a selection from the caret
/// positions of typed order (and recolours its letters by drawing order), which right-to-left lines
/// don't follow on screen, so it paints none and [`pointer`] paints it. Returns the selection
/// colours to hand to [`pointer`], which puts them back; `None` for text without right-to-left
/// characters.
pub fn hide_selection(ui: &mut Ui, text: &str, color: Color32) -> Option<egui::style::Selection> {
    if !text.chars().any(is_rtl_script) {
        return None;
    }
    let selection = ui.visuals().selection;
    ui.visuals_mut().selection.bg_fill = Color32::TRANSPARENT;
    ui.visuals_mut().selection.stroke.color = color;
    Some(selection)
}

/// The selected characters of a text box laid out by [`layout`], highlighted where they are drawn.
fn paint_selection(ui: &Ui, output: &egui::text_edit::TextEditOutput, text: &str, fill: Color32) {
    // egui reports a cursor range only while the box has focus.
    let Some(range) = output.cursor_range.filter(|r| !r.is_empty()) else { return };
    let [lo, hi] = range.sorted_cursors().map(|c| c.index.0);
    let chars: Vec<char> = text.chars().collect();
    let mut start = 0usize;
    for placed in &output.galley.rows {
        let count = placed.row.glyphs.len();
        let end = start.saturating_add(count);
        if lo < end && hi > start {
            let line = chars.get(start..end).unwrap_or_default();
            let (_, (order, _)) = display_order(line);
            let mut rtl = vec![false; count];
            for (i, r) in order {
                if let Some(slot) = rtl.get_mut(i) {
                    *slot = r;
                }
            }
            // Each selected character's extent, joined where they touch (overlapping translucent
            // boxes would show seams).
            let mut spans: Vec<(f32, f32)> = (lo.max(start)..hi.min(end))
                .filter_map(|i| {
                    let glyph = placed.row.glyphs.get(i - start)?;
                    let left = if rtl.get(i - start).copied().unwrap_or(false) { glyph.pos.x - glyph.advance_width } else { glyph.pos.x };
                    Some((left, left + glyph.advance_width))
                })
                .collect();
            spans.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut joined: Vec<(f32, f32)> = Vec::new();
            for (l, r) in spans {
                match joined.last_mut() {
                    Some(last) if l <= last.1 + 1.5 => last.1 = last.1.max(r),
                    _ => joined.push((l, r)),
                }
            }
            for (l, r) in joined {
                // Where egui draws the galley: its position less the galley's own left edge.
                let min = output.galley_pos + placed.pos.to_vec2() + egui::vec2(l - output.galley.rect.left(), 0.0);
                ui.painter().rect_filled(egui::Rect::from_min_size(min, egui::vec2(r - l, placed.row.size.y)), 0.0, fill.gamma_multiply(0.3));
            }
        }
        start = end.saturating_add(usize::from(placed.ends_with_newline));
    }
}

/// Put the caret where the pointer presses or drags in a text box laid out by [`layout`]: egui
/// looks for it in typed order, which a right-to-left line doesn't follow on screen.
pub fn pointer(ui: &mut Ui, output: &egui::text_edit::TextEditOutput, text: &str, selection: Option<egui::style::Selection>) {
    if let Some(selection) = selection {
        ui.visuals_mut().selection = selection;
        paint_selection(ui, output, text, selection.stroke.color);
    }
    if !text.chars().any(is_rtl_script) {
        return;
    }
    let response = &output.response.response;
    let Some(pos) = ui.input(|i| i.pointer.interact_pos()) else { return };
    let started = response.drag_started() || response.clicked();
    if !(started || response.dragged()) {
        return;
    }
    let Some(index) = index_at(&output.galley, pos - output.galley_pos) else { return };
    let anchor_id = Id::new(("rtl-text-anchor", response.id));
    let anchor = if started { index } else { ui.data(|d| d.get_temp::<usize>(anchor_id)).unwrap_or(index) };
    ui.data_mut(|d| d.insert_temp(anchor_id, anchor));
    let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), response.id) else { return };
    state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(anchor), CCursor::new(index))));
    state.store(ui.ctx(), response.id);
    ui.ctx().request_repaint();
}

/// The character index whose caret position (as [`layout`] placed it) is nearest to `at`.
fn index_at(galley: &Galley, at: egui::Vec2) -> Option<usize> {
    let mut start = 0usize;
    for row in &galley.rows {
        let count = row.glyphs.len();
        let top = row.pos.y;
        if at.y < top + row.size.y || std::ptr::eq(row, galley.rows.last()?) {
            let x = at.x - row.pos.x;
            let best = (0..=count)
                .min_by(|a, b| {
                    let xa = row.x_offset(egui::text::CharIndex(*a));
                    let xb = row.x_offset(egui::text::CharIndex(*b));
                    (xa - x).abs().total_cmp(&(xb - x).abs())
                })
                .unwrap_or(0);
            return Some(start + best);
        }
        start += count + usize::from(row.ends_with_newline);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_order_keeps_every_character_once() {
        for line in ["بسم الله الرحمن الرحيم", "تقرير 2026 (مسودة) PDF report", "abc", "", "ا\u{200F}ب (]", "\u{064B}"]
        {
            let chars: Vec<char> = line.chars().collect();
            let (drawn, (order, _)) = display_order(&chars);
            assert_eq!(drawn.len(), chars.len(), "{line:?}");
            let mut seen: Vec<usize> = order.iter().map(|(i, _)| *i).collect();
            seen.sort_unstable();
            assert_eq!(seen, (0..chars.len()).collect::<Vec<_>>(), "{line:?}");
        }
    }

    #[test]
    fn arabic_is_drawn_from_its_last_letter_in_joining_forms() {
        let chars: Vec<char> = "واحد اثنين".chars().collect();
        let (drawn, (order, rtl)) = display_order(&chars);
        assert!(rtl);
        // The leftmost glyph is the last letter typed, nun in its final form; the rightmost the
        // first, waw alone (it never joins forward).
        assert_eq!(order.first(), Some(&(9, true)));
        assert_eq!(drawn.first(), Some(&'\u{FEE6}'));
        assert_eq!(drawn.last(), Some(&'\u{FEED}'));
        // Beh between two behs is medial; a mark doesn't break the join.
        let beh: Vec<char> = "ببب".chars().collect();
        assert_eq!(display_order(&beh).0, vec!['\u{FE90}', '\u{FE92}', '\u{FE91}']);
        assert_eq!(joining_form(&['ب', '\u{064E}', 'ب'], 0), '\u{FE91}');
        let (_, (order, rtl)) = display_order(&['p', 'l']);
        assert!(!rtl && order.iter().all(|(_, r)| !r));
        assert_eq!(display_order(&[]).0, Vec::<char>::new());
    }

    /// Lam then alef is drawn as one lam-alef, alone or joined to the letter before, and the alef
    /// keeps its place as a character that takes no room.
    #[test]
    fn lam_alef_is_one_ligature() {
        let la: Vec<char> = "لا".chars().collect();
        assert_eq!(display_order(&la).0, vec!['\u{200B}', '\u{FEFB}']);
        let dollar: Vec<char> = "دولار".chars().collect();
        assert_eq!(joining_form(&dollar, 2), '\u{FEFB}', "waw doesn't join forward: the ligature stands alone");
        let word: Vec<char> = "استلام".chars().collect();
        assert_eq!(joining_form(&word, 3), '\u{FEFC}', "after teh the ligature is final");
        assert_eq!(joining_form(&word, 4), '\u{200B}');
        assert_eq!(joining_form(&['ل', 'أ'], 0), '\u{FEF7}');
        // A lam not followed by alef is an ordinary lam.
        assert_eq!(joining_form(&['ل', 'م'], 0), '\u{FEDF}');
    }

    /// The caret of every position of wrapped Arabic lies on its own row, moving left as the text
    /// goes on, and the end of the text is at the left end of its last row.
    #[test]
    fn carets_follow_right_to_left_rows() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::installed_font_definitions(false));
        let text = "الى مصرف ئيش التركي المحترم";
        let mut galley = None;
        // Two frames: the fonts are installed during the first.
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                galley = Some(layout(ui, text, &FontId::proportional(20.0), Color32::BLACK, 120.0, Some(egui::Align::Min)));
            });
            // No renderer takes the font texture here.
            output.textures_delta.clear();
        }
        let galley = galley.unwrap();
        assert!(galley.rows.len() > 1, "wraps: {}", galley.rows.len());
        let n = text.chars().count();
        let end = galley.pos_from_cursor(CCursor::new(n));
        let last = galley.rows.last().unwrap();
        assert!(end.center().y > last.min_y() && end.center().y < last.max_y(), "end on the last row: {end:?}");
        assert!(end.min.x <= last.pos.x + 2.0, "end at the row's left: {end:?} row at {:?}", last.pos);
        // Within the first row, carets move left.
        let first_len = galley.rows[0].glyphs.len();
        let xs: Vec<f32> = (0..first_len).map(|i| galley.pos_from_cursor(CCursor::new(i)).min.x).collect();
        assert!(xs.windows(2).all(|w| w[1] <= w[0] + 0.5), "{xs:?}");
        // Clicking where a caret is drawn finds that caret.
        for i in [1, n / 2, n - 1] {
            let at = galley.pos_from_cursor(CCursor::new(i)).center();
            let found = index_at(&galley, at.to_vec2()).unwrap();
            assert_eq!(galley.pos_from_cursor(CCursor::new(found)).center(), at, "{i} → {found}");
        }
    }

    #[test]
    fn lines_break_at_spaces_and_keep_paragraphs() {
        let chars: Vec<char> = "aa bb cc\ndd".chars().collect();
        let width = |s: &str| s.chars().count() as f32;
        assert_eq!(break_lines(&chars, 5.0, width), vec![0..5, 6..8, 9..11]);
        assert_eq!(break_lines(&chars, 100.0, width), vec![0..8, 9..11]);
        assert_eq!(break_lines(&[], 10.0, width), vec![0..0]);
        // A word wider than the line stays whole.
        let long: Vec<char> = "abcdefgh ij".chars().collect();
        assert_eq!(break_lines(&long, 3.0, width), vec![0..8, 9..11]);
    }
}
