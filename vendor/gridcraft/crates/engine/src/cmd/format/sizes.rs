use super::{
    CellRef, EngineError, Json, RangeRef, Result, Session, StyleId, bad, edit, f64_param, parse_lines, split_sheet, str_param, target_sheet,
};

// ---------------------------------------------------------------- sizes

/// The sheet and the row (or column) spans a size or visibility command works on. The lines are
/// `rows`/`cols` (`"2:5"`, `"B:B,D:F"`, optionally `Sheet!`-prefixed), else the selection's; the
/// sheet is `sheet`, else the prefix, else the active one. The selection belongs to the active
/// sheet, so another sheet needs explicit lines.
fn line_targets(s: &Session, p: &Json, rows: bool) -> Result<(usize, Vec<(u32, u32)>)> {
    let (sheet, lines) = resolve_lines(s, p, rows)?;
    // Sizes and visibility are formatting of the line, which a protected sheet must allow.
    if rows {
        crate::cmd::refuse_unless_allowed(s, sheet, "Formatting rows", |p| p.format_rows)?;
    } else {
        crate::cmd::refuse_unless_allowed(s, sheet, "Formatting columns", |p| p.format_columns)?;
    }
    Ok((sheet, lines))
}

fn resolve_lines(s: &Session, p: &Json, rows: bool) -> Result<(usize, Vec<(u32, u32)>)> {
    let key = if rows { "rows" } else { "cols" };
    let mut sheet = target_sheet(s, p)?;
    if let Some(t) = str_param(p, key) {
        let (prefix, body) = split_sheet(t);
        if let Some(name) = prefix {
            let named = s.doc()?.wb.sheet_index(&name).ok_or_else(|| bad(key, format!("no sheet named `{name}`")))?;
            if p.get("sheet").is_some_and(|v| !v.is_null()) && named != sheet {
                return Err(bad(key, format!("`{t}` names a sheet other than `sheet`")));
            }
            sheet = named;
        }
        // A comma-separated list ("B:B,D:F") targets several areas, like a multi-area selection.
        let lines = body
            .split(',')
            .map(|part| {
                let r = parse_lines(part).ok_or_else(|| bad(key, format!("not a range: {part}")))?;
                Ok(if rows { (r.start.row, r.end.row) } else { (r.start.col, r.end.col) })
            })
            .collect::<Result<Vec<_>>>()?;
        return Ok((sheet, lines));
    }
    let d = s.doc()?;
    if sheet != d.wb.active_sheet() {
        return Err(bad(key, format!("missing `{key}`: the selection is on the active sheet")));
    }
    Ok((sheet, d.selection.ranges.iter().map(|r| if rows { (r.start.row, r.end.row) } else { (r.start.col, r.end.col) }).collect()))
}

/// Sizes are pixels at 96 dpi, the model's unit (header drags send pixels). `points` (row height)
/// and `chars` (column width) take Excel's units, those of its Row Height and Column Width dialogs
/// and of the file (`ht` is in points).
pub(super) fn row_height(s: &mut Session, p: &Json) -> Result<Json> {
    let h = match (f64_param(p, "height"), f64_param(p, "points")) {
        (Some(h), _) => {
            if !(0.0..=546.0).contains(&h) {
                return Err(EngineError::Other("Row height must be between 0 and 546 pixels (409.5 points).".into()));
            }
            h
        }
        (None, Some(pt)) => {
            if !(0.0..=409.5).contains(&pt) {
                return Err(EngineError::Other("Row height must be between 0 and 409.5 points.".into()));
            }
            pt * 4.0 / 3.0
        }
        _ => return Err(bad("home.rowHeight", "missing `height` (pixels) or `points`")),
    };
    let (si, lines) = line_targets(s, p, true)?;
    edit(s, |cx| {
        let sh = cx.sheet_mut(si)?;
        for (a, b) in lines {
            for r in a..=b.min(a.saturating_add(1_048_575)) {
                let e = sh.rows.entry(r).or_default();
                e.size = Some(h as f32);
                e.hidden = h == 0.0;
                e.custom = true;
            }
        }
        cx.structural = true; // setting a height shows or hides rows: SUBTOTAL(10x) depends on it
        Ok(Json::Null)
    })
}

pub(super) fn col_width(s: &mut Session, p: &Json) -> Result<Json> {
    let w = match (f64_param(p, "width"), f64_param(p, "chars")) {
        (Some(w), _) => {
            if !(0.0..=1800.0).contains(&w) {
                return Err(EngineError::Other("Column width must be between 0 and 1800 pixels.".into()));
            }
            w
        }
        (None, Some(c)) => {
            if !(0.0..=255.0).contains(&c) {
                return Err(EngineError::Other("Column width must be between 0 and 255 characters.".into()));
            }
            chars_to_points(c)
        }
        _ => return Err(bad("home.columnWidth", "missing `width` (pixels) or `chars`")),
    };
    let (si, lines) = line_targets(s, p, false)?;
    edit(s, |cx| {
        let sh = cx.sheet_mut(si)?;
        for (a, b) in lines {
            for c in a..=b {
                let e = sh.cols.entry(c).or_default();
                e.size = Some(w as f32);
                e.hidden = w == 0.0;
            }
        }
        Ok(Json::Null)
    })
}

