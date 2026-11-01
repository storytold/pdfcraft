use super::create::finite;
use super::draw::set_appearance;
use super::lookup::{annot_dict, annot_ref, annotation_flags, annots, drawn_upright, is_fill_sign, page_ref, page_refs};
use super::modify::{fillable, touch, unlocked};
use super::types::{FLAG_LOCKED, LineEnding, Meta, NoteIcon, Rgb};
use super::{AnnotError, Dict, Document, ObjRef, Object, PdfString, appearance};

// ── reading ─────────────────────────────────────────────────────────────────────────────────

/// One comment as the viewer lists it (the same fields and rules as
/// `pdfcraft_render::Annotation`), read straight from the object graph. The engine uses it to
/// refresh the comment list after a comment edit without re-inspecting the whole document.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub page: usize,
    pub index: usize,
    pub subtype: String,
    pub author: Option<String>,
    pub contents: Option<String>,
    /// `/M` as written (a PDF date string).
    pub modified: Option<String>,
    pub name: Option<String>,
    pub in_reply_to: Option<String>,
    /// The page and `/Annots` position `/IRT` points at, by reference (`/NM` need not be unique).
    pub parent: Option<(usize, usize)>,
    /// `/RT /Group`: grouped with the `/IRT` annotation (Replace Text's caret), not a reply to it.
    pub grouped: bool,
    pub rect: [f32; 4],
    pub color: Option<[f32; 3]>,
    pub state: Option<String>,
    pub quads: Vec<[f32; 8]>,
    pub locked: bool,
    pub intent: Option<String>,
    /// A Fill & Sign mark ([`is_fill_sign`]).
    pub fill_sign: bool,
    /// The NoRotate flag (`/F` bit 5), as `pdfcraft_render::Annotation::no_rotate`.
    pub no_rotate: bool,
    /// A line's two ends (`/L`), or the corners of connected lines, a polygon or a cloud
    /// (`/Vertices`), in user space; empty for other comments.
    pub points: Vec<[f32; 2]>,
}

