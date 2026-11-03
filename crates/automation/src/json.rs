use super::error::{Result, ToolError};
use super::{Document, Value, comments, json};

/// How far from the page's corner a bookmark's position may be given, in points (a PDF page is at
/// most 14 400 across).
const MAX_BOOKMARK_POSITION: f64 = 1_000_000.0;

// ---- JSON helpers ----------------------------------------------------------------------------

/// The most bookmark levels `bookmark_from_text_styles` takes.
const MAX_STYLE_LEVELS: usize = 9;

/// bookmark_from_text_styles' `levels`: one style per bookmark level, the first level 1, each
/// `{ font, size, bold, italic }` as text_styles lists them.
pub(crate) fn text_style_levels(a: &Args) -> Result<Vec<pdfcraft_engine::text_style_bookmarks::TextStyle>> {
    let levels =
        a.get("levels").and_then(Value::as_array).ok_or_else(|| ToolError::InvalidArgs("levels must list a style for each bookmark level".into()))?;
    if levels.is_empty() || levels.len() > MAX_STYLE_LEVELS {
        return Err(ToolError::InvalidArgs(format!("levels must list 1 to {MAX_STYLE_LEVELS} styles")));
    }
    levels
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let bad = |what: &str| ToolError::InvalidArgs(format!("levels[{i}]: {what}"));
            let font =
                v.get("font").and_then(Value::as_str).filter(|f| !f.is_empty()).ok_or_else(|| bad("font must be a font name from text_styles"))?;
            let size =
                v.get("size").and_then(Value::as_f64).filter(|s| s.is_finite() && *s > 0.0).ok_or_else(|| bad("size must be a positive number"))?;
            let flag = |key: &str| match v.get(key) {
                None | Some(Value::Null) => Ok(false),
                Some(Value::Bool(b)) => Ok(*b),
                Some(_) => Err(bad(&format!("{key} must be true or false"))),
            };
            Ok(pdfcraft_engine::text_style_bookmarks::TextStyle { font: font.to_string(), size, bold: flag("bold")?, italic: flag("italic")? })
        })
        .collect()
}

pub(crate) fn parse_style_ranges(value: Option<&Value>) -> Result<Vec<pdfcraft_engine::RangeStyle>> {
    let Some(value) = value else { return Ok(Vec::new()) };
    let values = value.as_array().filter(|v| v.len() <= 1024).ok_or_else(|| Args::wrong("ranges", "an array of at most 1024 style objects"))?;
    values
        .iter()
        .map(|v| {
            let a = Args(v);
            let bounds = a
                .get("range")
                .and_then(Value::as_array)
                .filter(|v| v.len() == 2)
                .ok_or_else(|| Args::wrong("range", "[start,end] UTF-8 byte offsets"))?;
            let start = bounds
                .first()
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| Args::wrong("range", "nonnegative byte offsets"))?;
            let end = bounds
                .get(1)
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| Args::wrong("range", "nonnegative byte offsets"))?;
            let family = a
                .opt_str("font")?
                .map(|f| match f {
                    "helvetica" => Ok(pdfcraft_engine::FontFamily::Helvetica),
                    "times" => Ok(pdfcraft_engine::FontFamily::Times),
                    "courier" => Ok(pdfcraft_engine::FontFamily::Courier),
                    _ => Err(Args::wrong("font", "helvetica, times or courier")),
                })
                .transpose()?
                .map(|f| -> Result<_> { Ok((f, a.opt_bool("bold")?.unwrap_or(false), a.opt_bool("italic")?.unwrap_or(false))) })
                .transpose()?;
            Ok(pdfcraft_engine::RangeStyle {
                range: start..end,
                family,
                resource: a.opt_str("font_resource")?.map(str::to_owned),
                size: a.opt_num("size")?,
                color: a.opt_str("color")?.map(comments::parse_color).transpose()?,
                underline: a.opt_bool("underline")?,
                char_spacing: a.opt_num("char_spacing")?,
                scale: a.opt_num("scale")?,
            })
        })
        .collect()
}

/// Typed access to validated arguments.
pub(crate) struct Args<'a>(pub(crate) &'a Value);

