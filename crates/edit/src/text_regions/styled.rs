use super::grouping::{across, is_centred, row_rect};
use super::*;

// The draft is capped at 4 MiB; also bound the outline data prepared across all fallback faces.
// A glyph budget worth 64 full batches (15,360 glyphs with the production batch size).
// Lowering the internal batch size for tests lowers the budget by the same ratio.
const MAX_STYLED_FALLBACK_BATCHES: usize = 64;

struct FallbackGroup {
    face: &'static CraftFont,
    seen: std::collections::HashSet<char>,
    glyphs: Vec<Type3Glyph>,
    positions: Vec<usize>,
}

/// Shape in the source or an explicitly registered matching full face. Never use interface fonts.
fn shape_source(
    doc: &Document,
    fonts: &Dict,
    name: &str,
    text: &str,
) -> Result<Option<(Vec<u8>, pdfcraft_fonts::source_shaping::ShapedRun, String)>, EditError> {
    if !pdfcraft_fonts::source_shaping::required(text) {
        return Ok(None);
    }
    let source =
        fonts.get(name.as_bytes()).and_then(|f| doc.dict(f)).ok_or_else(|| EditError::Invalid("the shaped text has no font resource".into()))?;
    let family = Metrics::from_dict(doc, &source).base_font;
    let mut candidates = vec![source];
    candidates.extend(
        fonts
            .iter()
            .filter_map(|(_, f)| doc.dict(f))
            .filter(|f| pdfcraft_fonts::embedding::face_key(&Metrics::from_dict(doc, f).base_font) == pdfcraft_fonts::embedding::face_key(&family)),
    );
    for font in candidates {
        if let Some(bytes) = pdfcraft_fonts::pdf::embedded_sfnt(doc, &font)
            && pdfcraft_fonts::embedding::covers(&bytes, text)
            && let Ok(shaped) = pdfcraft_fonts::source_shaping::shape(&bytes, text)
        {
            return Ok(Some((bytes, shaped, family)));
        }
    }
    Err(EditError::Invalid("complex text needs the complete source TrueType/OpenType face; load that font before editing".into()))
}

/// For each output row (a range of the draft's characters), whether it is exactly the source row
/// at the same position: the same characters, none of them inserted or changed by the edit.
fn untouched_rows(block: &TextBlock, draft: &str, rows: &[std::ops::Range<usize>]) -> Vec<bool> {
    let old: Vec<char> = block.editor_text.chars().collect();
    let new: Vec<char> = draft.chars().collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old.iter().rev().zip(new.iter().rev()).take(old.len().min(new.len()).saturating_sub(prefix)).take_while(|(a, b)| a == b).count();
    // The source character a draft character is, if the edit left it alone.
    let origin = |i: usize| {
        if i < prefix {
            Some(i)
        } else if i >= new.len().saturating_sub(suffix) {
            (old.len() + i).checked_sub(new.len())
        } else {
            None
        }
    };
    let mut source_rows = Vec::new();
    let mut start = 0;
    for (i, ch) in old.iter().enumerate() {
        if *ch == '\n' {
            source_rows.push(start..i);
            start = i + 1;
        }
    }
    source_rows.push(start..old.len());
    // A line break inside a source run would pair editor rows with the wrong visual rows.
    if source_rows.len() != block.rows.len() {
        return vec![false; rows.len()];
    }
    rows.iter()
        .enumerate()
        .map(|(r, range)| {
            source_rows.get(r).is_some_and(|source| {
                !range.is_empty() && range.len() == source.len() && range.clone().zip(source.clone()).all(|(i, j)| origin(i) == Some(j))
            })
        })
        .collect()
}

