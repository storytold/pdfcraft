//! Wrapping right-to-left text into logical lines, and test-only display-order helpers.
//!
//! egui 0.37's epaint shapes text with the Unicode bidirectional algorithm (UAX #9), so
//! production code passes it logical (typing-order) text: [`wrap`] breaks a comment into logical
//! lines and lets epaint reorder each one. [`visual`], [`visual_auto`] and [`display_rtl`] are
//! kept for the tests that prove the old preordering is gone; they must not be used for painting
//! with egui 0.37, where they would reverse mixed-direction text a second time.

#[cfg(test)]
use std::borrow::Cow;
use std::ops::Range;

use unicode_bidi::BidiInfo;
#[cfg(test)]
use unicode_bidi::{Level, ParagraphInfo};

/// `text` rearranged for an egui call that lays out one line. Text without right-to-left
/// characters comes back borrowed and unchanged.
///
/// The base direction is left to right, as the interface is: a file name keeps its extension
/// at the end, the way the window title shows it. Use it for painting only; accessibility
/// labels and anything stored keep the logical text.
#[cfg(test)]
pub fn visual(text: &str) -> Cow<'_, str> {
    if !text.chars().any(is_rtl_script) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    push_visual(&mut out, text, Level::ltr());
    Cow::Owned(out)
}

/// `text` in display order for the paragraph direction `base`, appended to `out`.
#[cfg(test)]
fn push_visual(out: &mut String, text: &str, base: Level) {
    let info = BidiInfo::new(text, Some(base));
    for para in &info.paragraphs {
        push_line(out, text, &info, para, para.range.clone());
    }
}

/// The `line` of paragraph `para` (a non-empty byte range of `text`) in display order, as egui
/// needs it, appended to `out`.
#[cfg(test)]
fn push_line(out: &mut String, text: &str, info: &BidiInfo<'_>, para: &ParagraphInfo, line: Range<usize>) {
    let (levels, runs) = info.visual_runs(para, line);
    // First the true display order, every character left to right as it appears on screen.
    let mut shown = String::with_capacity(text.len());
    for run in runs {
        let rtl = levels.get(run.start).is_some_and(Level::is_rtl);
        // Runs lie on character boundaries; a missing slice is skipped, never a panic.
        let Some(piece) = text.get(run) else { continue };
        let chars = piece.chars().filter(|c| !is_bidi_control(*c));
        if rtl {
            shown.extend(chars.rev().map(mirrored));
        } else {
            shown.extend(chars);
        }
    }
    push_for_shaper(out, &shown);
}

/// `shown` (in screen order) as egui must get it: egui hands each stretch of right-to-left
/// script to the shaper as one run, which lays it out right to left (and joins its letters), so
/// each such stretch goes back to logical order. What lies between them (spaces, punctuation,
/// Latin) is drawn by the Latin face left to right and stays as it is. The stretches include
/// Persian and Arabic-Indic digits: Arabic script to the shaper, though they read left to right.
#[cfg(test)]
fn push_for_shaper(out: &mut String, shown: &str) {
    let mut stretch: Vec<char> = Vec::new();
    for c in shown.chars() {
        if is_rtl_script(c) || is_joiner(c) {
            stretch.push(c);
        } else {
            out.extend(stretch.drain(..).rev());
            out.push(c);
        }
    }
    out.extend(stretch.drain(..).rev());
}

/// One line of wrapped text (see [`wrap`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The line in logical (reading) order. epaint shapes and reorders it for display.
    pub text: String,
    /// The paragraph reads right to left: the line belongs against the right edge.
    pub rtl: bool,
}

/// Whether `text` holds any right-to-left script, so [`wrap`] or [`visual_auto`] is needed.
pub fn has_rtl(text: &str) -> bool {
    text.chars().any(is_rtl_script)
}

/// The paragraph direction for translated interface text: explicitly right to left for Arabic,
/// Kurdish and Persian, and `TextDirection::Auto` otherwise. A label whose first strong character is
/// Latin or a `{placeholder}` must not fall back to a left-to-right paragraph in an RTL locale.
pub fn ui_direction() -> egui::epaint::text::TextDirection {
    if crate::i18n::current().rtl() { egui::epaint::text::TextDirection::RightToLeft } else { egui::epaint::text::TextDirection::Auto }
}

