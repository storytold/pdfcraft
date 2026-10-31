use super::{BlockStyle, HashMap, Matrix, TextBlock, TextLine, paragraph_separator, same_base_font, same_font, strip_subset_tag, union};

fn same_plane(a: &TextLine, b: &TextLine) -> bool {
    let at = Matrix(a.origin.tm).then(&Matrix(a.origin.ctm));
    let bt = Matrix(b.origin.tm).then(&Matrix(b.origin.ctm));
    a.scope == b.scope
        && a.paint == b.paint
        && [0, 2].iter().all(|axis| {
            let ax = at.0.get(*axis).copied().unwrap_or(0.0);
            let ay = at.0.get(axis + 1).copied().unwrap_or(0.0);
            let bx = bt.0.get(*axis).copied().unwrap_or(0.0);
            let by = bt.0.get(axis + 1).copied().unwrap_or(0.0);
            let al = ax.hypot(ay);
            let bl = bx.hypot(by);
            al > 0.0 && bl > 0.0 && (ax / al - bx / bl).abs() < 0.001 && (ay / al - by / bl).abs() < 0.001
        })
}

pub(super) fn row_rect(row: &[usize], lines: &[TextLine]) -> Option<[f64; 4]> {
    row.iter().filter_map(|i| lines.get(*i)).map(|l| l.rect).reduce(union)
}

/// [`row_rect`] in its text's own frame ([`TextLine::frame_rect`]), so rows of turned text
/// compare along and across their baselines.
fn frame_row_rect(row: &[usize], lines: &[TextLine]) -> Option<[f64; 4]> {
    row.iter().filter_map(|i| lines.get(*i)).map(TextLine::frame_rect).reduce(union)
}

/// The left edge the rest of the page's text sits on, ignoring the rows of `block`. A single row
/// that does not share it is not merely an indented or full-width left-aligned line, so its own
/// centre is the only remaining explanation for where it sits.
fn neighbour_left(lines: &[TextLine], block: &TextBlock) -> Option<f64> {
    let mut edges: HashMap<i64, usize> = HashMap::new();
    for (i, line) in lines.iter().enumerate() {
        if block.lines.contains(&i) {
            continue;
        }
        // Quarter-point buckets: equal edges found through different runs still agree.
        *edges.entry((line.rect[0] * 4.0).round() as i64).or_insert(0) += 1;
    }
    let (best, _) = edges.into_iter().max_by_key(|(_, count)| *count)?;
    Some(best as f64 / 4.0)
}

