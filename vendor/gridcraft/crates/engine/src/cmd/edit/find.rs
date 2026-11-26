use super::cell::input_to_cell;
use super::{Cell, CellRef, EngineError, Json, Result, Session, Value, bad, bool_param, edit, json, str_param};

// ---------------------------------------------------------------- find & replace

fn matches(hay: &str, what: &str, case: bool, whole: bool) -> bool {
    let (h, w) = if case { (hay.to_string(), what.to_string()) } else { (hay.to_lowercase(), what.to_lowercase()) };
    let has_wild = w.contains(['*', '?']);
    if has_wild {
        let pat = if whole { w } else { format!("*{w}*") };
        return wildcard(&h, &pat);
    }
    if whole { h == w } else { h.contains(&w) }
}

/// `*`/`?` wildcard match with `~` escapes.
///
/// The pattern runs as a bit-parallel automaton over the text: one pass, each character costing
/// one step per 64 pattern positions. Backtracking to the last `*` after a mismatch rescans the
/// text and costs text × pattern comparisons on a long, repetitive cell, for every cell a
/// filter or Find visits.
pub(crate) fn wildcard(text: &str, pat: &str) -> bool {
    // The pattern as (character, escaped) items, runs of `*` folded into one.
    let mut p: Vec<(char, bool)> = Vec::new();
    let mut it = pat.chars();
    while let Some(c) = it.next() {
        if c == '~' {
            if let Some(n) = it.next() {
                p.push((n, true));
            }
        } else if c != '*' || p.last() != Some(&('*', false)) {
            p.push((c, false));
        }
    }
    let words = (p.len() + 1).div_ceil(64);
    let bit = |set: &mut [u64], i: usize| set[i / 64] |= 1 << (i % 64);
    // Positions holding a `*` and a `?`, and where each literal character sits in the pattern.
    let mut stars = vec![0u64; words];
    let mut any = vec![0u64; words];
    let mut at: std::collections::HashMap<char, Vec<usize>> = std::collections::HashMap::new();
    for (i, item) in p.iter().enumerate() {
        match item {
            ('*', false) => bit(&mut stars, i),
            ('?', false) => bit(&mut any, i),
            (c, _) => at.entry(*c).or_default().push(i),
        }
    }
    // A literal's bit mask is built when the text first reads it, and kept for at most
    // `MAX_CACHED_MASKS` literals: a long pattern of mostly distinct characters would otherwise
    // allocate a mask of the pattern's width for each one up front. When there are more literals
    // than that, the ones kept are those at the most pattern positions (the dearest to rebuild);
    // each of the rest has at most pattern length / `MAX_CACHED_MASKS` positions, so rebuilding
    // its mask in `scratch` costs less than one step of the scan.
    const MAX_CACHED_MASKS: usize = 256;
    let cacheable: Option<std::collections::HashSet<char>> = (at.len() > MAX_CACHED_MASKS).then(|| {
        let mut by_count: Vec<(&char, usize)> = at.iter().map(|(c, v)| (c, v.len())).collect();
        by_count.sort_unstable_by_key(|&(_, n)| std::cmp::Reverse(n));
        by_count.into_iter().take(MAX_CACHED_MASKS).map(|(c, _)| *c).collect()
    });
    let mut masks: std::collections::HashMap<char, Vec<u64>> = std::collections::HashMap::new();
    let mut scratch = vec![0u64; words];
    // A set bit i: the text read so far matches the first i pattern items. A `*` matches nothing
    // as well, so a state at a `*` is also at the item after it.
    let close = |set: &mut Vec<u64>| {
        let mut carry = 0u64;
        for w in 0..words {
            let s = set[w] & stars[w];
            let next = (s << 1) | carry;
            carry = s >> 63;
            set[w] |= next;
        }
    };
    let mut state = vec![0u64; words];
    bit(&mut state, 0);
    close(&mut state);
    let mut next = vec![0u64; words];
    for ch in text.chars() {
        let lit: Option<&[u64]> = match at.get(&ch) {
            None => None,
            Some(positions) if cacheable.as_ref().is_none_or(|k| k.contains(&ch)) => {
                let mask = masks.entry(ch).or_insert_with(|| {
                    let mut m = vec![0u64; words];
                    positions.iter().for_each(|&i| bit(&mut m, i));
                    m
                });
                Some(mask)
            }
            Some(positions) => {
                scratch.fill(0);
                positions.iter().for_each(|&i| bit(&mut scratch, i));
                Some(&scratch)
            }
        };
        let mut carry = 0u64;
        let mut alive = 0u64;
        for w in 0..words {
            // Items that read `ch` advance; a `*` reads it and stays.
            let m = state[w] & (any[w] | lit.map_or(0, |l| l[w]));
            next[w] = (m << 1) | carry | (state[w] & stars[w]);
            carry = m >> 63;
            alive |= next[w];
        }
        if alive == 0 {
            return false;
        }
        std::mem::swap(&mut state, &mut next);
        close(&mut state);
    }
    state[p.len() / 64] & (1 << (p.len() % 64)) != 0
}

