//! Equations (Insert › Symbols › Equation): floating shapes holding LaTeX source, laid out by
//! `gridcraft-math` and drawn from the cached layout.
//!
//! An equation is drawn to fit its frame, so resizing the frame scales it. Inserting sizes the
//! frame for the requested font size; editing keeps the current scale and resizes the frame to
//! the new content.

use gridcraft_math::{MathError, MathList};
use gridcraft_model::{Anchor, AnchorMode, Color, EquationCache, Shape, ShapeKind};
use serde_json::{Value as Json, json};

use super::{CommandSpec, EngineError, PROTECTED, Result, Session, bad, cell_param, cmd, edit, f64_param, has_doc, ok, str_param, u32_param};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "insert.equation",
            "Equation",
            ["Insert", "Symbols"],
            None,
            "{latex: \"x^2\", cell?|at?: \"B2\", size?: 11 (points), color?: hex|\"auto\"}",
            has_doc,
            insert_equation
        ),
        cmd!("equation.edit", "Edit Equation", [], None, "{id, latex}", has_doc, edit_equation),
    ]
}

/// Default equation font size, in points.
pub const DEFAULT_SIZE_PT: f64 = 11.0;
/// Space between an equation's frame and its content, in sheet pixels.
pub const PADDING: f32 = 4.0;
/// Sheet pixels per point.
const PX_PER_PT: f64 = 96.0 / 72.0;

/// The scale (sheet pixels per em) at which `list` fits a frame of `w` by `h` pixels.
pub fn fit_scale(list: &MathList, w: f32, h: f32) -> f32 {
    let (cw, ch) = (list.width.max(0.01), list.total_height().max(0.01));
    let s = ((w - 2.0 * PADDING) / cw).min((h - 2.0 * PADDING) / ch);
    if s.is_finite() { s.max(0.0) } else { 0.0 }
}

/// The frame size for `list` at `scale` pixels per em.
fn frame_for(list: &MathList, scale: f32) -> (f32, f32) {
    let w = list.width.max(0.5) * scale + 2.0 * PADDING;
    let h = list.total_height().max(1.0) * scale + 2.0 * PADDING;
    (w.clamp(8.0, 20_000.0), h.clamp(8.0, 20_000.0))
}

/// `latex` laid out, or a readable error with the position of the problem.
fn lay_out(cmd: &str, latex: &str) -> Result<MathList> {
    gridcraft_math::layout(latex).map_err(|e| bad(cmd, describe(latex, &e)))
}

/// "Error at character 5: Undefined control sequence: \foo".
pub fn describe(latex: &str, e: &MathError) -> String {
    match e.char_pos(latex) {
        Some(p) => format!("equation error at character {p}: {}", e.message),
        None => format!("equation error: {}", e.message),
    }
}

fn insert_equation(s: &mut Session, p: &Json) -> Result<Json> {
    const CMD: &str = "insert.equation";
    let latex = str_param(p, "latex").ok_or_else(|| bad(CMD, "`latex` must be the equation's LaTeX source, such as \"x^2 + y^2 = r^2\""))?;
    if latex.trim().is_empty() {
        return Err(bad(CMD, "`latex` is empty"));
    }
    let size = f64_param(p, "size").unwrap_or(DEFAULT_SIZE_PT);
    if !(1.0..=409.0).contains(&size) {
        return Err(bad(CMD, "`size` must be between 1 and 409 points"));
    }
    let color = match p.get("color") {
        None | Some(Json::Null) => Color::Auto,
        Some(c) => super::format::color_param(Some(c)).ok_or_else(|| bad(CMD, "`color` must be a hex colour such as \"#1F4E79\", or \"auto\""))?,
    };
    let list = lay_out(CMD, latex)?;
    let at = match cell_param(s, p, "cell")? {
        Some(c) => Some(c),
        None => cell_param(s, p, "at")?,
    };
    let d = s.doc()?;
    if d.wb.active().is_some_and(|sh| sh.is_protected()) {
        return Err(EngineError::Other(PROTECTED.into()));
    }
    let at = at.unwrap_or(d.selection.active);
    let id = d.wb.next_object_id();
    let (width, height) = frame_for(&list, (size * PX_PER_PT) as f32);
    let mut shape = Shape {
        id,
        kind: ShapeKind::Equation,
        anchor: Anchor { cell: at, dx: 0.0, dy: 0.0, width, height, mode: AnchorMode::MoveOnly },
        fill: Color::Auto,
        line: color,
        text: latex.to_string(),
        format: Default::default(),
        math: Default::default(),
        rotation: 0.0,
    };
    shape.math.refresh(latex);
    let sheet = d.wb.active_sheet();
    edit(s, |cx| {
        cx.sheet_mut(sheet)?.shapes.push(shape);
        Ok(json!({"equation": id}))
    })
}

