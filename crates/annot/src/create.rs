use super::draw::draw_appearance;
use super::lookup::{annot_dict, annot_ref, annots, fill_sign_shape, page_ref, set_annots};
use super::modify::{n, rich_text};
use super::types::{FLAG_NO_ROTATE, FLAG_NO_ZOOM, FLAG_PRINT, LineEnding, Markup, Meta, NOTE_SIZE, NewAnnotation, Rgb, Shape, StampGroup, Style};
use super::*;

pub(crate) fn num_array(v: &[f64]) -> Object {
    Object::Array(v.iter().map(|x| Object::Real(*x)).collect())
}

pub(crate) fn rgb(c: Rgb) -> Object {
    num_array(&c.map(|x| x.clamp(0.0, 1.0)))
}

pub(crate) fn finite(v: &[f64]) -> bool {
    v.iter().all(|x| x.is_finite() && x.abs() < 1e7)
}

pub(crate) fn normalize(r: [f64; 4]) -> [f64; 4] {
    [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]
}

pub(crate) fn bounds(points: impl Iterator<Item = [f64; 2]>) -> Option<[f64; 4]> {
    points.fold(None, |acc, [x, y]| match acc {
        None => Some([x, y, x, y]),
        Some([a, b, c, d]) => Some([a.min(x), b.min(y), c.max(x), d.max(y)]),
    })
}

pub(crate) fn grow(r: [f64; 4], by: f64) -> [f64; 4] {
    [r[0] - by, r[1] - by, r[2] + by, r[3] + by]
}

pub(crate) fn ending_draws(ending: LineEnding) -> bool {
    ending != LineEnding::None
}

pub(crate) fn ending_pair(start: LineEnding, end: LineEnding) -> Object {
    Object::Array(vec![Object::name(start.name()), Object::name(end.name())])
}

