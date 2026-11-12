//! A reported, shaped Arabic fallback for source subsets that cannot draw an edited paragraph.
//! The source operators are replaced in place; surrounding text and graphics remain untouched.

use super::interp::{Carry, Shown, Ts, interpret};
use super::{
    BlockStyle, Dict, Document, EditError, HashMap, LineEdit, Matrix, Metrics, Object, Op, TextBlock, TextLine, coincident_ops, content_streams,
    page_dict, parse, reveal_ops, rewritten_contents, splice,
};
use pdfcraft_fonts::appearance_text;

/// Whether the Arabic fallback can draw this edit at all: Arabic text (or a paragraph it wrote
/// before), only the paragraph-wide formatting it supports, and upright source rows that share
/// one transform. Runs may differ in size, colour, weight and font: each keeps its own look.
pub(super) fn eligible(members: &[&TextLine], text: &str, style: &BlockStyle) -> bool {
    let Some(first) = members.first().copied() else { return false };
    let own_fallback = members.iter().all(|line| line.font.starts_with("PCEdArabic"));
    !(pdfcraft_fonts::document_arabic_font().is_none()
        // Arabic, and the text that needs shaping too and has a bundled face: Devanagari (a glyph
        // per character can't draw its conjuncts) and pointed Hebrew (nor its niqqud). Thai has a
        // path of its own.
        || (!own_fallback
            && !text.chars().any(|c| {
                matches!(u32::from(c), 0x0600..=0x06ff | 0x0750..=0x077f | 0x0870..=0x08ff | 0xfb50..=0xfdff | 0xfe70..=0xfefc)
                    || (matches!(u32::from(c), 0x0900..=0x097f) && pdfcraft_fonts::document_devanagari_font().is_some())
                    || (crate::bidi::is_hebrew_mark(c) && pdfcraft_fonts::document_hebrew_font(false).is_some())
            }))
        || !style.ranges.is_empty()
        || style.aligns.iter().any(Option::is_some)
        || style.resource.is_some()
        || style.family.is_some()
        || style.char_spacing.is_some()
        || style.scale.is_some()
        || style.line_spacing.is_some()
        || style.underline == Some(true)
        || style.align == Some(crate::added::Align::Justify)
        || members.iter().any(|line| !line.upright() || line.origin.ctm.iter().zip(first.origin.ctm).any(|(a, b)| (a - b).abs() > 1e-9)))
}

/// Whether a uniform paragraph (one size, colour and face) must take the Arabic fallback up front
/// because no complete source face can draw it. A mixed paragraph first tries its source fonts,
/// run by run, and takes the fallback only when they cannot draw the edit (see
/// `rewrite_block_from`).
pub(super) fn needs_fallback(doc: &Document, fonts: &Dict, members: &[&TextLine], text: &str, style: &BlockStyle) -> bool {
    let Some(first) = members.first().copied() else { return false };
    let own_fallback = members.iter().all(|line| line.font.starts_with("PCEdArabic"));
    if !eligible(members, text, style)
        || members.iter().any(|line| {
            line.size != first.size
                || line.color != first.color
                || (!own_fallback && pdfcraft_fonts::embedding::face_key(&line.base_font) != pdfcraft_fonts::embedding::face_key(&first.base_font))
        })
    {
        return false;
    }
    // Prefer a usable complete source face. A fallback never replaces a face capable of the edit.
    let family = pdfcraft_fonts::embedding::face_key(&first.base_font);
    let reusable = |bytes: &[u8]| {
        pdfcraft_fonts::embedding::covers(bytes, text)
            && pdfcraft_fonts::embedding::validate(bytes).is_ok()
            && text.split('\n').all(|line| pdfcraft_fonts::source_shaping::shape(bytes, line).is_ok())
    };
    !fonts.iter().filter_map(|(_, font)| doc.dict(font)).any(|font| {
        pdfcraft_fonts::embedding::face_key(&Metrics::from_dict(doc, &font).base_font) == family
            && pdfcraft_fonts::pdf::embedded_sfnt(doc, &font).is_some_and(|bytes| reusable(&bytes))
    }) && !pdfcraft_fonts::embedding::matching_faces(&first.base_font).into_iter().any(|face| reusable(face.bytes))
}

