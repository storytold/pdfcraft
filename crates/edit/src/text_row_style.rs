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