/// Validate and compute `/Rect` for a new comment.
pub(crate) fn rect_for(shape: &Shape, style: &Style) -> Result<[f64; 4], AnnotError> {
    let bad = |what: &str| AnnotError::Invalid(format!("invalid {what}"));
    let half = style.width.max(0.0) / 2.0;
    let r = match shape {
        Shape::Note { at, .. } | Shape::Attachment { at, .. } => {
            if !finite(at) {
                return Err(bad("position"));
            }
            [at[0], at[1] - NOTE_SIZE, at[0] + NOTE_SIZE, at[1]]
        }
        Shape::TextMarkup { quads, .. } | Shape::Redact { quads, .. } => {
            if quads.is_empty() || !quads.iter().all(|q| finite(q)) {
                return Err(bad("text area (no quadrilaterals)"));
            }
            bounds(quads.iter().flat_map(|q| q.as_chunks::<2>().0.iter().map(|p| [p[0], p[1]]).collect::<Vec<_>>()))
                .ok_or_else(|| bad("text area"))?
        }
        Shape::Rectangle { rect }
        | Shape::Oval { rect }
        | Shape::TextBox { rect, .. }
        | Shape::Typewriter { rect, .. }
        | Shape::Stamp { rect, .. }
        | Shape::TypedSignature { rect, .. }
        | Shape::Mark { rect, .. } => {
            let r = normalize(*rect);
            if !finite(rect) || r[2] - r[0] < 1.0 || r[3] - r[1] < 1.0 {
                return Err(bad("rectangle (too small)"));
            }
            r
        }
        Shape::CustomStamp { rect, .. } => {
            let r = normalize(*rect);
            // Image signatures may be very thin; a positive PDF appearance box still works.
            if !finite(rect) || r[2] <= r[0] || r[3] <= r[1] {
                return Err(bad("rectangle (empty)"));
            }
            r
        }
        Shape::Line { from, to, start, end } => {
            if !finite(from) || !finite(to) || (from[0] - to[0]).hypot(from[1] - to[1]) < 1.0 {
                return Err(bad("line (too short)"));
            }
            let pad = half + if ending_draws(*start) || ending_draws(*end) { appearance::arrow_size(style.width) } else { 0.0 };
            grow(bounds([*from, *to].into_iter()).unwrap_or_default(), pad + 1.0)
        }
        Shape::Ink { strokes } | Shape::Signature { strokes } => {
            if strokes.iter().all(Vec::is_empty) || !strokes.iter().flatten().all(|p| finite(p)) {
                return Err(bad("drawing (no points)"));
            }
            // Strokes of three or more points are drawn as curves, which stay within their
            // points and control points.
            let controls = strokes.iter().filter(|s| s.len() > 2).flat_map(|s| {
                let pts: Vec<(f64, f64)> = s.iter().map(|p| (p[0], p[1])).collect();
                appearance::smooth_segments(&pts).into_iter().flat_map(|[a, b, _]| [[a.0, a.1], [b.0, b.1]])
            });
            grow(bounds(strokes.iter().flatten().copied().chain(controls)).unwrap_or_default(), half + 1.0)
        }
        Shape::Polygon { vertices, cloud } => {
            let b = bounds(vertices.iter().copied())
                .filter(|b| vertices.len() >= 3 && vertices.iter().all(|p| finite(p)) && b[2] - b[0] >= 1.0 && b[3] - b[1] >= 1.0);
            let b = b.ok_or_else(|| bad("polygon (it needs three points)"))?;
            grow(b, half + 1.0 + if *cloud { 1.5 * appearance::cloud_radius(style.width) } else { 0.0 })
        }
        Shape::PolyLine { vertices, start, end } => {
            let b = bounds(vertices.iter().copied())
                .filter(|b| vertices.len() >= 2 && vertices.iter().all(|p| finite(p)) && (b[2] - b[0]).max(b[3] - b[1]) >= 1.0);
            let extra = if ending_draws(*start) || ending_draws(*end) { appearance::arrow_size(style.width) } else { 0.0 };
            grow(b.ok_or_else(|| bad("connected lines (they need two points)"))?, half + 1.0 + extra)
        }
        Shape::Callout { rect, knee, point, .. } => {
            let r = normalize(*rect);
            if !finite(rect) || !finite(knee) || !finite(point) || r[2] - r[0] < 1.0 || r[3] - r[1] < 1.0 {
                return Err(bad("callout"));
            }
            let pad = half + appearance::arrow_size(style.width) + 1.0;
            let lead = grow(bounds([*knee, *point].into_iter()).unwrap_or_default(), pad);
            [r[0].min(lead[0]), r[1].min(lead[1]), r[2].max(lead[2]), r[3].max(lead[3])]
        }
        Shape::Caret { rect } => {
            let r = normalize(*rect);
            if !finite(rect) || r[2] - r[0] < 1.0 || r[3] - r[1] < 1.0 {
                return Err(bad("caret"));
            }
            r
        }
    };
    Ok(r)
}

pub(crate) fn base_dict(subtype: &str, rect: [f64; 4], page: ObjRef, contents: &str, author: &str, meta: &Meta) -> Dict {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Annot"));
    d.set(b"Subtype".to_vec(), Object::name(subtype));
    d.set(b"Rect".to_vec(), num_array(&rect));
    d.set(b"Contents".to_vec(), PdfString::text(contents));
    if !author.is_empty() {
        d.set(b"T".to_vec(), PdfString::text(author));
    }
    if let Some(date) = &meta.date {
        d.set(b"M".to_vec(), PdfString::literal(date.as_bytes().to_vec()));
        d.set(b"CreationDate".to_vec(), PdfString::literal(date.as_bytes().to_vec()));
    }
    if !meta.id.is_empty() {
        d.set(b"NM".to_vec(), PdfString::text(&meta.id));
    }
    d.set(b"P".to_vec(), Object::Ref(page));
    d.set(b"F".to_vec(), Object::Int(FLAG_PRINT));
    d
}