pub(super) fn rewrite(
    doc: &mut Document,
    page: usize,
    block: &TextBlock,
    lines: &[TextLine],
    draft: &str,
    style: &BlockStyle,
) -> Result<LineEdit, EditError> {
    let members: Vec<_> = block.lines.iter().filter_map(|index| lines.get(*index)).collect();
    let first = members.first().copied().ok_or_else(|| EditError::Invalid("the paragraph has no source runs".into()))?;
    // The source path's refusals that don't depend on its fonts hold for the fallback too.
    members.iter().try_for_each(|line| line.ensure_editable())?;
    let page = page_dict(doc, page)?;
    let size = style.size.unwrap_or(first.size);
    let color = style.color.unwrap_or(first.color);
    let sources = latin_sources(doc, &page.dict, &members, lines);
    // Every run keeps its source look (a size, colour or weight asked for applies to all of them).
    // Italic is for the Latin runs: Arabic script has no italic and stays upright. Latin stays in
    // the run's own font where it has the glyphs, unless another weight is asked for.
    let look = |line: &TextLine| appearance_text::SpanStyle {
        size: style.size.unwrap_or(line.size),
        color: style.color.unwrap_or(line.color),
        bold: style.bold.unwrap_or(line.bold),
        italic: line.italic,
        latin: sources.iter().position(|s| s.resource == line.font).filter(|_| style.bold.is_none_or(|bold| bold == line.bold)),
    };
    let spans: Vec<_> = row_sources(block, draft, block.edited_spans(draft))
        .into_iter()
        .map(|(range, index)| (range, look(lines.get(index).unwrap_or(first))))
        .collect();
    if spans.iter().any(|(_, s)| !s.size.is_finite() || !(0.1..=500.0).contains(&s.size) || s.color.iter().any(|v| !v.is_finite())) {
        return Err(EditError::Invalid("the paragraph's runs must have finite sizes between 0.1 and 500 points and finite colours".into()));
    }
    if spans.iter().any(|(range, s)| s.bold && draft.get(range.clone()).is_some_and(|t| t.chars().any(pdfcraft_fonts::arabic_has)))
        && !pdfcraft_fonts::arabic_bold_available()
    {
        return Err(EditError::Invalid(
            "bold Arabic needs a variable Arabic face with a weight axis, which this build doesn't include; choose a complete bold font".into(),
        ));
    }
    let [dx, dy] = style.offset.unwrap_or([0.0, 0.0]);
    if !size.is_finite() || !(0.1..=500.0).contains(&size) || [dx, dy].iter().chain(color.iter()).any(|v| !v.is_finite()) {
        return Err(EditError::Invalid("the paragraph's position, size and colour must be finite".into()));
    }
    let rtl = appearance_text::paragraph_rtl(draft);
    let natural = if block.rows.len() == 1 {
        let available = if rtl { block.rect[2] + dx - (page.crop(doc)[0] + 36.0) } else { page.crop(doc)[2] - 36.0 - block.rect[0] - dx };
        available.max(block.rect[2] - block.rect[0])
    } else {
        block.rect[2] - block.rect[0]
    };
    let width = style.width.unwrap_or(natural);
    if !width.is_finite() || !(0.1..=14_400.0).contains(&width) {
        return Err(EditError::Invalid("the paragraph's width must be between 0.1 and 14400 points".into()));
    }
    if let Some(c) = appearance_text::first_undrawable(draft) {
        return Err(crate::undrawable(c));
    }
    // Word and browsers tighten rows with `TJ` kerning that this drawing doesn't repeat, so a
    // source row laid out again is a little wider than it was drawn. Rows as wide as the
    // paragraph's own source rows still fit (by at most 5%), so an edit doesn't push a row's last
    // word onto a row of its own over the next paragraph.
    let wrap_width = if style.width.is_some() {
        width
    } else {
        block
            .rows
            .iter()
            .filter_map(|row| {
                let line = row.first().and_then(|i| lines.get(*i))?;
                let text: String = row.iter().filter_map(|i| lines.get(*i)).map(|l| l.text.as_str()).collect();
                appearance_text::wrap_styled_with(&text, size, 14_400.0, look(line), &[], &sources).ok()?.first().map(|l| l.width)
            })
            .fold(width, f64::max)
            .min(width * 1.05)
    };
    let laid = appearance_text::wrap_styled_with(draft, size, wrap_width, look(first), &spans, &sources)
        .map_err(|_| EditError::Invalid("the Arabic paragraph cannot be shaped within its limits".into()))?;
    let draws_fallback = appearance_text::draws_shaped(&laid);
    let align = style.align.unwrap_or(if rtl { crate::added::Align::Right } else { crate::added::Align::Left });
    let left = if rtl && style.width.is_none() && style.align.is_none() { block.rect[2] - width } else { block.rect[0] } + dx;
    let top = first.origin.baseline + size * laid.first().map_or(0.9, |line| line.ascent) + dy;
    let rect = [left, top - appearance_text::height(&laid, size), left + width, top];
    let alignment = match align {
        crate::added::Align::Center => 1,
        crate::added::Align::Right => 2,
        _ => 0,
    };
    let unchanged = unchanged_rows(block, draft, style);
    let layout = row_positions(block, lines, draft, &laid, style, size, &unchanged);
    let kept = layout.as_ref().map_or_else(|| vec![false; block.rows.len()], |layout| layout.kept.clone());
    let drawn = match layout {
        // Each edited row stays on its source row's baseline and against the edge it was set
        // against, so a heading and its subtitle keep their spacing. Rows the edit leaves alone
        // keep their own drawing and aren't drawn again.
        Some(layout) => {
            let (redrawn, positions): (Vec<_>, Vec<_>) = laid
                .iter()
                .zip(&layout.positions)
                .zip(&layout.owner)
                .filter(|(_, row)| !kept.get(**row).copied().unwrap_or(false))
                .map(|((line, at), _)| (line.clone(), *at))
                .unzip();
            appearance_text::draw_positioned(&redrawn, size, &positions, Some(color))
        }
        None => match source_pitch(block, lines) {
            // A reflowed paragraph keeps its own line pitch where its rows have ink-safe room.
            Some(pitch) => {
                let mut positions = Vec::with_capacity(laid.len());
                let mut y = top - size * laid.first().map_or(0.9, |line| line.ascent);
                for (i, line) in laid.iter().enumerate() {
                    if let Some(previous) = i.checked_sub(1).and_then(|p| laid.get(p)) {
                        y -= pitch.max(size * (previous.descent + line.ascent));
                    }
                    let x = match alignment {
                        1 => rect[0] + (rect[2] - rect[0] - line.width) / 2.0,
                        2 => rect[2] - line.width,
                        _ => rect[0],
                    };
                    positions.push([x, y]);
                }
                appearance_text::draw_positioned(&laid, size, &positions, Some(color))
            }
            None => appearance_text::draw(&laid, size, rect, alignment, color),
        },
    };
    let (content, template) = drawn.map_err(|_| EditError::Invalid("the Arabic paragraph's glyphs exceed the drawing limits".into()))?;
    let inverse = Matrix(first.origin.ctm).invert().ok_or_else(|| EditError::Invalid("the paragraph has a singular transform".into()))?;
    let reveal = reveal_ops(block, lines, style, first.origin.ctm)?;
    let streams = content_streams(doc, &page.dict)?;
    let pieces = crate::text::joined(&streams);
    let ops = pieces.parse().ops;
    // The source runs drawn again: those of every row the edit doesn't leave as it was.
    let kept_lines: std::collections::HashSet<usize> =
        block.rows.iter().zip(&kept).filter(|(_, kept)| **kept).flat_map(|(row, _)| row.iter().copied()).collect();
    let redrawn: Vec<&TextLine> = block.lines.iter().filter(|i| !kept_lines.contains(i)).filter_map(|i| lines.get(*i)).collect();
    let anchor = redrawn.first().copied().unwrap_or(first);
    let origin = &anchor.origin;
    // Validate the splice before materializing any new font objects.
    let at = anchor
        .ops
        .iter()
        .copied()
        .find(|index| ops.get(*index).is_some_and(|op| matches!(op.op.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"")))
        .ok_or_else(|| EditError::Invalid("the paragraph has no text-show operation".into()))?;
    // Marked paragraph boundaries are authoritative. A later edit can wrap close to a
    // neighbouring paragraph; overlap alone must not turn that paragraph into a duplicate.
    let same_scope: Vec<_> = lines.iter().filter(|line| members.iter().any(|target| line.scope == target.scope)).cloned().collect();
    let mut drop = coincident_ops(&same_scope, &redrawn);
    drop.extend(redrawn.iter().flat_map(|line| line.ops.iter().copied()));
    let mut resources = page.dict.get(b"Resources").and_then(|o| doc.dict(o)).unwrap_or_default();
    let mut fonts = resources.get(b"Font").and_then(|o| doc.dict(o)).unwrap_or_default();
    let shown = interpret(doc, &ops, &fonts, &mut HashMap::new(), &mut Carry::new(), None, None)?;
    let first_shown = shown.iter().find(|s| s.op == at).ok_or_else(|| EditError::Invalid("the source text state cannot be restored".into()))?;
    // Closing a text object applies its accumulated text clip. Doing so early, even before a
    // different selected run, would change later text and artwork in the same object.
    if shown.iter().any(|s| s.bt == first_shown.bt && s.state.render_mode >= 4) {
        return Err(EditError::Invalid("text used as a clipping path cannot use the Arabic fallback".into()));
    }
    let restore = restore_text_matrix(first_shown)?;
    let mut patches = HashMap::new();
    for source in shown.iter().filter(|s| drop.contains(&s.op)) {
        if matches!(source.state.render_mode, 1 | 2) {
            return Err(EditError::Invalid("the Arabic fallback cannot preserve stroked source text; choose a complete font".into()));
        }
        if source.state.render_mode >= 4 {
            return Err(EditError::Invalid("text used as a clipping path cannot use the Arabic fallback".into()));
        }
        patches.insert(source.op, source_advance(source, &ops)?);
    }
    let mut prefix = patches.remove(&at).ok_or_else(|| EditError::Invalid("the source text advance cannot be restored".into()))?;
    let advance = prefix.pop().ok_or_else(|| EditError::Invalid("the source text advance cannot be restored".into()))?;
    let template = appearance_text::materialize_fonts(doc, &template);
    let mut renamed = HashMap::new();
    for (old, font) in template.iter() {
        let stem = format!("PCEdArabic{}", String::from_utf8_lossy(old));
        let mut name = stem.clone();
        let mut suffix = 0usize;
        while fonts.contains(name.as_bytes()) {
            suffix = suffix.saturating_add(1);
            name = format!("{stem}_{suffix}");
        }
        fonts.set(name.clone().into_bytes(), font.clone());
        renamed.insert(old.clone(), name.into_bytes());
    }
    let mut drawing = parse(&content).ops;
    for op in &mut drawing {
        if op.is("Tf")
            && let Some(Object::Name(old)) = op.operands.first_mut()
            && let Some(name) = renamed.get(old)
        {
            *old = name.clone();
        }
    }
    let mut insert = vec![Op::new("ET", vec![]), Op::new("q", vec![])];
    insert.extend(reveal);
    // Generated glyph matrices already include point size and absolute positions. Do not inherit
    // the source's spacing, horizontal scale, rise, clipping or invisible rendering mode.
    insert.extend([
        Op::new("Tc", vec![Object::Int(0)]),
        Op::new("Tw", vec![Object::Int(0)]),
        Op::new("Tz", vec![Object::Int(100)]),
        Op::new("Ts", vec![Object::Int(0)]),
        Op::new("Tr", vec![Object::Int(0)]),
    ]);
    insert.push(Op::new("cm", inverse.0.map(Object::Real).to_vec()));
    insert.extend(drawing);
    insert.push(Op::new("Q", vec![]));
    // Insert at the captured show operation, after any source cm/q/Q between BT and the text.
    // Closing and reopening the text object keeps that exact graphics state for the inverse CTM.
    insert.push(Op::new("BT", vec![]));
    insert.extend(origin.state.ops());
    insert.extend(restore);
    // A quote's line move and spacing changes happen before the insertion. Every removed show
    // retains its original numeric advance, so surviving Tj/TJ and T* siblings keep both matrices.
    prefix.extend(insert);
    prefix.push(advance);
    patches.insert(at, prefix);
    let data = splice(&pieces, &ops, |i| (patches.remove(&i).unwrap_or_default(), !drop.contains(&i)));
    let contents = rewritten_contents(doc, &streams, data)?;
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(page.obj, |dict| {
        dict.set(b"Contents".to_vec(), contents);
        dict.set(b"Resources".to_vec(), Object::Dict(resources));
    })?;
    // Only a paragraph that draws with the fallback face reports it: one whose every word the
    // page's own fonts already had keeps them.
    let substituted = if draws_fallback { pdfcraft_fonts::document_arabic_font().map(|font| format!("{} Type3", font.family)) } else { None };
    Ok(LineEdit { substituted })
}