/// Where an alignment picked for a one-row paragraph is measured, as `[left, right]`: the column of
/// text it sits in (an overlapping paragraph's left edge, to the furthest right its rows reach)
/// when that column holds it, otherwise its own box. Of the columns that hold it, the narrowest is
/// its own, the nearest on a tie: a heading across the whole page, even one closer than the body,
/// is wider than the column the title sits in, and gives it no frame while a narrower one does. A lone row's own box is no frame
/// to align in: left-aligning a title centred on the page left it where it was, and since a single
/// row grows to the page's margin, centring it again moved it right each time. `None` when no
/// alignment is asked for, when the paragraph is several rows, turned, or given a width or a move of
/// its own, or when its `draft` text changes and no column holds it.
pub(crate) fn alignment_frame(lines: &[TextLine], blocks: &[TextBlock], block: &TextBlock, style: &BlockStyle, draft: &str) -> Option<[f64; 2]> {
    // Every row of the draft is aligned, by its own alignment or the paragraph's: moving the
    // paragraph to its column's edge would move a row that has none. Entries past the draft's
    // rows name no row.
    let asked = (0..crate::paragraph_align::count(draft)).all(|row| style.aligns.get(row).copied().flatten().or(style.align).is_some());
    if !asked || style.width.is_some() || style.offset.is_some() || block.rows.len() != 1 || block.rotation.abs() > 0.01 {
        return None;
    }
    let own = [block.rect[0], block.rect[2]];
    let tolerance = block.size.max(1.0) * 0.5;
    let gap = |other: &TextBlock| (other.rect[1] - block.rect[3]).max(block.rect[1] - other.rect[3]).max(0.0);
    // The column an overlapping paragraph makes, when it holds the row.
    let column = |other: &TextBlock| -> Option<[f64; 2]> {
        let left = other.rect[0];
        // Whole rows, not their runs: a row of several styles reaches its last run's right edge.
        let right = other
            .rows
            .iter()
            .filter_map(|row| row_rect(row, lines))
            .filter(|row| (row[0] - left).abs() < tolerance)
            .map(|row| row[2])
            .fold(f64::NEG_INFINITY, f64::max);
        (right.is_finite() && right - left > own[1] - own[0] && own[0] >= left - tolerance && own[1] <= right + tolerance).then_some([left, right])
    };
    let frame = blocks
        .iter()
        .filter(|other| other.lines.iter().all(|i| !block.lines.contains(i)) && other.rect[2] > own[0] && other.rect[0] < own[1])
        .filter_map(|other| column(other).map(|frame| (frame, gap(other))))
        .min_by(|(a, near_a), (b, near_b)| (a[1] - a[0]).total_cmp(&(b[1] - b[0])).then(near_a.total_cmp(near_b)))
        .map(|(frame, _)| frame);
    // The row's own box, with no column to align in, is no width for new text to wrap at: edited,
    // the row keeps the width it would have had without the alignment.
    match frame {
        Some(frame) => Some(frame),
        None if draft != block.editor_text => None,
        None => Some(own),
    }
}

/// Whether a block reads as centred, and so must keep its centre when it grows or wraps.
///
/// Several ragged rows whose left edges disagree while their centres agree are centred. A one row
/// block cannot say that: all of its left edges are the same, so the ragged test never fires and a
/// centred title would be taken for left-aligned and grow to the right (#2581). Fall back to the
/// page instead — a lone row sitting on the page's own centre, and not on the edge the rest of the
/// text uses, is a centred title. A full-width left-aligned line still lines up with that edge, so
/// it is not mistaken for one.
pub(crate) fn is_centred(block: &TextBlock, lines: &[TextLine], page_centre: f64, size: f64) -> bool {
    let centre = (block.rect[0] + block.rect[2]) / 2.0;
    let lefts: Vec<_> = block.rows.iter().filter_map(|r| row_rect(r, lines).map(|r| r[0])).collect();
    if block.rows.len() > 1 {
        let left_span = lefts.iter().copied().fold(f64::NEG_INFINITY, f64::max) - lefts.iter().copied().fold(f64::INFINITY, f64::min);
        return left_span > size * 0.5
            && block.rows.iter().all(|row| row_rect(row, lines).is_some_and(|r| ((r[0] + r[2]) / 2.0 - centre).abs() < size));
    }
    lefts
        .first()
        .is_some_and(|left| (centre - page_centre).abs() < size && neighbour_left(lines, block).is_none_or(|edge| (*left - edge).abs() > size * 0.5))
}

pub(crate) fn group_blocks(lines: &[TextLine]) -> Vec<TextBlock> {
    group_layout(lines, true)
}