/// Excel column width in characters (of the default font's digit width, 7 px) → pixels.
pub fn chars_to_points(c: f64) -> f64 {
    if c <= 0.0 {
        0.0
    } else if c < 1.0 {
        (c * 12.0).round()
    } else {
        (c * 7.0 + 5.0).round()
    }
}

/// Approximate text width in points for autofit (no font metrics in the engine: average glyph
/// widths for the default font; a front end with fonts measures instead, see `Session::text_width`).
pub fn approx_text_width(text: &str, size: f32, bold: bool) -> f32 {
    let mut w = 0.0;
    for c in text.chars() {
        w += match c {
            'i' | 'l' | 'j' | '\'' | '.' | ',' | ':' | ';' | '!' | '|' => 0.28,
            'f' | 't' | 'r' | 'I' | ' ' | '(' | ')' | '-' => 0.38,
            'm' | 'w' | 'M' | 'W' => 0.85,
            c if c.is_ascii_uppercase() => 0.65,
            c if c.is_ascii_digit() => 0.56,
            c if c.is_ascii() => 0.52,
            c if is_zero_width(c) => 0.0,
            c => arabic_em(c).unwrap_or(1.0),
        };
    }
    w * size * if bold { 1.07 } else { 1.0 } * 96.0 / 72.0
}

/// Arabic vowel and Quranic marks, which sit on the previous letter, and invisible format
/// characters such as direction marks and zero-width joiners.
fn is_zero_width(c: char) -> bool {
    matches!(
        c,
        '\u{0610}'..='\u{061A}'
            | '\u{061C}'
            | '\u{064B}'..='\u{065F}'
            | '\u{0670}'
            | '\u{06D6}'..='\u{06DC}'
            | '\u{06DF}'..='\u{06E4}'
            | '\u{06E7}'..='\u{06E8}'
            | '\u{06EA}'..='\u{06ED}'
            | '\u{0897}'..='\u{089F}'
            | '\u{08CA}'..='\u{08E1}'
            | '\u{08E3}'..='\u{08FF}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2069}'
            | '\u{FE20}'..='\u{FE2F}'
            | '\u{FEFF}'
    )
}

/// Width in ems of an Arabic-script character, measured from shaped Noto Sans Arabic, Noto Naskh
/// Arabic and DejaVu Sans: joined letters average about half an em, word ligatures such as ﷺ and the
/// rial sign about 1.2 em, the honorific ligatures (U+FD40–FD4B) about 1.6 em, and the bismillah
/// ligature about 11 em. The later honorific ligatures (U+FBC3–FBD2, U+FD4C–FD4F, U+FD90–FD91,
/// U+FDC8–FDCF) are not measured and get the 1.6 em of their siblings rather than the letter width.
/// Arabic-Indic digits are as wide as ASCII digits (0.53–0.57 em in Noto Sans Arabic, Arial, Tahoma
/// and Times New Roman); Arabic punctuation (0.2–0.5 em in the same fonts) keeps the letter width.
fn arabic_em(c: char) -> Option<f32> {
    match c {
        '\u{FDFD}' => Some(11.0),
        '\u{FBC3}'..='\u{FBD2}' | '\u{FD40}'..='\u{FD4F}' | '\u{FD90}'..='\u{FD91}' | '\u{FDC8}'..='\u{FDCF}' => Some(1.6),
        '\u{FDF0}'..='\u{FDFC}' | '\u{FDFE}'..='\u{FDFF}' => Some(1.2),
        '\u{0660}'..='\u{0669}' | '\u{06F0}'..='\u{06F9}' => Some(0.56),
        '\u{0600}'..='\u{06FF}' | '\u{0750}'..='\u{077F}' | '\u{0870}'..='\u{08FF}' | '\u{FB50}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFC}' => Some(0.48),
        _ => None,
    }
}

/// Distinct texts AutoFit measures exactly; past this many, `Session::text_width` is asked for a
/// quick measure so a huge column doesn't lay out every value.
const EXACT_TEXT_WIDTHS: usize = 2_000;