/// A character's bytes in a source font and its advance (em).
type SourceGlyph = (Vec<u8>, f64);

/// The fonts of the paragraph's runs that the fallback may draw Latin in: each page font
/// resource a run shows, with what it has glyphs for (a subset such as Word's Calibri keeps the
/// characters the document uses). The fallback's own fonts are left out, and so is a resource
/// whose name the drawing could not write as it is.
fn latin_sources<'a>(doc: &Document, page: &Dict, members: &[&TextLine], lines: &[TextLine]) -> Vec<appearance_text::LatinSource<'a>> {
    let fonts = page.get(b"Resources").and_then(|o| doc.dict(o)).and_then(|r| r.get(b"Font").and_then(|f| doc.dict(f))).unwrap_or_default();
    let mut out: Vec<appearance_text::LatinSource<'a>> = Vec::new();
    for line in members {
        let name = line.font.as_str();
        let regular = !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'+'));
        if !regular || name.starts_with("PCEdArabic") || out.iter().any(|s| s.resource == name) {
            continue;
        }
        let Some(font) = fonts.get(name.as_bytes()).and_then(|f| doc.dict(f)) else { continue };
        let metrics = std::rc::Rc::new(Metrics::from_dict(doc, &font));
        let learned = shown_forms(doc, page, name, &metrics, lines);
        let shaped = metrics.clone();
        let word = move |clusters: &[pdfcraft_fonts::ShapedCluster]| shown_word(&learned, &shaped, clusters);
        let seen: std::cell::RefCell<HashMap<char, Option<SourceGlyph>>> = Default::default();
        let glyph = move |c: char| seen.borrow_mut().entry(c).or_insert_with(|| source_glyph(&metrics, c)).clone();
        out.push(appearance_text::LatinSource { resource: name.to_owned(), glyph: Box::new(glyph), word: Some(Box::new(word)) });
    }
    out
}

/// The bytes that show `c` in a source font and their advance (em), when its encoding has it.
fn source_glyph(metrics: &Metrics, c: char) -> Option<SourceGlyph> {
    let bytes = metrics.encode(c.encode_utf8(&mut [0; 4]))?;
    let advance: f64 = metrics.codes(&bytes).iter().map(|(code, _)| metrics.width(*code)).sum();
    (advance.is_finite() && advance >= 0.0).then_some((bytes, advance))
}

/// What names a letter form: the shaped face's glyphs for it and the characters it shows.
type FormKey = (Vec<u32>, String);

/// A mark the page draws on a letter: the bytes that show it and its origin from the letter's (em).
type SeenMark = (Vec<u8>, [f64; 2]);

/// What a page teaches about one of its fonts (see [`shown_forms`]). `None` is a form, or the
/// marks on one, that the page draws in two different ways: it is never guessed.
#[derive(Default)]
struct Learned {
    /// The glyph of each letter form.
    forms: HashMap<FormKey, Option<SourceGlyph>>,
    /// The marks on a letter form, for the marks on it ([`same_marks`]).
    marks: HashMap<(FormKey, String), Option<Vec<SeenMark>>>,
    /// The words the page draws, each as the page first draws it: a word still in the text is
    /// drawn again glyph for glyph, whatever its forms and marks.
    words: HashMap<WordKey, SeenRun>,
}

/// A word as a page draws it: its letters from left to right, each with its advance (em), and
/// its marks, each with the letter it sits on and its origin from that letter's (em).
struct SeenRun {
    letters: Vec<SourceGlyph>,
    marks: Vec<(usize, Vec<u8>, [f64; 2])>,
}