pub(super) fn find(s: &mut Session, p: &Json) -> Result<Json> {
    let what = str_param(p, "what").unwrap_or("").to_string();
    if what.is_empty() {
        return Err(bad("edit.find", "missing `what`"));
    }
    let case = bool_param(p, "matchCase").unwrap_or(false);
    let whole = bool_param(p, "wholeCell").unwrap_or(false);
    let values = str_param(p, "lookIn") == Some("values");
    let by_cols = bool_param(p, "byColumns").unwrap_or(false);
    let all = bool_param(p, "all").unwrap_or(false);
    let workbook = str_param(p, "within") == Some("workbook");
    // `selection`: only the selected cells of the active sheet, as Excel searches a selected range.
    let in_selection = str_param(p, "within") == Some("selection");
    s.last_find = Some(p.clone());
    let d = s.doc()?;
    let sheets: Vec<usize> = if workbook { (0..d.wb.sheets.len()).collect() } else { vec![d.wb.active_sheet()] };
    let mut hits: Vec<(usize, CellRef, String)> = Vec::new();
    for si in sheets {
        let Some(sh) = d.wb.sheet(si) else { continue };
        let mut cells: Vec<(CellRef, String)> = sh
            .cells
            .iter()
            .filter(|(c, _)| !in_selection || d.selection.contains(*c))
            .filter_map(|(c, cell)| {
                let text = if values || cell.formula.is_none() || crate::display::formula_hidden(&d.wb, sh, c) {
                    crate::display::cell_text(&d.wb, sh, c)
                } else {
                    cell.input_text()
                };
                matches(&text, &what, case, whole).then_some((c, text))
            })
            .collect();
        if by_cols {
            cells.sort_by_key(|(c, _)| (c.col, c.row));
        }
        hits.extend(cells.into_iter().map(|(c, t)| (si, c, t)));
    }
    if hits.is_empty() {
        return Err(EngineError::Other(format!("We couldn't find what you were looking for: {what}")));
    }
    if all {
        let list: Vec<Json> =
            hits.iter().map(|(si, c, t)| json!({"sheet": d.wb.sheet(*si).map(|s| s.name.clone()), "cell": c.a1(), "value": t})).collect();
        return Ok(json!({"count": list.len(), "results": list}));
    }
    // Next hit after the active cell.
    let cur = (d.wb.active_sheet(), d.selection.active);
    let key = |si: usize, c: CellRef| if by_cols { (si, c.col, c.row) } else { (si, c.row, c.col) };
    let next = hits.iter().find(|(si, c, _)| key(*si, *c) > key(cur.0, cur.1)).or_else(|| hits.first()).cloned();
    let Some((si, c, t)) = next else { return Err(EngineError::Other("not found".into())) };
    let dd = s.doc_mut()?;
    dd.wb_switch_sheet(si);
    if in_selection {
        // The range stays selected and the active cell moves through it, as in Excel.
        dd.selection.active = c;
    } else {
        dd.selection = crate::Selection::at(c);
    }
    Ok(json!({"cell": c.a1(), "sheet": si, "value": t, "count": hits.len()}))
}

