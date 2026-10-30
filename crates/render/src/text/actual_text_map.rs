//! Precise selection for a replacement whose painted Unicode proves its cluster geometry.
//!
//! A replacement is kept whole during page layout. Only the final emission expands this
//! mapping, so physical sorting cannot reverse the logical order a second time.

use super::TextGlyph;
use super::device::{Drawn, merge};
use pdfcraft_fonts::embedding::default_ignorable;
use unicode_bidi::ParagraphBidiInfo;

const MAX_CHARS: usize = 65_536;
const MAX_BYTES: usize = 262_144;

/// A painted cluster: a glyph with visible Unicode and the empty-Unicode glyphs (marks,
/// ligature parts) drawn right after it.
struct Cluster {
    drawn: Vec<Drawn>,
    /// Its Unicode, invisible format characters removed.
    unicode: String,
    /// Where it starts along the baseline (the writing direction `u`).
    along: f64,
    /// The span its advancing glyphs cover along the baseline.
    interval: [f64; 2],
}

/// A cluster matched to the logical characters it paints: `first..=last` are indices into the
/// logical text's characters, and `glyph` is the cluster's geometry carrying that text.
struct Mapped {
    first: usize,
    last: usize,
    glyph: TextGlyph,
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
        '‹' => '›',
        '›' => '‹',
        _ => c,
    }
}

fn hanging(c: char) -> bool {
    c.is_whitespace() && !matches!(c, '\r' | '\n' | '\u{85}' | '\u{2028}' | '\u{2029}' | '\u{a0}' | '\u{2007}' | '\u{202f}')
}

