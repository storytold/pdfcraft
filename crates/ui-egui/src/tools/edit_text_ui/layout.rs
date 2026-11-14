use super::{Arc, BTreeMap, Color32, FontFamily, FontId, LineEditor, Rect};

/// The look shown for a paragraph: the family and weight guessed from its PDF font name.
pub(super) fn source_look(base_font: &str, size: f64, color: [f64; 3], detected_bold: bool, detected_italic: bool) -> pdfcraft_engine::AddedText {
    use pdfcraft_engine::FontFamily as F;
    let name = base_font.to_ascii_lowercase();
    let family = if ["courier", "mono", "consolas", "menlo", "monaco", "lucida console"].iter().any(|s| name.contains(s)) {
        F::Courier
    } else if !name.contains("sans") && ["times", "serif", "roman", "cambria", "georgia", "palatino", "garamond"].iter().any(|s| name.contains(s)) {
        F::Times
    } else {
        F::Helvetica
    };
    pdfcraft_engine::AddedText {
        family,
        bold: detected_bold || ["bold", "black", "heavy", "semibold", "demi"].iter().any(|s| name.contains(s)),
        italic: detected_italic || ["italic", "oblique", "slanted"].iter().any(|s| name.contains(s)),
        size: (size * 10.0).round() / 10.0,
        color,
        ..Default::default()
    }
}

pub(super) fn look_of(b: &pdfcraft_engine::TextBlock) -> pdfcraft_engine::AddedText {
    let mut look = source_look(&b.base_font, b.size, b.color, b.bold, b.italic);
    // Arabic text in an installed font shows that font in the font list.
    if b.text.chars().any(|c| matches!(u32::from(c), 0x0600..=0x06FF | 0x0750..=0x077F | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF)) {
        look.arabic_font = pdfcraft_fonts::arabic_font_family(&b.base_font).map(str::to_string);
    }
    look
}

/// The paragraph's installed font (`look.arabic_font`) as an interface font family, so the editor
/// draws the text with the page's shapes and widths and breaks its lines where the page will.
/// It is added to egui on first use and is there from the next frame; `None` until then, and
/// when the paragraph isn't set in an installed font.
pub(crate) fn document_family(ctx: &egui::Context, look: &pdfcraft_engine::AddedText) -> Option<FontFamily> {
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    let wanted = look.arabic_font.as_deref()?;
    let face = pdfcraft_fonts::arabic_candidates(Some(wanted), &[], false, look.bold).into_iter().next().filter(|f| f.family == wanted)?;
    let name = format!("pdf-font:{}:{}", face.family, face.style);
    let family = FontFamily::Name(name.clone().into());
    if ctx.fonts(|f| f.families().contains(&family)) {
        return Some(family);
    }
    let mut data = egui::FontData::from_owned(face.bytes()?.to_vec());
    data.index = face.index;
    ctx.add_font(FontInsert::new(&name, data, vec![InsertFontFamily { family, priority: FontPriority::Highest }]));
    None
}

pub(crate) fn editor_font(look: &pdfcraft_engine::AddedText, size: f32) -> FontId {
    let family = match (look.family, look.bold) {
        (pdfcraft_engine::FontFamily::Courier, _) => FontFamily::Monospace,
        (_, true) => FontFamily::Name("semibold".into()),
        (_, false) => FontFamily::Proportional,
    };
    FontId::new(size, family)
}

