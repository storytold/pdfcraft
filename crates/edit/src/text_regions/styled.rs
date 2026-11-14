use super::super::interp::TextState;
use super::grouping::{across, is_centred, row_rect};
use super::{
    BlockStyle, CraftFont, Dict, Document, EditError, HashMap, LineEdit, MAX_TYPE3_GLYPHS, Matrix, Metrics, Object, Op, PdfString, Pieces,
    RangeStyle, TextBlock, TextLine, Type3Glyph, builtin_thai_choice, coincident_ops, content_streams, fallback_face, japanese_glyph_from,
    needs_type3, no_fallback_font, no_glyph_for, page_dict, reveal_ops, rewritten_contents, row_break, source_encodes, source_family, source_program,
    source_show, source_space, source_width, splice, type3_faces, type3_glyph, write_type3_glyphs,
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

struct LegacyFace {
    /// Shared by every resource name of one font object: its program is decoded and kept once.
    glyphs: Option<std::rc::Rc<pdfcraft_fonts::pdf::LegacyThaiFont>>,
    metrics: Metrics,
    /// Weight and slant ([`FontStyle::of`]).
    style: FontStyle,
    /// The font object the resource names, when it is given by reference.
    object: Option<pdfcraft_cos::ObjRef>,
    /// Its text-space fallback ([`source_space`]), in ems.
    space: f64,
}

/// What one edit may decode of its THSarabunPSK subsets' font data: their programs (each at most
/// 16 MiB), which it keeps, and the streams of the metrics read to measure them and to adapt their
/// programs' cmaps: their CMaps and glyph maps, and a subset's program again, for which of its
/// glyphs have outlines. A page may name many of them. The metrics read to find the subsets among
/// the page's fonts aren't charged to it.
const MAX_THAI_PROGRAMS: usize = 64 << 20;

struct PreparedCharacters {
    values: Vec<StyledChar>,
    substituted: Option<String>,
    thai: ThaiShaping,
}

/// How the Thai spans of an edit are shaped, by the font name their characters carry.
#[derive(Default)]
struct ThaiShaping {
    /// Draft-only names drawn with the bundled Thai outlines.
    outlined: std::collections::HashSet<String>,
    /// Source faces shaped in their existing codes, by resource name.
    legacy: HashMap<String, LegacyFace>,
    /// Draft-only names whose rows, Latin ones included, are shaped and embedded with the
    /// complete program of this same-face resource: a span its subsets can't show in place.
    program: HashMap<String, String>,
}

/// How a Thai span keeps its source face ([`legacy_thai_font`]).
enum ThaiSpan {
    /// In this resource's existing codes.
    InPlace(String),
    /// Shaped by this same-face resource's complete program with its own layout.
    Program(String),
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
        let thai_choice = builtin_thai_choice(fonts);
        let mut thai = ThaiShaping::default();
        let mut thai_programs = pdfcraft_cos::DecodeBudget::new(MAX_THAI_PROGRAMS, 0);
        let mut chars = Vec::new();
        let mut substituted = None;
        let mut fallback = FallbackBuilder::new(max_glyphs);
        let mut resolver = FontResolver { doc, fonts, metrics, full_faces: HashMap::new(), programs: HashMap::new() };
        for (range, index) in spans {
            let source = lines.get(index).unwrap_or(first);
            let span = draft.get(range.clone()).unwrap_or_default();
            // Keep the exact source face. A different Thai face requires an explicit choice.
            // Interface fallback fonts never enter a PDF.
            let selected_resource = overrides.get(&index).and_then(|r| r.resource.as_deref()).or(style.resource.as_deref());
            let use_thai_outlines = if selected_resource == Some(thai_choice.as_str()) {
                let face = pdfcraft_fonts::document_thai_font().ok_or_else(|| {
                    EditError::Invalid(format!(
                        "the bundled Thai font ({thai_choice}) is not in this build, which has no craft-fonts Thai face; load a complete Thai font or choose one the page lists"
                    ))
                })?;
                if !covers_rows(face.bytes, span) {
                    let missing = span.chars().find(|&c| !row_break(c) && !pdfcraft_fonts::embedding::covers(face.bytes, c.encode_utf8(&mut [0; 4])));
                    return Err(EditError::Invalid(match missing {
                        Some(c) => format!("{} has no glyph for {c} (U+{:04X}); choose a font that covers the text", face.family, u32::from(c)),
                        None => format!("{} cannot show every character in this selection; choose a font that covers the text", face.family),
                    }));
                }
                substituted = Some(face.family.to_owned());
                true
            } else {
                selected_resource.is_none() && matching_thai_outline(resolver.doc, resolver.fonts, source, span)
            };
            if use_thai_outlines {
                substituted = pdfcraft_fonts::document_thai_font().map(|f| f.family.to_owned());
            }
            let existing_thai = if !use_thai_outlines {
                let chosen = match selected_resource {
                    None => Some(source.clone()),
                    Some(resource) => resolver.fonts.get(resource.as_bytes()).and_then(|f| resolver.doc.dict(f)).map(|font| {
                        let mut chosen = source.clone();
                        chosen.font = resource.to_owned();
                        chosen.base_font = Metrics::from_dict(resolver.doc, &font).base_font;
                        chosen
                    }),
                };
                match chosen {
                    Some(chosen) => legacy_thai_font(resolver.doc, resolver.fonts, &chosen, span, &mut thai.legacy, &mut thai_programs)?,
                    None => None,
                }
            } else {
                None
            };
            let outline_name = if use_thai_outlines {
                // A draft-only name prevents this choice from affecting another style span that
                // explicitly selects the original resource. It is never saved into the PDF.
                let mut name = format!("PCOutlineThai{index}");
                while resolver.metrics.contains_key(&name) {
                    name.push('_');
                }
                if let Some(m) = resolver
                    .metrics
                    .get(&source.font)
                    .or_else(|| source.origin.state.font.as_ref().and_then(|(name, _)| resolver.metrics.get(name)))
                    .cloned()
                {
                    resolver.metrics.insert(name.clone(), m);
                }
                thai.outlined.insert(name.clone());
                Some(name)
            } else {
                None
            };
            let (existing_thai, program_name) = match existing_thai {
                Some(ThaiSpan::InPlace(name)) => (Some(name), None),
                Some(ThaiSpan::Program(resource)) => {
                    // Draft-only too: another span may still show this resource's existing codes.
                    let mut name = format!("PCProgramThai{index}");
                    while resolver.metrics.contains_key(&name) {
                        name.push('_');
                    }
                    if let Some(m) = resolver.metrics.get(&source.font).cloned() {
                        resolver.metrics.insert(name.clone(), m);
                    }
                    thai.program.insert(name.clone(), resource);
                    (None, Some(name))
                }
                None => (None, None),
            };
            let start = range.start;
            for (offset, ch) in draft.get(range).unwrap_or_default().char_indices() {
                // A row break moves to the next row instead of asking a font for a glyph. The CR
                // of a CRLF stays a `\r` the rows skip, so the pair is one break; every other break
                // is written as `\n`. Either way it keeps its place among the draft's characters,
                // which rows are counted in.
                if row_break(ch) {
                    let crlf = ch == '\r'
                        && start.checked_add(offset).and_then(|at| draft.get(at.saturating_add(1)..)).is_some_and(|rest| rest.starts_with('\n'));
                    chars.push((if crlf { '\r' } else { '\n' }, index, source.font.clone()));
                    continue;
                }
                let text = ch.to_string();
                let requested = overrides.get(&index).and_then(|range| range.resource.as_deref()).or(style.resource.as_deref());
                let name = if let Some(name) = outline_name.as_ref().or(program_name.as_ref()) {
                    name.clone()
                } else if let Some(name) = &existing_thai {
                    name.clone()
                } else if let Some(name) = resolver.encodable_font(source, &text, requested)? {
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
        Ok(PreparedCharacters { values: chars, substituted, thai })
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

/// End of a contiguous run sharing both its style and font resource.
fn styled_run_end(items: &[StyledChar], start: usize) -> usize {
    let Some(first) = items.get(start) else { return start };
    let mut end = start + 1;
    while items.get(end).is_some_and(|next| next.1 == first.1 && next.2 == first.2) {
        end += 1;
    }
    end
}

type StyledRows = Vec<Vec<StyledChar>>;
type RowRanges = Vec<std::ops::Range<usize>>;
enum ShapedFont {
    Existing(Vec<(String, Vec<u8>)>),
    Program { bytes: Vec<u8>, family: String },
    ThaiOutlines,
}
struct ShapedText {
    run: pdfcraft_fonts::source_shaping::ShapedRun,
    font: ShapedFont,
}
type ShapedRunCache = HashMap<(usize, usize), ShapedText>;

struct ShapedRows {
    runs: ShapedRunCache,
    advances: HashMap<(usize, usize), f64>,
    char_advances: HashMap<(usize, usize), Vec<f64>>,
}

fn prepare_shaped_rows(
    doc: &Document,
    fonts: &Dict,
    thai: &ThaiShaping,
    rows: &[Vec<StyledChar>],
    kept: &[bool],
    advance: impl Fn(&StyledChar) -> f64,
) -> Result<ShapedRows, EditError> {
    let mut runs = HashMap::new();
    let mut advances = HashMap::new();
    let mut char_advances = HashMap::new();
    for (row_index, row) in rows.iter().enumerate().filter(|(index, _)| !kept.get(*index).copied().unwrap_or(false)) {
        let mut at = 0;
        while let Some(item) = row.get(at) {
            let end = styled_run_end(row, at);
            let segment = row.get(at..end).unwrap_or_default();
            let widths: Vec<f64> = segment.iter().map(&advance).collect();
            advances.insert((row_index, at), widths.iter().copied().sum());
            char_advances.insert((row_index, at), widths);
            let text: String = segment.iter().map(|character| character.0).collect();
            if let Some(shaped) = shape_source(doc, fonts, thai, &item.2, &text)? {
                runs.insert((row_index, at), shaped);
            }
            at = end;
        }
    }
    Ok(ShapedRows { runs, advances, char_advances })
}

type ShapedResources = HashMap<(usize, usize), Vec<(String, Vec<u8>)>>;

fn embed_shaped_resources(doc: &mut Document, fonts: &mut Dict, runs: &ShapedRunCache) -> Result<ShapedResources, EditError> {
    let mut resources = HashMap::new();
    for (key, shaped) in runs {
        let glyphs = match &shaped.font {
            ShapedFont::Existing(codes) => codes.clone(),
            ShapedFont::ThaiOutlines => crate::added::thai_glyph_resources(doc, fonts, &shaped.run)?,
            ShapedFont::Program { bytes, family } => {
                let mappings: Vec<_> = shaped.run.glyphs.iter().map(|glyph| (glyph.text.clone(), glyph.id)).collect();
                let resource = pdfcraft_fonts::embedding::embed_mapping(doc, fonts, bytes, family, &mappings).map_err(EditError::Invalid)?;
                (0..shaped.run.glyphs.len())
                    .map(|i| {
                        let code = u16::try_from(i.saturating_add(1)).map_err(|_| EditError::Invalid("shaped glyph limit".into()))?;
                        Ok((resource.clone(), code.to_be_bytes().to_vec()))
                    })
                    .collect::<Result<Vec<_>, EditError>>()?
            }
        };
        resources.insert(*key, glyphs);
    }
    Ok(resources)
}

/// A positioned glyph run, kept in one text object across Type 3 font shards.
struct ShapedRunDrawing<'a> {
    doc: &'a Document,
    fonts: &'a Dict,
    shaped: &'a pdfcraft_fonts::source_shaping::ShapedRun,
    resources: &'a [(String, Vec<u8>)],
    text: &'a str,
    matrix: [f64; 6],
    size: f64,
    thai: bool,
}

impl ShapedRunDrawing<'_> {
    /// Draw and return the run's advance in text space, including cluster-safe spacing.
    fn draw(&self, block_ops: &mut Vec<Op>, state: &mut TextState) -> Result<f64, EditError> {
        let Self { doc, fonts, shaped, resources, text, matrix: tm, size, thai } = *self;
        let n = pdfcraft_content::num;
        let (resource, _) = self.resources.first().ok_or_else(|| EditError::Invalid("shaped run has no glyphs".into()))?;
        let mut properties = Dict::new();
        properties.set(b"ActualText".to_vec(), Object::String(PdfString::text(text)));
        block_ops.push(Op::new("BDC", vec![Object::name("Span"), Object::Dict(properties)]));
        state.font = Some((resource.clone(), size));
        // One text object for the run: each glyph is placed where shaping put it by a TJ
        // adjustment from where the glyph before it ends (its advance in the embedded
        // font's widths), and raised by the text rise for a mark set above or below
        // (ISO 32000-1 §9.4.3, §9.3.7). Font-program codes are separate from Unicode;
        // ToUnicode and ActualText keep the logical text.
        let per_unit = 1000.0 / size;
        if !per_unit.is_finite() {
            return Err(EditError::Invalid("the shaped text has no size".into()));
        }
        let mut shown_fonts = HashMap::new();
        for (name, _) in resources {
            if !shown_fonts.contains_key(name) {
                let font = fonts.get(name.as_bytes()).and_then(|f| doc.dict(f)).ok_or_else(|| EditError::Invalid("shaped font is missing".into()))?;
                shown_fonts.insert(name.clone(), Metrics::from_dict(doc, &font));
            }
        }
        // Keep the original text settings. TJ compensates for the automatic PDF advance
        // after each glyph, while shaping keeps marks attached to their cluster.
        let (char_spacing, word_spacing, rise) = (state.char_spacing, state.word_spacing, state.rise);
        block_ops.push(Op::new("BT", vec![]));
        block_ops.extend(state.ops());
        block_ops.push(Op::new("Tm", tm.iter().map(|v| n(*v)).collect()));
        // `pen`: where the next glyph would start, in thousandths of the font size.
        let (mut pen, mut extra, mut raised) = (0.0, 0.0, rise);
        let mut shown_glyphs: Vec<Object> = Vec::new();
        let mut cluster_spacing = 0.0;
        let mut current_resource = resource.as_str();
        for (i, glyph) in shaped.glyphs.iter().enumerate() {
            let (name, code) = resources.get(i).ok_or_else(|| EditError::Invalid("shaped glyph resource is missing".into()))?;
            let shown = shown_fonts.get(name).ok_or_else(|| EditError::Invalid("shaped font is missing".into()))?;
            if name != current_resource {
                if !shown_glyphs.is_empty() {
                    block_ops.push(Op::new("TJ", vec![Object::Array(std::mem::take(&mut shown_glyphs))]));
                }
                block_ops.push(Op::new("Tf", vec![Object::name(name), n(size)]));
                current_resource = name;
            }
            if thai && !glyph.text.is_empty() {
                // Apply spacing only after a whole cluster, keeping marks on their base.
                extra += cluster_spacing;
                cluster_spacing = char_spacing * glyph.text.chars().count() as f64 + word_spacing * glyph.text.matches(' ').count() as f64;
            }
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
            shown_glyphs.push(Object::String(PdfString::literal(code.clone())));
            let cid = code.iter().fold(0_u32, |value, byte| value.saturating_mul(256).saturating_add(u32::from(*byte)));
            pen = target
                + if code.is_empty() {
                    0.0
                } else {
                    shown.width(cid) * 1000.0 + (char_spacing + if shown.is_space(cid, code.len()) { word_spacing } else { 0.0 }) * per_unit
                };
            if !thai {
                extra += char_spacing * glyph.text.chars().count() as f64 + word_spacing * glyph.text.matches(' ').count() as f64;
            }
        }
        let end = (shaped.advance * size + extra + cluster_spacing) * per_unit;
        if (end - pen).abs() > 1e-6 {
            shown_glyphs.push(n(pen - end));
        }
        if !shown_glyphs.is_empty() {
            block_ops.push(Op::new("TJ", vec![Object::Array(shown_glyphs)]));
        }
        if (raised - rise).abs() > 1e-9 {
            block_ops.push(Op::new("Ts", vec![n(rise)]));
        }
        block_ops.push(Op::new("ET", vec![]));
        (state.char_spacing, state.word_spacing) = (char_spacing, word_spacing);
        extra += cluster_spacing;
        block_ops.push(Op::new("EMC", vec![]));
        Ok(shaped.advance * size + extra)
    }
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

/// The calibration factors fit each word to its box at its own size (its `Tf` in its text space,
/// which a range's size doesn't change). An OCR phrase drawn at one size needs the inverse ratio
/// too, so every word still spans the box the scan shows it in. The ratio is not clamped again:
/// a word OCR sized at three times the phrase's needs three times the calibration.
fn fit_to_phrase(calibration: &mut HashMap<usize, f64>, lines: &[TextLine], phrase: Option<f64>) {
    let Some(phrase) = phrase else { return };
    for (index, factor) in calibration.iter_mut() {
        let own = lines.get(*index).and_then(|source| Some(source.origin.state.font.as_ref()?.1 * source.origin.k));
        if let Some(own) = own.filter(|own| own.is_finite() && *own > 0.0) {
            let fitted = *factor * own / phrase;
            if fitted.is_finite() {
                *factor = fitted;
            }
        }
    }
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

/// How far below a row of `size` the next one is written: the source's leading unless a size
/// or line spacing is chosen.
fn row_step(style: &BlockStyle, size: f64, source_leading: f64) -> f64 {
    if style.line_spacing.is_none() && style.size.is_none() {
        source_leading
    } else {
        style.line_spacing.unwrap_or(1.5).clamp(0.5, 5.0) * style.size.unwrap_or(size)
    }
}

type WrappedStyledRows = (StyledRows, RowRanges, Vec<usize>, Vec<bool>, Vec<usize>);

/// For each of `chars`, whether a row may end just before it inside a run of non-space characters
/// (Chinese, Japanese and Thai), worked out once per run: Thai words need the whole run, not just
/// two neighbours.
fn styled_break_before(chars: &[StyledChar]) -> Vec<bool> {
    let mut break_before = vec![false; chars.len()];
    let mut run_start = 0;
    while run_start < chars.len() {
        let mut run_end = run_start;
        while chars.get(run_end).is_some_and(|c| !c.0.is_whitespace()) {
            run_end += 1;
        }
        let run: String = chars.get(run_start..run_end).unwrap_or_default().iter().map(|c| c.0).collect();
        for (i, cut) in crate::linebreak::breaks_before_chars(&run).into_iter().enumerate() {
            if let Some(slot) = break_before.get_mut(run_start.saturating_add(i)) {
                *slot = cut;
            }
        }
        run_start = run_end.max(run_start.saturating_add(1));
    }
    break_before
}

fn wrap_styled_rows(
    chars: &[StyledChar],
    width: f64,
    measure: impl Fn(&[StyledChar]) -> Result<f64, EditError>,
) -> Result<WrappedStyledRows, EditError> {
    let break_before = styled_break_before(chars);
    let mut rows: Vec<Vec<StyledChar>> = Vec::new();
    let mut row_ranges = Vec::new();
    // True when this row starts because the previous word did not fit, not because of a newline.
    let mut row_soft = Vec::new();
    let mut next_soft = false;
    // How many separator characters a wrap trimmed off the row that just ended, for the row
    // about to start (0 for a Chinese, Japanese or Thai word split, which trims none; meaningless unless
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
            // A word ends at a space, or where Chinese, Japanese or Thai text may break inside it.
            while chars.get(end).is_some_and(|c| !c.0.is_whitespace()) && !break_before.get(end).copied().unwrap_or(false) {
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
    lines: &[TextLine],
    sources: Vec<(std::ops::Range<usize>, usize)>,
    ranges: &[&'a RangeStyle],
    style: &BlockStyle,
) -> StyledLines<'a> {
    let mut styled_lines = lines.to_vec();
    let mut styled_spans = Vec::new();
    let mut underlines = HashMap::new();
    let mut overrides = HashMap::new();
    for (span, source) in sources {
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

/// Select one existing subset of the exact source face for a Thai span. Never replace its
/// family/weight to make an edit succeed; missing glyphs need an explicit font choice. The
/// subsets' programs are read within `budget`, shared by the edit's spans.
fn legacy_thai_font(
    doc: &Document,
    fonts: &Dict,
    source: &TextLine,
    span: &str,
    cache: &mut HashMap<String, LegacyFace>,
    budget: &mut pdfcraft_cos::DecodeBudget,
) -> Result<Option<ThaiSpan>, EditError> {
    if !source.base_font.rsplit('+').next().unwrap_or(&source.base_font).starts_with("THSarabunPSK")
        || !span.chars().any(|c| matches!(u32::from(c), 0x0e00..=0x0e7f))
    {
        return Ok(None);
    }
    let family = pdfcraft_fonts::embedding::face_key(&source.base_font);
    let mut candidates = vec![source.font.clone()];
    // Every name on the page is read here for its face name alone, aliases of one font and other
    // faces' fonts too, with no total, as before: charged to the edit's allowance, each alias would
    // pay again and other faces would spend what the face's subsets need.
    candidates.extend(fonts.iter().filter_map(|(name, font)| {
        let font = doc.dict(font)?;
        (pdfcraft_fonts::embedding::face_key(&Metrics::from_dict(doc, &font).base_font) == family).then(|| String::from_utf8_lossy(name).into_owned())
    }));
    for name in candidates.into_iter().take(128) {
        if cache.contains_key(&name) {
            continue;
        }
        let Some(resource) = fonts.get(name.as_bytes()) else { continue };
        let Some(font) = doc.dict(resource) else { continue };
        let object = resource.as_ref();
        let face = match object.and_then(|o| cache.values().find(|face| face.object == Some(o))) {
            Some(alias) => LegacyFace { glyphs: alias.glyphs.clone(), metrics: alias.metrics.clone(), object, ..*alias },
            None => {
                // Read within the edit's allowance; past it the subset goes unread, and the edit is
                // refused below.
                let metrics = Metrics::from_dict_within(doc, &font, budget);
                let (space, glyphs) = if metrics.past_allowance {
                    (0.0, None)
                } else {
                    let space = source_space(doc, fonts, &metrics);
                    (space, pdfcraft_fonts::pdf::LegacyThaiFont::from_pdf(doc, &font, space, budget).ok().flatten().map(std::rc::Rc::new))
                };
                let style = FontStyle::of(doc, &font, &metrics);
                LegacyFace { glyphs, metrics, style, object, space }
            }
        };
        cache.insert(name, face);
    }
    // A subset left unread would count as unusable, silently changing which subset draws the text.
    // A read refused for want of the allowance is what refuses the edit: reads that spent it to the
    // byte refused nothing.
    if budget.refused() > 0 {
        return Err(EditError::Invalid(format!(
            "the page's {} subsets need more than {} MiB of font data for one edit; choose a replacement font for this text",
            source.base_font.rsplit('+').next().unwrap_or(&source.base_font),
            MAX_THAI_PROGRAMS >> 20
        )));
    }
    // A face none of whose subsets can be shaped in place (no Thai, or neither positioned nor in
    // the legacy glyph order) is shaped like any other source font (`shape_source`).
    if !has_thai_source_face(cache, &family) {
        return Ok(None);
    }
    let rows = || span.split(row_break).filter(|row| !row.is_empty());
    for row in rows() {
        if let Err(error) = shape_existing_thai(cache, &source.font, row) {
            // Not in the face's existing codes. A complete program of the face that shapes every
            // row with its own layout (one Load font added as CFF, or whose contextual glyphs it
            // gave no code yet) takes the whole span, so no cluster is split between fonts.
            let Some(source_font) = fonts.get(source.font.as_bytes()).and_then(|f| doc.dict(f)) else { return Err(error) };
            let complete = program_candidates(doc, fonts, &source.font, source_font, &source.base_font)
                .into_iter()
                .find(|(_, font)| rows().all(|row| shape_with_program(doc, font, row).is_some()));
            return match complete {
                Some((name, _)) => Ok(Some(ThaiSpan::Program(name))),
                None => Err(error),
            };
        }
    }
    Ok(Some(ThaiSpan::InPlace(source.font.clone())))
}

/// Whether a subset of the face `family` (a [`face_key`](pdfcraft_fonts::embedding::face_key))
/// in `cache` can be shaped in place: a THSarabunPSK program with its own Thai mark positioning,
/// or a verified legacy positional one.
fn has_thai_source_face(cache: &HashMap<String, LegacyFace>, family: &str) -> bool {
    cache.values().any(|face| face.glyphs.is_some() && pdfcraft_fonts::embedding::face_key(&face.metrics.base_font) == family)
}

/// A visual row may use several subsets of the same exact face (often a separate subset for
/// punctuation). Shape Thai clusters intact, and retain each glyph's original PDF font/code.
fn shape_existing_thai(cache: &HashMap<String, LegacyFace>, primary: &str, text: &str) -> Result<ShapedText, EditError> {
    if text.len() > 262_144 {
        return Err(EditError::Invalid("the Thai source row exceeds its shaping budget".into()));
    }
    let first = cache.get(primary).ok_or_else(|| EditError::Invalid("the Thai source face is missing".into()))?;
    let family = pdfcraft_fonts::embedding::face_key(&first.metrics.base_font);
    let mut candidates = vec![(primary, first)];
    candidates.extend(
        cache
            .iter()
            .filter(|(name, face)| {
                name.as_str() != primary && face.style == first.style && pdfcraft_fonts::embedding::face_key(&face.metrics.base_font) == family
            })
            .map(|(name, face)| (name.as_str(), face)),
    );
    // Stable source-first choice, then resource name; never depend on hash iteration order.
    if let Some(tail) = candidates.get_mut(1..) {
        tail.sort_by_key(|(name, _)| *name);
    }
    let thai = |c: char| matches!(u32::from(c), 0x0e01..=0x0e5b | 0xf700..=0xf71a);
    let mut run = pdfcraft_fonts::source_shaping::ShapedRun { glyphs: Vec::new(), advance: 0.0 };
    let mut resources = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((at, ch)) = chars.next() {
        if thai(ch) {
            let mut end = at + ch.len_utf8();
            while let Some(&(next, c)) = chars.peek() {
                if !thai(c) {
                    break;
                }
                chars.next();
                end = next + c.len_utf8();
            }
            let segment = text.get(at..end).ok_or_else(|| EditError::Invalid("invalid Thai character boundary".into()))?;
            let unshaped = |why: String| {
                EditError::Invalid(format!(
                    "{} cannot keep this edit in the original face: {}; load the complete original font or explicitly choose a replacement",
                    first.metrics.base_font,
                    if why.is_empty() { "its source glyph program is incomplete" } else { &why }
                ))
            };
            // The whole run in one subset keeps any kerning between its clusters. A subsetter may
            // instead have split the run's glyphs across subsets of the face: then each cluster
            // (a base and the marks on it, which shape together) comes from a subset that has it.
            let pieces = match shape_in_one_subset(&candidates, segment) {
                Ok(whole) => vec![whole],
                Err(_) => thai_clusters(segment)
                    .into_iter()
                    .map(|cluster| shape_in_one_subset(&candidates, cluster).map_err(unshaped))
                    .collect::<Result<Vec<_>, _>>()?,
            };
            for (name, shaped) in pieces {
                for (mut glyph, code) in shaped.shaped.glyphs.into_iter().zip(shaped.codes) {
                    glyph.position[0] += run.advance;
                    resources.push((name.to_owned(), code));
                    run.glyphs.push(glyph);
                }
                run.advance += shaped.shaped.advance;
            }
        } else {
            let logical = ch.to_string();
            let found = candidates.iter().find_map(|(name, face)| {
                let code = face.metrics.encode(&logical)?;
                let first = face.metrics.codes(&code).first()?.0;
                // `encode` accepts a CID the subset emptied (#1443): drawing it would show nothing.
                // Try the next subset; a space then falls back to its measured gap below.
                if face.metrics.has_outline(first) == Some(false) {
                    return None;
                }
                Some((*name, code, face.metrics.width(first)))
            });
            let (name, code, width) = if let Some(found) = found {
                found
            } else if ch == ' ' {
                // The source subset may omit spaces. Use a gap measured from a matching
                // document subset, then the face's existing text-space fallback.
                let width = candidates
                    .iter()
                    .find_map(|(_, f)| {
                        let code = f.metrics.encode(" ")?;
                        Some(f.metrics.width(f.metrics.codes(&code).first()?.0))
                    })
                    .unwrap_or(first.space);
                (primary, Vec::new(), width)
            } else {
                return Err(EditError::Invalid(format!(
                    "{} has no source glyph for {ch} (U+{:04X}); load the complete original face or explicitly select a replacement",
                    first.metrics.base_font,
                    u32::from(ch)
                )));
            };
            run.glyphs.push(pdfcraft_fonts::source_shaping::ShapedGlyph { id: Default::default(), position: [run.advance, 0.0], text: logical });
            resources.push((name.to_owned(), code));
            run.advance += width;
        }
        if run.glyphs.len() > 65_534 || !run.advance.is_finite() {
            return Err(EditError::Invalid("the Thai source row exceeds its shaping budget".into()));
        }
    }
    Ok(ShapedText { run, font: ShapedFont::Existing(resources) })
}

/// Shape `text` in the first candidate subset that has every glyph it needs, and name it. The
/// error is the first subset's reason (empty when no candidate has a usable glyph program).
fn shape_in_one_subset<'a>(candidates: &[(&'a str, &LegacyFace)], text: &str) -> Result<(&'a str, pdfcraft_fonts::pdf::ExistingThaiRun), String> {
    let mut why = String::new();
    for (name, face) in candidates {
        let Some(font) = &face.glyphs else { continue };
        match font.shape(text) {
            Ok(shaped) => return Ok((name, shaped)),
            Err(e) if why.is_empty() => why = e,
            Err(_) => {}
        }
    }
    Err(why)
}

/// `text` (a run of Thai characters) cut before each character that doesn't attach to the one
/// before it: a cluster is a base or leading vowel with the vowels and tone marks above and below
/// it, and Sara Am, whose Nikhahit sits on the base. Legacy presentation forms (U+F700–U+F71A)
/// are kept with what precedes them.
fn thai_clusters(text: &str) -> Vec<&str> {
    let attaches = |c: char| matches!(u32::from(c), 0x0e31 | 0x0e33..=0x0e3a | 0x0e47..=0x0e4e | 0xf700..=0xf71a);
    let mut starts: Vec<usize> = text.char_indices().filter(|(at, c)| *at == 0 || !attaches(*c)).map(|(at, _)| at).collect();
    starts.push(text.len());
    // Each boundary is a `char_indices` position or the end, strictly increasing: every `get`
    // succeeds and no cluster is empty.
    starts.windows(2).filter_map(|w| text.get(*w.first()?..*w.get(1)?)).collect()
}

/// Whether the approved Thai face can replace this source, whose embedded subset cannot
/// shape the replacement. A usable complete source program always takes precedence.
fn matching_thai_outline(doc: &Document, fonts: &Dict, source: &TextLine, span: &str) -> bool {
    let Some(face) = pdfcraft_fonts::document_thai_font() else { return false };
    // The bundled face is Regular and upright. A source its descriptor or name marks bold or
    // italic keeps that style, so completing it with these outlines would be a silent change:
    // it needs an explicit choice instead. (Variable-font instances often keep the family name.)
    let style = fonts
        .get(source.font.as_bytes())
        .and_then(|f| doc.dict(f))
        .map_or(FontStyle { bold: source.bold, italic: source.italic }, |f| FontStyle::of(doc, &f, &Metrics::from_dict(doc, &f)));
    if style.bold || style.italic {
        return false;
    }
    if !span.chars().any(|c| matches!(u32::from(c), 0x0e00..=0x0e7f))
        || !pdfcraft_fonts::embedding::matching_faces(&source.base_font).iter().any(|f| std::ptr::eq(*f, face))
        || !covers_rows(face.bytes, span)
    {
        return false;
    }
    // A registered complete matching program also completes the source subset.
    // Do not replace it with bundled outlines merely because the original resource is a subset.
    // Only one in the source's style does (`program_candidates`), so only that one counts here.
    let family = pdfcraft_fonts::embedding::face_key(&source.base_font);
    !fonts.iter().filter_map(|(_, font)| doc.dict(font)).any(|font| {
        let metrics = Metrics::from_dict(doc, &font);
        pdfcraft_fonts::embedding::face_key(&metrics.base_font) == family
            && FontStyle::of(doc, &font, &metrics) == style
            && pdfcraft_fonts::pdf::embedded_sfnt(doc, &font).is_some_and(|bytes| {
                pdfcraft_fonts::embedding::validate(&bytes).is_ok()
                    && pdfcraft_fonts::source_shaping::has_thai_layout(&bytes)
                    && covers_rows(&bytes, span)
                    && span.split(row_break).all(|row| pdfcraft_fonts::source_shaping::shape(&bytes, row).is_ok())
            })
    })
}

/// Whether a complete program draws every row of `span`. [`row_break`] characters move to the next
/// row and are never drawn, so they need no glyph (#6272); `embedding::covers` exempts only CR and LF.
fn covers_rows(bytes: &[u8], span: &str) -> bool {
    span.split(row_break).all(|row| pdfcraft_fonts::embedding::covers(bytes, row))
}

/// Use the same Thai script selection and mark/cluster positioning as Add Text, including
/// rows beginning with Latin or digits (guessing from their first character would select Latin).
fn shape_thai_row(text: &str) -> Result<pdfcraft_fonts::source_shaping::ShapedRun, EditError> {
    if text.len() > 262_144 || text.contains(row_break) {
        return Err(EditError::Invalid("shape one bounded text row at a time".into()));
    }
    let clusters =
        pdfcraft_fonts::shape_thai(text).map_err(|e| EditError::Invalid(format!("the matching Thai font cannot shape this text ({e:?})")))?;
    let mut glyphs = Vec::new();
    let mut advance = 0.0;
    for cluster in clusters {
        for (i, (id, [x, y])) in cluster.glyphs.into_iter().enumerate() {
            if glyphs.len() >= 65_534 {
                return Err(EditError::Invalid("shaped row exceeds its geometry budget".into()));
            }
            glyphs.push(pdfcraft_fonts::source_shaping::ShapedGlyph {
                id: id.into(),
                position: [advance + x, y],
                text: if i == 0 { cluster.text.clone() } else { String::new() },
            });
        }
        advance += cluster.advance;
    }
    if !advance.is_finite() {
        return Err(EditError::Invalid("shaped row exceeds its geometry budget".into()));
    }
    Ok(pdfcraft_fonts::source_shaping::ShapedRun { glyphs, advance })
}

/// The resources whose complete program may shape text in the face of `base_font`: `source`
/// (the resource `name`) first, then the page's other fonts of that face (at most 128).
fn program_candidates(doc: &Document, fonts: &Dict, name: &str, source: Dict, base_font: &str) -> Vec<(String, Dict)> {
    let key = pdfcraft_fonts::embedding::face_key(base_font);
    let style = FontStyle::of(doc, &source, &Metrics::from_dict(doc, &source));
    let mut candidates = vec![(name.to_owned(), source)];
    candidates.extend(
        fonts
            .iter()
            .filter_map(|(n, f)| Some((String::from_utf8_lossy(n).into_owned(), doc.dict(f)?)))
            .filter(|(_, f)| {
                let m = Metrics::from_dict(doc, f);
                pdfcraft_fonts::embedding::face_key(&m.base_font) == key && FontStyle::of(doc, f, &m) == style
            })
            .take(128),
    );
    candidates
}

/// A font's weight and slant, for matching subsets and complete programs of one face: a Regular
/// program never completes text that a descriptor marks bold, say.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FontStyle {
    bold: bool,
    italic: bool,
}

impl FontStyle {
    /// Bold from the name or the descriptor's weight ([`Metrics`]); italic from the name or a
    /// non-zero `/ItalicAngle`. The descriptor's Italic flag alone isn't trusted: exporters set
    /// it on upright faces (a THSarabunPSK subset with `/Flags 96` and `/ItalicAngle 0`).
    fn of(doc: &Document, font: &Dict, metrics: &Metrics) -> Self {
        let descendant =
            font.get(b"DescendantFonts").and_then(|d| doc.resolve(d).as_array().and_then(|a| a.first()).cloned()).and_then(|d| doc.dict(&d));
        let descriptor = descendant.as_ref().unwrap_or(font).get(b"FontDescriptor").and_then(|d| doc.dict(d));
        let angle = descriptor.as_ref().and_then(|d| d.get(b"ItalicAngle")).and_then(|a| doc.resolve(a).as_f64()).unwrap_or(0.0);
        let name = metrics.base_font.to_ascii_lowercase();
        Self { bold: metrics.bold, italic: name.contains("italic") || name.contains("oblique") || (angle.is_finite() && angle.abs() > 0.1) }
    }
}

/// `text` shaped by `font`'s embedded program with its own layout, when the program covers it and
/// may be embedded again (`embed_mapping` refuses one that may not, so a later program is tried).
fn shape_with_program(doc: &Document, font: &Dict, text: &str) -> Option<(Vec<u8>, pdfcraft_fonts::source_shaping::ShapedRun)> {
    let bytes = pdfcraft_fonts::pdf::embedded_sfnt(doc, font)?;
    if pdfcraft_fonts::embedding::validate(&bytes).is_err() || !pdfcraft_fonts::embedding::covers(&bytes, text) {
        return None;
    }
    let shaped = pdfcraft_fonts::source_shaping::shape(&bytes, text).ok()?;
    Some((bytes, shaped))
}

/// Shape in the source or an explicitly registered matching full face. Never use interface fonts.
fn shape_source(doc: &Document, fonts: &Dict, thai: &ThaiShaping, name: &str, text: &str) -> Result<Option<ShapedText>, EditError> {
    if thai.outlined.contains(name) {
        let shaped = shape_thai_row(text)?;
        return Ok(Some(ShapedText { run: shaped, font: ShapedFont::ThaiOutlines }));
    }
    if let Some(resource) = thai.program.get(name) {
        let font = fonts
            .get(resource.as_bytes())
            .and_then(|f| doc.dict(f))
            .ok_or_else(|| EditError::Invalid("the complete font resource is missing".into()))?;
        let family = Metrics::from_dict(doc, &font).base_font;
        let (bytes, shaped) = shape_with_program(doc, &font, text)
            .ok_or_else(|| EditError::Invalid(format!("{family} cannot shape this text with its own layout; choose a font that covers it")))?;
        return Ok(Some(ShapedText { run: shaped, font: ShapedFont::Program { bytes, family } }));
    }
    if !pdfcraft_fonts::source_shaping::required(text) {
        return Ok(None);
    }
    let source =
        fonts.get(name.as_bytes()).and_then(|f| doc.dict(f)).ok_or_else(|| EditError::Invalid("the shaped text has no font resource".into()))?;
    let family = Metrics::from_dict(doc, &source).base_font;
    // In the face's existing codes first; what they can't show, a complete program of the face
    // may (`legacy_thai_font` already gave the span to one that can).
    let in_place = (family.rsplit('+').next().unwrap_or(&family).starts_with("THSarabunPSK")
        && text.chars().any(|c| matches!(u32::from(c), 0x0e00..=0x0e7f))
        && has_thai_source_face(&thai.legacy, &pdfcraft_fonts::embedding::face_key(&family)))
    .then(|| shape_existing_thai(&thai.legacy, name, text));
    let in_place = match in_place {
        Some(Ok(shaped)) => return Ok(Some(shaped)),
        Some(Err(error)) => Some(error),
        None => None,
    };
    for (_, font) in program_candidates(doc, fonts, name, source, &family) {
        if let Some((bytes, shaped)) = shape_with_program(doc, &font, text) {
            return Ok(Some(ShapedText { run: shaped, font: ShapedFont::Program { bytes, family } }));
        }
    }
    if let Some(error) = in_place {
        return Err(error);
    }
    let fallback = pdfcraft_fonts::document_thai_font()
        .filter(|_| text.chars().any(|c| matches!(u32::from(c), 0x0e00..=0x0e7f)))
        .map(|face| format!("; or select {} in the document-font menu to replace it", face.family))
        .unwrap_or_default();
    Err(EditError::Invalid(format!(
        "complex text in {} needs its complete source TrueType/OpenType font; use Load font to keep it{fallback}",
        family.rsplit('+').next().unwrap_or(&family)
    )))
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

fn segment_underline_runs(
    widths: &[f64],
    segment_width: f64,
    x: f64,
    default_style: usize,
    row_styles: Option<&[Option<usize>]>,
    pos: usize,
) -> Vec<(usize, f64, f64)> {
    let width_sum = widths.iter().copied().sum::<f64>();
    let width_scale = if width_sum > 1e-6 { segment_width / width_sum } else { 0.0 };
    let mut cursor = x;
    let mut runs: Vec<(usize, f64, f64)> = Vec::new();
    for (offset, width) in widths.iter().enumerate() {
        let style_index = row_styles.and_then(|styles| styles.get(pos.saturating_add(offset))).and_then(|style| *style).unwrap_or(default_style);
        let next = cursor + *width * width_scale;
        match runs.last_mut() {
            Some((prev, _, to)) if *prev == style_index => *to = next,
            _ => runs.push((style_index, cursor, next)),
        }
        cursor = next;
    }
    runs
}

/// Keep spaces in a subset without a space glyph attached to an adjacent glyph-bearing segment on
/// the same row so `source_show` wraps them in one `/ActualText` span (#4229).
///
/// The returned slot is the space's style index from before that reassignment, where the
/// grouping key changed. Underlines stay with that style; the row itself keeps the neighbour's
/// key so the space is not a group of its own.
fn assign_subset_space_fonts(row: &mut [(char, usize, String)], metrics: &std::collections::BTreeMap<String, Metrics>) -> Vec<Option<usize>> {
    let mut right_any = vec![None; row.len()];
    let mut next_any: Option<(usize, String)> = None;
    for (i, item) in row.iter().enumerate().rev() {
        if item.0 != ' ' {
            next_any = Some((item.1, item.2.clone()));
        } else if let Some(slot) = right_any.get_mut(i) {
            *slot = next_any.clone();
        }
    }
    let mut prev_any: Option<(usize, String)> = None;
    let mut underline_style = vec![None; row.len()];
    for (i, item) in row.iter_mut().enumerate() {
        if item.0 != ' ' {
            prev_any = Some((item.1, item.2.clone()));
            continue;
        }
        if metrics.get(&item.2).is_none_or(|m| m.encode(" ").is_some()) {
            continue;
        }
        if let Some((style, font)) = prev_any.as_ref().or_else(|| right_any.get(i).and_then(Option::as_ref)) {
            let original = item.1;
            item.1 = *style;
            item.2 = font.clone();
            if let Some(slot) = underline_style.get_mut(i)
                && item.1 != original
            {
                *slot = Some(original);
            }
        }
    }
    underline_style
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
/// separator characters it consumed (0 for a Chinese, Japanese or Thai word split), so a rejoin restores
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
/// beyond `ops.len()`, since `splice`'s callback never visits that index). A dropped operator
/// `i` leaves `left_behind(i, keep)` in its place: its line move, spacing and advance.
fn splice_with_marker(
    pieces: &Pieces,
    ops: &[Op],
    marker: Option<(usize, Option<usize>, usize)>,
    at: usize,
    mut block_ops: Vec<Op>,
    drop: &std::collections::HashSet<usize>,
    mut left_behind: impl FnMut(usize, bool) -> Vec<Op>,
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
        let keep = !drop.contains(&i);
        insert.extend(left_behind(i, keep));
        (insert, keep)
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

/// The source run each part of the draft is styled from: the ones given, which must cover the draft
/// in order, on character boundaries, from the block's runs, or else the edit's own.
fn draft_sources(block: &TextBlock, draft: &str, style: &BlockStyle) -> Result<Vec<(std::ops::Range<usize>, usize)>, EditError> {
    if style.sources.is_empty() {
        return Ok(block.edited_spans(draft));
    }
    let mut at = 0;
    for (range, source) in &style.sources {
        if range.start != at || range.start >= range.end || draft.get(range.clone()).is_none() || !block.lines.contains(source) {
            return Err(EditError::Invalid("the text's style sources must cover it in order with the paragraph's own runs".into()));
        }
        at = range.end;
    }
    if at != draft.len() {
        return Err(EditError::Invalid("the text's style sources must cover it in order with the paragraph's own runs".into()));
    }
    Ok(style.sources.clone())
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
    let sources = draft_sources(block, draft, style)?;
    let styled = apply_range_styles(first, lines, sources, &ranges, style);
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
    let thai = prepared.thai;
    let chars = prepared.values;
    let substituted = prepared.substituted;
    let phrase_size = block.phrase_size();
    let size_of = |source: &TextLine, index: usize| {
        overrides.get(&index).and_then(|r| r.size).or(style.size).or(phrase_size).unwrap_or(source.size).max(0.1) / source.origin.k.max(1e-6)
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
    let mut calibration = calibration_factors(lines, &metrics, style, &overrides, space_of);
    fit_to_phrase(&mut calibration, lines, phrase_size);
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
            let end = styled_run_end(items, at);
            let segment = items.get(at..end).unwrap_or_default();
            let string: String = segment.iter().map(|v| v.0).collect();
            if let Some(shaped) = shape_source(doc, &fonts, &thai, &item.2, &string)? {
                let shaped = &shaped.run;
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
    let (mut rows, row_ranges, row_lines, row_soft, row_gap) = wrap_styled_rows(&chars, width, measure)?;
    let space_styles: Vec<_> = rows.iter_mut().map(|row| assign_subset_space_fonts(row, &metrics)).collect();
    let streams = content_streams(doc, &p.dict)?;
    let pieces = crate::text::joined(&streams);
    let ops = pieces.parse().ops;
    // A row the edit doesn't reach keeps its source operators, so its kerning, TJ word gaps and
    // glyph widths stay exactly as they were instead of being rebuilt from the text state. A row
    // aligned on its own is reached; the rows around it are not.
    let plain = BlockStyle { ranges: Vec::new(), sources: Vec::new(), aligns: Vec::new(), ..style.clone() } == BlockStyle::default();
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
    let at = if split { start } else { o.bt_op };
    validate_contextual_rows(&rows, &kept)?;
    let measured: Vec<_> = rows.iter().map(|row| measure(row)).collect::<Result<_, _>>()?;
    let shaped = prepare_shaped_rows(doc, &fonts, &thai, &rows, &kept, advance)?;
    let ShapedRows { runs: shaped_runs, advances: segment_advances, char_advances } = shaped;
    let glyph_resources = embed_shaped_resources(doc, &mut fonts, &shaped_runs)?;
    let n = pdfcraft_content::num;
    let mut block_ops = vec![Op::new("q", vec![])];
    block_ops.extend(reveal_ops(block, lines, style, o.ctm)?);
    let back = Matrix([o.ctm[0], o.ctm[1], o.ctm[2], o.ctm[3], 0.0, 0.0])
        .invert()
        .ok_or_else(|| EditError::Invalid("the paragraph is drawn in a space it cannot be edited in".into()))?;
    let (tx, ty) = back.apply(dx, dy);
    block_ops.push(Op::new("cm", vec![n(1.0), n(0.0), n(0.0), n(1.0), n(tx), n(ty)]));
    let run_ctm = Matrix([1.0, 0.0, 0.0, 1.0, tx, ty]).then(&Matrix(o.ctm)).0;
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
    // Revealed OCR text that something is painted over goes on top of the page, where it starts
    // from the initial state.
    let largest =
        block.lines.iter().filter_map(|i| lines.get(*i)).map(|l| l.size).chain(style.ranges.iter().filter_map(|r| r.size)).fold(first.size, f64::max);
    // At least one line of the largest glyphs, so a larger range reaches up as far as it draws.
    const LINE_EM: f64 = 1.2;
    let pitch = source_leading.max(row_step(style, largest, source_leading)).max(largest * LINE_EM);
    let widest = measured.iter().copied().fold(width, f64::max);
    // This path lays rows out along user-space baselines, so the paragraph is upright here.
    let reach = crate::Reach { rect: block.rect, offset: [dx, dy], width: widest, rows: rows.len(), pitch, upright: true };
    let over = crate::revealed_under_paint(doc, page, block.invisible, reach.bounds(), &ops, at);
    let initial = TextState::initial();
    let drawn_from = if over { &initial } else { after };
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
            let end = styled_run_end(row, pos);
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
            block_ops.extend(state.paint_ops(drawn_from, run_ctm));
            if let Some(shaped) = shaped_runs.get(&(row_index, pos)) {
                let resources = glyph_resources.get(&(row_index, pos)).ok_or_else(|| EditError::Invalid("shaped font is missing".into()))?;
                let drawing = ShapedRunDrawing {
                    doc,
                    fonts: &fonts,
                    shaped: &shaped.run,
                    resources,
                    text: &string,
                    matrix: tm,
                    size: size_of(source, item.1),
                    thai: string.chars().any(|c| matches!(u32::from(c), 0x0e00..=0x0e7f)),
                };
                segment_width = drawing.draw(&mut block_ops, &mut state)? * state.scale * source.origin.k;
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
            // A space reassigned onto a neighbour keeps that neighbour's grouping key, and its
            // own style for the underline. Stroke each run of one underline style separately.
            let widths = char_advances.get(&(row_index, pos)).map(Vec::as_slice).unwrap_or(&[]);
            for (style_index, from, to) in
                segment_underline_runs(widths, segment_width, x, item.1, space_styles.get(row_index).map(Vec::as_slice), pos)
            {
                let source = lines.get(style_index).unwrap_or(first);
                if underlines.get(&style_index).copied().or(style.underline) != Some(true) || to <= from {
                    continue;
                }
                let [r, g, b] = overrides.get(&style_index).and_then(|r| r.color).or(style.color).unwrap_or(source.color);
                let underline_size = size_of(source, style_index) * source.origin.k;
                let (x0, y0) = back.apply(from - o.ctm[4], baseline - o.ctm[5] - underline_size * 0.12);
                let (x1, y1) = back.apply(to - o.ctm[4], baseline - o.ctm[5] - underline_size * 0.12);
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
        baseline -= row_step(style, anchor.size, source_leading);
    }
    push_whole_paragraph_marker(&mut block_ops, marked, "EMC");
    block_ops.push(Op::new("Q", vec![]));
    let on_top = crate::lift_on_top(over, &mut block_ops);
    block_ops.extend(after.ops());
    if split {
        block_ops.insert(0, Op::new("ET", vec![]));
        block_ops.extend(crate::text::reopen_text_object(o.tlm, o.tm, after.font.as_ref().map(|(_, size)| *size), after.scale));
    }
    // A kept source row sits outside `block_ops`. Bound the whole paragraph when a marker is
    // safe. The close is not part of `block_ops`: that vector can end inside a text object
    // when the rewrite splits one, and a kept row's operators come later in the stream.
    let marker = if marked { None } else { paragraph_marker(block, lines, &ops) };
    // A dropped `'` or `"` leaves its line move and spacing, and a dropped show its advance.
    let mut advances = crate::text::dropped_advances(doc, &ops, &res, &drop);
    let split_at = split.then_some(start);
    let left_behind = |i: usize, keep: bool| {
        let mut out = crate::text::dropped_side_effects(&ops, i, keep, split_at);
        out.extend(advances.remove(&i));
        out
    };
    let data = splice_with_marker(&pieces, &ops, marker, at, block_ops, &drop, left_behind);
    let contents = rewritten_contents(doc, &streams, data)?;
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(p.obj, |d| {
        d.set(b"Contents".to_vec(), contents);
        d.set(b"Resources".to_vec(), Object::Dict(res));
    })?;
    crate::draw_on_top(doc, &p, o.ctm, &on_top)?;
    Ok(LineEdit { substituted })
}

#[cfg(test)]
mod tests {
    use super::styled_run_end;

    #[test]
    fn styled_runs_stop_at_style_or_font_changes() {
        let items = vec![('a', 0, "F1".into()), (' ', 0, "F1".into()), ('b', 1, "F1".into()), ('c', 1, "F2".into())];
        assert_eq!(styled_run_end(&items, 0), 2);
        assert_eq!(styled_run_end(&items, 1), 2);
        assert_eq!(styled_run_end(&items, 2), 3);
        assert_eq!(styled_run_end(&items, 3), 4);
        assert_eq!(styled_run_end(&items, 4), 4);
        assert_eq!(styled_run_end(&[], 0), 0);
    }
}
