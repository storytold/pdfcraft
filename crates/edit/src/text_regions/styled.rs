use super::grouping::{across, is_centred, row_rect};
use super::{
    BlockStyle, CraftFont, Dict, Document, EditError, HashMap, LineEdit, MAX_TYPE3_GLYPHS, Matrix, Metrics, Object, Op, PdfString, Pieces,
    RangeStyle, TextBlock, TextLine, Type3Glyph, coincident_ops, content_streams, fallback_face, japanese_glyph_from, needs_type3, no_fallback_font,
    no_glyph_for, page_dict, reveal_ops, rewritten_contents, source_encodes, source_family, source_program, source_show, source_space, source_width,
    splice, type3_faces, type3_glyph, write_type3_glyphs,
};

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

struct FallbackBuilder {
    groups: Vec<FallbackGroup>,
    names: std::collections::BTreeMap<(String, bool, char), usize>,
    glyphs: usize,
    per_font: usize,
    budget: usize,
}

struct FontResolver<'a> {
    doc: &'a mut Document,
    fonts: &'a mut Dict,
    metrics: &'a mut std::collections::BTreeMap<String, Metrics>,
    full_faces: HashMap<String, String>,
    programs: HashMap<String, Option<std::sync::Arc<Vec<u8>>>>,
}

impl FontResolver<'_> {
    fn encodable_font(&mut self, source: &TextLine, text: &str, requested: Option<&str>) -> Result<Option<String>, EditError> {
        if let Some(resource) = requested {
            if !self.metrics.get(resource).is_some_and(|metrics| source_encodes(metrics, text)) {
                return Err(EditError::Invalid(format!(
                    "the selected font resource {resource} cannot show U+{:04X}",
                    text.chars().next().map_or(0, u32::from)
                )));
            }
            return Ok(Some(resource.to_owned()));
        }
        if self.metrics.get(&source.font).is_some_and(|metrics| source_encodes(metrics, text)) {
            return Ok(Some(source.font.clone()));
        }
        if let Some((name, _)) = self.metrics.iter().find(|(_, metrics)| {
            pdfcraft_fonts::embedding::face_key(&metrics.base_font) == pdfcraft_fonts::embedding::face_key(&source.base_font)
                && source_encodes(metrics, text)
        }) {
            return Ok(Some(name.clone()));
        }
        if let Some(face) =
            pdfcraft_fonts::embedding::matching_faces(&source.base_font).into_iter().find(|face| pdfcraft_fonts::embedding::covers(face.bytes, text))
        {
            return self.embed_face(face.name(), face.bytes, face.family, "the full source font could not be loaded").map(Some);
        }
        let Some(bytes) = self
            .programs
            .entry(source.font.clone())
            .or_insert_with(|| source_program(self.doc, self.fonts, &source.font).map(std::sync::Arc::new))
            .clone()
            .filter(|bytes| pdfcraft_fonts::embedding::covers(bytes, text))
        else {
            return Ok(None);
        };
        let key = format!("source {}", source.font);
        if let Some(name) = self.full_faces.get(&key) {
            return Ok(Some(name.clone()));
        }
        let family = pdfcraft_fonts::embedding::names(&bytes).map_or_else(|_| source.base_font.clone(), |(family, _)| family);
        self.embed_face(key, &bytes, &family, "the source font could not be loaded").map(Some)
    }

    fn embed_face(&mut self, key: String, bytes: &[u8], family: &str, missing_message: &str) -> Result<String, EditError> {
        if let Some(name) = self.full_faces.get(&key) {
            return Ok(name.clone());
        }
        let name = pdfcraft_fonts::embedding::embed(self.doc, self.fonts, bytes, family).map_err(EditError::Invalid)?;
        let font =
            self.fonts.get(name.as_bytes()).and_then(|resource| self.doc.dict(resource)).ok_or_else(|| EditError::Invalid(missing_message.into()))?;
        self.metrics.insert(name.clone(), Metrics::from_dict(self.doc, &font));
        self.full_faces.insert(key, name.clone());
        Ok(name)
    }

    fn substitute_font(&mut self, source: &TextLine) -> String {
        let family = source_family(&source.base_font);
        let base = family.base_font(source.bold, source.italic);
        let name = format!("PCEd{}", base.replace('-', ""));
        let mut font = Dict::new();
        font.set(b"Type".to_vec(), Object::name("Font"));
        font.set(b"Subtype".to_vec(), Object::name("Type1"));
        font.set(b"BaseFont".to_vec(), Object::name(base));
        font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        self.metrics.entry(name.clone()).or_insert_with(|| Metrics::from_dict(self.doc, &font));
        self.fonts.set(name.clone().into_bytes(), Object::Dict(font));
        name
    }
}

struct PreparedCharacters {
    values: Vec<StyledChar>,
    substituted: Option<String>,
}