/// Match complete painted clusters to their proven visual occurrences. In particular, repeated
/// Arabic letters are not matched by an arbitrary FIFO of equal strings. Their source character
/// indices come from the complete Unicode bidi permutation, checked against the physical order.
pub(super) fn precise(drawn: &[Drawn], logical: &str) -> Option<Vec<TextGlyph>> {
    // 1. Refuse what this mapping can't prove: empty or oversized input, or a paragraph break.
    if logical.is_empty()
        || logical.len() > MAX_BYTES
        || drawn.len() > MAX_CHARS
        || drawn.iter().fold(0usize, |n, d| n.saturating_add(d.2.text.len())) > MAX_BYTES
        || logical.contains(['\r', '\n', '\u{85}', '\u{2028}', '\u{2029}'])
    {
        return None;
    }
    let chars: Vec<_> = logical.char_indices().collect();
    if chars.len() > MAX_CHARS {
        return None;
    }
    // 2. One row: `u` is the writing direction and `across` its normal; every advancing glyph
    // sits on the first one's baseline, to within 2% of its height.
    let first = drawn.iter().find(|d| d.3.is_some_and(|p| p.advances))?;
    let u = first.1;
    let across = [-u[1], u[0]];
    let origin = first.3?.baseline;
    let baseline = origin[0] * across[0] + origin[1] * across[1];
    let height = f64::from((first.2.quad[0][0] - first.2.quad[3][0]).hypot(first.2.quad[0][1] - first.2.quad[3][1]));
    if !baseline.is_finite() || !height.is_finite() || height <= 0.0 {
        return None;
    }
    for d in drawn {
        if !super::layout::compatible(u, d.1) || d.2.quad.iter().flatten().any(|n| !n.is_finite()) {
            return None;
        }
        let p = d.3?;
        if p.baseline.iter().any(|n| !n.is_finite()) {
            return None;
        }
        if p.advances && (p.baseline[0] * across[0] + p.baseline[1] * across[1] - baseline).abs() > (height * 0.02).max(0.01) {
            return None;
        }
    }
    // 3. Group the glyphs, in drawing order, into clusters.
    let along = |g: &TextGlyph| f64::from(g.quad[0][0]) * u[0] + f64::from(g.quad[0][1]) * u[1];
    let mut clusters: Vec<Cluster> = Vec::new();
    for d in drawn {
        let unicode: String = d.2.text.chars().filter(|&c| !default_ignorable(c)).collect();
        if unicode.is_empty() {
            // A declared invisible control contributes no ink or separate character geometry.
            if !d.2.text.is_empty() {
                continue;
            }
            // Source shaping assigns a cluster's text to its first glyph. Its immediately
            // following empty-Unicode auxiliaries remain attached before physical sorting.
            clusters.last_mut()?.drawn.push(d.clone());
        } else {
            clusters.push(Cluster { drawn: vec![d.clone()], unicode, along: 0.0, interval: [0.0; 2] });
        }
    }
    if clusters.is_empty() {
        return None;
    }
    // 4. Each cluster's span along the baseline, from its advancing glyphs; then the clusters in
    // physical order, as they sit along `u`.
    for c in &mut clusters {
        let mut low = f64::INFINITY;
        let mut high = f64::NEG_INFINITY;
        for d in c.drawn.iter().filter(|d| d.3.is_some_and(|p| p.advances)) {
            let a = along(&d.2);
            let b = f64::from(d.2.quad[1][0]) * u[0] + f64::from(d.2.quad[1][1]) * u[1];
            low = low.min(a.min(b));
            high = high.max(a.max(b));
        }
        if !low.is_finite() || !high.is_finite() || high <= low {
            return None;
        }
        c.interval = [low, high];
        c.along = low;
    }
    clusters.sort_by(|a, b| a.along.total_cmp(&b.along));
    // An auxiliary mark must belong unambiguously to its preceding painted cluster: its centre
    // projects inside that cluster's em advance, and inside no other source cluster's advance.
    // Empty or ambiguous replacement artwork therefore retains the whole-span fallback.
    let intervals: Vec<_> = clusters.iter().map(|c| (c.interval[0], c.interval[1])).collect();
    let mut starts: Vec<_> = intervals.iter().map(|&(a, _)| a).collect();
    let mut ends: Vec<_> = intervals.iter().map(|&(_, b)| b).collect();
    starts.sort_by(f64::total_cmp);
    ends.sort_by(f64::total_cmp);
    for (i, c) in clusters.iter().enumerate() {
        for d in c.drawn.iter().filter(|d| d.3.is_some_and(|p| !p.advances)) {
            let centre = d.2.quad.iter().map(|p| f64::from(p[0]) * u[0] + f64::from(p[1]) * u[1]).sum::<f64>() / 4.0;
            if !(centre >= intervals[i].0 && centre <= intervals[i].1) {
                return None;
            }
            let covering = starts.partition_point(|&a| a <= centre).saturating_sub(ends.partition_point(|&b| b < centre));
            if covering != 1 {
                return None;
            }
        }
    }
    // 5. The logical text in visual order (UAX #9): `visual[k]` is the index of the character
    // painted k-th. `ordinal` numbers the visible characters in logical order, so owners that are
    // neighbours in the text have consecutive ordinals.
    let bidi = ParagraphBidiInfo::new(logical, None);
    let levels = bidi.reordered_levels_per_char(0..logical.len());
    let visual = ParagraphBidiInfo::reorder_visual(&levels);
    let visible: Vec<_> = chars.iter().enumerate().filter_map(|(i, (_, c))| (!default_ignorable(*c)).then_some(i)).collect();
    let mut ordinal = vec![None; chars.len()];
    for (at, &i) in visible.iter().enumerate() {
        ordinal[i] = Some(at);
    }
    // 6. Walk the clusters in physical order and give each the next `n` visible characters of the
    // visual order as its owners. The owners must be neighbours in the logical text and spell the
    // cluster's Unicode (a bracket at a right-to-left level may be painted mirrored). Characters
    // from `omit_from` on are left out of the walk.
    let try_map = |omit_from: usize| -> Option<Vec<Mapped>> {
        let expected: Vec<_> = visual.iter().copied().filter(|&i| i < omit_from && !default_ignorable(chars[i].1)).collect();
        let mut cursor: usize = 0;
        let mut mapped = Vec::new();
        for cluster in &clusters {
            let n = cluster.unicode.chars().count();
            let end = cursor.checked_add(n)?;
            let mut owners = expected.get(cursor..end)?.to_vec();
            owners.sort_unstable();
            if owners.windows(2).any(|p| ordinal[p[0]].and_then(|n| n.checked_add(1)) != ordinal[p[1]]) {
                return None;
            }
            let original: String = owners.iter().map(|&i| chars[i].1).collect();
            let mirror: String = owners.iter().map(|&i| if levels[i].is_rtl() { mirrored(chars[i].1) } else { chars[i].1 }).collect();
            if cluster.unicode != original && cluster.unicode != mirror {
                return None;
            }
            let first = *owners.first()?;
            let last = *owners.last()?;
            let glyph = merge(&cluster.drawn, logical.get(chars[first].0..chars.get(last + 1).map_or(logical.len(), |c| c.0))?.to_string())?.2;
            mapped.push(Mapped { first, last, glyph });
            cursor = end;
        }
        (cursor == expected.len()).then_some(mapped)
    };
    // 7. Every character first; failing that, without the trailing spaces (and invisible
    // characters) a producer may leave unpainted at the end of the line.
    let mut mapped = match try_map(chars.len()) {
        Some(mapped) => mapped,
        None => {
            let mut suffix = chars.len();
            while suffix > 0 && (hanging(chars[suffix - 1].1) || default_ignorable(chars[suffix - 1].1)) {
                suffix -= 1;
            }
            if suffix == 0 || !chars[suffix..].iter().any(|&(_, c)| hanging(c)) {
                return None;
            }
            try_map(suffix)?
        }
    };
    // 8. Emit in logical order: each cluster's glyph; an invisible character between clusters as
    // an empty glyph at the edge of the cluster it precedes; then the trailing invisible characters
    // and hanging spaces at the line's reading end. The result must spell `logical` exactly.
    mapped.sort_by_key(|m| m.first);
    let last_logical = mapped.last()?.last;
    let mut out: Vec<TextGlyph> = Vec::new();
    let mut end = 0;
    for m in mapped {
        let start_byte = chars[m.first].0;
        let gap = logical.get(end..start_byte)?;
        if !gap.chars().all(default_ignorable) {
            return None;
        }
        for c in gap.chars() {
            out.push(invisible(&m.glyph, levels[m.first].is_rtl(), true, c));
        }
        end = chars.get(m.last + 1).map_or(logical.len(), |c| c.0);
        out.push(m.glyph);
    }
    let boundary = merge(drawn, String::new())?.2;
    let tail = logical.get(end..)?;
    for c in tail.chars() {
        if default_ignorable(c) {
            out.push(invisible(out.last()?, levels[last_logical].is_rtl(), false, c));
        } else if hanging(c) {
            let (a, b) = if bidi.paragraph_level.is_rtl() { (boundary.quad[0], boundary.quad[3]) } else { (boundary.quad[1], boundary.quad[2]) };
            let quad = [a, a, b, b];
            let rect = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
            out.push(TextGlyph { text: c.to_string(), rect, quad, direction: boundary.direction });
        } else {
            return None;
        }
    }
    (out.iter().map(|g| g.text.as_str()).collect::<String>() == logical).then_some(out)
}

