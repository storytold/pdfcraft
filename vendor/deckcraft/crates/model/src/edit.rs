//! Text editing on a [`TextBody`]: positions are (paragraph, character offset). Inserting text
//! with `\n` splits paragraphs; formatting a range splits runs at its ends.

use crate::text::{Paragraph, Run, RunKind, RunProps, TextBody};
use crate::words;
use unicode_segmentation::UnicodeSegmentation;

/// (paragraph, char offset in the paragraph).
pub type Pos = (usize, usize);

fn clamp(body: &TextBody, p: Pos) -> Pos {
    if body.paragraphs.is_empty() {
        return (0, 0);
    }
    let pi = p.0.min(body.paragraphs.len() - 1);
    let n = body.paragraphs.get(pi).map(Paragraph::char_len).unwrap_or(0);
    (pi, p.1.min(n))
}

fn order(a: Pos, b: Pos) -> (Pos, Pos) {
    if a <= b { (a, b) } else { (b, a) }
}

/// Split the paragraph's runs so a run boundary falls at char `at`; returns the index of the
/// first run starting at or after `at`.
fn split_at(p: &mut Paragraph, at: usize) -> usize {
    let mut pos = 0;
    let mut i = 0;
    while i < p.runs.len() {
        let len = p.runs.get(i).map(Run::char_len).unwrap_or(0);
        if pos == at {
            return i;
        }
        if at < pos + len {
            let Some(r) = p.runs.get(i) else { return i };
            if r.kind != RunKind::Text {
                return i + 1;
            }
            let k = at - pos;
            let byte = r.text.char_indices().nth(k).map(|(b, _)| b).unwrap_or(r.text.len());
            let (a, b) = r.text.split_at(byte);
            let second = Run { text: b.to_string(), props: r.props.clone(), kind: RunKind::Text };
            let first = a.to_string();
            if let Some(r) = p.runs.get_mut(i) {
                r.text = first;
            }
            p.runs.insert(i + 1, second);
            return i + 1;
        }
        pos += len;
        i += 1;
    }
    p.runs.len()
}

/// The explicit run properties in effect at `pos` (the run before the caret, or the end mark).
pub fn props_at(body: &TextBody, pos: Pos) -> RunProps {
    let (pi, ch) = clamp(body, pos);
    let Some(p) = body.paragraphs.get(pi) else { return RunProps::default() };
    let mut acc = 0;
    let mut last = None;
    for r in &p.runs {
        let len = r.char_len();
        if ch <= acc + len && ch > acc {
            return r.props.clone();
        }
        if acc + len >= ch && ch == 0 {
            return r.props.clone();
        }
        acc += len;
        last = Some(&r.props);
    }
    last.cloned().unwrap_or_else(|| p.end_props.clone())
}

/// Insert `text` at `pos` (with `props`, else the props at `pos`). `\n` starts a new paragraph
/// (same level and paragraph props); `\u{b}` inserts a line break. Returns the caret after it.
pub fn insert(body: &mut TextBody, pos: Pos, text: &str, props: Option<RunProps>) -> Pos {
    if body.paragraphs.is_empty() {
        body.paragraphs.push(Paragraph::default());
    }
    let (pi0, mut ch) = clamp(body, pos);
    let props = props.unwrap_or_else(|| props_at(body, (pi0, ch)));
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    // Work on the paragraph being edited apart from the body, so each new paragraph is pushed to a
    // list and the body's vector is spliced once instead of shifted for every `\n`.
    let Some(slot) = body.paragraphs.get_mut(pi0) else { return (pi0, ch) };
    let mut cur = std::mem::take(slot);
    let mut done: Vec<Paragraph> = Vec::new();
    let mut first = true;
    for part in text.split('\n') {
        if !first {
            // Split the current paragraph at ch.
            let idx = split_at(&mut cur, ch);
            let tail: Vec<Run> = cur.runs.drain(idx..).collect();
            let newp = Paragraph {
                level: cur.level,
                props: cur.props.clone(),
                runs: tail,
                def_props: cur.def_props.clone(),
                end_props: cur.end_props.clone(),
            };
            cur.end_props = props.clone();
            done.push(std::mem::replace(&mut cur, newp));
            ch = 0;
        }
        first = false;
        // The run index just after the last insertion: the next one lands there with no rescan.
        let mut next: Option<usize> = None;
        for (k, seg) in part.split('\u{b}').enumerate() {
            if k > 0 {
                let idx = next.unwrap_or_else(|| split_at(&mut cur, ch));
                cur.runs.insert(idx, Run { text: String::new(), props: props.clone(), kind: RunKind::Break });
                ch += 1;
                next = Some(idx + 1);
            }
            if seg.is_empty() {
                continue;
            }
            let idx = next.unwrap_or_else(|| split_at(&mut cur, ch));
            cur.runs.insert(idx, Run::with(seg, props.clone()));
            ch += seg.chars().count();
            next = Some(idx + 1);
        }
        cur.normalize();
        if cur.runs.is_empty() {
            cur.end_props = props.clone();
        }
    }
    let pi = pi0 + done.len();
    done.push(cur);
    body.paragraphs.splice(pi0..=pi0, done);
    (pi, ch)
}