pub(super) fn autofit_cols(s: &mut Session, p: &Json) -> Result<Json> {
    let (si, lines) = line_targets(s, p, false)?;
    let measure = s.text_width.clone();
    edit(s, |cx| {
        let Some(sh) = cx.wb.sheet(si) else { return Ok(Json::Null) };
        let mut measured: std::collections::HashMap<StyleId, std::collections::HashMap<String, f32>> = Default::default();
        let mut distinct = 0usize;
        // Selected columns, then one pass over the populated cells (the store is row-oriented, so a
        // scan per column would revisit every row for each selected column, empty or not).
        let cols: std::collections::BTreeSet<u32> = lines.iter().flat_map(|&(a, b)| a..=b.min(a + 16383)).collect();
        let mut maxes: std::collections::BTreeMap<u32, f32> = Default::default();
        if let Some(u) = sh.used_range() {
            // One pass per run of adjacent selected columns, so columns between distant selections
            // (A and XFD) are never visited.
            let mut runs: Vec<(u32, u32)> = Vec::new();
            for &c in &cols {
                match runs.last_mut() {
                    Some((_, end)) if *end + 1 == c => *end = c,
                    _ => runs.push((c, c)),
                }
            }
            // Every run is a pass over all the stored rows: many runs (alternating columns) are one
            // pass over their whole span instead, the unselected columns skipped.
            if runs.len() > 4
                && let (Some(&(first, _)), Some(&(_, last))) = (runs.first(), runs.last())
            {
                runs = vec![(first, last)];
            }
            for (first, last) in runs {
                let span = RangeRef::new(CellRef::new(u.start.row, first), CellRef::new(u.end.row, last));
                for (cell, _) in sh.cells.iter_range(span) {
                    if !cols.contains(&cell.col) {
                        continue;
                    }
                    if sh.merge_at(cell).is_some_and(|m| !m.is_single()) {
                        continue;
                    }
                    let id = sh.style_id(cell);
                    let st = cx.wb.styles.get(id);
                    let text = crate::display::cell_text(&cx.wb, sh, cell);
                    for line in text.lines() {
                        let w = match &measure {
                            Some(m) => {
                                let known = measured.entry(id).or_default();
                                match known.get(line) {
                                    Some(w) => *w,
                                    None => {
                                        let w = m(line, &st.font, distinct < EXACT_TEXT_WIDTHS);
                                        known.insert(line.to_owned(), w);
                                        distinct += 1;
                                        w
                                    }
                                }
                            }
                            None => approx_text_width(line, st.font.size, st.font.bold),
                        };
                        let w = w + 6.0 + st.align.indent as f32 * 9.0 + sh.filter_button_space(cell);
                        let max = maxes.entry(cell.col).or_insert(0.0);
                        *max = max.max(w);
                    }
                }
            }
        }
        let widths: Vec<(u32, f32)> = maxes.into_iter().filter(|&(_, m)| m > 0.0).map(|(c, m)| (c, m.clamp(8.0, 1785.0))).collect();
        let sh = cx.sheet_mut(si)?;
        for (c, w) in widths {
            let e = sh.cols.entry(c).or_default();
            e.size = Some(w.ceil());
            e.hidden = false;
        }
        Ok(Json::Null)
    })
}

pub(super) fn autofit_rows(s: &mut Session, p: &Json) -> Result<Json> {
    let (si, lines) = line_targets(s, p, true)?;
    edit(s, |cx| {
        for (a, b) in lines {
            let b = b.min(a + 100_000);
            if let Some(sh) = cx.wb.sheet_mut(si) {
                for r in a..=b {
                    if let Some(e) = sh.rows.get_mut(&r) {
                        e.custom = false;
                        e.hidden = false;
                    }
                }
            }
            cx.fit_rows.extend((a..=b).map(|r| (si, r)));
        }
        cx.structural = true; // autofit reveals hidden rows: SUBTOTAL(10x) depends on it
        Ok(Json::Null)
    })
}

pub(super) fn default_width(s: &mut Session, p: &Json) -> Result<Json> {
    let w = f64_param(p, "width").ok_or_else(|| bad("home.defaultWidth", "missing `width`"))?;
    let si = target_sheet(s, p)?;
    edit(s, |cx| {
        cx.sheet_mut(si)?.default_col_width = w.clamp(0.0, 1800.0) as f32;
        Ok(Json::Null)
    })
}

pub(super) fn hide(s: &mut Session, p: &Json, rows: bool, on: bool) -> Result<Json> {
    let (si, lines) = line_targets(s, p, rows)?;
    edit(s, |cx| {
        let sh = cx.sheet_mut(si)?;
        for (a, b) in lines {
            // Unhiding a single selected line also reveals hidden neighbours (Excel needs the
            // lines on both sides selected; we're forgiving).
            let (a, b) = if !on && a == b { (a.saturating_sub(1), b + 1) } else { (a, b) };
            for i in a..=b.min(a.saturating_add(1_048_575)) {
                let map = if rows { &mut sh.rows } else { &mut sh.cols };
                if on {
                    map.entry(i).or_default().hidden = true;
                } else if let Some(e) = map.get_mut(&i) {
                    e.hidden = false;
                    if e.size == Some(0.0) {
                        e.size = None;
                    }
                }
            }
        }
        cx.structural = true; // SUBTOTAL(10x) depends on hidden rows
        Ok(Json::Null)
    })
}