/// Acrobat's `/Subj` for each tool (shown as the comment type in other viewers).
fn subject(shape: &Shape) -> &'static str {
    match shape {
        Shape::Note { .. } => "Sticky Note",
        Shape::TextMarkup { kind: Markup::Highlight, .. } => "Highlight",
        Shape::TextMarkup { kind: Markup::Underline, .. } => "Underline",
        Shape::TextMarkup { kind: Markup::StrikeOut, .. } => "Strikethrough",
        Shape::TextMarkup { kind: Markup::Squiggly, .. } => "Squiggly",
        Shape::Rectangle { .. } => "Rectangle",
        Shape::Oval { .. } => "Oval",
        Shape::Line { start: LineEnding::None, end: LineEnding::OpenArrow, .. } => "Arrow",
        Shape::Line { .. } => "Line",
        Shape::Ink { .. } => "Pencil",
        Shape::TextBox { .. } => "Text Box",
        Shape::Typewriter { .. } => "Typewriter",
        Shape::Mark { mark, .. } => mark.label(),
        Shape::Signature { .. } => "Signature",
        Shape::Redact { .. } => "Redact",
        Shape::Stamp { stamp, .. } => match stamp.group() {
            StampGroup::Dynamic => "Dynamic stamp",
            StampGroup::SignHere => "Sign Here",
            StampGroup::StandardBusiness => "Stamp",
        },
        Shape::CustomStamp { .. } => "Stamp",
        Shape::TypedSignature { .. } => "Signature",
        Shape::Polygon { cloud: true, .. } => "Cloud",
        Shape::Polygon { .. } => "Polygon",
        Shape::PolyLine { .. } => "Polygonal Line",
        Shape::Callout { .. } => "Callout",
        Shape::Caret { .. } => "Inserted Text",
        Shape::Attachment { .. } => "File Attachment",
    }
}

/// Where a callout's leader line meets its text box: the middle of the side facing `knee`.
pub fn callout_attach(rect: [f64; 4], knee: [f64; 2]) -> [f64; 2] {
    let [x0, y0, x1, y1] = normalize(rect);
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    if knee[0] < x0 {
        [x0, cy]
    } else if knee[0] > x1 {
        [x1, cy]
    } else if knee[1] > y1 {
        [cx, y1]
    } else {
        [cx, y0]
    }
}