fn edit_equation(s: &mut Session, p: &Json) -> Result<Json> {
    const CMD: &str = "equation.edit";
    let id = u32_param(p, "id").ok_or_else(|| bad(CMD, "`id` must be an equation id"))?;
    let latex = str_param(p, "latex").ok_or_else(|| bad(CMD, "`latex` must be the equation's LaTeX source"))?;
    if latex.trim().is_empty() {
        return Err(bad(CMD, "`latex` is empty; delete the equation instead"));
    }
    let d = s.doc()?;
    let sheet = d.wb.active_sheet();
    let sh = d.wb.active().ok_or_else(|| bad(CMD, "no such equation"))?;
    let shape = sh.shapes.iter().find(|x| x.id == id && x.kind == ShapeKind::Equation).ok_or_else(|| bad(CMD, "no such equation"))?;
    if sh.is_protected() {
        return Err(EngineError::Other(PROTECTED.into()));
    }
    // The same source is a no-op, unless it has no layout yet (a file with more equations than
    // are laid out at once): then OK lays it out.
    if shape.text == latex && matches!(shape.math.get(latex), Some(Ok(_))) {
        return ok();
    }
    let list = lay_out(CMD, latex)?;
    // Keep the size the equation is drawn at now; without a layout, use the default size.
    let scale = match shape.math.get(&shape.text) {
        Some(Ok(old)) => fit_scale(old, shape.anchor.width, shape.anchor.height),
        _ => 0.0,
    };
    let scale = if scale > 0.1 { scale } else { (DEFAULT_SIZE_PT * PX_PER_PT) as f32 };
    let (width, height) = frame_for(&list, scale);
    edit(s, |cx| {
        let shape = cx.sheet_mut(sheet)?.shapes.iter_mut().find(|x| x.id == id).ok_or_else(|| bad(CMD, "no such equation"))?;
        shape.text = latex.to_string();
        shape.math = EquationCache::default();
        shape.math.refresh(latex);
        shape.anchor.width = width;
        shape.anchor.height = height;
        Ok(json!({"equation": id}))
    })
}

// ---------------------------------------------------------------- PDF

/// The first [`gridcraft_math::MAX_SOURCE_BYTES`] bytes of an equation's source, cut at a char
/// boundary. A longer source never lays out (the editor refuses it, and a file can carry one of
/// megabytes), so what shows it as text, the sheet painter and the PDF fallback, shows this much
/// and does no work on the rest.
pub fn source_excerpt(text: &str) -> &str {
    let cap = gridcraft_math::MAX_SOURCE_BYTES;
    if text.len() <= cap {
        return text;
    }
    let mut end = cap;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or("")
}

/// The PDF face for a glyph: Symbol for Greek letters and operators, else the Times (or
/// Helvetica, Courier) face for its style. `None` when no standard font can show `c`.
fn pdf_font(c: char, face: gridcraft_math::Face) -> Option<gridcraft_pdf::Font> {
    use gridcraft_math::Face;
    use gridcraft_pdf::Font;
    let text = match face {
        Face::Italic => Font::TimesItalic,
        Face::Bold => Font::TimesBold,
        Face::BoldItalic => Font::TimesBoldItalic,
        Face::Sans => Font::Helvetica,
        Face::SansBold => Font::HelveticaBold,
        Face::SansItalic => Font::HelveticaOblique,
        Face::Mono => Font::Courier,
        Face::Roman | Face::Large(_) => Font::TimesRoman,
    };
    // Letters and digits stay in the text face; everything Symbol has goes to Symbol, whose
    // operators (minus, equals, plus) match the maths better than the text faces'.
    if c.is_ascii_alphanumeric() {
        return Some(text);
    }
    if Font::Symbol.can_encode(c) {
        return Some(Font::Symbol);
    }
    text.can_encode(c).then_some(text)
}

