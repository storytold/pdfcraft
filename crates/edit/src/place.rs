use super::marks::{MarkKind, matrix_number, n};
use super::*;

pub(crate) fn page_list(doc: &Document) -> Vec<pdfcraft_model::Page> {
    pdfcraft_model::pages(doc)
}

pub(crate) fn check(pages: &[usize], count: usize) -> Result<(), EditError> {
    match pages.iter().find(|p| **p >= count) {
        Some(p) => Err(EditError::NoSuchPage(*p)),
        None => Ok(()),
    }
}

/// The page's `/Contents` as a list of stream references (inline content is promoted).
pub(crate) fn contents(doc: &mut Document, page: &pdfcraft_model::Page) -> Result<Vec<Object>, EditError> {
    let obj = doc.get(page.obj);
    let d = obj.as_dict().cloned().unwrap_or_default();
    Ok(match d.get(b"Contents").cloned() {
        None => Vec::new(),
        Some(Object::Ref(r)) => match doc.get(r).as_array() {
            Some(a) => a.clone(),
            None => vec![Object::Ref(r)],
        },
        Some(Object::Array(a)) => a,
        Some(other) => vec![Object::Ref(doc.add(other))],
    })
}

/// Add the font (and an opacity state) to the page's own resources.
pub(crate) fn add_resources(doc: &mut Document, page: &pdfcraft_model::Page, opacity: Option<f64>, content: Option<&[u8]>) -> Result<(), EditError> {
    let mut faces: Vec<(Vec<u8>, &str)> = Vec::new();
    for (name, base) in [(&b"PCHelv"[..], "Helvetica"), (b"PCTimes", "Times-Roman"), (b"PCCour", "Courier")] {
        // Helvetica always (marks use it); the others only when the content names them.
        if name != b"PCHelv" && !content.is_some_and(|c| c.windows(name.len() + 1).any(|w| w[0] == b'/' && &w[1..] == name)) {
            continue;
        }
        faces.push((name.to_vec(), base));
    }
    add_page_resources(doc, page, opacity, &faces)
}

/// The fonts a page draws with: its own resources' or the ones it inherits.
pub(crate) fn page_fonts(doc: &Document, page: &pdfcraft_model::Page) -> Dict {
    let res = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default()
}

/// A name in `fonts` for the standard face `base` that can't take over a font the page already
/// draws with: `preferred` when it is free or already names exactly the dictionary
/// [`add_page_resources`] writes for that face, otherwise the first such `preferred_1`,
/// `preferred_2`… (one of the first `fonts.len() + 1` always is). A font with anything more
/// (`/ToUnicode`, `/Widths`…) is someone else's, even in the same face.
pub(crate) fn free_font_name(doc: &Document, fonts: &Dict, preferred: &str, base: &str) -> String {
    let usable = |name: &str| match fonts.get(name.as_bytes()) {
        None => true,
        Some(font) => doc.resolve(font).as_dict().is_some_and(|d| {
            d.len() == 4
                && d.name(b"Type") == Some(b"Font")
                && d.name(b"Subtype") == Some(b"Type1")
                && d.name(b"BaseFont") == Some(base.as_bytes())
                && d.name(b"Encoding") == Some(b"WinAnsiEncoding")
        }),
    };
    if usable(preferred) {
        return preferred.to_string();
    }
    (1..=fonts.len().saturating_add(1)).map(|k| format!("{preferred}_{k}")).find(|name| usable(name)).unwrap_or_else(|| preferred.to_string())
}