/// Lay out one line of translated interface text with the locale's explicit base direction.
pub fn layout_ui(ui: &egui::Ui, text: impl Into<String>, font: egui::FontId, color: egui::Color32) -> std::sync::Arc<egui::Galley> {
    layout_one_line(ui, text, font, color, ui_direction())
}

/// Lay out one line of file or path text left to right, so an Arabic file name keeps `.pdf` at
/// its visual end exactly as a Latin one does. The text stays logical for accessibility.
pub fn layout_ltr(ui: &egui::Ui, text: impl Into<String>, font: egui::FontId, color: egui::Color32) -> std::sync::Arc<egui::Galley> {
    layout_one_line(ui, text, font, color, egui::epaint::text::TextDirection::LeftToRight)
}

fn layout_one_line(
    ui: &egui::Ui,
    text: impl Into<String>,
    font: egui::FontId,
    color: egui::Color32,
    direction: egui::epaint::text::TextDirection,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.into(), font, color);
    job.direction = direction;
    ui.painter().layout_job(job)
}

/// Multi-line text with right-to-left script (a comment in Persian, Arabic, Hebrew), broken
/// into logical-order lines that `fits` accepts (it measures a line in logical order).
///
/// The lines keep reading order and each line's paragraph direction ([`Line::rtl`]): egui 0.37's
/// epaint already shapes every line with the Unicode bidirectional algorithm (UAX #9), so
/// preordering here would reorder mixed Persian/Latin text a second time. Lines break between
/// words, in reading order, and a word wider than a line between characters. Copying and
/// accessibility use the same logical text.
pub fn wrap(text: &str, mut fits: impl FnMut(&str) -> bool) -> Vec<Line> {
    let info = BidiInfo::new(text, None);
    let mut lines = Vec::new();
    for para in &info.paragraphs {
        let whole = text.get(para.range.clone()).unwrap_or_default();
        // unicode-bidi ends a paragraph at the `\r` of `\r\n`; the `\n` is no new paragraph.
        if whole == "\n" && text.get(..para.range.start).is_some_and(|before| before.ends_with('\r')) {
            continue;
        }
        let content = para.range.start..para.range.start.saturating_add(whole.trim_end_matches(is_paragraph_separator).len());
        let rtl = para.level.is_rtl();
        for line in break_lines(text, content, &mut fits) {
            lines.push(Line { text: text.get(line).unwrap_or_default().to_owned(), rtl });
        }
    }
    lines
}

/// One line of `text` in display order, its direction from its first strong character (an
/// author's name). Text without right-to-left characters comes back borrowed and unchanged.
///
/// Only the bidi unit tests use this now: interface labels keep logical text and let the egui
/// fork reorder, so no production call site preorders a label.
#[cfg(test)]
pub fn visual_auto(text: &str) -> Cow<'_, str> {
    if !has_rtl(text) {
        return Cow::Borrowed(text);
    }
    let base = BidiInfo::new(text, None).paragraphs.first().map_or(Level::ltr(), |para| para.level);
    let mut out = String::with_capacity(text.len());
    push_visual(&mut out, text, base);
    Cow::Owned(out)
}

/// The byte ranges of the lines `range` of `text` breaks into: as many words as `fits` accepts
/// per line, a word too wide for any line split between characters. Always at least one line.
fn break_lines(text: &str, range: Range<usize>, fits: &mut impl FnMut(&str) -> bool) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let mut current: Option<Range<usize>> = None;
    for word in words(text, range.clone()) {
        if let Some(line) = current.as_mut() {
            if fits(text.get(line.start..word.end).unwrap_or_default()) {
                line.end = word.end;
                continue;
            }
            lines.push(line.clone());
        }
        // The word starts a line.
        let mut start = word.start;
        loop {
            let end = fitting_prefix(text, start..word.end, fits);
            if end >= word.end {
                current = Some(start..word.end);
                break;
            }
            lines.push(start..end);
            start = end;
        }
    }
    lines.push(current.unwrap_or(range.start..range.start));
    lines
}

/// The end of the longest start of `range` that `fits`, at least one character.
fn fitting_prefix(text: &str, range: Range<usize>, fits: &mut impl FnMut(&str) -> bool) -> usize {
    let piece = text.get(range.clone()).unwrap_or_default();
    let mut end = range.end;
    for (i, (at, c)) in piece.char_indices().enumerate() {
        let next = range.start.saturating_add(at).saturating_add(c.len_utf8());
        if i > 0 && !fits(text.get(range.start..next).unwrap_or_default()) {
            end = range.start.saturating_add(at);
            break;
        }
    }
    end
}

