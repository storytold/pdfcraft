use super::autonum::autonum;
use super::run::{RunEffects, TextShadow};
use super::style::{Cell, Opts, ParaShaped, Style, face_chain, make_style};
use super::symbol::{covers_symbol, is_symbol_font, map_symbol_bullet, symbol_font_char, symbol_map_char};
use super::{
    Arc, Bullet, Caps, Ctx, Fill, FontDb, FontFace, FontSlot, HashMap, ParaProps, RunKind, RunProps, ScriptTag, Shape, ShapeParams, TextBody,
    UnicodeSegmentation, bidi, resolve, script,
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn shape_para(
    ctx: &Ctx,
    shape: &Shape,
    body: &TextBody,
    pi: usize,
    scale: f64,
    opts: &Opts,
    number: Option<u32>,
    vertical: Upright,
) -> ParaShaped {
    let db = FontDb::global();
    let Some(para) = body.paragraphs.get(pi) else {
        let face = db.face("Inter", "Regular");
        return ParaShaped {
            cells: vec![],
            styles: vec![],
            faces: vec![],
            props: ParaProps::default(),
            lead_size: 18.0,
            lead_face: face.clone(),
            end_size: 18.0,
            end_face: face,
            bullet: None,
            level: 0,
            rtl: false,
        };
    };
    let props = resolve::para(ctx, shape, para);
    let rtl = props.rtl.unwrap_or(false);
    let mut styles: Vec<Style> = vec![];
    let mut faces: Vec<Arc<FontFace>> = vec![];
    // The logical paragraph: its text and each character's style.
    let mut text = String::new();
    let mut char_style: Vec<usize> = vec![];
    for run in &para.runs {
        let rp = resolve::run(ctx, shape, para, &run.props);
        let st = make_style(ctx, &rp, scale, opts.prompt_color);
        let t: String = match &run.kind {
            RunKind::Text => run.text.clone(),
            RunKind::Break => "\u{b}".into(),
            RunKind::Field { field } => opts.fields.field_in(field, rp.lang.as_deref()).unwrap_or_else(|| run.text.clone()),
            RunKind::Math { .. } => run.text.clone(),
        };
        let si = styles.len();
        styles.push(st);
        char_style.extend(std::iter::repeat_n(si, t.chars().count()));
        text.push_str(&t);
    }
    let cells = shape_cells(ctx, &text, &char_style, &styles, &mut faces, rtl, vertical);
    let lead = para.runs.first().map(|r| resolve::run(ctx, shape, para, &r.props)).unwrap_or_else(|| resolve::run(ctx, shape, para, &para.end_props));
    let lead_style = make_style(ctx, &lead, scale, opts.prompt_color);
    let lead_size = lead_style.size;
    let lead_face = lead_style.face.clone();
    // The paragraph mark, which ends the last line when no break does.
    let end_style = make_style(ctx, &resolve::run(ctx, shape, para, &para.end_props), scale, opts.prompt_color);
    let (end_size, end_face) = (end_style.size, end_style.face.clone());
    // Bullet.
    let bullet = match &props.bullet {
        Some(Bullet::Char { char }) => {
            let mut rp = lead.clone();
            if let Some(f) = &props.bullet_font {
                rp.font = Some(f.clone());
            }
            rp.bold = Some(false);
            rp.italic = Some(false);
            if let Some(c) = &props.bullet_color {
                rp.fill = Some(Fill::solid(c.clone()));
            }
            rp.size = Some(props.bullet_size_points.unwrap_or(lead.size.unwrap_or(18.0) * props.bullet_size.unwrap_or(1.0)));
            rp.underline = None;
            rp.strike = None;
            rp.highlight = None;
            let mut st = make_style(ctx, &rp, scale, opts.prompt_color);
            st.baseline = 0.0;
            // Symbol fonts (Wingdings) aren't available: map common private-use bullets.
            let ch = map_symbol_bullet(char, props.bullet_font.as_deref());
            bullet_face(db, &mut st, &ch);

            // A symbol font (Wingdings, Symbol…) that is installed draws its own glyph, found
            // at U+F000 + the character code as such fonts map them; otherwise common symbol
            // bullets are mapped to Unicode look-alikes.
            let ch = match symbol_font_char(&st.face, char, props.bullet_font.as_deref()) {
                Some(c) => c.to_string(),
                None => map_symbol_bullet(char, props.bullet_font.as_deref()),
            };
            bullet_face(db, &mut st, &ch);
            // A face the cascade still picks may lack the bullet: draw the dot rather than .notdef.
            let chars: Vec<char> = ch.chars().collect();
            let ch = if deckcraft_fonts::covers_cluster(&st.face, &chars) { ch } else { "•".into() };
            bullet_face(db, &mut st, &ch);
            Some((ch, st))
        }
        Some(Bullet::AutoNum { scheme, start_at }) => {
            let mut rp = lead.clone();
            if let Some(c) = &props.bullet_color {
                rp.fill = Some(Fill::solid(c.clone()));
            }
            rp.size = Some(props.bullet_size_points.unwrap_or(lead.size.unwrap_or(18.0) * props.bullet_size.unwrap_or(1.0)));
            rp.underline = None;
            let mut st = make_style(ctx, &rp, scale, opts.prompt_color);
            let label = autonum(scheme, start_at.saturating_sub(1).saturating_add(number.unwrap_or(1)));
            bullet_face(db, &mut st, &label);
            Some((label, st))
        }
        _ => None,
    };
    ParaShaped { cells, styles, faces, props, lead_size, lead_face, end_size, end_face, bullet, level: para.level, rtl }
}

/// A face that draws the bullet `label` when the bullet's own can't (circled numbers ①, arrows
/// and other symbols missing from the text font), by the usual fallback.
fn bullet_face(db: &FontDb, st: &mut Style, label: &str) {
    let chars: Vec<char> = label.chars().collect();
    if !chars.is_empty() && !deckcraft_fonts::covers_cluster(&st.face, &chars) {
        st.face = db.cascade(&chars, std::slice::from_ref(&st.face), st.style_name);
    }
}

/// Shape a logical paragraph into cells: bidi levels, per-grapheme faces, items of one level,
/// script, face and shaping style, each shaped with the whole paragraph as context.
/// A run's outer shadow and glow (inner shadows aren't drawn on text), at the layout's `scale`.
pub(crate) fn run_effects(ctx: &Ctx, r: &RunProps, scale: f64) -> Option<RunEffects> {
    let shadow = r.shadow.as_ref().filter(|s| !s.inner).map(|s| {
        let a = s.dir.to_radians();
        TextShadow { color: ctx.color(&s.color, None), blur: s.blur.max(0.0) * scale, dx: a.cos() * s.dist * scale, dy: a.sin() * s.dist * scale }
    });
    let glow = r.glow.as_ref().filter(|g| g.radius > 0.0).map(|g| (ctx.color(&g.color, None), g.radius * scale));
    (shadow.is_some() || glow.is_some()).then_some(RunEffects { shadow, glow })
}

#[allow(clippy::too_many_arguments)]
fn shape_cells(
    ctx: &Ctx,
    text: &str,
    char_style: &[usize],
    styles: &[Style],
    faces: &mut Vec<Arc<FontFace>>,
    rtl: bool,
    vertical: Upright,
) -> Vec<Cell> {
    let db = FontDb::global();
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    // Byte offset of each character (and of the end).
    let mut byte_of: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
    byte_of.push(text.len());
    let char_at_byte: HashMap<usize, usize> = byte_of.iter().enumerate().map(|(i, b)| (*b, i)).collect();
    let style_rtl: Vec<bool> = styles.iter().map(|st| st.lang.as_deref().is_some_and(bidi::is_rtl_lang)).collect();
    let rtl_lang: Vec<bool> = char_style.iter().map(|&si| style_rtl.get(si).copied().unwrap_or(false)).collect();
    let levels = bidi::levels_lang(text, &chars, rtl, &rtl_lang);
    let scripts = script::resolve(&chars);
    let mut boundary = vec![false; n];
    let mut graphemes: Vec<(usize, usize)> = Vec::new();
    for (b, g) in text.grapheme_indices(true) {
        let Some(&i) = char_at_byte.get(&b) else { continue };
        if let Some(x) = boundary.get_mut(i) {
            *x = true;
        }
        graphemes.push((i, i + g.chars().count()));
    }
    let face_index = |faces: &mut Vec<Arc<FontFace>>, f: &Arc<FontFace>| -> usize {
        if let Some(i) = faces.iter().position(|x| Arc::ptr_eq(x, f)) {
            return i;
        }
        faces.push(f.clone());
        faces.len() - 1
    };
    // A face per grapheme cluster (never split between faces).
    let mut char_face = vec![0usize; n];
    let mut chains: HashMap<(usize, FontSlot, Option<ScriptTag>), Vec<Arc<FontFace>>> = HashMap::new();
    let mut prev: Option<Arc<FontFace>> = None;
    for &(a, b) in &graphemes {
        let Some(cluster) = chars.get(a..b) else { continue };
        let si = char_style.get(a).copied().unwrap_or(0);
        let Some(st) = styles.get(si) else { continue };
        let (sc, slot) = scripts.get(a).copied().unwrap_or((None, FontSlot::Latin));
        // Spaces, digits and punctuation stay in the face of the text before them when it can
        // draw them, so a phrase isn't cut into items at every space; not in a symbol font
        // (`a:sym`), which PowerPoint uses for the symbol codes alone (Wingdings' space is about
        // four times a text font's).
        // Emoji, and pictographs asked to look like text (U+FE0E), pick their own face.
        let neutral =
            cluster.iter().all(|c| script::script_of(*c).is_none()) && !deckcraft_fonts::is_emoji_cluster(cluster) && !cluster.contains(&'\u{FE0E}');
        // A symbol-font character (U+F0xx) with an `a:sym` font draws in that font when it's there.
        // Some installed Symbol faces (Apple's on macOS) have no private-use mapping: they map the
        // glyphs to real Unicode, so the PUA code is remapped to that character when shaping.
        let sym_face = st
            .sym
            .as_deref()
            .filter(|_| cluster.first().is_some_and(|c| ('\u{F000}'..='\u{F0FF}').contains(c)))
            .filter(|f| db.has_family(f))
            .map(|f| db.face(f, st.style_name))
            .filter(|f| covers_symbol(f, cluster.first().copied().unwrap_or(' ')));
        let face = match (sym_face, prev.as_ref()) {
            (Some(f), _) => f,
            (None, Some(p))
                if neutral
                    && !p.is_color()
                    && !is_symbol_font(&p.family)
                    && deckcraft_fonts::covers_cluster(p, cluster)
                    && char_style.get(a.wrapping_sub(1)) == Some(&si) =>
            {
                p.clone()
            }
            (None, _) => {
                let chain = chains.entry((si, slot, sc)).or_insert_with(|| face_chain(ctx, st, slot, sc));
                db.cascade(cluster, chain, st.style_name)
            }
        };
        let fi = face_index(faces, &face);
        for f in char_face.get_mut(a..b).into_iter().flatten() {
            *f = fi;
        }
        prev = Some(face);
    }
    let mut cells: Vec<Cell> = (0..n)
        .map(|i| Cell {
            ch: chars.get(i).copied().unwrap_or(' '),
            style: char_style.get(i).copied().unwrap_or(0),
            face: char_face.get(i).copied().unwrap_or(0),
            level: levels.get(i).copied().unwrap_or(0),
            glyphs: vec![],
            adv: 0.0,
            head: i,
            boundary: boundary.get(i).copied().unwrap_or(true),
            upright: false,
        })
        .collect();
    // In East Asian vertical text, ideographs, kana, Hangul, full-width forms and the like stand
    // upright (a whole grapheme at a time); other text lies on its side.
    if vertical != Upright::None {
        for &(a, b) in &graphemes {
            let up = chars.get(a).is_some_and(|c| if vertical == Upright::All { !c.is_whitespace() } else { is_upright(*c) });
            for c in cells.get_mut(a..b).into_iter().flatten() {
                c.upright = up;
            }
        }
    }
    // Items: maximal ranges of one level, face, script and shaping style (case mapping, language).
    let upper = |i: usize| styles.get(char_style.get(i).copied().unwrap_or(0)).is_some_and(|st| st.caps != Caps::None);
    let lang = |i: usize| styles.get(char_style.get(i).copied().unwrap_or(0)).and_then(|st| st.lang.as_deref());
    let kern = |i: usize| styles.get(char_style.get(i).copied().unwrap_or(0)).is_some_and(|st| st.kern);
    let up = |i: usize| cells.get(i).is_some_and(|c| c.upright);
    let key = |i: usize| (levels.get(i).copied(), char_face.get(i).copied(), scripts.get(i).map(|s| s.0), upper(i), lang(i), kern(i), up(i));
    let mut items: Vec<(usize, usize)> = Vec::new();
    let mut a = 0;
    while a < n {
        let mut b = a + 1;
        while b < n && key(b) == key(a) {
            b += 1;
        }
        items.push((a, b));
        a = b;
    }
    // Upright glyphs keep their horizontal forms: PowerPoint turns "、", "。" and "「" as they are
    // (no `vert` alternates).
    for (a, b) in items {
        shape_item(&mut cells, styles, faces, text, &byte_of, &char_at_byte, a..b, scripts.get(a).and_then(|s| s.0), lang(a), upper(a));
    }
    // Stacked text: each character takes one em down the line, centred across it (PowerPoint
    // stacks 18 pt Latin letters about 18 pt apart, a space only its own width).
    if vertical == Upright::All {
        for i in 0..n {
            let Some(c) = cells.get(i) else { continue };
            if !c.upright || c.head != i || c.glyphs.is_empty() {
                continue;
            }
            let size = styles.get(c.style).map(|st| st.size).unwrap_or(18.0);
            let width: f64 = (i..n).take_while(|&j| cells.get(j).is_some_and(|d| d.head == i)).filter_map(|j| cells.get(j)).map(|d| d.adv).sum();
            let pad = (size - width) / 2.0;
            if let Some(c) = cells.get_mut(i) {
                for g in &mut c.glyphs {
                    g.1 += pad;
                }
            }
            for j in i..n {
                match cells.get_mut(j) {
                    Some(d) if d.head == i => d.adv = if j == i { size } else { 0.0 },
                    _ => break,
                }
            }
        }
    }
    cells
}

/// A character in capitals (`cap="all"`/`"small"`): its uppercase when that is one character.
/// PowerPoint leaves characters whose capital is a sequence as they are: "ß" stays "ß" (not "SS"),
/// "ﬁ" and "ŉ" stay.
pub(crate) fn caps_char(ch: char) -> char {
    let mut up = ch.to_uppercase();
    match (up.next(), up.next()) {
        (Some(u), None) => u,
        _ => ch,
    }
}

/// Does `c` stand upright in East Asian vertical text? Ideographs, kana, Hangul, Bopomofo, CJK
/// symbols and punctuation, full-width forms, enclosed and compatibility CJK, circled numbers
/// and emoji (Unicode's Vertical_Orientation U and T, roughly); Latin, digits and the rest of the
/// half-width text lie on their side as in `vert`.
pub(crate) fn is_upright(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF
            | 0x2460..=0x24FF
            | 0x2E80..=0x2FFF
            | 0x3000..=0x303F
            | 0x3040..=0x30FF
            | 0x3100..=0x312F
            | 0x3130..=0x318F
            | 0x3190..=0x31FF
            | 0x3200..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA960..=0xA97F
            | 0xAC00..=0xD7FF
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE1F
            | 0xFE30..=0xFE6F
            | 0xFF01..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F000..=0x1FAFF
            | 0x20000..=0x3FFFF
    )
}