struct CharacterPreparation<'a> {
    doc: &'a mut Document,
    first: &'a TextLine,
    lines: &'a [TextLine],
    draft: &'a str,
    spans: Vec<(std::ops::Range<usize>, usize)>,
    style: &'a BlockStyle,
    overrides: &'a HashMap<usize, &'a RangeStyle>,
    fonts: &'a mut Dict,
    metrics: &'a mut std::collections::BTreeMap<String, Metrics>,
    max_glyphs: usize,
}

impl CharacterPreparation<'_> {
    fn prepare(self) -> Result<PreparedCharacters, EditError> {
        let Self { doc, first, lines, draft, spans, style, overrides, fonts, metrics, max_glyphs } = self;
        let mut chars = Vec::new();
        let mut substituted = None;
        let mut fallback = FallbackBuilder::new(max_glyphs);
        let mut resolver = FontResolver { doc, fonts, metrics, full_faces: HashMap::new(), programs: HashMap::new() };
        for (range, index) in spans {
            let source = lines.get(index).unwrap_or(first);
            let start = range.start;
            for (offset, ch) in draft.get(range).unwrap_or_default().char_indices() {
                if ch == '\n' {
                    chars.push((ch, index, source.font.clone()));
                    continue;
                }
                if ch == '\r' {
                    let crlf = start.checked_add(offset).and_then(|at| draft.get(at.saturating_add(1)..)).is_some_and(|rest| rest.starts_with('\n'));
                    chars.push((if crlf { '\r' } else { '\n' }, index, source.font.clone()));
                    continue;
                }
                let text = ch.to_string();
                let requested = overrides.get(&index).and_then(|range| range.resource.as_deref()).or(style.resource.as_deref());
                let name = if let Some(name) = resolver.encodable_font(source, &text, requested)? {
                    name
                } else if needs_type3(&text) {
                    fallback.prepare(source, ch, &text, chars.len())?
                } else {
                    let name = resolver.substitute_font(source);
                    substituted = Some(source_family(&source.base_font).base_font(source.bold, source.italic).to_owned());
                    name
                };
                chars.push((ch, index, name));
            }
        }
        drop(resolver);
        for group in fallback.groups {
            let written = write_type3_glyphs(doc, fonts, group.face, false, group.glyphs, fallback.per_font);
            for name in &written.names {
                let font = fonts
                    .get(name.as_bytes())
                    .and_then(|resource| doc.dict(resource))
                    .ok_or_else(|| EditError::Invalid("the fallback font could not be loaded".into()))?;
                metrics.insert(name.clone(), Metrics::from_dict(doc, &font));
            }
            for position in group.positions {
                let item = chars.get_mut(position).ok_or_else(|| EditError::Invalid("the fallback character is missing".into()))?;
                let (font, _) = written.codes.get(&item.0).ok_or_else(|| EditError::Invalid("the fallback glyph is missing".into()))?;
                item.2 = written.names.get(*font).cloned().ok_or_else(|| EditError::Invalid("the fallback font is missing".into()))?;
            }
            substituted = Some(written.family.to_owned());
        }
        Ok(PreparedCharacters { values: chars, substituted })
    }
}

impl FallbackBuilder {
    fn new(max_glyphs: usize) -> Self {
        let per_font = max_glyphs.clamp(1, MAX_TYPE3_GLYPHS);
        Self {
            groups: Vec::new(),
            names: std::collections::BTreeMap::new(),
            glyphs: 0,
            per_font,
            budget: per_font.saturating_mul(MAX_STYLED_FALLBACK_BATCHES),
        }
    }

    fn prepare(&mut self, source: &TextLine, ch: char, text: &str, position: usize) -> Result<String, EditError> {
        let key = (source.base_font.clone(), source.bold, ch);
        let group_index = if let Some(index) = self.names.get(&key) {
            *index
        } else {
            let (faces, _) = type3_faces(text, source_family(&source.base_font), source.bold)?;
            let face = fallback_face(&faces, &[ch], |face, ch| japanese_glyph_from(face, ch).is_ok()).ok_or_else(|| no_fallback_font(text))?;
            let index = self.groups.iter().position(|group| std::ptr::eq(group.face, face)).unwrap_or_else(|| {
                let index = self.groups.len();
                self.groups.push(FallbackGroup { face, seen: Default::default(), glyphs: Vec::new(), positions: Vec::new() });
                index
            });
            self.names.insert(key, index);
            index
        };
        let group = self.groups.get_mut(group_index).ok_or_else(|| EditError::Invalid("the fallback face is missing".into()))?;
        if group.seen.insert(ch) {
            if self.glyphs >= self.budget {
                return Err(EditError::Invalid(format!(
                    "the replacement needs more than {} fallback glyphs; edit a smaller paragraph or load a complete font",
                    self.budget
                )));
            }
            let (path, width) = type3_glyph(group.face, ch)?.ok_or_else(|| no_glyph_for(group.face, ch))?;
            let code = u8::try_from((group.glyphs.len() % self.per_font).saturating_add(1))
                .map_err(|_| EditError::Invalid("replacement text has too many unique characters".into()))?;
            group.glyphs.push(Type3Glyph { code, ch, path, width });
            self.glyphs = self.glyphs.saturating_add(1);
        }
        group.positions.push(position);
        Ok(String::new())
    }
}