pub(crate) fn text_value(doc: &Document, d: &Dict, key: &[u8]) -> Option<String> {
    let o = doc.resolve(d.get(key)?);
    let s = match &*o {
        Object::String(s) => s.to_text(),
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        _ => return None,
    };
    let s = s.trim_matches('\0').trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Whether an annotation subtype is a comment. Links, form widgets, pop-ups and the non-markup
/// multimedia and print-production annotations (ISO 32000-2 §12.5.6: Screen, Movie, RichMedia,
/// 3D, PrinterMark, TrapNet, Watermark) are not.
pub fn is_comment_subtype(subtype: &str) -> bool {
    !matches!(subtype, "Link" | "Widget" | "Popup" | "Screen" | "Movie" | "RichMedia" | "3D" | "PrinterMark" | "TrapNet" | "Watermark")
}

/// Where a reference to a reference (`5 0 obj 6 0 R endobj`) ends, as `pdfcraft_render::inspect`
/// follows it, within a few steps (a cycle stops there).
fn final_ref(doc: &Document, mut r: ObjRef) -> ObjRef {
    for _ in 0..8 {
        match &*doc.get(r) {
            Object::Ref(next) => r = *next,
            _ => break,
        }
    }
    r
}

/// Every comment (see [`is_comment_subtype`]), ordered by page and then top edge.
pub fn summaries(doc: &Document) -> Vec<Summary> {
    let mut out = Vec::new();
    let Ok(pages) = page_refs(doc) else { return out };
    let lists: Vec<Vec<Object>> = pages.iter().map(|p| annots(doc, *p)).collect();
    // Where each annotation is first listed, so a reply's `/IRT` finds its parent by reference.
    let mut slots: std::collections::HashMap<ObjRef, (usize, usize)> = std::collections::HashMap::new();
    for (page, list) in lists.iter().enumerate() {
        for (index, entry) in list.iter().enumerate() {
            if let Some(r) = entry.as_ref() {
                slots.entry(final_ref(doc, r)).or_insert((page, index));
            }
        }
    }
    for (page, list) in lists.iter().enumerate() {
        for (index, entry) in list.iter().enumerate() {
            let obj = entry.as_ref().map_or_else(|| doc.resolve(entry), |r| doc.get(final_ref(doc, r)));
            let Some(d) = obj.as_dict() else { continue };
            let Some(subtype) = d.name(b"Subtype").map(|s| String::from_utf8_lossy(s).into_owned()) else { continue };
            if !is_comment_subtype(&subtype) {
                continue;
            }
            let nums = |k: &[u8]| -> Vec<f32> {
                d.get(k)
                    .map(|o| doc.resolve(o))
                    .and_then(|o| o.as_array().map(|a| a.iter().map(|x| doc.resolve(x).as_f64().unwrap_or(0.0) as f32).collect()))
                    .unwrap_or_default()
            };
            let r = nums(b"Rect");
            let rect = if r.len() == 4 { [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])] } else { [0.0; 4] };
            let c = nums(b"C");
            let mut color = (c.len() == 3).then(|| [c[0], c[1], c[2]]);
            if subtype == "FreeText"
                && let Some(da) = text_value(doc, d, b"DA")
            {
                let t: Vec<&str> = da.split_whitespace().collect();
                if let Some(i) = t.iter().position(|x| *x == "rg")
                    && i >= 3
                {
                    let f = |k: usize| t[k].parse::<f32>().unwrap_or(0.0);
                    color = Some([f(i - 3), f(i - 2), f(i - 1)]);
                }
            }
            let quads = nums(b"QuadPoints").as_chunks::<8>().0.to_vec();
            // Keep in step with `pdfcraft_render::inspect`.
            let points = match subtype.as_str() {
                "Line" => Some(nums(b"L")).filter(|l| l.len() == 4).map(|l| l.as_chunks::<2>().0.to_vec()).unwrap_or_default(),
                "PolyLine" | "Polygon" => nums(b"Vertices").as_chunks::<2>().0.to_vec(),
                _ => Vec::new(),
            };
            let irt = d.get(b"IRT").map(|o| o.as_ref().map_or_else(|| doc.resolve(o), |r| doc.get(final_ref(doc, r))));
            let in_reply_to = irt.and_then(|o| o.as_dict().cloned()).and_then(|p| text_value(doc, &p, b"NM"));
            let parent = d.get(b"IRT").and_then(Object::as_ref).and_then(|r| slots.get(&final_ref(doc, r)).copied());
            out.push(Summary {
                page,
                index,
                subtype,
                author: text_value(doc, d, b"T"),
                // Acrobat ends lines with CR, which the interface would draw as nothing (keep in
                // step with `pdfcraft_render::inspect`).
                contents: text_value(doc, d, b"Contents").map(|s| s.replace("\r\n", "\n").replace('\r', "\n")),
                modified: text_value(doc, d, b"M"),
                name: text_value(doc, d, b"NM"),
                in_reply_to,
                parent,
                grouped: d.name(b"RT") == Some(b"Group"),
                rect,
                color,
                state: text_value(doc, d, b"State"),
                quads,
                locked: annotation_flags(doc, d) & FLAG_LOCKED != 0,
                intent: d.get(b"IT").and_then(|o| doc.resolve(o).as_name().map(|n| String::from_utf8_lossy(n).into_owned())),
                fill_sign: is_fill_sign(doc, d),
                no_rotate: drawn_upright(doc, d),
                points,
            });
        }
    }
    out.sort_by(|a, b| a.page.cmp(&b.page).then(b.rect[3].total_cmp(&a.rect[3])));
    out
}

/// Change a comment's author (`/T`), subject (`/Subj`) and, for notes, icon (`/Name`, redrawn).
/// `None` leaves a value as it is.
pub fn set_info(
    doc: &mut Document,
    page: usize,
    index: usize,
    author: Option<&str>,
    subject: Option<&str>,
    icon: Option<NoteIcon>,
    meta: &Meta,
) -> Result<(), AnnotError> {
    let (_, r) = annot_ref(doc, page, index)?;
    unlocked(doc, r)?;
    let is_note = annot_dict(doc, r).name(b"Subtype") == Some(b"Text");
    if icon.is_some() && !is_note {
        return Err(AnnotError::Invalid("only sticky notes have an icon".into()));
    }
    doc.update_dict(r, |d| {
        if let Some(a) = author {
            d.set(b"T".to_vec(), PdfString::text(a));
        }
        if let Some(s) = subject {
            d.set(b"Subj".to_vec(), PdfString::text(s));
        }
        if let Some(i) = icon {
            d.set(b"Name".to_vec(), Object::name(i.name()));
        }
        touch(d, meta);
    })?;
    if icon.is_some() {
        set_appearance(doc, r)?;
    }
    Ok(())
}