/// Which characters stand upright in vertical text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Upright {
    /// None (horizontal text, vert, vert270, mongolianVert).
    None,
    /// Ideographs, kana and the like (eaVert).
    Cjk,
    /// Every character, one under the other (wordArtVert).
    All,
}

/// Size of superscript and subscript text relative to the run's size.
pub(crate) const SUB_SUPER: f64 = 0.66;

/// The OpenType features to change from the shaper's defaults for an item, as PowerPoint draws
/// it: kerning only where the run asks for it (`kern`), and no standard or contextual ligatures in
/// Latin, Greek and Cyrillic ("fi", "ffi" and Calibri's "ti" stay apart). Other scripts need
/// their ligatures (Arabic lam-alef), and emoji sequences are joined by them.
pub(crate) fn shaping_features(cells: &[Cell], styles: &[Style], script: Option<ScriptTag>) -> Vec<deckcraft_fonts::Feature> {
    let mut f = vec![];
    if !cells.first().and_then(|c| styles.get(c.style)).is_some_and(|st| st.kern) {
        f.extend(deckcraft_fonts::feature("-kern"));
    }
    let alphabetic = matches!(script.as_ref(), Some(b"Latn" | b"Grek" | b"Cyrl"));
    let emoji = cells.iter().any(|c| matches!(c.ch as u32, 0x200D | 0x20E3 | 0xFE0F | 0x2600..=0x27BF | 0x1F000..=0x1FAFF | 0xE0020..=0xE007F));
    if alphabetic && !emoji {
        f.extend(deckcraft_fonts::feature("-liga"));
        f.extend(deckcraft_fonts::feature("-clig"));
    }
    f
}

