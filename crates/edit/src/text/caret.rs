//! Caret geometry read from PDF text operators, never from an interface replacement face.

use super::TextLine;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct CharacterPosition {
    pub ch: char,
    pub left: f64,
    pub right: f64,
}

/// One visual row of the draft. Ranges use UTF-8 byte offsets in the supplied text; a hard
/// newline is excluded from its row. Coordinates are PDF user-space points, before page rotation.
#[derive(Clone, Debug, PartialEq)]
pub struct DraftRow {
    pub range: std::ops::Range<usize>,
    pub baseline: f64,
    pub rect: [f64; 4],
    /// Start/end x for each Unicode scalar in `range`, in logical text order. A ligature's
    /// advance is divided among its scalars; PDF does not encode internal ligature carets.
    pub characters: Vec<[f64; 2]>,
    pub ends_with_newline: bool,
}

/// Refuse ambiguous reading-order mappings rather than moving a caret to unrelated text.
/// Turned, vertical and reordered contextual text keeps the editor's existing fallback.
pub(super) fn draft_rows(text: &str, lines: &[TextLine], source: [f64; 4], leading: f64) -> Vec<DraftRow> {
    build_rows(text, lines, source, leading).unwrap_or_default()
}

fn build_rows(text: &str, lines: &[TextLine], source: [f64; 4], leading: f64) -> Option<Vec<DraftRow>> {
    if lines.is_empty() || text.chars().count() > 131_072 || !leading.is_finite() || leading <= 0.0 {
        return None;
    }
    let mut ordered: Vec<_> = lines.iter().collect();
    if ordered.iter().any(|l| !l.upright() || l.visual_order || !l.characters.iter().map(|c| c.ch).eq(l.text.chars())) {
        return None;
    }
    ordered.sort_by(|a, b| b.baseline()[1].total_cmp(&a.baseline()[1]).then(a.rect[0].total_cmp(&b.rect[0])));
    let mut groups: Vec<(f64, [f64; 4], Vec<&CharacterPosition>)> = Vec::new();
    for line in ordered {
        if line.rect.iter().any(|v| !v.is_finite()) || !line.baseline()[1].is_finite() {
            return None;
        }
        if let Some((baseline, rect, chars)) = groups.last_mut()
            && (*baseline - line.baseline()[1]).abs() < 0.1
        {
            *rect = [rect[0].min(line.rect[0]), rect[1].min(line.rect[1]), rect[2].max(line.rect[2]), rect[3].max(line.rect[3])];
            chars.extend(&line.characters);
        } else {
            groups.push((line.baseline()[1], line.rect, line.characters.iter().collect()));
        }
    }
    for (_, _, chars) in &groups {
        if chars.iter().any(|c| !c.left.is_finite() || !c.right.is_finite() || c.right < c.left)
            || chars.windows(2).any(|c| c[1].left + 0.01 < c[0].left)
        {
            return None;
        }
    }
    let input: Vec<_> = text.char_indices().collect();
    let mut positions = vec![None; input.len()];
    let mut cursor = 0;
    for (group, (_, _, chars)) in groups.iter().enumerate() {
        for c in chars {
            if c.ch.is_whitespace() {
                if input.get(cursor).is_some_and(|(_, ch)| ch.is_whitespace() && *ch != '\n') {
                    *positions.get_mut(cursor)? = Some((group, [c.left, c.right]));
                    cursor += 1;
                }
                continue;
            }
            while input.get(cursor).is_some_and(|(_, ch)| ch.is_whitespace()) {
                cursor += 1;
            }
            if input.get(cursor)?.1 != c.ch {
                return None;
            }
            *positions.get_mut(cursor)? = Some((group, [c.left, c.right]));
            cursor += 1;
        }
    }
    if input.get(cursor..)?.iter().any(|(_, ch)| !ch.is_whitespace()) {
        return None;
    }
    let initial = groups.first().map(|(baseline, rect, _)| (*baseline, *rect)).unwrap_or((source[3] - leading * 0.8, source));
    // Leading blank lines precede the first emitted glyph row.
    let blanks = input.iter().take_while(|(_, c)| c.is_whitespace()).filter(|(_, c)| *c == '\n').count() as f64;
    let mut baseline = initial.0 + blanks * leading;
    let mut rect = initial.1;
    rect[1] += blanks * leading;
    rect[3] += blanks * leading;
    let mut group = 0;
    let mut start = 0;
    // Linear lookahead also bounds a draft consisting of many empty lines.
    let mut next_group = None;
    let mut following = vec![None; positions.len()];
    for (slot, position) in following.iter_mut().zip(&positions).rev() {
        if let Some((group, _)) = position {
            next_group = Some(*group);
        }
        *slot = next_group;
    }
    let mut rows = Vec::new();
    for (i, (_, ch)) in input.iter().enumerate() {
        if let Some((next, _)) = positions.get(i).copied().flatten()
            && next != group
        {
            if start < i {
                rows.push(make_row(text, &input, &positions, start..i, baseline, rect, false)?);
                start = i;
            }
            group = next;
        }
        if let Some((g, _)) = positions.get(i).copied().flatten() {
            let (y, r, _) = groups.get(g)?;
            baseline = *y;
            rect = *r;
        }
        if *ch == '\n' {
            rows.push(make_row(text, &input, &positions, start..i, baseline, rect, true)?);
            start = i + 1;
            baseline -= leading;
            rect[1] -= leading;
            rect[3] -= leading;
            // After a hard break the next written row belongs to this new row, even if the
            // PDF emitted no glyphs for one or more blank lines.
            if let Some(next) = following.get(start).copied().flatten() {
                group = next;
            }
        }
    }
    rows.push(make_row(text, &input, &positions, start..input.len(), baseline, rect, false)?);
    Some(rows)
}