/// What Comment properties shows for one comment.
#[derive(Clone, Debug, PartialEq)]
pub struct Props {
    pub subtype: String,
    pub author: String,
    pub subject: String,
    pub color: Option<Rgb>,
    pub opacity: f64,
    /// Border / line width, for shapes, lines, drawings and text boxes.
    pub width: Option<f64>,
    /// Notes only.
    pub icon: Option<NoteIcon>,
    /// `/M`, as written.
    pub modified: Option<String>,
    /// The appearance can be redrawn (so colour, opacity and width can change).
    pub restylable: bool,
    /// The Locked flag.
    pub locked: bool,
    /// `/LE`: two names for a line or polyline (`None` when unset), one for a callout.
    pub endings: Option<Vec<LineEnding>>,
    /// Whether the comment has a fill control (see [`fillable`]).
    pub fillable: bool,
    /// The interior colour (`None`: no fill).
    pub fill: Option<Rgb>,
    /// Text boxes, typewriter text and callouts: the `/DA` font size in points.
    pub font_size: Option<f64>,
}

/// The current properties of the comment at `(page, index)`.
pub fn props(doc: &Document, page: usize, index: usize) -> Option<Props> {
    let p = page_ref(doc, page).ok()?;
    let entry = annots(doc, p).get(index).cloned()?;
    let obj = doc.resolve(&entry);
    let d = obj.as_dict()?;
    let subtype = String::from_utf8_lossy(d.name(b"Subtype")?).into_owned();
    let c: Vec<f64> = d.get(b"C").and_then(|o| o.as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).collect())).unwrap_or_default();
    let color = if subtype == "FreeText" { Some(appearance::parse_da(d).0) } else { (c.len() == 3).then(|| [c[0], c[1], c[2]]) };
    let width = matches!(subtype.as_str(), "Square" | "Circle" | "Line" | "Ink" | "Polygon" | "PolyLine" | "FreeText").then(|| {
        if subtype == "FreeText" {
            if !d.contains(b"BS") && !d.contains(b"Border") { 0.0 } else { appearance::border_width(d) }
        } else {
            d.get(b"BS").and_then(|b| b.as_dict()).and_then(|b| b.get(b"W")).and_then(|w| w.as_f64()).unwrap_or(1.0)
        }
    });
    Some(Props {
        author: text_value(doc, d, b"T").unwrap_or_default(),
        subject: text_value(doc, d, b"Subj").unwrap_or_default(),
        color,
        opacity: d.get(b"CA").and_then(|o| o.as_f64()).unwrap_or(1.0),
        width,
        icon: (subtype == "Text").then(|| d.name(b"Name").and_then(|n| NoteIcon::from_name(&String::from_utf8_lossy(n))).unwrap_or(NoteIcon::Note)),
        modified: text_value(doc, d, b"M"),
        restylable: appearance::build(d).is_some(),
        locked: annotation_flags(doc, d) & FLAG_LOCKED != 0,
        fillable: fillable(&subtype),
        fill: fill_of(d),
        font_size: (subtype == "FreeText").then(|| appearance::parse_da(d).1),
        subtype,
        endings: line_endings_of(d),
    })
}

/// A shape’s `/IC` or FreeText `/C` as RGB. Grey and CMYK fills are converted.
fn fill_of(d: &Dict) -> Option<Rgb> {
    let key: &[u8] = if d.name(b"Subtype") == Some(b"FreeText") { b"C" } else { b"IC" };
    let c: Vec<f64> = d.get(key)?.as_array()?.iter().filter_map(|x| x.as_f64()).collect();
    let rgb = match c.as_slice() {
        [g] => [*g; 3],
        [r, g, b] => [*r, *g, *b],
        [c, m, y, k] => [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)],
        _ => return None,
    };
    finite(&rgb).then(|| rgb.map(|x| x.clamp(0.0, 1.0)))
}

/// `/LE` for a line, polyline or callout. Unknown names yield `None` so the control stays hidden.
pub(crate) fn line_endings_of(d: &Dict) -> Option<Vec<LineEnding>> {
    let parse = |name: &[u8]| std::str::from_utf8(name).ok().and_then(LineEnding::parse);
    match d.name(b"Subtype")? {
        b"Line" | b"PolyLine" => match d.get(b"LE") {
            None => Some(vec![LineEnding::None, LineEnding::None]),
            Some(o) => o.as_array().filter(|a| a.len() == 2)?.iter().map(|e| parse(e.as_name()?)).collect(),
        },
        b"FreeText" if d.contains(b"CL") => Some(vec![parse(d.name(b"LE").unwrap_or(b"None"))?]),
        _ => None,
    }
}
