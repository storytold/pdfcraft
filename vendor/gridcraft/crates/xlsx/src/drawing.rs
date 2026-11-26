//! DrawingML spreadsheet drawings (`xl/drawings/drawingN.xml`): anchors, pictures, shapes,
//! equations and chart frames.
//!
//! An equation is written as `mc:AlternateContent`: the `a14` choice is a text box holding the
//! equation as Office Math (`a14:m`), the fallback the same text box holding the LaTeX as plain
//! text. Both carry the source in `descr="gridcraft:latex:…"`, which wins on reading; without it,
//! Office Math in the subset `omml` reads is converted back to LaTeX.

use std::fmt::Write as _;

use craft_xlsx::{Anchor as CraftAnchor, Drawing as CraftDrawing, Item as CraftItem, Marker as CraftMarker, Placement as CraftPlacement};
use gridcraft_core::{CellRef, MAX_COLS, MAX_ROWS};
use gridcraft_model::{Anchor, AnchorMode, Color, Image, Shape, ShapeFormat, ShapeKind, ShapeTextAlign, ShapeTextAnchor, Sheet, Theme};

use crate::IoError;
use crate::read::Ctx;
use crate::tables::EMU_PER_PX;
use crate::xml::{El, esc, esc_attr};

mod ink;

// ---------------------------------------------------------------- reading

fn emu_px(v: Option<i64>) -> f32 {
    v.map(|e| (e as f64 / EMU_PER_PX).clamp(-1e7, 1e7) as f32).unwrap_or(0.0)
}

fn emu_to_px(v: i64) -> f32 {
    (v as f64 / EMU_PER_PX).clamp(-1e7, 1e7) as f32
}

pub(crate) fn marker(e: &El) -> (CellRef, f32, f32) {
    let num = |n: &str| e.child(n).and_then(|c| c.text.trim().parse::<i64>().ok());
    let col = num("col").unwrap_or(0).clamp(0, MAX_COLS as i64 - 1) as u32;
    let row = num("row").unwrap_or(0).clamp(0, MAX_ROWS as i64 - 1) as u32;
    (CellRef::new(row, col), emu_px(num("colOff")).max(0.0), emu_px(num("rowOff")).max(0.0))
}

fn craft_marker(m: CraftMarker) -> (CellRef, f32, f32) {
    let col = m.col.min(MAX_COLS - 1);
    let row = m.row.min(MAX_ROWS - 1);
    (CellRef::new(row, col), emu_to_px(m.col_off).max(0.0), emu_to_px(m.row_off).max(0.0))
}

fn placement_to_anchor(p: &CraftPlacement, sheet: &Sheet) -> Anchor {
    match p {
        CraftPlacement::TwoCell { from, to, edit_as } => {
            let (cell, dx, dy) = craft_marker(*from);
            let (to, tdx, tdy) = craft_marker(*to);
            let x1 = sheet.col_left(cell.col) + dx as f64;
            let y1 = sheet.row_top(cell.row) + dy as f64;
            let x2 = sheet.col_left(to.col) + tdx as f64;
            let y2 = sheet.row_top(to.row) + tdy as f64;
            let mode = match edit_as.as_deref() {
                Some("absolute") => AnchorMode::Absolute,
                Some("oneCell") => AnchorMode::MoveOnly,
                _ => AnchorMode::MoveAndSize,
            };
            Anchor { cell, dx, dy, width: (x2 - x1).max(0.0) as f32, height: (y2 - y1).max(0.0) as f32, mode }
        }
        CraftPlacement::OneCell { from, cx, cy } => {
            let (cell, dx, dy) = craft_marker(*from);
            Anchor { cell, dx, dy, width: emu_to_px(*cx).max(0.0), height: emu_to_px(*cy).max(0.0), mode: AnchorMode::MoveOnly }
        }
        CraftPlacement::Absolute { x, y, cx, cy } => {
            let x = emu_to_px(*x).max(0.0) as f64;
            let y = emu_to_px(*y).max(0.0) as f64;
            let col = sheet.col_at(x);
            let row = sheet.row_at(y);
            Anchor {
                cell: CellRef::new(row, col),
                dx: (x - sheet.col_left(col)).max(0.0) as f32,
                dy: (y - sheet.row_top(row)).max(0.0) as f32,
                width: emu_to_px(*cx).max(0.0),
                height: emu_to_px(*cy).max(0.0),
                mode: AnchorMode::Absolute,
            }
        }
    }
}

fn from_craft_el(e: &craft_xml::El) -> El {
    let name = e.name.split_once(':').map_or(e.name.as_str(), |(_, local)| local).to_string();
    let attrs = e.attrs.iter().map(|(k, v)| (k.split_once(':').map_or(k.as_str(), |(_, local)| local).to_string(), v.clone())).collect();
    let mut children = Vec::new();
    let mut text = String::new();
    for node in &e.children {
        match node {
            craft_xml::Node::El(c) => children.push(from_craft_el(c)),
            craft_xml::Node::Text(t) | craft_xml::Node::CData(t) => text.push_str(t),
            craft_xml::Node::Pi { .. } | craft_xml::Node::Comment(_) => {}
        }
    }
    El { name, attrs, children, text }
}

