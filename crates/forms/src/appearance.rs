//! Field appearances (§12.7.4.3 and §12.5.5), execution plan M6.2.
//!
//! Text and choice fields are drawn from their `/DA` (font, size, colour) and the widget's `/MK`
//! (background and border colours) and `/BS` (border width and style), inside a `/Tx BMC … EMC`
//! marked-content block as viewers expect. Supported: single-line, multiline (wrapped), comb,
//! password, quadding, auto font size (`0 Tf`), combo boxes and list boxes (selection shown).
//!
//! The `/DA` font is used when the form's `/DR` defines it as a simple font; text is encoded in
//! `WinAnsi`. Otherwise (composite fonts, missing resources) Helvetica is used, so text is always
//! visible. Widths use the approximate Helvetica metrics of `printcraft-fonts`.

use std::fmt::Write as _;

use printcraft_cos::{Dict, Document, Object, Stream};
use printcraft_fonts::{helvetica_width, literal, win_ansi, wrap};

use crate::{Field, FieldKind, Widget, acroform, flags};

/// Format a number for content streams.
fn n(v: f64) -> String {
    let s = format!("{:.3}", if v.abs() < 5e-4 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// Format a number for content streams (up to three decimals).
#[must_use]
pub fn fmt(v: f64) -> String {
    n(v)
}

/// A parsed default appearance string: font resource name, size (0 = auto) and colour operator.
#[derive(Clone, Debug, PartialEq)]
pub struct Da {
    pub font: String,
    pub size: f64,
    /// The colour operation as written (`0 g`, `1 0 0 rg`, `0 0 0 1 k`).
    pub color: String,
}

#[must_use]
pub fn parse_da(da: &str) -> Da {
    let toks: Vec<&str> = da.split_whitespace().collect();
    let mut out = Da { font: "Helv".into(), size: 0.0, color: "0 g".into() };
    for (i, t) in toks.iter().enumerate() {
        match *t {
            "Tf" if i >= 2 => {
                out.font = toks[i - 2].trim_start_matches('/').to_string();
                out.size = toks[i - 1].parse::<f64>().ok().filter(|s| s.is_finite() && *s >= 0.0).unwrap_or(0.0).min(300.0);
            }
            "g" if i >= 1 && toks[i - 1].parse::<f64>().is_ok() => out.color = format!("{} g", toks[i - 1]),
            "rg" if i >= 3 && toks[i - 3..i].iter().all(|x| x.parse::<f64>().is_ok()) => out.color = format!("{} rg", toks[i - 3..i].join(" ")),
            "k" if i >= 4 && toks[i - 4..i].iter().all(|x| x.parse::<f64>().is_ok()) => out.color = format!("{} k", toks[i - 4..i].join(" ")),
            _ => {}
        }
    }
    out
}

/// The font resource for `name` from the form's `/DR`, if it is a simple font we can encode for.
fn dr_font(doc: &Document, name: &str) -> Option<Object> {
    let af = acroform(doc)?;
    let dr = doc.resolve(af.get(b"DR")?);
    let fonts = doc.resolve(dr.as_dict()?.get(b"Font")?);
    let entry = fonts.as_dict()?.get(name.as_bytes())?.clone();
    let font = doc.resolve(&entry);
    let fd = font.as_dict()?;
    match fd.name(b"Subtype") {
        Some(b"Type1" | b"TrueType" | b"MMType1") => {}
        _ => return None,
    }
    // Symbolic fonts (ZapfDingbats, Symbol) can't show WinAnsi text.
    if matches!(fd.name(b"BaseFont"), Some(b"ZapfDingbats" | b"Symbol")) {
        return None;
    }
    Some(entry)
}

fn helvetica() -> Object {
    let mut f = Dict::new();
    f.set(b"Type".to_vec(), Object::name("Font"));
    f.set(b"Subtype".to_vec(), Object::name("Type1"));
    f.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
    f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    Object::Dict(f)
}

fn color_array(doc: &Document, mk: Option<&Dict>, key: &[u8]) -> Option<String> {
    let a = doc.resolve(mk?.get(key)?);
    let v: Vec<f64> = a.as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
    match v.as_slice() {
        [g] => Some(format!("{} g", n(*g))),
        [r, g, b] => Some(format!("{} {} {} rg", n(*r), n(*g), n(*b))),
        [c, m, y, k] => Some(format!("{} {} {} {} k", n(*c), n(*m), n(*y), n(*k))),
        _ => None,
    }
}

fn stroke_op(fill: &str) -> String {
    match fill.rsplit_once(' ') {
        Some((nums, "g")) => format!("{nums} G"),
        Some((nums, "rg")) => format!("{nums} RG"),
        Some((nums, "k")) => format!("{nums} K"),
        _ => "0 G".into(),
    }
}

/// Background and border from `/MK` and `/BS`; returns (content, border width).
fn frame(doc: &Document, wd: &Dict, w: f64, h: f64) -> (String, f64) {
    let mk = wd.get(b"MK").map(|m| doc.resolve(m)).and_then(|m| m.as_dict().cloned());
    let mut c = String::new();
    if let Some(bg) = color_array(doc, mk.as_ref(), b"BG") {
        let _ = write!(c, "{bg}\n0 0 {} {} re f\n", n(w), n(h));
    }
    let bs = wd.get(b"BS").map(|b| doc.resolve(b)).and_then(|b| b.as_dict().cloned());
    let bw = bs.as_ref().and_then(|b| b.get(b"W")).and_then(|x| doc.resolve(x).as_f64()).unwrap_or(1.0).max(0.0);
    if let Some(bc) = color_array(doc, mk.as_ref(), b"BC")
        && bw > 0.0
    {
        let style = bs.as_ref().and_then(|b| b.name(b"S").map(<[u8]>::to_vec)).unwrap_or_default();
        if style == b"U" {
            // Underline: only the bottom edge.
            let _ = write!(c, "{}\n{} w\n0 {} m {} {} l S\n", stroke_op(&bc), n(bw), n(bw / 2.0), n(w), n(bw / 2.0));
            return (c, bw);
        }
        let dashed = style == b"D";
        let _ = write!(
            c,
            "{}\n{} w\n{}{} {} {} {} re S\n[] 0 d\n",
            stroke_op(&bc),
            n(bw),
            if dashed { "[3] 0 d\n" } else { "" },
            n(bw / 2.0),
            n(bw / 2.0),
            n(w - bw),
            n(h - bw)
        );
        if style == b"B" || style == b"I" {
            // Beveled: a light top-left and a darker bottom-right inside the border; inset:
            // grey top-left and light grey bottom-right.
            let (tl, br) = if style == b"B" { ("1 g", "0.5 g") } else { ("0.5 g", "0.75 g") };
            let (x0, y0, x1, y1) = (bw, bw, w - bw, h - bw);
            let k = bw;
            let _ = write!(
                c,
                "{tl}\n{} {} m {} {} l {} {} l {} {} l {} {} l {} {} l f\n{br}\n{} {} m {} {} l {} {} l {} {} l {} {} l {} {} l f\n",
                n(x0),
                n(y0),
                n(x0),
                n(y1),
                n(x1),
                n(y1),
                n(x1 - k),
                n(y1 - k),
                n(x0 + k),
                n(y1 - k),
                n(x0 + k),
                n(y0 + k),
                n(x1),
                n(y1),
                n(x1),
                n(y0),
                n(x0),
                n(y0),
                n(x0 + k),
                n(y0 + k),
                n(x1 - k),
                n(y0 + k),
                n(x1 - k),
                n(y1 - k)
            );
            return (c, 2.0 * bw);
        }
        return (c, bw);
    }
    (c, 0.0)
}

/// The appearance of a text or choice field's widget showing `values`.
#[must_use]
pub fn field_appearance(doc: &Document, f: &Field, w: &Widget, values: &[String]) -> Stream {
    field_appearance_as(doc, f, w, values, true)
}

/// [`field_appearance`], where `format` false means `values` are already what to show (a
/// custom Format script ran).
#[must_use]
pub fn field_appearance_as(doc: &Document, f: &Field, w: &Widget, values: &[String], format: bool) -> Stream {
    // The Format event: what is shown, not what is stored.
    let formatted: Vec<String>;
    let values = if format
        && matches!(f.kind, crate::FieldKind::Text | crate::FieldKind::Combo)
        && values.len() == 1
        && f.actions.format != crate::af::Format::None
    {
        formatted = vec![crate::af::format_value(&f.actions.format, &values[0])];
        &formatted[..]
    } else {
        values
    };
    let wobj = doc.get(w.obj);
    let wd = wobj.as_dict().cloned().unwrap_or_default();
    let (width, height) = ((w.rect[2] - w.rect[0]).max(1.0), (w.rect[3] - w.rect[1]).max(1.0));
    let da = parse_da(wd.get(b"DA").and_then(|o| doc.resolve(o).as_string().map(printcraft_cos::PdfString::to_text)).as_deref().unwrap_or(&f.da));
    let (font_name, font_obj) = match dr_font(doc, &da.font) {
        Some(o) => (da.font.clone(), o),
        None => ("Helv".to_string(), helvetica()),
    };
    let (mut c, bw) = frame(doc, &wd, width, height);
    let pad = 2.0 + bw;
    let inner_w = (width - 2.0 * pad).max(1.0);
    let q = wd.get(b"Q").and_then(|o| doc.resolve(o).as_int()).unwrap_or(f.quadding);
    let mut body: Vec<u8> = Vec::new();
    let show = |body: &mut Vec<u8>, x: f64, y: f64, text: &str| {
        body.extend(format!("1 0 0 1 {} {} Tm ", n(x), n(y)).bytes());
        body.extend(literal(&win_ansi(text)));
        body.extend_from_slice(b" Tj\n");
    };
    let x_for = |text: &str, size: f64| -> f64 {
        let tw = helvetica_width(text, size);
        match q {
            1 => pad + (inner_w - tw) / 2.0,
            2 => width - pad - tw,
            _ => pad,
        }
    };
    let mut size = da.size;
    if f.kind == FieldKind::List {
        // Every option, one per line from the top; selected ones highlighted.
        if size == 0.0 {
            size = 12.0;
        }
        let line = size * 1.15;
        let top = wd.get(b"TI").and_then(|o| doc.resolve(o).as_int()).unwrap_or(0).max(0) as usize;
        let mut y = height - pad;
        for (export, display) in f.options.iter().skip(top) {
            if y - line < 0.0 {
                break;
            }
            if values.contains(export) {
                let _ = write!(c, "0.6 0.75 0.86 rg\n{} {} {} {} re f\n", n(bw), n(y - line), n(width - 2.0 * bw), n(line));
            }
            show(&mut body, pad, y - line + size * 0.25, display);
            y -= line;
        }
    } else {
        let text: String = if f.kind == FieldKind::Combo {
            values.iter().map(|v| f.options.iter().find(|(e, _)| e == v).map_or(v.clone(), |(_, d)| d.clone())).collect::<Vec<_>>().join(", ")
        } else {
            values.first().cloned().unwrap_or_default()
        };
        let text = if f.has(flags::PASSWORD) { "*".repeat(text.chars().count()) } else { text };
        let comb = f.has(flags::COMB) && !f.has(flags::MULTILINE) && !f.has(flags::PASSWORD) && f.max_len.is_some_and(|m| m > 0);
        if f.kind == FieldKind::Text && f.has(flags::MULTILINE) {
            if size == 0.0 {
                // Auto size: the largest size (≤ 12) whose wrapped lines fit the height.
                size = 12.0;
                while size > 4.0 && wrap(&text, size, inner_w).len() as f64 * size * 1.15 > height - 2.0 * pad {
                    size -= 0.5;
                }
            }
            let mut y = height - pad - size * 0.85;
            for line in wrap(&text, size, inner_w) {
                if y < -size {
                    break;
                }
                show(&mut body, x_for(&line, size), y, &line);
                y -= size * 1.15;
            }
        } else {
            if size == 0.0 {
                size = ((height - 2.0 * pad) / 1.15).clamp(4.0, 12.0);
                let tw = helvetica_width(&text, size);
                if tw > inner_w && !comb {
                    size = (size * inner_w / tw).max(4.0);
                }
            }
            // Centre Helvetica's ascent (0.718) and descent (0.207) vertically.
            let y = (height - 0.925 * size) / 2.0 + 0.207 * size;
            if comb {
                let cells = f.max_len.unwrap_or(1).max(1);
                let cell = width / cells as f64;
                for (i, ch) in text.chars().take(cells).enumerate() {
                    let s = ch.to_string();
                    show(&mut body, cell * i as f64 + (cell - helvetica_width(&s, size)) / 2.0, y, &s);
                }
            } else {
                show(&mut body, x_for(&text, size), y, &text);
            }
        }
    }
    let mut content = c.into_bytes();
    content.extend(
        format!(
            "/Tx BMC\nq\n{} {} {} {} re W n\nBT\n/{} {} Tf\n{}\n",
            n(bw),
            n(bw),
            n(width - 2.0 * bw),
            n(height - 2.0 * bw),
            font_name,
            n(size),
            da.color
        )
        .bytes(),
    );
    content.extend(body);
    content.extend_from_slice(b"ET\nQ\nEMC\n");
    let mut fonts = Dict::new();
    fonts.set(font_name.into_bytes(), font_obj);
    let mut res = Dict::new();
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), Object::Array([0.0, 0.0, width, height].iter().map(|v| Object::Real(*v)).collect()));
    d.set(b"Resources".to_vec(), Object::Dict(res));
    Stream::flate(d, &content)
}

