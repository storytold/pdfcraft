use super::create::{
    add_annotation, base_dict, bounds, callout_attach, ending_draws, ending_pair, finite, grow, normalize, num_array, rect_for, rgb,
};
use super::draw::{draw_appearance, redraw_or_restore, set_appearance};
use super::info::{line_endings_of, text_value};
use super::lookup::{annot_dict, annot_ref, annotation_flags, annots, page_ref, set_annots};
use super::types::{
    FLAG_HIDDEN, FLAG_LOCKED, FLAG_NO_ROTATE, FLAG_NO_ZOOM, FLAG_PRINT, LineEnding, Markup, Meta, NewAnnotation, ReviewState, Rgb, Shape, Style,
    StyleChange, TextReplacement,
};
use super::{AnnotError, Dict, Document, HashMap, HashSet, ObjRef, Object, PdfString, appearance};

/// A comment another program drew, whose text the standard fonts can't draw. Redrawing it would
/// replace those characters with `?`, so the edit is refused before the dictionary changes.
pub(crate) fn refuse_foreign_text(doc: &Document, d: &Dict) -> Result<(), AnnotError> {
    let drawn_elsewhere =
        d.get(b"AP").is_some_and(|ap| doc.resolve(ap).as_dict().is_some_and(|ap| ap.contains(b"N"))) && pdfcraft_fonts::undrawn_mark(d).is_none();
    if drawn_elsewhere && let Some((what, c)) = appearance::undrawable_text(d) {
        return Err(AnnotError::Undrawable(format!(
            "PdfCraft can't redraw this comment: the standard fonts can't draw \"{}\" (U+{:04X}) in {what}, which would then show as ? (the comment was left as it is)",
            c.escape_debug(),
            u32::from(c)
        )));
    }
    Ok(())
}