/// Most characters a cell holds (Excel's limit).
const REPLACE_CELL_MAX_CHARS: usize = 32_767;
/// Longest text one replacement may give a cell, in bytes: the character limit, each up to four
/// bytes. Every occurrence copies the replacement text, so a short cell full of matches and a
/// long replacement would otherwise expand to gigabytes. The character limit is checked on what
/// fits in this.
const REPLACE_CELL_MAX_BYTES: usize = REPLACE_CELL_MAX_CHARS * 4;
/// Most bytes of replaced text one Replace All builds across all its cells.
const REPLACE_TOTAL_MAX_BYTES: usize = 256 * 1024 * 1024;

pub(super) fn replace(s: &mut Session, p: &Json) -> Result<Json> {
    let what = str_param(p, "what").unwrap_or("").to_string();
    let with = str_param(p, "with").unwrap_or("");
    if what.is_empty() {
        return Err(bad("edit.replace", "missing `what`"));
    }
    let case = bool_param(p, "matchCase").unwrap_or(false);
    let whole = bool_param(p, "wholeCell").unwrap_or(false);
    let all = bool_param(p, "all").unwrap_or(true);
    let workbook = str_param(p, "within") == Some("workbook");
    let selection = (str_param(p, "within") == Some("selection")).then(|| s.doc().map(|d| d.selection.clone())).transpose()?;
    let active = s.doc()?.selection.active;
    let mut skipped = 0usize;
    let n = edit(s, |cx| {
        let sheets: Vec<usize> = if workbook { (0..cx.wb.sheets.len()).collect() } else { vec![cx.wb.active_sheet()] };
        let mut n = 0;
        let mut total = 0usize;
        for si in sheets {
            let cells: Vec<(CellRef, String)> =
                // A hidden formula's source is not searched: a count or error would reveal its text.
                cx.wb
                    .sheet(si)
                    .map(|sh| sh.cells.iter().filter(|(c, cell)| cell.formula.is_none() && cell.py.is_none() || !crate::display::formula_hidden(&cx.wb, sh, *c)).map(|(c, _)| (c, sh.input_text(c))).collect())
                    .unwrap_or_default();
            for (c, text) in cells {
                if (!all && c != active) || selection.as_ref().is_some_and(|sel| !sel.contains(c)) {
                    continue;
                }
                if !matches(&text, &what, case, whole) {
                    continue;
                }
                let new = if whole {
                    (with.len() <= REPLACE_CELL_MAX_BYTES).then(|| with.to_string())
                } else if case {
                    replace_cs_within(&text, &what, with, REPLACE_CELL_MAX_BYTES)
                } else {
                    replace_ci_within(&text, &what, with, REPLACE_CELL_MAX_BYTES)
                };
                let new = new.filter(|n| n.chars().count() <= REPLACE_CELL_MAX_CHARS);
                let Some(new) = new else {
                    return Err(EngineError::Other(format!("The replacement makes the text of {} too long.", c.a1())));
                };
                total = total.saturating_add(new.len());
                if total > REPLACE_TOTAL_MAX_BYTES {
                    return Err(EngineError::Other("The replacements make too much text. Replace in a smaller range.".into()));
                }
                // A Python cell's text is its code: the replacement edits the code and the cell
                // stays a Python cell. Code that would no longer fit in a PY formula is left as it was.
                let old_py = cx.wb.sheet(si).and_then(|sh| sh.cell(c)).and_then(|x| x.py.clone());
                if let Some(old) = old_py {
                    let py = gridcraft_model::PyCode { object: old.object, ..gridcraft_model::PyCode::new(&new) };
                    if py.code == old.code {
                        continue;
                    }
                    if !py.fits() {
                        skipped += 1;
                        continue;
                    }
                    // Its old spill is left to the recalculation, which removes it and
                    // recalculates what read it.
                    let sh = cx.sheet_mut(si)?;
                    let style = sh.style_id(c);
                    sh.set_cell(
                        c,
                        Cell { value: Value::Error(gridcraft_core::CellError::Blocked), formula: None, style, py: Some(std::sync::Arc::new(py)) },
                    );
                    cx.touch(si, c);
                    n += 1;
                    continue;
                }
                let cell = input_to_cell(&new, si, c, &mut cx.wb).unwrap_or_else(|_| Some(Cell::value(Value::text(new.as_str()))));
                let sh = cx.sheet_mut(si)?;
                match cell {
                    Some(cell) => sh.set_cell(c, cell),
                    None => {
                        sh.remove_cell(c);
                    }
                }
                cx.touch(si, c);
                n += 1;
            }
        }
        Ok(n)
    })?;
    if skipped > 0 {
        return Ok(json!({"replaced": n, "skipped": skipped}));
    }
    Ok(json!({"replaced": n}))
}