fn invisible(g: &TextGlyph, rtl: bool, start: bool, c: char) -> TextGlyph {
    let (a, b) = if rtl == start { (g.quad[1], g.quad[2]) } else { (g.quad[0], g.quad[3]) };
    let quad = [a, a, b, b];
    TextGlyph { text: c.to_string(), rect: [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])], quad, direction: g.direction }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::device::GlyphPosition;

    fn glyph(text: &str, x: f32, y: f32, width: f32) -> Drawn {
        let quad = [[x, y + 20.0], [x + width, y + 20.0], [x + width, y], [x, y]];
        (
            0,
            [1.0, 0.0],
            TextGlyph { text: text.into(), rect: [x, y, x + width, y + 20.0], quad, direction: [1.0, 0.0] },
            Some(GlyphPosition { baseline: [f64::from(x), f64::from(y + 16.0)], advances: true }),
        )
    }

    #[test]
    fn exact_mapping_uses_physical_occurrences_and_rejects_unproved_geometry() {
        let a = glyph("A", 10.0, 0.0, 10.0);
        let b = glyph("B", 20.0, 0.0, 10.0);
        let mapped = precise(&[b.clone(), a.clone()], "AB").unwrap();
        assert_eq!(mapped.iter().map(|g| g.text.as_str()).collect::<Vec<_>>(), ["A", "B"]);
        assert_eq!(mapped[0].rect, a.2.rect);
        let mut changed = b.clone();
        changed.3.as_mut().unwrap().baseline[1] += 10.0;
        assert!(precise(&[a.clone(), changed], "AB").is_none(), "identical text on different baselines is not one mapped row");
        let mut changed = b.clone();
        changed.1 = [-1.0, 0.0];
        assert!(precise(&[a.clone(), changed], "AB").is_none());
        let mut changed = b.clone();
        changed.2.quad[0][0] = f32::NAN;
        assert!(precise(&[a.clone(), changed], "AB").is_none());
        let mut changed = b.clone();
        changed.3 = None;
        assert!(precise(&[a.clone(), changed], "AB").is_none());
        assert!(precise(&[a.clone(), b.clone()], "A\u{85}B").is_none());
        assert!(precise(&[a.clone(), b.clone()], "icon label").is_none());
        let mut changed = a.clone();
        changed.2.text = "B".into();
        let mut other = b;
        other.2.text = "A".into();
        assert!(precise(&[changed, other], "AB").is_none(), "a Unicode multiset is not an occurrence proof");
        let mut huge = a;
        huge.2.text = "A".repeat(MAX_BYTES + 1);
        assert!(precise(&[huge], "A").is_none());
    }

    #[test]
    fn empty_unicode_marks_cannot_be_assigned_inside_two_advancing_clusters() {
        let mut mark = glyph("A", 14.0, 0.0, 2.0);
        mark.3.as_mut().unwrap().advances = false;
        let base = glyph("", 10.0, 0.0, 10.0);
        let long = glyph("B", 0.0, 0.0, 100.0);
        let middle = glyph("C", 5.0, 0.0, 1.0);
        assert!(precise(&[mark, base, long, middle], "BCA").is_none(), "an earlier non-neighbour also owns the mark's position");
    }
}