/// Rewrite visual rows while keeping the source style on each surviving character. Resource
/// names may differ between Unicode subsets of the same face; encoding searches those subsets.
pub(crate) fn rewrite_styled_block(
    doc: &mut Document,
    page: usize,
    block: &TextBlock,
    lines: &[TextLine],
    text: Option<&str>,
    style: &BlockStyle,
    max_glyphs: usize,
) -> Result<LineEdit, EditError> {
    let first = block.lines.first().and_then(|i| lines.get(*i)).ok_or_else(|| EditError::Invalid("the paragraph has no source runs".into()))?;
    let draft = match text {
        Some(t) if t != block.text => t,
        _ => &block.editor_text,
    };
    if draft.len() > 4_000_000 {
        return Err(EditError::Invalid("the replacement text is too large".into()));
    }
    if draft == block.editor_text && *style == BlockStyle::default() {
        return Ok(LineEdit { substituted: None });
    }
    let mut ranges: Vec<_> = style.ranges.iter().collect();
    ranges.sort_by_key(|r| r.range.start);
    if ranges.len() > 1024
        || ranges.iter().any(|r| {
            r.range.start >= r.range.end
                || draft.get(r.range.clone()).is_none()
                || [r.size, r.char_spacing, r.scale].iter().flatten().any(|n| !n.is_finite())
                || r.size.is_some_and(|n| n <= 0.0 || n > 14400.0)
                || r.color.is_some_and(|c| c.iter().any(|n| !n.is_finite() || !(0.0..=1.0).contains(n)))
        })
        || ranges.windows(2).any(|r| r[0].range.end > r[1].range.start)
    {
        return Err(EditError::Invalid("text style ranges must be disjoint UTF-8 boundaries with finite formatting (at most 1024 ranges)".into()));
    }
    let source_count = lines.len();
    let mut styled_lines = lines.to_vec();
    let mut styled_spans = Vec::new();
    let mut underlines = HashMap::new();
    let mut overrides = HashMap::new();
    for (span, source) in block.edited_spans(draft) {
        let mut edges = vec![span.start, span.end];
        for r in &ranges {
            for edge in [r.range.start, r.range.end] {
                if span.contains(&edge) {
                    edges.push(edge);
                }
            }
        }
        edges.sort_unstable();
        edges.dedup();
        for bounds in edges.windows(2) {
            let (Some(start), Some(end)) = (bounds.first().copied(), bounds.get(1).copied()) else { continue };
            let mut index = source;
            let selected = ranges.iter().find(|r| r.range.contains(&start)).copied();
            if selected.is_some() || style.family.is_some() || style.bold.is_some() {
                let default_range = RangeStyle {
                    range: start..end,
                    family: None,
                    resource: None,
                    size: None,
                    color: None,
                    underline: None,
                    char_spacing: None,
                    scale: None,
                };
                let r = selected.unwrap_or(&default_range);
                let mut line = lines.get(source).unwrap_or(first).clone();
                if let Some((family, bold, italic)) =
                    r.family.or(style.family).or_else(|| style.bold.map(|b| (source_family(&line.base_font), b, line.italic)))
                {
                    let base = family.base_font(bold, italic);
                    line.font = format!("PCRange{}", base.replace('-', ""));
                    line.base_font = base.to_owned();
                    line.bold = bold;
                    line.italic = italic;
                }
                if let Some(resource) = &r.resource {
                    line.font = resource.clone();
                }
                if let Some(size) = r.size {
                    line.size = size;
                }
                if let Some(color) = r.color {
                    line.color = color;
                    line.origin.state.fill = vec![Op::new("rg", color.map(Object::Real).to_vec())];
                }
                if let Some(spacing) = r.char_spacing {
                    line.origin.state.char_spacing = spacing / line.origin.k.max(1e-6);
                }
                if let Some(scale) = r.scale {
                    line.origin.state.scale = (scale / 100.0).clamp(0.1, 10.0);
                }
                index = styled_lines.len();
                styled_lines.push(line);
                if let Some(r) = selected {
                    overrides.insert(index, r);
                }
                if let Some(underline) = r.underline {
                    underlines.insert(index, underline);
                }
            }
            styled_spans.push((start..end, index));
        }
    }
    let lines = styled_lines.as_slice();
    let p = page_dict(doc, page)?;
    let mut res = p.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    let mut fonts = res.get(b"Font").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    for line in lines.iter().filter(|line| line.font.starts_with("PCRange")) {
        let mut f = Dict::new();
        f.set(b"Type".to_vec(), Object::name("Font"));
        f.set(b"Subtype".to_vec(), Object::name("Type1"));
        f.set(b"BaseFont".to_vec(), Object::name(&line.base_font));
        f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        fonts.set(line.font.as_bytes().to_vec(), Object::Dict(f));
    }
    let mut metrics = std::collections::BTreeMap::new();
    for (name, object) in fonts.iter().take(256) {
        if let Some(font) = doc.resolve(object).as_dict() {
            metrics.insert(String::from_utf8_lossy(name).into_owned(), Metrics::from_dict(doc, font));
        }
    }
    for index in &block.lines {
        if let Some(line) = lines.get(*index)
            && !metrics.contains_key(&line.font)
            && let Some(font) = fonts.get(line.font.as_bytes()).and_then(|f| doc.resolve(f).as_dict().cloned())
        {
            metrics.insert(line.font.clone(), Metrics::from_dict(doc, &font));
        }
    }
    if [style.size, style.line_spacing, style.char_spacing, style.scale, style.width].iter().flatten().any(|v| !v.is_finite())
        || style.color.is_some_and(|c| c.iter().any(|v| !v.is_finite()))
    {
        return Err(EditError::Invalid("the paragraph's formatting must use finite numbers".into()));
    }
    let [dx, dy] = style.offset.unwrap_or([0.0, 0.0]);
    if !dx.is_finite() || !dy.is_finite() || style.width.is_some_and(|w| !w.is_finite()) {
        return Err(EditError::Invalid("the paragraph's position and width must be finite".into()));
    }
    // A visual row grows even when it contains several source/font runs. Multi-row
    // paragraphs retain their container width; an explicit width always wins.
    let natural = if block.invisible || block.rows.len() == 1 {
        (p.crop(doc)[2] - 36.0 - (block.rect[0] + dx)).max(block.rect[2] - block.rect[0])
    } else {
        block.rect[2] - block.rect[0]
    };
    let width = style.width.unwrap_or(natural).clamp(1.0, 14_400.0);
    // Each tuple is a character, its source style and an encodable resource name.
    let mut chars = Vec::new();
    let mut substituted = None;
    let mut fallback_groups: Vec<FallbackGroup> = Vec::new();
    let mut fallback_names: std::collections::BTreeMap<(String, bool, char), usize> = std::collections::BTreeMap::new();
    let mut fallback_glyphs = 0_usize;
    let per_font = max_glyphs.clamp(1, MAX_TYPE3_GLYPHS);
    let fallback_budget = per_font.saturating_mul(MAX_STYLED_FALLBACK_BATCHES);
    let mut full_faces: HashMap<String, String> = HashMap::new();
    let mut programs: HashMap<String, Option<std::sync::Arc<Vec<u8>>>> = HashMap::new();
    for (range, index) in styled_spans {
        let source = lines.get(index).unwrap_or(first);
        let start = range.start;
        for (offset, ch) in draft.get(range).unwrap_or_default().char_indices() {
            if ch == '\n' {
                chars.push((ch, index, source.font.clone()));
                continue;
            }
            // A carriage return breaks the row too. Before `\n` the pair is one break, so the
            // CR stays a `\r` the rows skip. Either way it keeps its place among the draft's
            // characters, which rows are counted in.
            if ch == '\r' {
                let crlf = start.checked_add(offset).and_then(|at| draft.get(at.saturating_add(1)..)).is_some_and(|rest| rest.starts_with('\n'));
                chars.push((if crlf { '\r' } else { '\n' }, index, source.font.clone()));
                continue;
            }
            let s = ch.to_string();
            let name = if let Some(resource) = overrides.get(&index).and_then(|r| r.resource.as_ref()).or(style.resource.as_ref()) {
                if !metrics.get(resource).is_some_and(|m| source_encodes(m, &s)) {
                    return Err(EditError::Invalid(format!("the selected font resource {resource} cannot show U+{:04X}", u32::from(ch))));
                }
                resource.clone()
            } else if metrics.get(&source.font).is_some_and(|m| source_encodes(m, &s)) {
                source.font.clone()
            } else if let Some((name, _)) = metrics.iter().find(|(_, m)| {
                pdfcraft_fonts::embedding::face_key(&m.base_font) == pdfcraft_fonts::embedding::face_key(&source.base_font) && source_encodes(m, &s)
            }) {
                name.clone()
            } else if let Some(face) =
                pdfcraft_fonts::embedding::matching_faces(&source.base_font).into_iter().find(|f| pdfcraft_fonts::embedding::covers(f.bytes, &s))
            {
                let key = face.name();
                if let Some(name) = full_faces.get(&key) {
                    name.clone()
                } else {
                    let name = pdfcraft_fonts::embedding::embed(doc, &mut fonts, face.bytes, face.family).map_err(EditError::Invalid)?;
                    let font = fonts
                        .get(name.as_bytes())
                        .and_then(|f| doc.dict(f))
                        .ok_or_else(|| EditError::Invalid("the full source font could not be loaded".into()))?;
                    metrics.insert(name.clone(), Metrics::from_dict(doc, &font));
                    full_faces.insert(key, name.clone());
                    name
                }
            } else if let Some(bytes) = programs
                .entry(source.font.clone())
                .or_insert_with(|| source_program(doc, &fonts, &source.font).map(std::sync::Arc::new))
                .clone()
                .filter(|bytes| pdfcraft_fonts::embedding::covers(bytes, &s))
            {
                // The document's own complete program of this font has the glyph: draw it with
                // that face rather than another family's fallback.
                let key = format!("source {}", source.font);
                if let Some(name) = full_faces.get(&key) {
                    name.clone()
                } else {
                    let family = pdfcraft_fonts::embedding::names(&bytes).map_or_else(|_| source.base_font.clone(), |(family, _)| family);
                    let name = pdfcraft_fonts::embedding::embed(doc, &mut fonts, &bytes, &family).map_err(EditError::Invalid)?;
                    let font = fonts
                        .get(name.as_bytes())
                        .and_then(|f| doc.dict(f))
                        .ok_or_else(|| EditError::Invalid("the source font could not be loaded".into()))?;
                    metrics.insert(name.clone(), Metrics::from_dict(doc, &font));
                    full_faces.insert(key, name.clone());
                    name
                }
            } else {
                let family = source_family(&source.base_font);
                if needs_type3(&s) {
                    let key = (source.base_font.clone(), source.bold, ch);
                    let group_index = if let Some(index) = fallback_names.get(&key) {
                        *index
                    } else {
                        // Keep the old per-character face choice: a serif Japanese character
                        // and a Cyrillic character may need different faces even in one style.
                        // Hebrew uses the separate RTL path before reaching this writer.
                        let (faces, _) = type3_faces(&s, family, source.bold)?;
                        let face =
                            fallback_face(&faces, &[ch], |face, ch| japanese_glyph_from(face, ch).is_ok()).ok_or_else(|| no_fallback_font(&s))?;
                        let index = fallback_groups.iter().position(|group| std::ptr::eq(group.face, face)).unwrap_or_else(|| {
                            let index = fallback_groups.len();
                            fallback_groups.push(FallbackGroup { face, seen: Default::default(), glyphs: Vec::new(), positions: Vec::new() });
                            index
                        });
                        fallback_names.insert(key, index);
                        index
                    };
                    let group = fallback_groups.get_mut(group_index).ok_or_else(|| EditError::Invalid("the fallback face is missing".into()))?;
                    if group.seen.insert(ch) {
                        if fallback_glyphs >= fallback_budget {
                            return Err(EditError::Invalid(format!(
                                "the replacement needs more than {fallback_budget} fallback glyphs; edit a smaller paragraph or load a complete font"
                            )));
                        }
                        let (path, width) = type3_glyph(group.face, ch)?.ok_or_else(|| no_glyph_for(group.face, ch))?;
                        let code = u8::try_from((group.glyphs.len() % per_font).saturating_add(1))
                            .map_err(|_| EditError::Invalid("replacement text has too many unique characters".into()))?;
                        group.glyphs.push(Type3Glyph { code, ch, path, width });
                        fallback_glyphs = fallback_glyphs.saturating_add(1);
                    }
                    group.positions.push(chars.len());
                    // Filled from the prepared font/code map before measuring or drawing.
                    String::new()
                } else {
                    let base = family.base_font(source.bold, source.italic);
                    let name = format!("PCEd{}", base.replace('-', ""));
                    let mut f = Dict::new();
                    f.set(b"Type".to_vec(), Object::name("Font"));
                    f.set(b"Subtype".to_vec(), Object::name("Type1"));
                    f.set(b"BaseFont".to_vec(), Object::name(base));
                    f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
                    metrics.entry(name.clone()).or_insert_with(|| Metrics::from_dict(doc, &f));
                    fonts.set(name.clone().into_bytes(), Object::Dict(f));
                    substituted = Some(base.to_string());
                    name
                }
            };
            chars.push((ch, index, name));
        }
    }
    // All fallback outlines have now succeeded. Batch by actual face, preserving source
    // style indices; the existing writer then coalesces adjacent equal-font/style characters.
    for group in fallback_groups {
        let fallback = write_type3_glyphs(doc, &mut fonts, group.face, false, group.glyphs, per_font);
        for name in &fallback.names {
            let font = fonts
                .get(name.as_bytes())
                .and_then(|f| doc.dict(f))
                .ok_or_else(|| EditError::Invalid("the fallback font could not be loaded".into()))?;
            metrics.insert(name.clone(), Metrics::from_dict(doc, &font));
        }
        for position in group.positions {
            let item = chars.get_mut(position).ok_or_else(|| EditError::Invalid("the fallback character is missing".into()))?;
            let (font, _) = fallback.codes.get(&item.0).ok_or_else(|| EditError::Invalid("the fallback glyph is missing".into()))?;
            item.2 = fallback.names.get(*font).cloned().ok_or_else(|| EditError::Invalid("the fallback font is missing".into()))?;
        }
        substituted = Some(fallback.family.to_owned());
    }
    let size_of = |source: &TextLine, index: usize| {
        overrides.get(&index).and_then(|r| r.size).or(style.size).unwrap_or(source.size).max(0.1) / source.origin.k.max(1e-6)
    };
    // A source run may carry TJ kerning/justification that is absent from its text state.
    // Keep its measured advance when rebuilding the run instead of making it wrap on entry.
    // Each font's space width, worked out once: when a font can't show a space, finding one
    // reads every font on the page, and doing that for every character of a paragraph made each
    // keystroke in a long paragraph take half a second (#2616).
    // Only for the fonts the paragraph uses, when first asked: working out every page font's width
    // up front would scan the page once per font for a short edit on a page of many fonts.
    let spaces: std::cell::RefCell<HashMap<String, f64>> = std::cell::RefCell::new(HashMap::new());
    let space_of = |name: &str, m: &Metrics| {
        if let Some(w) = spaces.borrow().get(name) {
            return *w;
        }
        let w = source_space(doc, &fonts, m);
        spaces.borrow_mut().insert(name.to_owned(), w);
        w
    };
    let mut calibration = HashMap::new();
    for (index, source) in lines.iter().enumerate() {
        if let Some(m) = metrics.get(&source.font) {
            let nominal = source_width(
                m,
                &source.text,
                source.origin.state.font.as_ref().map_or(1.0, |(_, s)| *s),
                space_of(&source.font, m),
                source.origin.state.char_spacing,
                source.origin.state.word_spacing,
            ) * source.origin.state.scale
                * across(source);
            let changed_face = style.resource.is_some() || overrides.get(&index).is_some_and(|r| r.family.is_some() || r.resource.is_some());
            let factor = if nominal > 0.0 && !changed_face { ((source.rect[2] - source.rect[0]) / nominal).clamp(0.5, 2.0) } else { 1.0 };
            calibration.insert(index, factor);
        }
    }
    let advance = |item: &(char, usize, String)| -> f64 {
        let source = lines.get(item.1).unwrap_or(first);
        let Some(m) = metrics.get(&item.2) else { return 0.0 };
        source_width(
            m,
            &item.0.to_string(),
            size_of(source, item.1),
            space_of(&item.2, m),
            overrides
                .get(&item.1)
                .and_then(|r| r.char_spacing)
                .or(style.char_spacing)
                .map_or(source.origin.state.char_spacing, |c| c / source.origin.k.max(1e-6)),
            source.origin.state.word_spacing,
        ) * overrides.get(&item.1).and_then(|r| r.scale).or(style.scale).map_or(source.origin.state.scale, |s| (s / 100.0).clamp(0.1, 10.0))
            * across(source)
            * calibration.get(&item.1).copied().unwrap_or(1.0)
    };
    let measure = |items: &[(char, usize, String)]| -> Result<f64, EditError> {
        let mut width = 0.0;
        let mut at = 0;
        while let Some(item) = items.get(at) {
            let mut end = at + 1;
            while items.get(end).is_some_and(|v| v.1 == item.1 && v.2 == item.2) {
                end += 1;
            }
            let segment = items.get(at..end).unwrap_or_default();
            let string: String = segment.iter().map(|v| v.0).collect();
            if let Some((_, shaped, _)) = shape_source(doc, &fonts, &item.2, &string)? {
                let source = lines.get(item.1).unwrap_or(first);
                let scale =
                    overrides.get(&item.1).and_then(|r| r.scale).or(style.scale).map_or(source.origin.state.scale, |s| (s / 100.0).clamp(0.1, 10.0));
                let spacing = overrides
                    .get(&item.1)
                    .and_then(|r| r.char_spacing)
                    .or(style.char_spacing)
                    .map_or(source.origin.state.char_spacing, |s| s / source.origin.k.max(1e-6));
                width += (shaped.advance * size_of(source, item.1)
                    + spacing * string.chars().count() as f64
                    + source.origin.state.word_spacing * string.matches(' ').count() as f64)
                    * scale
                    * source.origin.k;
            } else {
                width += segment.iter().map(&advance).sum::<f64>();
            }
            at = end;
        }
        Ok(width)
    };
    let mut rows: Vec<Vec<(char, usize, String)>> = Vec::new();
    // Each row's characters, as indexes into the draft's characters (`chars` holds all of them).
    let mut row_ranges = Vec::new();
    let mut row_start = 0;
    let mut row = Vec::new();
    let mut row_width = 0.0;
    // Wrap at word boundaries, retaining explicit source row breaks and whitespace.
    let mut at = 0;
    while at < chars.len() {
        let Some(item) = chars.get(at) else { break };
        // The CR of a CRLF: the `\n` after it breaks the row.
        if item.0 == '\r' {
            at += 1;
            continue;
        }
        if item.0 == '\n' {
            row_ranges.push(row_start..row_start + row.len());
            rows.push(std::mem::take(&mut row));
            row_width = 0.0;
            at += 1;
            row_start = at;
            continue;
        }
        let mut end = at + 1;
        if !item.0.is_whitespace() {
            // A word ends at a space, or where Chinese or Japanese text may break between characters.
            while let (Some(before), Some(c)) = (chars.get(end - 1), chars.get(end))
                && !c.0.is_whitespace()
                && !crate::linebreak::between(before.0, c.0)
            {
                end += 1;
            }
        }
        let word = chars.get(at..end).unwrap_or_default();
        let w = measure(word)?;
        if !row.is_empty() && !item.0.is_whitespace() && row_width + w > width + 0.5 {
            while row.last().is_some_and(|c: &(char, usize, String)| c.0.is_whitespace()) {
                row.pop();
            }
            row_ranges.push(row_start..row_start + row.len());
            rows.push(std::mem::take(&mut row));
            row_width = 0.0;
            row_start = at;
        }
        row.extend_from_slice(word);
        row_width += w;
        at = end;
    }
    if !row.is_empty() {
        row_ranges.push(row_start..row_start + row.len());
        rows.push(row);
    }
    let streams = content_streams(doc, &p.dict)?;
    let pieces = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    // A row the edit doesn't reach keeps its source operators, so its kerning, TJ word gaps and
    // glyph widths stay exactly as they were instead of being rebuilt from the text state.
    let plain = BlockStyle { ranges: Vec::new(), ..style.clone() } == BlockStyle::default();
    let mut kept = if plain {
        untouched_rows(block, draft, &row_ranges).into_iter().zip(&rows).map(|(same, row)| same && row.iter().all(|c| c.1 < source_count)).collect()
    } else {
        vec![false; rows.len()]
    };
    let kept_lines: std::collections::HashSet<usize> =
        kept.iter().enumerate().filter(|(_, k)| **k).filter_map(|(r, _)| block.rows.get(r)).flatten().copied().collect();
    let rewritten: Vec<&TextLine> = block.lines.iter().filter(|i| !kept_lines.contains(i)).filter_map(|i| lines.get(*i)).collect();
    // The rewrite goes where the last rewritten row was drawn, after any backgrounds painted
    // between the rows. A dropped `'` or `"` also moved to the next line, which a kept row relies on.
    let lead =
        rewritten.last().copied().filter(|_| !rewritten.iter().flat_map(|l| &l.ops).any(|i| ops.get(*i).is_some_and(|op| op.is("'") || op.is("\""))));
    let (members, lead) = match lead {
        Some(lead) if !kept_lines.is_empty() => (rewritten, lead),
        _ => {
            kept.iter_mut().for_each(|k| *k = false);
            let all: Vec<&TextLine> = block.lines.iter().filter_map(|i| lines.get(*i)).collect();
            let last = all.last().copied().unwrap_or(first);
            (all, last)
        }
    };
    let o = &lead.origin;
    let mut drop = coincident_ops(lines, &members);
    drop.extend(members.iter().flat_map(|l| l.ops.iter().copied()));
    let start = lead.ops.first().copied().unwrap_or(o.bt_op);
    let split = ops
        .get(o.bt_op..start)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .any(|(i, op)| matches!(op.op.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"") && !drop.contains(&(o.bt_op + i)));
    // What is in force where the rewrite is drawn, and what the text after it expects.
    let after = if split { &o.state } else { &o.bt_state };
    for (row, _) in rows.iter().zip(&kept).filter(|(_, k)| !**k) {
        let text: String = row.iter().map(|v| v.0).collect();
        if pdfcraft_fonts::source_shaping::required(&text) && row.first().is_some_and(|first| row.iter().any(|v| v.1 != first.1 || v.2 != first.2)) {
            return Err(EditError::Invalid(
                "contextual/bidi text across font or style boundaries needs a uniform full-face row; mixed contextual runs are not yet supported"
                    .into(),
            ));
        }
    }
    let measured: Vec<_> = rows.iter().map(|r| measure(r)).collect::<Result<_, _>>()?;
    let mut shaped_runs = HashMap::new();
    let mut segment_advances = HashMap::new();
    for (row_index, row) in rows.iter().enumerate().filter(|(r, _)| !kept.get(*r).copied().unwrap_or(false)) {
        let mut at = 0;
        while let Some(item) = row.get(at) {
            let mut end = at + 1;
            while row.get(end).is_some_and(|v| v.1 == item.1 && v.2 == item.2) {
                end += 1;
            }
            segment_advances.insert((row_index, at), row.get(at..end).unwrap_or_default().iter().map(&advance).sum::<f64>());
            let string: String = row.get(at..end).unwrap_or_default().iter().map(|v| v.0).collect();
            if let Some((bytes, shaped, family)) = shape_source(doc, &fonts, &item.2, &string)? {
                shaped_runs.insert((row_index, at), (bytes, shaped, family));
            }
            at = end;
        }
    }
    let mut glyph_resources = HashMap::new();
    for (key, (bytes, shaped, family)) in &shaped_runs {
        let mappings: Vec<_> = shaped.glyphs.iter().map(|g| (g.text.clone(), g.id)).collect();
        let resource = pdfcraft_fonts::embedding::embed_mapping(doc, &mut fonts, bytes, family, &mappings).map_err(EditError::Invalid)?;
        glyph_resources.insert(*key, resource);
    }
    let n = pdfcraft_content::num;
    let mut block_ops = vec![Op::new("q", vec![])];
    block_ops.extend(reveal_ops(block, lines, style, o.ctm)?);
    let back = Matrix([o.ctm[0], o.ctm[1], o.ctm[2], o.ctm[3], 0.0, 0.0])
        .invert()
        .ok_or_else(|| EditError::Invalid("the paragraph is drawn in a space it cannot be edited in".into()))?;
    let (tx, ty) = back.apply(dx, dy);
    block_ops.push(Op::new("cm", vec![n(1.0), n(0.0), n(0.0), n(1.0), n(tx), n(ty)]));
    let crop = p.crop(doc);
    let centered = is_centred(block, lines, (crop[0] + crop[2]) / 2.0, first.size);
    let source_leading = block
        .rows
        .windows(2)
        .filter_map(|rows| {
            let a = rows[0].first().and_then(|i| lines.get(*i))?;
            let b = rows[1].first().and_then(|i| lines.get(*i))?;
            Some((a.origin.baseline - b.origin.baseline).abs())
        })
        .rfind(|v| *v > 0.0)
        .unwrap_or(first.size * 1.5);
    let mut baseline = first.origin.baseline;
    for (row_index, row) in rows.iter().enumerate() {
        let anchor = block.rows.get(row_index).and_then(|r| r.first()).and_then(|i| lines.get(*i));
        if style.line_spacing.is_none()
            && style.size.is_none()
            && let Some(anchor) = anchor
        {
            baseline = anchor.origin.baseline;
        }
        let anchor = anchor.unwrap_or(first);
        if kept.get(row_index).copied().unwrap_or(false) {
            // Kept rows only occur without spacing or size changes, so this is the step below.
            baseline -= source_leading;
            continue;
        }
        let w = measured.get(row_index).copied().unwrap_or(0.0);
        let align = style.align.unwrap_or(if centered { crate::added::Align::Center } else { crate::added::Align::Left });
        let original_rect = block.rows.get(row_index).and_then(|row| row_rect(row, lines)).unwrap_or(block.rect);
        let left = if style.width.is_none() && style.align.is_none() { original_rect[0] } else { block.rect[0] };
        let available = if style.width.is_none() && style.align.is_none() { original_rect[2] - original_rect[0] } else { width };
        let mut x = left
            + match align {
                crate::added::Align::Center => (available - w) / 2.0,
                crate::added::Align::Right => available - w,
                _ => 0.0,
            };
        let mut pos = 0;
        while pos < row.len() {
            let Some(item) = row.get(pos) else { break };
            let source = lines.get(item.1).unwrap_or(first);
            let mut end = pos + 1;
            while row.get(end).is_some_and(|other| other.1 == item.1 && other.2 == item.2) {
                end += 1;
            }
            let segment = row.get(pos..end).unwrap_or_default();
            let string: String = segment.iter().map(|c| c.0).collect();
            let m = metrics.get(&item.2).ok_or_else(|| EditError::Invalid("the paragraph font is missing".into()))?;
            let mut state = source.origin.state.clone();
            state.font = Some((item.2.clone(), size_of(source, item.1)));
            if let Some(c) = overrides.get(&item.1).and_then(|r| r.char_spacing).or(style.char_spacing) {
                state.char_spacing = c / source.origin.k.max(1e-6);
            }
            if let Some(s) = overrides.get(&item.1).and_then(|r| r.scale).or(style.scale) {
                state.scale = (s / 100.0).clamp(0.1, 10.0);
            }
            if !shaped_runs.contains_key(&(row_index, pos)) {
                state.scale *= calibration.get(&item.1).copied().unwrap_or(1.0);
            }
            if let Some([r, g, b]) = overrides.get(&item.1).and_then(|r| r.color).or(style.color) {
                state.fill = vec![Op::new("rg", vec![n(r), n(g), n(b)])];
            }
            let inverse = Matrix(o.ctm).invert().ok_or_else(|| EditError::Invalid("the paragraph has a singular transform".into()))?;
            let mut tm = Matrix(source.origin.tm).then(&Matrix(source.origin.ctm)).then(&inverse).0;
            let (ax, ay) = back.apply(x - source.origin.x, baseline - source.origin.baseline);
            tm[4] += ax;
            tm[5] += ay;
            let mut segment_width = segment_advances.get(&(row_index, pos)).copied().unwrap_or(0.0);
            // Each run in its own graphics state, restating what its source set inside its text
            // object (`gs`, stroke, line width, `Tr`; #3508/#3509) from the state the rewrite is
            // drawn in: a `gs` has no inverse, so one run's ExtGState would stay for the next.
            block_ops.push(Op::new("q", vec![]));
            block_ops.extend(state.paint_ops(after));
            if let Some((_, shaped, _)) = shaped_runs.get(&(row_index, pos)) {
                let resource = glyph_resources.get(&(row_index, pos)).ok_or_else(|| EditError::Invalid("shaped font is missing".into()))?;
                let mut properties = Dict::new();
                properties.set(b"ActualText".to_vec(), Object::String(PdfString::text(&string)));
                block_ops.push(Op::new("BDC", vec![Object::name("Span"), Object::Dict(properties)]));
                state.font = Some((resource.clone(), size_of(source, item.1)));
                // One text object for the run: each glyph is placed where shaping put it by a TJ
                // adjustment from where the glyph before it ends (its advance in the embedded
                // font's widths), and raised by the text rise for a mark set above or below
                // (ISO 32000-1 §9.4.3, §9.3.7). Font-program codes are separate from Unicode;
                // ToUnicode and ActualText keep the logical text.
                let size = size_of(source, item.1);
                let per_unit = 1000.0 / size;
                if !per_unit.is_finite() {
                    return Err(EditError::Invalid("the shaped text has no size".into()));
                }
                let shown = fonts
                    .get(resource.as_bytes())
                    .and_then(|f| doc.resolve(f).as_dict().cloned())
                    .map(|d| Metrics::from_dict(doc, &d))
                    .ok_or_else(|| EditError::Invalid("shaped font is missing".into()))?;
                // Spacing is part of each glyph's position, so the run itself has none.
                let (char_spacing, word_spacing, rise) = (state.char_spacing, state.word_spacing, state.rise);
                state.char_spacing = 0.0;
                state.word_spacing = 0.0;
                block_ops.push(Op::new("BT", vec![]));
                block_ops.extend(state.ops());
                block_ops.push(Op::new("Tm", tm.iter().map(|v| n(*v)).collect()));
                // `pen`: where the next glyph would start, in thousandths of the font size.
                let (mut pen, mut extra, mut raised) = (0.0, 0.0, rise);
                let mut shown_glyphs: Vec<Object> = Vec::new();
                for (i, glyph) in shaped.glyphs.iter().enumerate() {
                    let target = (glyph.position[0] * size + extra) * per_unit;
                    let glyph_rise = rise + glyph.position[1] * size;
                    if (glyph_rise - raised).abs() > 1e-9 || shown_glyphs.len() >= 4096 {
                        if !shown_glyphs.is_empty() {
                            block_ops.push(Op::new("TJ", vec![Object::Array(std::mem::take(&mut shown_glyphs))]));
                        }
                        if (glyph_rise - raised).abs() > 1e-9 {
                            block_ops.push(Op::new("Ts", vec![n(glyph_rise)]));
                            raised = glyph_rise;
                        }
                    }
                    if (target - pen).abs() > 1e-6 {
                        shown_glyphs.push(n(pen - target));
                    }
                    let code = u16::try_from(i + 1).map_err(|_| EditError::Invalid("shaped glyph limit".into()))?;
                    shown_glyphs.push(Object::String(PdfString::literal(code.to_be_bytes().to_vec())));
                    pen = target + shown.width(u32::from(code)) * 1000.0;
                    extra += char_spacing * glyph.text.chars().count() as f64 + word_spacing * glyph.text.matches(' ').count() as f64;
                }
                if !shown_glyphs.is_empty() {
                    block_ops.push(Op::new("TJ", vec![Object::Array(shown_glyphs)]));
                }
                if (raised - rise).abs() > 1e-9 {
                    block_ops.push(Op::new("Ts", vec![n(rise)]));
                }
                block_ops.push(Op::new("ET", vec![]));
                (state.char_spacing, state.word_spacing) = (char_spacing, word_spacing);
                block_ops.push(Op::new("EMC", vec![]));
                segment_width = (shaped.advance * size + extra) * state.scale * source.origin.k;
            } else {
                block_ops.push(Op::new("BT", vec![]));
                block_ops.extend(state.ops());
                block_ops.push(Op::new("Tm", tm.iter().map(|v| n(*v)).collect()));
                block_ops.extend(source_show(
                    m,
                    &string,
                    size_of(source, item.1),
                    spaces.borrow().get(&item.2).copied().unwrap_or_else(|| source_space(doc, &fonts, m)),
                    state.char_spacing,
                    state.word_spacing,
                )?);
                block_ops.push(Op::new("ET", vec![]));
            }
            if underlines.get(&item.1).copied().or(style.underline) == Some(true) {
                let [r, g, b] = overrides.get(&item.1).and_then(|r| r.color).or(style.color).unwrap_or(source.color);
                let underline_size = size_of(source, item.1) * source.origin.k;
                let (x0, y0) = back.apply(x - o.ctm[4], baseline - o.ctm[5] - underline_size * 0.12);
                let (x1, y1) = back.apply(x + segment_width - o.ctm[4], baseline - o.ctm[5] - underline_size * 0.12);
                block_ops.push(Op::new("RG", vec![n(r), n(g), n(b)]));
                block_ops.push(Op::new("w", vec![n(underline_size * 0.06 / o.ctm[2].hypot(o.ctm[3]).max(1e-6))]));
                block_ops.push(Op::new("m", vec![n(x0), n(y0)]));
                block_ops.push(Op::new("l", vec![n(x1), n(y1)]));
                block_ops.push(Op::new("S", vec![]));
            }
            block_ops.push(Op::new("Q", vec![]));
            x += segment_width;
            pos = end;
        }
        baseline -= if style.line_spacing.is_none() && style.size.is_none() {
            source_leading
        } else {
            style.line_spacing.unwrap_or(1.5).clamp(0.5, 5.0) * style.size.unwrap_or(anchor.size)
        };
    }
    block_ops.push(Op::new("Q", vec![]));
    block_ops.extend(after.ops());
    if split {
        block_ops.insert(0, Op::new("ET", vec![]));
        block_ops.push(Op::new("BT", vec![]));
        block_ops.push(Op::new("Tm", o.tlm.iter().map(|v| n(*v)).collect()));
    }
    let at = if split { start } else { o.bt_op };
    let data = splice(&pieces, &ops, |i| (if i == at { std::mem::take(&mut block_ops) } else { Vec::new() }, !drop.contains(&i)));
    let contents = rewritten_contents(doc, &streams, data);
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(p.obj, |d| {
        d.set(b"Contents".to_vec(), contents);
        d.set(b"Resources".to_vec(), Object::Dict(res));
    })?;
    Ok(LineEdit { substituted })
}