/// The byte ranges of the whitespace-separated words of `range` in `text`.
fn words(text: &str, range: Range<usize>) -> Vec<Range<usize>> {
    let mut words = Vec::new();
    let mut start = None;
    for (at, c) in text.get(range.clone()).unwrap_or_default().char_indices() {
        let at = range.start.saturating_add(at);
        match (c.is_whitespace(), start) {
            (true, Some(s)) => {
                words.push(s..at);
                start = None;
            }
            (false, None) => start = Some(at),
            _ => {}
        }
    }
    if let Some(s) = start {
        words.push(s..range.end);
    }
    words
}

/// Characters that end a paragraph (bidi class B).
fn is_paragraph_separator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{1C}'..='\u{1E}' | '\u{85}' | '\u{2029}')
}

/// A label of a right-to-left interface language, in logical order, rearranged for egui: each
/// line reads right to left, with Latin words and numbers inside it left to right.
///
/// - `{name}` placeholders stay whole and unmirrored, so [`crate::i18n::fmt`] still fills them.
///   What it inserts is drawn left to right at that place.
/// - Spaces and separator dots at either end stay at that end: the code appends to a label on
///   its right (`"Issued by: "` + a name), as the interface is still laid out left to right.
/// - A short caption's closing colon stays on the right too, next to the widget it captions.
///
/// Text without right-to-left characters comes back unchanged. Like [`visual`], this is for
/// painting one line; a label egui wraps breaks at the wrong end. Kept for the tests that prove
/// the old preordering is gone; production catalogs stay logical.
#[cfg(test)]
pub fn display_rtl(text: &str) -> String {
    if !text.chars().any(is_rtl_script) {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        push_rtl_line(&mut out, line);
    }
    out
}

/// The most words a caption may have for its closing colon to stay on the right.
#[cfg(test)]
const CAPTION_WORDS: usize = 3;
/// First of the private-use characters that stand for placeholders while a line is reordered.
/// They are strong left-to-right, like the names they replace.
#[cfg(test)]
const PLACEHOLDER_BASE: u32 = 0xE000;
/// Placeholders protected per line; a line with more keeps the rest as plain text.
#[cfg(test)]
const MAX_PLACEHOLDERS: usize = 64;

#[cfg(test)]
fn push_rtl_line(out: &mut String, line: &str) {
    let edge = |c: char| c.is_whitespace() || c == '·';
    let core = line.trim_start_matches(edge);
    let lead = line.get(..line.len().saturating_sub(core.len())).unwrap_or_default();
    let trimmed = core.trim_end_matches(edge);
    let tail = core.get(trimmed.len()..).unwrap_or_default();
    let mut core = trimmed;
    let mut colon = "";
    if let Some(caption) = core.strip_suffix(':')
        && caption.split_whitespace().count() <= CAPTION_WORDS
    {
        core = caption;
        colon = ":";
    }
    // A line that already uses the stand-in characters is reordered with its placeholders as text.
    let reserved = core.chars().any(|c| (PLACEHOLDER_BASE..PLACEHOLDER_BASE.saturating_add(MAX_PLACEHOLDERS as u32)).contains(&u32::from(c)));
    let (masked, names) = if reserved { (core.to_owned(), Vec::new()) } else { mask_placeholders(core) };
    let mut visual = String::with_capacity(masked.len());
    push_visual(&mut visual, &masked, Level::rtl());
    out.push_str(lead);
    for c in visual.chars() {
        let index = u32::from(c).checked_sub(PLACEHOLDER_BASE).and_then(|i| usize::try_from(i).ok());
        match index.and_then(|i| names.get(i)) {
            Some(name) => out.push_str(name),
            None => out.push(c),
        }
    }
    out.push_str(colon);
    out.push_str(tail);
}