/// Delete the range between `a` and `b`; returns the caret at the start.
pub fn delete(body: &mut TextBody, a: Pos, b: Pos) -> Pos {
    if body.paragraphs.is_empty() {
        return (0, 0);
    }
    let (s, e) = order(clamp(body, a), clamp(body, b));
    if s == e {
        return s;
    }
    if s.0 == e.0 {
        if let Some(p) = body.paragraphs.get_mut(s.0) {
            let end_props = props_of_range(p, s.1);
            let i0 = split_at(p, s.1);
            let i1 = split_at(p, e.1);
            if i1 > i0 {
                p.runs.drain(i0..i1);
            }
            p.normalize();
            if p.runs.is_empty() {
                p.end_props = end_props;
            }
        }
        return s;
    }
    // Keep the head of s.0 and the tail of e.0, joined; drop paragraphs between.
    let tail: Vec<Run> = match body.paragraphs.get_mut(e.0) {
        Some(p) => {
            let i = split_at(p, e.1);
            p.runs.drain(i..).collect()
        }
        None => vec![],
    };
    if let Some(p) = body.paragraphs.get_mut(s.0) {
        let i = split_at(p, s.1);
        p.runs.truncate(i);
        p.runs.extend(tail);
        p.normalize();
    }
    if e.0 > s.0 && e.0 < body.paragraphs.len() {
        body.paragraphs.drain(s.0 + 1..=e.0);
    }
    s
}

fn props_of_range(p: &Paragraph, at: usize) -> RunProps {
    let mut acc = 0;
    for r in &p.runs {
        let len = r.char_len();
        if at < acc + len || (at == acc + len && at > 0) {
            return r.props.clone();
        }
        acc += len;
    }
    p.runs.last().map(|r| r.props.clone()).unwrap_or_else(|| p.end_props.clone())
}

/// Apply `f` to the run properties of every run in [a, b). An empty range changes the empty
/// paragraph's end mark (so typing there picks the formatting up).
pub fn format(body: &mut TextBody, a: Pos, b: Pos, f: &dyn Fn(&mut RunProps)) {
    if body.paragraphs.is_empty() {
        return;
    }
    let (s, e) = order(clamp(body, a), clamp(body, b));
    for pi in s.0..=e.0 {
        let Some(p) = body.paragraphs.get_mut(pi) else { continue };
        let from = if pi == s.0 { s.1 } else { 0 };
        let to = if pi == e.0 { e.1 } else { p.char_len() };
        if p.runs.is_empty() || (from == to && s == e) {
            f(&mut p.end_props);
            continue;
        }
        let i0 = split_at(p, from);
        let i1 = split_at(p, to);
        for r in p.runs.iter_mut().take(i1).skip(i0) {
            f(&mut r.props);
        }
        if to == p.char_len() {
            f(&mut p.end_props);
        }
        p.normalize();
    }
}

/// Apply `f` to every run in the body (and end marks).
pub fn format_all(body: &mut TextBody, f: &dyn Fn(&mut RunProps)) {
    for p in &mut body.paragraphs {
        for r in &mut p.runs {
            f(&mut r.props);
        }
        f(&mut p.end_props);
        p.normalize();
    }
}

/// The text between two positions (`\n` between paragraphs).
pub fn text_range(body: &TextBody, a: Pos, b: Pos) -> String {
    let (s, e) = order(clamp(body, a), clamp(body, b));
    let mut out = String::new();
    for pi in s.0..=e.0 {
        let Some(p) = body.paragraphs.get(pi) else { continue };
        let t: Vec<char> = p.text().chars().collect();
        let from = if pi == s.0 { s.1 } else { 0 };
        let to = if pi == e.0 { e.1 } else { t.len() };
        out.extend(t.get(from.min(t.len())..to.min(t.len())).unwrap_or(&[]));
        if pi != e.0 {
            out.push('\n');
        }
    }
    out
}

/// A formatted copy of the range.
pub fn slice(body: &TextBody, a: Pos, b: Pos) -> TextBody {
    let (s, e) = order(clamp(body, a), clamp(body, b));
    let mut out = TextBody { body: body.body.clone(), list_style: body.list_style.clone(), paragraphs: vec![] };
    for pi in s.0..=e.0 {
        let Some(p) = body.paragraphs.get(pi) else { continue };
        let mut p = p.clone();
        let to = if pi == e.0 { e.1 } else { p.char_len() };
        let i1 = split_at(&mut p, to);
        p.runs.truncate(i1);
        let from = if pi == s.0 { s.1 } else { 0 };
        let i0 = split_at(&mut p, from);
        p.runs.drain(..i0);
        out.paragraphs.push(p);
    }
    out
}