/// On/Off appearances for a check box or radio button that has none (a check mark or a dot,
/// drawn as paths, so no symbol font is needed).
/// The streams are added to `doc` as indirect objects (streams can't be direct objects).
pub fn check_box_states(doc: &mut Document, w: &Widget, kind: FieldKind, on_name: &str) -> Dict {
    let wobj = doc.get(w.obj);
    let wd = wobj.as_dict().cloned().unwrap_or_default();
    let (width, height) = ((w.rect[2] - w.rect[0]).max(1.0), (w.rect[3] - w.rect[1]).max(1.0));
    let (frame_c, _) = frame(doc, &wd, width, height);
    let mut form = |content: String| -> Object {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("XObject"));
        d.set(b"Subtype".to_vec(), Object::name("Form"));
        d.set(b"BBox".to_vec(), Object::Array([0.0, 0.0, width, height].iter().map(|v| Object::Real(*v)).collect()));
        Object::Ref(doc.add(Object::Stream(Stream::flate(d, content.as_bytes()))))
    };
    let s = width.min(height);
    let (cx, cy) = (width / 2.0, height / 2.0);
    let mark = if kind == FieldKind::Radio {
        let r = s * 0.25;
        let k = 0.5523 * r;
        format!(
            "0 g\n{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c f\n",
            n(cx + r),
            n(cy),
            n(cx + r),
            n(cy + k),
            n(cx + k),
            n(cy + r),
            n(cx),
            n(cy + r),
            n(cx - k),
            n(cy + r),
            n(cx - r),
            n(cy + k),
            n(cx - r),
            n(cy),
            n(cx - r),
            n(cy - k),
            n(cx - k),
            n(cy - r),
            n(cx),
            n(cy - r),
            n(cx + k),
            n(cy - r),
            n(cx + r),
            n(cy - k),
            n(cx + r),
            n(cy)
        )
    } else {
        format!(
            "0 G\n{} w 1 J 1 j\n{} {} m {} {} l {} {} l S\n",
            n((s * 0.1).max(1.0)),
            n(cx - s * 0.28),
            n(cy),
            n(cx - s * 0.08),
            n(cy - s * 0.22),
            n(cx + s * 0.3),
            n(cy + s * 0.25)
        )
    };
    let mut nd = Dict::new();
    nd.set(on_name.as_bytes().to_vec(), form(format!("{frame_c}{mark}")));
    nd.set(b"Off".to_vec(), form(frame_c));
    let mut ap = Dict::new();
    ap.set(b"N".to_vec(), Object::Dict(nd));
    ap
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_appearance_strings_parse() {
        assert_eq!(parse_da("/Helv 0 Tf 0 g"), Da { font: "Helv".into(), size: 0.0, color: "0 g".into() });
        assert_eq!(parse_da("0.1 0.2 0.3 rg /F1 11 Tf"), Da { font: "F1".into(), size: 11.0, color: "0.1 0.2 0.3 rg".into() });
        assert_eq!(parse_da("/Cour 9 Tf 0 0 0 1 k").color, "0 0 0 1 k");
        assert_eq!(parse_da("garbage").font, "Helv");
        assert_eq!(stroke_op("1 0 0 rg"), "1 0 0 RG");
    }
}