/// Add the standard `faces` (resource name, base font) and an opacity state to the page's own
/// resources.
pub(crate) fn add_page_resources(
    doc: &mut Document,
    page: &pdfcraft_model::Page,
    opacity: Option<f64>,
    faces: &[(Vec<u8>, &str)],
) -> Result<(), EditError> {
    let mut res = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut fonts = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
    for (name, base) in faces {
        let mut font = Dict::new();
        font.set(b"Type".to_vec(), Object::name("Font"));
        font.set(b"Subtype".to_vec(), Object::name("Type1"));
        font.set(b"BaseFont".to_vec(), Object::name(base));
        font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        fonts.set(name.clone(), Object::Dict(font));
    }
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    if let Some(o) = opacity {
        let mut gs = res.get(b"ExtGState").map(|g| doc.resolve(g)).and_then(|g| g.as_dict().cloned()).unwrap_or_default();
        let mut s = Dict::new();
        s.set(b"Type".to_vec(), Object::name("ExtGState"));
        s.set(b"CA".to_vec(), Object::Real(o));
        s.set(b"ca".to_vec(), Object::Real(o));
        let name = format!("PCGS{}", (o * 100.0).round() as i64);
        gs.set(name.into_bytes(), Object::Dict(s));
        res.set(b"ExtGState".to_vec(), Object::Dict(gs));
    }
    // The page gets its own (possibly copied) resources; shared dictionaries stay untouched.
    doc.update_dict(page.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?;
    Ok(())
}

pub(crate) const WRAP_OPEN: &[u8] = b"q %PdfCraft\n";

/// Starts with an end of line, so a comment left open at the end of the content before it
/// can't swallow the `Q`. Older files have the close without it.
pub(crate) const WRAP_CLOSE: &[u8] = b"\nQ %PdfCraft\n";

pub(crate) const OLD_WRAP_CLOSE: &[u8] = b"Q %PdfCraft\n";

/// Whether `bytes` are a wrapper's close: canonical `EMC` closures, then the existing
/// graphics closures. Legacy closes without the leading newline remain valid on their own.
pub(crate) fn is_wrap_close(bytes: &[u8]) -> bool {
    let mut rest = bytes;
    while let Some(shorter) = rest.strip_prefix(b"\nEMC".as_slice()) {
        rest = shorter;
    }
    let legacy = rest.len() == bytes.len();
    loop {
        if rest == WRAP_CLOSE || (legacy && rest == OLD_WRAP_CLOSE) {
            return true;
        }
        match rest.strip_prefix(b"\nQ".as_slice()) {
            Some(shorter) => rest = shorter,
            None => return false,
        }
    }
}

/// A `Wrap`-tagged stream's role from its exact bytes: `Some(true)` for an open, `Some(false)`
/// for a close.
pub(crate) fn wrapper_role(doc: &Document, o: &Object) -> Option<bool> {
    if stream_tag(doc, o).as_deref() != Some("Wrap") {
        return None;
    }
    let Object::Stream(stream) = &*doc.resolve(o) else { return None };
    // Wrappers are a few bytes; one that decodes to more isn't PdfCraft's.
    let bytes = stream.decoded_within(64 * 1024).ok()?;
    if bytes == WRAP_OPEN { Some(true) } else { is_wrap_close(&bytes).then_some(false) }
}

#[cfg(test)]
pub(crate) fn stream_bytes(doc: &Document, o: &Object) -> Option<Vec<u8>> {
    match &*doc.resolve(o) {
        Object::Stream(s) => s.decoded().ok(),
        _ => None,
    }
}

/// The `/PCMark` tag in a content stream's dictionary: a mark kind, or `Wrap` for the q/Q
/// wrapper. Read from the dictionary, so finding marks never decompresses page content.
pub(crate) fn stream_tag(doc: &Document, o: &Object) -> Option<String> {
    let obj = doc.resolve(o);
    obj.as_dict()?.name(b"PCMark").map(|n| String::from_utf8_lossy(n).into_owned())
}

pub(crate) fn tagged(tag: &str) -> Dict {
    let mut d = Dict::new();
    d.set(b"PCMark".to_vec(), Object::name(tag));
    d
}

/// What placing a mark on top needs, from the page's actual operators (not the `/PCMark` tags,
/// which another writer may keep, copy or forge).
pub(crate) enum Isolation {
    /// The content parses cleanly with balanced `q`/`Q` and closed text objects and paths.
    /// `suffix`: the stream from which the content leaves graphics state set (or a comment open)
    /// that a mark on top must not inherit, so it needs a wrapper from there to the end.
    /// `wrappers`: PdfCraft wrapper pairs (open, close) as stream indexes.
    /// `marked`: unfinished marked-content sequences to close before a new mark.
    Clean { suffix: Option<usize>, wrappers: Vec<(usize, usize)>, marked: usize },
    /// The content could not be read that precisely (it doesn't decode, is too large, or has
    /// stray bytes, unbalanced `q`/`Q` or an unfinished text object or path). `unclosed` is
    /// the count of `q` left open at the end, when known. `marked` counts known unfinished
    /// marked-content sequences independently of graphics malformation, or zero when unknown.
    Unknown { unclosed: usize, marked: usize },
    /// Marked content nests past [`ISOLATION_MAX_DEPTH`]: how many sequences a mark on top would
    /// have to close (some may be hidden optional content) is unknown.
    TooDeep,
}

const ISOLATION_MAX_STREAMS: usize = 100_000;

const ISOLATION_MAX_BYTES: usize = 128 * 1024 * 1024;

const ISOLATION_MAX_OPS: usize = 1_000_000;

pub(crate) const ISOLATION_MAX_DEPTH: usize = 10_000;

pub(crate) fn isolation(doc: &Document, list: &[Object]) -> Isolation {
    let unknown = Isolation::Unknown { unclosed: 0, marked: 0 };
    if list.len() > ISOLATION_MAX_STREAMS {
        return unknown;
    }
    let mut remaining = ISOLATION_MAX_BYTES;
    let mut data = Vec::with_capacity(list.len());
    for object in list {
        let resolved = doc.resolve(object);
        let Object::Stream(stream) = &*resolved else { return unknown };
        let Ok(bytes) = stream.decoded_within(remaining) else { return unknown };
        let Some(left) = remaining.checked_sub(bytes.len().saturating_add(1)) else { return unknown };
        remaining = left;
        data.push(bytes);
    }
    let pieces = pdfcraft_content::Pieces::join(&data);
    let parsed = pieces.parse();
    if parsed.skipped != 0 || parsed.ops.len() > ISOLATION_MAX_OPS {
        return unknown;
    }
    let edges = pieces.comment_edges(&parsed);
    let (mut text_open, mut path_open, mut dirty, mut malformed) = (false, false, false, false);
    let mut stack: Vec<usize> = Vec::new();
    let (mut marked, mut marked_known) = (0usize, true);
    let mut split = 0;
    let mut previous_last = None;
    let mut wrappers = Vec::new();
    for op in &parsed.ops {
        let (first, last) = pieces.pieces_of(op);
        // A wrapper can only go in before a whole stream, outside graphics and marked-content
        // scopes, between operators, where no comment would swallow the wrapper's `q`.
        if stack.is_empty()
            && marked_known
            && marked == 0
            && !text_open
            && !path_open
            && !dirty
            && previous_last.is_none_or(|previous| first > previous)
            && !edges.get(first).is_some_and(|edge| edge.0)
        {
            split = first;
        }
        previous_last = Some(last);
        // Match the reader's tolerant operands: BMC uses the last name, BDC the last
        // name/object pair, and EMC ignores surplus operands. Invalid begins are ignored.
        let begin_marked = match op.op.as_slice() {
            b"BMC" => op.operands.last().is_some_and(|o| matches!(o, Object::Name(_))),
            b"BDC" => op.operands.len().checked_sub(2).and_then(|i| op.operands.get(i)).is_some_and(|o| matches!(o, Object::Name(_))),
            _ => false,
        };
        if marked_known {
            if begin_marked {
                if marked < ISOLATION_MAX_DEPTH {
                    marked += 1;
                } else {
                    // Once the budget is exceeded, later EMCs cannot restore a known depth.
                    marked_known = false;
                    marked = 0;
                }
            } else if op.is("EMC") {
                marked = marked.saturating_sub(1);
            }
        }
        match op.op.as_slice() {
            b"BT" if !text_open => text_open = true,
            b"ET" if text_open => text_open = false,
            b"BT" | b"ET" => malformed = true,
            b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re" | b"W" | b"W*" => path_open = true,
            b"n" | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => path_open = false,
            _ => {}
        }
        match op.op.as_slice() {
            b"q" if op.operands.is_empty() && stack.len() < ISOLATION_MAX_DEPTH => stack.push(first),
            b"Q" if op.operands.is_empty() => match stack.pop() {
                Some(open) => {
                    // Only a standalone PdfCraft wrapper pair may later be removed again.
                    let wrap = |i: usize| list.get(i).is_some_and(|o| stream_tag(doc, o).as_deref() == Some("Wrap"));
                    if wrap(open)
                        && wrap(first)
                        && data.get(open).is_some_and(|d| d == WRAP_OPEN)
                        && data.get(first).is_some_and(|d| is_wrap_close(d))
                    {
                        wrappers.push((open, first));
                    }
                }
                None => malformed = true,
            },
            b"q" | b"Q" => malformed = true,
            _ if stack.is_empty() => dirty = true,
            _ => {}
        }
    }
    if !marked_known {
        return Isolation::TooDeep;
    }
    if malformed || text_open || path_open || !stack.is_empty() {
        return Isolation::Unknown { unclosed: if malformed { 0 } else { stack.len() }, marked };
    }
    let comment_open = edges.last().is_some_and(|edge| edge.1);
    Isolation::Clean { suffix: (dirty || comment_open || marked != 0).then_some(split), wrappers, marked }
}

/// Put a mark's content on a page: behind (with the behind-marks at the start) or on top
/// (appended, after wrapping the original content in q/Q).
pub(crate) fn place(doc: &mut Document, page: &pdfcraft_model::Page, kind: MarkKind, content: Vec<u8>, behind: bool) -> Result<(), EditError> {
    place_tagged(doc, page, kind.tag(), content, behind).map(drop)
}

/// Put content on a page, tagged `tag` (see [`place`]). On top, whatever graphics state the
/// content before leaves set (a `cm`, a clip, text state…) must not reach the mark, including
/// content appended after an earlier PdfCraft wrapper by another tool (#3084): the stretch of
/// content that leaves state set is wrapped in q/Q, once. Any known unfinished marked-content
/// sequences are closed at the end too, so hidden optional content cannot hide the new mark; when
/// their number is unknown, nothing is placed on top. Returns the new content stream.
pub(crate) fn place_tagged(doc: &mut Document, page: &pdfcraft_model::Page, tag: &str, content: Vec<u8>, behind: bool) -> Result<ObjRef, EditError> {
    let mut list = contents(doc, page)?;
    // The behind-marks at the start: backgrounds in the order added, then watermarks.
    let behind_kinds = [MarkKind::Background.tag(), MarkKind::Watermark.tag()];
    let behind_marks = list.iter().take_while(|o| stream_tag(doc, o).is_some_and(|t| behind_kinds.contains(&t.as_str()))).count();
    let wrap = if behind {
        None
    } else {
        match isolation(doc, &list) {
            Isolation::Clean { suffix, marked, .. } => suffix.map(|split| (split, 0, marked)),
            // Content that can't be read precisely is wrapped whole, as before: after the
            // behind-marks at the start. A `q` it leaves open is closed at its end, where
            // closing it changes nothing already drawn.
            Isolation::Unknown { unclosed, marked } => (behind_marks < list.len()).then_some((behind_marks, unclosed, marked)),
            // Content left inside an optional-content sequence it cannot close could be hidden.
            Isolation::TooDeep => return Err(too_deep_on_top()),
        }
    };
    let mark_ref = doc.add(Object::Stream(Stream::flate(tagged(tag), &content)));
    let mark = Object::Ref(mark_ref);
    if behind {
        // Right after the backgrounds already there, so a new background is painted over the
        // older ones (#3591) and a watermark over all of them. Not after the watermarks: on a
        // page with no content, one on top is at the start too and can't be told apart, so a
        // newer watermark behind goes under the older ones.
        let at = list.iter().take(behind_marks).rposition(|o| stream_tag(doc, o).as_deref() == Some(MarkKind::Background.tag())).map_or(0, |i| i + 1);
        list.insert(at, mark);
    } else {
        if let Some((split, unclosed, marked)) = wrap {
            let mut close_bytes = b"\nEMC".repeat(marked);
            close_bytes.extend_from_slice(&b"\nQ".repeat(unclosed));
            close_bytes.extend_from_slice(WRAP_CLOSE);
            let open = Object::Ref(doc.add(Object::Stream(Stream::from_raw(tagged("Wrap"), WRAP_OPEN.to_vec()))));
            let close = Object::Ref(doc.add(Object::Stream(Stream::from_raw(tagged("Wrap"), close_bytes))));
            list.insert(split.min(list.len()), open);
            list.push(close);
        }
        list.push(mark);
    }
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(list)))?;
    Ok(mark_ref)
}

