use super::draw::{Block, Look, rgb_of};
use super::{
    Baseline, CellRef, Color, Font, HAlign, MAX_COLS, Page, Point, RangeRef, ReadingOrder, Sheet, Style, StyleTable, TextDirection, Underline,
    VAlign, Value,
};
use unicode_segmentation::UnicodeSegmentation;

const PAD_PX: f32 = 2.0;

/// Most lines [`wrap_lines`] returns (it may return one more, the rest of a paragraph).
const MAX_WRAP_LINES: usize = 2000;

pub(super) struct TextLine {
    pub(super) text: String,
    pub(super) direction: TextDirection,
}

/// Word-wraps `text` to `width` points, keeping each paragraph's base direction on every
/// continuation line.
///
/// Widths are kept as running sums of glyph widths (the sum `text_width` makes), so each
/// character is measured once: re-measuring the line for every word, or the rest of an
/// over-long word for every piece cut from it, made a long paragraph or word quadratic.
/// A paragraph whose width is not that sum is measured as drawn by [`wrap_measured_paragraph`].
pub(super) fn wrap_lines(text: &str, font: Font, size: f32, width: f32, direction: TextDirection) -> Vec<TextLine> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let direction = direction.resolve(para);
        if gridcraft_pdf::width_is_char_sum(font, para, direction) {
            wrap_summed_paragraph(para, font, size, width, direction, &mut out);
        } else {
            let fits = |s: &str| gridcraft_pdf::text_width_with_direction(font, size, s, direction) <= width;
            wrap_measured_paragraph(para, &fits, direction, &mut out);
        }
        if out.len() > MAX_WRAP_LINES {
            return out;
        }
    }
    out
}

fn wrap_summed_paragraph(para: &str, font: Font, size: f32, width: f32, direction: TextDirection, out: &mut Vec<TextLine>) {
    let units_of = |s: &str| -> u64 { s.chars().map(|c| u64::from(font.char_width(c))).sum() };
    let fits = |units: u64| units as f32 * size / 1000.0 <= width;
    let space = units_of(" ");
    let mut line = String::new();
    let mut line_units: u64 = 0;
    for word in para.split(' ') {
        let word_units = units_of(word);
        if line.is_empty() {
            line.push_str(word);
            line_units = word_units;
        } else if fits(line_units + space + word_units) {
            line.push(' ');
            line.push_str(word);
            line_units += space + word_units;
        } else {
            out.push(TextLine { text: std::mem::take(&mut line), direction });
            line.push_str(word);
            line_units = word_units;
        }
        if !fits(line_units) && line.graphemes(true).nth(1).is_some() {
            let mut piece = String::new();
            let mut units: u64 = 0;
            for g in line.graphemes(true) {
                let w = units_of(g);
                if !piece.is_empty() && !fits(units + w) {
                    out.push(TextLine { text: std::mem::take(&mut piece), direction });
                    units = 0;
                    if out.len() > MAX_WRAP_LINES {
                        return;
                    }
                }
                piece.push_str(g);
                units += w;
            }
            line = piece;
            line_units = units;
        }
        if out.len() > MAX_WRAP_LINES {
            return;
        }
    }
    out.push(TextLine { text: line, direction });
}

/// One paragraph of [`wrap_lines`] whose width is not a sum of character widths (shaped Arabic,
/// right-to-left reordering or bidi controls), measuring each candidate line as drawn.
fn wrap_measured_paragraph(para: &str, fits: &impl Fn(&str) -> bool, direction: TextDirection, out: &mut Vec<TextLine>) {
    let mut line = String::new();
    for word in para.split(' ') {
        if !line.is_empty() {
            let candidate = format!("{line} {word}");
            if fits(&candidate) {
                line = candidate;
                continue;
            }
            out.push(TextLine { text: std::mem::take(&mut line), direction });
        }
        line = if fits(word) { word.to_string() } else { break_measured_word(word, fits, direction, out) };
        if out.len() > MAX_WRAP_LINES {
            return;
        }
    }
    out.push(TextLine { text: line, direction });
}