struct StyledLines<'a> {
    lines: Vec<TextLine>,
    spans: Vec<(std::ops::Range<usize>, usize)>,
    underlines: HashMap<usize, bool>,
    overrides: HashMap<usize, &'a RangeStyle>,
}

type StyledChar = (char, usize, String);
type StyledRows = Vec<Vec<StyledChar>>;
type RowRanges = Vec<std::ops::Range<usize>>;
type ShapedRunCache = HashMap<(usize, usize), (Vec<u8>, pdfcraft_fonts::source_shaping::ShapedRun, String)>;

struct ShapedRows {
    runs: ShapedRunCache,
    advances: HashMap<(usize, usize), f64>,
}

fn prepare_shaped_rows(
    doc: &Document,
    fonts: &Dict,
    rows: &[Vec<StyledChar>],
    kept: &[bool],
    advance: impl Fn(&StyledChar) -> f64,
) -> Result<ShapedRows, EditError> {
    let mut runs = HashMap::new();
    let mut advances = HashMap::new();
    for (row_index, row) in rows.iter().enumerate().filter(|(index, _)| !kept.get(*index).copied().unwrap_or(false)) {
        let mut at = 0;
        while let Some(item) = row.get(at) {
            let mut end = at + 1;
            while row.get(end).is_some_and(|next| next.1 == item.1 && next.2 == item.2) {
                end += 1;
            }
            let segment = row.get(at..end).unwrap_or_default();
            advances.insert((row_index, at), segment.iter().map(&advance).sum::<f64>());
            let text: String = segment.iter().map(|character| character.0).collect();
            if let Some((bytes, shaped, family)) = shape_source(doc, fonts, &item.2, &text)? {
                runs.insert((row_index, at), (bytes, shaped, family));
            }
            at = end;
        }
    }
    Ok(ShapedRows { runs, advances })
}

fn embed_shaped_resources(doc: &mut Document, fonts: &mut Dict, runs: &ShapedRunCache) -> Result<HashMap<(usize, usize), String>, EditError> {
    let mut resources = HashMap::new();
    for (key, (bytes, shaped, family)) in runs {
        let mappings: Vec<_> = shaped.glyphs.iter().map(|glyph| (glyph.text.clone(), glyph.id)).collect();
        let resource = pdfcraft_fonts::embedding::embed_mapping(doc, fonts, bytes, family, &mappings).map_err(EditError::Invalid)?;
        resources.insert(*key, resource);
    }
    Ok(resources)
}

fn validate_contextual_rows(rows: &[Vec<StyledChar>], kept: &[bool]) -> Result<(), EditError> {
    for (row, _) in rows.iter().zip(kept).filter(|(_, retained)| !**retained) {
        let text: String = row.iter().map(|character| character.0).collect();
        if pdfcraft_fonts::source_shaping::required(&text)
            && row.first().is_some_and(|first| row.iter().any(|character| character.1 != first.1 || character.2 != first.2))
        {
            return Err(EditError::Invalid(
                "contextual/bidi text across font or style boundaries needs a uniform full-face row; mixed contextual runs are not yet supported"
                    .into(),
            ));
        }
    }
    Ok(())
}

fn calibration_factors(
    lines: &[TextLine],
    metrics: &std::collections::BTreeMap<String, Metrics>,
    style: &BlockStyle,
    overrides: &HashMap<usize, &RangeStyle>,
    space_of: impl Fn(&str, &Metrics) -> f64,
) -> HashMap<usize, f64> {
    let mut calibration = HashMap::new();
    for (index, source) in lines.iter().enumerate() {
        if let Some(metrics) = metrics.get(&source.font) {
            let nominal = source_width(
                metrics,
                &source.text,
                source.origin.state.font.as_ref().map_or(1.0, |(_, size)| *size),
                space_of(&source.font, metrics),
                source.origin.state.char_spacing,
                source.origin.state.word_spacing,
            ) * source.origin.state.scale
                * across(source);
            let changed_face =
                style.resource.is_some() || overrides.get(&index).is_some_and(|range| range.family.is_some() || range.resource.is_some());
            let factor = if nominal > 0.0 && !changed_face { ((source.rect[2] - source.rect[0]) / nominal).clamp(0.5, 2.0) } else { 1.0 };
            calibration.insert(index, factor);
        }
    }
    calibration
}

fn validated_ranges<'a>(draft: &str, style: &'a BlockStyle) -> Result<Vec<&'a RangeStyle>, EditError> {
    let mut ranges: Vec<_> = style.ranges.iter().collect();
    ranges.sort_by_key(|range| range.range.start);
    let invalid_range = ranges.iter().any(|range| {
        range.range.start >= range.range.end
            || draft.get(range.range.clone()).is_none()
            || [range.size, range.char_spacing, range.scale].iter().flatten().any(|value| !value.is_finite())
            || range.size.is_some_and(|value| value <= 0.0 || value > 14_400.0)
            || range.color.is_some_and(|color| color.iter().any(|value| !value.is_finite() || !(0.0..=1.0).contains(value)))
    });
    if ranges.len() > 1024 || invalid_range || ranges.windows(2).any(|pair| pair[0].range.end > pair[1].range.start) {
        return Err(EditError::Invalid("text style ranges must be disjoint UTF-8 boundaries with finite formatting (at most 1024 ranges)".into()));
    }
    Ok(ranges)
}