impl Args<'_> {
    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key).filter(|v| !v.is_null())
    }
    pub(crate) fn missing(key: &str) -> ToolError {
        ToolError::InvalidArgs(format!("missing argument {key}"))
    }
    pub(crate) fn wrong(key: &str, what: &str) -> ToolError {
        ToolError::InvalidArgs(format!("{key} must be {what}"))
    }
    pub(crate) fn str(&self, key: &str) -> Result<&str> {
        self.opt_str(key)?.ok_or_else(|| Self::missing(key))
    }
    pub(crate) fn opt_str(&self, key: &str) -> Result<Option<&str>> {
        self.get(key).map(|v| v.as_str().ok_or_else(|| Self::wrong(key, "a string"))).transpose()
    }
    pub(crate) fn int(&self, key: &str) -> Result<i64> {
        self.opt_int(key)?.ok_or_else(|| Self::missing(key))
    }
    pub(crate) fn opt_int(&self, key: &str) -> Result<Option<i64>> {
        self.get(key).map(|v| v.as_i64().ok_or_else(|| Self::wrong(key, "an integer"))).transpose()
    }
    pub(crate) fn opt_num(&self, key: &str) -> Result<Option<f64>> {
        self.get(key).map(|v| v.as_f64().filter(|f| f.is_finite()).ok_or_else(|| Self::wrong(key, "a number"))).transpose()
    }
    pub(crate) fn opt_bool(&self, key: &str) -> Result<Option<bool>> {
        self.get(key).map(|v| v.as_bool().ok_or_else(|| Self::wrong(key, "true or false"))).transpose()
    }
    pub(crate) fn ints(&self, key: &str) -> Result<Vec<i64>> {
        self.opt_ints(key)?.ok_or_else(|| Self::missing(key))
    }
    pub(crate) fn opt_ints(&self, key: &str) -> Result<Option<Vec<i64>>> {
        let Some(v) = self.get(key) else { return Ok(None) };
        let arr = v.as_array().ok_or_else(|| Self::wrong(key, "an array of integers"))?;
        arr.iter().map(|x| x.as_i64().ok_or_else(|| Self::wrong(key, "an array of integers"))).collect::<Result<Vec<_>>>().map(Some)
    }
    /// A 1-based bookmark path → 0-based indices.
    pub(crate) fn path(&self, key: &str) -> Result<Vec<usize>> {
        let p = self.opt_path(key)?.ok_or_else(|| Self::missing(key))?;
        if p.is_empty() {
            return Err(ToolError::InvalidArgs(format!("{key} must not be empty")));
        }
        Ok(p)
    }
    pub(crate) fn opt_path(&self, key: &str) -> Result<Option<Vec<usize>>> {
        let Some(v) = self.opt_ints(key)? else { return Ok(None) };
        v.iter().map(|i| if *i >= 1 { Ok(*i as usize - 1) } else { Err(Self::wrong(key, "1-based positions")) }).collect::<Result<Vec<_>>>().map(Some)
    }
    /// A bookmark's `left` or `top`: points on the displayed page, `None` when left out, an error
    /// when it is not a number or lies absurdly far off the page.
    pub(crate) fn bookmark_coordinate(&self, key: &str) -> Result<Option<f32>> {
        match self.opt_num(key)? {
            None => Ok(None),
            Some(v) if v.is_finite() && v.abs() <= MAX_BOOKMARK_POSITION => Ok(Some(v as f32)),
            Some(_) => Err(ToolError::InvalidArgs(format!("{key} must be between -{MAX_BOOKMARK_POSITION} and {MAX_BOOKMARK_POSITION} points"))),
        }
    }
    pub(crate) fn strs(&self, key: &str) -> Result<Vec<&str>> {
        let v = self.get(key).ok_or_else(|| Self::missing(key))?;
        let arr = v.as_array().ok_or_else(|| Self::wrong(key, "an array of strings"))?;
        arr.iter().map(|x| x.as_str().ok_or_else(|| Self::wrong(key, "an array of strings"))).collect()
    }
}