/// Insert a formatted body at `pos` (paste): first paragraph merges into the current one.
pub fn insert_body(body: &mut TextBody, pos: Pos, src: &TextBody) -> Pos {
    let mut caret = clamp(body, pos);
    // Plain text runs go straight in at the run index after the previous one and the paragraph is
    // normalized once, not rescanned and merged for every source run.
    let mut next: Option<usize> = None;
    let mut dirty = false;
    for (k, sp) in src.paragraphs.iter().enumerate() {
        if k > 0 {
            if let (true, Some(p)) = (dirty, body.paragraphs.get_mut(caret.0)) {
                p.normalize();
            }
            (next, dirty) = (None, false);
            caret = insert(body, caret, "\n", None);
            if let Some(p) = body.paragraphs.get_mut(caret.0) {
                p.level = sp.level;
                p.props = sp.props.clone();
            }
        }
        for r in &sp.runs {
            if r.kind == RunKind::Text
                && !r.text.is_empty()
                && !r.text.contains(['\n', '\r', '\u{b}'])
                && let Some(p) = body.paragraphs.get_mut(caret.0)
            {
                let idx = next.unwrap_or_else(|| split_at(p, caret.1));
                p.runs.insert(idx, Run::with(&r.text, r.props.clone()));
                caret.1 += r.text.chars().count();
                (next, dirty) = (Some(idx + 1), true);
                continue;
            }
            // Anything else takes the general path, after the pending runs are merged.
            if let (true, Some(p)) = (dirty, body.paragraphs.get_mut(caret.0)) {
                p.normalize();
            }
            (next, dirty) = (None, false);
            match r.kind {
                RunKind::Text => caret = insert(body, caret, &r.text, Some(r.props.clone())),
                RunKind::Break => caret = insert(body, caret, "\u{b}", Some(r.props.clone())),
                _ => {
                    if let Some(p) = body.paragraphs.get_mut(caret.0) {
                        let idx = split_at(p, caret.1);
                        p.runs.insert(idx, r.clone());
                        caret.1 += r.char_len();
                    }
                }
            }
        }
    }
    if let (true, Some(p)) = (dirty, body.paragraphs.get_mut(caret.0)) {
        p.normalize();
    }
    caret
}

fn para_text(body: &TextBody, pi: usize) -> String {
    body.paragraphs.get(pi).map(Paragraph::text).unwrap_or_default()
}

/// The word a double-click selects around `pos` (see [`words::word_at`]): the word without its
/// trailing spaces, as PowerPoint selects it.
pub fn word_at(body: &TextBody, pos: Pos) -> (Pos, Pos) {
    let (pi, ch) = clamp(body, pos);
    let (s, e) = words::word_at(&para_text(body, pi), ch);
    ((pi, s), (pi, e))
}

/// Next/previous word stop (Ctrl+Right/Left on Windows, Option+arrows on the Mac). At the end of
/// a paragraph Ctrl+Right goes to the start of the next one; at its start Ctrl+Left goes to the
/// end of the previous one.
pub fn word_move(body: &TextBody, pos: Pos, forward: bool) -> Pos {
    let (pi, ch) = clamp(body, pos);
    let t = para_text(body, pi);
    if forward {
        match words::next_stop(&t, ch) {
            Some(s) => (pi, s),
            None if pi + 1 < body.paragraphs.len() => (pi + 1, 0),
            None => (pi, ch),
        }
    } else {
        match words::prev_stop(&t, ch) {
            Some(s) => (pi, s),
            None if pi > 0 => (pi - 1, body.paragraphs.get(pi - 1).map(Paragraph::char_len).unwrap_or(0)),
            None => (pi, 0),
        }
    }
}

/// Ctrl+Up: the start of the paragraph, or of the previous one when already there.
pub fn para_up(body: &TextBody, pos: Pos) -> Pos {
    let (pi, ch) = clamp(body, pos);
    if ch > 0 { (pi, 0) } else { (pi.saturating_sub(1), 0) }
}

/// Ctrl+Down: the start of the next paragraph, or the end of the last one.
pub fn para_down(body: &TextBody, pos: Pos) -> Pos {
    let (pi, _) = clamp(body, pos);
    if pi + 1 < body.paragraphs.len() { (pi + 1, 0) } else { (pi, body.paragraphs.get(pi).map(Paragraph::char_len).unwrap_or(0)) }
}

/// Delete a word from `pos` as Ctrl+Backspace (`forward == false`) or Ctrl+Delete does in
/// PowerPoint; returns the caret.
///
/// Ctrl+Backspace deletes back to the previous word stop (at a paragraph's start it joins the
/// paragraph to the one before). Ctrl+Delete deletes the rest of the word, or the spaces after a
/// word and the next word, or up to the next stop (see [`words::delete_forward_end`]), then
/// tidies the spacing as PowerPoint's smart cut does: spaces left before closing punctuation or
/// at the end of the paragraph after a word go, and sentence punctuation left against a word
/// gets a space. At a paragraph's end it joins the next paragraph.
pub fn delete_word(body: &mut TextBody, pos: Pos, forward: bool) -> Pos {
    if body.paragraphs.is_empty() {
        return (0, 0);
    }
    let (pi, ch) = clamp(body, pos);
    let t = para_text(body, pi);
    if !forward {
        let to = match words::prev_stop(&t, ch) {
            Some(s) => (pi, s),
            None => char_move(body, (pi, ch), false),
        };
        return delete(body, to, (pi, ch));
    }
    let Some(e) = words::delete_forward_end(&t, ch) else {
        let to = char_move(body, (pi, ch), true);
        return delete(body, (pi, ch), to);
    };
    let at = delete(body, (pi, ch), (pi, e));
    smart_space(body, at)
}