/// Cuts `word` into the longest grapheme prefixes that fit (at least one grapheme each), pushes
/// all but the last to `out` and returns the last. Each cut gallops from one grapheme, so it
/// measures prefixes about as long as the piece rather than the rest of the word.
fn break_measured_word(word: &str, fits: &impl Fn(&str) -> bool, direction: TextDirection, out: &mut Vec<TextLine>) -> String {
    let ends: Vec<usize> = word.grapheme_indices(true).map(|(i, g)| i + g.len()).collect();
    let (mut start, mut first) = (0, 0);
    loop {
        let len = ends.len().saturating_sub(first);
        let end = |count: usize| ends.get(first + count.saturating_sub(1)).copied().unwrap_or(word.len());
        let fits_prefix = |count: usize| fits(word.get(start..end(count)).unwrap_or_default());
        let (mut fit, mut over, mut probe) = (1, len + 1, 2);
        while fit < len {
            let count = probe.min(len);
            if fits_prefix(count) {
                fit = count;
                probe = count.saturating_mul(2);
            } else {
                over = count;
                break;
            }
        }
        if fit >= len {
            return word.get(start..).unwrap_or_default().to_string();
        }
        while over - fit > 1 {
            let mid = fit + (over - fit) / 2;
            if fits_prefix(mid) {
                fit = mid;
            } else {
                over = mid;
            }
        }
        let cut = end(fit);
        out.push(TextLine { text: word.get(start..cut).unwrap_or_default().to_string(), direction });
        start = cut;
        first += fit;
        if out.len() > MAX_WRAP_LINES {
            return word.get(start..).unwrap_or_default().to_string();
        }
    }
}

fn font_for(style: &Style) -> Font {
    Font::styled(Font::family_for(&style.font.name), style.font.bold, style.font.italic)
}

fn text_direction(order: ReadingOrder) -> TextDirection {
    match order {
        ReadingOrder::Context => TextDirection::Context,
        ReadingOrder::LeftToRight => TextDirection::LeftToRight,
        ReadingOrder::RightToLeft => TextDirection::RightToLeft,
    }
}

/// The alignment a cell is laid out with before a right-to-left sheet is mirrored. Explicit Left
/// and Right are swapped there, as in the grid, so they print on the side the user chose.
fn text_align(align: HAlign, value: &Value, direction: TextDirection, right_to_left_sheet: bool) -> HAlign {
    match align {
        HAlign::General => match value {
            Value::Number(_) => HAlign::Right,
            Value::Bool(_) | Value::Error(_) => HAlign::Center,
            _ if direction == TextDirection::RightToLeft && !right_to_left_sheet => HAlign::Right,
            _ => HAlign::Left,
        },
        HAlign::Left if right_to_left_sheet => HAlign::Right,
        HAlign::Right if right_to_left_sheet => HAlign::Left,
        HAlign::CenterAcross | HAlign::Distributed => HAlign::Center,
        HAlign::Justify | HAlign::Fill => HAlign::Left,
        other => other,
    }
}

/// Is a cell free for text to overflow into?
fn empty_for_overflow(sh: &Sheet, c: CellRef, merges: &[RangeRef]) -> bool {
    sh.value_ref(c).is_none_or(|v| v.is_empty()) && !merges.iter().any(|m| m.contains(c))
}

fn extend_text_bounds(sh: &Sheet, c: CellRef, merges: &[RangeRef], k: f32, need: f32, align: HAlign, bounds: (f32, f32)) -> (f32, f32) {
    let (mut clip_l, mut clip_r) = bounds;
    let grow = |dir: i64, mut remaining: f32| -> f32 {
        let mut ext = 0.0;
        let mut col = c.col as i64 + dir;
        while remaining > 0.0 && col >= 0 && col < MAX_COLS as i64 {
            let nc = CellRef::new(c.row, col as u32);
            if !empty_for_overflow(sh, nc, merges) {
                break;
            }
            let width = sh.col_width(nc.col) * k;
            ext += width;
            remaining -= width;
            col += dir;
            if ext > 20_000.0 {
                break;
            }
        }
        ext
    };
    match align {
        HAlign::Right => clip_l -= grow(-1, need),
        HAlign::Center => {
            clip_l -= grow(-1, need / 2.0);
            clip_r += grow(1, need / 2.0);
        }
        _ => clip_r += grow(1, need),
    }
    (clip_l, clip_r)
}

/// Unscaled width of the empty Center Across Selection cells right of `c`, which its text is centred over too.
pub(super) fn center_across_width(styles: &StyleTable, sh: &Sheet, c: CellRef, merges: &[RangeRef]) -> f32 {
    let mut ext = 0.0;
    let mut col = c.col + 1;
    while col < MAX_COLS && ext < 20_000.0 {
        let nc = CellRef::new(c.row, col);
        if !empty_for_overflow(sh, nc, merges) || styles.get(sh.style_id(nc)).align.h != HAlign::CenterAcross {
            break;
        }
        ext += sh.col_width(col);
        col += 1;
    }
    ext
}

pub(super) struct TextPlacement<'a> {
    pub cell: CellRef,
    pub look: &'a Look,
    pub rect: (f32, f32, f32, f32),
    pub merged: bool,
    pub merges: &'a [RangeRef],
    pub limits: (f32, f32),
}