/// The name-tree keys of a document's own attachments, leaving out the files attached to pages as comments.
pub(crate) fn document_attachment_keys(doc: &Document) -> Vec<String> {
    doc.info
        .attachments
        .iter()
        .filter_map(|x| match &x.source {
            pdfcraft_render::AttachmentSource::Document { key } => Some(key.clone()),
            pdfcraft_render::AttachmentSource::Annotation { .. } => None,
        })
        .collect()
}

/// Bookmark rows in listing order as JSON: a row holds the rows that follow it on the page at the
/// next level down in `children`, and paths and pages are 1-based.
pub(crate) fn nest_rows(rows: &[pdfcraft_engine::BookmarkRow], pages: &[pdfcraft_render::PageInfo]) -> Vec<Value> {
    // The open rows, outermost first, with their depth and the children gathered so far.
    let mut open: Vec<(usize, Value, Vec<Value>)> = Vec::new();
    let mut roots = Vec::new();
    for row in rows {
        let depth = row.path.len().saturating_sub(1);
        while open.last().is_some_and(|(d, _, _)| *d >= depth) {
            close_row(&mut open, &mut roots);
        }
        let path: Vec<usize> = row.path.iter().map(|p| p.saturating_add(1)).collect();
        let mut fields = json!({ "path": path, "title": row.title, "page": row.page.map(|p| p.saturating_add(1)), "open": row.open });
        // Only what differs from the defaults (black, regular), as the PDF itself stores it.
        if let Some(color) = row.color {
            fields["color"] = json!(color);
        }
        if row.bold {
            fields["bold"] = json!(true);
        }
        if row.italic {
            fields["italic"] = json!(true);
        }
        if let Some(destination) = row.page.and_then(|p| pages.get(p)).and_then(|p| destination(&row.view, p)) {
            fields["destination"] = destination;
        }
        open.push((depth, fields, Vec::new()));
    }
    while !open.is_empty() {
        close_row(&mut open, &mut roots);
    }
    roots
}

/// Where on its page a bookmark or link goes, in points from the top-left of the displayed page
/// like every `rect`: the destination type (`XYZ`, `Fit`, `FitH`, `FitV`, `FitR`) and its
/// position, `null` where the destination leaves it unspecified (`/XYZ null null null` keeps the
/// reader's position and zoom). `None` for a destination without a type.
pub(crate) fn destination(view: &pdfcraft_render::DestView, page: &pdfcraft_render::PageInfo) -> Option<Value> {
    use pdfcraft_render::DestView;
    let points = |left: Option<f32>, top: Option<f32>| {
        let [x, y] = page.dest_fraction(left, top, 0);
        let round = |v: f32| f64::from((v * 100.0).round() / 100.0);
        (x.map(|x| round(x * page.width)), y.map(|y| round(y * page.height)))
    };
    Some(match view {
        DestView::Top => return None,
        DestView::Xyz { left, top, zoom } => {
            let (x, y) = points(*left, *top);
            json!({ "fit": "XYZ", "left": x, "top": y, "zoom": zoom.map(f64::from) })
        }
        DestView::Fit => json!({ "fit": "Fit" }),
        DestView::FitH { top } => {
            let (x, y) = points(None, *top);
            json!({ "fit": "FitH", "left": x, "top": y })
        }
        DestView::FitV { left } => {
            let (x, y) = points(*left, None);
            json!({ "fit": "FitV", "left": x, "top": y })
        }
        DestView::FitR { rect } => json!({ "fit": "FitR", "rect": comments::rect_to_view(page, *rect) }),
    })
}

/// Closes the innermost open row: it joins the row above it as a child, or the top level.
fn close_row(open: &mut Vec<(usize, Value, Vec<Value>)>, roots: &mut Vec<Value>) {
    if let Some((_, mut fields, children)) = open.pop() {
        fields["children"] = Value::Array(children);
        match open.last_mut() {
            Some((_, _, siblings)) => siblings.push(fields),
            None => roots.push(fields),
        }
    }
}

pub(crate) fn one_based(pages: &[i64]) -> Result<Vec<usize>> {
    pages
        .iter()
        .map(|p| if *p >= 1 { Ok(*p as usize - 1) } else { Err(ToolError::InvalidArgs(format!("page numbers start at 1 (got {p})"))) })
        .collect()
}