fn form_key(cluster: &pdfcraft_fonts::ShapedCluster) -> FormKey {
    (cluster.glyphs.iter().map(|(id, _)| *id).collect(), cluster.text.clone())
}

/// The key of a ligature the source draws as one glyph and the shaped face as two clusters
/// (lam-alef): `first` is drawn left of `second`, so in a right-to-left word it is read after it.
fn pair_key(first: &pdfcraft_fonts::ShapedCluster, second: &pdfcraft_fonts::ShapedCluster) -> FormKey {
    (first.glyphs.iter().chain(&second.glyphs).map(|(id, _)| *id).collect(), format!("{}{}", second.text, first.text))
}

/// Punctuation and digits drawn beside Arabic letters that have one glyph whatever is next to
/// them and no mirrored form, so the font's own glyph for the character is the right one.
fn plain_beside_letters(c: char) -> bool {
    matches!(
        c,
        ' ' | '.' | ',' | ':' | ';' | '!' | '?' | '-' | '/' | '%' | '"' | '\'' | '\u{060c}' | '\u{061b}' | '\u{061f}' | '\u{066a}'..='\u{066c}'
    ) || c.is_ascii_digit()
        || matches!(u32::from(c), 0x0660..=0x0669 | 0x06f0..=0x06f9)
}

/// A shaped word (its clusters in the order they are drawn) in a source font, when the font can
/// draw all of it: `None` when any letter form, or any mark on one, is something the page never
/// draws.
///
/// The punctuation and digits around the letters take the glyphs the font's encoding has. The
/// letters between them are drawn as the page draws that very word when it has it (see
/// [`Learned::words`]), else form by form (see [`letters_by_form`]).
fn shown_word(learned: &Learned, metrics: &Metrics, clusters: &[pdfcraft_fonts::ShapedCluster]) -> Option<appearance_text::SourceWord> {
    let mut word = appearance_text::SourceWord::default();
    let plain = |cluster: &pdfcraft_fonts::ShapedCluster| {
        let mut chars = cluster.text.chars();
        chars.next().filter(|c| chars.next().is_none() && plain_beside_letters(*c))
    };
    for run in clusters.chunk_by(|a, b| plain(a).is_some() == plain(b).is_some()) {
        if let Some(first) = run.first().filter(|first| plain(first).is_some()) {
            // Tried cluster by cluster: a comma a font draws needs no letter beside it.
            let _ = first;
            for cluster in run {
                let (bytes, advance) = source_glyph(metrics, plain(cluster)?)?;
                word.bytes.extend(bytes);
                word.advance += advance;
            }
            continue;
        }
        match word_key(run).and_then(|key| learned.words.get(&key)) {
            Some(seen) => {
                let mut starts = Vec::with_capacity(seen.letters.len());
                for (bytes, advance) in &seen.letters {
                    starts.push(word.advance);
                    word.bytes.extend(bytes);
                    word.advance += advance;
                }
                for (letter, bytes, [dx, dy]) in &seen.marks {
                    word.marks.push(appearance_text::SourceMark { bytes: bytes.clone(), at: [starts.get(*letter)? + dx, *dy] });
                }
            }
            None => letters_by_form(learned, run, &mut word)?,
        }
    }
    Some(word)
}

/// What names a word whatever order its marks were typed or shaped in: its letters as read, and
/// the marks on each.
type WordKey = (String, Vec<String>);

/// The [`WordKey`] of shaped clusters in the order they are drawn, right to left: read from the
/// last one, each letter is followed by its marks.
fn word_key(clusters: &[pdfcraft_fonts::ShapedCluster]) -> Option<WordKey> {
    let mut letters = String::new();
    let mut marks: Vec<Vec<char>> = Vec::new();
    for c in clusters.iter().rev().flat_map(|cluster| cluster.text.chars()) {
        if letter_mark(c) {
            marks.last_mut()?.push(c);
        } else if joining_letter(c) {
            letters.push(c);
            marks.push(Vec::new());
        } else {
            return None;
        }
    }
    Some((letters, marks.into_iter().map(|on| same_marks(on.into_iter())).collect()))
}

/// Adds shaped letters and their marks (in the order they are drawn) to `word`, each letter in
/// the form the page already draws and each mark where the page draws it on that form. In a
/// right-to-left word the marks typed after a letter are drawn before it, so they are held until
/// their letter comes.
fn letters_by_form(learned: &Learned, clusters: &[pdfcraft_fonts::ShapedCluster], word: &mut appearance_text::SourceWord) -> Option<()> {
    let mut held: Vec<&str> = Vec::new();
    let mut rest = clusters;
    while let Some((first, tail)) = rest.split_first() {
        if !first.text.is_empty() && first.text.chars().all(letter_mark) {
            held.push(&first.text);
            rest = tail;
            continue;
        }
        let pair = tail.first().and_then(|second| learned.forms.get(&pair_key(first, second)).cloned().flatten());
        let (key, glyph, used) = match pair {
            Some(glyph) => (None, glyph, 2),
            None => {
                // A letter the shaped face keeps in one cluster with its marks.
                let key = if first.text.chars().any(letter_mark) {
                    let letters: String = first.text.chars().filter(|c| !letter_mark(*c)).collect();
                    let base = first.glyphs.iter().map(|(id, _)| vec![*id]).find(|id| learned.forms.contains_key(&(id.clone(), letters.clone())))?;
                    held.push(first.text.get(letters.len()..)?);
                    (base, letters)
                } else {
                    form_key(first)
                };
                (Some(key.clone()), learned.forms.get(&key).cloned().flatten()?, 1)
            }
        };
        if !held.is_empty() {
            let typed = same_marks(held.drain(..).flat_map(str::chars));
            let seen = learned.marks.get(&(key?, typed)).cloned().flatten()?;
            word.marks.extend(seen.into_iter().map(|(bytes, [dx, dy])| appearance_text::SourceMark { bytes, at: [word.advance + dx, dy] }));
        }
        word.bytes.extend(glyph.0);
        word.advance += glyph.1;
        rest = rest.get(used..).unwrap_or_default();
    }
    held.is_empty().then_some(())
}

/// A letter whose glyph depends on its neighbours and that a source glyph can stand for: Arabic
/// letters, without the marks drawn on them.
fn joining_letter(c: char) -> bool {
    matches!(u32::from(c), 0x0621..=0x064a | 0x066e..=0x066f | 0x0671..=0x06d3 | 0x06fa..=0x06ff | 0x0750..=0x077f)
}

/// A mark drawn on a letter (harakat, shadda, the small Quranic signs): no letter's form depends on it.
fn letter_mark(c: char) -> bool {
    matches!(u32::from(c), 0x064b..=0x065f | 0x0670 | 0x06d6..=0x06dc | 0x06df..=0x06e4 | 0x06e7..=0x06e8 | 0x06ea..=0x06ed)
}

/// The most glyphs of one font read from a page to learn its letter forms.
const FORM_GLYPHS: usize = 65_536;