pub(super) fn draw_text(pg: &mut Page, b: &Block, placement: TextPlacement<'_>) {
    let TextPlacement { cell: c, look: l, rect, merged, merges, limits } = placement;
    let (x, y, w, h) = rect;
    let sh = b.sh;
    let wb = b.wb;
    let v = sh.value(c);
    if v.is_empty() || l.cf.as_ref().is_some_and(|cf| cf.hide_value) {
        return;
    }
    if !sh.show_zeros && v.as_f64() == Some(0.0) {
        return;
    }
    let st = &l.style;
    let mut size = st.font.size.clamp(1.0, 409.0) * b.scale;
    let font = font_for(st);
    let pad = PAD_PX * b.k;
    let indent = st.align.indent as f32 * 9.0 * b.k;
    let avail = (w - 2.0 * pad - indent).max(0.0);
    let code = st.num_fmt.as_str();
    let formatted = crate::display::format(&v, code, wb);
    let mut color = formatted.color.and_then(gridcraft_numfmt::FormatColor::rgb).or_else(|| rgb_of(st.font.color, wb)).unwrap_or([0, 0, 0]);
    if l.cf.as_ref().and_then(|x| x.style.as_ref()).is_some_and(|s| s.font.color != Color::Auto) {
        color = rgb_of(st.font.color, wb).unwrap_or(color);
    }
    let is_num = matches!(v, Value::Number(_));
    let wrap = st.align.wrap;
    let mut text = if sh.show_formulas {
        crate::display::cell_text(wb, sh, c)
    } else if code == "General"
        && let Value::Number(n) = v
    {
        let digit = gridcraft_pdf::text_width(font, size, "0").max(0.1);
        let max_chars = (avail / digit).floor().max(1.0) as usize;
        gridcraft_numfmt::format_general_fit(n, max_chars.min(11)).unwrap_or_else(|| "#".repeat(max_chars.min(255)))
    } else {
        // The look's code, which a conditional format can override (#1163).
        crate::display::value_text_as(wb, &v, code)
    };
    if !wrap {
        text = text.replace(['\n', '\r'], " ");
    }
    if text.is_empty() {
        return;
    }
    let direction = text_direction(st.align.reading_order.unwrap_or_default()).resolve(&text);
    let mut tw = gridcraft_pdf::text_width_with_direction(font, size, &text, direction);
    if st.align.shrink && !wrap && tw > avail && tw > 0.0 {
        size = (size * avail / tw).max(1.0);
        tw = gridcraft_pdf::text_width_with_direction(font, size, &text, direction);
    }
    if is_num && !wrap && tw > avail + 0.01 && !sh.show_formulas {
        let hash = gridcraft_pdf::text_width(font, size, "#").max(0.1);
        text = "#".repeat(((avail / hash).floor().max(1.0) as usize).min(255));
        tw = gridcraft_pdf::text_width_with_direction(font, size, &text, direction);
    }
    let h_align = text_align(st.align.h, &v, direction, sh.right_to_left);
    let rot = st.align.rotation;
    let rotated = rot != 0 && rot != 255;
    // Rotated text wraps along its own direction: upright (±90°) text to the row height.
    let lines: Vec<TextLine> = match (wrap, rotated) {
        (true, false) => wrap_lines(&text, font, size, avail.max(size), direction),
        (true, true) if rot.abs() == 90 => wrap_lines(&text, font, size, (h - 2.0 * pad).max(size), direction),
        (true, true) => vec![TextLine { text: text.lines().next().unwrap_or_default().to_string(), direction }],
        (false, _) => vec![TextLine { text, direction }],
    };
    let extents = lines
        .iter()
        .filter_map(|line| pg.text_vertical_extents_with_direction(font, size, &line.text, line.direction))
        .reduce(|a, b| (a.0.max(b.0), a.1.max(b.1)))
        .map(|(ascent, descent)| (ascent.max(size * 0.8), descent.max(size * 0.22)));
    let line_h = extents.map_or(size * 1.2, |(ascent, descent)| (ascent + descent + size * 0.2).max(size * 1.2));
    let n = lines.len().max(1) as f32;
    let first_baseline = if let Some((ascent, descent)) = extents {
        match st.align.v {
            VAlign::Top => y + pad + ascent,
            VAlign::Center | VAlign::Justify | VAlign::Distributed => y + (h - (n - 1.0) * line_h - ascent - descent) / 2.0 + ascent,
            VAlign::Bottom => y + h - pad - descent - (n - 1.0) * line_h,
        }
    } else {
        match st.align.v {
            VAlign::Top => y + pad + size * 0.8,
            VAlign::Center | VAlign::Justify | VAlign::Distributed => y + (h - n * line_h) / 2.0 + line_h * 0.5 + size * 0.33,
            VAlign::Bottom => y + h - pad - size * 0.22 - (n - 1.0) * line_h,
        }
    };
    // Overflow into empty neighbours (single-line text, not merged).
    // Center Across Selection spans the empty Center Across cells to its right; the text stays within that run.
    let spans = st.align.h == HAlign::CenterAcross && !wrap && !merged && rot == 0;
    let across = if spans { center_across_width(&wb.styles, sh, c, merges) * b.k } else { 0.0 };
    let (mut clip_l, mut clip_r) = (x, x + w + across);
    if !wrap && !merged && !is_num && rot.abs() != 90 && across <= 0.0 && tw + 2.0 * pad + indent > w {
        let need = tw + 2.0 * pad + indent - w;
        (clip_l, clip_r) = extend_text_bounds(sh, c, merges, b.k, need, h_align, (clip_l, clip_r));
    }
    let clip_l = clip_l.max(limits.0);
    let clip_r = clip_r.min(limits.1);
    pg.save();
    pg.clip_rect(b.text_x(clip_l, clip_r - clip_l), y, (clip_r - clip_l).max(0.0), h);
    if rotated {
        // Rotated text: anchored at the bottom-left (upward) or top-left (downward) of the cell.
        // Wrapped lines stack rightward for upward text and leftward for downward text, so
        // downward text starts its first line far enough in for the last to end at the left pad.
        let deg = (rot as f32).clamp(-90.0, 90.0);
        let angle = -deg.to_radians();
        let last = lines.len().saturating_sub(1) as f32;
        for (i, t) in lines.iter().enumerate() {
            let i = i as f32;
            let (ox, oy) = if deg > 0.0 {
                (x + pad + extents.map_or(size * 0.8, |e| e.0) * deg.to_radians().sin() + i * line_h, y + h - pad)
            } else {
                (x + pad + (last - i) * line_h, y + pad + extents.map_or(size * 0.8, |e| e.0))
            };
            let tw = gridcraft_pdf::text_width_with_direction(font, size, &t.text, t.direction);
            let (sn, cs) = angle.sin_cos();
            let (ascent, descent) = extents.unwrap_or((size * 0.8, size * 0.22));
            let ox = if sh.right_to_left { b.mirror_axis - ox - tw * cs - (ascent - descent) * sn } else { ox };
            pg.text_rotated_with_direction(Baseline { origin: Point::new(ox, oy), angle }, size, font, color, &t.text, t.direction);
        }
        pg.restore();
        return;
    }
    if rot == 255 {
        // Vertical stacked text.
        let stacked: String = lines.iter().map(|line| line.text.as_str()).collect();
        let chars: Vec<&str> = stacked.graphemes(true).take(500).collect();
        let mut yy = y + pad + extents.map_or(size * 0.8, |e| e.0);
        for ch in chars {
            let cw = gridcraft_pdf::text_width(font, size, ch);
            pg.text_with_direction(Point::new(b.text_x(x + (w - cw) / 2.0, cw), yy), size, font, color, ch, direction);
            yy += line_h;
            if yy > y + h + line_h {
                break;
            }
        }
        pg.restore();
        return;
    }
    for (i, line) in lines.iter().enumerate() {
        let lw = gridcraft_pdf::text_width_with_direction(font, size, &line.text, line.direction);
        let lx = match h_align {
            HAlign::Right => x + w - pad - lw - indent,
            HAlign::Center => x + (w + across - lw) / 2.0,
            _ => x + pad + indent,
        };
        let ly = first_baseline + i as f32 * line_h;
        if ly - size > y + h || ly < y - line_h {
            continue;
        }
        let lx = b.text_x(lx, lw);
        pg.text_with_direction(Point::new(lx, ly), size, font, color, &line.text, line.direction);
        if st.font.underline != Underline::None {
            let uw = (size * 0.06).max(0.2);
            pg.stroke_line(lx, ly + size * 0.12, lx + lw, ly + size * 0.12, uw, color);
            if matches!(st.font.underline, Underline::Double | Underline::DoubleAccounting) {
                pg.stroke_line(lx, ly + size * 0.24, lx + lw, ly + size * 0.24, uw, color);
            }
        }
        if st.font.strike {
            pg.stroke_line(lx, ly - size * 0.3, lx + lw, ly - size * 0.3, (size * 0.06).max(0.2), color);
        }
    }
    pg.restore();
}