/// `text` with each `{name}` placeholder and each keyboard shortcut (`⇧⌘+`) replaced by one
/// stand-in character, and what they replaced, in order. A shortcut's symbols have no direction
/// of their own, so left alone they would be drawn in reverse.
#[cfg(test)]
fn mask_placeholders(text: &str) -> (String, Vec<&str>) {
    let mut masked = String::with_capacity(text.len());
    let mut names: Vec<&str> = Vec::new();
    let mut rest = text;
    while names.len() < MAX_PLACEHOLDERS
        && let Some((start, end)) = next_atom(rest)
    {
        let (Some(before), Some(atom), Some(after)) = (rest.get(..start), rest.get(start..end), rest.get(end..)) else { break };
        // `names.len()` is below MAX_PLACEHOLDERS, so the sum is a private-use scalar.
        let Some(stand_in) = u32::try_from(names.len()).ok().and_then(|i| char::from_u32(PLACEHOLDER_BASE.saturating_add(i))) else { break };
        masked.push_str(before);
        masked.push(stand_in);
        names.push(atom);
        rest = after;
    }
    masked.push_str(rest);
    (masked, names)
}

/// The byte range of the first placeholder or keyboard shortcut in `text`.
#[cfg(test)]
fn next_atom(text: &str) -> Option<(usize, usize)> {
    let placeholder = text.find('{').and_then(|open| {
        let len = text.get(open..)?.find('}')?;
        Some((open, open.saturating_add(len).saturating_add(1)))
    });
    let shortcut = text.find(is_modifier_key).map(|at| {
        // The shortcut is the stretch around the symbol up to a space, a bracket or script text.
        let outside = |c: char| c.is_whitespace() || is_rtl_script(c) || matches!(c, '(' | ')' | '[' | ']' | '{' | '}');
        let start = text
            .get(..at)
            .and_then(|before| before.rfind(outside).and_then(|i| before.get(i..)?.chars().next().map(|c| i.saturating_add(c.len_utf8()))))
            .unwrap_or(0);
        let end = text.get(at..).and_then(|after| after.find(outside)).map_or(text.len(), |i| at.saturating_add(i));
        (start, end)
    });
    match (placeholder, shortcut) {
        (Some(p), Some(s)) => Some(if s.0 < p.0 { s } else { p }),
        (p, s) => p.or(s),
    }
}

/// The modifier-key symbols of keyboard shortcuts: ⌘ ⇧ ⌥ ⌃.
#[cfg(test)]
fn is_modifier_key(c: char) -> bool {
    matches!(c, '\u{2318}' | '\u{21E7}' | '\u{2325}' | '\u{2303}')
}

/// Characters of the right-to-left scripts' blocks: letters, marks and the scripts' own
/// punctuation, all drawn by the script's face.
fn is_rtl_script(c: char) -> bool {
    matches!(u32::from(c), 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFC | 0x1_0800..=0x1_0FFF | 0x1_E800..=0x1_EFFF)
}

/// The zero-width (non-)joiners inside a word (Persian `می‌روم`): egui shapes them with the
/// letter before them.
#[cfg(test)]
fn is_joiner(c: char) -> bool {
    matches!(c, '\u{200C}' | '\u{200D}')
}

/// Direction marks, embeddings, overrides and isolates: they steer the reordering and are not drawn.
#[cfg(test)]
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