pub(super) fn color32(color: [f64; 3]) -> Color32 {
    Color32::from_rgb(
        (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/// The dark box behind text too pale to read on white.
pub(super) const DARK_EDITOR_FILL: Color32 = Color32::from_gray(0x26);

/// WCAG relative luminance of an sRGB colour.
pub(super) fn luminance(c: Color32) -> f32 {
    let channel = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
}

/// WCAG contrast ratio between two colours (1 to 21).
pub(super) fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The inline editor's background for text drawn in `text`: white, or a dark box when the text
/// reads better on that (white or pale text, which would vanish on white).
pub(crate) fn editor_fill(text: Color32) -> Color32 {
    if contrast(text, Color32::WHITE) >= contrast(text, DARK_EDITOR_FILL) { Color32::WHITE } else { DARK_EDITOR_FILL }
}

/// One TextEdit lays out all source runs; its caret and selection span the whole region.
pub(crate) fn install_source_fonts(ctx: &egui::Context, doc: &pdfcraft_engine::Document, page: usize) -> BTreeMap<String, String> {
    // Bundled replacement selectors are menu choices, never source resource names. Installing
    // an unused choice would make even a standard-font editor wait a frame for its font atlas.
    let programs: Vec<_> = doc.text_editor_fonts(page).into_iter().filter(|font| !font.choice_only).collect();
    let mut definitions = ctx.fonts(|f| f.definitions().clone());
    // Only the current editor's document fonts live in the atlas. Never accumulate the
    // programs of previously opened files or add them to the interface's default families.
    definitions.font_data.retain(|name, _| !name.starts_with("pdf-source-"));
    definitions.families.retain(|family, _| !matches!(family, FontFamily::Name(name) if name.starts_with("pdf-source-")));
    let mut families = BTreeMap::new();
    for (index, program) in programs.iter().enumerate() {
        let name = format!("pdf-source-{index}");
        definitions.font_data.insert(name.clone(), Arc::new(egui::FontData::from_owned(program.bytes.clone())));
        families.insert(program.resource.clone(), name);
    }
    for program in &programs {
        let Some(name) = families.get(&program.resource) else { continue };
        // Word-created PDFs often split one face into several subsets. Use all of the
        // document's matching subsets before falling back for newly inserted characters.
        let mut stack = vec![name.clone()];
        stack.extend(
            programs
                .iter()
                .filter(|p| {
                    pdfcraft_fonts::embedding::face_key(&p.face) == pdfcraft_fonts::embedding::face_key(&program.face)
                        && p.resource != program.resource
                })
                .filter_map(|p| families.get(&p.resource).cloned()),
        );
        if let Some(fallback) = definitions.families.get(&FontFamily::Proportional) {
            stack.extend(fallback.iter().cloned());
        }
        definitions.families.insert(FontFamily::Name(name.clone().into()), stack);
    }
    if !families.is_empty() {
        ctx.set_fonts(definitions);
    }
    families
}

pub(super) fn editor_job(ui: &egui::Ui, ed: &LineEditor, text: &str, scale: f32, width: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    // An unchanged PDF row already has an authoritative line break. Do not rewrap it
    // because font rounding or a fallback face measures a fraction of a pixel wider.
    job.wrap.max_width = if text == ed.original && ed.look == ed.look0 && ed.extras == ed.extras0 { f32::INFINITY } else { width };
    if let Some((block, lines)) = &ed.source {
        let lefts: Vec<_> = block.rows.iter().filter_map(|r| r.first().and_then(|i| lines.get(*i)).map(|l| l.rect[0])).collect();
        let left_span = lefts.iter().copied().fold(f64::NEG_INFINITY, f64::max) - lefts.iter().copied().fold(f64::INFINITY, f64::min);
        let centered = block.rows.len() > 1
            && left_span > block.size * 0.5
            && block.rows.iter().all(|row| {
                let rect = row
                    .iter()
                    .filter_map(|i| lines.get(*i))
                    .map(|l| l.rect)
                    .reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]);
                rect.is_some_and(|r| ((r[0] + r[2] - block.rect[0] - block.rect[2]) / 2.0).abs() < block.size)
            });
        if centered {
            job.halign = egui::Align::Center;
        }
        // An OCR phrase is set in one size, as the engine redraws it: each word's own is a guess
        // from its box. Its words still keep their widths.
        let phrase = block.phrase_size();
        for (range, index) in block.edited_spans(text) {
            let Some(line) = lines.get(index) else { continue };
            let base = phrase.unwrap_or(line.size);
            let mut look = source_look(&line.base_font, base, line.color, line.bold, line.italic);
            look.size = base;
            if ed.look.family != ed.look0.family || ed.family_chosen {
                look.family = ed.look.family;
            }
            if ed.look.bold != ed.look0.bold {
                look.bold = ed.look.bold;
            }
            if ed.look.italic != ed.look0.italic {
                look.italic = ed.look.italic;
            }
            if ed.look.color != ed.look0.color {
                look.color = ed.look.color;
            }
            if ed.look.size != ed.look0.size {
                look.size = ed.look.size;
            }
            let mut font = editor_font(&look, (look.size as f32 * scale).clamp(1.0, 300.0));
            let original_face =
                ed.look.family == ed.look0.family && !ed.family_chosen && ed.look.bold == ed.look0.bold && ed.look.italic == ed.look0.italic;
            let embedded = original_face && ed.fonts.contains_key(&line.font);
            if embedded && let Some(name) = ed.fonts.get(&line.font) {
                font.family = FontFamily::Name(name.clone().into());
            }
            let native_width = ui.fonts_mut(|f| f.layout_no_wrap(line.text.clone(), font.clone(), Color32::BLACK).size().x);
            let target_width = ((line.rect[2] - line.rect[0]) * look.size / base.max(0.01)) as f32 * scale;
            let spacing = if original_face { (target_width - native_width) / line.text.chars().count().saturating_sub(1).max(1) as f32 } else { 0.0 };
            job.append(
                text.get(range).unwrap_or_default(),
                0.0,
                egui::TextFormat {
                    font_id: font,
                    extra_letter_spacing: spacing + ed.extras.char_spacing as f32 * scale,
                    color: color32(look.color),
                    italics: look.italic && !embedded,
                    ..Default::default()
                },
            );
        }
    } else {
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: editor_font(&ed.look, ed.size),
                color: color32(ed.look.color),
                italics: ed.look.italic,
                ..Default::default()
            },
        );
    }
    job
}

