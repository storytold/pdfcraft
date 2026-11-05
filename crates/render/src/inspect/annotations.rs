use super::core::{limited_pdf_string, pretty_date, spend};
use super::model::{
    Annot3d, Annotation, AttachmentSource, DestView, Dimension3d, Field, FieldKind, Format3d, LayerOp, Link, LinkTarget, Projection3d,
    RemoteDestination, View3d, link_replies,
};
use super::navigation::text_string;
use super::{Dictionary, DocInfo, Entry, HashMap, HashSet, Inspector, Object, ObjectId};

/// At most this many `/State` entries of a set-layer-visibility action are read.
const MAX_LAYER_STATE: usize = 1024;

/// File specifications retained from a remote-Go-To action are deliberately small.
pub(super) const MAX_REMOTE_FILE_SPEC_BYTES: usize = 4096;

const MAX_REMOTE_DESTINATION_NAME_BYTES: usize = 1024;

/// Text and geometry `DocInfo::annotations` keeps, as counted by [`retained`]. Past it the rest
/// are not listed: real comments stay far below, many small ones sharing one long string do not.
pub(super) const MAX_ANNOTATION_BYTES: usize = 64 << 20;

/// Web-address text the link listing, and name, description and embedded-file key text the
/// attachment listing, keep together. Many links or attachments can share one long string, which
/// each would otherwise copy. Past it, a link to a web address lists as a URI action without its
/// address, an attachment under its generic name without a description, and an embedded file whose
/// key does not fit is not listed.
pub(super) const MAX_LINK_ATTACHMENT_TEXT_BYTES: usize = 16 << 20;

/// 3D models `DocInfo::annots_3d` lists. Real files have one per page or fewer.
pub(super) const MAX_ANNOTS_3D: usize = 1024;

/// A 3D view chosen by name is looked for among this many of the stream's views...
const MAX_VIEWS_SEARCHED: usize = 1024;
/// ...and only for a name this long or shorter. Real view names are a few words.
const MAX_VIEW_NAME_BYTES: usize = 1024;

/// Whether a `/C2W` matrix has a camera: its y and z axes not zero and not parallel.
fn has_camera_axes(c: &[f64; 12]) -> bool {
    let (y, z) = ([c[3], c[4], c[5]], [c[6], c[7], c[8]]);
    let length = |v: [f64; 3]| v.iter().map(|x| x * x).sum::<f64>().sqrt();
    let cross = [y[1] * z[2] - y[2] * z[1], y[2] * z[0] - y[0] * z[2], y[0] * z[1] - y[1] * z[0]];
    let (ly, lz) = (length(y), length(z));
    ly > 0.0 && lz > 0.0 && length(cross) > ly * lz * 1e-9
}

/// The bytes a listed annotation keeps.
pub(super) fn retained(a: &Annotation) -> usize {
    let text = [&a.author, &a.contents, &a.modified, &a.name, &a.in_reply_to, &a.state, &a.intent];
    text.into_iter()
        .map(|t| t.as_ref().map_or(0, String::len))
        .chain([
            a.subtype.len(),
            std::mem::size_of_val(a.quads.as_slice()),
            std::mem::size_of_val(a.points.as_slice()),
            std::mem::size_of::<Annotation>(),
        ])
        .fold(0, usize::saturating_add)
}

impl<'a> Inspector<'a> {
    /// The format of a 3D annotation's model: its `/3DD` stream's `/Subtype`, following one 3D
    /// reference dictionary (`/Type /3DRef`, a stream another annotation shares; §13.6.3).
    fn model_3d(&self, page: usize, index: usize, annot: &Dictionary) -> Annot3d {
        let rect = rect4(self.resolve(annot.get(b"Rect").unwrap_or(&Object::Null)));
        let mut data = annot.get(b"3DD").ok().and_then(|o| self.dict(o));
        if let Some(d) = data
            && self.name(d, b"Type").as_deref() == Some("3DRef")
        {
            data = d.get(b"3DD").ok().and_then(|o| self.dict(o));
        }
        let format = match data.and_then(|d| self.name(d, b"Subtype")).as_deref() {
            Some("PRC") => Format3d::Prc,
            Some("U3D") => Format3d::U3d,
            _ => Format3d::Unknown,
        };
        // A null entry is an absent one (§7.3.7).
        let chosen = match annot.get(b"3DV").ok().filter(|s| !matches!(self.resolve(s), Object::Null)) {
            Some(selector) => self.select_view(selector, data, true),
            None => self.default_view(data),
        };
        Annot3d { page, index, rect, format, view: chosen.and_then(|v| self.view_3d(v)) }
    }