/// Exporters currently represent a block with one style. Keep their style boundaries until
/// they support rich runs, while edit selection may encompass the complete visual region.
/// Dotted-leader entries stay on one row and use the title run's style.
pub(crate) fn group_reading_blocks(lines: &[TextLine]) -> Vec<TextBlock> {
    let mut blocks = Vec::new();
    let mut start = 0;
    let mut offset = 0;
    for row in lines.chunk_by(|a, b| same_plane(a, b) && (a.origin.baseline - b.origin.baseline).abs() < a.size.min(b.size) * 0.2) {
        let mut text = String::new();
        for (i, line) in row.iter().enumerate() {
            if !text.ends_with(char::is_whitespace)
                && !line.text.starts_with(char::is_whitespace)
                && i.checked_sub(1).and_then(|i| row.get(i)).is_some_and(|prev| line.rect[0] - prev.rect[2] > line.size * 0.12)
            {
                text.push(' ');
            }
            text.push_str(&line.text);
        }
        if leader_entry(&text)
            && let Some(first) = row.first()
        {
            // Tab leaders are a single reading row, even when the title, dots and page
            // number are separate text objects far apart. Do not merge neighbouring entries.
            blocks.extend(group_layout(lines.get(start..offset).unwrap_or_default(), false));
            let mut line = first.clone();
            line.text = text;
            line.rect = row.iter().map(|line| line.rect).reduce(union).unwrap_or(first.rect);
            blocks.extend(group_layout(std::slice::from_ref(&line), false));
            start = offset + row.len();
        }
        offset += row.len();
    }
    blocks.extend(group_layout(lines.get(start..).unwrap_or_default(), false));
    blocks
}

/// Dotted leaders end in a page number, with a title before the dots. A dot in an article
/// number, or an ellipsis in ordinary prose, is not enough to make a contents entry.
fn leader_entry(text: &str) -> bool {
    let text = text.trim_end();
    let prefix = text.trim_end_matches(|c: char| {
        c.is_ascii_digit() || matches!(c, 'i' | 'v' | 'x' | 'l' | 'c' | 'd' | 'm' | 'I' | 'V' | 'X' | 'L' | 'C' | 'D' | 'M')
    });
    if prefix.len() == text.len() {
        return false;
    }
    let title = prefix.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '.' | '…'));
    let dots = prefix
        .get(title.len()..)
        .unwrap_or_default()
        .chars()
        .map(|c| match c {
            '.' => 1,
            '…' => 3,
            _ => 0,
        })
        .sum::<usize>();
    dots >= 3 && title.chars().any(char::is_alphabetic)
}

/// Text-space units → user space along the baseline. It is the vertical scale ([`Origin::k`])
/// unless the text matrix stretches the glyphs, as an OCR layer fitting each word to the scan does.
pub(super) fn across(line: &TextLine) -> f64 {
    let m = Matrix(line.origin.tm).then(&Matrix(line.origin.ctm)).0;
    let h = m[0].hypot(m[1]);
    if h.is_finite() && h > 1e-9 { h } else { line.origin.k }
}

/// Invisible text (rendering mode 3), as in an OCR layer over a scanned image.
fn invisible(line: &TextLine) -> bool {
    line.origin.state.render_mode == 3
}

fn same_style(a: &TextLine, b: &TextLine) -> bool {
    same_font(a, b.font.as_bytes(), &b.base_font, b.face.as_deref())
        // With no `/BaseFont` (or only a subset tag) there is no name to compare: the resource or
        // fallback face `same_font` matched is the whole identity, so keep the lines together.
        && (same_base_font(&a.base_font, &b.base_font)
            || (strip_subset_tag(&a.base_font).is_empty() && strip_subset_tag(&b.base_font).is_empty()))
        && a.bold == b.bold
        && a.italic == b.italic
        && a.color == b.color
        && (a.rotation() - b.rotation()).abs() < 0.01
        && (a.size - b.size).abs() < 0.01
}

/// Does a row open with a list marker? A bullet or a dash, or an ordered marker — digits (or a
/// single letter) followed by `.` or `)` and then a space or the row's end, so prose such as
/// `1.5` or `Dr. Smith` is left alone.
fn opens_with_marker(text: &str) -> bool {
    let Some(first) = text.chars().next() else { return false };
    if matches!(first, '•' | '◦' | '▪' | '‣' | '⁃' | '*' | '-' | '–' | '—') {
        return true;
    }
    if !first.is_ascii_digit() && !first.is_ascii_alphabetic() {
        return false;
    }
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    let letters = text.chars().take_while(char::is_ascii_alphabetic).count();
    // Both runs are ASCII characters, so their length in characters is their length in bytes:
    // the slice below starts on a character boundary.
    let rest = if digits > 0 {
        &text[digits..]
    } else if letters == 1 {
        &text[1..]
    } else {
        return false;
    };
    let mut after = rest.chars();
    matches!(after.next(), Some('.' | ')')) && after.next().is_none_or(char::is_whitespace)
}