/// `galley` drawn in no colour: its rows still place the caret and the selection.
pub(super) fn unseen(mut galley: Arc<egui::Galley>) -> Arc<egui::Galley> {
    for placed in &mut Arc::make_mut(&mut galley).rows {
        for vertex in &mut Arc::make_mut(&mut placed.row).visuals.mesh.vertices {
            vertex.color = Color32::TRANSPARENT;
        }
    }
    galley
}

/// Align the editable galley's baselines to the source rows instead of egui's interface
/// leading. Glyphs, meshes, caret and selection receive the same transform.
pub(super) fn editor_galley(ui: &egui::Ui, ed: &LineEditor, text: &str, scale: f32, width: f32) -> Arc<egui::Galley> {
    let mut job = editor_job(ui, ed, text, scale, width);
    let centered = job.halign == egui::Align::Center;
    job.halign = egui::Align::LEFT;
    let mut galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let Some((block, lines)) = &ed.source else { return galley };
    let sources: Vec<_> = block
        .rows
        .iter()
        .filter_map(|row| {
            let first = row.first().and_then(|i| lines.get(*i))?;
            let rect = row
                .iter()
                .filter_map(|i| lines.get(*i))
                .map(|l| l.rect)
                .reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])?;
            Some((first.baseline()[1], rect))
        })
        .collect();
    let Some((first_baseline, first_rect)) = sources.first().copied() else { return galley };
    let leading = if ed.extras.line_spacing > 0.0 {
        ed.extras.line_spacing * ed.look.size
    } else {
        let fallback = if block.lines.len() == 1 && !text.contains('\n') {
            block.lines.first().and_then(|i| lines.get(*i)).map_or(block.size * 1.2, |line| line.wrap_leading())
        } else {
            block.size * 1.5
        };
        sources.windows(2).map(|r| (r[0].0 - r[1].0).abs()).rfind(|v| *v > 0.0).unwrap_or(fallback)
    };
    let g = Arc::make_mut(&mut galley);
    // Which line of the text (between `\n`s) each row is on: rows aligned on their own go where
    // the rewrite will put them, in the paragraph's width.
    let mut line = 0usize;
    for (index, placed) in g.rows.iter_mut().enumerate() {
        let own = ed.look.aligns.get(line).copied().flatten();
        if placed.ends_with_newline {
            line = line.saturating_add(1);
        }
        let (last_baseline, last_rect) = sources.last().copied().unwrap_or((first_baseline, first_rect));
        let (baseline, source_rect) = sources
            .get(index)
            .copied()
            .unwrap_or((last_baseline - index.saturating_add(1).saturating_sub(sources.len()) as f64 * leading, last_rect));
        let changed_size = ed.look.size != ed.look0.size;
        let ratio = if changed_size { ed.look.size / ed.look0.size.max(0.01) } else { 1.0 };
        let ascent = (source_rect[3] - sources.get(index).map_or(last_baseline, |s| s.0)) as f32 * scale * ratio as f32;
        let descent = (sources.get(index).map_or(last_baseline, |s| s.0) - source_rect[1]) as f32 * scale * ratio as f32;
        let base_y = (block.rect[3] - baseline) as f32 * scale;
        let row = Arc::make_mut(&mut placed.row);
        placed.pos.y = base_y - ascent;
        let free = (width - row.size.x).max(0.0);
        placed.pos.x = match own {
            Some(pdfcraft_engine::TextAlign::Center) => free / 2.0,
            Some(pdfcraft_engine::TextAlign::Right) => free,
            Some(pdfcraft_engine::TextAlign::Left | pdfcraft_engine::TextAlign::Justify) => 0.0,
            None if centered => (width - row.size.x) / 2.0,
            None => (source_rect[0] - block.rect[0]) as f32 * scale,
        };
        for glyph in &mut row.glyphs {
            let dy = ascent - glyph.pos.y;
            glyph.pos.y = ascent;
            glyph.font_ascent = ascent;
            glyph.font_height = ascent + descent;
            glyph.line_height = ascent + descent;
            if !glyph.uv_rect.is_nothing() {
                for vertex in row.visuals.mesh.vertices.iter_mut().skip(glyph.first_vertex as usize).take(4) {
                    vertex.pos.y += dy;
                }
            }
        }
        row.size.y = (ascent + descent).max(1.0);
    }
    if text == ed.original && ed.look == ed.look0 && ed.extras == ed.extras0 && block.rotation.abs() < 0.5 {
        place_on_source_words(g, block, lines, text, scale);
    }
    let mut bounds = Rect::NOTHING;
    let mut meshes = Rect::NOTHING;
    for placed in &mut g.rows {
        let row = Arc::make_mut(&mut placed.row);
        row.visuals.mesh_bounds = row.visuals.mesh.vertices.iter().fold(Rect::NOTHING, |r, v| r.union(Rect::from_min_max(v.pos, v.pos)));
        bounds = bounds.union(Rect::from_min_size(placed.pos, row.size));
        meshes = meshes.union(row.visuals.mesh_bounds.translate(placed.pos.to_vec2()));
    }
    g.rect = Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(bounds.right().max(0.0), bounds.bottom().max(0.0)));
    g.mesh_bounds = meshes;
    galley
}