/// One glyph a page shows: the bytes that show it, its text, where its origin is on the page and
/// how far it advances, and the size of its em across and up the page.
struct Placed {
    bytes: Vec<u8>,
    text: String,
    at: [f64; 2],
    advance: f64,
    em: [f64; 2],
    /// More glyphs of the same mark, drawn straight after it and beside it, that the font gives
    /// no text of their own: Word draws a shadda with a kasra as two glyphs, names both marks on
    /// the first and calls the second a space.
    with: Vec<(Vec<u8>, [f64; 2])>,
}

/// The glyphs a page's content shows in the font resource `name`, where it draws them upright.
/// Only this font's widths are known, so after a show in another font the pen is unknown until
/// the content places it again.
fn placed_glyphs(ops: &[Op], name: &str, metrics: &Metrics) -> Vec<Placed> {
    let mut out = Vec::new();
    let (mut ctm, mut saved) = (Matrix::IDENTITY, Vec::new());
    let (mut tm, mut line) = (Matrix::IDENTITY, Matrix::IDENTITY);
    let (mut size, mut spacing, mut word_spacing, mut scale, mut leading, mut rise) = (0.0f64, 0.0f64, 0.0f64, 1.0f64, 0.0f64, 0.0f64);
    let (mut in_font, mut lost) = (false, false);
    // Where the last letter sits: what a mark is drawn above or below.
    let mut baseline: Option<f64> = None;
    let number = |op: &Op, at: usize| op.operands.get(at).and_then(Object::as_f64).filter(|v| v.is_finite());
    for op in ops {
        let next_line = |by: [f64; 2], line: &mut Matrix, tm: &mut Matrix, lost: &mut bool| {
            *line = Matrix::translate(by[0], by[1]).then(line);
            *tm = *line;
            *lost = false;
        };
        match op.op.as_slice() {
            // The text state is part of the graphics state: `Q` puts the spacing, font and size back.
            b"q" if saved.len() < 64 => saved.push((ctm, size, spacing, word_spacing, scale, leading, rise, in_font)),
            b"Q" => {
                if let Some(state) = saved.pop() {
                    (ctm, size, spacing, word_spacing, scale, leading, rise, in_font) = state;
                }
            }
            b"cm" => ctm = Matrix::from_operands(&op.operands).map_or(ctm, |m| m.then(&ctm)),
            b"BT" => {
                (tm, line, lost) = (Matrix::IDENTITY, Matrix::IDENTITY, false);
            }
            b"Tf" => {
                in_font = op.operands.first().and_then(Object::as_name) == Some(name.as_bytes());
                size = number(op, 1).unwrap_or(0.0);
            }
            b"Tc" => spacing = number(op, 0).unwrap_or(0.0),
            b"Tw" => word_spacing = number(op, 0).unwrap_or(0.0),
            b"Tz" => scale = number(op, 0).unwrap_or(100.0) / 100.0,
            b"TL" => leading = number(op, 0).unwrap_or(0.0),
            b"Ts" => rise = number(op, 0).unwrap_or(0.0),
            b"Td" => next_line([number(op, 0).unwrap_or(0.0), number(op, 1).unwrap_or(0.0)], &mut line, &mut tm, &mut lost),
            b"TD" => {
                leading = -number(op, 1).unwrap_or(0.0);
                next_line([number(op, 0).unwrap_or(0.0), -leading], &mut line, &mut tm, &mut lost);
            }
            b"T*" => next_line([0.0, -leading], &mut line, &mut tm, &mut lost),
            b"Tm" => {
                if let Some(m) = Matrix::from_operands(&op.operands) {
                    (tm, line, lost) = (m, m, false);
                }
            }
            b"'" => next_line([0.0, -leading], &mut line, &mut tm, &mut lost),
            b"\"" => {
                word_spacing = number(op, 0).unwrap_or(0.0);
                spacing = number(op, 1).unwrap_or(0.0);
                next_line([0.0, -leading], &mut line, &mut tm, &mut lost);
            }
            _ => {}
        }
        if !matches!(op.op.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"") {
            continue;
        }
        if !in_font {
            lost = true;
            continue;
        }
        let shown: Vec<&Object> = match op.operands.last() {
            Some(Object::Array(items)) => items.iter().collect(),
            Some(other) => vec![other],
            None => Vec::new(),
        };
        for item in shown {
            let Some(string) = item.as_string() else {
                if let Some(step) = item.as_f64().filter(|v| v.is_finite()) {
                    tm = Matrix::translate(-step / 1000.0 * size * scale, 0.0).then(&tm);
                }
                continue;
            };
            let mut at = 0usize;
            for (code, len) in metrics.codes(&string.bytes) {
                let bytes = string.bytes.get(at..at.saturating_add(len)).unwrap_or_default().to_vec();
                at = at.saturating_add(len);
                let space = if len == 1 && code == 32 { word_spacing } else { 0.0 };
                let advance = (metrics.width(code) * size + spacing + space) * scale;
                let [a, b, c, d, ..] = tm.then(&ctm).0;
                let origin = tm.then(&ctm).apply(0.0, rise);
                let em = [size * scale * a, size * d];
                let upright = b.abs() <= 1e-6 * a.abs() && c.abs() <= 1e-6 * d.abs() && em.iter().all(|v| v.is_finite() && *v > 0.0);
                if !lost && upright && advance.is_finite() {
                    if out.len() >= FORM_GLYPHS {
                        return out;
                    }
                    let text = metrics.text_of(code).map(std::borrow::Cow::into_owned).unwrap_or_default();
                    let at = [origin.0, origin.1];
                    // A glyph without text, or a "space" drawn off the letters' baseline, straight
                    // after a mark and beside it: the rest of that mark.
                    let nameless =
                        text.is_empty() || (text.chars().all(char::is_whitespace) && baseline.is_some_and(|y: f64| (at[1] - y).abs() > 0.08 * em[1]));
                    let part_of = out.last_mut().filter(|last: &&mut Placed| {
                        nameless
                            && !last.text.is_empty()
                            && last.text.chars().all(letter_mark)
                            && (last.at[0] - at[0]).abs() <= 0.5 * em[0]
                            && last.em == em
                    });
                    match part_of {
                        Some(mark) => mark.with.push((bytes, at)),
                        None => {
                            if !text.is_empty() && text.chars().all(joining_letter) {
                                baseline = Some(at[1]);
                            }
                            out.push(Placed { bytes, text, at, advance: advance * a, em, with: Vec::new() });
                        }
                    }
                }
                if advance.is_finite() {
                    tm = Matrix::translate(advance, 0.0).then(&tm);
                }
            }
        }
    }
    out
}

/// The letters of one word as a page draws them, from left to right, and the marks drawn on it.
struct SeenWord<'a> {
    letters: Vec<&'a Placed>,
    marks: Vec<&'a Placed>,
}