/// Add a comment; returns its index in the page's `/Annots`.
pub fn add_annotation(doc: &mut Document, new: &NewAnnotation, meta: &Meta) -> Result<usize, AnnotError> {
    let page = page_ref(doc, new.page)?;
    let style = &new.style;
    if !finite(&style.color) || !style.opacity.is_finite() || !style.width.is_finite() {
        return Err(AnnotError::Invalid("invalid style".into()));
    }
    let rect = rect_for(&new.shape, style)?;
    let mut d = base_dict(new.shape.subtype(), rect, page, &new.contents, &new.author, meta);
    d.set(b"Subj".to_vec(), PdfString::text(subject(&new.shape)));
    let opacity = style.opacity.clamp(0.0, 1.0);
    if opacity < 1.0 {
        d.set(b"CA".to_vec(), Object::Real(opacity));
    }
    let border = |d: &mut Dict| {
        let mut bs = Dict::new();
        bs.set(b"W".to_vec(), Object::Real(style.width.max(0.0)));
        bs.set(b"S".to_vec(), Object::name("S"));
        d.set(b"BS".to_vec(), Object::Dict(bs));
    };
    let mut popup = None;
    match &new.shape {
        Shape::Note { icon, .. } => {
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"Name".to_vec(), Object::name(icon.name()));
            d.set(b"F".to_vec(), Object::Int(FLAG_PRINT | FLAG_NO_ZOOM | FLAG_NO_ROTATE));
            d.set(b"Open".to_vec(), Object::Bool(false));
            popup = Some([rect[2] + 10.0, rect[3] - 120.0, rect[2] + 210.0, rect[3]]);
        }
        Shape::TextMarkup { quads, .. } => {
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"QuadPoints".to_vec(), num_array(&quads.concat()));
        }
        Shape::Redact { quads, overlay, look } => {
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"IC".to_vec(), rgb(style.fill.unwrap_or([0.0, 0.0, 0.0])));
            d.set(b"QuadPoints".to_vec(), num_array(&quads.concat()));
            if !overlay.is_empty() {
                d.set(b"OverlayText".to_vec(), PdfString::text(overlay));
                let [r, g, b] = look.color.map(|x| x.clamp(0.0, 1.0));
                let size = if look.size > 0.0 { look.size.min(400.0) } else { 0.0 };
                d.set(
                    b"DA".to_vec(),
                    PdfString::literal(format!("{} {} {} rg /{} {} Tf", n(r), n(g), n(b), look.font.resource(), n(size)).into_bytes()),
                );
                d.set(b"Q".to_vec(), Object::Int(look.align.min(2) as i64));
                if look.repeat {
                    d.set(b"Repeat".to_vec(), Object::Bool(true));
                }
            }
        }
        Shape::Rectangle { .. } | Shape::Oval { .. } => {
            d.set(b"C".to_vec(), rgb(style.color));
            if let Some(f) = style.fill {
                d.set(b"IC".to_vec(), rgb(f));
            }
            border(&mut d);
        }
        Shape::Line { from, to, start, end } => {
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"L".to_vec(), num_array(&[from[0], from[1], to[0], to[1]]));
            if ending_draws(*start) || ending_draws(*end) {
                d.set(b"LE".to_vec(), ending_pair(*start, *end));
            }
            border(&mut d);
        }
        Shape::Mark { mark, .. } => {
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"Name".to_vec(), Object::name(mark.name()));
            border(&mut d);
            // Drawn upright for the page as displayed, as a stamp is (#2392).
            let turn = pdfcraft_model::pages(doc).get(new.page).map_or(0, |p| p.rotation(doc));
            if matches!(turn, 90 | 180 | 270) {
                d.set(b"PCStampRotate".to_vec(), Object::Int(turn));
            }
        }
        Shape::Stamp { stamp, by, .. } => {
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"Name".to_vec(), Object::name(stamp.name()));
            // Marks the stamp as drawn by PdfCraft: other stamps with standard names keep
            // their own artwork.
            d.set(b"PCStamp".to_vec(), Object::Bool(true));
            if let Some(b) = by {
                d.set(b"PCByLine".to_vec(), PdfString::text(b));
            }
            // The label is drawn for the page as displayed. A later page rotation is a new edit.
            let turn = pdfcraft_model::pages(doc).get(new.page).map_or(0, |p| p.rotation(doc));
            if matches!(turn, 90 | 180 | 270) {
                d.set(b"PCStampRotate".to_vec(), Object::Int(turn));
            }
        }
        Shape::TypedSignature { contours, .. } => {
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"Name".to_vec(), Object::name("PCTypedSignature"));
            let list = contours.iter().filter(|c| c.len() > 2).map(|c| num_array(&c.iter().flat_map(|p| [p[0], p[1]]).collect::<Vec<_>>())).collect();
            d.set(b"PCOutline".to_vec(), Object::Array(list));
        }
        Shape::CustomStamp { name, picture, image, size, .. } => {
            let clean: String = name.chars().filter(char::is_ascii_alphanumeric).collect();
            d.set(b"Name".to_vec(), Object::name(&format!("PCCustom{clean}")));
            d.set(b"PCPicture".to_vec(), Object::Ref(*picture));
            d.set(b"PCPictureImage".to_vec(), Object::Bool(*image));
            d.set(b"PCPictureSize".to_vec(), num_array(&[size.0, size.1]));
            // An image signature's rectangle is in user space; on a turned page its appearance is
            // turned back (see `appearance::build`), so it reads upright as displayed.
            if *image && matches!(clean.as_str(), "Signature" | "Initials") {
                let turn = pdfcraft_model::pages(doc).get(new.page).map_or(0, |p| p.rotation(doc));
                if turn != 0 {
                    d.set(b"PCPictureRotate".to_vec(), Object::Int(turn));
                }
            }
        }
        Shape::Ink { strokes } | Shape::Signature { strokes } => {
            d.set(b"C".to_vec(), rgb(style.color));
            let list = strokes.iter().filter(|s| !s.is_empty()).map(|s| num_array(&s.concat())).collect();
            d.set(b"InkList".to_vec(), Object::Array(list));
            border(&mut d);
        }
        Shape::Polygon { vertices, cloud } => {
            d.set(b"C".to_vec(), rgb(style.color));
            if let Some(f) = style.fill {
                d.set(b"IC".to_vec(), rgb(f));
            }
            d.set(b"Vertices".to_vec(), num_array(&vertices.concat()));
            if *cloud {
                d.set(b"IT".to_vec(), Object::name("PolygonCloud"));
                let mut be = Dict::new();
                be.set(b"S".to_vec(), Object::name("C"));
                be.set(b"I".to_vec(), Object::Int(1));
                d.set(b"BE".to_vec(), Object::Dict(be));
            }
            border(&mut d);
        }
        Shape::PolyLine { vertices, start, end } => {
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"Vertices".to_vec(), num_array(&vertices.concat()));
            if ending_draws(*start) || ending_draws(*end) {
                d.set(b"LE".to_vec(), ending_pair(*start, *end));
            }
            border(&mut d);
        }
        Shape::Caret { .. } => {
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"Sy".to_vec(), Object::name("None"));
        }
        Shape::Attachment { icon, file, data, .. } => {
            if file.trim().is_empty() {
                return Err(AnnotError::Invalid("the attached file needs a name".into()));
            }
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"Name".to_vec(), Object::name(icon.name()));
            d.set(b"F".to_vec(), Object::Int(FLAG_PRINT | FLAG_NO_ZOOM | FLAG_NO_ROTATE));
            // The file: an embedded file stream inside a file specification (§7.11.4).
            let mut params = Dict::new();
            params.set(b"Size".to_vec(), Object::Int(data.len() as i64));
            if let Some(date) = &meta.date {
                params.set(b"ModDate".to_vec(), PdfString::literal(date.as_bytes().to_vec()));
            }
            let mut ef = Dict::new();
            ef.set(b"Type".to_vec(), Object::name("EmbeddedFile"));
            ef.set(b"Params".to_vec(), Object::Dict(params));
            let ef = doc.add(Object::Stream(pdfcraft_cos::Stream::flate(ef, data)));
            let mut efd = Dict::new();
            efd.set(b"F".to_vec(), Object::Ref(ef));
            efd.set(b"UF".to_vec(), Object::Ref(ef));
            let mut fs = Dict::new();
            fs.set(b"Type".to_vec(), Object::name("Filespec"));
            fs.set(b"F".to_vec(), PdfString::text(file.trim()));
            fs.set(b"UF".to_vec(), PdfString::text(file.trim()));
            fs.set(b"EF".to_vec(), Object::Dict(efd));
            d.set(b"FS".to_vec(), Object::Ref(doc.add(Object::Dict(fs))));
            if new.contents.is_empty() {
                d.set(b"Contents".to_vec(), PdfString::text(file.trim()));
            }
        }
        Shape::Callout { rect: tb, knee, point, ending, .. } => {
            let tb = normalize(*tb);
            let attach = callout_attach(tb, *knee);
            d.set(b"IT".to_vec(), Object::name("FreeTextCallout"));
            d.set(b"CL".to_vec(), num_array(&[point[0], point[1], knee[0], knee[1], attach[0], attach[1]]));
            d.set(b"LE".to_vec(), Object::name(ending.name()));
            // The text box inside /Rect (§12.5.6.6 /RD).
            d.set(b"RD".to_vec(), num_array(&[tb[0] - rect[0], tb[1] - rect[1], rect[2] - tb[2], rect[3] - tb[3]]));
        }
        _ => {}
    }
    match &new.shape {
        Shape::TextBox { font_size, .. } | Shape::Typewriter { font_size, .. } | Shape::Callout { font_size, .. } => {
            if matches!(new.shape, Shape::Typewriter { .. }) {
                d.set(b"IT".to_vec(), Object::name("FreeTextTypeWriter"));
            }
            let size = if font_size.is_finite() && *font_size > 0.0 { font_size.clamp(1.0, 400.0) } else { 12.0 };
            let [r, g, b] = style.color.map(|x| x.clamp(0.0, 1.0));
            d.set(b"DA".to_vec(), PdfString::literal(format!("{} {} {} rg /Helv {} Tf", n(r), n(g), n(b), n(size)).into_bytes()));
            d.set(b"Q".to_vec(), Object::Int(0));
            if let Some(f) = style.fill {
                d.set(b"C".to_vec(), rgb(f));
            }
            border(&mut d);
            rich_text(&mut d);
            // The text is laid out for the page as displayed, like a stamp's label.
            if !matches!(new.shape, Shape::Callout { .. }) {
                let turn = pdfcraft_model::pages(doc).get(new.page).map_or(0, |p| p.rotation(doc));
                if matches!(turn, 90 | 180 | 270) {
                    d.set(b"PCTextRotate".to_vec(), Object::Int(turn));
                }
            }
        }
        _ => {}
    }
    if fill_sign_shape(&new.shape) {
        // Survives a later subject edit, so flatten-on-save still finds the mark.
        d.set(b"PCFillSign".to_vec(), Object::Bool(true));
    }
    // A redaction's overlay text becomes page content when it is applied, where `?` would be there
    // for good: refused before anything is added. A comment's own text is stored as typed and
    // drawn with `?` for what the standard fonts can't draw (marked, see `draw_appearance`).
    if d.name(b"Subtype") == Some(b"Redact")
        && let Some((what, c)) = appearance::undrawable_text(&d)
    {
        return Err(AnnotError::Undrawable(pdfcraft_fonts::undrawable_message(what, c)));
    }
    let r = doc.add(Object::Dict(d.clone()));
    draw_appearance(doc, r)?;
    let mut list = annots(doc, page);
    let index = list.len();
    list.push(Object::Ref(r));
    if let Some(pr) = popup {
        let mut p = Dict::new();
        p.set(b"Type".to_vec(), Object::name("Annot"));
        p.set(b"Subtype".to_vec(), Object::name("Popup"));
        p.set(b"Rect".to_vec(), num_array(&pr));
        p.set(b"Parent".to_vec(), Object::Ref(r));
        p.set(b"Open".to_vec(), Object::Bool(false));
        p.set(b"F".to_vec(), Object::Int(FLAG_PRINT | FLAG_NO_ZOOM | FLAG_NO_ROTATE));
        let pref = doc.add(Object::Dict(p));
        doc.update_dict(r, |d| d.set(b"Popup".to_vec(), Object::Ref(pref)))?;
        list.push(Object::Ref(pref));
    }
    set_annots(doc, page, list)?;
    Ok(index)
}