fn too_deep_on_top() -> EditError {
    EditError::Invalid("the page's marked content is nested too deeply to add anything on top of it".into())
}

/// The refusal [`place_tagged`] gives for a mark on top of `page`, checked before an edit changes
/// anything (removing old marks, adding resources) so a refused edit leaves the document as it was.
pub(crate) fn check_on_top(doc: &Document, page: &pdfcraft_model::Page) -> Result<(), EditError> {
    let obj = doc.get(page.obj);
    let list = match obj.as_dict().and_then(|d| d.get(b"Contents")) {
        None => return Ok(()),
        Some(Object::Ref(r)) => doc.get(*r).as_array().cloned().unwrap_or_else(|| vec![Object::Ref(*r)]),
        Some(Object::Array(a)) => a.clone(),
        Some(other) => vec![other.clone()],
    };
    match isolation(doc, &list) {
        Isolation::TooDeep => Err(too_deep_on_top()),
        _ => Ok(()),
    }
}

/// Put content tagged `tag` first inside the q/Q wrapper, so it applies to the page's original
/// content and nothing else (a header's shrink). A page without a wrapper has no original content
/// on it and is left alone.
pub(crate) fn place_inside_wrapper(doc: &mut Document, page: &pdfcraft_model::Page, tag: &str, content: Vec<u8>) -> Result<(), EditError> {
    let mut list = contents(doc, page)?;
    let Some(open) = list.iter().position(|o| stream_tag(doc, o).as_deref() == Some("Wrap")) else { return Ok(()) };
    let stream = Object::Ref(doc.add(Object::Stream(Stream::flate(tagged(tag), &content))));
    list.insert(open + 1, stream);
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(list)))?;
    Ok(())
}