fn replacement_text<'a>(block: &'a TextBlock, text: Option<&'a str>) -> Result<&'a str, EditError> {
    let draft = match text {
        Some(text) if text != block.text => text,
        _ => &block.editor_text,
    };
    if draft.len() > 4_000_000 {
        return Err(EditError::Invalid("the replacement text is too large".into()));
    }
    Ok(draft)
}

fn has_no_effective_style_change(block: &TextBlock, draft: &str, style: &BlockStyle) -> bool {
    draft == block.editor_text && *style == BlockStyle::default()
}

fn page_font_metrics(
    doc: &Document,
    resource_dict: &Dict,
    block: &TextBlock,
    lines: &[TextLine],
) -> (Dict, std::collections::BTreeMap<String, Metrics>) {
    let mut fonts = resource_dict.get(b"Font").and_then(|resource| doc.resolve(resource).as_dict().cloned()).unwrap_or_default();
    for line in lines.iter().filter(|line| line.font.starts_with("PCRange")) {
        let mut font = Dict::new();
        font.set(b"Type".to_vec(), Object::name("Font"));
        font.set(b"Subtype".to_vec(), Object::name("Type1"));
        font.set(b"BaseFont".to_vec(), Object::name(&line.base_font));
        font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        fonts.set(line.font.as_bytes().to_vec(), Object::Dict(font));
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
            && let Some(font) = fonts.get(line.font.as_bytes()).and_then(|resource| doc.resolve(resource).as_dict().cloned())
        {
            metrics.insert(line.font.clone(), Metrics::from_dict(doc, &font));
        }
    }
    (fonts, metrics)
}

fn paragraph_geometry(doc: &Document, page: &pdfcraft_model::Page, block: &TextBlock, style: &BlockStyle) -> Result<(f64, f64, f64), EditError> {
    if [style.size, style.line_spacing, style.char_spacing, style.scale, style.width].iter().flatten().any(|value| !value.is_finite())
        || style.color.is_some_and(|color| color.iter().any(|value| !value.is_finite()))
    {
        return Err(EditError::Invalid("the paragraph's formatting must use finite numbers".into()));
    }
    let [dx, dy] = style.offset.unwrap_or([0.0, 0.0]);
    if !dx.is_finite() || !dy.is_finite() || style.width.is_some_and(|width| !width.is_finite()) {
        return Err(EditError::Invalid("the paragraph's position and width must be finite".into()));
    }
    let natural = if block.invisible || block.rows.len() == 1 {
        (page.crop(doc)[2] - 36.0 - (block.rect[0] + dx)).max(block.rect[2] - block.rect[0])
    } else {
        block.rect[2] - block.rect[0]
    };
    Ok((dx, dy, style.width.unwrap_or(natural).clamp(1.0, 14_400.0)))
}

type WrappedStyledRows = (StyledRows, RowRanges, Vec<usize>, Vec<bool>, Vec<usize>);

fn wrap_styled_rows(
    chars: &[StyledChar],
    width: f64,
    measure: impl Fn(&[StyledChar]) -> Result<f64, EditError>,
) -> Result<WrappedStyledRows, EditError> {
    let mut rows: Vec<Vec<StyledChar>> = Vec::new();
    let mut row_ranges = Vec::new();
    // True when this row starts because the previous word did not fit, not because of a newline.
    let mut row_soft = Vec::new();
    let mut next_soft = false;
    // How many separator characters a wrap trimmed off the row that just ended, for the row
    // about to start (0 for a Chinese/Japanese word split, which trims none; meaningless unless
    // the row it belongs to is soft). Mirrors `next_soft`.
    let mut row_gap = Vec::new();
    let mut next_gap = 0usize;
    // Logical draft rows drive per-paragraph alignment independently of soft wraps.
    let mut row_lines: Vec<usize> = Vec::new();
    let mut draft_line = 0usize;
    let mut row_start = 0;
    let mut row = Vec::new();
    let mut row_width = 0.0;
    let mut at = 0;
    while at < chars.len() {
        let Some(item) = chars.get(at) else { break };
        if item.0 == '\r' {
            at += 1;
            continue;
        }
        if item.0 == '\n' {
            row_soft.push(next_soft);
            next_soft = false;
            row_gap.push(next_gap);
            next_gap = 0;
            row_ranges.push(row_start..row_start + row.len());
            row_lines.push(draft_line);
            draft_line = draft_line.saturating_add(1);
            rows.push(std::mem::take(&mut row));
            row_width = 0.0;
            at += 1;
            row_start = at;
            continue;
        }
        let mut end = at + 1;
        if !item.0.is_whitespace() {
            // A word ends at a space, or where Chinese or Japanese text may break between characters.
            while let (Some(before), Some(current)) = (chars.get(end - 1), chars.get(end))
                && !current.0.is_whitespace()
                && !crate::linebreak::between(before.0, current.0)
            {
                end += 1;
            }
        }
        let word = chars.get(at..end).unwrap_or_default();
        let word_width = measure(word)?;
        if !row.is_empty() && !item.0.is_whitespace() && row_width + word_width > width + 0.5 {
            let mut trimmed = 0usize;
            while row.last().is_some_and(|c: &StyledChar| c.0.is_whitespace()) {
                row.pop();
                trimmed += 1;
            }
            row_soft.push(next_soft);
            next_soft = true;
            row_gap.push(next_gap);
            next_gap = trimmed;
            row_ranges.push(row_start..row_start + row.len());
            row_lines.push(draft_line);
            rows.push(std::mem::take(&mut row));
            row_width = 0.0;
            row_start = at;
        }
        row.extend_from_slice(word);
        row_width += word_width;
        at = end;
    }
    if !row.is_empty() {
        row_soft.push(next_soft);
        row_gap.push(next_gap);
        row_ranges.push(row_start..row_start + row.len());
        row_lines.push(draft_line);
        rows.push(row);
    }
    Ok((rows, row_ranges, row_lines, row_soft, row_gap))
}