/// While a paragraph is unchanged the page shows its own text, and the editor's galley is drawn
/// unseen: it only places the caret and the selection. Put each source word's glyphs across that
/// word's box on the page, so a click or a drag lands on the word under the pointer. The editor's
/// face is often a stand-in whose advances differ from the PDF's (a font program it can't load, or
/// an OCR layer whose every word has its own size), and stretching each run to its width still let
/// every word boundary drift, so a drag over one word selected another two words along.
///
/// A word takes its `ink` box when the run's words and boxes pair up, else the run's box spans all
/// its words. Within a word the stand-in's proportions are kept; spaces fill the gaps between
/// words. A row with a character this can't place, or with boxes out of reading order, is left as
/// laid out.
fn place_on_source_words(g: &mut egui::Galley, block: &pdfcraft_engine::TextBlock, lines: &[pdfcraft_engine::TextLine], text: &str, scale: f32) {
    let starts: Vec<usize> = text.char_indices().map(|(at, _)| at).collect();
    let chars: Vec<char> = text.chars().collect();
    let x = |user: f64| ((user - block.rect[0]) * f64::from(scale)) as f32;
    let right_to_left = place_right_to_left_rows(g, block, lines, &chars, &starts, &x);
    // Each character's glyph advance, in the galley's character order (a newline has no glyph).
    let mut advance = vec![0.0f32; chars.len()];
    let mut at = 0usize;
    for placed in &g.rows {
        for (glyph, slot) in placed.row.glyphs.iter().zip(advance.iter_mut().skip(at)) {
            *slot = glyph.advance_width.max(0.0);
        }
        at = at.saturating_add(placed.row.glyphs.len()).saturating_add(usize::from(placed.ends_with_newline));
    }
    if at != chars.len() {
        return;
    }
    // Where each character goes: its left edge and width, in galley coordinates.
    let mut target: Vec<Option<(f32, f32)>> = vec![None; chars.len()];
    let place = |range: std::ops::Range<usize>, left: f32, right: f32, target: &mut [Option<(f32, f32)>]| {
        let natural: f32 = advance.get(range.clone()).map_or(0.0, |a| a.iter().sum());
        let count = range.len().max(1) as f32;
        let mut pos = left;
        for i in range {
            let share = if natural > 0.0 { advance.get(i).copied().unwrap_or(0.0) / natural } else { 1.0 / count };
            let width = ((right - left) * share).max(0.0);
            if let Some(slot) = target.get_mut(i) {
                *slot = Some((pos, width));
            }
            pos += width;
        }
    };
    for (range, index) in &block.spans {
        let Some(line) = lines.get(*index) else { continue };
        let (first, end) = (starts.partition_point(|b| *b < range.start), starts.partition_point(|b| *b < range.end));
        let mut words = Vec::new();
        let mut i = first;
        while i < end {
            if chars.get(i).is_some_and(|c| c.is_whitespace()) {
                i += 1;
                continue;
            }
            let from = i;
            while i < end && chars.get(i).is_some_and(|c| !c.is_whitespace()) {
                i += 1;
            }
            words.push(from..i);
        }
        let (Some(head), Some(tail)) = (words.first().cloned(), words.last().cloned()) else { continue };
        if line.ink.len() == words.len() {
            for (word, ink) in words.iter().zip(&line.ink) {
                place(word.clone(), x(ink[0]), x(ink[2]), &mut target);
            }
        } else {
            place(head.start..tail.end, x(line.rect[0]), x(line.rect[2]), &mut target);
        }
    }
    let mut first = 0usize;
    for (index, placed) in g.rows.iter_mut().enumerate() {
        let row = Arc::make_mut(&mut placed.row);
        let range = first..first.saturating_add(row.glyphs.len());
        first = range.end.saturating_add(usize::from(placed.ends_with_newline));
        if right_to_left.get(index).copied().unwrap_or(false) {
            continue;
        }
        let Some(mut spots) = target.get(range.clone()).map(<[_]>::to_vec) else { continue };
        let row_chars = chars.get(range).unwrap_or_default();
        // Spaces between two placed characters share the gap between them; at the row's ends
        // they sit against the nearest placed character.
        let mut i = 0;
        while i < spots.len() {
            if spots.get(i).is_some_and(Option::is_some) {
                i += 1;
                continue;
            }
            let from = i;
            while i < spots.len() && spots.get(i).is_some_and(Option::is_none) {
                i += 1;
            }
            if row_chars.get(from..i).is_none_or(|c| !c.iter().all(|c| c.is_whitespace())) {
                break;
            }
            let left = from.checked_sub(1).and_then(|p| spots.get(p).copied().flatten()).map(|(at, w)| at + w);
            let right = spots.get(i).copied().flatten().map(|(at, _)| at);
            let (left, right) = match (left, right) {
                (Some(l), Some(r)) => (l, r.max(l)),
                (Some(l), None) => (l, l),
                (None, Some(r)) => (r, r),
                (None, None) => break,
            };
            let width = (right - left) / (i - from) as f32;
            for (k, spot) in spots.iter_mut().enumerate().take(i).skip(from) {
                *spot = Some((left + width * (k - from) as f32, width));
            }
        }
        let placed_all: Option<Vec<(f32, f32)>> = spots.into_iter().collect();
        let Some(spots) = placed_all else { continue };
        if spots.iter().any(|(at, w)| !at.is_finite() || !w.is_finite()) || spots.windows(2).any(|p| p[1].0 + 0.01 < p[0].0) {
            continue;
        }
        for (glyph, (at, width)) in row.glyphs.iter_mut().zip(spots) {
            let new_x = at - placed.pos.x;
            let dx = new_x - glyph.pos.x;
            glyph.pos.x = new_x;
            glyph.advance_width = width;
            if !glyph.uv_rect.is_nothing() {
                for vertex in row.visuals.mesh.vertices.iter_mut().skip(glyph.first_vertex as usize).take(4) {
                    vertex.pos.x += dx;
                }
            }
        }
        row.size.x = row.glyphs.iter().map(|g| g.pos.x + g.advance_width).fold(row.size.x.min(0.0), f32::max);
    }
}

