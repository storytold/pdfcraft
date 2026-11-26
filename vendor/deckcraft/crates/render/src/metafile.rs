//! Windows Metafile and Enhanced Metafile pictures (`image/x-wmf`, `image/x-emf`): parsed by
//! `craft-metafile` and drawn as vectors into a bitmap of the size the slide needs, so they go
//! through the same crop, tile and recolour path as any other picture.
//!
//! Text items name the font the file asks for; they are drawn with the face slide text would use
//! for that family (`FontDb::document_face`, so a missing font falls back the same way), symbol
//! fonts mapped as bullets map them, and the file's own advances (the dx array) placing each
//! character.

use std::collections::HashMap;
use std::sync::Arc;

use craft_metafile::{Cap, HAlign, Item, Join, LineStyle, PlacedClip, PlacedItem, Text, Tile, VAlign};
use deckcraft_fonts::{FontDb, FontFace};
use kurbo::{Affine, BezPath, Rect, Shape as _};
use skrifa::MetadataProvider as _;
use vello_cpu::{Pixmap, RenderContext, Resources, peniko};

/// Longest side of a metafile drawn without a target size (export, thumbnails of the picture).
pub const DEFAULT_SIDE: u32 = 1024;
/// Largest side drawn.
const MAX_SIDE: u32 = 8192;
/// Pattern cells smaller than this (output pixels) are drawn as the tile's average colour: a
/// sub-pixel pattern would only alias.
const MIN_CELL_PX: f64 = 1.5;
/// Size of text whose font gives no height (LOGFONT height 0), in text units.
const DEFAULT_EM: f64 = 12.0;

/// Is `bytes` a WMF or EMF file?
pub fn is_metafile(bytes: &[u8]) -> bool {
    craft_metafile::is_metafile(bytes)
}

