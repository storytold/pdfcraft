use super::core::{limited_pdf_string, pretty_date, spend};
use super::model::{Annotation, AttachmentSource, DestView, Field, FieldKind, LayerOp, Link, LinkTarget, RemoteDestination, link_replies};
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
            // A callout: its arrow point and knee, and its text box (keep in step with
            // `pdfcraft_annot::summaries`).
            let (points, text_box) = match self.callout(d, rect) {
                Some((lead, text_box)) if subtype == "FreeText" => (lead, Some(text_box)),
                _ => (points, None),
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
                text_box,
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

    /// A callout's arrow point and knee (`/CL`, four or six numbers, without the end that meets
    /// the text box) and its text box (`rect` inset by `/RD`); ISO 32000-2 Table 177. `None` when
    /// either is missing or unusable.
    fn callout(&self, d: &Dictionary, rect: [f32; 4]) -> Option<(Vec<[f32; 2]>, [f32; 4])> {
        let nums = |key: &[u8]| -> Option<Vec<f32>> {
            match self.resolve(d.get(key).ok()?) {
                // Four or six numbers (Table 177). A longer array is not a callout line, and is
                // not read.
                Object::Array(v) if v.len() <= 6 => v.iter().map(|o| self.resolve(o).as_float().ok()).collect(),
                _ => None,
            }
        };
        let cl = nums(b"CL").filter(|l| matches!(l.len(), 4 | 6) && l.iter().all(|x| x.is_finite()))?;
        let rd = nums(b"RD").filter(|v| v.len() == 4 && v.iter().all(|x| x.is_finite() && *x >= 0.0))?;
        let tb = [rect[0] + rd[0], rect[1] + rd[1], rect[2] - rd[2], rect[3] - rd[3]];
        if tb[2] - tb[0] < 1.0 || tb[3] - tb[1] < 1.0 {
            return None;
        }
        let (_, lead) = cl.as_chunks::<2>().0.split_last()?;
        Some((lead.to_vec(), tb))
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