/// PowerPoint's smart-cut spacing after a word was deleted at `at`; returns the caret.
fn smart_space(body: &mut TextBody, at: Pos) -> Pos {
    let (pi, s) = at;
    let chars: Vec<char> = para_text(body, pi).chars().collect();
    let after = chars.get(s).copied();
    let before = s.checked_sub(1).and_then(|i| chars.get(i)).copied();
    if after.is_none_or(words::is_closing_punct) && before.is_some_and(|c| c == ' ') {
        let mut w = s;
        while w > 0 && chars.get(w - 1) == Some(&' ') {
            w -= 1;
        }
        if w > 0 && chars.get(w - 1).is_some_and(|c| c.is_alphanumeric()) {
            return delete(body, (pi, w), (pi, s));
        }
        return at;
    }
    if let (Some(b), Some(a)) = (before, after)
        && words::is_sentence_punct(b)
        && a.is_alphanumeric()
    {
        let digits = s >= 2 && chars.get(s - 2).is_some_and(|c| c.is_numeric()) && a.is_numeric();
        if !digits {
            return insert(body, at, " ", None);
        }
    }
    at
}

/// The position after the grapheme cluster at `pos`, or the next paragraph's start at a paragraph end.
pub fn cluster_end(body: &TextBody, pos: Pos) -> Pos {
    let (pi, ch) = clamp(body, pos);
    let text = body.paragraphs.get(pi).map(Paragraph::text).unwrap_or_default();
    // Boundaries come from the whole paragraph, so a caret inside a cluster deletes only to its end.
    let mut end = 0;
    for g in text.graphemes(true) {
        end += g.chars().count();
        if end > ch {
            return (pi, end);
        }
    }
    char_move(body, pos, true)
}

/// One character left/right across paragraph boundaries.
pub fn char_move(body: &TextBody, pos: Pos, forward: bool) -> Pos {
    let (pi, ch) = clamp(body, pos);
    let n = body.paragraphs.get(pi).map(Paragraph::char_len).unwrap_or(0);
    if forward {
        if ch < n {
            (pi, ch + 1)
        } else if pi + 1 < body.paragraphs.len() {
            (pi + 1, 0)
        } else {
            (pi, ch)
        }
    } else if ch > 0 {
        (pi, ch - 1)
    } else if pi > 0 {
        (pi - 1, body.paragraphs.get(pi - 1).map(Paragraph::char_len).unwrap_or(0))
    } else {
        (0, 0)
    }
}

/// The position of the `n`th character of the body's plain text (`\n` between paragraphs).
fn pos_of_char(body: &TextBody, mut n: usize) -> Pos {
    for (pi, p) in body.paragraphs.iter().enumerate() {
        let len = p.char_len();
        if n <= len {
            return (pi, n);
        }
        n -= len + 1;
    }
    end(body)
}

/// Make the body's plain text (paragraphs joined by `\n`) read `text`, as if it had been typed in
/// place: only the span between the longest common prefix and suffix changes, so untouched text
/// keeps its run and paragraph formatting and new text takes the formatting where it lands. Text
/// that shares nothing with the old text replaces it as plain text.
pub fn replace_text(body: &mut TextBody, text: &str) {
    // Normalise line endings first so the common prefix/suffix cannot split a `\r\n` pair.
    let text = &text.replace("\r\n", "\n").replace('\r', "\n");
    let old: Vec<char> = body.text().chars().collect();
    let new: Vec<char> = text.chars().collect();
    if old == new {
        return;
    }
    let pre = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let room = old.len().min(new.len()) - pre;
    let suf = old.iter().rev().zip(new.iter().rev()).take(room).take_while(|(a, b)| a == b).count();
    if pre == 0 && suf == 0 {
        *body = TextBody { body: body.body.clone(), list_style: body.list_style.clone(), ..TextBody::from_text(text) };
        return;
    }
    let a = pos_of_char(body, pre);
    let b = pos_of_char(body, old.len() - suf);
    let at = delete(body, a, b);
    let mid: String = new.get(pre..new.len() - suf).unwrap_or(&[]).iter().collect();
    if !mid.is_empty() {
        insert(body, at, &mid, None);
    }
}

/// End of the body.
pub fn end(body: &TextBody) -> Pos {
    let pi = body.paragraphs.len().saturating_sub(1);
    (pi, body.paragraphs.get(pi).map(Paragraph::char_len).unwrap_or(0))
}

/// The case Shift+F3 changes `s` to next, as PowerPoint cycles it: lowercase → Capitalize Each
/// Word → UPPERCASE → lowercase; any other mix goes to UPPERCASE first.
pub fn next_case(s: &str) -> &'static str {
    if s == s.to_lowercase() && s.chars().any(char::is_alphabetic) {
        "title"
    } else if s == s.to_uppercase() && s.chars().any(char::is_alphabetic) {
        "lower"
    } else {
        "upper"
    }
}

/// Whether the `.` at `i` sits between two ASCII digits (a decimal point or version number).
fn is_decimal_point(all: &[char], i: usize) -> bool {
    i > 0 && all[i - 1].is_ascii_digit() && all.get(i + 1).is_some_and(|n| n.is_ascii_digit())
}

/// Change case: `upper`, `lower`, `sentence`, `title`, `toggle`.
pub fn change_case(s: &str, mode: &str) -> String {
    change_case_parts(&[s], mode).concat()
}