fn apply_range_styles<'a>(
    first: &TextLine,
    block: &TextBlock,
    lines: &[TextLine],
    draft: &str,
    ranges: &[&'a RangeStyle],
    style: &BlockStyle,
) -> StyledLines<'a> {
    let mut styled_lines = lines.to_vec();
    let mut styled_spans = Vec::new();
    let mut underlines = HashMap::new();
    let mut overrides = HashMap::new();
    for (span, source) in block.edited_spans(draft) {
        let mut edges = vec![span.start, span.end];
        for range in ranges {
            for edge in [range.range.start, range.range.end] {
                if span.contains(&edge) {
                    edges.push(edge);
                }
            }
        }
        edges.sort_unstable();
        edges.dedup();
        for bounds in edges.windows(2) {
            let (Some(start), Some(end)) = (bounds.first().copied(), bounds.get(1).copied()) else { continue };
            let selected = ranges.iter().find(|range| range.range.contains(&start)).copied();
            let mut index = source;
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
                let range = selected.unwrap_or(&default_range);
                let mut line = lines.get(source).unwrap_or(first).clone();
                if let Some((family, bold, italic)) =
                    range.family.or(style.family).or_else(|| style.bold.map(|b| (source_family(&line.base_font), b, line.italic)))
                {
                    let base = family.base_font(bold, italic);
                    line.font = format!("PCRange{}", base.replace('-', ""));
                    line.base_font = base.to_owned();
                    line.bold = bold;
                    line.italic = italic;
                }
                if let Some(resource) = &range.resource {
                    line.font = resource.clone();
                }
                if let Some(size) = range.size {
                    line.size = size;
                }
                if let Some(color) = range.color {
                    line.color = color;
                    line.origin.state.fill = vec![Op::new("rg", color.map(Object::Real).to_vec())];
                }
                if let Some(spacing) = range.char_spacing {
                    line.origin.state.char_spacing = spacing / line.origin.k.max(1e-6);
                }
                if let Some(scale) = range.scale {
                    line.origin.state.scale = (scale / 100.0).clamp(0.1, 10.0);
                }
                index = styled_lines.len();
                styled_lines.push(line);
                if let Some(range) = selected {
                    overrides.insert(index, range);
                }
                if let Some(underline) = range.underline {
                    underlines.insert(index, underline);
                }
            }
            styled_spans.push((start..end, index));
        }
    }
    StyledLines { lines: styled_lines, spans: styled_spans, underlines, overrides }
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

/// Operator index of the paragraph's first text object, and of the next line's text object
/// when one follows.
fn paragraph_span(block: &TextBlock, lines: &[TextLine]) -> Option<(usize, Option<usize>)> {
    let belongs: std::collections::HashSet<usize> = block.lines.iter().copied().collect();
    let start = block.lines.iter().filter_map(|i| lines.get(*i)).map(|line| line.origin.bt_op).min()?;
    let end =
        block.lines.iter().filter_map(|i| lines.get(*i)).flat_map(|line| line.ops.iter().copied().chain(std::iter::once(line.origin.bt_op))).max()?;
    let next = lines
        .iter()
        .enumerate()
        .filter(|(index, _)| !belongs.contains(index))
        .filter_map(|(_, line)| (line.origin.bt_op > end).then_some(line.origin.bt_op))
        .min();
    Some((start, next))
}

/// The scope shared by every line of the paragraph, when each one is in the same marked region.
fn shared_scope(block: &TextBlock, lines: &[TextLine]) -> Option<usize> {
    let mut scopes = block.lines.iter().filter_map(|i| lines.get(*i)).map(|line| line.scope);
    let first = scopes.next().flatten()?;
    scopes.all(|scope| scope == Some(first)).then_some(first)
}

