//! Row-level typography compatibility, independent of inline style boundaries.

use super::TextLine;

fn family(line: &TextLine) -> String {
    const SUFFIXES: [&str; 16] = [
        "bolditalic",
        "boldoblique",
        "semibold",
        "demibold",
        "regular",
        "boldit",
        "italic",
        "oblique",
        "medium",
        "light",
        "bold",
        "roman",
        "psmt",
        "mt",
        "ps",
        "it",
    ];
    let mut key = pdfcraft_fonts::embedding::face_key(&line.base_font);
    // Weight and style suffixes stack (TimesNewRomanPS-BoldMT), so strip until none is left.
    while let Some(base) = SUFFIXES.iter().find_map(|suffix| key.strip_suffix(suffix)).filter(|base| !base.is_empty()) {
        key = base.to_owned();
    }
    key
}

/// The size most of a row is set in: the one whose runs cover the most of its width, so one
/// larger word does not make a body row look like a heading.
pub(super) fn dominant_size(row: &[usize], lines: &[TextLine]) -> Option<f64> {
    let mut widths: Vec<(f64, f64)> = Vec::new();
    for line in row.iter().filter_map(|index| lines.get(*index)) {
        let width = (line.rect[2] - line.rect[0]).max(0.0);
        match widths.iter_mut().find(|(size, _)| (size - line.size).abs() < 0.01) {
            Some((_, total)) => *total += width,
            None => widths.push((line.size, width)),
        }
    }
    widths.into_iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|(size, _)| size)
}

pub(super) fn compatible(previous: &[usize], next: &[usize], lines: &[TextLine]) -> bool {
    let representative =
        |row: &[usize]| row.iter().filter_map(|index| lines.get(*index)).max_by(|a, b| (a.rect[2] - a.rect[0]).total_cmp(&(b.rect[2] - b.rect[0])));
    match (representative(previous), representative(next)) {
        (Some(previous), Some(next)) => {
            // A row in one of the generated fallback faces holds characters the paragraph's own
            // font lacks; the edit chose that face, so it is no change of family.
            previous.size.min(next.size) >= previous.size.max(next.size) * 0.65
                && (previous.face.is_some() || next.face.is_some() || family(previous) == family(next))
        }
        _ => false,
    }
}