fn make_row(
    text: &str,
    input: &[(usize, char)],
    positions: &[Option<(usize, [f64; 2])>],
    range: std::ops::Range<usize>,
    baseline: f64,
    mut rect: [f64; 4],
    ends_with_newline: bool,
) -> Option<DraftRow> {
    let mut characters: Vec<_> = positions.get(range.clone())?.iter().map(|p| p.map(|(_, x)| x)).collect();
    let mut i = 0;
    while i < characters.len() {
        if characters.get(i)?.is_some() {
            i += 1;
            continue;
        }
        let from = i;
        while characters.get(i).is_some_and(Option::is_none) {
            i += 1;
        }
        if input.get(range.start + from..range.start + i)?.iter().any(|(_, c)| !c.is_whitespace()) {
            return None;
        }
        let before = from.checked_sub(1).and_then(|p| characters.get(p).copied().flatten()).map(|x| x[1]);
        let after = characters.get(i).copied().flatten().map(|x| x[0]);
        let left = before.or(after).unwrap_or(rect[0]);
        let right = after.unwrap_or(left).max(left);
        let step = (right - left) / (i - from) as f64;
        for (k, slot) in characters.iter_mut().enumerate().take(i).skip(from) {
            *slot = Some([left + step * (k - from) as f64, left + step * (k - from + 1) as f64]);
        }
    }
    let mut characters: Vec<_> = characters.into_iter().collect::<Option<_>>()?;
    // TJ kerning belongs to the boundary before the next glyph, not to the interface font.
    for i in 1..characters.len() {
        let next = characters.get(i)?[0];
        characters.get_mut(i - 1)?[1] = next;
    }
    if let (Some(first), Some(last)) = (characters.first(), characters.last()) {
        rect[0] = first[0];
        rect[2] = last[1];
    } else {
        rect[2] = rect[0];
    }
    let byte = |index| input.get(index).map_or(text.len(), |(at, _)| *at);
    Some(DraftRow { range: byte(range.start)..byte(range.end), baseline, rect, characters, ends_with_newline })
}