/// A row that opens with a list marker opens its own item: joining it to the row above is what
/// let an edit of one paragraph delete every item after it (#2564). The other error, splitting
/// an ordinary paragraph, costs one more paragraph to click and never any text — so err this way.
fn starts_list_item(row: &[usize], lines: &[TextLine]) -> bool {
    row.iter().filter_map(|i| lines.get(*i)).map(|l| l.text.trim_start()).find(|t| !t.is_empty()).is_some_and(opens_with_marker)
}

fn group_layout(lines: &[TextLine], mixed: bool) -> Vec<TextBlock> {
    let mut rows: Vec<Vec<usize>> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let joins = rows.last().and_then(|r| r.last()).and_then(|i| lines.get(*i)).is_some_and(|prev| {
            let gap = line.frame_rect()[0] - prev.frame_rect()[2];
            // OCR words (invisible text over a scan) join only across a word space: a wider gap
            // is a table cell or a tab stop, which a rewrite would close up.
            let scanned = !mixed || (invisible(prev) == invisible(line) && (!invisible(line) || gap < prev.size.max(line.size) * 0.6));
            scanned
                && (mixed || same_style(prev, line))
                && same_plane(prev, line)
                // A superscript or subscript run (a footnote marker, say) sits on the same visual
                // row as the text around it, raised or lowered by `Ts` from the body baseline
                // (#3252). That shift is typically up to half the body size, not the marker's own
                // smaller size — but the wider tolerance only applies between a run and a
                // distinctly smaller or larger one, with no horizontal overlap (`gap >= 0.0`):
                // two equal-size runs can land within it too, at a baseline shift tight columns
                // or compressed text (`Tz`) can produce between genuinely separate rows, and the
                // original 20% tolerance is what keeps those apart.
                && ((prev.frame_origin().1 - line.frame_origin().1).abs() < prev.size.min(line.size) * 0.2
                    || (gap >= 0.0
                        && prev.size.min(line.size) < prev.size.max(line.size) * 0.9
                        && (prev.frame_origin().1 - line.frame_origin().1).abs() < prev.size.max(line.size) * 0.5))
                && gap >= -prev.size.min(line.size) * 0.2
                && gap < prev.size.max(line.size) * 1.5
                // Separate same-style text objects that merely touch or overlap, with no space
                // between them, are separate labels (for example a paragraph dragged against
                // another, #594). Style runs and words separated by a space or gap still join.
                && (prev.origin.bt_op == line.origin.bt_op
                    || gap > prev.size.min(line.size) * 0.01
                    || !same_style(prev, line)
                    || prev.text.ends_with(char::is_whitespace)
                    || line.text.starts_with(char::is_whitespace))
        });
        if joins && let Some(row) = rows.last_mut() {
            row.push(i);
        } else {
            rows.push(vec![i]);
        }
    }
    let mut blocks: Vec<TextBlock> = Vec::new();
    for row_at in 0..rows.len() {
        let Some(row) = rows.get(row_at) else { continue };
        let Some(first) = row.first().and_then(|i| lines.get(*i)) else { continue };
        let (Some(rect), Some(frame)) = (row_rect(row, lines), frame_row_rect(row, lines)) else { continue };
        let joins = blocks.last().is_some_and(|block| {
            // A bullet of its own opens a list item (#2564): such a row is a new paragraph, and
            // letting it join is what made an edit of the row above delete this item and the ones
            // after it.
            if starts_list_item(row, lines) {
                return false;
            }
            let Some(prev_row) = block.rows.last() else { return false };
            let Some(prev) = prev_row.first().and_then(|i| lines.get(*i)) else { return false };
            let Some(pr) = frame_row_rect(prev_row, lines) else { return false };
            let gap = prev.frame_origin().1 - first.frame_origin().1;
            let tol = prev.size.max(first.size).max(1.0);
            // The block opens with a list marker and this row sits further right: the item's text
            // is at a hanging indent, so the row it wrapped onto continues it (#2564). A row at
            // the block's own left edge is already covered by the match on `pr[0]`. Both boxes are
            // in the frame, as the rest of this comparison is, so turned text matches too.
            let hanging = frame[0] > pr[0]
                && frame[0] - pr[0] <= tol * 3.0
                && block.lines.first().and_then(|&i| lines.get(i)).is_some_and(|l| opens_with_marker(l.text.trim_start()));
            let aligned = (pr[0] - frame[0]).abs() < tol
                || ((pr[0] + pr[2] - frame[0] - frame[2]) / 2.0).abs() < tol
                || (pr[2] - frame[2]).abs() < tol
                || hanging;
            // Once a paragraph has two rows, its own leading is the measured baseline gap. A
            // blank line is about twice that, so a wider gap is the next paragraph (#2517). A
            // single row uses the text object's TL when it has one: a gap equal to that TL
            // joins, and a gap of twice the TL does not. `Td` does not set TL (`TD` does). A
            // step that repeats on the following row is that leading; one large gap, with no
            // second step to compare, stays a break. The old cap of 2.5 × the size joined the
            // blank line too.
            let own_leading = block.rows.len().checked_sub(2).and_then(|i| {
                let above = block.rows.get(i)?.first().and_then(|n| lines.get(*n));
                let below = prev_row.first().and_then(|n| lines.get(*n));
                match (above, below) {
                    (Some(above), Some(below)) => Some((above.frame_origin().1 - below.frame_origin().1).abs()),
                    _ => None,
                }
            });
            let stated = (prev.origin.state.leading > 0.0).then(|| prev.wrap_leading()).filter(|lead| lead.is_finite() && *lead > tol * 0.4);
            // The next row is not in this block yet. Its gap is only a leading when it matches
            // this one: a blank line is a single wider gap and has no matching step after it.
            let repeated = rows.get(row_at.saturating_add(1)).and_then(|next| {
                let nfirst = next.first().and_then(|i| lines.get(*i))?;
                let nframe = frame_row_rect(next, lines)?;
                let ngap = first.frame_origin().1 - nfirst.frame_origin().1;
                let same = same_plane(first, nfirst)
                    && (mixed || same_style(first, nfirst))
                    && first.size.min(nfirst.size) >= first.size.max(nfirst.size) * 0.4
                    && (nframe[0] - frame[0]).abs() < tol;
                let scale = gap.abs().max(ngap.abs());
                if same && ngap > tol * 0.8 && scale < tol * 4.0 && (ngap - gap).abs() <= scale * 0.08 + 0.5 { Some(gap) } else { None }
            });
            let max_gap = own_leading.filter(|lead| *lead > tol * 0.4).or(stated).or(repeated).map_or(tol * 2.05, |lead| lead * 1.5);
            // OCR lines stay apart: the scan fixes where each one is, so they never rewrap together.
            (!mixed || !(invisible(prev) || invisible(first)))
                && (mixed || same_style(prev, first))
                && same_plane(prev, first)
                // The rewrite sets every row in the first one's horizontal scale.
                && (prev.origin.state.scale - first.origin.state.scale).abs() < 1e-6
                && aligned
                && first.size.min(prev.size) >= first.size.max(prev.size) * 0.4
                && gap > first.size.min(prev.size) * 0.8
                && gap < max_gap
        });
        if !joins {
            blocks.push(TextBlock {
                text: String::new(),
                editor_text: String::new(),
                reflow_text: String::new(),
                spans: Vec::new(),
                rect,
                base_font: first.base_font.clone(),
                size: first.size,
                bold: first.bold,
                italic: first.italic,
                color: first.color,
                rotation: first.rotation(),
                lines: Vec::new(),
                rows: Vec::new(),
                invisible: true,
            });
        }
        let Some(block) = blocks.last_mut() else { continue };
        let mut row_text = String::new();
        if !block.rows.is_empty() {
            block.editor_text.push('\n');
        }
        for &index in row {
            let Some(line) = lines.get(index) else { continue };
            // Some producers position separate words without encoding a space.
            if let Some(prev) = block.lines.last().filter(|i| row.contains(i)).and_then(|i| lines.get(*i))
                && !row_text.ends_with(char::is_whitespace)
                && !line.text.starts_with(char::is_whitespace)
                && line.frame_rect()[0] - prev.frame_rect()[2] > line.size * 0.12
            {
                row_text.push(' ');
                block.editor_text.push(' ');
            }
            let start = block.editor_text.len();
            block.editor_text.push_str(&line.text);
            block.spans.push((start..block.editor_text.len(), index));
            row_text.push_str(&line.text);
            block.invisible &= line.origin.state.render_mode == 3;
            block.lines.push(index);
        }
        if !block.text.is_empty() {
            if block.text.ends_with('-') {
                block.text.pop();
            } else {
                block.text.push(' ');
            }
        }
        block.text.push_str(row_text.trim_matches(paragraph_separator));
        block.rect = union(block.rect, rect);
        block.rows.push(row.clone());
    }
    for block in &mut blocks {
        block.reflow_text = reflow_text(block, lines);
    }
    blocks
}