    /// The 3D stream's default view (§13.6.3): its `/DV`, else its first view.
    fn default_view(&self, data: Option<&'a Dictionary>) -> Option<&'a Dictionary> {
        let d = data?;
        d.get(b"DV").ok().and_then(|dv| self.select_view(dv, data, false)).or_else(|| self.views(data).first().and_then(|v| self.dict(v)))
    }

    fn views(&self, data: Option<&'a Dictionary>) -> &'a [Object] {
        data.and_then(|d| d.get(b"VA").ok()).and_then(|o| self.resolve(o).as_array().ok()).map_or(&[], Vec::as_slice)
    }

    /// A view given by `/3DV` or `/DV` (§13.6.2): a view dictionary, an index or name into the
    /// stream's `/VA`, or `/F`, `/L` (first, last) and, for `/3DV`, `/D` (the default).
    fn select_view(&self, selector: &'a Object, data: Option<&'a Dictionary>, may_default: bool) -> Option<&'a Dictionary> {
        let views = self.views(data);
        match self.resolve(selector) {
            Object::Dictionary(_) | Object::Stream(_) => self.dict(selector),
            Object::Integer(i) => usize::try_from(*i).ok().and_then(|i| views.get(i)).and_then(|v| self.dict(v)),
            Object::Name(n) => match n.as_slice() {
                b"F" => views.first().and_then(|v| self.dict(v)),
                b"L" => views.last().and_then(|v| self.dict(v)),
                b"D" if may_default => self.default_view(data),
                _ => None,
            },
            // A name matches a view's internal name `/IN`; a view without one is matched by its
            // external name `/XN`. Compared as written, without decoding, over a bounded number
            // of views and only for a short name, so no file makes this scan costly.
            Object::String(s, _) if s.len() <= MAX_VIEW_NAME_BYTES => {
                let named = |d: &Dictionary| {
                    let key: &[u8] = if d.has(b"IN") { b"IN" } else { b"XN" };
                    matches!(d.get(key).map(|o| self.resolve(o)), Ok(Object::String(n, _)) if n == s)
                };
                views.iter().take(MAX_VIEWS_SEARCHED).filter_map(|v| self.dict(v)).find(|d| named(d))
            }
            _ => None,
        }
    }

    /// A 3D view dictionary (§13.6.4) with a camera given by `/C2W`; `None` for views positioned
    /// another way (a U3D path) or without a usable matrix.
    fn view_3d(&self, view: &Dictionary) -> Option<View3d> {
        let num = |o: &Object| match self.resolve(o) {
            Object::Integer(i) => Some(*i as f64),
            Object::Real(r) => Some(f64::from(*r)),
            _ => None,
        };
        if self.name(view, b"MS").is_some_and(|ms| ms != "M") {
            return None;
        }
        let c2w = self.resolve(view.get(b"C2W").ok()?).as_array().ok()?;
        let mut camera_to_world = [0.0; 12];
        for (out, o) in camera_to_world.iter_mut().zip(c2w) {
            *out = num(o)?;
        }
        if c2w.len() != 12 || camera_to_world.iter().any(|v| !v.is_finite()) || !has_camera_axes(&camera_to_world) {
            return None;
        }
        let center_of_orbit = view.get(b"CO").ok().and_then(num).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(0.0);
        let dimension = |d: Option<&Object>, default: Dimension3d| match d.map(|o| self.resolve(o)) {
            Some(Object::Name(n)) => match n.as_slice() {
                b"W" => Dimension3d::Width,
                b"H" => Dimension3d::Height,
                b"Max" => Dimension3d::Max,
                b"Min" => Dimension3d::Min,
                _ => default,
            },
            _ => default,
        };
        let p = view.get(b"P").ok().and_then(|o| self.dict(o));
        let projection = match p {
            Some(p) if self.name(p, b"Subtype").as_deref() == Some("O") => Projection3d::Orthographic {
                scale: p.get(b"OS").ok().and_then(num).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(1.0),
                scaled: dimension(p.get(b"OB").ok(), Dimension3d::Width),
            },
            Some(p) => Projection3d::Perspective {
                fov: p.get(b"FOV").ok().and_then(num).filter(|v| *v > 0.0 && *v < 180.0).unwrap_or(90.0),
                scaled: dimension(p.get(b"PS").ok(), Dimension3d::Width),
            },
            None => Projection3d::Perspective { fov: 90.0, scaled: Dimension3d::Width },
        };
        let background = view
            .get(b"BG")
            .ok()
            .and_then(|o| self.dict(o))
            .and_then(|bg| bg.get(b"C").ok())
            .and_then(|c| self.resolve(c).as_array().ok())
            .filter(|c| c.len() == 3)
            .and_then(|c| Some([num(&c[0])? as f32, num(&c[1])? as f32, num(&c[2])? as f32]));
        Some(View3d { camera_to_world, center_of_orbit, projection, background })
    }

    /// Whether an annotation is a Fill & Sign mark. Keep in step with
    /// `pdfcraft_annot::is_fill_sign`, which the engine's comment list uses.
    fn is_fill_sign(&self, d: &Dictionary, subtype: &str) -> bool {
        let yes = |key: &[u8]| matches!(d.get(key).map(|o| self.resolve(o)), Ok(Object::Boolean(true)));
        if yes(b"PCFillSign") {
            return true;
        }
        match subtype {
            "FreeText" => self.name(d, b"IT").as_deref() == Some("FreeTextTypeWriter"),
            "Stamp" => match self.name(d, b"Name").as_deref() {
                Some("PCCheck" | "PCCross" | "PCDot" | "PCLine" | "PCTypedSignature") => true,
                Some("PCCustomSignature" | "PCCustomInitials") => yes(b"PCPictureImage"),
                _ => false,
            },
            "Ink" => self.text(d, b"Subj").as_deref() == Some("Signature"),
            _ => false,
        }
    }

    // ── annotations ─────────────────────────────────────────────────────────────────────────

    /// `text_budget` is what is left of [`MAX_LINK_ATTACHMENT_TEXT_BYTES`].
    pub(super) fn annotations(&self, info: &mut DocInfo, text_budget: &mut usize) {
        let mut pages: Vec<(ObjectId, usize)> = self.page_index.iter().map(|(&id, &page)| (id, page)).collect();
        pages.sort_unstable_by_key(|&(_, page)| page);
        // An annotation, or a whole `/Annots` array, that several entries or pages list is on the
        // first of them (§12.5.2 allows one page per annotation). Both are told apart by the object
        // a reference resolves to, since an indirect object may itself hold a reference. Where each
        // annotation is listed is collected before any is read, so a reply's `/IRT` finds its
        // parent by reference even when the parent comes later.
        let mut seen_arrays = HashSet::new();
        let mut slots: HashMap<ObjectId, (usize, usize)> = HashMap::new();
        let mut entries = Vec::new();
        for (id, page) in pages {
            let Ok(page_dict) = self.doc.get_dictionary(id) else { continue };
            if let Ok(annots) = page_dict.get(b"Annots")
                && let Some(r) = self.resolved_id(annots)
                && !seen_arrays.insert(r)
            {
                continue;
            }
            let Ok(Object::Array(annots)) = page_dict.get(b"Annots").map(|o| self.resolve(o)) else { continue };
            for (index, a) in annots.iter().enumerate() {
                if let Some(r) = self.resolved_id(a) {
                    let Entry::Vacant(slot) = slots.entry(r) else { continue };
                    slot.insert((page, index));
                }
                entries.push((page, index, a));
            }
        }
        let mut kept = 0usize;
        // Once the comment budget is spent, comments are no longer built, but links and attachments
        // are still listed.
        let mut comments_full = false;
        for (page, index, a) in entries {
            let Some(d) = self.dict(a) else { continue };
            let Some(subtype) = self.name(d, b"Subtype") else { continue };
            if subtype == "FileAttachment"
                && let Ok(fs) = d.get(b"FS")
            {
                let fallback = |budget: &mut usize| self.budgeted_text(d, b"Contents", budget);
                info.attachments.push(self.attachment(fs, AttachmentSource::Annotation { page, index }, text_budget, fallback));
            }
            if subtype == "Link" {
                if let Some(link) = self.link(page, index, d, text_budget) {
                    if matches!(&link.target, LinkTarget::Other(s) if s == "JavaScript") {
                        info.has_javascript = true;
                    }
                    info.links.push(link);
                }
                continue;
            }
            if subtype == "3D" {
                // Hidden (2) and NoView (32) annotations are not shown (§12.5.3).
                let flags = d.get(b"F").ok().and_then(|f| self.resolve(f).as_i64().ok()).unwrap_or(0);
                if flags & (2 | 32) == 0 && info.annots_3d.len() < MAX_ANNOTS_3D {
                    info.annots_3d.push(self.model_3d(page, index, d));
                }
                continue;
            }
            // Not comments: form widgets, pop-ups and non-markup annotations such as the
            // Screen annotation that drives a LaTeX `animate` player (keep in step with
            // `pdfcraft_annot::is_comment_subtype`).
            if matches!(subtype.as_str(), "Widget" | "Popup" | "Screen" | "Movie" | "RichMedia" | "3D" | "PrinterMark" | "TrapNet" | "Watermark") {
                continue;
            }
            if comments_full {
                continue;
            }
            let rect = rect4(self.resolve(d.get(b"Rect").unwrap_or(&Object::Null)));
            let mut color = match d.get(b"C").map(|o| self.resolve(o)) {
                Ok(Object::Array(c)) if c.len() == 3 => {
                    let f = |i: usize| c[i].as_float().unwrap_or(0.0);
                    Some([f(0), f(1), f(2)])
                }
                _ => None,
            };
            // A text box's /C is its background; its text colour is in /DA.
            if subtype == "FreeText"
                && let Some(da) = self.text(d, b"DA")
            {
                let t: Vec<&str> = da.split_whitespace().collect();
                if let Some(i) = t.iter().position(|x| *x == "rg")
                    && i >= 3
                {
                    let f = |k: usize| t[k].parse::<f32>().unwrap_or(0.0);
                    color = Some([f(i - 3), f(i - 2), f(i - 1)]);
                }
            }
            let quads = match d.get(b"QuadPoints").map(|o| self.resolve(o)) {
                Ok(Object::Array(q)) => {
                    let v: Vec<f32> = q.iter().map(|o| o.as_float().unwrap_or(0.0)).collect();
                    v.as_chunks::<8>().0.to_vec()
                }
                _ => Vec::new(),
            };
            let in_reply_to = d.get(b"IRT").ok().and_then(|o| self.dict(o)).and_then(|p| self.text(p, b"NM"));
            let parent = d.get(b"IRT").ok().and_then(|o| self.resolved_id(o)).and_then(|r| slots.get(&r).copied());
            let fill_sign = self.is_fill_sign(d, &subtype);
            // Keep in step with `pdfcraft_annot::summaries`.
            let key: &[u8] = if subtype == "Line" { b"L" } else { b"Vertices" };
            let points = match d.get(key).map(|o| self.resolve(o)) {
                Ok(Object::Array(v)) if matches!(subtype.as_str(), "Line" | "PolyLine" | "Polygon") => {
                    let v: Vec<f32> = v.iter().map(|o| o.as_float().unwrap_or(0.0)).collect();
                    let pts: Vec<[f32; 2]> = v.as_chunks::<2>().0.to_vec();
                    if subtype == "Line" && pts.len() != 2 { Vec::new() } else { pts }
                }
                _ => Vec::new(),
            };
            let annotation = Annotation {
                page,
                subtype,
                author: self.text(d, b"T"),
                // Acrobat ends lines with CR, which the interface would draw as nothing (keep
                // in step with `pdfcraft_annot::summaries`).
                contents: self.text(d, b"Contents").map(|s| s.replace("\r\n", "\n").replace('\r', "\n")),
                modified: self.text(d, b"M").map(|m| pretty_date(&m)),
                name: self.text(d, b"NM"),
                in_reply_to,
                parent,
                grouped: self.name(d, b"RT").as_deref() == Some("Group"),
                rect,
                color,
                index,
                state: self.text(d, b"State"),
                quads,
                locked: d.get(b"F").ok().and_then(|f| self.resolve(f).as_i64().ok()).unwrap_or(0) & 128 != 0,
                intent: self.name(d, b"IT"),
                fill_sign,
                no_rotate: d.get(b"F").is_ok_and(|f| draw_flags(self.resolve(f)) & 16 != 0),
                points,
            };
            kept = kept.saturating_add(retained(&annotation));
            if kept > MAX_ANNOTATION_BYTES {
                comments_full = true;
                continue;
            }
            info.annotations.push(annotation);
        }
        info.annotations.sort_by(|a, b| a.page.cmp(&b.page).then(b.rect[3].total_cmp(&a.rect[3])));
        link_replies(&mut info.annotations);
    }

    fn link(&self, page: usize, index: usize, d: &Dictionary, text_budget: &mut usize) -> Option<Link> {
        let rect = rect4(self.resolve(d.get(b"Rect").ok()?));
        let target = if let Ok(dest) = d.get(b"Dest") {
            let (page, view) = self.dest(dest, 0)?;
            LinkTarget::Page(page, view)
        } else {
            let a = d.get(b"A").ok().and_then(|a| self.dict(a))?;
            match self.name(a, b"S").as_deref() {
                Some("GoTo") => {
                    let (page, view) = self.dest(a.get(b"D").ok()?, 0)?;
                    LinkTarget::Page(page, view)
                }
                Some("URI") => {
                    let uri = a.get(b"URI").ok().and_then(|u| self.resolve(u).as_str().ok())?;
                    match spend(uri, text_budget, |b| String::from_utf8_lossy(b).into_owned()) {
                        Some(uri) => LinkTarget::Uri(uri),
                        None => LinkTarget::Other("URI".into()),
                    }
                }
                Some("SetOCGState") => self.layer_state(a),
                Some("GoToR") => self.remote_go_to(a).unwrap_or_else(|| LinkTarget::Other("GoToR".into())),
                Some("Launch") => self.launch_pdf(a).unwrap_or_else(|| LinkTarget::Other("Launch".into())),
                Some(other) => LinkTarget::Other(other.to_string()),
                None => return None,
            }
        };
        Some(Link { page, index, rect, target })
    }

    fn remote_go_to(&self, action: &Dictionary) -> Option<LinkTarget> {
        let file = self.action_file(action)?;
        let destination = match action.get(b"D") {
            Err(_) => RemoteDestination::Default,
            Ok(o) => match self.resolve(o) {
                Object::Array(items) => {
                    let page = items.first().and_then(|o| self.resolve(o).as_i64().ok()).and_then(|n| usize::try_from(n).ok())?;
                    let kind = items.get(1).map(|o| self.resolve(o)).and_then(|o| o.as_name().ok());
                    let operands: [Option<f32>; 4] = std::array::from_fn(|i| items.get(i + 2).and_then(|o| self.resolve(o).as_float().ok()));
                    RemoteDestination::Page { page, view: DestView::from_operands(kind, &operands) }
                }
                Object::Name(name) if name.len() <= MAX_REMOTE_DESTINATION_NAME_BYTES => {
                    let name = String::from_utf8_lossy(name).into_owned();
                    if name.is_empty() || name.len() > MAX_REMOTE_DESTINATION_NAME_BYTES {
                        return None;
                    }
                    RemoteDestination::Named(name)
                }
                Object::String(bytes, _) if bytes.len() <= MAX_REMOTE_DESTINATION_NAME_BYTES => {
                    let name = text_string(bytes);
                    if name.is_empty() || name.len() > MAX_REMOTE_DESTINATION_NAME_BYTES {
                        return None;
                    }
                    RemoteDestination::Named(name)
                }
                _ => return None,
            },
        };
        Some(LinkTarget::RemoteGoTo { launch: false, file, destination, new_window: self.new_window(action) })
    }

    /// The non-empty local file an action's `/F` names (a string or a file specification), shared
    /// by GoToR and Launch. A URL file system (`/FS /URL`) names no local file.
    fn action_file(&self, action: &Dictionary) -> Option<String> {
        if self.name(action, b"FS").as_deref() == Some("URL") {
            return None;
        }
        let file = match self.resolve(action.get(b"F").ok()?) {
            Object::String(bytes, _) => limited_pdf_string(bytes, MAX_REMOTE_FILE_SPEC_BYTES)?,
            Object::Dictionary(spec) => {
                if self.name(spec, b"FS").as_deref() == Some("URL") {
                    return None;
                }
                spec.get(b"UF")
                    .ok()
                    .and_then(|o| self.pdf_text(o, MAX_REMOTE_FILE_SPEC_BYTES))
                    .or_else(|| spec.get(b"F").ok().and_then(|o| self.pdf_text(o, MAX_REMOTE_FILE_SPEC_BYTES)))?
            }
            _ => return None,
        };
        (!file.is_empty()).then_some(file)
    }

    fn new_window(&self, action: &Dictionary) -> Option<bool> {
        match action.get(b"NewWindow").map(|o| self.resolve(o)) {
            Ok(Object::Boolean(value)) => Some(*value),
            _ => None,
        }
    }

    /// A Launch action (ISO 32000-2 §12.6.4.6, Table 207) whose `/F` names a `.pdf` file, read as
    /// the remote Go-To to that file's first page that it amounts to (#4168). Launching anything
    /// else, or with the deprecated `/Win` launch parameters (an application, an operation, its
    /// arguments), is not followed: a document must not start programs. `\` separators, which
    /// Windows writers put in such paths, are read as `/` (§7.11.2.1 separates components with
    /// `/`) when the path has no `/` of its own. Launch has no `/D`, so one present (written
    /// for GoToR, or junk) is ignored rather than checked as a destination.
    fn launch_pdf(&self, action: &Dictionary) -> Option<LinkTarget> {
        if action.has(b"Win") {
            return None;
        }
        let file = self.action_file(action)?;
        let new_window = self.new_window(action);
        let file = if file.contains('/') { file } else { file.replace('\\', "/") };
        let pdf = file.rsplit('/').next().is_some_and(|name| name.len() > 4 && name.to_ascii_lowercase().ends_with(".pdf"));
        pdf.then_some(LinkTarget::RemoteGoTo { launch: true, file, destination: RemoteDestination::Default, new_window })
    }

    fn pdf_text(&self, o: &Object, max_bytes: usize) -> Option<String> {
        match self.resolve(o) {
            Object::String(bytes, _) => limited_pdf_string(bytes, max_bytes),
            _ => None,
        }
    }

    /// A set-OCG-state action: `/State` is `ON`, `OFF` or `Toggle`, each followed by the groups
    /// it applies to; `/PreserveRB` is true unless it is `false`.
    fn layer_state(&self, a: &Dictionary) -> LinkTarget {
        let mut changes = Vec::new();
        if let Ok(Object::Array(items)) = a.get(b"State").map(|o| self.resolve(o)) {
            let mut op = None;
            for item in items.iter().take(MAX_LAYER_STATE) {
                match item {
                    Object::Name(n) => {
                        op = match n.as_slice() {
                            b"ON" => Some(LayerOp::On),
                            b"OFF" => Some(LayerOp::Off),
                            b"Toggle" => Some(LayerOp::Toggle),
                            _ => None,
                        }
                    }
                    Object::Reference(id) => changes.extend(op.map(|op| (op, *id))),
                    _ => {}
                }
            }
        }
        let preserve_rb = !matches!(a.get(b"PreserveRB").map(|o| self.resolve(o)), Ok(Object::Boolean(false)));
        LinkTarget::SetLayers { changes, preserve_rb }
    }

    // ── form fields ─────────────────────────────────────────────────────────────────────────

    pub(super) fn field(
        &self,
        o: &'a Object,
        parent: Option<(&str, Option<String>, Option<i64>)>,
        out: &mut Vec<Field>,
        seen: &mut HashSet<ObjectId>,
        depth: u32,
    ) {
        if depth > 32 {
            return;
        }
        if let Object::Reference(id) = o
            && !seen.insert(*id)
        {
            return;
        }
        let Some(d) = self.dict(o) else { return };
        let (parent_name, parent_ft, parent_ff) = parent.unwrap_or(("", None, None));
        let partial = self.text(d, b"T");
        let full = match (&partial, parent_name.is_empty()) {
            (Some(t), true) => t.clone(),
            (Some(t), false) => format!("{parent_name}.{t}"),
            (None, _) => parent_name.to_string(),
        };
        let ft = self.name(d, b"FT").or(parent_ft);
        let ff = d.get(b"Ff").ok().and_then(|x| x.as_i64().ok()).or(parent_ff);
        // Non-terminal: has kids that are fields (they carry /T). Widgets-only kids are terminal.
        let kids = match d.get(b"Kids").map(|k| self.resolve(k)) {
            Ok(Object::Array(k)) => k.as_slice(),
            _ => &[],
        };
        let field_kids: Vec<_> = kids.iter().filter(|k| self.dict(k).is_some_and(|kd| kd.get(b"T").is_ok())).collect();
        if !field_kids.is_empty() {
            for k in field_kids {
                self.field(k, Some((&full, ft.clone(), ff)), out, seen, depth + 1);
            }
            return;
        }
        let flags = ff.unwrap_or(0);
        let kind = match ft.as_deref() {
            Some("Tx") => FieldKind::Text,
            Some("Btn") if flags & (1 << 16) != 0 => FieldKind::PushButton,
            Some("Btn") if flags & (1 << 15) != 0 => FieldKind::Radio,
            Some("Btn") => FieldKind::CheckBox,
            Some("Ch") if flags & (1 << 17) != 0 => FieldKind::Combo,
            Some("Ch") => FieldKind::List,
            Some("Sig") => FieldKind::Signature,
            _ => FieldKind::Unknown,
        };
        let value = match d.get(b"V").map(|v| self.resolve(v)) {
            Ok(Object::Name(n)) => Some(String::from_utf8_lossy(n).into_owned()),
            Ok(Object::String(bytes, _)) => Some(text_string(bytes)),
            Ok(Object::Array(a)) => Some(
                a.iter()
                    .filter_map(|x| match self.resolve(x) {
                        Object::String(bytes, _) => Some(text_string(bytes)),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            Ok(Object::Dictionary(_)) if kind == FieldKind::Signature => Some("signed".into()),
            _ => None,
        };
        // The widget is either this dict (merged field/widget) or its first kid.
        let widget = if d.get(b"Rect").is_ok() { Some(d) } else { kids.first().and_then(|k| self.dict(k)) };
        let page = widget.and_then(|w| w.get(b"P").ok()).and_then(|p| self.page_of(p)).or_else(|| self.find_widget_page(o, kids));
        let has_actions = d.get(b"AA").is_ok() || widget.is_some_and(|w| w.get(b"AA").is_ok() || w.get(b"A").is_ok());
        let rect = widget.and_then(|w| w.get(b"Rect").ok()).map(|r| rect4(self.resolve(r)));
        out.push(Field { name: full, kind, value, page, tooltip: self.text(d, b"TU"), has_actions, rect });
    }

    /// The first page whose `/Annots` lists the field or one of its kids.
    fn find_widget_page(&self, field: &Object, kids: &[Object]) -> Option<usize> {
        let pages = self.annotation_pages.get_or_init(|| self.annotation_pages());
        std::iter::once(field).chain(kids).filter_map(|o| if let Object::Reference(id) = o { pages.get(id).copied() } else { None }).min()
    }

    /// Each annotation listed in a page's `/Annots`, with the first page that lists it.
    fn annotation_pages(&self) -> HashMap<ObjectId, usize> {
        let mut pages = HashMap::new();
        for (idx, &pid) in self.page_ids.iter().enumerate() {
            let Some(annots) = self.doc.get_dictionary(pid).ok().and_then(|d| d.get(b"Annots").ok()) else { continue };
            let Ok(annots) = self.resolve(annots).as_array() else { continue };
            for a in annots {
                if let Object::Reference(id) = a {
                    pages.entry(*id).or_insert(idx);
                }
            }
        }
        pages
    }

    /// The topmost field above widget `id` (itself without a `/Parent`), or `None` when it or an
    /// ancestor is already reachable from the `/Fields` tree, or a `/Parent` loop hides the top.
    pub(super) fn unlisted_field(&self, id: ObjectId, seen: &HashSet<ObjectId>) -> Option<ObjectId> {
        let mut top = id;
        let mut visited = HashSet::from([id]);
        loop {
            let o = Object::Reference(top);
            let d = self.dict(&o)?;
            let parent = match d.get(b"Parent") {
                Ok(Object::Reference(p)) => Some(*p),
                _ => None,
            };
            let Some(p) = parent else {
                return if seen.contains(&top) { None } else { Some(top) };
            };
            if !visited.insert(p) || visited.len() > 64 {
                return None;
            }
            if seen.contains(&p) {
                return None;
            }
            top = p;
        }
    }
}

/// An annotation's `/F` as the renderer reads it: a real is truncated, and a value that isn't a
/// `u32` counts as no flags.
fn draw_flags(o: &Object) -> u32 {
    let f = match o {
        Object::Integer(i) => *i,
        Object::Real(r) => *r as i64,
        _ => 0,
    };
    u32::try_from(f).unwrap_or(0)
}

fn rect4(o: &Object) -> [f32; 4] {
    match o {
        Object::Array(a) if a.len() == 4 => {
            let v: Vec<f32> = a.iter().map(|x| x.as_float().unwrap_or(0.0)).collect();
            [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])]
        }
        _ => [0.0; 4],
    }
}