/// The words a page draws in Arabic letters: letters on one baseline at one size, each starting
/// where the one before ends, with the marks drawn over them. Where the glyphs are counts, not
/// the order the content shows them in: Word shows a mark before its letter, in a show of its own.
fn seen_words(glyphs: &[Placed]) -> Vec<SeenWord<'_>> {
    let is = |glyph: &Placed, test: fn(char) -> bool| !glyph.text.is_empty() && glyph.text.chars().all(test);
    let mut bases: Vec<&Placed> = glyphs.iter().filter(|glyph| !is(glyph, letter_mark)).collect();
    // Rows from the top of the page down, each from left to right.
    bases.sort_by(|a, b| b.at[1].total_cmp(&a.at[1]).then(a.at[0].total_cmp(&b.at[0])));
    let mut rows: Vec<Vec<&Placed>> = Vec::new();
    for glyph in bases {
        match rows.last_mut().filter(|row| {
            row.first()
                .is_some_and(|head| (head.at[1] - glyph.at[1]).abs() <= 0.2 * glyph.em[1] && (head.em[0] - glyph.em[0]).abs() <= 1e-3 * glyph.em[0])
        }) {
            Some(row) => row.push(glyph),
            None => rows.push(vec![glyph]),
        }
    }
    let mut words: Vec<SeenWord<'_>> = Vec::new();
    for row in &mut rows {
        row.sort_by(|a, b| a.at[0].total_cmp(&b.at[0]));
        let mut open = false;
        for glyph in row.iter().copied() {
            if !is(glyph, joining_letter) {
                open = false;
                continue;
            }
            let joined = open
                && words
                    .last()
                    .and_then(|word| word.letters.last())
                    .is_some_and(|last| (-0.7..=0.3).contains(&((glyph.at[0] - last.at[0] - last.advance) / glyph.em[0])));
            match words.last_mut().filter(|_| joined) {
                Some(word) => word.letters.push(glyph),
                None => words.push(SeenWord { letters: vec![glyph], marks: Vec::new() }),
            }
            open = true;
        }
    }
    // Each mark belongs to the word it is drawn over: the nearest one, within a third of an em.
    for mark in glyphs.iter().filter(|glyph| is(glyph, letter_mark)) {
        let away = |word: &SeenWord<'_>| {
            let (first, last) = (word.letters.first()?, word.letters.last()?);
            let up = (mark.at[1] - first.at[1]).abs() / first.em[1];
            let across = (first.at[0] - mark.at[0]).max(mark.at[0] - last.at[0] - last.advance).max(0.0) / first.em[0];
            (up <= 0.9 && across <= 0.35 && (first.em[0] - mark.em[0]).abs() <= 1e-3 * first.em[0]).then_some(across + up * 0.01)
        };
        let nearest = words.iter().enumerate().filter_map(|(index, word)| Some((away(word)?, index))).min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some(word) = nearest.and_then(|(_, index)| words.get_mut(index)) {
            word.marks.push(mark);
        }
    }
    words
}

/// What the page already draws in the font resource `name`: the glyph of each Arabic letter form,
/// and where each mark sits on each form. A subset such as Word's keeps the glyphs but no layout
/// tables, so which glyph is which form of a letter, and where a mark goes on it, is only known
/// from the words the page shows: a word's letters are read as drawn (right to left), shaped
/// again, and each glyph is the form shaped at its place.
///
/// Only letters that make a whole word of the page's own text teach anything: part of a word
/// would be shaped as if it ended there. And what the page draws in two different ways is left
/// out, so a guess never puts a wrong glyph or a misplaced mark on the page.
fn shown_forms(doc: &Document, page: &Dict, name: &str, metrics: &Metrics, lines: &[TextLine]) -> Learned {
    let mut learned = Learned::default();
    let Ok(streams) = content_streams(doc, page) else { return learned };
    let ops = crate::text::joined(&streams).parse().ops;
    // The page's words without their marks, each with the ways the page marks it.
    let mut marked: HashMap<String, Vec<&str>> = HashMap::new();
    for word in lines.iter().flat_map(|line| line.text.split(|c: char| !joining_letter(c) && !letter_mark(c))).filter(|word| !word.is_empty()) {
        let ways = marked.entry(word.chars().filter(|c| !letter_mark(*c)).collect()).or_default();
        if !ways.contains(&word) {
            ways.push(word);
        }
    }
    let glyphs = placed_glyphs(&ops, name, metrics);
    for word in seen_words(&glyphs) {
        learn_word(&mut learned, &word, &marked, metrics);
    }
    learned
}

/// Adds what one drawn word teaches (see [`shown_forms`]), when it is one of the page's words.
fn learn_word(learned: &mut Learned, word: &SeenWord<'_>, marked: &HashMap<String, Vec<&str>>, metrics: &Metrics) {
    let logical: String = word.letters.iter().rev().map(|glyph| glyph.text.as_str()).collect();
    let Some(ways) = marked.get(&logical) else { return };
    let glyph_of = |glyph: &Placed| {
        let advance: f64 = metrics.codes(&glyph.bytes).iter().map(|(code, _)| metrics.width(*code)).sum();
        (advance.is_finite() && advance >= 0.0).then(|| (glyph.bytes.clone(), advance))
    };
    // The marks on each drawn letter. A word whose marks can't be told apart teaches nothing.
    let Some((on, marks)) = marks_of(word, ways) else { return };
    // The word as a whole: the first way the page draws it.
    if let Some(letters) = word.letters.iter().map(|glyph| glyph_of(glyph)).collect::<Option<Vec<_>>>() {
        let all = marks.iter().enumerate().flat_map(|(letter, (_, seen))| seen.iter().map(move |(bytes, at)| (letter, bytes.clone(), *at))).collect();
        learned.words.entry((logical.clone(), on)).or_insert(SeenRun { letters, marks: all });
    }
    // Its letter forms: each glyph is one cluster of the word shaped again, or a ligature the
    // shaped face keeps as two.
    let Ok(clusters) = pdfcraft_fonts::shape_arabic(&logical, true) else { return };
    let mut rest = clusters.as_slice();
    let mut keys = Vec::with_capacity(word.letters.len());
    for glyph in &word.letters {
        let (key, used) = match rest {
            [first, ..] if first.text == glyph.text => (form_key(first), 1),
            [first, second, ..] if pair_key(first, second).1 == glyph.text => (pair_key(first, second), 2),
            _ => return,
        };
        rest = rest.get(used..).unwrap_or_default();
        keys.push(key);
    }
    if !rest.is_empty() {
        return;
    }
    for ((key, glyph), (typed, seen)) in keys.into_iter().zip(&word.letters).zip(marks) {
        let form = glyph_of(glyph);
        learned
            .forms
            .entry(key.clone())
            .and_modify(|known| {
                if *known != form {
                    *known = None;
                }
            })
            .or_insert(form);
        // A ligature's marks sit on two letters: they are known for the whole word only.
        if typed.is_empty() || glyph.text.chars().count() != 1 {
            continue;
        }
        let same = |known: &Vec<SeenMark>| {
            known.len() == seen.len()
                && known.iter().zip(&seen).all(|(a, b)| a.0 == b.0 && (a.1[0] - b.1[0]).abs() <= 0.03 && (a.1[1] - b.1[1]).abs() <= 0.03)
        };
        learned
            .marks
            .entry((key, typed))
            .and_modify(|known| {
                if !known.as_ref().is_some_and(same) {
                    *known = None;
                }
            })
            .or_insert(Some(seen));
    }
}