/// Draw `content` on top of a page (0-based) as permanent page content, with standard Helvetica
/// available as `/PCHelv`. The stream is tagged `tag` (not a mark kind, so Remove never takes
/// it away); the original content is wrapped in q/Q first. Used for applied redaction boxes.
/// Returns the new content stream.
pub fn stamp(doc: &mut Document, page: usize, tag: &str, content: Vec<u8>) -> Result<ObjRef, EditError> {
    let all = page_list(doc);
    check(&[page], all.len())?;
    check_on_top(doc, &all[page])?;
    add_resources(doc, &all[page], None, Some(&content))?;
    let p = page_list(doc).swap_remove(page);
    place_tagged(doc, &p, tag, content, false)
}

pub(crate) fn begin(kind: MarkKind, subtype: &str, matrix: [f64; 6]) -> String {
    format!(
        "q\n/Artifact <</Type /Pagination /Subtype /{subtype} /PCMark /{}>> BDC\n{} {} {} {} {} {} cm\n",
        kind.tag(),
        matrix_number(matrix[0]),
        matrix_number(matrix[1]),
        matrix_number(matrix[2]),
        matrix_number(matrix[3]),
        matrix_number(matrix[4]),
        matrix_number(matrix[5])
    )
}

pub(crate) const END: &str = "EMC\nQ\n";

/// The refusal for page text with `c`, which nothing here can draw.
pub(crate) fn undrawable(c: char) -> EditError {
    EditError::Invalid(undrawable_message(c))
}

/// The wording of the refusal for text with `c`, which the standard fonts can't draw.
pub fn undrawable_message(c: char) -> String {
    format!("the standard fonts can't draw \"{c}\" (U+{:04X}); only Western European characters can be added as text for now", u32::from(c))
}

pub(crate) fn text_op(x: f64, y: f64, text: &str) -> Vec<u8> {
    let mut v = format!("1 0 0 1 {} {} Tm ", n(x), n(y)).into_bytes();
    v.extend(literal(&win_ansi(text)));
    v.extend_from_slice(b" Tj\n");
    v
}