/// Where an extra `/P` marker belongs.
///
/// The marker is omitted when the paragraph is already one marked region and the next line is
/// in a different one. Otherwise it closes after the paragraph's last `ET` (`ops.len()` means
/// append it after the stream). Closing at the next line's `BT` is too late when that line's
/// region opened with `BDC` before the `BT`: the `EMC` would end the neighbour's region. Closing
/// inside the rewritten operators leaves a kept source row outside the marker.
///
/// A neighbour in the same text object has no `BT` of its own. Closing after the shared `ET`
/// would take that neighbour into the marker. The marker then opens after `BT` and closes
/// before the neighbour's text, so `BT` and `ET` stay matched.
fn paragraph_marker(block: &TextBlock, lines: &[TextLine], ops: &[Op]) -> Option<(usize, Option<usize>, usize)> {
    let (start, next) = paragraph_span(block, lines)?;
    if let Some(region) = shared_scope(block, lines)
        && let Some(at) = next
    {
        let next_scope = lines.iter().find(|line| line.origin.bt_op == at).map(|line| line.scope);
        if next_scope != Some(Some(region)) {
            return None;
        }
    }
    let belongs: std::collections::HashSet<usize> = block.lines.iter().copied().collect();
    let end =
        block.lines.iter().filter_map(|i| lines.get(*i)).flat_map(|line| line.ops.iter().copied().chain(std::iter::once(line.origin.bt_op))).max()?;
    let after_text = ops
        .iter()
        .enumerate()
        .skip(end.saturating_add(1))
        .find(|(_, op)| op.op.as_slice() == b"ET")
        .map(|(index, _)| index.saturating_add(1))
        .unwrap_or(ops.len());
    let neighbour = lines
        .iter()
        .enumerate()
        .filter(|(index, _)| !belongs.contains(index))
        .filter_map(|(_, line)| line.ops.iter().copied().filter(|op| *op > end).min())
        .min();
    let open = block.lines.iter().filter_map(|i| lines.get(*i)).flat_map(|line| line.ops.iter().copied()).min()?;
    if let Some(op) = neighbour.filter(|op| *op < after_text) {
        // `BT` stays outside the marker. Both ends are inside the one text object.
        if open > start {
            return Some((open, None, op));
        }
    }
    // The last paragraph of a shared text object has no neighbour after it. Opening at the
    // shared `BT` would take the preceding paragraph into the marker. Open at this paragraph's
    // first operator instead, and close before the shared `ET`, so both ends stay inside it.
    let preceding =
        lines.iter().enumerate().filter(|(index, _)| !belongs.contains(index)).any(|(_, line)| line.ops.iter().any(|op| *op > start && *op < open));
    if preceding {
        if let Some(et) = after_text.checked_sub(1).filter(|index| ops.get(*index).is_some_and(|op| op.op.as_slice() == b"ET"))
            && open > start
            && open < et
        {
            return Some((open, None, et));
        }
        return None;
    }
    // `None` is the next-line index: the close is after this paragraph's `ET`, not at the
    // following line's `BT`.
    Some((start, None, after_text))
}

struct RewriteAnchor<'a> {
    lead: &'a TextLine,
    drop: std::collections::HashSet<usize>,
    start: usize,
    split: bool,
}

fn rewrite_anchor<'a>(
    block: &TextBlock,
    lines: &'a [TextLine],
    first: &'a TextLine,
    ops: &[Op],
    kept_lines: &std::collections::HashSet<usize>,
    kept: &mut [bool],
) -> RewriteAnchor<'a> {
    let rewritten: Vec<&TextLine> = block.lines.iter().filter(|index| !kept_lines.contains(index)).filter_map(|index| lines.get(*index)).collect();
    let lead = rewritten
        .last()
        .copied()
        .filter(|_| !rewritten.iter().flat_map(|line| &line.ops).any(|index| ops.get(*index).is_some_and(|op| op.is("'") || op.is("\""))));
    let (members, lead) = match lead {
        Some(lead) if !kept_lines.is_empty() => (rewritten, lead),
        _ => {
            kept.iter_mut().for_each(|value| *value = false);
            let all: Vec<&TextLine> = block.lines.iter().filter_map(|index| lines.get(*index)).collect();
            let last = all.last().copied().unwrap_or(first);
            (all, last)
        }
    };
    let origin = &lead.origin;
    let mut drop = coincident_ops(lines, &members);
    drop.extend(members.iter().flat_map(|line| line.ops.iter().copied()));
    let start = lead.ops.first().copied().unwrap_or(origin.bt_op);
    let split = ops
        .get(origin.bt_op..start)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .any(|(index, op)| matches!(op.op.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"") && !drop.contains(&(origin.bt_op + index)));
    RewriteAnchor { lead, drop, start, split }
}