/// The document's security as the next save writes it (never includes passwords).
pub(crate) fn security(d: &Document) -> Value {
    match d.security_summary() {
        None => json!({ "protected": false }),
        Some(s) => {
            let p = s.permissions;
            json!({
                "protected": true,
                "method": s.method,
                "pending": s.pending,
                "printing": if !p.print() { "none" } else if p.print_high_quality() { "high" } else { "low" },
                "modify": p.modify(), "assemble": p.assemble(), "copy": p.copy(), "annotate": p.annotate(),
                "fill_forms": p.fill_forms(), "accessibility": p.extract_for_accessibility(),
            })
        }
    }
}

/// What every tool that changes a document returns.
pub(crate) fn summary(d: &Document) -> Value {
    json!({
        "doc": d.id.0,
        "name": d.name,
        "path": d.path,
        "pages": d.info.pages.len(),
        "dirty": d.dirty,
        "editable": d.read_only_reason.is_none(),
        "read_only_reason": d.read_only_reason,
        "encrypted": d.info.encrypted,
        "undo": d.can_undo(),
        "redo": d.can_redo(),
    })
}

pub(crate) fn info(d: &Document) -> Value {
    let i = &d.info;
    let page1 = |p: usize| p + 1;
    let mut security = security(d);
    if let Some(s) = d.security_summary() {
        security["opened_as_owner"] = json!(s.owner);
    }
    json!({
        "document": summary(d),
        "pdf_version": i.pdf_version,
        "file_size": i.file_size,
        "title": i.title, "author": i.author, "subject": i.subject, "keywords": i.keywords,
        // Custom document properties (Document Properties ▸ Custom): /Info entries beyond the
        // ones the spec names, set and cleared with doc_set_info.
        "custom": i.custom.iter().map(|(name, value)| json!({ "name": name, "value": value })).collect::<Vec<_>>(),
        "creator": i.creator, "producer": i.producer,
        "tagged": i.tagged,
        "has_javascript": i.has_javascript,
        // Where the document opens (/OpenAction, named destinations resolved): 1-based page and
        // destination, null without one.
        "open_destination": i.open_dest.as_ref().map(|(p, view)| json!({ "page": page1(*p), "destination": i.pages.get(*p).and_then(|pg| destination(view, pg)) })),
        // XFA forms: "static" (the PDF's own fields work; the XFA data is ignored) or "dynamic"
        // (laid out from the template by PdfCraft, see xfa_layout; placeholder pages when that failed).
        "xfa": i.xfa.map(|x| match x { pdfcraft_render::Xfa::Static => "static", pdfcraft_render::Xfa::Dynamic => "dynamic" }),
        "xfa_layout": d.xfa.as_ref().map(|x| json!({ "pages": x.pages, "fields": x.fields, "warnings": x.warnings })),
        // A script of the form ran too long and was abandoned: its scripts no longer run in this
        // document (calculations, buttons) until it is opened again.
        "xfa_scripts_off": d.xfa_scripts_off(),
        // What was rewritten from, or could not be written to, the XFA data.
        "xfa_warnings": d.xfa_warnings,
        "security": security,
        "pages": i.pages.iter().enumerate().map(|(n, p)| json!({
            "page": n + 1, "label": p.label, "width": p.width, "height": p.height, "rotation": p.rotation,
        })).collect::<Vec<_>>(),
        "outline": outline(&i.outline),
        "outline_more": i.outline_more,
        // Rectangles use the tools' convention (top-left of the displayed page, like
        // comment_list and link_list), not raw PDF user space, so they can be fed back to
        // geometry-taking tools (#129).
        "annotations": i.annotations.iter().map(|a| json!({
            "page": page1(a.page), "type": a.subtype, "author": a.author, "contents": a.contents,
            "modified": a.modified, "name": a.name, "in_reply_to": a.in_reply_to,
            "rect": view_rect(i, a.page, a.shown_rect(i.pages.get(a.page).map_or(0, |p| p.rotation))),
        })).collect::<Vec<_>>(),
        "fields": i.fields.iter().map(|f| json!({
            "name": f.name, "kind": format!("{:?}", f.kind), "value": f.value, "page": f.page.map(page1),
            "tooltip": f.tooltip, "has_actions": f.has_actions,
        })).collect::<Vec<_>>(),
        "links": i.links.iter().map(|l| json!({
            "page": page1(l.page), "rect": view_rect(i, l.page, l.rect),
            "target": match &l.target {
                pdfcraft_render::LinkTarget::Page(p, _) => json!({ "page": page1(*p) }),
                pdfcraft_render::LinkTarget::Uri(u) => json!({ "uri": u }),
                pdfcraft_render::LinkTarget::SetLayers { changes, preserve_rb } => json!({
                    "layers": changes.iter().map(|(op, ocg)| json!({
                        "layer": i.layers.iter().find(|l| l.id == *ocg).map(|l| l.name.as_str()),
                        "state": match op {
                            pdfcraft_render::LayerOp::On => "on",
                            pdfcraft_render::LayerOp::Off => "off",
                            pdfcraft_render::LayerOp::Toggle => "toggle",
                        },
                    })).collect::<Vec<_>>(),
                    "preserve_rb": preserve_rb,
                }),
                pdfcraft_render::LinkTarget::RemoteGoTo { launch, file, destination, new_window } => json!({
                    "action": if *launch { "Launch" } else { "GoToR" },
                    "remote_document": file,
                    "destination": match destination {
                        pdfcraft_render::RemoteDestination::Default => json!({ "kind": "default" }),
                        pdfcraft_render::RemoteDestination::Page { page, view } => json!({ "kind": "page", "page": page1(*page), "view": format!("{view:?}") }),
                        pdfcraft_render::RemoteDestination::Named(name) => json!({ "kind": "named", "name": name }),
                    },
                    "new_window": new_window,
                }),
                pdfcraft_render::LinkTarget::Other(o) => json!({ "other": o }),
            },
        })).collect::<Vec<_>>(),
        "layers": i.layers.iter().map(|l| json!({ "name": l.name, "visible": l.visible, "initial": l.initial })).collect::<Vec<_>>(),
        // Full-screen presentation settings a page carries (ISO 32000-2 §12.4.4): only pages with
        // a `/Trans` or `/Dur` appear; `style` is null for a display time alone.
        "transitions": i.transitions.iter().map(|t| json!({
            "page": page1(t.page), "style": t.style.map(pdfcraft_render::TransitionStyle::name), "duration": t.duration,
            "horizontal": t.horizontal, "inward": t.inward, "direction": t.direction, "scale": t.scale, "opaque": t.opaque,
            "display": t.display,
        })).collect::<Vec<_>>(),
        "attachments": i.attachments.iter().map(|a| json!({ "name": a.name, "description": a.description, "size": a.size })).collect::<Vec<_>>(),
        // `substitute` names the face standing in for a program the document does not embed, and
        // `substitution` says why. Both are null for an embedded font: nothing stands in. A null
        // `substitute` with a non-null `substitution` means no face was available at all, so
        // nothing is drawn for that font -- which is the answer to "why is my text missing?".
        "fonts": i.fonts.iter().map(|f| json!({
            "name": f.name, "kind": f.kind, "embedded": f.embedded, "subset": f.subset, "encoding": f.encoding,
            "substitute": f.substitute, "substitution": f.substitution,
        })).collect::<Vec<_>>(),
        "warnings": i.warnings,
        "repairs": d.repair_log(),
    })
}

/// A user-space rectangle on 0-based `page` in displayed-page coordinates (points from the
/// top-left of the page as displayed, y down, after `/Rotate`), the way every tool reports
/// rectangles; unchanged when the page is unknown (a malformed annotation that points at no page).
/// Public so `pdfcraft-cli` reports the same rectangles as the tools.
pub fn view_rect(i: &pdfcraft_render::DocInfo, page: usize, rect: [f32; 4]) -> [f32; 4] {
    i.pages.get(page).map_or(rect, |p| comments::rect_to_view(p, rect))
}

fn outline(items: &[pdfcraft_render::OutlineItem]) -> Value {
    Value::Array(items.iter().map(|o| json!({ "title": o.title, "page": o.page.map(|p| p + 1), "children": outline(&o.children) })).collect())
}