/// Draw the metafile `bytes` into straight RGBA with its longer side `side` px (transparent where
/// it draws nothing). `None` when it doesn't parse or has no size.
pub fn rasterize(bytes: &[u8], side: u32) -> Option<image::RgbaImage> {
    let mut pic = craft_metafile::parse(bytes).ok()?;
    if !pic.has_size() {
        return None;
    }
    let side = side.clamp(16, MAX_SIDE) as f64;
    let (pw, ph) = (pic.width as f64, pic.height as f64);
    let k = side / pw.max(ph);
    let w = (pw * k).round().clamp(1.0, MAX_SIDE as f64) as u16;
    let h = (ph * k).round().clamp(1.0, MAX_SIDE as f64) as u16;
    let pixmaps: Vec<Option<Arc<Pixmap>>> = pic.items.iter_mut().map(|it| it.take_pixels().and_then(|(w, h, px)| premul(w, h, &px))).collect();
    // Pattern tiles shared by several fills are converted once.
    let mut tiles: HashMap<*const Tile, (Option<Arc<Pixmap>>, peniko::Color)> = HashMap::new();
    let mut ctx = RenderContext::new_with(w, h, vello_cpu::RenderSettings { num_threads: 0, ..Default::default() });
    let r = Rect::new(0.0, 0.0, w as f64, h as f64);
    ctx.push_clip_layer(&r.to_path(0.1));
    for it in pic.place(r, [0.0; 4]) {
        match it {
            PlacedItem::Path { path, fill, stroke, line, even_odd, region, clips } => clipped(&mut ctx, region.as_deref(), &clips, |ctx| {
                if let Some(f) = fill {
                    ctx.set_paint(rgba(f));
                    ctx.set_fill_rule(if even_odd { peniko::Fill::EvenOdd } else { peniko::Fill::NonZero });
                    ctx.fill_path(&path);
                }
                // The pen's caps, joins and dashes (GDI's default: round, solid); a cosmetic
                // (zero-width) pen is one pixel.
                if let Some((s, sw)) = stroke {
                    ctx.set_paint(rgba(s));
                    ctx.set_stroke(pen(sw.max(1.0), &line));
                    ctx.stroke_path(&path);
                }
            }),
            PlacedItem::Bitmap { index, rect, transform, region, clips } => clipped(&mut ctx, region.as_deref(), &clips, |ctx| {
                let Some(Some(pm)) = pixmaps.get(index) else { return };
                let (bw, bh) = (pm.width().max(1) as f64, pm.height().max(1) as f64);
                ctx.set_paint(vello_cpu::Image {
                    image: vello_cpu::ImageSource::Pixmap(pm.clone()),
                    sampler: peniko::ImageSampler::default().with_quality(peniko::ImageQuality::Medium),
                });
                let c = transform.as_coeffs();
                if c[1] == 0.0 && c[2] == 0.0 {
                    ctx.set_paint_transform(Affine::translate((rect.x0, rect.y0)) * Affine::scale_non_uniform(rect.width() / bw, rect.height() / bh));
                    ctx.fill_rect(&rect);
                } else {
                    // A turned or skewed bitmap: `transform` maps the unit square onto its place.
                    ctx.set_paint_transform(transform * Affine::scale_non_uniform(1.0 / bw, 1.0 / bh));
                    ctx.fill_path(&(transform * Rect::new(0.0, 0.0, 1.0, 1.0).to_path(0.1)));
                }
                ctx.reset_paint_transform();
            }),
            PlacedItem::Pattern { index, path, even_odd, tile, region, clips } => clipped(&mut ctx, region.as_deref(), &clips, |ctx| {
                let Some(Item::Pattern { tile: t, .. }) = pic.items.get(index) else { return };
                let key = Arc::as_ptr(t);
                let (pm, avg) = tiles.entry(key).or_insert_with(|| (premul(t.width, t.height, &t.rgba), average(&t.rgba))).clone();
                ctx.set_fill_rule(if even_odd { peniko::Fill::EvenOdd } else { peniko::Fill::NonZero });
                let cell = tile.determinant().abs().sqrt();
                match pm {
                    Some(pm) if cell >= MIN_CELL_PX => {
                        ctx.set_paint(vello_cpu::Image {
                            image: vello_cpu::ImageSource::Pixmap(pm),
                            sampler: peniko::ImageSampler {
                                x_extend: peniko::Extend::Repeat,
                                y_extend: peniko::Extend::Repeat,
                                quality: peniko::ImageQuality::Low,
                                alpha: 1.0,
                            },
                        });
                        ctx.set_paint_transform(tile);
                        ctx.fill_path(&path);
                        ctx.reset_paint_transform();
                    }
                    _ => {
                        ctx.set_paint(avg);
                        ctx.fill_path(&path);
                    }
                }
            }),
            PlacedItem::Text { index, transform, clip, region, clips } => clipped(&mut ctx, region.as_deref(), &clips, |ctx| {
                let Some(Item::Text(t)) = pic.items.get(index) else { return };
                if let Some(c) = clip {
                    ctx.push_clip_layer(&c.to_path(0.1));
                }
                draw_text(ctx, t, transform);
                if clip.is_some() {
                    ctx.pop_layer();
                }
            }),
        }
    }
    ctx.set_fill_rule(peniko::Fill::NonZero);
    ctx.pop_layer();
    ctx.flush();
    let mut px = vec![0u8; w as usize * h as usize * 4];
    if let Some(pm) = vello_cpu::PixmapMut::new(w, h, &mut px) {
        ctx.render(pm, &mut Resources::new());
    }
    // Premultiplied to straight.
    for p in px.as_chunks_mut::<4>().0 {
        let a = p[3] as u32;
        if a != 0 && a != 255 {
            for c in p.iter_mut().take(3) {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    image::RgbaImage::from_raw(w as u32, h as u32, px)
}

/// Draws `draw` inside the device-context clip and every clip path. No region is the default clip.
fn clipped(ctx: &mut RenderContext, region: Option<&BezPath>, clips: &[Arc<PlacedClip>], draw: impl FnOnce(&mut RenderContext)) {
    if let Some(region) = region {
        ctx.push_clip_layer(region);
    }
    for c in clips {
        ctx.set_fill_rule(if c.even_odd { peniko::Fill::EvenOdd } else { peniko::Fill::NonZero });
        ctx.push_clip_layer(&c.path);
    }
    if !clips.is_empty() {
        ctx.set_fill_rule(peniko::Fill::NonZero);
    }
    draw(ctx);
    for _ in 0..clips.len() + usize::from(region.is_some()) {
        ctx.pop_layer();
    }
}

/// A GDI pen as a stroke `width` wide (target units): its caps, joins, miter limit and dashes.
fn pen(width: f64, line: &LineStyle) -> kurbo::Stroke {
    let cap = match line.cap {
        Cap::Round => kurbo::Cap::Round,
        Cap::Square => kurbo::Cap::Square,
        Cap::Flat => kurbo::Cap::Butt,
    };
    let join = match line.join {
        Join::Round => kurbo::Join::Round,
        Join::Bevel => kurbo::Join::Bevel,
        Join::Miter => kurbo::Join::Miter,
    };
    let stroke = kurbo::Stroke::new(width).with_caps(cap).with_join(join).with_miter_limit(f64::from(line.miter_limit));
    // A dash pattern with no length would never advance: such a pen draws solid.
    let dash: Vec<f64> = line.dash.iter().map(|d| f64::from(*d)).collect();
    if dash.iter().all(|d| d.is_finite() && *d >= 0.0) && dash.iter().sum::<f64>() > 0.0 { stroke.with_dashes(0.0, dash) } else { stroke }
}

/// One glyph placed in text space (x along the baseline from the string's start, font units at
/// `scale` text units each).
struct Glyph {
    face: Arc<FontFace>,
    gid: u32,
    x: f64,
    y: f64,
}

/// Draws a text item; `m` maps its text space to the output.
fn draw_text(ctx: &mut RenderContext, t: &Text, m: Affine) {
    let db = FontDb::global();
    let font = &t.font;
    let family = if font.family.trim().is_empty() { "Arial" } else { font.family.trim() };
    let (bold, italic) = (font.weight >= 600, font.italic);
    let style = match (bold, italic) {
        (true, true) => "Bold Italic",
        (true, false) => "Bold",
        (false, true) => "Italic",
        (false, false) => "Regular",
    };
    let face = db.document_face(family, style);
    let upem = face.upem.max(1.0);
    let (asc, desc) = win_metrics(&face);
    // LOGFONT height: negative is the em, positive the cell (ascent plus descent).
    let h = f64::from(font.height);
    let em = if h < 0.0 {
        -h
    } else if h > 0.0 {
        h * upem / (asc + desc).max(1.0)
    } else {
        DEFAULT_EM
    };
    let k = em / upem;
    if !k.is_finite() || k <= 0.0 {
        return;
    }
    // Characters as drawn: symbol fonts (Wingdings…) map like bullets; each character takes the
    // face that has it.
    let chars: Vec<char> = t.text.chars().map(|c| deckcraft_text::symbol_char(&face, family, c).unwrap_or(c)).collect();
    let faces: Vec<Arc<FontFace>> = chars
        .iter()
        .map(|c| {
            if c.is_whitespace() || c.is_control() || face.covers(*c) { face.clone() } else { db.cascade(&[*c], std::slice::from_ref(&face), style) }
        })
        .collect();
    // Shape runs of one face; record each glyph's pen position and its character.
    let mut shaped: Vec<(Glyph, usize)> = Vec::with_capacity(chars.len());
    let mut char_pen: Vec<f64> = vec![0.0; chars.len() + 1];
    let mut pen = 0.0;
    let mut i = 0;
    while i < chars.len() {
        let Some(f) = faces.get(i).cloned() else { break };
        let mut j = i + 1;
        while faces.get(j).is_some_and(|g| g.id() == f.id()) {
            j += 1;
        }
        let run: String = chars.get(i..j).unwrap_or(&[]).iter().collect();
        // Byte offset in the run → character index in the string.
        let byte_char: Vec<usize> = run.char_indices().map(|(b, _)| b).collect();
        let ku = f.upem.max(1.0);
        for g in deckcraft_fonts::shape(&f, &run, &[], |c| c) {
            let ci = i + byte_char.partition_point(|b| *b < g.cluster).min(j - i - 1);
            let s = em / ku;
            shaped.push((Glyph { face: f.clone(), gid: g.gid, x: pen + f64::from(g.x_offset) * s, y: -f64::from(g.y_offset) * s }, ci));
            pen += f64::from(g.x_advance) * s;
            if let Some(p) = char_pen.get_mut(ci + 1) {
                *p = pen;
            }
        }
        i = j;
    }
    // Character start pens: a character's start is the previous one's end.
    for ci in 1..char_pen.len() {
        let prev = char_pen.get(ci - 1).copied().unwrap_or(0.0);
        if let Some(p) = char_pen.get_mut(ci)
            && *p < prev
        {
            *p = prev;
        }
    }
    // The file's advances place each character; glyphs keep their offset within their character.
    let use_dx = !t.dx.is_empty() && t.dx.len() == chars.len();
    let mut width = pen;
    let mut x_scale = 1.0;
    if use_dx {
        let mut starts = Vec::with_capacity(chars.len() + 1);
        let mut x = 0.0;
        for d in &t.dx {
            starts.push(x);
            x += f64::from(*d);
        }
        width = x;
        for (g, ci) in &mut shaped {
            let (start, pen_start) = (starts.get(*ci).copied().unwrap_or(0.0), char_pen.get(*ci).copied().unwrap_or(0.0));
            g.x = start + (g.x - pen_start);
        }
    } else if font.width > 0.0 {
        // A LOGFONT width narrows or widens the font against its average character width.
        let avg = avg_char_width(&face) * k;
        if avg > 0.0 {
            x_scale = (f64::from(font.width) / avg).clamp(0.05, 20.0);
            width *= x_scale;
            for (g, _) in &mut shaped {
                g.x *= x_scale;
            }
        }
    }
    let x0 = match t.h_align {
        HAlign::Left => 0.0,
        HAlign::Center => -width / 2.0,
        HAlign::Right => -width,
    };
    let (a, d) = (asc * k, desc * k);
    let base = match t.v_align {
        VAlign::Baseline => 0.0,
        VAlign::Top => a,
        VAlign::Bottom => -d,
    };
    let at = m * Affine::translate((x0, base));
    ctx.set_fill_rule(peniko::Fill::NonZero);
    if let Some(bg) = t.background
        && width > 0.0
    {
        ctx.set_paint(rgba(bg));
        ctx.fill_path(&(at * Rect::new(0.0, -a, width, d).to_path(0.1)));
    }
    let color = rgba(t.color);
    ctx.set_paint(color);
    for (g, _) in &shaped {
        let path = db.outline(&g.face, g.gid);
        if path.elements().is_empty() {
            continue;
        }
        let s = em / g.face.upem.max(1.0);
        let skew = if italic && !g.face.italic { Affine::new([1.0, 0.0, -0.2, 1.0, 0.0, 0.0]) } else { Affine::IDENTITY };
        let gm = at * Affine::translate((g.x, g.y)) * Affine::scale_non_uniform(x_scale, 1.0) * skew * Affine::scale(s);
        let sc = scale_of(gm);
        if !sc.is_finite() || sc > 1e5 {
            continue;
        }
        let p: BezPath = gm * path.as_ref().clone();
        if p.elements().len() > 10_000 || p.bounding_box().width() > 1e6 || p.bounding_box().height() > 1e6 {
            continue;
        }
        ctx.fill_path(&p);
        if bold && g.face.weight < 600.0 {
            ctx.set_stroke(kurbo::Stroke::new(em * 0.025 * scale_of(at)).with_join(kurbo::Join::Round));
            ctx.stroke_path(&p);
        }
    }
    // Underline and strike-out: the font's own positions and thickness.
    let metrics = face.skrifa().map(|f| f.metrics(skrifa::instance::Size::unscaled(), face.location()));
    let deco = |on: bool, d: Option<skrifa::metrics::Decoration>, fallback: (f64, f64)| -> Option<Rect> {
        if !on || width <= 0.0 {
            return None;
        }
        let (off, th) = d.map_or(fallback, |d| (f64::from(d.offset) * k, f64::from(d.thickness) * k));
        let th = if th > 0.0 { th } else { em / 20.0 };
        // Offsets are y-up from the baseline; text space is y-down.
        Some(Rect::new(0.0, -off - th / 2.0, width, -off + th / 2.0))
    };
    let lines = [
        deco(font.underline, metrics.as_ref().and_then(|m| m.underline), (-em * 0.1, em / 20.0)),
        deco(font.strike, metrics.as_ref().and_then(|m| m.strikeout), (em * 0.26, em / 20.0)),
    ];
    for r in lines.into_iter().flatten() {
        ctx.fill_path(&(at * r.to_path(0.1)));
    }
}

/// Mean scale of an affine transform's linear part.
fn scale_of(m: Affine) -> f64 {
    m.determinant().abs().sqrt()
}

/// The face's Windows ascent and descent (OS/2 `usWinAscent`/`usWinDescent`, as GDI measures the
/// cell), in font units; the typographic ones when the font has no OS/2 table.
fn win_metrics(face: &FontFace) -> (f64, f64) {
    use skrifa::raw::TableProvider as _;
    let os2 = face.skrifa().and_then(|f| f.os2().ok()).map(|o| (f64::from(o.us_win_ascent()), f64::from(o.us_win_descent())));
    match os2 {
        Some((a, d)) if a + d > 0.0 => (a, d),
        _ => (face.ascent.max(0.0), face.descent.max(0.0)),
    }
}

/// OS/2 `xAvgCharWidth` in font units (half an em without it).
fn avg_char_width(face: &FontFace) -> f64 {
    use skrifa::raw::TableProvider as _;
    face.skrifa().and_then(|f| f.os2().ok()).map(|o| f64::from(o.x_avg_char_width())).filter(|w| *w > 0.0).unwrap_or(face.upem * 0.5)
}

fn rgba(c: [u8; 4]) -> peniko::Color {
    peniko::Color::from_rgba8(c[0], c[1], c[2], c[3])
}

/// The average colour of straight RGBA pixels (alpha-weighted).
fn average(px: &[u8]) -> peniko::Color {
    let (mut r, mut g, mut b, mut a, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
    for p in px.as_chunks::<4>().0 {
        let w = u64::from(p[3]);
        r += u64::from(p[0]) * w;
        g += u64::from(p[1]) * w;
        b += u64::from(p[2]) * w;
        a += w;
        n += 1;
    }
    if a == 0 || n == 0 {
        return peniko::Color::from_rgba8(0, 0, 0, 0);
    }
    peniko::Color::from_rgba8((r / a) as u8, (g / a) as u8, (b / a) as u8, (a / n) as u8)
}

/// Straight RGBA rows to a premultiplied pixmap.
fn premul(w: u32, h: u32, px: &[u8]) -> Option<Arc<Pixmap>> {
    if w == 0 || h == 0 || w > u16::MAX as u32 || h > u16::MAX as u32 || px.len() < w as usize * h as usize * 4 {
        return None;
    }
    let data = px
        .as_chunks::<4>()
        .0
        .iter()
        .take(w as usize * h as usize)
        .map(|p| {
            let a = p[3] as u16;
            let m = |c: u8| ((c as u16 * a + 127) / 255) as u8;
            vello_cpu::color::PremulRgba8 { r: m(p[0]), g: m(p[1]), b: m(p[2]), a: p[3] }
        })
        .collect();
    Some(Arc::new(Pixmap::from_parts(data, w as u16, h as u16)))
}