/// Colour of a DrawingML colour choice inside `e` (e.g. a `solidFill`).
pub fn dml_color(e: &El) -> Option<Color> {
    if let Some(c) = e.child("srgbClr").and_then(|c| c.attr("val")) {
        return Color::from_hex(c);
    }
    if let Some(c) = e.child("sysClr").and_then(|c| c.attr("lastClr")) {
        return Color::from_hex(c);
    }
    if let Some(c) = e.child("schemeClr").and_then(|c| c.attr("val")) {
        let i = match c {
            "bg1" | "lt1" => 0,
            "tx1" | "dk1" => 1,
            "bg2" | "lt2" => 2,
            "tx2" | "dk2" => 3,
            "accent1" => 4,
            "accent2" => 5,
            "accent3" => 6,
            "accent4" => 7,
            "accent5" => 8,
            "accent6" => 9,
            "hlink" => 10,
            "folHlink" => 11,
            _ => return None,
        };
        return Some(Color::Theme(i, 0));
    }
    None
}

fn mime_for(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" | "jpe" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        "emf" => "image/x-emf",
        "wmf" => "image/x-wmf",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

pub fn read_drawing(cx: &mut Ctx<'_>, part: &str, sheet: &mut Sheet) -> Result<(), IoError> {
    let Some(bytes) = cx.pkg.read(part)? else {
        cx.warn(format!("missing part {part}"));
        return Ok(());
    };
    let opts = craft_xml::Options::canonical(&craft_xlsx::ns::CANONICAL).with_limits(craft_xml::Limits {
        max_input: crate::package::MAX_PART as usize,
        max_depth: crate::xml::MAX_DEPTH,
        max_nodes: crate::xml::MAX_XML_NODES,
        ..craft_xml::Limits::DEFAULT
    });
    let root = match opts.parse(&bytes) {
        Ok(r) => r,
        Err(e) if e.is_limit() => return Err(IoError::TooLarge(format!("{part}: {e}"))),
        Err(e) => {
            cx.warn(format!("skipped {part}: {e}"));
            return Ok(());
        }
    };
    let dr = CraftDrawing::read(&root);
    let rels = cx.pkg.rels(part)?;
    let mut anchors: Vec<&CraftAnchor> = Vec::new();
    let mut alt_anchors: Vec<CraftAnchor> = Vec::new();
    for item in &dr.items {
        match item {
            CraftItem::Anchor(a) => anchors.push(a),
            CraftItem::Other(e) if e.name == "mc:AlternateContent" => {
                alt_anchors.extend(CraftDrawing::alternate_anchors(e));
            }
            CraftItem::Other(_) => {}
        }
    }
    for (n, craft_a) in anchors.into_iter().chain(alt_anchors.iter()).enumerate() {
        let anchor = placement_to_anchor(&craft_a.placement, sheet);
        let n = n + 1;
        let obj_el = from_craft_el(&craft_a.object);
        let objs: Vec<&El> = match obj_el.name.as_str() {
            "AlternateContent" => obj_el
                .child("Choice")
                .filter(|c| {
                    c.child("graphicFrame").is_some()
                        || c.child("sp").and_then(|sp| sp.child("txBody")).and_then(crate::omml::equation_source).is_some()
                })
                .or_else(|| obj_el.child("Fallback"))
                .map(|c| c.children.iter().collect())
                .unwrap_or_default(),
            _ => vec![&obj_el],
        };
        for obj in objs {
            match obj.name.as_str() {
                "pic" => {
                    let Some(rid) = obj.path(&["blipFill", "blip"]).and_then(|b| b.attr("embed")) else { continue };
                    let Some(rel) = rels.iter().find(|r| r.id == rid && !r.external) else {
                        cx.warn("a linked (not embedded) picture was skipped");
                        continue;
                    };
                    let target = rel.target.clone();
                    let Some(data) = cx.pkg.read(&target)? else {
                        cx.warn(format!("missing picture {target}"));
                        continue;
                    };
                    let alt = obj.path(&["nvPicPr", "cNvPr"]).and_then(|c| c.attr("descr")).unwrap_or("").to_string();
                    let id = cx.next_object_id();
                    sheet.images.push(Image { id, anchor, data: data.into(), mime: mime_for(&target).to_string(), alt });
                }
                "graphicFrame" => {
                    let Some(gd) = obj.path(&["graphic", "graphicData"]) else { continue };
                    let uri = gd.attr("uri").unwrap_or("");
                    if let Some(rid) = gd.child("chart").and_then(|c| c.attr("id")) {
                        let Some(rel) = rels.iter().find(|r| r.id == rid) else {
                            cx.warn(format!("a chart without its chart part was skipped ({part}, object {n})"));
                            continue;
                        };
                        let target = rel.target.clone();
                        if let Some(cx_xml) = cx.optional_xml(&target)? {
                            let chartex = uri.contains("chartex");
                            match if chartex { crate::chartex::read_chartex(&cx_xml) } else { crate::chart::read_chart(&cx_xml) } {
                                Some(mut ch) => {
                                    ch.id = cx.next_object_id();
                                    ch.anchor = anchor;
                                    sheet.charts.push(ch);
                                }
                                None => cx.warn(format!("chart {target} has no supported plot and was skipped")),
                            }
                        } else {
                            cx.warn(format!("chart {target} is missing from the package and was skipped ({part}, object {n})"));
                        }
                    } else if uri.contains("chartex") || uri.contains("chartEx") {
                        cx.warn(format!("a histogram, waterfall, treemap… chart without its chart part was skipped ({part}, object {n})"));
                    } else {
                        cx.warn(format!("an embedded object (SmartArt, OLE…) was skipped ({part}, object {n})"));
                    }
                }
                "sp" | "cxnSp" => {
                    if let Some(s) = read_shape(obj, anchor) {
                        let id = cx.next_object_id();
                        sheet.shapes.push(Shape { id, ..s });
                    }
                }
                "grpSp" => cx.warn(format!("grouped shapes are not supported and were skipped ({part}, object {n})")),
                _ => {}
            }
        }
    }
    Ok(())
}

fn read_shape(obj: &El, anchor: Anchor) -> Option<Shape> {
    let sp = obj.child("spPr")?;
    if let Some(shape) = ink::read(obj, anchor) {
        return Some(shape);
    }
    let prst = sp.child("prstGeom").and_then(|g| g.attr("prst")).unwrap_or("rect");
    let tx_box = obj.path(&["nvSpPr", "cNvSpPr"]).is_some_and(|c| c.flag("txBox", false));
    let ln = sp.child("ln");
    let arrow = ln.is_some_and(|l| l.child("tailEnd").or_else(|| l.child("headEnd")).is_some_and(|t| t.attr("type").is_some_and(|v| v != "none")));
    // WordArt: our marker, a text body with the non-warping text warp `textNoShape`, on a rectangle
    // that is a text box (as the writer saves it) or has no fill and no outline of its own (as
    // WordArt usually comes). `textNoShape` is the standard "no warp" value, so on an ordinary
    // shape (an ellipse, a filled rectangle) it, like any other warp, leaves the shape what its
    // geometry says.
    let word_art = prst == "rect"
        && (tx_box || (sp.child("noFill").is_some() && ln.is_none_or(|l| l.child("solidFill").is_none())))
        && obj.path(&["txBody", "bodyPr", "prstTxWarp"]).is_some_and(|w| w.attr("prst") == Some("textNoShape"));
    let kind = match prst {
        _ if word_art => ShapeKind::WordArt,
        _ if tx_box => ShapeKind::TextBox,
        "roundRect" => ShapeKind::RoundedRectangle,
        "ellipse" => ShapeKind::Ellipse,
        "triangle" => ShapeKind::Triangle,
        "line" | "straightConnector1" if arrow => ShapeKind::Arrow,
        "line" | "straightConnector1" => ShapeKind::Line,
        "rightArrow" => ShapeKind::Arrow,
        _ => ShapeKind::Rectangle,
    };
    // An equation: GridCraft's own source while the marker in the description still matches the
    // body (another application that edits the Office Math, or the text of a box without Office
    // Math, leaves the description alone, as Excel does), else the Office Math converted back to
    // LaTeX. A box without Office Math whose text was edited is a text box with the edited text,
    // not the stale source, which the next save would write over the edit; as the marker joins
    // run text across paragraphs, such a box must also hold the paragraphs the writer wrote.
    let descr = obj.path(&["nvSpPr", "cNvPr"]).and_then(|c| c.attr("descr")).unwrap_or("");
    let tb = obj.child("txBody");
    let has_math = tb.is_some_and(|tb| crate::omml::find_math(tb, 0).is_some());
    let source = match (has_math, described_source(descr)) {
        (_, Some((marker, src))) if tb.is_some_and(|tb| crate::omml::body_marker(tb) == marker && (has_math || holds_written_text(tb, src))) => {
            Some(src.to_string())
        }
        (true, _) => tb.and_then(crate::omml::equation_source),
        (false, _) => None,
    };
    if let Some(text) = source {
        let ink = obj.child("txBody").and_then(|tb| first_run_color(tb, 0)).unwrap_or_default();
        return Some(Shape {
            id: 0,
            kind: ShapeKind::Equation,
            anchor,
            fill: Color::Auto,
            line: ink,
            text,
            format: Default::default(),
            rotation: 0.0,
            math: Default::default(),
        });
    }
    let fill = sp.child("solidFill").and_then(dml_color).unwrap_or_default();
    let line = ln.and_then(|l| l.child("solidFill")).and_then(dml_color).unwrap_or_default();
    let text = tb.map(|tb| if has_math { math_body_text(tb) } else { body_text(tb) }).unwrap_or_default();
    let mut format = read_text_format(obj.child("txBody"));
    format.no_line = ln.is_some_and(|l| l.child("noFill").is_some());
    let rotation = sp
        .child("xfrm")
        .and_then(|x| x.attr("rot"))
        .and_then(|r| r.parse::<f64>().ok())
        .filter(|r| r.is_finite())
        .map_or(0.0, |r| ((r / 60000.0).rem_euclid(360.0)) as f32);
    Some(Shape { id: 0, kind, anchor, fill, line, text, format, rotation, math: Default::default() })
}

/// A shape's text formatting: vertical anchor (`a:bodyPr anchor`), the first paragraph's
/// alignment and direction (`a:pPr algn`, `rtl`) and the first run's properties (`a:rPr`: size,
/// bold, italic, fill, outline, `a:latin` typeface).
fn read_text_format(body: Option<&El>) -> ShapeFormat {
    let mut f = ShapeFormat::default();
    let Some(body) = body else { return f };
    f.anchor = body.child("bodyPr").and_then(|b| b.attr("anchor")).map(|a| match a {
        "ctr" => ShapeTextAnchor::Middle,
        "b" => ShapeTextAnchor::Bottom,
        _ => ShapeTextAnchor::Top,
    });
    let ppr = body.kids("p").next().and_then(|p| p.child("pPr"));
    f.align = ppr.and_then(|p| p.attr("algn")).map(|a| match a {
        "ctr" => ShapeTextAlign::Center,
        "r" => ShapeTextAlign::Right,
        _ => ShapeTextAlign::Left,
    });
    f.rtl = ppr.is_some_and(|p| p.flag("rtl", false));
    let rpr = body.kids("p").flat_map(|p| p.kids("r")).find_map(|r| r.child("rPr"));
    if let Some(r) = rpr {
        // `sz` is in hundredths of a point (100..=400000 in the schema).
        f.font_size = r.attr_f64("sz").filter(|v| v.is_finite()).map(|v| (v / 100.0).clamp(1.0, 4000.0) as f32);
        f.bold = r.flag("b", false);
        f.italic = r.flag("i", false);
        f.font = r.child("latin").and_then(|l| l.attr("typeface")).filter(|t| !t.is_empty() && !t.starts_with('+')).map(str::to_string);
        f.text_color = r.child("solidFill").and_then(dml_color);
        f.text_shadow = r.child("effectLst").and_then(|e| e.child("outerShdw")).and_then(dml_color);
        f.text_outline = r.child("ln").and_then(|l| {
            let c = l.child("solidFill").and_then(dml_color)?;
            // Width in EMU (12700 per point); Excel's default hairline is 0.75 pt.
            let w = l.attr_f64("w").filter(|w| w.is_finite()).map_or(0.75, |w| (w / 12700.0).clamp(0.0, 100.0)) as f32;
            Some((c, w))
        });
    }
    f
}

/// The `descr` prefix that carries an equation's LaTeX source, as
/// `gridcraft:latex:<marker>:<source>`: the marker is [`crate::omml::body_marker`] of the text
/// body written beside it, 16 hex digits.
const LATEX_DESCR: &str = "gridcraft:latex:";

/// The marker and source of a `gridcraft:latex:<marker>:<source>` description; `None` for any
/// other description, a malformed marker or a blank source.
fn described_source(descr: &str) -> Option<(&str, &str)> {
    let rest = descr.strip_prefix(LATEX_DESCR)?;
    let (marker, src) = (rest.get(..16)?, rest.get(17..)?);
    (rest.as_bytes().get(16) == Some(&b':') && marker.bytes().all(|b| b.is_ascii_hexdigit()) && !src.trim().is_empty()).then_some((marker, src))
}

/// The colour of the first run (DrawingML `rPr` with a fill) under `e`.
fn first_run_color(e: &El, depth: usize) -> Option<Color> {
    if depth > 12 {
        return None;
    }
    if e.name == "rPr"
        && let Some(c) = e.child("solidFill").and_then(dml_color)
    {
        return Some(c);
    }
    e.children.iter().find_map(|c| first_run_color(c, depth + 1))
}

// ---------------------------------------------------------------- writing

/// The size, in points, of the plain-text box that stands in for an equation.
const PLAIN_TEXT_PT: f32 = 11.0;

/// An equation shape (inside the anchor): Office Math with a plain-text fallback when the source
/// is in the subset OMML covers, else the plain-text box alone. Both carry the source.
fn write_equation(s: &mut String, sh: &Shape, id: usize, theme: &Theme) {
    // The font size the equation is drawn at (it scales with its frame), in points.
    let size_pt = sh
        .math
        .get(&sh.text)
        .and_then(Result::ok)
        .map(|l| {
            let (w, h) = (l.width.max(0.01), l.total_height().max(0.01));
            ((sh.anchor.width - 8.0) / w).min((sh.anchor.height - 8.0) / h) * 72.0 / 96.0
        })
        .filter(|v| v.is_finite() && *v >= 1.0)
        .unwrap_or(11.0)
        .min(409.0);
    let fill = srgb(&sh.line, theme).map(|c| format!("<a:solidFill>{c}</a:solidFill>")).unwrap_or_default();
    let sz = (size_pt * 100.0).round() as u32;
    // Each box carries the marker of its own body beside the source in its description, the
    // Office Math box and the plain-text fallback alike; the reader takes the source only while
    // the body it reads still matches its marker.
    let tx = |body: &str| {
        format!(
            "<xdr:txBody><a:bodyPr wrap=\"none\" lIns=\"0\" tIns=\"0\" rIns=\"0\" bIns=\"0\" rtlCol=\"0\" anchor=\"ctr\" anchorCtr=\"1\"/><a:lstStyle/>{body}</xdr:txBody>"
        )
    };
    let marker = |tx: &str| crate::xml::parse(tx.as_bytes()).map(|e| crate::omml::body_marker(&e)).unwrap_or_else(|_| "0".repeat(16));
    let sp = |tx: &str, marker: &str| {
        format!(
            "<xdr:sp macro=\"\" textlink=\"\"><xdr:nvSpPr><xdr:cNvPr id=\"{id}\" name=\"Equation {}\" descr=\"{}\"/><xdr:cNvSpPr txBox=\"1\"/></xdr:nvSpPr><xdr:spPr>{}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:noFill/></xdr:spPr>{tx}</xdr:sp>",
            id - 1,
            esc_attr(&format!("{LATEX_DESCR}{marker}:{}", sh.text)),
            xfrm(&sh.anchor, "a:xfrm")
        )
    };
    // The plain text is the LaTeX source, much wider than the typeset equation: at the equation's
    // size it would overflow the frame in Excel, so it uses a normal text size (11 pt, Excel's
    // default for a text box), smaller only when the frame itself is.
    let plain_sz = (size_pt.min(PLAIN_TEXT_PT) * 100.0).round() as u32;
    let plain = plain_paragraphs(&sh.text, &format!("<a:rPr lang=\"en-US\" sz=\"{plain_sz}\">{fill}</a:rPr>"));
    match gridcraft_math::to_omml(&sh.text, size_pt, sh.line.resolve(theme)) {
        Ok(omml) => {
            let math = format!(
                "<a:p><a:pPr algn=\"ctr\"/><a14:m><m:oMathPara xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\"><m:oMathParaPr><m:jc m:val=\"centerGroup\"/></m:oMathParaPr>{omml}</m:oMathPara></a14:m><a:endParaRPr lang=\"en-US\" sz=\"{sz}\"/></a:p>"
            );
            let _ = write!(
                s,
                "<mc:AlternateContent xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\"><mc:Choice xmlns:a14=\"http://schemas.microsoft.com/office/drawing/2010/main\" Requires=\"a14\">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>",
                sp(&tx(&math), &marker(&tx(&math))),
                sp(&tx(&plain), &marker(&tx(&plain)))
            );
        }
        Err(_) => s.push_str(&sp(&tx(&plain), &marker(&tx(&plain)))),
    }
}

/// The paragraphs of an equation's plain text, one per line of `text`, each a run with `rpr` as
/// its properties: the fallback body, and the body of an equation outside the Office Math subset.
fn plain_paragraphs(text: &str, rpr: &str) -> String {
    text.split('\n').map(|line| format!("<a:p><a:pPr algn=\"ctr\"/><a:r>{rpr}<a:t>{}</a:t></a:r></a:p>", esc(line))).collect()
}

/// The text of a text body: the runs of each paragraph, paragraphs joined with newlines.
fn body_text(tb: &El) -> String {
    tb.kids("p").map(|p| p.kids("r").filter_map(|r| r.child("t")).map(|t| t.text.as_str()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

/// A rejected Office Math box still needs its visible run text. Do not reinterpret
/// that text as LaTeX. Keep traversal and output bounded independently of conversion.
fn math_body_text(tb: &El) -> String {
    const MAX_BYTES: usize = 1 << 20;
    fn append(text: &str, out: &mut String) {
        let mut end = text.len().min(MAX_BYTES.saturating_sub(out.len()));
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        out.push_str(&text[..end]);
    }
    fn visit(e: &El, depth: usize, left: &mut usize, out: &mut String) {
        if depth > 64 || *left == 0 || out.len() >= MAX_BYTES {
            return;
        }
        *left -= 1;
        if e.name.ends_with("Pr") {
            return;
        }
        if e.name == "t" {
            append(&e.text, out);
        } else if e.name == "AlternateContent" {
            // Choice and fallback describe the same content, not two text runs.
            if let Some(c) = e.child("Fallback").or_else(|| e.child("Choice")) {
                visit(c, depth + 1, left, out);
            }
        } else {
            for child in &e.children {
                visit(child, depth + 1, left, out);
                if *left == 0 || out.len() >= MAX_BYTES {
                    break;
                }
            }
        }
    }
    let mut out = String::new();
    let mut left = 100_000;
    for (i, p) in tb.kids("p").enumerate() {
        if left == 0 || out.len() >= MAX_BYTES {
            break;
        }
        if i > 0 {
            append("\n", &mut out);
        }
        visit(p, 0, &mut left, &mut out);
    }
    out
}

/// Whether a body without Office Math still holds the text the writer wrote for `src`: the same
/// paragraphs, read back through the same parser. The marker joins run text across paragraphs,
/// so a paragraph split that changes no character passes it; the text does not.
fn holds_written_text(tb: &El, src: &str) -> bool {
    let written = format!("<xdr:txBody xmlns:xdr=\"x\" xmlns:a=\"a\">{}</xdr:txBody>", plain_paragraphs(src, ""));
    crate::xml::parse(written.as_bytes()).is_ok_and(|w| body_text(&w) == body_text(tb))
}

fn to_placement(sheet: &Sheet, a: &Anchor) -> CraftPlacement {
    let px = |v: f64| ((v.max(0.0)) * EMU_PER_PX).round() as i64;
    let x1 = sheet.col_left(a.cell.col) + a.dx as f64;
    let y1 = sheet.row_top(a.cell.row) + a.dy as f64;
    let x2 = x1 + a.width.max(0.0) as f64;
    let y2 = y1 + a.height.max(0.0) as f64;
    let tc = sheet.col_at(x2);
    let tr = sheet.row_at(y2);
    let tdx = x2 - sheet.col_left(tc);
    let tdy = y2 - sheet.row_top(tr);
    let edit_as = match a.mode {
        AnchorMode::MoveAndSize => None,
        AnchorMode::MoveOnly => Some("oneCell".to_string()),
        AnchorMode::Absolute => Some("absolute".to_string()),
    };
    CraftPlacement::TwoCell {
        from: CraftMarker { col: a.cell.col, col_off: px(a.dx as f64), row: a.cell.row, row_off: px(a.dy as f64) },
        to: CraftMarker { col: tc, col_off: px(tdx), row: tr, row_off: px(tdy) },
        edit_as,
    }
}

fn parse_craft_fragment(xml: &str) -> Option<craft_xml::El> {
    let opts = craft_xml::Options::default().with_limits(craft_xml::Limits {
        max_input: crate::package::MAX_PART as usize,
        max_depth: crate::xml::MAX_DEPTH,
        max_nodes: crate::xml::MAX_XML_NODES,
        ..craft_xml::Limits::DEFAULT
    });
    opts.parse(xml.as_bytes()).ok()
}

fn xfrm(a: &Anchor, tag: &str) -> String {
    xfrm_rot(a, tag, 0.0)
}

/// `deg` is written as `rot`, in 60000ths of a degree.
fn xfrm_rot(a: &Anchor, tag: &str, deg: f32) -> String {
    let e = |v: f32| ((v.max(0.0) as f64) * EMU_PER_PX).round() as i64;
    let rot = (f64::from(deg.rem_euclid(360.0)) * 60000.0).round() as i64;
    let rot = if rot > 0 { format!(" rot=\"{rot}\"") } else { String::new() };
    format!("<{tag}{rot}><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{}\" cy=\"{}\"/></{tag}>", e(a.width), e(a.height))
}

pub fn srgb(c: &Color, theme: &Theme) -> Option<String> {
    let [r, g, b] = c.resolve(theme)?;
    Some(format!("<a:srgbClr val=\"{r:02X}{g:02X}{b:02X}\"/>"))
}

/// One drawing object to write.
pub enum Obj<'a> {
    Image(&'a Image, String),
    Chart(&'a gridcraft_model::Chart, String),
    Shape(&'a Shape),
}

/// The drawing part for a sheet's objects (relationship ids already assigned).
pub fn write_drawing(sheet: &Sheet, theme: &Theme, objs: &[Obj<'_>]) -> String {
    let mut items = Vec::with_capacity(objs.len());
    for (i, o) in objs.iter().enumerate() {
        let id = i + 2;
        match o {
            Obj::Image(img, rid) => {
                let placement = to_placement(sheet, &img.anchor);
                let mut anchor = CraftAnchor::picture(placement, id as u32, &format!("Picture {}", id - 1), &img.alt, rid);
                if let Some(ext) = anchor.object.child_mut("xdr:spPr").and_then(|sp| sp.child_mut("a:xfrm")).and_then(|x| x.child_mut("a:ext")) {
                    let e = |v: f32| ((v.max(0.0) as f64) * EMU_PER_PX).round() as i64;
                    ext.set_attr("cx", e(img.anchor.width));
                    ext.set_attr("cy", e(img.anchor.height));
                }
                items.push(CraftItem::Anchor(anchor));
            }
            Obj::Chart(ch, rid) if crate::chartex::is_chartex(ch.kind) => {
                let placement = to_placement(sheet, &ch.anchor);
                let (prefix, ns) = crate::chartex::requires(ch.kind);
                let xml = format!(
                    "<mc:AlternateContent xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\"><mc:Choice xmlns:{prefix}=\"{ns}\" Requires=\"{prefix}\"><xdr:graphicFrame macro=\"\"><xdr:nvGraphicFramePr><xdr:cNvPr id=\"{id}\" name=\"Chart {}\"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr>{}<a:graphic><a:graphicData uri=\"http://schemas.microsoft.com/office/drawing/2014/chartex\"><cx:chart xmlns:cx=\"http://schemas.microsoft.com/office/drawing/2014/chartex\" r:id=\"{rid}\"/></a:graphicData></a:graphic></xdr:graphicFrame></mc:Choice><mc:Fallback><xdr:sp macro=\"\" textlink=\"\"><xdr:nvSpPr><xdr:cNvPr id=\"0\" name=\"\"/><xdr:cNvSpPr><a:spLocks noTextEdit=\"1\"/></xdr:cNvSpPr></xdr:nvSpPr><xdr:spPr>{}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></xdr:spPr><xdr:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang=\"en-US\"/><a:t>This chart type can't be shown by this application.</a:t></a:r></a:p></xdr:txBody></xdr:sp></mc:Fallback></mc:AlternateContent>",
                    id - 1,
                    xfrm(&ch.anchor, "xdr:xfrm"),
                    xfrm(&ch.anchor, "a:xfrm")
                );
                if let Some(object) = parse_craft_fragment(&xml) {
                    items.push(CraftItem::Anchor(CraftAnchor { placement, object, client_data: None }));
                }
            }
            Obj::Chart(ch, rid) => {
                let placement = to_placement(sheet, &ch.anchor);
                let mut anchor = CraftAnchor::chart(placement, id as u32, &format!("Chart {}", id - 1), rid);
                if let Some(ext) = anchor.object.child_mut("xdr:xfrm").and_then(|x| x.child_mut("a:ext")) {
                    let e = |v: f32| ((v.max(0.0) as f64) * EMU_PER_PX).round() as i64;
                    ext.set_attr("cx", e(ch.anchor.width));
                    ext.set_attr("cy", e(ch.anchor.height));
                }
                items.push(CraftItem::Anchor(anchor));
            }
            Obj::Shape(sh) if sh.kind == ShapeKind::Equation => {
                let placement = to_placement(sheet, &sh.anchor);
                let mut xml = String::new();
                write_equation(&mut xml, sh, id, theme);
                if let Some(object) = parse_craft_fragment(&xml) {
                    items.push(CraftItem::Anchor(CraftAnchor { placement, object, client_data: None }));
                }
            }
            Obj::Shape(sh) => {
                let placement = to_placement(sheet, &sh.anchor);
                let xml = if let Some(xml) = ink::write(sh, theme, id) {
                    xml
                } else {
                    let mut s = String::new();
                    write_plain_shape(&mut s, sh, id, theme);
                    s
                };
                if let Some(object) = parse_craft_fragment(&xml) {
                    items.push(CraftItem::Anchor(CraftAnchor { placement, object, client_data: None }));
                }
            }
        }
    }
    let dr = CraftDrawing { items, attrs: Vec::new() };
    String::from_utf8_lossy(&dr.to_xml()).into_owned()
}

fn write_plain_shape(s: &mut String, sh: &Shape, id: usize, theme: &Theme) {
    let (prst, tx) = match sh.kind {
        ShapeKind::Rectangle => ("rect", false),
        ShapeKind::RoundedRectangle => ("roundRect", false),
        ShapeKind::Ellipse => ("ellipse", false),
        ShapeKind::Triangle => ("triangle", false),
        ShapeKind::Line | ShapeKind::Arrow => ("line", false),
        // Equations are written by `write_equation`; this arm keeps the match whole.
        ShapeKind::TextBox | ShapeKind::WordArt | ShapeKind::Equation => ("rect", true),
        ShapeKind::Icon | ShapeKind::Ink => ("rect", false),
    };
    let _ = write!(
        s,
        "<xdr:sp macro=\"\" textlink=\"\"><xdr:nvSpPr><xdr:cNvPr id=\"{id}\" name=\"Shape {}\"/><xdr:cNvSpPr{}/></xdr:nvSpPr><xdr:spPr>{}<a:prstGeom prst=\"{prst}\"><a:avLst/></a:prstGeom>",
        id - 1,
        if tx { " txBox=\"1\"" } else { "" },
        xfrm_rot(&sh.anchor, "a:xfrm", sh.rotation)
    );
    if let Some(c) = srgb(&sh.fill, theme) {
        let _ = write!(s, "<a:solidFill>{c}</a:solidFill>");
    }
    let line_c = srgb(&sh.line, theme);
    if sh.format.no_line {
        s.push_str("<a:ln><a:noFill/></a:ln>");
    } else if line_c.is_some() || sh.kind == ShapeKind::Arrow {
        s.push_str("<a:ln w=\"9525\">");
        match line_c {
            Some(c) => {
                let _ = write!(s, "<a:solidFill>{c}</a:solidFill>");
            }
            None => s.push_str("<a:solidFill><a:srgbClr val=\"000000\"/></a:solidFill>"),
        }
        if sh.kind == ShapeKind::Arrow {
            s.push_str("<a:tailEnd type=\"triangle\"/>");
        }
        s.push_str("</a:ln>");
    }
    s.push_str("</xdr:spPr>");
    // A WordArt with no text still writes its body, which carries the WordArt marker.
    if !sh.text.is_empty() || sh.kind == ShapeKind::WordArt {
        let f = &sh.format;
        let anchor = match f.anchor {
            Some(ShapeTextAnchor::Top) => " anchor=\"t\"",
            Some(ShapeTextAnchor::Middle) => " anchor=\"ctr\"",
            Some(ShapeTextAnchor::Bottom) => " anchor=\"b\"",
            None if sh.kind == ShapeKind::TextBox => " anchor=\"t\"",
            None => "",
        };
        if sh.kind == ShapeKind::WordArt {
            // Our WordArt marker: a (non-warping) text warp, which the reader maps back to WordArt.
            let _ = write!(
                s,
                "<xdr:txBody><a:bodyPr wrap=\"square\" rtlCol=\"0\"{anchor}><a:prstTxWarp prst=\"textNoShape\"><a:avLst/></a:prstTxWarp></a:bodyPr><a:lstStyle/>"
            );
        } else {
            let _ = write!(s, "<xdr:txBody><a:bodyPr wrap=\"square\" rtlCol=\"0\"{anchor}/><a:lstStyle/>");
        }
        let algn = match f.align {
            None => "",
            Some(ShapeTextAlign::Left) => " algn=\"l\"",
            Some(ShapeTextAlign::Center) => " algn=\"ctr\"",
            Some(ShapeTextAlign::Right) => " algn=\"r\"",
        };
        let rtl = if f.rtl { " rtl=\"1\"" } else { "" };
        let ppr = if algn.is_empty() && rtl.is_empty() { String::new() } else { format!("<a:pPr{algn}{rtl}/>") };
        let (rpr_attrs, rpr_body) = run_props(f, theme);
        for line in sh.text.split('\n') {
            if line.is_empty() {
                let _ = write!(s, "<a:p>{ppr}<a:endParaRPr lang=\"en-US\"{rpr_attrs}/></a:p>");
            } else if rpr_body.is_empty() {
                let _ = write!(s, "<a:p>{ppr}<a:r><a:rPr lang=\"en-US\"{rpr_attrs}/><a:t>{}</a:t></a:r></a:p>", esc(line));
            } else {
                let _ = write!(s, "<a:p>{ppr}<a:r><a:rPr lang=\"en-US\"{rpr_attrs}>{rpr_body}</a:rPr><a:t>{}</a:t></a:r></a:p>", esc(line));
            }
        }
        s.push_str("</xdr:txBody>");
    }
    s.push_str("</xdr:sp>");
}

/// A text run's `a:rPr` attributes (size, bold, italic) and children in schema order (outline,
/// fill, typeface).
fn run_props(f: &ShapeFormat, theme: &Theme) -> (String, String) {
    let mut attrs = String::new();
    if let Some(sz) = f.font_size.filter(|v| v.is_finite()) {
        let _ = write!(attrs, " sz=\"{}\"", (sz.clamp(1.0, 4000.0) * 100.0).round() as u32);
    }
    if f.bold {
        attrs.push_str(" b=\"1\"");
    }
    if f.italic {
        attrs.push_str(" i=\"1\"");
    }
    let mut body = String::new();
    if let Some((c, w)) = &f.text_outline
        && let Some(c) = srgb(c, theme)
    {
        let emu = (w.clamp(0.0, 100.0) * 12700.0).round() as u32;
        let _ = write!(body, "<a:ln w=\"{emu}\"><a:solidFill>{c}</a:solidFill></a:ln>");
    }
    if let Some(c) = f.text_color.as_ref().and_then(|c| srgb(c, theme)) {
        let _ = write!(body, "<a:solidFill>{c}</a:solidFill>");
    }
    if let Some(c) = f.text_shadow.as_ref().and_then(|c| srgb(c, theme)) {
        let _ = write!(
            body,
            "<a:effectLst><a:outerShdw blurRad=\"38100\" dist=\"38100\" dir=\"2700000\" algn=\"tl\" rotWithShape=\"0\">{c}</a:outerShdw></a:effectLst>"
        );
    }
    if let Some(font) = &f.font {
        let _ = write!(body, "<a:latin typeface=\"{}\"/>", esc_attr(font));
    }
    (attrs, body)
}

/// File extension for an image MIME type.
pub fn image_ext(mime: &str, data: &[u8]) -> &'static str {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpeg",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        "image/svg+xml" => "svg",
        "image/x-emf" => "emf",
        "image/x-wmf" => "wmf",
        "image/webp" => "webp",
        _ => {
            if data.starts_with(b"\x89PNG") {
                "png"
            } else if data.starts_with(&[0xFF, 0xD8]) {
                "jpeg"
            } else if data.starts_with(b"GIF8") {
                "gif"
            } else {
                "png"
            }
        }
    }
}

pub fn image_content_type(ext: &str) -> &'static str {
    match ext {
        "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        "emf" => "image/x-emf",
        "wmf" => "image/x-wmf",
        "webp" => "image/webp",
        _ => "image/png",
    }
}

#[cfg(test)]
mod math_fallback_tests {
    use super::*;

    #[test]
    fn fallback_uses_one_alternative_and_ignores_properties() {
        let tb = crate::xml::parse(br#"<txBody><p><rPr><t>hidden</t></rPr><r><t>before </t></r><AlternateContent><Choice><r><t>choice</t></r></Choice><Fallback><r><t>fallback</t></r></Fallback></AlternateContent><r><t> after</t></r></p><p><r><t>next</t></r></p></txBody>"#).unwrap();
        assert_eq!(math_body_text(&tb), "before fallback after\nnext");
    }

    #[test]
    fn fallback_output_cap_keeps_utf8_intact() {
        let xml = format!("<txBody><p><r><t>a{}</t></r></p></txBody>", "λ".repeat(1 << 19));
        let tb = crate::xml::parse(xml.as_bytes()).unwrap();
        let text = math_body_text(&tb);
        assert_eq!(text.len(), (1 << 20) - 1);
        assert!(text.starts_with('a') && text.ends_with('λ'));
    }
}