/// [`place_on_source_words`] for the rows of right-to-left paragraphs, which egui lays out with the
/// bidirectional algorithm: a word's letters, and the words of a row, don't run left to right in
/// typed order (a number inside Arabic runs left to right within it). Each word's laid-out extent
/// is moved onto its box on the page, paired with the boxes from left to right, and everything
/// between the words is stretched with them. Returns the rows placed; a row whose words can't all
/// be paired, or whose boxes don't keep their order, is left to the left-to-right placement.
/// A left and a right edge (galley x).
type Span = (f32, f32);

/// The boxes of a run's `count` words, left to right (user space x), from its `ink`: one box per
/// word, or one per glyph (Word draws right-to-left text a glyph or a few at a time), joined at the
/// widest gaps between them. `None` when the gaps don't tell the words apart.
fn word_boxes(ink: &[[f64; 4]], count: usize, size: f64) -> Option<Vec<(f64, f64)>> {
    let mut boxes: Vec<(f64, f64)> = ink.iter().map(|b| (b[0].min(b[2]), b[0].max(b[2]))).collect();
    boxes.sort_by(|a, b| a.0.total_cmp(&b.0));
    if count == 0 || boxes.len() < count {
        return None;
    }
    if boxes.len() == count {
        return Some(boxes);
    }
    // The gap before each box, from the right edge of everything left of it.
    let mut reach = f64::NEG_INFINITY;
    let mut gaps: Vec<(f64, usize)> = Vec::with_capacity(boxes.len());
    for (i, b) in boxes.iter().enumerate() {
        if i > 0 {
            gaps.push((b.0 - reach, i));
        }
        reach = reach.max(b.1);
    }
    gaps.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (between, within) = gaps.split_at(count - 1);
    // The word gaps must stand out from the gaps inside words.
    let narrowest = between.last().map_or(f64::INFINITY, |g| g.0);
    let widest_inside = within.first().map_or(f64::NEG_INFINITY, |g| g.0);
    if narrowest <= 0.0 || narrowest <= widest_inside + size * 0.02 {
        return None;
    }
    let mut starts: Vec<usize> = between.iter().map(|g| g.1).collect();
    starts.sort_unstable();
    let mut words = Vec::with_capacity(count);
    let mut from = 0;
    for end in starts.into_iter().chain(std::iter::once(boxes.len())) {
        let part = boxes.get(from..end)?;
        words.push(part.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(l, r), b| (l.min(b.0), r.max(b.1))));
        from = end;
    }
    Some(words)
}