/// Shape cells `range` (one item) and hang each cluster's glyphs on its first cell, sharing the
/// cluster's advance among its graphemes (by the ligature's GDEF carets when it has them).
#[allow(clippy::too_many_arguments)]
fn shape_item(
    cells: &mut [Cell],
    styles: &[Style],
    faces: &[Arc<FontFace>],
    text: &str,
    byte_of: &[usize],
    char_at_byte: &HashMap<usize, usize>,
    range: std::ops::Range<usize>,
    script: Option<ScriptTag>,
    lang: Option<&str>,
    upper: bool,
) {
    let Some(first) = cells.get(range.start) else { return };
    let (fi, level) = (first.face, first.level);
    let Some(face) = faces.get(fi) else { return };
    let rtl = level % 2 == 1;
    let (Some(&b0), Some(&b1)) = (byte_of.get(range.start), byte_of.get(range.end)) else { return };
    let features = shaping_features(cells.get(range.clone()).unwrap_or(&[]), styles, script);
    let params = ShapeParams { features: &features, rtl, script, language: lang };
    let sym = styles.get(first.style).and_then(|st| st.sym.as_deref());
    let glyphs = deckcraft_fonts::shape_range(face, text, b0..b1, &params, |ch| {
        let ch = if upper { caps_char(ch) } else { ch };
        symbol_map_char(face, ch, sym).unwrap_or(ch)
    });
    let upem = face.upem.max(1.0);
    let scale_of = |cell: &Cell| {
        let st = styles.get(cell.style);
        let size = st.map(|s| s.size).unwrap_or(18.0);
        let small = st.is_some_and(|s| s.caps == Caps::Small) && cell.ch.is_lowercase();
        let size = if small { size * 0.8 } else { size };
        // Superscript and subscript glyphs are drawn at two thirds of the size, so they advance by
        // two thirds too.
        let size = if st.is_some_and(|s| s.baseline != 0.0) { size * SUB_SUPER } else { size };
        size / upem
    };
    // Cluster heads in this item, ascending; each cluster spans to the next head.
    let mut heads: Vec<usize> = glyphs.iter().filter_map(|g| char_at_byte.get(&g.cluster).copied()).filter(|h| range.contains(h)).collect();
    heads.sort_unstable();
    heads.dedup();
    let mut width: HashMap<usize, f64> = HashMap::new();
    let mut lig: HashMap<usize, (u32, usize)> = HashMap::new();
    for g in &glyphs {
        let Some(&h) = char_at_byte.get(&g.cluster) else { continue };
        let Some(cell) = cells.get_mut(h) else { continue };
        if matches!(cell.ch, '\t'..='\u{b}') {
            continue;
        }
        let s = scale_of(cell);
        let w = width.entry(h).or_insert(0.0);
        cell.glyphs.push((g.gid, *w + g.x_offset as f64 * s, -(g.y_offset as f64) * s));
        *w += g.x_advance as f64 * s;
        let e = lig.entry(h).or_insert((g.gid, 0));
        e.1 += 1;
    }
    for (hi, &h) in heads.iter().enumerate() {
        let end = heads.get(hi + 1).copied().unwrap_or(range.end).min(range.end);
        let w = width.get(&h).copied().unwrap_or(0.0);
        for c in cells.get_mut(h..end).into_iter().flatten() {
            c.head = h;
        }
        let stops: Vec<usize> = (h..end).filter(|&i| i == h || cells.get(i).is_some_and(|c| c.boundary)).collect();
        let m = stops.len().max(1);
        // Visual widths of the cluster's graphemes, left to right.
        let mut bounds = vec![0.0];
        if m > 1
            && let Some(&(gid, 1)) = lig.get(&h)
        {
            let s = cells.get(h).map(scale_of).unwrap_or(0.0);
            let carets = deckcraft_fonts::ligature_carets(face, gid);
            if carets.len() + 1 >= m {
                bounds.extend(carets.iter().take(m - 1).map(|c| (*c as f64 * s).clamp(0.0, w)));
            }
        }
        if bounds.len() != m {
            bounds = (0..m).map(|j| w * j as f64 / m as f64).collect();
        }
        bounds.push(w);
        for (j, &i) in stops.iter().enumerate() {
            let vj = if rtl { m - 1 - j } else { j };
            let share = bounds.get(vj + 1).copied().unwrap_or(w) - bounds.get(vj).copied().unwrap_or(0.0);
            if let Some(c) = cells.get_mut(i) {
                c.adv = share.max(0.0);
            }
        }
    }
    // Tracking (character spacing) after every drawn grapheme and space.
    for i in range {
        let Some(c) = cells.get(i) else { continue };
        let drawn = cells.get(c.head).is_some_and(|hd| !hd.glyphs.is_empty());
        if c.boundary
            && (drawn || c.ch == ' ')
            && let Some(sp) = styles.get(c.style).map(|s| s.spacing)
            && let Some(c) = cells.get_mut(i)
        {
            c.adv += sp;
        }
    }
}