/// The text a width change should reflow. A `/PCSoft` line is a wrap and rejoins. A `/PCHard`
/// line, or a short line that is not marked, keeps its newline. An unmarked line that reaches
/// the paragraph's right edge and is wide enough to be a measure rejoins too: that is how a
/// file written elsewhere wraps a body line.
fn reflow_text(block: &TextBlock, lines: &[TextLine]) -> String {
    let parts: Vec<&str> = block.editor_text.split('\n').collect();
    if parts.len() != block.rows.len() {
        return block.editor_text.clone();
    }
    let mut out = String::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            match break_gap(block, lines, index) {
                Some(_) if out.ends_with('-') => {
                    out.pop();
                }
                // `gap` is the exact number of separator characters the wrap consumed (0 for a
                // Chinese/Japanese word split, which has no space of its own) when the marker or
                // the source recorded it; a line already ending in whitespace is left alone so a
                // row whose own text kept a trailing space isn't doubled.
                Some(gap) if !out.ends_with(char::is_whitespace) => out.push_str(&" ".repeat(gap)),
                Some(_) => {}
                None => out.push('\n'),
            }
        }
        out.push_str(part);
    }
    out
}

/// `None`: a hard break, kept as a newline. `Some(gap)`: a soft wrap, rejoined with exactly `gap`
/// spaces — the count a `/PCSoft` marker recorded ([`TextLine::soft_gap`]), or, for a line this
/// app did not mark (a file written elsewhere, or an older save), a single space when the
/// geometry looks like an ordinary wrapped body line (`#2529`).
fn break_gap(block: &TextBlock, lines: &[TextLine], row_index: usize) -> Option<usize> {
    let line = block.rows.get(row_index).and_then(|row| row.first()).and_then(|i| lines.get(*i))?;
    if line.hard_break {
        return None;
    }
    if line.soft_break {
        return Some(line.soft_gap);
    }
    let prev = block.rows.get(row_index.wrapping_sub(1)).and_then(|row| row_rect(row, lines))?;
    let next = block.rows.get(row_index).and_then(|row| row_rect(row, lines))?;
    let size = line.size.max(1.0);
    let slack = block.rect[2] - prev[2];
    let width = prev[2] - prev[0];
    // Short rows define the right edge by their own letters. A body line that fills the
    // measure leaves almost no room for the next word.
    (slack <= size * 0.75 && width >= size * 8.0 && (next[2] - next[0]) + size * 0.25 > slack).then_some(1)
}