/// Change the case of consecutive pieces of text (the runs of a paragraph) as one string, so
/// `title` and `sentence` carry their word/sentence state across piece boundaries. Each output
/// piece is the changed form of the matching input piece, even where a character changes length.
pub fn change_case_parts(parts: &[&str], mode: &str) -> Vec<String> {
    if mode == "lower" {
        let lower = parts.concat().to_lowercase();
        let mut chars = lower.chars();
        // Context changes sigma's form, while each character's expansion length stays the same.
        return parts.iter().map(|s| chars.by_ref().take(s.chars().map(|c| c.to_lowercase().count()).sum()).collect()).collect();
    }
    let mut start = true;
    // All characters of all pieces, so word-final sigma sees neighbours across piece boundaries.
    let all: Vec<char> = parts.iter().flat_map(|s| s.chars()).collect();
    let mut base = 0;
    parts
        .iter()
        .map(|s| match mode {
            "upper" => s.to_uppercase(),
            "toggle" => s.chars().map(|c| if c.is_uppercase() { c.to_lowercase().collect::<String>() } else { c.to_uppercase().collect() }).collect(),
            _ => {
                // `title` restarts after whitespace, sentence case after sentence punctuation.
                let title = mode == "title";
                let mut out = String::new();
                for (j, c) in s.chars().enumerate() {
                    let i = base + j;
                    if start && c.is_alphabetic() {
                        out.extend(c.to_uppercase());
                        start = false;
                    } else if c == 'Σ' && i > 0 && all[i - 1].is_alphabetic() && !all.get(i + 1).is_some_and(|n| n.is_alphabetic()) {
                        // Word-final capital sigma lowercases to ς, as `str::to_lowercase` does.
                        out.push('ς');
                    } else {
                        out.extend(c.to_lowercase());
                    }
                    if (title && c.is_whitespace()) || (!title && matches!(c, '.' | '!' | '?') && !(c == '.' && is_decimal_point(&all, i))) {
                        start = true;
                    }
                }
                base += s.chars().count();
                out
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(t: &str) -> TextBody {
        TextBody::from_text(t)
    }

    #[test]
    fn title_and_sentence_final_sigma() {
        assert_eq!(change_case("ΟΔΟΣ", "title"), "Οδος");
        assert_eq!(change_case("ΟΔΟΣ ΜΟΥ.", "sentence"), "Οδος μου.");
        assert_eq!(change_case("ΟΔΟΣ", "lower"), "οδος");
    }

    #[test]
    fn sentence_case_decimal_point_is_not_a_boundary() {
        assert_eq!(change_case("I HAVE 3.5 APPLES", "sentence"), "I have 3.5 apples");
        assert_eq!(change_case_parts(&["V 1.", "2 BETA"], "sentence").concat(), "V 1.2 beta");
        assert_eq!(change_case("HELLO. WORLD", "sentence"), "Hello. World");
    }

    #[test]
    fn title_sigma_across_runs_is_not_final() {
        assert_eq!(change_case_parts(&["ΟΔΟΣ", "ΑΣ"], "title"), ["Οδοσ", "ας"]);
        assert_eq!(change_case_parts(&["ΟΔΟ", "ΣΑ"], "title"), ["Οδο", "σα"]);
        assert_eq!(change_case_parts(&["ΟΔΟΣ", " ΜΟΥ"], "title"), ["Οδος", " Μου"]);
    }

    #[test]
    fn lowercase_keeps_context_and_run_boundaries() {
        assert_eq!(change_case_parts(&["ΟΔΟ", "Σ"], "lower"), ["οδο", "ς"]);
        assert_eq!(change_case_parts(&["ΟΣ", "Α"], "lower"), ["οσ", "α"]);
        assert_eq!(change_case_parts(&["", "ΟΔΟ", "", "Σ", ""], "lower"), ["", "οδο", "", "ς", ""]);
        assert_eq!(change_case_parts(&["İΟΔΟ", "Σ"], "lower"), ["i\u{307}οδο", "ς"]);
        assert_eq!(change_case_parts(&["Ο", "\u{301}", "Σ"], "lower"), ["ο", "\u{301}", "ς"]);
        assert_eq!(change_case_parts(&["ΟΣ", "\u{301}", "Α"], "lower"), ["οσ", "\u{301}", "α"]);
        assert_eq!(change_case_parts(&["", "", ""], "lower"), ["", "", ""]);
        assert!(change_case_parts(&[], "lower").is_empty());
    }

    #[test]
    fn insert_and_split() {
        let mut b = body("Hello");
        let c = insert(&mut b, (0, 5), " world\nNext", None);
        assert_eq!(b.text(), "Hello world\nNext");
        assert_eq!(c, (1, 4));
        let c = insert(&mut b, (0, 0), ">", None);
        assert_eq!(c, (0, 1));
        assert_eq!(b.paragraphs[0].runs.len(), 1);
    }

    #[test]
    fn delete_within_and_across() {
        let mut b = body("abc\ndef\nghi");
        assert_eq!(delete(&mut b, (0, 1), (2, 1)), (0, 1));
        assert_eq!(b.text(), "ahi");
        let mut b = body("abcdef");
        delete(&mut b, (0, 4), (0, 2));
        assert_eq!(b.text(), "abef");
        // hostile positions
        delete(&mut b, (99, 99), (0, 0));
        assert_eq!(b.text(), "");
    }

    #[test]
    fn format_splits_runs() {
        let mut b = body("abcdef");
        format(&mut b, (0, 2), (0, 4), &|p| p.bold = Some(true));
        let p = &b.paragraphs[0];
        assert_eq!(p.runs.len(), 3);
        assert_eq!(p.runs[1].text, "cd");
        assert_eq!(p.runs[1].props.bold, Some(true));
        assert_eq!(props_at(&b, (0, 3)).bold, Some(true));
        assert_eq!(text_range(&b, (0, 1), (0, 5)), "bcde");
        let s = slice(&b, (0, 1), (0, 5));
        assert_eq!(s.text(), "bcde");
        let mut c = body("XY");
        let caret = insert_body(&mut c, (0, 1), &s);
        assert_eq!(c.text(), "XbcdeY");
        assert_eq!(caret, (0, 5));
    }

    /// The paste as it ran before the fast path: one `insert` (and normalize) per source run.
    fn insert_body_by_runs(body: &mut TextBody, pos: Pos, src: &TextBody) -> Pos {
        let mut caret = clamp(body, pos);
        for (k, sp) in src.paragraphs.iter().enumerate() {
            if k > 0 {
                caret = insert(body, caret, "\n", None);
                if let Some(p) = body.paragraphs.get_mut(caret.0) {
                    p.level = sp.level;
                    p.props = sp.props.clone();
                }
            }
            for r in &sp.runs {
                match r.kind {
                    RunKind::Text => caret = insert(body, caret, &r.text, Some(r.props.clone())),
                    RunKind::Break => caret = insert(body, caret, "\u{b}", Some(r.props.clone())),
                    _ => {
                        if let Some(p) = body.paragraphs.get_mut(caret.0) {
                            let idx = split_at(p, caret.1);
                            p.runs.insert(idx, r.clone());
                            caret.1 += r.char_len();
                        }
                    }
                }
            }
        }
        caret
    }

    #[test]
    fn paste_matches_inserting_run_by_run() {
        let bold = |t: &str| Run::with(t, RunProps { bold: Some(true), ..Default::default() });
        let mut src = body("");
        src.paragraphs = vec![
            Paragraph {
                runs: vec![Run::with("ab", RunProps::default()), Run::with("cd", RunProps::default()), bold("ef"), bold("g")],
                ..Default::default()
            },
            Paragraph {
                runs: vec![
                    bold("h"),
                    Run { text: String::new(), props: RunProps::default(), kind: RunKind::Break },
                    Run::with("i", RunProps::default()),
                ],
                level: 1,
                ..Default::default()
            },
            Paragraph { runs: vec![Run::with("jk", RunProps::default())], ..Default::default() },
        ];
        for at in [0, 1, 3, 6] {
            let mut dest = body("XYZabc");
            format(&mut dest, (0, 2), (0, 4), &|p| p.italic = Some(true));
            let mut want = dest.clone();
            let want_caret = insert_body_by_runs(&mut want, (0, at), &src);
            let caret = insert_body(&mut dest, (0, at), &src);
            assert_eq!(caret, want_caret, "caret at {at}");
            assert_eq!(format!("{:?}", dest.paragraphs), format!("{:?}", want.paragraphs), "paste at {at}");
        }
    }

    #[test]
    fn multiline_insert_splits_into_paragraphs() {
        let mut b = body("abcd");
        let c = insert(&mut b, (0, 2), "x\ny\u{b}z\n\nw", None);
        assert_eq!(c, (3, 1));
        let texts: Vec<String> = b.paragraphs.iter().map(Paragraph::text).collect();
        assert_eq!(texts, ["abx", "y\u{b}z", "", "wcd"]);
    }

    #[test]
    fn words_and_moves() {
        let b = body("Hello brave world");
        assert_eq!(word_at(&b, (0, 7)), ((0, 6), (0, 11)));
        assert_eq!(word_move(&b, (0, 0), true), (0, 6));
        assert_eq!(word_move(&b, (0, 8), false), (0, 6));
        assert_eq!(char_move(&b, (0, 17), true), (0, 17));
        assert_eq!(change_case("hello world. again", "sentence"), "Hello world. Again");
        assert_eq!(change_case("hello world", "title"), "Hello World");
    }

    #[test]
    fn line_break_insert() {
        let mut b = body("ab");
        let c = insert(&mut b, (0, 1), "\u{b}", None);
        assert_eq!(c, (0, 2));
        assert_eq!(b.paragraphs[0].char_len(), 3);
        assert_eq!(b.paragraphs.len(), 1);
    }

    /// Ctrl+Backspace / Ctrl+Delete from `at`: the text and caret after (PowerPoint's results).
    fn del_word(text: &str, at: Pos, forward: bool) -> (String, Pos) {
        let mut b = body(text);
        let c = delete_word(&mut b, at, forward);
        (b.text(), c)
    }

    #[test]
    fn ctrl_backspace_like_powerpoint() {
        let t = "ab   cd...ef gh";
        let want = [
            ("ab   cd...ef gh", 0),
            ("b   cd...ef gh", 0),
            ("   cd...ef gh", 0),
            ("  cd...ef gh", 0),
            (" cd...ef gh", 0),
            ("cd...ef gh", 0),
            ("ab   d...ef gh", 5),
            ("ab   ...ef gh", 5),
            ("ab   cd..ef gh", 7),
            ("ab   cd.ef gh", 7),
            ("ab   cdef gh", 7),
            ("ab   cd...f gh", 10),
            ("ab   cd... gh", 10),
            ("ab   cd...gh", 10),
            ("ab   cd...ef h", 13),
            ("ab   cd...ef ", 13),
        ];
        for (c, (txt, caret)) in want.iter().enumerate() {
            assert_eq!(del_word(t, (0, c), false), (txt.to_string(), (0, *caret)), "caret {c}");
        }
        let w = "Hello world  foo, bar\nnext para";
        assert_eq!(del_word(w, (0, 12), false), ("Hello  foo, bar\nnext para".into(), (0, 6)));
        assert_eq!(del_word(w, (0, 17), false), ("Hello world  foo bar\nnext para".into(), (0, 16)));
        // At a paragraph's start it joins the paragraph before.
        assert_eq!(del_word(w, (1, 0), false), ("Hello world  foo, barnext para".into(), (0, 21)));
        assert_eq!(del_word("ab\n  cd", (1, 2), false), ("ab\ncd".into(), (1, 0)));
        // A line break is a word of its own.
        let mut b = body("ab");
        insert(&mut b, (0, 2), "\u{b}cd", None);
        assert_eq!(delete_word(&mut b, (0, 3), false), (0, 2));
        assert_eq!(b.text(), "abcd");
        let mut b = body("ab");
        insert(&mut b, (0, 2), "\u{b}cd", None);
        assert_eq!(delete_word(&mut b, (0, 2), true), (0, 2));
        assert_eq!(b.text(), "abcd");
        assert_eq!(del_word("x", (0, 0), false), ("x".into(), (0, 0)));
        // Emoji sequences and combining marks are never split.
        assert_eq!(del_word("a 👨\u{200d}👩\u{200d}👧", (0, 7), false), ("a ".into(), (0, 2)));
        assert_eq!(del_word("ce\u{301}te x", (0, 5), false), (" x".into(), (0, 0)));
        assert_eq!(del_word("ce\u{301}te x", (0, 6), false), ("x".into(), (0, 0)));
        // Each CJK character is a word.
        assert_eq!(del_word("日本語", (0, 3), false), ("日本".into(), (0, 2)));
    }

    #[test]
    fn ctrl_delete_like_powerpoint() {
        let t = "ab   cd...ef gh";
        let want = [
            ("cd...ef gh", 0),
            ("a   cd...ef gh", 1),
            ("ab...ef gh", 2),
            ("ab cd...ef gh", 3),
            ("ab  cd...ef gh", 4),
            ("ab...ef gh", 2),
            ("ab   c...ef gh", 6),
            ("ab   cdef gh", 7),
            ("ab   cd. ef gh", 9),
            ("ab   cd.. ef gh", 10),
            ("ab   cd... gh", 11),
            ("ab   cd...e gh", 11),
            ("ab   cd...ef", 12),
            ("ab   cd...ef", 12),
            ("ab   cd...ef g", 14),
            ("ab   cd...ef gh", 15),
        ];
        for (c, (txt, caret)) in want.iter().enumerate() {
            assert_eq!(del_word(t, (0, c), true), (txt.to_string(), (0, *caret)), "caret {c}");
        }
        let w = "Hello world  foo, bar\nnext para";
        let want = [
            (0, "world  foo, bar\nnext para", 0),
            (2, "He world  foo, bar\nnext para", 2),
            (5, "Hello  foo, bar\nnext para", 5),
            (6, "Hello foo, bar\nnext para", 6),
            (8, "Hello wo  foo, bar\nnext para", 8),
            (11, "Hello world, bar\nnext para", 11),
            (12, "Hello world foo, bar\nnext para", 12),
            (16, "Hello world  foobar\nnext para", 16),
            (17, "Hello world  foo,\nnext para", 17),
            (18, "Hello world  foo, \nnext para", 18),
            (21, "Hello world  foo, barnext para", 21),
        ];
        for (c, txt, caret) in want {
            assert_eq!(del_word(w, (0, c), true), (txt.to_string(), (0, caret)), "caret {c}");
        }
        assert_eq!(del_word(w, (1, 0), true), ("Hello world  foo, bar\npara".into(), (1, 0)));
        assert_eq!(del_word(w, (1, 2), true), ("Hello world  foo, bar\nne para".into(), (1, 2)));
        assert_eq!(del_word("ab  \ncd", (0, 2), true), ("ab\ncd".into(), (0, 2)));
        assert_eq!(del_word("ab  \ncd", (0, 3), true), ("ab\ncd".into(), (0, 2)));
        assert_eq!(del_word("ab\t cd", (0, 2), true), ("ab cd".into(), (0, 2)));
        assert_eq!(del_word("ab cd(x) ef", (0, 3), true), ("ab (x) ef".into(), (0, 3)));
        assert_eq!(del_word("ab cd, ef", (0, 3), true), ("ab, ef".into(), (0, 2)));
        assert_eq!(del_word("ab. (cd) ef", (0, 5), true), ("ab. () ef".into(), (0, 5)));
        assert_eq!(del_word("ab \"cd\" ef", (0, 4), true), ("ab \"\" ef".into(), (0, 4)));
        assert_eq!(del_word("ab, cd", (0, 4), true), ("ab, ".into(), (0, 4)));
        // Right-to-left text deletes in logical order like any other.
        assert_eq!(del_word("مرحبا بالعالم hello", (0, 6), true), ("مرحبا hello".into(), (0, 6)));
        assert_eq!(del_word("مرحبا بالعالم hello", (0, 8), false), ("مرحبا لعالم hello".into(), (0, 6)));
        // Numbers keep their separators: no space is pushed into "3.14".
        assert_eq!(del_word("3.x 14", (0, 2), true), ("3.14".into(), (0, 2)));
        // Hostile positions clamp.
        assert_eq!(del_word("ab", (9, 9), true).0, "ab");
        assert_eq!(del_word("", (0, 0), false).0, "");
        let mut empty = TextBody::default();
        assert_eq!(delete_word(&mut empty, (3, 3), true), (0, 0));
    }

    #[test]
    fn paragraph_moves() {
        let b = body("one two\nthree\n\nfour");
        assert_eq!(para_down(&b, (0, 2)), (1, 0));
        assert_eq!(para_down(&b, (1, 0)), (2, 0));
        assert_eq!(para_down(&b, (3, 1)), (3, 4));
        assert_eq!(para_up(&b, (3, 2)), (3, 0));
        assert_eq!(para_up(&b, (3, 0)), (2, 0));
        assert_eq!(para_up(&b, (0, 0)), (0, 0));
        assert_eq!(word_move(&b, (0, 4), true), (0, 7));
        assert_eq!(word_move(&b, (0, 7), true), (1, 0));
        assert_eq!(word_move(&b, (1, 0), false), (0, 7));
        assert_eq!(word_move(&b, (2, 0), true), (3, 0));
        assert_eq!(word_move(&b, (3, 4), true), (3, 4));
        assert_eq!(word_move(&b, (0, 0), false), (0, 0));
    }

    #[test]
    fn case_cycle() {
        assert_eq!(next_case("hello world"), "title");
        assert_eq!(next_case("Hello World"), "upper");
        assert_eq!(next_case("HELLO WORLD"), "lower");
        assert_eq!(next_case("Hello world"), "upper");
        assert_eq!(next_case("hELLO wORLD"), "upper");
        assert_eq!(next_case("hello. world is HERE"), "upper");
        assert_eq!(next_case("123"), "upper");
    }

    /// Speaker notes: a bold heading over indented bullet points.
    fn notes() -> TextBody {
        use crate::text::{Bullet, ParaProps};
        let mut b = body("Overview\nFirst point\nSecond point");
        format(&mut b, (0, 0), (0, 8), &|p| p.bold = Some(true));
        for p in b.paragraphs.iter_mut().skip(1) {
            p.level = 1;
            p.props =
                ParaProps { margin_left: Some(36.0), indent: Some(-18.0), bullet: Some(Bullet::Char { char: "•".into() }), ..Default::default() };
        }
        b
    }

    #[test]
    fn replace_text_crlf_matches_from_text() {
        // The kept suffix must not start inside a `\r\n` pair.
        let mut b = TextBody::from_text("x\ny");
        replace_text(&mut b, "z\r\ny");
        assert_eq!(b.text(), "z\ny");
        // CRLF in the kept prefix, and the same text with CRLF changes nothing.
        let before = notes();
        let mut b = before.clone();
        replace_text(&mut b, "Overview\r\nFirst point\r\nSecond point");
        assert_eq!(b, before);
        replace_text(&mut b, "Overview\r\nFirst point\r\nSecond\rpoint");
        assert_eq!(b.text(), "Overview\nFirst point\nSecond\npoint");
        assert_eq!(b.paragraphs[0].runs[0].props.bold, Some(true));
    }

    #[test]
    fn replace_text_keeps_formatting() {
        let before = notes();
        // Same text: nothing changes.
        let mut b = before.clone();
        replace_text(&mut b, "Overview\nFirst point\nSecond point");
        assert_eq!(b, before);
        // A word typed into a bullet: heading stays bold, bullets keep level, indent and bullet.
        replace_text(&mut b, "Overview\nFirst main point\nSecond point");
        assert_eq!(b.text(), "Overview\nFirst main point\nSecond point");
        assert_eq!(b.paragraphs[0].runs[0].props.bold, Some(true));
        assert_eq!(b.paragraphs[1].props, before.paragraphs[1].props);
        assert_eq!((b.paragraphs[1].level, b.paragraphs[2].level), (1, 1));
        // Typing at the end of the heading continues it in bold.
        replace_text(&mut b, "Overview (5 min)\nFirst main point\nSecond point");
        assert_eq!(b.paragraphs[0].runs.len(), 1);
        assert_eq!(b.paragraphs[0].runs[0].props.bold, Some(true));
        // Enter after a bullet starts another bullet; deleting one joins nothing else.
        replace_text(&mut b, "Overview (5 min)\nFirst main point\nThird\nSecond point");
        assert_eq!(b.paragraphs.len(), 4);
        assert_eq!((b.paragraphs[2].level, &b.paragraphs[2].props), (1, &before.paragraphs[1].props));
        replace_text(&mut b, "Overview (5 min)\nSecond point");
        assert_eq!(b.text(), "Overview (5 min)\nSecond point");
        assert_eq!(b.paragraphs[0].runs[0].props.bold, Some(true));
        assert_eq!(b.paragraphs[1].level, 1);
        // Text with nothing in common is new plain text.
        replace_text(&mut b, "Plain");
        assert_eq!(b, body("Plain"));
        replace_text(&mut b, "");
        assert!(b.is_empty());
        // Into an empty body.
        let mut e = TextBody::default();
        replace_text(&mut e, "a\nb");
        assert_eq!(e.text(), "a\nb");
    }
}