/// Whether case-folding `s` maps every char to exactly one char of the same UTF-8 length, so
/// byte positions in the folded text still line up with the original's character boundaries.
fn folds_in_place(s: &str) -> bool {
    s.chars().all(|c| {
        let mut folded = c.to_lowercase();
        let first = folded.next();
        folded.next().is_none() && first.is_some_and(|l| l.len_utf8() == c.len_utf8())
    })
}

#[cfg(test)]
pub(super) fn replace_ci(text: &str, what: &str, with: &str) -> String {
    replace_ci_within(text, what, with, usize::MAX).unwrap_or_default()
}

/// `text` with every `what` replaced by `with`, or `None` once the result passes `limit` bytes
/// (checked before each copy, so nothing larger is allocated).
pub(super) fn replace_cs_within(text: &str, what: &str, with: &str, limit: usize) -> Option<String> {
    if what.is_empty() {
        return (text.len() <= limit).then(|| text.to_string());
    }
    let mut out = String::new();
    let mut i = 0;
    for (pos, m) in text.match_indices(what) {
        if out.len() + (pos - i) + with.len() > limit {
            return None;
        }
        out.push_str(text.get(i..pos).unwrap_or(""));
        out.push_str(with);
        i = pos + m.len();
    }
    let rest = text.get(i..).unwrap_or("");
    if out.len() + rest.len() > limit {
        return None;
    }
    out.push_str(rest);
    Some(out)
}

/// Case-insensitive [`replace_cs_within`].
pub(super) fn replace_ci_within(text: &str, what: &str, with: &str, limit: usize) -> Option<String> {
    let lower = text.to_lowercase();
    let w = what.to_lowercase();
    // Slicing `text` at positions found in `lower` is only sound when folding kept every byte
    // boundary: a length check alone misses foldings that keep the total length while shifting
    // boundaries (U+212A KELVIN SIGN shrinks 3→1 bytes, U+0130 grows 2→3), where the tail used
    // to be dropped. Those fall back to the case-sensitive replace, as length changes already do.
    if lower.len() != text.len() || w.is_empty() || !folds_in_place(text) || !folds_in_place(what) {
        return replace_cs_within(text, what, with, limit);
    }
    let mut out = String::new();
    let mut i = 0;
    while let Some(pos) = lower.get(i..).and_then(|s| s.find(&w)) {
        if out.len() + pos + with.len() > limit {
            return None;
        }
        out.push_str(text.get(i..i + pos).unwrap_or(""));
        out.push_str(with);
        i += pos + w.len();
    }
    let rest = text.get(i..).unwrap_or("");
    if out.len() + rest.len() > limit {
        return None;
    }
    out.push_str(rest);
    Some(out)
}