fn place_right_to_left_rows(
    g: &mut egui::Galley,
    block: &pdfcraft_engine::TextBlock,
    lines: &[pdfcraft_engine::TextLine],
    chars: &[char],
    starts: &[usize],
    x: &dyn Fn(f64) -> f32,
) -> Vec<bool> {
    let mut placed_rows = vec![false; g.rows.len()];
    let rtl: Vec<bool> = g.rows.iter().map(|p| p.row.glyphs.iter().any(|glyph| glyph.is_rtl())).collect();
    if !rtl.contains(&true) {
        return placed_rows;
    }
    // Each character's row and laid-out left and right edges, in galley coordinates.
    let mut spot: Vec<Option<(usize, f32, f32)>> = vec![None; chars.len()];
    let mut at = 0usize;
    for (r, placed) in g.rows.iter().enumerate() {
        for (k, glyph) in placed.row.glyphs.iter().enumerate() {
            if let Some(slot) = spot.get_mut(at + k) {
                let left = placed.pos.x + glyph.pos.x;
                *slot = Some((r, left, left + glyph.advance_width.max(0.0)));
            }
        }
        at = at.saturating_add(placed.row.glyphs.len()).saturating_add(usize::from(placed.ends_with_newline));
    }
    if at != chars.len() {
        return placed_rows;
    }
    // Per row, (laid out, on the page) pairs of x: each word's left and right edges.
    let mut anchors: Vec<Vec<(f32, f32)>> = vec![Vec::new(); g.rows.len()];
    let mut broken = vec![false; g.rows.len()];
    for (range, index) in &block.spans {
        let Some(line) = lines.get(*index) else { continue };
        let (first, end) = (starts.partition_point(|b| *b < range.start), starts.partition_point(|b| *b < range.end));
        let mut words = Vec::new();
        let mut i = first;
        while i < end {
            if chars.get(i).is_some_and(|c| c.is_whitespace()) {
                i += 1;
                continue;
            }
            let from = i;
            while i < end && chars.get(i).is_some_and(|c| !c.is_whitespace()) {
                i += 1;
            }
            words.push(from..i);
        }
        // A word's laid-out extent, when all of it is on one row.
        let extent = |word: &std::ops::Range<usize>| {
            let mut found: Option<(usize, f32, f32)> = None;
            for i in word.clone() {
                let (r, l, rt) = spot.get(i).copied().flatten()?;
                found = match found {
                    None => Some((r, l, rt)),
                    Some((fr, fl, frt)) if fr == r => Some((r, fl.min(l), frt.max(rt))),
                    Some(_) => return None,
                };
            }
            found
        };
        let Some(laid) = words.iter().map(extent).collect::<Option<Vec<_>>>() else { continue };
        let Some(&(row, _, _)) = laid.first() else { continue };
        if !rtl.get(row).copied().unwrap_or(false) {
            continue;
        }
        if laid.iter().any(|(r, _, _)| *r != row) {
            broken[row] = true;
            continue;
        }
        // Left and right edges: of the words as laid out, and of their boxes on the page.
        let words_ink = word_boxes(&line.ink, words.len(), line.size);
        let (mut laid, mut boxes): (Vec<Span>, Vec<Span>) = if let Some(ink) = words_ink {
            (laid.iter().map(|(_, l, r)| (*l, *r)).collect(), ink.iter().map(|b| (x(b.0), x(b.1))).collect())
        } else {
            let whole = laid.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(l, r), (_, wl, wr)| (l.min(*wl), r.max(*wr)));
            (vec![whole], vec![(x(line.rect[0]), x(line.rect[2]))])
        };
        laid.sort_by(|a, b| a.0.total_cmp(&b.0));
        boxes.sort_by(|a, b| a.0.total_cmp(&b.0));
        if let Some(row_anchors) = anchors.get_mut(row) {
            for ((l, r), (bl, br)) in laid.into_iter().zip(boxes) {
                row_anchors.push((l, bl));
                row_anchors.push((r, br));
            }
        }
    }
    for (index, placed) in g.rows.iter_mut().enumerate() {
        let Some(mut pairs) = anchors.get(index).cloned() else { continue };
        if pairs.is_empty() || broken.get(index).copied().unwrap_or(true) {
            continue;
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Neighbouring words' ink can overlap (a glyph overhangs its advance): those edges meet
        // halfway.
        for i in 1..pairs.len() {
            if pairs[i].1 < pairs[i - 1].1 {
                let middle = (pairs[i].1 + pairs[i - 1].1) / 2.0;
                pairs[i - 1].1 = middle;
                pairs[i].1 = middle;
            }
        }
        if pairs.iter().any(|(l, p)| !l.is_finite() || !p.is_finite()) || pairs.windows(2).any(|w| w[1].1 + 0.01 < w[0].1) {
            continue;
        }
        // Piecewise linear from laid-out x to page x; past the ends, moved with the nearest word.
        let map = |v: f32| -> f32 {
            let (Some(head), Some(tail)) = (pairs.first(), pairs.last()) else { return v };
            if v <= head.0 {
                return head.1 + (v - head.0);
            }
            if v >= tail.0 {
                return tail.1 + (v - tail.0);
            }
            pairs
                .windows(2)
                .find(|w| v >= w[0].0 && v <= w[1].0)
                .map_or(v, |w| if w[1].0 - w[0].0 > 1e-3 { w[0].1 + (v - w[0].0) * (w[1].1 - w[0].1) / (w[1].0 - w[0].0) } else { w[0].1 })
        };
        let origin = placed.pos.x;
        let row = Arc::make_mut(&mut placed.row);
        for glyph in &mut row.glyphs {
            let left = map(origin + glyph.pos.x) - origin;
            let right = map(origin + glyph.pos.x + glyph.advance_width.max(0.0)) - origin;
            let dx = left - glyph.pos.x;
            glyph.pos.x = left;
            glyph.advance_width = (right - left).max(0.0);
            if !glyph.uv_rect.is_nothing() {
                for vertex in row.visuals.mesh.vertices.iter_mut().skip(glyph.first_vertex as usize).take(4) {
                    vertex.pos.x += dx;
                }
            }
        }
        row.size.x = row.glyphs.iter().map(|glyph| glyph.pos.x + glyph.advance_width).fold(row.size.x.min(0.0), f32::max);
        if let Some(done) = placed_rows.get_mut(index) {
            *done = true;
        }
    }
    placed_rows
}

/// Caret colour that stays visible on a field filled with `fill`: black on light fills, white on dark ones.
pub(crate) fn caret_color_on(fill: Color32) -> Color32 {
    let luma = 299 * u32::from(fill.r()) + 587 * u32::from(fill.g()) + 114 * u32::from(fill.b());
    if luma > 127_500 { Color32::BLACK } else { Color32::WHITE }
}