/// The marks on one letter in an order that doesn't depend on how they were typed or shaped: the
/// same marks are the same key.
fn same_marks(marks: impl Iterator<Item = char>) -> String {
    let mut sorted: Vec<char> = marks.collect();
    sorted.sort_unstable();
    sorted.into_iter().collect()
}

/// The marks on each letter of a drawn word (see [`shown_forms`]): for each letter as read, its
/// marks ([`same_marks`]); and for each drawn letter, from left to right, the marks it carries and
/// the glyphs that draw them, each with its origin from the letter's (em).
///
/// The page's text says which letter carries which marks; a mark that sits on several letters of
/// the word is matched to them from right to left. One glyph may draw two marks (Word draws a
/// shadda and its vowel as one). `None` when the text marks the word in more than one way that
/// fits the marks drawn, or in none.
#[allow(clippy::type_complexity)]
fn marks_of(word: &SeenWord<'_>, ways: &[&str]) -> Option<(Vec<String>, Vec<(String, Vec<SeenMark>)>)> {
    let drawn = same_marks(word.marks.iter().flat_map(|mark| mark.text.chars()));
    let marks_on = |way: &str| {
        let mut on: Vec<String> = Vec::new();
        for c in way.chars() {
            match on.last_mut().filter(|_| letter_mark(c)) {
                Some(marks) => marks.push(c),
                None if letter_mark(c) => return None,
                None => on.push(String::new()),
            }
        }
        Some(on.into_iter().map(|marks| same_marks(marks.chars())).collect::<Vec<_>>())
    };
    let mut fitting: Vec<Vec<String>> =
        ways.iter().filter(|way| same_marks(way.chars().filter(|c| letter_mark(*c))) == drawn).filter_map(|way| marks_on(way)).collect();
    fitting.dedup();
    let [on] = fitting.as_slice() else { return None };
    // Each drawn letter with the letters of the text it shows (a ligature shows two), right to left.
    let mut letter = 0usize;
    let mut used = vec![false; word.marks.len()];
    let mut out = Vec::with_capacity(word.letters.len());
    for glyph in word.letters.iter().rev() {
        let shows = glyph.text.chars().count();
        let typed = same_marks(on.iter().skip(letter).take(shows).flat_map(|marks| marks.chars()));
        letter = letter.saturating_add(shows);
        // Until every mark on the letter is drawn: the rightmost glyph not yet taken that draws
        // only marks still missing.
        let mut missing: Vec<char> = typed.chars().collect();
        let mut seen: Vec<SeenMark> = Vec::with_capacity(missing.len());
        let from_letter = |at: [f64; 2]| [(at[0] - glyph.at[0]) / glyph.em[0], (at[1] - glyph.at[1]) / glyph.em[1]];
        while !missing.is_empty() {
            let draws_missing = |mark: &Placed| {
                let mut left = missing.clone();
                mark.text.chars().all(|c| left.iter().position(|m| *m == c).map(|at| left.swap_remove(at)).is_some())
            };
            let (index, mark) = word
                .marks
                .iter()
                .enumerate()
                .filter(|(index, mark)| !used.get(*index).copied().unwrap_or(true) && draws_missing(mark))
                .max_by(|a, b| a.1.at[0].total_cmp(&b.1.at[0]))?;
            if let Some(slot) = used.get_mut(index) {
                *slot = true;
            }
            for c in mark.text.chars() {
                if let Some(at) = missing.iter().position(|m| *m == c) {
                    missing.swap_remove(at);
                }
            }
            seen.push((mark.bytes.clone(), from_letter(mark.at)));
            seen.extend(mark.with.iter().map(|(bytes, at)| (bytes.clone(), from_letter(*at))));
        }
        out.push((typed, seen));
    }
    out.reverse();
    Some((on.clone(), out))
}

/// The distance between a paragraph's evenly spaced source rows of one size, when it has two
/// or more (`None` for one row, mixed sizes or uneven rows).
fn source_pitch(block: &TextBlock, lines: &[TextLine]) -> Option<f64> {
    let firsts: Vec<&TextLine> = block.rows.iter().map(|row| row.first().and_then(|i| lines.get(*i))).collect::<Option<_>>()?;
    let (first, last) = (firsts.first()?, firsts.last()?);
    if firsts.len() < 2 || firsts.iter().any(|l| (l.size - first.size).abs() > 0.01) {
        return None;
    }
    let pitch = (first.origin.baseline - last.origin.baseline) / (firsts.len() - 1) as f64;
    let even = firsts.windows(2).all(|pair| ((pair[0].origin.baseline - pair[1].origin.baseline) - pitch).abs() <= first.size * 0.1);
    (even && pitch.is_finite() && pitch > 0.0 && pitch <= first.size * 4.0).then_some(pitch)
}

/// [`TextBlock::edited_spans`], except that a draft row replaced as a whole keeps its own source
/// row's look when the draft still has one row per source row: retyping a heading and its
/// subtitle must not set the subtitle in the heading's size. Inserted text inside a row still
/// inherits its neighbours, as everywhere else.
fn row_sources(block: &TextBlock, draft: &str, spans: Vec<(std::ops::Range<usize>, usize)>) -> Vec<(std::ops::Range<usize>, usize)> {
    if draft.split('\n').count() != block.rows.len() {
        return spans;
    }
    let mut start = 0usize;
    let mut out = Vec::with_capacity(spans.len());
    for (row, text) in block.rows.iter().zip(draft.split('\n')) {
        let range = start..start.saturating_add(text.len());
        start = range.end.saturating_add(1);
        let inside: Vec<_> = spans
            .iter()
            .filter_map(|(r, index)| {
                let clipped = r.start.max(range.start)..r.end.min(range.end);
                (clipped.start < clipped.end).then_some((clipped, *index))
            })
            .collect();
        match row.first() {
            Some(own) if !inside.iter().any(|(_, index)| row.contains(index)) => out.extend(inside.into_iter().map(|(r, _)| (r, *own))),
            _ => out.extend(inside),
        }
    }
    // The line feeds between rows draw nothing; keep them with the span before them.
    out.sort_by_key(|(r, _)| r.start);
    out
}