/// Draws an equation's layout into the box `(x, y, w, h)` (page points) of a PDF page, with
/// `pad` points of padding. Glyphs no standard font has are drawn as `[?]` boxes.
pub(crate) fn draw_pdf(pg: &mut gridcraft_pdf::Page, list: &MathList, rect: (f32, f32, f32, f32), pad: f32, ink: gridcraft_pdf::Rgb) {
    use gridcraft_math::{Face, Item, PathCmd};
    let (x, y, w, h) = rect;
    let (cw, ch) = (list.width.max(0.01), list.total_height().max(0.01));
    let s = ((w - 2.0 * pad) / cw).min((h - 2.0 * pad) / ch);
    if !(s.is_finite() && s > 0.01) {
        return;
    }
    let s = s.min(4000.0);
    let (ox, oy) = (x + (w - cw * s) / 2.0, y + (h - ch * s) / 2.0);
    let col = |c: Option<[u8; 3]>| c.unwrap_or(ink);
    for item in list.items.iter().take(gridcraft_math::MAX_ITEMS) {
        match item {
            Item::Glyph(g) => {
                let color = col(g.color);
                let (gx, base) = (ox + g.x * s, oy + g.y * s);
                // The radical sign: Symbol's glyph (AFM bounding box -147 to 917 high, right edge
                // at 558, in 1/1000 em) fitted to the TeX box, so its top meets the vinculum.
                if g.ch == '\u{221A}' {
                    let size = (g.height + g.depth) * s / 1.064;
                    if size.is_finite() && size >= 0.5 {
                        let top = base - g.height * s;
                        pg.text(gx + g.width * s - 0.558 * size, top + 0.917 * size, size, gridcraft_pdf::Font::Symbol, color, "\u{221A}");
                    }
                    continue;
                }
                let large = matches!(g.face, Face::Large(_));
                let size = if large { (g.height + g.depth) * s } else { g.size * s };
                if !(size.is_finite() && size >= 0.5) {
                    continue;
                }
                let pick = pdf_font(g.ch, g.face).map(|f| (g.ch, f)).or_else(|| g.alt.and_then(|a| pdf_font(a, g.face).map(|f| (a, f))));
                match pick {
                    Some((c, font)) => {
                        let text = c.to_string();
                        if large {
                            // Centred on the glyph's box, like the screen.
                            let mid = oy + (g.y - (g.height - g.depth) / 2.0) * s;
                            let tw = gridcraft_pdf::text_width(font, size, &text);
                            pg.text(gx + (g.width * s - tw) / 2.0, mid + 0.25 * size, size, font, color, &text);
                        } else {
                            pg.text(gx, base, size, font, color, &text);
                        }
                    }
                    None => {
                        // [?]: the glyph's box with a question mark.
                        let (bw, bh) = ((g.width.max(0.3)) * s, (g.height + g.depth) * s);
                        pg.rect_stroke(gx, base - g.height * s, bw, bh, (0.05 * s).clamp(0.2, 1.0), color);
                        let qs = (bh * 0.6).min(bw * 1.2).max(1.0);
                        let qw = gridcraft_pdf::text_width(gridcraft_pdf::Font::Helvetica, qs, "?");
                        pg.text(gx + (bw - qw) / 2.0, base - g.height * s + (bh + qs * 0.7) / 2.0, qs, gridcraft_pdf::Font::Helvetica, color, "?");
                    }
                }
            }
            Item::Rule(r) => {
                if r.width > 0.0 && r.height > 0.0 {
                    pg.fill_rect(ox + r.x * s, oy + r.y * s, r.width * s, (r.height * s).max(0.25), col(r.color));
                }
            }
            Item::Path(p) => {
                let (px, py) = (ox + p.x * s, oy + p.y * s);
                let mut subs: Vec<Vec<[f32; 2]>> = vec![];
                let mut cur: Vec<[f32; 2]> = vec![];
                let at = |a: f32, b: f32| [px + a * s, py + b * s];
                for c in &p.commands {
                    match *c {
                        PathCmd::MoveTo(a, b) => {
                            if cur.len() > 1 {
                                subs.push(std::mem::take(&mut cur));
                            }
                            cur = vec![at(a, b)];
                        }
                        PathCmd::LineTo(a, b) => cur.push(at(a, b)),
                        PathCmd::QuadTo(x1, y1, a, b) => {
                            let p0 = cur.last().copied().unwrap_or(at(x1, y1));
                            let (c1, p1) = (at(x1, y1), at(a, b));
                            for k in 1..=8 {
                                let t = k as f32 / 8.0;
                                let u = 1.0 - t;
                                cur.push([u * u * p0[0] + 2.0 * u * t * c1[0] + t * t * p1[0], u * u * p0[1] + 2.0 * u * t * c1[1] + t * t * p1[1]]);
                            }
                        }
                        PathCmd::CubicTo(x1, y1, x2, y2, a, b) => {
                            let p0 = cur.last().copied().unwrap_or(at(x1, y1));
                            let (c1, c2, p1) = (at(x1, y1), at(x2, y2), at(a, b));
                            for k in 1..=10 {
                                let t = k as f32 / 10.0;
                                let u = 1.0 - t;
                                let (a3, b3, c3, d3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                                cur.push([a3 * p0[0] + b3 * c1[0] + c3 * c2[0] + d3 * p1[0], a3 * p0[1] + b3 * c1[1] + c3 * c2[1] + d3 * p1[1]]);
                            }
                        }
                        PathCmd::Close => {
                            if cur.len() > 1 {
                                subs.push(std::mem::take(&mut cur));
                            }
                        }
                    }
                }
                if cur.len() > 1 {
                    subs.push(cur);
                }
                let color = col(p.color);
                for sub in subs.iter().filter(|v| v.len() <= 4000) {
                    if p.fill && sub.len() >= 3 {
                        // The PDF fill rule handles concave outlines itself.
                        pg.polygon(sub, Some(color), None);
                    } else {
                        pg.polyline(sub, (0.05 * s).clamp(0.2, 1.0), color, &[]);
                    }
                }
            }
        }
    }
}
