//! Visual order for single-line interface text that may hold right-to-left script (file names,
//! document titles).
//!
//! egui shapes each font-face run with the script's own direction, so the letters of an Arabic
//! word join and run right to left, but it places the runs themselves left to right in logical
//! order: `واحد اثنين.pdf` would show its two words swapped. [`visual`] puts the runs in display
//! order and leaves the letters of each word alone for the shaper.
//!
//! [`display_rtl`] does the same for the labels of a right-to-left interface language (the Arabic
//! catalog), whose base direction is right to left.

use std::borrow::Cow;

use unicode_bidi::{BidiInfo, Level};

/// `text` rearranged for an egui call that lays out one line. Text without right-to-left
/// characters comes back borrowed and unchanged.
///
/// The base direction is left to right, as the interface is: a file name keeps its extension
/// at the end, the way the window title shows it. Use it for painting only; accessibility
/// labels and anything stored keep the logical text.
pub fn visual(text: &str) -> Cow<'_, str> {
    if !text.chars().any(is_rtl_script) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    push_visual(&mut out, text, Level::ltr());
    Cow::Owned(out)
}

/// `text` in display order for the paragraph direction `base`, appended to `out`.
fn push_visual(out: &mut String, text: &str, base: Level) {
    let info = BidiInfo::new(text, Some(base));
    for para in &info.paragraphs {
        let (levels, runs) = info.visual_runs(para, para.range.clone());
        for run in runs {
            let rtl = levels.get(run.start).is_some_and(Level::is_rtl);
            // Runs lie on character boundaries; a missing slice is skipped, never a panic.
            let Some(piece) = text.get(run) else { continue };
            if rtl {
                push_rtl_run(out, piece);
            } else {
                out.extend(piece.chars().filter(|c| !is_bidi_control(*c)));
            }
        }
    }
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
/// painting one line; a label egui wraps breaks at the wrong end.
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
const CAPTION_WORDS: usize = 3;
/// First of the private-use characters that stand for placeholders while a line is reordered.
/// They are strong left-to-right, like the names they replace.
const PLACEHOLDER_BASE: u32 = 0xE000;
/// Placeholders protected per line; a line with more keeps the rest as plain text.
const MAX_PLACEHOLDERS: usize = 64;

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
fn is_modifier_key(c: char) -> bool {
    matches!(c, '\u{2318}' | '\u{21E7}' | '\u{2325}' | '\u{2303}')
}

/// One right-to-left run, in display order. Stretches of right-to-left script stay in logical
/// order (egui gives each to the shaper as one face run, which reverses and joins it); what
/// lies between them (spaces, punctuation: drawn by the Latin face, left to right) is reversed
/// here, with brackets mirrored.
fn push_rtl_run(out: &mut String, run: &str) {
    let mut pieces: Vec<(bool, String)> = Vec::new();
    for c in run.chars().filter(|c| !is_bidi_control(*c)) {
        let script = is_rtl_script(c);
        match pieces.last_mut() {
            Some((kind, piece)) if *kind == script => piece.push(c),
            _ => pieces.push((script, c.to_string())),
        }
    }
    for (script, piece) in pieces.iter().rev() {
        if *script {
            out.push_str(piece);
        } else {
            out.extend(piece.chars().rev().map(mirrored));
        }
    }
}

/// Characters of the right-to-left scripts' blocks: letters, marks and the scripts' own
/// punctuation, all drawn by the script's face.
fn is_rtl_script(c: char) -> bool {
    matches!(u32::from(c), 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFC | 0x1_0800..=0x1_0FFF | 0x1_E800..=0x1_EFFF)
}

/// Direction marks, embeddings, overrides and isolates: they steer the reordering and are not drawn.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

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