/// Baseline positions that keep every row where it was: when the draft still has one row per
/// source row and no width, alignment or move is asked for. Every row keeps its source baseline
/// and the edge the source rows share: the right one (the paragraph reads right to left, or only
/// the right edges line up), the left one, or their centre. A row that wraps (bold made it wider,
/// or words were added) goes on below its first line, an ink-safe line apart, and moves the rows
/// after it down by as much: the rows above it, a title or a heading, stay where they are. An
/// `unchanged` row that nothing above moved is kept as the source drew it, however it would lay
/// out here.
fn row_positions(
    block: &TextBlock,
    lines: &[TextLine],
    draft: &str,
    laid: &[appearance_text::Line],
    style: &BlockStyle,
    size: f64,
    unchanged: &[bool],
) -> Option<RowLayout> {
    let rows: Vec<&str> = draft.split('\n').collect();
    if style.width.is_some() || style.align.is_some() || style.offset.is_some() || rows.len() != block.rows.len() {
        return None;
    }
    // The draft row each laid line belongs to: a row's lines hold its text, in order.
    let mut owner = Vec::with_capacity(laid.len());
    let mut next = laid.iter();
    for (index, row) in rows.iter().enumerate() {
        let row = row.strip_suffix('\r').unwrap_or(row);
        let mut text = String::new();
        loop {
            let line = next.next()?;
            text.push_str(&line.text);
            owner.push(index);
            if text.len() >= row.len() {
                break;
            }
        }
        if text != row {
            return None;
        }
    }
    if next.next().is_some() {
        return None;
    }
    let rects: Vec<[f64; 4]> = block
        .rows
        .iter()
        .map(|row| {
            row.iter().filter_map(|i| lines.get(*i)).map(|l| l.rect).reduce(|a, r| [a[0].min(r[0]), a[1].min(r[1]), a[2].max(r[2]), a[3].max(r[3])])
        })
        .collect::<Option<_>>()?;
    let baselines: Vec<f64> = block.rows.iter().map(|row| lines.get(*row.first()?).map(|l| l.origin.baseline)).collect::<Option<_>>()?;
    let near = |a: f64, b: f64| (a - b).abs() <= 2.0;
    let rtl = appearance_text::paragraph_rtl(draft);
    let (right, left) = (rects.iter().all(|r| near(r[2], block.rect[2])), rects.iter().all(|r| near(r[0], block.rect[0])));
    let centre = (block.rect[0] + block.rect[2]) / 2.0;
    let centred = rects.iter().all(|r| near((r[0] + r[2]) / 2.0, centre));
    let mut out = Vec::with_capacity(laid.len());
    let mut kept = vec![false; rows.len()];
    let mut shift = 0.0;
    let mut previous: Option<(usize, f64, &appearance_text::Line)> = None;
    for (line, &index) in laid.iter().zip(&owner) {
        let rect = rects.get(index)?;
        let y = match previous {
            // A kept row is not drawn here: how it would wrap moves nothing.
            Some((row, y, _)) if row == index && kept.get(index).copied().unwrap_or(false) => y,
            // A wrapped row's next line, one ink-safe line below the last.
            Some((row, y, above)) if row == index => {
                let step = size * (above.descent + line.ascent);
                shift += step;
                y - step
            }
            _ => {
                if let Some(row) = kept.get_mut(index) {
                    *row = unchanged.get(index).copied().unwrap_or(false) && shift <= 1e-9;
                }
                baselines.get(index)? - shift
            }
        };
        let x = if right && (rtl || !left) {
            block.rect[2] - line.width
        } else if left {
            block.rect[0]
        } else if centred {
            centre - line.width / 2.0
        } else if rtl {
            rect[2] - line.width
        } else {
            rect[0]
        };
        if !(x.is_finite() && y.is_finite()) {
            return None;
        }
        out.push([x, y]);
        previous = Some((index, y, line));
    }
    Some(RowLayout { positions: out, owner, kept })
}

/// Where [`row_positions`] puts the laid lines: each line's position and the draft row it is in,
/// and the rows kept as the source drew them (not drawn again).
struct RowLayout {
    positions: Vec<[f64; 2]>,
    owner: Vec<usize>,
    kept: Vec<bool>,
}

/// The rows of the paragraph an edit leaves as they were: the same text in the draft as in the
/// source, and no size, colour or weight asked for. All `false` when no row changes.
fn unchanged_rows(block: &TextBlock, draft: &str, style: &BlockStyle) -> Vec<bool> {
    let source: Vec<&str> = block.editor_text.split('\n').collect();
    let rows: Vec<&str> = draft.split('\n').collect();
    let restyled = style.size.is_some() || style.color.is_some() || style.bold.is_some();
    if restyled || source.len() != rows.len() || source.len() != block.rows.len() {
        return vec![false; block.rows.len()];
    }
    let kept: Vec<bool> = (0..rows.len())
        .map(|k| {
            let same = |a: Option<&&str>, b: Option<&&str>| match (a, b) {
                (Some(a), Some(b)) => a.strip_suffix('\r').unwrap_or(a).trim_end() == b.strip_suffix('\r').unwrap_or(b).trim_end(),
                _ => false,
            };
            same(source.get(k), rows.get(k))
        })
        .collect();
    if kept.iter().all(|k| *k) { vec![false; kept.len()] } else { kept }
}

fn advance_op(advance: f64, state: &Ts) -> Result<Op, EditError> {
    let denominator = state.size * state.scale;
    let adjustment = if denominator != 0.0 { -1000.0 * advance / denominator } else { 0.0 };
    if !adjustment.is_finite() || (denominator == 0.0 && advance != 0.0) {
        return Err(EditError::Invalid("the source text advance cannot be restored".into()));
    }
    Ok(Op::new("TJ", vec![Object::Array(vec![pdfcraft_content::num(adjustment)])]))
}

pub(super) fn source_advance(source: &Shown, ops: &[Op]) -> Result<Vec<Op>, EditError> {
    let mut replacement = Vec::new();
    if let Some(op) = ops.get(source.op) {
        if op.is("\"") {
            replacement.push(Op::new("Tw", vec![op.operands.first().cloned().unwrap_or(Object::Int(0))]));
            replacement.push(Op::new("Tc", vec![op.operands.get(1).cloned().unwrap_or(Object::Int(0))]));
        }
        if op.is("'") || op.is("\"") {
            replacement.push(Op::new("T*", vec![]));
        }
    }
    replacement.push(advance_op(source.advance, &source.state)?);
    Ok(replacement)
}

pub(super) fn restore_text_matrix(source: &Shown) -> Result<Vec<Op>, EditError> {
    let tm = source.tm.0;
    let tlm = source.tlm.0;
    if tm.iter().chain(tlm.iter()).any(|v| !v.is_finite()) || tm[..4] != tlm[..4] {
        return Err(EditError::Invalid("the source text matrix cannot be restored".into()));
    }
    let mut restore = vec![Op::new("Tm", tlm.map(Object::Real).to_vec())];
    if tm[4..] != tlm[4..] {
        // Text-show operators translate only along the line's x axis. Restore TLM with Tm,
        // then restore that advance with TJ: setting Tm to TM alone would lose T* / Td's origin.
        let advance = if tlm[0].abs() >= tlm[1].abs() { (tm[4] - tlm[4]) / tlm[0] } else { (tm[5] - tlm[5]) / tlm[1] };
        restore.push(advance_op(advance, &source.state)?);
    }
    Ok(restore)
}