/// Counterrotate a newly placed picture stamp's appearance into displayed-page axes.
/// The picture may be an image or a PDF page. The normal appearance's transformed bounding
/// box is fitted to `/Rect` by PDF viewers.
pub fn orient_image_stamp(doc: &mut Document, page: usize, index: usize, rotation: i64) -> Result<(), AnnotError> {
    let matrix = match rotation {
        90 => [0.0, 1.0, -1.0, 0.0, 0.0, 0.0],
        180 => [-1.0, 0.0, 0.0, -1.0, 0.0, 0.0],
        270 => [0.0, -1.0, 1.0, 0.0, 0.0, 0.0],
        _ => return Ok(()),
    };
    let (_, r) = annot_ref(doc, page, index)?;
    let d = annot_dict(doc, r);
    if d.name(b"Subtype") != Some(b"Stamp") || d.get(b"PCPicture").is_none() {
        return Err(AnnotError::Invalid("only a picture stamp can be oriented".into()));
    }
    let normal = d
        .get(b"AP")
        .map(|ap| doc.resolve(ap))
        .and_then(|ap| ap.as_dict().and_then(|ap| ap.reference(b"N")))
        .ok_or_else(|| AnnotError::Invalid("the stamp has no normal appearance".into()))?;
    let Object::Stream(mut stream) = doc.get(normal).as_ref().clone() else {
        return Err(AnnotError::Invalid("the stamp's normal appearance is not a stream".into()));
    };
    stream.dict.set(b"Matrix".to_vec(), num_array(&matrix));
    doc.set(normal, Object::Stream(stream));
    Ok(())
}