#[cfg(test)]
fn mirrored(c: char) -> char {
    match c {
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
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_without_rtl_is_borrowed_unchanged() {
        for s in ["", "report.pdf", "日本語の文字.pdf", "a (b) [c]", "caf\u{00E9} \u{200E}x"] {
            assert!(matches!(visual(s), Cow::Borrowed(v) if v == s), "{s:?}");
        }
    }

    #[test]
    fn arabic_words_swap_and_the_extension_stays_last() {
        // The reported name: "one two.pdf" in Arabic.
        assert_eq!(visual("واحد اثنين.pdf"), "اثنين واحد.pdf");
        assert_eq!(visual("ملف.pdf"), "ملف.pdf");
        assert_eq!(visual("تقرير نهائي جدا.pdf"), "جدا نهائي تقرير.pdf");
    }

    #[test]
    fn latin_and_numbers_keep_their_own_order() {
        assert_eq!(visual("report واحد اثنين final.pdf"), "report اثنين واحد final.pdf");
        // A number inside right-to-left text reads left to right, to the left of what precedes it.
        assert_eq!(visual("ملف 2024.pdf"), "2024 ملف.pdf");
        assert_eq!(visual(r"C:\Users\me\واحد اثنين\a.pdf"), r"C:\Users\me\اثنين واحد\a.pdf");
    }

    #[test]
    fn hebrew_and_brackets() {
        assert_eq!(visual("שלום עולם.pdf"), "עולם שלום.pdf");
        // Brackets between right-to-left words flip with the direction.
        assert_eq!(visual("واحد (اثنين) ثلاثة.pdf"), "ثلاثة (اثنين) واحد.pdf");
    }

    #[test]
    fn direction_controls_are_not_drawn() {
        assert_eq!(visual("\u{202B}واحد اثنين\u{202C}.pdf"), "اثنين واحد.pdf");
        assert_eq!(visual("\u{200F}واحد\u{061C}"), "واحد");
    }

    #[test]
    fn rtl_labels_read_right_to_left() {
        // No right-to-left characters: unchanged.
        for s in ["", "Save as…", "{n} pages", "日本語"] {
            assert_eq!(display_rtl(s), s);
        }
        assert_eq!(display_rtl("حفظ"), "حفظ");
        // "Save as…": the second word and the ellipsis end up on the left.
        assert_eq!(display_rtl("حفظ باسم…"), "…باسم حفظ");
        // Latin words and numbers keep their own order inside the line.
        assert_eq!(display_rtl("إنشاء PDF من ملف"), "ملف من PDF إنشاء");
        assert_eq!(display_rtl("تصدير إلى Microsoft Word الآن"), "الآن Microsoft Word إلى تصدير");
        assert_eq!(display_rtl("صالح لمدة 5 سنوات"), "سنوات 5 لمدة صالح");
        // Keyboard shortcuts are not reversed.
        assert_eq!(display_rtl("طباعة (⌘P)"), "(⌘P) طباعة");
        assert_eq!(display_rtl("تدوير العرض (⇧⌘+)"), "(⇧⌘+) العرض تدوير");
        assert_eq!(display_rtl("التمرير مع ⌘ أيضًا"), "أيضًا ⌘ مع التمرير");
        assert_eq!(display_rtl("نص (عادي)"), "(عادي) نص");
    }

    #[test]
    fn rtl_labels_keep_placeholders_whole() {
        // Two placeholders around a colon stay together, left to right: a file name and its error.
        assert_eq!(display_rtl("تعذّر فتح {name}: {e}"), "{name}: {e} فتح تعذّر");
        assert_eq!(display_rtl("فشل {label}، السبب {e}"), "{e} السبب ،{label} فشل");
        assert_eq!(display_rtl("{n} صفحات"), "صفحات {n}");
        assert_eq!(display_rtl("الصفحة {p} من {n}"), "{n} من {p} الصفحة");
        // Adjacent placeholders keep their order, as numbers around a slash would.
        assert_eq!(display_rtl("جارٍ البحث… {s}/{p}"), "{s}/{p} …البحث جارٍ");
        // Braces that are not a placeholder are ordinary punctuation.
        assert_eq!(display_rtl("حرف } واحد"), "واحد { حرف");
        let many = format!("ا {}", "{a} ".repeat(200));
        assert_eq!(display_rtl(&many).matches("{a}").count(), 200);
    }

    #[test]
    fn rtl_labels_keep_their_edges() {
        // Edge spaces and separator dots stay where the code concatenates.
        assert_eq!(display_rtl(" (صفحة {p})"), " ({p} صفحة)");
        assert_eq!(display_rtl(" · محمي بكلمة مرور"), " · مرور بكلمة محمي");
        assert_eq!(display_rtl("سلسلة المفاتيح  ·  "), "المفاتيح سلسلة  ·  ");
        // A caption's colon stays next to its widget; a sentence's colon ends the sentence.
        assert_eq!(display_rtl("الاسم:"), "الاسم:");
        assert_eq!(display_rtl("صادرة عن: "), "عن صادرة: ");
        assert_eq!(display_rtl("اختر المعرّف الرقمي الذي تريد استخدامه:"), ":استخدامه تريد الذي الرقمي المعرّف اختر");
        // Each line is its own paragraph.
        assert_eq!(display_rtl("تعذّر عرض الصفحة.\n{e}"), ".الصفحة عرض تعذّر\n{e}");
    }

    #[test]
    fn rtl_labels_never_panic_on_odd_input() {
        let long = "ا {n} ".repeat(20_000);
        for s in [
            "ا{",
            "ا}",
            "ا{}",
            "ا{{{",
            "}ا{",
            "ا\u{E000}{n}",
            "⌘",
            "ا ⌘",
            "⌘ا⌘{",
            "ا (⇧⌘",
            "\u{064B}:",
            ":",
            "ا:",
            " : ا : ",
            "ا\n\n\nب",
            "\u{202E}ا{n}",
            long.as_str(),
        ] {
            let v = display_rtl(s);
            assert!(v.chars().count() <= s.chars().count(), "{:?}", s.chars().take(12).collect::<String>());
        }
    }

    /// `wrap` with a width of `n` characters.
    fn wrap_at(text: &str, n: usize) -> Vec<Line> {
        wrap(text, |s| s.chars().count() <= n)
    }

    fn rtl(text: &str) -> Line {
        Line { text: text.to_owned(), rtl: true }
    }

    fn ltr(text: &str) -> Line {
        Line { text: text.to_owned(), rtl: false }
    }

    /// Check that a Persian paragraph, including words joined by a zero-width non-joiner, reaches epaint as
    /// one logical right-to-left run.
    #[test]
    fn persian_paragraph_keeps_logical_order_for_epaint() {
        // The line stays in reading order; epaint shapes and reorders it (see `on_screen`).
        assert_eq!(wrap_at("من به مدرسه می‌روم", 80), [rtl("من به مدرسه می‌روم")]);
        // The zero-width non-joiner keeps a word whole, so the shaper gets it as one run.
        assert_eq!(wrap_at("کتاب‌ها", 80), [rtl("کتاب‌ها")]);
    }

    /// Check that Arabic text wraps into lines in logical reading order rather than visual order.
    #[test]
    fn arabic_lines_break_in_reading_order() {
        // The first words go on the first line, in logical order for epaint to reorder.
        assert_eq!(wrap_at("واحد اثنين ثلاثة أربعة", 11), [rtl("واحد اثنين"), rtl("ثلاثة أربعة")]);
        assert_eq!(wrap_at("واحد اثنين ثلاثة أربعة", 80), [rtl("واحد اثنين ثلاثة أربعة")]);
    }

    /// Check that each paragraph takes its base direction from its first strong character, not from leading
    /// numbers or neutral text.
    #[test]
    fn base_direction_comes_from_the_first_strong_character() {
        // Latin first: a left-to-right paragraph with an Arabic phrase inside it.
        assert_eq!(wrap_at("See واحد اثنين here", 80), [ltr("See واحد اثنين here")]);
        // Persian first: right to left, with the Latin words and number in reading order.
        assert_eq!(wrap_at("فایل PDF نسخه 2 است", 80), [rtl("فایل PDF نسخه 2 است")]);
        assert_eq!(wrap_at("سلام Hello World", 80), [rtl("سلام Hello World")]);
        // Leading neutrals and numbers don't decide it.
        assert_eq!(wrap_at("12. واحد اثنين", 80), [rtl("12. واحد اثنين")]);
        // Each paragraph finds its own direction.
        assert_eq!(wrap_at("واحد اثنين\nOK then\n\nשלום עולם", 80), [rtl("واحد اثنين"), ltr("OK then"), ltr(""), rtl("שלום עולם")]);
    }

    /// Check that right-to-left punctuation and brackets stay in logical order for epaint to mirror when
    /// displaying.
    #[test]
    fn rtl_punctuation_and_brackets() {
        // Logical text: epaint mirrors punctuation and brackets for display.
        assert_eq!(wrap_at("سلام دنیا.", 80), [rtl("سلام دنیا.")]);
        assert_eq!(wrap_at("واحد (اثنين) ثلاثة", 80), [rtl("واحد (اثنين) ثلاثة")]);
        assert_eq!(wrap_at("آیا آماده است؟", 80), [rtl("آیا آماده است؟")]);
    }

    /// Check that Persian and Arabic-Indic digits stay in logical order inside a right-to-left paragraph.
    #[test]
    fn persian_digits_go_to_the_shaper_reversed() {
        // Persian and Arabic-Indic digits stay in logical order; epaint's shaper preserves their
        // left-to-right reading inside the right-to-left paragraph.
        assert_eq!(wrap_at("سال ۱۴۰۲ شمسی", 80), [rtl("سال ۱۴۰۲ شمسی")]);
        assert_eq!(wrap_at("صفحه ١٢", 80), [rtl("صفحه ١٢")]);
        assert_eq!(wrap_at("ب۱۲", 80), [rtl("ب۱۲")]);
    }

    #[test]
    fn wrapping_handles_odd_widths_and_input() {
        assert_eq!(wrap_at("", 10), Vec::<Line>::new());
        // A word wider than the line is broken between characters.
        assert_eq!(wrap_at("abcdefghij واحد", 4), [ltr("abcd"), ltr("efgh"), ltr("ij"), ltr("واحد")]);
        // Extra spaces between words don't make empty lines; \r\n is one break.
        assert_eq!(wrap_at("واحد    اثنين\r\nب", 5), [rtl("واحد"), rtl("اثنين"), rtl("ب")]);
        // Nothing fits: one character per line, never an endless loop.
        assert_eq!(wrap("واحد", |_| false).len(), 4);
        let long = "ا ب ۱ (ج) ".repeat(5_000);
        for s in ["\u{202E}ا\u{202C}b", "\u{064B}", "\n\n", "\u{2029}ا", long.as_str()] {
            let lines = wrap_at(s, 7);
            let shown: usize = lines.iter().map(|l| l.text.chars().count()).sum();
            assert!(shown <= s.chars().count(), "{:?}", s.chars().take(12).collect::<String>());
        }
    }

    /// The characters egui puts on screen for each line of `text`, left to right, zero-width
    /// ones left out, with the interface fonts.
    fn on_screen(text: &str) -> Vec<String> {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let mut shown = Vec::new();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for line in wrap(text, |_| true) {
                let galley = ui.painter().layout_no_wrap(line.text, egui::FontId::proportional(13.0), egui::Color32::WHITE);
                let atlas = ui.fonts_mut(|fonts| fonts.image());
                let mut visual = String::new();
                for row in &galley.rows {
                    // 0.37 keeps glyphs in logical order for selection. Read their actual
                    // positions to test visual order instead of assuming vector order.
                    let mut glyphs: Vec<_> = row.glyphs.iter().filter(|g| g.advance_width > 0.0).collect();
                    glyphs.sort_by(|a, b| a.pos.x.total_cmp(&b.pos.x));
                    visual.extend(glyphs.into_iter().map(|g| match g.chr {
                        // `chr` remains logical even when the shaper mirrors the painted
                        // bracket. Read the bitmap's middle-row ink to check what is visible.
                        '(' | ')' => {
                            let [x0, y0] = g.uv_rect.min.map(usize::from);
                            let [x1, y1] = g.uv_rect.max.map(usize::from);
                            let y = usize::midpoint(y0, y1);
                            let ink: Vec<f32> = (x0..x1).map(|x| f32::from(atlas[(x, y)].a())).collect();
                            let total: f32 = ink.iter().sum();
                            assert!(total > 0.0 && !ink.is_empty(), "a bracket must have visible ink");
                            let center = ink.iter().enumerate().map(|(i, alpha)| i as f32 * alpha).sum::<f32>() / total;
                            if center < (ink.len() - 1) as f32 / 2.0 { '(' } else { ')' }
                        }
                        _ => g.chr,
                    }));
                }
                shown.push(visual);
            }
        });
        output.drop_without_applying_deltas();
        shown
    }

    fn rev(s: &str) -> String {
        s.chars().rev().collect()
    }

    #[test]
    fn egui_draws_lines_in_reading_order() {
        // Only with an Arabic-script face (the craft-fonts build input, as in CI); without one
        // egui has no shaper run to check.
        if pdfcraft_fonts::ui_arabic_fonts().is_empty() {
            return;
        }
        assert_eq!(on_screen("من به مدرسه می\u{200C}روم"), [rev("من به مدرسه میروم")]);
        assert_eq!(on_screen("فایل PDF نسخه 2 است"), [format!("{} 2 {} PDF {}", rev("است"), rev("نسخه"), rev("فایل"))]);
        assert_eq!(on_screen("سال ۱۴۰۲ شمسی"), [format!("{} ۱۴۰۲ {}", rev("شمسی"), rev("سال"))]);
        assert_eq!(on_screen("ب۱۲"), ["۱۲ب"]);
        assert_eq!(on_screen("See واحد اثنين here"), [format!("See {} {} here", rev("اثنين"), rev("واحد"))]);
        assert_eq!(on_screen("واحد (اثنين)."), [format!(".({}) {}", rev("اثنين"), rev("واحد"))]);
    }

    /// A file name keeps its extension at the visual end even when the first strong word is
    /// Arabic: with an explicit `LeftToRight` base, `.pdf` is right of every Arabic glyph.
    #[test]
    fn file_names_are_laid_out_left_to_right() {
        if pdfcraft_fonts::ui_arabic_fonts().is_empty() {
            return;
        }
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let mut positions: Vec<(char, f32)> = Vec::new();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let galley = layout_ltr(ui, "واحد اثنين.pdf", egui::FontId::proportional(13.0), egui::Color32::WHITE);
            positions = galley.rows.iter().flat_map(|row| row.glyphs.iter().map(|g| (g.chr, g.pos.x))).collect();
        })
        .drop_without_applying_deltas();
        let rightmost_arabic = positions.iter().filter(|(c, _)| is_rtl_script(*c)).map(|(_, x)| *x).fold(f32::MIN, f32::max);
        let leftmost_extension = positions.iter().filter(|(c, _)| matches!(*c, '.' | 'p' | 'd' | 'f')).map(|(_, x)| *x).fold(f32::MAX, f32::min);
        assert!(rightmost_arabic < leftmost_extension, "the extension must stay to the right: {positions:?}");
        let last = positions.iter().filter(|(c, _)| !c.is_whitespace()).max_by(|a, b| a.1.total_cmp(&b.1));
        assert_eq!(last.map(|(c, _)| *c), Some('f'), "the painted line ends in the extension");
    }

    /// Arabic/Kurdish labels get an explicit right-to-left paragraph, even when their first
    /// strong character is Latin. With English the same text stays left to right.
    #[test]
    fn arabic_ui_labels_use_an_explicit_paragraph_direction() {
        if pdfcraft_fonts::ui_arabic_fonts().is_empty() {
            return;
        }
        let previous = crate::i18n::current();
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let read = |lang: &str| {
            crate::i18n::set_current(crate::i18n::Lang::from_code(lang).expect("registered language"));
            let mut got = None;
            ctx.run_ui(egui::RawInput::default(), |ui| {
                let galley = layout_ui(ui, "PDF واحد", egui::FontId::proportional(13.0), egui::Color32::WHITE);
                let positions: Vec<_> = galley.rows.iter().flat_map(|row| row.glyphs.iter().map(|g| (g.chr, g.pos.x))).collect();
                got = Some((galley.job.direction, positions));
            })
            .drop_without_applying_deltas();
            got.expect("one layout")
        };
        let (direction, positions) = read("ar");
        crate::i18n::set_current(previous);
        assert_eq!(direction, egui::epaint::text::TextDirection::RightToLeft);
        let p = positions.iter().find(|(c, _)| *c == 'P').map(|(_, x)| *x).expect("a Latin P");
        let rightmost_arabic = positions.iter().filter(|(c, _)| is_rtl_script(*c)).map(|(_, x)| *x).fold(f32::MIN, f32::max);
        assert!(p > rightmost_arabic, "in an RTL paragraph the first Latin run is at the right: {positions:?}");

        let (direction, positions) = read("en");
        crate::i18n::set_current(previous);
        assert_eq!(direction, egui::epaint::text::TextDirection::Auto);
        let p = positions.iter().find(|(c, _)| *c == 'P').map(|(_, x)| *x).expect("a Latin P");
        let leftmost_arabic = positions.iter().filter(|(c, _)| is_rtl_script(*c)).map(|(_, x)| *x).fold(f32::MAX, f32::min);
        assert!(p < leftmost_arabic, "with English the Latin run stays at the left: {positions:?}");
    }

    #[test]
    fn single_line_with_its_own_direction() {
        assert!(matches!(visual_auto("Ada Lovelace"), Cow::Borrowed("Ada Lovelace")));
        assert_eq!(visual_auto("علی رضایی"), "رضایی علی");
        assert_eq!(visual_auto("علی 2"), "2 علی");
        assert_eq!(visual_auto("Team علی رضایی"), "Team رضایی علی");
    }

    #[test]
    fn odd_input_never_panics() {
        let long = "ا ب ".repeat(20_000);
        let odd = [
            "\u{064B}",
            "\u{064B}\u{064C} \u{0301}",
            "\u{202E}\u{202E}\u{202E}ا",
            "\u{2067}ا\u{2066}b",
            "ا\nب\r\nج\u{2029}د",
            "\u{FEFF}ا\u{0000}ب",
            "ا\u{10FFFF}\u{E000}ب",
            "((((ا]]]]",
            long.as_str(),
        ];
        for s in odd {
            let v = visual(s);
            assert!(v.chars().count() <= s.chars().count(), "{:?}", s.chars().take(12).collect::<String>());
        }
    }
}