/// Open (`op` is `"BMC"`) or close (`"EMC"`) the `/P` marker that wraps the whole rewrite when
/// `marked` (every row is in this rewrite, see [`rewrite_styled_block`]), so the paragraph
/// cannot be mistaken for a neighbour by geometry alone (#2517).
fn push_whole_paragraph_marker(block_ops: &mut Vec<Op>, marked: bool, op: &str) {
    if !marked {
        return;
    }
    let operands = if op == "BMC" { vec![Object::name("P")] } else { Vec::new() };
    block_ops.push(Op::new(op, operands));
}

/// An empty marker between two rows of one rewrite. `/PCHard` is a newline that was already in
/// the text. `/PCSoft` is a wrap and rejoins when the box widens, with `/N` the exact number of
/// separator characters it consumed (0 for a Chinese/Japanese word split), so a rejoin restores
/// what was actually there instead of guessing a single space (#2529). Neither is a paragraph
/// region, so the lines stay in one scope.
fn push_row_break_marker(block_ops: &mut Vec<Op>, row_soft: &[bool], row_gap: &[usize], row_index: usize) {
    if row_soft.get(row_index).copied().unwrap_or(false) {
        let mut properties = Dict::new();
        let gap = row_gap.get(row_index).copied().unwrap_or(1);
        properties.set(b"N".to_vec(), Object::Int(i64::try_from(gap).unwrap_or(i64::MAX)));
        block_ops.push(Op::new("BDC", vec![Object::name("PCSoft"), Object::Dict(properties)]));
    } else {
        block_ops.push(Op::new("BMC", vec![Object::name("PCHard")]));
    }
    block_ops.push(Op::new("EMC", vec![]));
}