/// Format a number for content streams and DA strings.
pub(crate) fn n(v: f64) -> String {
    let s = format!("{:.3}", if v.abs() < 5e-4 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

// ── edits ───────────────────────────────────────────────────────────────────────────────────

/// A text box's rich text (§12.7.4.3): `/DS` (default style) and `/RC` (XHTML) written from its
/// plain `/Contents`, `/DA` and `/Q`, so viewers that edit rich text show the same thing.
pub(crate) fn rich_text(d: &mut Dict) {
    let text = d.get(b"Contents").and_then(|c| c.as_string()).map(PdfString::to_text).unwrap_or_default();
    let (col, size) = appearance::parse_da(d);
    let hex = format!(
        "#{:02X}{:02X}{:02X}",
        (col[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (col[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (col[2].clamp(0.0, 1.0) * 255.0).round() as u8
    );
    let align = match d.get(b"Q").and_then(Object::as_int) {
        Some(1) => "center",
        Some(2) => "right",
        _ => "left",
    };
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let body: String = text.split('\n').map(|line| format!("<p dir=\"ltr\">{}</p>", esc(line.trim_end_matches('\r')))).collect();
    let style = format!(
        "font-size:{}pt;text-align:{align};color:{hex};font-weight:normal;font-style:normal;font-family:Helvetica;font-stretch:normal",
        n(size)
    );
    let rc = format!(
        "<?xml version=\"1.0\"?><body xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\" xfa:spec=\"2.0.2\" style=\"{style}\">{body}</body>"
    );
    d.set(b"DS".to_vec(), PdfString::text(&format!("font: Helvetica {}pt; text-align:{align}; color:{hex}", n(size))));
    d.set(b"RC".to_vec(), PdfString::text(&rc));
}

pub(crate) fn touch(d: &mut Dict, meta: &Meta) {
    if let Some(date) = &meta.date {
        d.set(b"M".to_vec(), PdfString::literal(date.as_bytes().to_vec()));
    }
}

/// Delete a comment together with its pop-up and its replies (and theirs), as Acrobat does.
pub fn delete_annotation(doc: &mut Document, page: usize, index: usize) -> Result<(), AnnotError> {
    let p = page_ref(doc, page)?;
    let list = annots(doc, p);
    let target = list.get(index).cloned().ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    if let Some(r) = target.as_ref() {
        unlocked(doc, r)?;
    }
    // An inline comment has no reference to collect; the pop-up it names still goes.
    let seed = target.as_ref().or_else(|| target.as_dict().and_then(|d| d.reference(b"Popup")));
    let doomed = with_replies_and_popups(doc, &list, seed);
    let kept: Vec<Object> = list.into_iter().enumerate().filter(|(i, e)| *i != index && !goes_with(e, &doomed)).map(|(_, e)| e).collect();
    Ok(set_annots(doc, p, kept)?)
}

/// Whether the `/Annots` entry `entry` goes with `going` (from [`with_replies_and_popups`]): it is
/// one of them, or it is written inline (a file can, though the standard asks for references) and
/// replies to (`/IRT`) or pops up for (`/Parent`) one of them. An inline entry has no reference
/// for the walk to collect, so it is matched here.
pub fn goes_with(entry: &Object, going: &HashSet<ObjRef>) -> bool {
    match entry.as_ref() {
        Some(r) => going.contains(&r),
        None => entry.as_dict().is_some_and(|d| [d.reference(b"IRT"), d.reference(b"Parent")].into_iter().flatten().any(|r| going.contains(&r))),
    }
}

/// `doomed` and what goes with it, as Acrobat deletes a comment: the entries of `list` (one or
/// more pages' `/Annots`) that reply to (`/IRT`) or pop up for (`/Parent`) anything going, their
/// own replies, and the pop-ups going annotations name (`/Popup`). An inline reply has no
/// reference to collect, but the pop-up it names is. Each annotation is visited once, so a long or
/// cyclic chain costs no more than its length. Filter a list with [`goes_with`], which also
/// matches inline entries.
pub fn with_replies_and_popups(doc: &Document, list: &[Object], doomed: impl IntoIterator<Item = ObjRef>) -> HashSet<ObjRef> {
    let mut hanging: HashMap<ObjRef, Vec<ObjRef>> = HashMap::new();
    for e in list {
        let obj = doc.resolve(e);
        let Some(d) = obj.as_dict() else { continue };
        // What goes with the annotation this entry hangs off: the entry, or for an inline one the
        // pop-up it names.
        let Some(goes) = e.as_ref().or_else(|| d.reference(b"Popup")) else { continue };
        for on in [d.reference(b"IRT"), d.reference(b"Parent")].into_iter().flatten() {
            hanging.entry(on).or_default().push(goes);
        }
    }
    let mut going = HashSet::new();
    let mut todo: Vec<ObjRef> = doomed.into_iter().collect();
    while let Some(r) = todo.pop() {
        if !going.insert(r) {
            continue;
        }
        todo.extend(hanging.get(&r).into_iter().flatten());
        todo.extend(doc.get(r).as_dict().and_then(|d| d.reference(b"Popup")));
    }
    going
}

/// Change a comment's text. Text boxes are redrawn to show it, and their rectangle follows the
/// new text: the wrap width and top edge stay, the height fits the wrapped lines.
pub fn set_contents(doc: &mut Document, page: usize, index: usize, text: &str, meta: &Meta) -> Result<(), AnnotError> {
    let (_, r) = annot_ref(doc, page, index)?;
    let free_text = annot_dict(doc, r).name(b"Subtype") == Some(b"FreeText");
    doc.update_dict(r, |d| {
        d.set(b"Contents".to_vec(), PdfString::text(text));
        // The rich-text version follows the new plain text (text boxes), or would contradict it.
        if free_text {
            rich_text(d);
        } else {
            d.remove(b"RC");
        }
        touch(d, meta);
    })?;
    if free_text {
        // The box grows and shrinks with the text instead of clipping it. A locked box keeps its
        // text editable (as in Acrobat), so the re-fit ignores the lock. New text is drawn even
        // where the standard fonts can't draw some of it (marked and reported, see
        // `draw_appearance`).
        match fitted_box(doc, r, text) {
            Some(rect) => apply_text_box(doc, r, rect, meta, true)?,
            None => draw_appearance(doc, r)?,
        }
    }
    Ok(())
}

/// The rectangle a FreeText annotation's text box needs for `text`: the current wrap width and
/// top edge stay and the height fits the wrapped lines, as at creation. `None` keeps the old
/// rectangle (the box can't be measured, so only the appearance is redrawn).
fn fitted_box(doc: &Document, r: ObjRef, text: &str) -> Option<[f64; 4]> {
    let d = annot_dict(doc, r);
    let nums = |key: &[u8]| -> Option<Vec<f64>> { d.get(key)?.as_array()?.iter().map(|o| o.as_f64()).collect() };
    let (_, size) = appearance::parse_da(&d);
    let pad = 2.0 + border_width_of(&d);
    let rect = nums(b"Rect").filter(|v| v.len() == 4 && v.iter().all(|x| x.is_finite()))?;
    let rect = [rect[0], rect[1], rect[2], rect[3]];
    // A callout re-fits its text box (`/Rect` inset by `/RD`); the leader line keeps its place.
    let tb = if d.contains(b"CL") {
        let rd = nums(b"RD").filter(|v| v.len() == 4 && v.iter().all(|x| *x >= 0.0))?;
        [rect[0] + rd[0], rect[1] + rd[1], rect[2] - rd[2], rect[3] - rd[3]]
    } else {
        rect
    };
    let (x0, top) = (tb[0].min(tb[2]), tb[1].max(tb[3]));
    let w = tb[2] - tb[0];
    let w = if w.is_finite() && w >= 1.0 {
        w
    } else {
        // No usable old width: fall back to the creation-time width (at most 300 pt).
        let longest = text.lines().map(|l| appearance::text_width(l, size)).fold(0.0, f64::max);
        (longest + 2.0 * pad + 4.0).clamp(40.0, 300.0)
    };
    let lines = appearance::wrap(text, size, (w - 2.0 * pad).max(1.0)).len().max(1) as f64;
    Some([x0, top - lines * size * 1.2 - 2.0 * pad - 2.0, x0 + w, top])
}

/// Reply to a comment; returns the reply's index in the page's `/Annots`.
///
/// A reply is a text annotation with `/IRT` pointing at its parent (§12.5.6.2). It gets the
/// parent's rectangle and an empty appearance, so it shows in comment lists but never paints a
/// second icon on the page.
pub fn add_reply(doc: &mut Document, page: usize, index: usize, text: &str, author: &str, meta: &Meta) -> Result<usize, AnnotError> {
    reply(doc, page, index, text, author, meta, None)
}

/// Set a comment's review status (Acrobat: "Set status ▸ Accepted"…). Like Acrobat this adds a
/// state reply (`/State`, `/StateModel /Review`) by `author`; the latest one wins.
pub fn set_review_state(doc: &mut Document, page: usize, index: usize, state: ReviewState, author: &str, meta: &Meta) -> Result<usize, AnnotError> {
    let text = format!("{} set by {}", state.name(), if author.is_empty() { "unknown" } else { author });
    reply(doc, page, index, &text, author, meta, Some(state))
}

fn reply(
    doc: &mut Document,
    page: usize,
    index: usize,
    text: &str,
    author: &str,
    meta: &Meta,
    state: Option<ReviewState>,
) -> Result<usize, AnnotError> {
    let (p, parent) = annot_ref(doc, page, index)?;
    let pd = annot_dict(doc, parent);
    if pd.name(b"Subtype") == Some(b"Popup") {
        return Err(AnnotError::Invalid("pop-ups can't be replied to".into()));
    }
    let rect = pd.get(b"Rect").cloned().unwrap_or_else(|| num_array(&[0.0, 0.0, 0.0, 0.0]));
    let mut d = base_dict("Text", [0.0; 4], p, text, author, meta);
    d.set(b"Rect".to_vec(), rect);
    d.set(b"IRT".to_vec(), Object::Ref(parent));
    d.set(b"F".to_vec(), Object::Int(FLAG_PRINT | FLAG_NO_ZOOM | FLAG_NO_ROTATE));
    d.set(b"Name".to_vec(), Object::name("Comment"));
    if let Some(c) = pd.get(b"C") {
        d.set(b"C".to_vec(), c.clone());
    }
    if let Some(s) = state {
        d.set(b"State".to_vec(), PdfString::text(s.name()));
        d.set(b"StateModel".to_vec(), PdfString::text("Review"));
        d.set(b"Subj".to_vec(), PdfString::text("Status"));
    }
    // An empty form: nothing is drawn for the reply itself.
    let mut fd = Dict::new();
    fd.set(b"Type".to_vec(), Object::name("XObject"));
    fd.set(b"Subtype".to_vec(), Object::name("Form"));
    fd.set(b"BBox".to_vec(), num_array(&[0.0, 0.0, 0.0, 0.0]));
    let ap = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(fd, Vec::new())));
    let mut apd = Dict::new();
    apd.set(b"N".to_vec(), Object::Ref(ap));
    d.set(b"AP".to_vec(), Object::Dict(apd));
    let r = doc.add(Object::Dict(d));
    let mut list = annots(doc, p);
    list.push(Object::Ref(r));
    let i = list.len() - 1;
    set_annots(doc, p, list)?;
    Ok(i)
}

fn flags(doc: &Document, r: ObjRef) -> i64 {
    doc.get(r).as_dict().map(|d| annotation_flags(doc, d)).unwrap_or(0)
}

/// Refuse to change a locked comment (Acrobat: Properties ▸ Locked).
pub(crate) fn unlocked(doc: &Document, r: ObjRef) -> Result<(), AnnotError> {
    if flags(doc, r) & FLAG_LOCKED != 0 { Err(AnnotError::Invalid("the comment is locked".into())) } else { Ok(()) }
}

/// Lock or unlock a comment (the Locked flag). A locked comment can't be moved, resized,
/// restyled or deleted; its text and replies stay editable, as in Acrobat.
pub fn set_locked(doc: &mut Document, page: usize, index: usize, locked: bool) -> Result<(), AnnotError> {
    let (_, r) = annot_ref(doc, page, index)?;
    let f = if locked { flags(doc, r) | FLAG_LOCKED } else { flags(doc, r) & !FLAG_LOCKED };
    doc.update_dict(r, |d| d.set(b"F".to_vec(), Object::Int(f)))?;
    Ok(())
}

/// Mark or unmark a comment with a checkmark. Like Acrobat this adds a hidden state reply with
/// `/StateModel /Marked` by `author`; the latest one wins. It's private bookkeeping, not a status.
pub fn set_marked(doc: &mut Document, page: usize, index: usize, marked: bool, author: &str, meta: &Meta) -> Result<usize, AnnotError> {
    let (p, parent) = annot_ref(doc, page, index)?;
    let pd = annot_dict(doc, parent);
    if pd.name(b"Subtype") == Some(b"Popup") {
        return Err(AnnotError::Invalid("pop-ups can't be marked".into()));
    }
    let state = if marked { "Marked" } else { "Unmarked" };
    let text = format!("{state} set by {}", if author.is_empty() { "unknown" } else { author });
    let mut d = base_dict("Text", [0.0; 4], p, &text, author, meta);
    d.set(b"Rect".to_vec(), pd.get(b"Rect").cloned().unwrap_or_else(|| num_array(&[0.0; 4])));
    d.set(b"IRT".to_vec(), Object::Ref(parent));
    d.set(b"F".to_vec(), Object::Int(FLAG_HIDDEN | FLAG_PRINT | FLAG_NO_ZOOM | FLAG_NO_ROTATE));
    d.set(b"State".to_vec(), PdfString::text(state));
    d.set(b"StateModel".to_vec(), PdfString::text("Marked"));
    let r = doc.add(Object::Dict(d));
    let mut list = annots(doc, p);
    list.push(Object::Ref(r));
    let i = list.len() - 1;
    set_annots(doc, p, list)?;
    Ok(i)
}

/// Move a comment (and its pop-up) by `(dx, dy)` points. The appearance moves with `/Rect`.
pub fn move_annotation(doc: &mut Document, page: usize, index: usize, dx: f64, dy: f64, meta: &Meta) -> Result<(), AnnotError> {
    if !finite(&[dx, dy]) {
        return Err(AnnotError::Invalid("invalid offset".into()));
    }
    let (_, r) = annot_ref(doc, page, index)?;
    unlocked(doc, r)?;
    let shift = |o: &Object, every: bool| -> Option<Object> {
        let a = o.as_array()?;
        Some(Object::Array(
            a.iter()
                .enumerate()
                .map(|(i, v)| match v.as_f64() {
                    Some(x) if every || i < 4 => Object::Real(x + if i % 2 == 0 { dx } else { dy }),
                    _ => v.clone(),
                })
                .collect(),
        ))
    };
    let popup = annot_dict(doc, r).reference(b"Popup");
    doc.update_dict(r, |d| {
        for (k, every) in [(&b"Rect"[..], false), (b"QuadPoints", true), (b"L", false), (b"CL", true), (b"Vertices", true)] {
            if let Some(v) = d.get(k).and_then(|o| shift(o, every)) {
                d.set(k.to_vec(), v);
            }
        }
        if let Some(Object::Array(list)) = d.get(b"InkList").cloned() {
            let moved = list.iter().map(|s| shift(s, true).unwrap_or_else(|| s.clone())).collect();
            d.set(b"InkList".to_vec(), Object::Array(moved));
        }
        touch(d, meta);
    })?;
    if let Some(p) = popup
        && doc.get(p).as_dict().is_some()
    {
        doc.update_dict(p, |d| {
            if let Some(v) = d.get(b"Rect").and_then(|o| shift(o, false)) {
                d.set(b"Rect".to_vec(), v);
            }
        })?;
    }
    Ok(())
}

/// Resize a rectangle, oval, text box, stamp or drawing to `rect`. Stamps keep their appearance,
/// which PDF viewers scale from its bounding box into the new rectangle; drawings scale their
/// strokes and get a new one.
pub fn set_rect(doc: &mut Document, page: usize, index: usize, rect: [f64; 4], meta: &Meta) -> Result<(), AnnotError> {
    let (_, r) = annot_ref(doc, page, index)?;
    unlocked(doc, r)?;
    let d = annot_dict(doc, r);
    let subtype = String::from_utf8_lossy(d.name(b"Subtype").unwrap_or_default()).into_owned();
    if subtype == "Stamp" {
        let rect = normalize(rect);
        if !finite(&rect) || rect[2] <= rect[0] || rect[3] <= rect[1] {
            return Err(AnnotError::Invalid("invalid rectangle (too small)".into()));
        }
        doc.update_dict(r, |d| {
            d.set(b"Rect".to_vec(), num_array(&rect));
            touch(d, meta);
        })?;
        return Ok(());
    }
    if subtype == "Ink" {
        return resize_ink(doc, r, normalize(rect), meta);
    }
    if !matches!(subtype.as_str(), "Square" | "Circle" | "FreeText") {
        return Err(AnnotError::Invalid(format!("{subtype} comments can't be resized")));
    }
    apply_text_box(doc, r, rect, meta, false)
}

/// At most this many points reshape a comment (a drawn polygon has a handful).
const MAX_POINTS: usize = 100_000;

/// Reshape a line, connected lines, a polygon or a cloud: its points in user space (a line's two
/// ends, the others' corners), as dragging the comment's handles sets them. `/Rect` follows the
/// points with room for the line's width and endings, as when the comment was drawn, and the
/// appearance is redrawn. A measurement keeps the points it measured.
pub fn set_points(doc: &mut Document, page: usize, index: usize, points: &[[f64; 2]], meta: &Meta) -> Result<(), AnnotError> {
    let (_, r) = annot_ref(doc, page, index)?;
    unlocked(doc, r)?;
    let d = annot_dict(doc, r);
    if d.contains(b"Measure") {
        return Err(AnnotError::Invalid("a measurement's points are where it measured".into()));
    }
    if points.len() > MAX_POINTS {
        return Err(AnnotError::Invalid(format!("at most {MAX_POINTS} points")));
    }
    let style = Style { width: border_width_of(&d), ..Style::default() };
    let ends = line_endings_of(&d).unwrap_or_default();
    let (start, end) = (ends.first().copied().unwrap_or(LineEnding::None), ends.get(1).copied().unwrap_or(LineEnding::None));
    let subtype = String::from_utf8_lossy(d.name(b"Subtype").unwrap_or_default()).into_owned();
    let (shape, key): (Shape, &[u8]) = match (subtype.as_str(), points) {
        ("Line", [from, to]) => (Shape::Line { from: *from, to: *to, start, end }, b"L"),
        ("Line", _) => return Err(AnnotError::Invalid("a line has two ends".into())),
        ("PolyLine", _) => (Shape::PolyLine { vertices: points.to_vec(), start, end }, b"Vertices"),
        ("Polygon", _) => {
            let cloud = d.get(b"BE").and_then(|b| b.as_dict()).is_some_and(|b| b.name(b"S") == Some(b"C"));
            (Shape::Polygon { vertices: points.to_vec(), cloud }, b"Vertices")
        }
        (other, _) => return Err(AnnotError::Invalid(format!("{other} comments have no points to move"))),
    };
    // Checks the points too: finite, and far enough apart to draw.
    let rect = rect_for(&shape, &style)?;
    let before = d;
    doc.update_dict(r, |d| {
        d.set(key.to_vec(), num_array(&points.concat()));
        d.set(b"Rect".to_vec(), num_array(&rect));
        touch(d, meta);
    })?;
    redraw_or_restore(doc, r, before)
}

/// Scale a drawing's strokes into `rect`. The margin between the strokes and the old `/Rect` (half
/// the line width and a little more) stays the same on each side, so the line isn't clipped.
fn resize_ink(doc: &mut Document, r: ObjRef, rect: [f64; 4], meta: &Meta) -> Result<(), AnnotError> {
    let d = annot_dict(doc, r);
    let strokes: Vec<Vec<[f64; 2]>> = d
        .get(b"InkList")
        .map(|l| doc.resolve(l))
        .and_then(|l| l.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|s| {
            let v: Vec<f64> = doc.resolve(s).as_array().map(|a| a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect()).unwrap_or_default();
            v.as_chunks::<2>().0.iter().map(|p| [p[0], p[1]]).collect()
        })
        .collect();
    let Some(inner) = bounds(strokes.iter().flatten().copied()) else {
        return Err(AnnotError::Invalid("the drawing has no strokes".into()));
    };
    let old = d.get(b"Rect").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
    let old: Vec<f64> = old.iter().filter_map(|v| doc.resolve(v).as_f64()).collect();
    let old = match old[..] {
        [a, b, c, e] => normalize([a, b, c, e]),
        _ => inner,
    };
    let margin = [inner[0] - old[0], inner[1] - old[1], old[2] - inner[2], old[3] - inner[3]].map(|m| m.max(0.0));
    let target = [rect[0] + margin[0], rect[1] + margin[1], rect[2] - margin[2], rect[3] - margin[3]];
    if !finite(&rect) || target[2] < target[0] || target[3] < target[1] || rect[2] - rect[0] < 1.0 || rect[3] - rect[1] < 1.0 {
        return Err(AnnotError::Invalid("invalid rectangle (too small)".into()));
    }
    // An axis with no extent (a straight stroke) is centred rather than scaled.
    let axis = |v: f64, a0: f64, a1: f64, b0: f64, b1: f64| if a1 > a0 { b0 + (v - a0) / (a1 - a0) * (b1 - b0) } else { (b0 + b1) / 2.0 };
    let scaled: Vec<Vec<[f64; 2]>> = strokes
        .iter()
        .map(|s| {
            s.iter().map(|p| [axis(p[0], inner[0], inner[2], target[0], target[2]), axis(p[1], inner[1], inner[3], target[1], target[3])]).collect()
        })
        .collect();
    let before = annot_dict(doc, r);
    doc.update_dict(r, |d| {
        d.set(b"InkList".to_vec(), Object::Array(scaled.iter().map(|s| num_array(&s.concat())).collect()));
        d.set(b"Rect".to_vec(), num_array(&rect));
        touch(d, meta);
    })?;
    redraw_or_restore(doc, r, before)
}

/// Store `rect` as a FreeText annotation's text box (or a square's/oval's rectangle): a callout's
/// leader line re-attaches and `/Rect` grows to hold it, and the appearance is redrawn
/// (`new_text`: for changed contents, see [`set_appearance`] and `draw_appearance`).
fn apply_text_box(doc: &mut Document, r: ObjRef, rect: [f64; 4], meta: &Meta, new_text: bool) -> Result<(), AnnotError> {
    let rect = normalize(rect);
    if !finite(&rect) || rect[2] - rect[0] < 1.0 || rect[3] - rect[1] < 1.0 {
        return Err(AnnotError::Invalid("invalid rectangle (too small)".into()));
    }
    let d = annot_dict(doc, r);
    if !new_text {
        refuse_foreign_text(doc, &d)?;
    }
    // A callout's rectangle is its text box: the leader line re-attaches and `/Rect` grows to hold it.
    let callout = d.get(b"CL").and_then(|o| o.as_array()).map(|a| a.iter().filter_map(|x| x.as_f64()).collect::<Vec<f64>>()).filter(|l| l.len() == 6);
    let (outer, cl) = match callout {
        Some(mut l) => {
            let attach = callout_attach(rect, [l[2], l[3]]);
            (l[4], l[5]) = (attach[0], attach[1]);
            let pad = border_width_of(&d) / 2.0 + appearance::arrow_size(border_width_of(&d)) + 1.0;
            let lead = grow(bounds([[l[0], l[1]], [l[2], l[3]]].into_iter()).unwrap_or_default(), pad);
            ([rect[0].min(lead[0]), rect[1].min(lead[1]), rect[2].max(lead[2]), rect[3].max(lead[3])], Some(l))
        }
        None => (rect, None),
    };
    doc.update_dict(r, |d| {
        d.set(b"Rect".to_vec(), num_array(&outer));
        match &cl {
            Some(l) => {
                d.set(b"CL".to_vec(), num_array(l));
                d.set(b"RD".to_vec(), num_array(&[rect[0] - outer[0], rect[1] - outer[1], outer[2] - rect[2], outer[3] - rect[3]]));
            }
            None => {
                d.remove(b"RD");
            }
        }
        touch(d, meta);
    })?;
    let before = d;
    if new_text {
        if let Err(e) = draw_appearance(doc, r) {
            doc.set(r, Object::Dict(before));
            return Err(e);
        }
        Ok(())
    } else {
        redraw_or_restore(doc, r, before)
    }
}

fn border_width_of(d: &Dict) -> f64 {
    d.get(b"BS").and_then(|b| b.as_dict()).and_then(|b| b.get(b"W")).and_then(|w| w.as_f64()).unwrap_or(1.0).max(0.0)
}

/// Change a comment's colour, opacity, line width and/or line endings, and redraw it.
pub fn set_style(doc: &mut Document, page: usize, index: usize, change: StyleChange, meta: &Meta) -> Result<(), AnnotError> {
    let StyleChange { color, opacity, width, endings } = change;
    let (_, r) = annot_ref(doc, page, index)?;
    unlocked(doc, r)?;
    let d = annot_dict(doc, r);
    let subtype = String::from_utf8_lossy(d.name(b"Subtype").unwrap_or_default()).into_owned();
    // Check before changing anything: a stale appearance would contradict the new style.
    if appearance::build(&d).is_none() {
        return Err(AnnotError::Unsupported(subtype.clone()));
    }
    if color.is_some_and(|c| !finite(&c)) || opacity.is_some_and(|o| !o.is_finite()) || width.is_some_and(|w| !w.is_finite()) {
        return Err(AnnotError::Invalid("invalid style".into()));
    }
    if let Some(ends) = &endings {
        let callout = subtype == "FreeText" && d.contains(b"CL");
        let pair = matches!(subtype.as_str(), "Line" | "PolyLine");
        if (callout && ends.len() != 1) || (pair && ends.len() != 2) || (!callout && !pair) {
            return Err(AnnotError::Invalid("this comment has no line endings to change".into()));
        }
    }
    refuse_foreign_text(doc, &d)?;
    let free_text = subtype == "FreeText";
    let before = d;
    doc.update_dict(r, |d| {
        if let Some(c) = color {
            if free_text {
                let (_, size) = appearance::parse_da(d);
                let [r, g, b] = c.map(|x| x.clamp(0.0, 1.0));
                d.set(b"DA".to_vec(), PdfString::literal(format!("{} {} {} rg /Helv {} Tf", n(r), n(g), n(b), n(size)).into_bytes()));
                rich_text(d);
            } else {
                d.set(b"C".to_vec(), rgb(c));
            }
        }
        if let Some(o) = opacity {
            let o = o.clamp(0.0, 1.0);
            if o < 1.0 {
                d.set(b"CA".to_vec(), Object::Real(o));
            } else {
                d.remove(b"CA");
            }
        }
        if let Some(w) = width {
            let mut bs = d.get(b"BS").and_then(|b| b.as_dict()).cloned().unwrap_or_default();
            bs.set(b"W".to_vec(), Object::Real(w.max(0.0)));
            d.set(b"BS".to_vec(), Object::Dict(bs));
            d.remove(b"Border");
        }
        if let Some(ends) = endings {
            if ends.len() == 1 {
                if let Some(ending) = ends.first() {
                    d.set(b"LE".to_vec(), Object::name(ending.name()));
                }
            } else if let (Some(start), Some(end)) = (ends.first(), ends.get(1)) {
                d.set(b"LE".to_vec(), ending_pair(*start, *end));
            }
        }
        ensure_ending_room(d);
        touch(d, meta);
    })?;
    redraw_or_restore(doc, r, before)
}

/// Change the font size of a text box, typewriter text or callout (`/DA` keeps its colour; the
/// rich-text strings follow) and redraw it. The box grows to hold the wrapped text at the new
/// size and never shrinks, so a box the user drew large stays large. Sizes are 1 to 400 pt.
pub fn set_font_size(doc: &mut Document, page: usize, index: usize, size: f64, meta: &Meta) -> Result<(), AnnotError> {
    let (_, r) = annot_ref(doc, page, index)?;
    unlocked(doc, r)?;
    let d = annot_dict(doc, r);
    if d.name(b"Subtype") != Some(b"FreeText") {
        return Err(AnnotError::Invalid("only text boxes, typewriter text and callouts have a font size".into()));
    }
    if !size.is_finite() || !(1.0..=400.0).contains(&size) {
        return Err(AnnotError::Invalid("font size must be 1\u{2013}400 pt".into()));
    }
    refuse_foreign_text(doc, &d)?;
    let before = d;
    doc.update_dict(r, |d| {
        let (col, _) = appearance::parse_da(d);
        d.set(b"DA".to_vec(), PdfString::literal(format!("{} {} {} rg /Helv {} Tf", n(col[0]), n(col[1]), n(col[2]), n(size)).into_bytes()));
        rich_text(d);
        touch(d, meta);
    })?;
    let text = text_value(doc, &annot_dict(doc, r), b"Contents").unwrap_or_default();
    let result = match fitted_box(doc, r, &text) {
        Some(fit) => {
            let cur = annot_dict(doc, r);
            let nums: Vec<f64> =
                cur.get(b"Rect").and_then(|o| o.as_array()).map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default();
            // Keep the top and sides; only the bottom edge moves, and only downwards.
            match four(&nums).map(normalize) {
                Some(old) if !cur.contains(b"CL") => apply_text_box(doc, r, [fit[0], fit[1].min(old[1]), fit[2], fit[3]], meta, false),
                _ => apply_text_box(doc, r, fit, meta, false),
            }
        }
        None => redraw_or_restore(doc, r, before.clone()),
    };
    if result.is_err() {
        doc.set(r, Object::Dict(before));
    }
    result
}

/// Whether a comment has a fill: `/IC` for shapes, `/C` for FreeText (§12.5.6).
pub fn fillable(subtype: &str) -> bool {
    matches!(subtype, "Square" | "Circle" | "Polygon" | "FreeText")
}

/// Fill a shape or FreeText comment with `fill`, or remove its fill (`None`), and
/// redraw its appearance (Acrobat: Properties ▸ Fill Color).
pub fn set_fill(doc: &mut Document, page: usize, index: usize, fill: Option<Rgb>, meta: &Meta) -> Result<(), AnnotError> {
    let (_, r) = annot_ref(doc, page, index)?;
    unlocked(doc, r)?;
    let d = annot_dict(doc, r);
    let subtype = String::from_utf8_lossy(d.name(b"Subtype").unwrap_or_default()).into_owned();
    if !fillable(&subtype) {
        return Err(AnnotError::Invalid("only rectangles, ovals, polygons and text boxes can be filled".into()));
    }
    // Check before changing anything: a stale appearance would contradict the new fill.
    if appearance::build(&d).is_none() {
        return Err(AnnotError::Unsupported(subtype));
    }
    if fill.is_some_and(|c| !finite(&c)) {
        return Err(AnnotError::Invalid("invalid fill colour".into()));
    }
    refuse_foreign_text(doc, &d)?;
    let before = d;
    doc.update_dict(r, |d| {
        let key: &[u8] = if subtype == "FreeText" { b"C" } else { b"IC" };
        match fill {
            Some(c) => d.set(key.to_vec(), rgb(c)),
            None => {
                d.remove(key);
            }
        }
        touch(d, meta);
    })?;
    redraw_or_restore(doc, r, before)
}

/// Grow a line or polyline's rectangle so a non-`None` ending is not clipped. Idempotent.
fn ensure_ending_room(d: &mut Dict) {
    let subtype = d.name(b"Subtype").unwrap_or_default();
    if !matches!(subtype, b"Line" | b"PolyLine") {
        return;
    }
    if !line_endings_of(d).is_some_and(|ends| ends.iter().any(|e| ending_draws(*e))) {
        return;
    }
    let key: &[u8] = if subtype == b"Line" { b"L" } else { b"Vertices" };
    let Some(pts) = pair_points(d, key) else { return };
    let Some(bounds) = bounds(pts.iter().copied()) else { return };
    let w = border_width_of(d);
    let need = grow(bounds, w / 2.0 + appearance::arrow_size(w) + 1.0);
    let vals: Vec<f64> = d.get(b"Rect").and_then(|o| o.as_array()).map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default();
    let Some(rect) = four(&vals) else { return };
    let union = [rect[0].min(need[0]), rect[1].min(need[1]), rect[2].max(need[2]), rect[3].max(need[3])];
    if union != rect {
        d.set(b"Rect".to_vec(), num_array(&union));
    }
}

fn four(v: &[f64]) -> Option<[f64; 4]> {
    Some([*v.first()?, *v.get(1)?, *v.get(2)?, *v.get(3)?])
}

fn pair_points(d: &Dict, key: &[u8]) -> Option<Vec<[f64; 2]>> {
    let v: Vec<f64> = d.get(key)?.as_array()?.iter().filter_map(|o| o.as_f64()).collect();
    if v.len() < 4 || !v.len().is_multiple_of(2) || v.iter().any(|x| !x.is_finite()) {
        return None;
    }
    Some(v.as_chunks::<2>().0.to_vec())
}

/// Replace Text (Acrobat's proposal): strike out `quads` and add a caret at the end of the
/// struck text holding `replacement`, grouped with the strikeout (`/IRT` + `/RT /Group`) so they
/// move, list and delete as one. Returns the strikeout's index.
///
/// The two annotations need their own `/NM`: XFDF names a reply by the `/NM` of what it replies
/// to, so one id for both writes `inreplyto` equal to the caret's own `name`, and a reader that
/// resolves names (Acrobat, and our own importer) loses the pair. `caret_meta` is a second id
/// from the caller, so the edit stays deterministic.
pub fn add_text_replacement(doc: &mut Document, replacement: TextReplacement<'_>) -> Result<usize, AnnotError> {
    let TextReplacement { page, quads, replacement, author, strike, caret, strike_meta: meta, caret_meta } = replacement;
    let last = *quads.last().ok_or_else(|| AnnotError::Invalid("select the text to replace first".into()))?;
    let s = add_annotation(
        doc,
        &NewAnnotation {
            page,
            shape: Shape::TextMarkup { kind: Markup::StrikeOut, quads: quads.to_vec() },
            style: strike.clone(),
            contents: String::new(),
            author: author.into(),
        },
        meta,
    )?;
    let (_, sr) = annot_ref(doc, page, s)?;
    doc.update_dict(sr, |d| d.set(b"IT".to_vec(), Object::name("StrikeOutTextEdit")))?;
    // The caret sits on the baseline at the end of the last line, its point at the text.
    let xs = [last[0], last[2], last[4], last[6]];
    let ys = [last[1], last[3], last[5], last[7]];
    let x = xs.iter().copied().fold(f64::MIN, f64::max);
    let (y0, y1) = (ys.iter().copied().fold(f64::MAX, f64::min), ys.iter().copied().fold(f64::MIN, f64::max));
    let h = ((y1 - y0) * 0.6).clamp(4.0, 24.0);
    let rect = [x - h / 2.0, y0 - h * 0.6, x + h / 2.0, y0 + h * 0.4];
    let c = add_annotation(
        doc,
        &NewAnnotation { page, shape: Shape::Caret { rect }, style: caret.clone(), contents: replacement.into(), author: author.into() },
        caret_meta,
    )?;
    let (_, cr) = annot_ref(doc, page, c)?;
    doc.update_dict(cr, |d| {
        d.set(b"IRT".to_vec(), Object::Ref(sr));
        d.set(b"RT".to_vec(), Object::name("Group"));
        d.set(b"Subj".to_vec(), PdfString::text("Replace Text"));
    })?;
    Ok(s)
}

/// The Eraser (Draw tools): remove the parts of drawing `(page, index)` within `radius` points
/// of `path`, splitting strokes where they are cut. A drawing with nothing left is deleted.
/// Returns whether anything was erased.
pub fn erase_ink(doc: &mut Document, page: usize, index: usize, path: &[[f64; 2]], radius: f64, meta: &Meta) -> Result<bool, AnnotError> {
    let (_, r) = annot_ref(doc, page, index)?;
    unlocked(doc, r)?;
    let d = annot_dict(doc, r);
    if d.name(b"Subtype") != Some(b"Ink") {
        return Err(AnnotError::Invalid("only drawings can be erased".into()));
    }
    if path.is_empty() || !path.iter().all(|p| finite(p)) || !radius.is_finite() || radius <= 0.0 {
        return Err(AnnotError::Invalid("invalid eraser path".into()));
    }
    let strokes: Vec<Vec<[f64; 2]>> = d
        .get(b"InkList")
        .map(|l| doc.resolve(l))
        .and_then(|l| l.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|s| {
            let v: Vec<f64> = doc.resolve(s).as_array().map(|a| a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect()).unwrap_or_default();
            v.as_chunks::<2>().0.iter().map(|p| [p[0], p[1]]).collect()
        })
        .collect();
    // Distance from a point to the eraser's path (segments, or a single point).
    let near = |p: [f64; 2]| -> bool {
        if path.len() == 1 {
            return (p[0] - path[0][0]).hypot(p[1] - path[0][1]) <= radius;
        }
        path.windows(2).any(|w| {
            let (a, b) = (w[0], w[1]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let len2 = dx * dx + dy * dy;
            let t = if len2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
            (p[0] - (a[0] + t * dx)).hypot(p[1] - (a[1] + t * dy)) <= radius
        })
    };
    let mut kept: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut erased = false;
    for s in &strokes {
        // Densify so long segments are cut where the eraser crosses them.
        let mut pts: Vec<[f64; 2]> = Vec::new();
        for w in s.windows(2) {
            let n = ((w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]) / (radius / 2.0)).ceil().clamp(1.0, 2000.0) as usize;
            for k in 0..n {
                let t = k as f64 / n as f64;
                pts.push([w[0][0] + (w[1][0] - w[0][0]) * t, w[0][1] + (w[1][1] - w[0][1]) * t]);
            }
        }
        if let Some(last) = s.last() {
            pts.push(*last);
        }
        let mut run: Vec<[f64; 2]> = Vec::new();
        for p in pts {
            if near(p) {
                erased = true;
                if run.len() >= 2 {
                    kept.push(std::mem::take(&mut run));
                }
                run.clear();
            } else {
                run.push(p);
            }
        }
        if run.len() >= 2 {
            kept.push(run);
        }
    }
    if !erased {
        return Ok(false);
    }
    if kept.is_empty() {
        delete_annotation(doc, page, index)?;
        return Ok(true);
    }
    let width = d.get(b"BS").and_then(|b| b.as_dict()).and_then(|b| b.get(b"W")).and_then(|w| w.as_f64()).unwrap_or(1.0);
    let rect = grow(bounds(kept.iter().flatten().copied()).unwrap_or_default(), width / 2.0 + 1.0);
    doc.update_dict(r, |d| {
        d.set(b"InkList".to_vec(), Object::Array(kept.iter().map(|s| num_array(&s.concat())).collect()));
        d.set(b"Rect".to_vec(), num_array(&rect));
        touch(d, meta);
    })?;
    set_appearance(doc, r)?;
    Ok(true)
}