/// [`splice`] with the rewritten rows spliced in at `at`, and (when `marker` names one) a `/P`
/// marker opened at its start and closed at its `next` line or, when nothing follows, after the
/// paragraph's own last `ET` (appended past the stream's last operator when that close is at or
/// beyond `ops.len()`, since `splice`'s callback never visits that index).
fn splice_with_marker(
    pieces: &Pieces,
    ops: &[Op],
    marker: Option<(usize, Option<usize>, usize)>,
    at: usize,
    mut block_ops: Vec<Op>,
    drop: &std::collections::HashSet<usize>,
) -> Vec<Vec<u8>> {
    let data = splice(pieces, ops, |i| {
        let mut insert = Vec::new();
        if let Some((start_at, next, close)) = marker {
            if i == start_at {
                insert.push(Op::new("BMC", vec![Object::name("P")]));
            }
            if next.is_none() && i == close {
                insert.push(Op::new("EMC", vec![]));
            }
            if next == Some(i) {
                insert.push(Op::new("EMC", vec![]));
            }
        }
        if i == at {
            insert.append(&mut block_ops);
        }
        (insert, !drop.contains(&i))
    });
    if !marker.is_some_and(|(_, next, close)| next.is_none() && close >= ops.len()) {
        return data;
    }
    let mut data = data;
    if let Some(last) = data.last_mut() {
        if last.last().is_some_and(|byte| !byte.is_ascii_whitespace()) {
            last.push(b'\n');
        }
        // `splice` never visits `ops.len()`, so a close after the final operator is appended.
        last.extend_from_slice(&pdfcraft_content::serialize_ops(&[Op::new("EMC", vec![])]));
    }
    data
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
    let draft = replacement_text(block, text)?;
    if has_no_effective_style_change(block, draft, style) {
        return Ok(LineEdit { substituted: None });
    }
    let ranges = validated_ranges(draft, style)?;
    let source_count = lines.len();
    let styled = apply_range_styles(first, block, lines, draft, &ranges, style);
    let lines = styled.lines.as_slice();
    let styled_spans = styled.spans;
    let underlines = styled.underlines;
    let overrides = styled.overrides;
    let p = page_dict(doc, page)?;
    let mut res = p.dict.get(b"Resources").and_then(|resource| doc.resolve(resource).as_dict().cloned()).unwrap_or_default();
    let (mut fonts, mut metrics) = page_font_metrics(doc, &res, block, lines);
    let (dx, dy, width) = paragraph_geometry(doc, &p, block, style)?;
    let prepared = CharacterPreparation {
        doc,
        first,
        lines,
        draft,
        spans: styled_spans,
        style,
        overrides: &overrides,
        fonts: &mut fonts,
        metrics: &mut metrics,
        max_glyphs,
    }
    .prepare()?;
    let chars = prepared.values;
    let substituted = prepared.substituted;
    // The size in text space. A line's size is in points, always positive; a source that sets a
    // negative `Tf` (mirrored glyphs under a flipped CTM) keeps its sign, or the text would flip.
    let size_of = |source: &TextLine, index: usize| {
        let points = overrides.get(&index).and_then(|r| r.size).or(style.size).unwrap_or(source.size).max(0.1);
        (points / source.origin.k.max(1e-6)).copysign(source.origin.state.font.as_ref().map_or(1.0, |(_, size)| *size))
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
    let calibration = calibration_factors(lines, &metrics, style, &overrides, space_of);
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
    let (rows, row_ranges, row_lines, row_soft, row_gap) = wrap_styled_rows(&chars, width, measure)?;
    let streams = content_streams(doc, &p.dict)?;
    let pieces = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    // A row the edit doesn't reach keeps its source operators, so its kerning, TJ word gaps and
    // glyph widths stay exactly as they were instead of being rebuilt from the text state. A row
    // aligned on its own is reached; the rows around it are not.
    let plain = BlockStyle { ranges: Vec::new(), aligns: Vec::new(), ..style.clone() } == BlockStyle::default();
    let mut kept = if plain {
        untouched_rows(block, draft, &row_ranges)
            .into_iter()
            .zip(&rows)
            .zip(&row_lines)
            .map(|((same, row), line)| same && row.iter().all(|c| c.1 < source_count) && style.aligns.get(*line).copied().flatten().is_none())
            .collect()
    } else {
        vec![false; rows.len()]
    };
    let kept_lines: std::collections::HashSet<usize> =
        kept.iter().enumerate().filter(|(_, k)| **k).filter_map(|(r, _)| block.rows.get(r)).flatten().copied().collect();
    // The rewrite is placed after the last rewritten row unless a text-show operator there
    // would make a kept following row depend on that position.
    let anchor = rewrite_anchor(block, lines, first, &ops, &kept_lines, &mut kept);
    let RewriteAnchor { lead, drop, start, split } = anchor;
    let o = &lead.origin;
    // What is in force where the rewrite is drawn, and what the text after it expects.
    let after = if split { &o.state } else { &o.bt_state };
    validate_contextual_rows(&rows, &kept)?;
    let measured: Vec<_> = rows.iter().map(|row| measure(row)).collect::<Result<_, _>>()?;
    let shaped = prepare_shaped_rows(doc, &fonts, &rows, &kept, advance)?;
    let ShapedRows { runs: shaped_runs, advances: segment_advances } = shaped;
    let glyph_resources = embed_shaped_resources(doc, &mut fonts, &shaped_runs)?;
    let n = pdfcraft_content::num;
    let mut block_ops = vec![Op::new("q", vec![])];
    block_ops.extend(reveal_ops(block, lines, style, o.ctm)?);
    let back = Matrix([o.ctm[0], o.ctm[1], o.ctm[2], o.ctm[3], 0.0, 0.0])
        .invert()
        .ok_or_else(|| EditError::Invalid("the paragraph is drawn in a space it cannot be edited in".into()))?;
    let (tx, ty) = back.apply(dx, dy);
    block_ops.push(Op::new("cm", vec![n(1.0), n(0.0), n(0.0), n(1.0), n(tx), n(ty)]));
    // A new row can land one leading above the next paragraph and look like the same
    // paragraph. When every row is in this rewrite, a paragraph marker keeps them in one
    // scope so the neighbour stays out (#2517). A row left in the source stream cannot share
    // that marker, so it is omitted and geometry groups those rows as before.
    let marked = kept.iter().all(|row| !*row);
    push_whole_paragraph_marker(&mut block_ops, marked, "BMC");
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
        if !row.is_empty() && row_index > 0 {
            push_row_break_marker(&mut block_ops, &row_soft, &row_gap, row_index);
        }
        let w = measured.get(row_index).copied().unwrap_or(0.0);
        // The row's own alignment, or the paragraph's.
        let asked = row_lines.get(row_index).and_then(|l| style.aligns.get(*l)).copied().flatten().or(style.align);
        let align = asked.unwrap_or(if centered { crate::added::Align::Center } else { crate::added::Align::Left });
        let original_rect = block.rows.get(row_index).and_then(|row| row_rect(row, lines)).unwrap_or(block.rect);
        let left = if style.width.is_none() && asked.is_none() { original_rect[0] } else { block.rect[0] };
        let available = if style.width.is_none() && asked.is_none() { original_rect[2] - original_rect[0] } else { width };
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
                let underline_size = size_of(source, item.1).abs() * source.origin.k;
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
    push_whole_paragraph_marker(&mut block_ops, marked, "EMC");
    block_ops.push(Op::new("Q", vec![]));
    block_ops.extend(after.ops());
    if split {
        block_ops.insert(0, Op::new("ET", vec![]));
        block_ops.push(Op::new("BT", vec![]));
        block_ops.push(Op::new("Tm", o.tlm.iter().map(|v| n(*v)).collect()));
    }
    let at = if split { start } else { o.bt_op };
    // A kept source row sits outside `block_ops`. Bound the whole paragraph when a marker is
    // safe. The close is not part of `block_ops`: that vector can end inside a text object
    // when the rewrite splits one, and a kept row's operators come later in the stream.
    let marker = if marked { None } else { paragraph_marker(block, lines, &ops) };
    let data = splice_with_marker(&pieces, &ops, marker, at, block_ops, &drop);
    let contents = rewritten_contents(doc, &streams, data);
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(p.obj, |d| {
        d.set(b"Contents".to_vec(), contents);
        d.set(b"Resources".to_vec(), Object::Dict(res));
    })?;
    Ok(LineEdit { substituted })
}
