//! Document inspection: everything panels need that is not pixels.
//!
//! Page geometry comes from hayro (which resolves inheritance and rotation); the rest comes from
//! lopdf (bootstrap, replaced by `pdfcraft-model` in M2). Inspection is *tolerant*: when lopdf
//! cannot load a file that hayro can render, panels are simply empty and `warnings` says why.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use std::panic::{AssertUnwindSafe, catch_unwind};

use hayro::hayro_syntax::DecryptionError;
use hayro::hayro_syntax::LoadPdfError;
use hayro::hayro_syntax::Pdf;
use lopdf::{Dictionary, Document, LoadOptions, Object, ObjectId};

use crate::OpenError;

/// lopdf decodes object and cross-reference streams while it loads, with no limit unless one is
/// set: a few hundred bytes of nested FlateDecode then inflate to gigabytes. Real object and
/// xref streams are far below this.
const LOAD_STREAM_LIMIT: usize = 256 << 20;

/// At most this many `/State` entries of a set-layer-visibility action are read.
const MAX_LAYER_STATE: usize = 1024;

/// At most this many layers are read from all of `/RBGroups` together.
const MAX_LAYER_GROUP_ENTRIES: usize = 4096;

fn load_options(password: Option<&str>) -> LoadOptions {
    LoadOptions { password: password.map(str::to_owned), max_decompressed_size: Some(LOAD_STREAM_LIMIT), ..LoadOptions::default() }
}

#[derive(Clone, Debug, Default)]
pub struct DocInfo {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
    pub pdf_version: String,
    pub file_size: usize,
    pub encrypted: bool,
    pub tagged: bool,
    pub has_javascript: bool,
    /// The form is (also) an XFA form, which isn't read yet (`form.xfa-*` in the parity list).
    pub xfa: Option<Xfa>,
    pub pages: Vec<PageInfo>,
    pub outline: Vec<OutlineItem>,
    pub annotations: Vec<Annotation>,
    pub fields: Vec<Field>,
    pub links: Vec<Link>,
    pub layers: Vec<Layer>,
    /// Radio-button layer groups (`/OCProperties /D /RBGroups`): turning one layer of a group on
    /// turns the others off.
    pub layer_groups: Vec<Vec<(u32, u16)>>,
    pub fonts: Vec<FontInfo>,
    pub attachments: Vec<Attachment>,
    pub warnings: Vec<String>,
}

/// What kind of XFA form a document has (`/AcroForm /XFA`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Xfa {
    /// The pages and fields are ordinary PDF; the XFA packets describe the same form (and hold
    /// its data, which Acrobat shows in preference to the fields).
    Static,
    /// The form is laid out from the XFA packets at open (`/NeedsRendering`, or no fields): the
    /// PDF pages are only a placeholder.
    Dynamic,
}

#[derive(Clone, Debug)]
pub struct PageInfo {
    /// Displayed size in points, after `/Rotate` and `/UserUnit`.
    pub width: f32,
    pub height: f32,
    /// Page label (`/PageLabels`), falling back to the 1-based page number.
    pub label: String,
    /// Effective crop box in user space [x0, y0, x1, y1] (the visible region).
    pub crop: [f32; 4],
    /// Clockwise page rotation in degrees (0, 90, 180, 270).
    pub rotation: u16,
}

impl PageInfo {
    /// A point in view space (points, y down, after `/Rotate`; what the text layer uses) →
    /// PDF user space.
    pub fn view_to_user(&self, x: f32, y: f32) -> [f32; 2] {
        let [cx0, cy0, cx1, cy1] = self.crop;
        let (a, b) = (x / self.width.max(1e-3), y / self.height.max(1e-3));
        let (u, v) = match self.rotation {
            90 => (b, 1.0 - a),
            180 => (1.0 - a, 1.0 - b),
            270 => (1.0 - b, a),
            _ => (a, b),
        };
        [cx0 + u * (cx1 - cx0), cy1 - v * (cy1 - cy0)]
    }

    /// PDF user space → view space (the inverse of [`Self::view_to_user`]).
    pub fn user_to_view(&self, x: f32, y: f32) -> [f32; 2] {
        let [cx0, cy0, cx1, cy1] = self.crop;
        let (u, v) = ((x - cx0) / (cx1 - cx0).max(1e-3), (cy1 - y) / (cy1 - cy0).max(1e-3));
        let (a, b) = match self.rotation {
            90 => (1.0 - v, u),
            180 => (1.0 - u, 1.0 - v),
            270 => (v, 1.0 - u),
            _ => (u, v),
        };
        [a * self.width, b * self.height]
    }

    /// A view-space rectangle → the normalized user-space rectangle covering the same area of the
    /// page (`[x0, y0, x1, y1]`, `x0 <= x1`, `y0 <= y1`). Under `/Rotate` the corners swap roles,
    /// so a view rectangle's top-left is not in general the user rectangle's `[x0, y1]`.
    pub fn view_rect_to_user(&self, r: [f32; 4]) -> [f32; 4] {
        let (a, b) = (self.view_to_user(r[0], r[1]), self.view_to_user(r[2], r[3]));
        [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
    }

    /// A view-space rectangle as a text-markup quad in user space: top-left, top-right,
    /// bottom-left, bottom-right as read on screen.
    pub fn view_rect_to_quad(&self, r: [f32; 4]) -> [f64; 8] {
        let mut q = [0.0; 8];
        for (i, (x, y)) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])].into_iter().enumerate() {
            let [ux, uy] = self.view_to_user(x, y);
            q[2 * i] = ux as f64;
            q[2 * i + 1] = uy as f64;
        }
        q
    }
}

/// A clickable link annotation.
#[derive(Clone, Debug)]
pub struct Link {
    pub page: usize,
    pub rect: [f32; 4],
    pub target: LinkTarget,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LinkTarget {
    Page(usize),
    Uri(String),
    /// A set-layer-visibility action (`SetOCGState`, ISO 32000-2 §12.6.4.13): each change in
    /// order, naming the layer by its optional content group. With `preserve_rb`, a layer turned
    /// on turns off the other layers of its radio-button groups.
    SetLayers {
        changes: Vec<(LayerOp, (u32, u16))>,
        preserve_rb: bool,
    },
    Other(String),
}

/// What a set-layer-visibility action does to a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerOp {
    On,
    Off,
    Toggle,
}

#[derive(Clone, Debug)]
pub struct OutlineItem {
    pub title: String,
    pub page: Option<usize>,
    pub children: Vec<OutlineItem>,
    pub open: bool,
}

#[derive(Clone, Debug)]
pub struct Annotation {
    pub page: usize,
    pub subtype: String,
    pub author: Option<String>,
    pub contents: Option<String>,
    pub modified: Option<String>,
    /// `/NM` of this annotation, and of the one it replies to (`/IRT`), for threading.
    pub name: Option<String>,
    pub in_reply_to: Option<String>,
    /// Rect in PDF user space: [x0, y0, x1, y1].
    pub rect: [f32; 4],
    pub color: Option<[f32; 3]>,
    /// Position in the page's `/Annots` (how edits address the comment).
    pub index: usize,
    /// For a status reply (`/State`, Acrobat's "Set status"): the state it sets.
    pub state: Option<String>,
    /// Text markup quadrilaterals (`/QuadPoints`), 8 numbers each.
    pub quads: Vec<[f32; 8]>,
    /// The Locked flag (`/F` bit 8): the comment can't be moved, resized, restyled or deleted.
    pub locked: bool,
    /// `/IT`, the intent: `FreeTextCallout`, `PolygonCloud`, `FreeTextTypeWriter`…
    pub intent: Option<String>,
}

impl Annotation {
    /// A checkmark reply (`/StateModel /Marked`): "Marked" or "Unmarked" rather than a review status.
    pub fn is_mark(&self) -> bool {
        matches!(self.state.as_deref(), Some("Marked" | "Unmarked"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    CheckBox,
    Radio,
    PushButton,
    Combo,
    List,
    Signature,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub kind: FieldKind,
    pub value: Option<String>,
    pub page: Option<usize>,
    pub tooltip: Option<String>,
    pub has_actions: bool,
    /// Rect of the first widget, in user space.
    pub rect: Option<[f32; 4]>,
}

#[derive(Clone, Debug)]
pub struct Layer {
    /// Object number and generation of the optional content group.
    pub id: (u32, u16),
    pub name: String,
    pub visible: bool,
}

#[derive(Clone, Debug)]
pub struct Attachment {
    pub name: String,
    pub description: Option<String>,
    pub size: Option<usize>,
    pub source: AttachmentSource,
}

/// Where an attachment lives (so its bytes can be fetched on demand).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachmentSource {
    /// Document-level: `/Names /EmbeddedFiles` entry with this key.
    Document { key: String },
    /// A FileAttachment annotation: page index and position in the page's `/Annots`.
    Annotation { page: usize, index: usize },
}

/// A font used by the document (Document Properties ▸ Fonts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontInfo {
    /// BaseFont without the subset tag.
    pub name: String,
    /// Type1, TrueType, Type0, Type3, MMType1, CIDFontType0/2 …
    pub kind: String,
    pub embedded: bool,
    pub subset: bool,
    pub encoding: Option<String>,
}

/// Inspect a document. Fails only if the renderer itself cannot open the file (or needs a
/// password). Never panics: parser crashes are caught and reported.
pub fn inspect(bytes: Arc<Vec<u8>>, password: Option<&str>) -> Result<DocInfo, OpenError> {
    let pdf = catch_unwind(AssertUnwindSafe(|| Pdf::new_with_password(bytes.clone(), password.unwrap_or(""))))
        .map_err(|p| OpenError::Invalid(format!("the parser crashed: {}", crate::raster::panic_message(&p))))?
        .map_err(|e| match e {
            LoadPdfError::Decryption(DecryptionError::PasswordProtected) => {
                if password.is_some() {
                    OpenError::WrongPassword
                } else {
                    OpenError::NeedsPassword
                }
            }
            LoadPdfError::Decryption(other) => OpenError::Unsupported(format!("encryption not supported: {other:?}")),
            LoadPdfError::Invalid => OpenError::Invalid("no readable page tree or cross-reference data was found".into()),
        })?;
    let mut info = DocInfo { file_size: bytes.len(), pdf_version: version_label(&format!("{:?}", pdf.version())), ..Default::default() };
    let pages = catch_unwind(AssertUnwindSafe(|| {
        let mut out = Vec::new();
        for (i, page) in pdf.pages().iter().enumerate() {
            let (w, h) = page.render_dimensions();
            let c = page.intersected_crop_box();
            let rotation = match page.rotation() {
                hayro::hayro_syntax::page::Rotation::None => 0,
                hayro::hayro_syntax::page::Rotation::Horizontal => 90,
                hayro::hayro_syntax::page::Rotation::Flipped => 180,
                hayro::hayro_syntax::page::Rotation::FlippedHorizontal => 270,
            };
            let crop = [c.x0 as f32, c.y0 as f32, c.x1 as f32, c.y1 as f32];
            // Degenerate boxes still get a usable placeholder size so layout never divides by zero.
            let (w, h) = if w.is_finite() && h.is_finite() && w >= 1.0 && h >= 1.0 { (w, h) } else { (612.0, 792.0) };
            out.push(PageInfo { width: w, height: h, label: (i + 1).to_string(), crop, rotation });
        }
        out
    }));
    match pages {
        Ok(p) => info.pages = p,
        Err(p) => return Err(OpenError::Invalid(format!("the page tree could not be read: {}", crate::raster::panic_message(&p)))),
    }
    let options = load_options(password);
    inspect_structure(&mut info, |tmp| {
        let doc = Document::load_mem_with_options(&bytes, options).map_err(|e| e.to_string())?;
        Inspector::new(&doc).fill(tmp);
        Ok(())
    });
    if info.pages.is_empty() {
        return Err(OpenError::Invalid("the document has no pages".into()));
    }
    Ok(info)
}

// Commit auxiliary metadata only after inspection succeeds; renderer geometry is the fallback.
fn inspect_structure(info: &mut DocInfo, fill: impl FnOnce(&mut DocInfo) -> Result<(), String>) {
    let structure = catch_unwind(AssertUnwindSafe(|| {
        let mut tmp = DocInfo { pages: info.pages.clone(), ..Default::default() };
        fill(&mut tmp)?;
        Ok::<_, String>(tmp)
    }));
    match structure {
        Ok(Ok(mut filled)) => {
            filled.file_size = info.file_size;
            filled.pdf_version = std::mem::take(&mut info.pdf_version);
            *info = filled;
        }
        Ok(Err(e)) => info.warnings.push(format!("Some document structure (bookmarks, comments, fields) could not be read: {e}")),
        Err(p) => {
            info.warnings.push(format!("Document structure inspection crashed and was skipped: {}", crate::raster::panic_message(&p)));
        }
    }
}

struct Inspector<'a> {
    doc: &'a Document,
    page_index: HashMap<ObjectId, usize>,
    /// Named destinations (`/Names /Dests` tree), keyed by raw string bytes, built once.
    /// Looking each name up by walking the tree was O(links × names): minutes on manuals.
    named: HashMap<Vec<u8>, &'a Object>,
}

impl<'a> Inspector<'a> {
    fn new(doc: &'a Document) -> Self {
        let page_index = doc.get_pages().into_iter().map(|(n, id)| (id, n as usize - 1)).collect();
        let mut me = Self { doc, page_index, named: HashMap::new() };
        let mut named = HashMap::new();
        if let Some(tree) =
            doc.catalog().ok().and_then(|c| c.get(b"Names").ok()).and_then(|o| me.dict(o)).and_then(|n| n.get(b"Dests").ok()).and_then(|o| me.dict(o))
        {
            me.name_tree_raw(tree, &mut HashSet::new(), 0, &mut named);
        }
        me.named = named;
        me
    }

    fn fill(&self, info: &mut DocInfo) {
        // lopdf drops /Encrypt from the trailer once it has decrypted the file.
        info.encrypted = self.doc.is_encrypted() || self.doc.was_encrypted() || self.doc.trailer.get(b"Encrypt").is_ok();
        if let Some(d) = self.doc.trailer.get(b"Info").ok().and_then(|o| self.dict(o)) {
            info.title = self.text(d, b"Title");
            info.author = self.text(d, b"Author");
            info.subject = self.text(d, b"Subject");
            info.keywords = self.text(d, b"Keywords");
            info.creator = self.text(d, b"Creator");
            info.producer = self.text(d, b"Producer");
        }
        let Ok(catalog) = self.doc.catalog() else { return };
        info.tagged = catalog
            .get(b"MarkInfo")
            .ok()
            .and_then(|o| self.dict(o))
            .and_then(|m| m.get(b"Marked").ok())
            .and_then(|o| o.as_bool().ok())
            .unwrap_or(false);
        self.page_labels(catalog, &mut info.pages);
        if let Some(first) = catalog.get(b"Outlines").ok().and_then(|o| self.dict(o)).and_then(|d| d.get(b"First").ok()) {
            let mut seen = HashSet::new();
            info.outline = self.outline_siblings(first, &mut seen, 0);
        }
        self.annotations(info);
        if let Some(form) = catalog.get(b"AcroForm").ok().and_then(|o| self.dict(o))
            && let Ok(fields) = form.get(b"Fields").and_then(|o| self.resolve(o).as_array())
        {
            let mut seen = HashSet::new();
            for f in fields {
                self.field(f, None, &mut info.fields, &mut seen, 0);
            }
        }
        info.has_javascript |= info.fields.iter().any(|f| f.has_actions);
        let needs_rendering = catalog.get(b"NeedsRendering").ok().and_then(|o| self.resolve(o).as_bool().ok()).unwrap_or(false);
        if catalog.get(b"AcroForm").ok().and_then(|o| self.dict(o)).is_some_and(|form| form.get(b"XFA").is_ok()) {
            info.xfa = Some(if needs_rendering || info.fields.is_empty() { Xfa::Dynamic } else { Xfa::Static });
        }
        self.layers(catalog, &mut info.layers);
        info.layer_groups = self.layer_groups(catalog);
        self.fonts(&mut info.fonts);
        if let Some(names) = catalog.get(b"Names").ok().and_then(|o| self.dict(o)) {
            if let Some(ef) = names.get(b"EmbeddedFiles").ok().and_then(|o| self.dict(o)) {
                let mut seen = HashSet::new();
                for (name, spec) in self.name_tree(ef, &mut seen, 0) {
                    info.attachments.push(self.attachment(name.clone(), spec, AttachmentSource::Document { key: name }));
                }
            }
            info.has_javascript |= names.get(b"JavaScript").is_ok();
        }
    }

    // ── object helpers ──────────────────────────────────────────────────────────────────────

    fn resolve(&self, o: &'a Object) -> &'a Object {
        self.doc.dereference(o).map(|(_, o)| o).unwrap_or(o)
    }

    fn dict(&self, o: &'a Object) -> Option<&'a Dictionary> {
        match self.resolve(o) {
            Object::Dictionary(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }

    fn text(&self, d: &Dictionary, key: &[u8]) -> Option<String> {
        let o = self.resolve(d.get(key).ok()?);
        let s = match o {
            Object::String(..) => lopdf::decode_text_string(o).ok()?,
            Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
            _ => return None,
        };
        let s = s.trim_matches('\0').trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    fn name(&self, d: &Dictionary, key: &[u8]) -> Option<String> {
        self.resolve(d.get(key).ok()?).as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned())
    }

    fn page_of(&self, o: &Object) -> Option<usize> {
        match o {
            Object::Reference(id) => self.page_index.get(id).copied(),
            // Some producers write page *numbers* in remote-style destinations.
            Object::Integer(n) => usize::try_from(*n).ok(),
            _ => None,
        }
    }

    // ── destinations ────────────────────────────────────────────────────────────────────────

    fn dest_page(&self, dest: &Object, depth: u32) -> Option<usize> {
        if depth > 8 {
            return None;
        }
        match self.resolve(dest) {
            Object::Array(a) => a.first().and_then(|p| self.page_of(p)),
            Object::Dictionary(d) => d.get(b"D").ok().and_then(|d| self.dest_page(d, depth + 1)),
            Object::String(key, _) | Object::Name(key) => self.named_dest(key).and_then(|d| self.dest_page(d, depth + 1)),
            _ => None,
        }
    }

    fn named_dest(&self, key: &[u8]) -> Option<&'a Object> {
        let catalog = self.doc.catalog().ok()?;
        if let Some(v) = self.named.get(key) {
            return Some(v);
        }
        // PDF 1.1 style /Dests dictionary.
        catalog.get(b"Dests").ok().and_then(|o| self.dict(o)).and_then(|d| d.get(key).ok())
    }

    /// A name tree's leaves keyed by their raw key bytes (first occurrence wins).
    fn name_tree_raw(&self, node: &'a Dictionary, seen: &mut HashSet<*const Dictionary>, depth: u32, out: &mut HashMap<Vec<u8>, &'a Object>) {
        if depth > 32 || !seen.insert(node as *const _) {
            return;
        }
        if let Ok(Object::Array(pairs)) = node.get(b"Names").map(|o| self.resolve(o)) {
            for pair in pairs.chunks(2) {
                if let [k, v] = pair {
                    let key = match self.resolve(k) {
                        Object::String(s, _) => s.clone(),
                        Object::Name(n) => n.clone(),
                        _ => continue,
                    };
                    out.entry(key).or_insert(v);
                }
            }
        }
        if let Ok(Object::Array(kids)) = node.get(b"Kids").map(|o| self.resolve(o)) {
            for k in kids {
                if let Some(d) = self.dict(k) {
                    self.name_tree_raw(d, seen, depth + 1, out);
                }
            }
        }
    }

    fn name_tree(&self, node: &'a Dictionary, seen: &mut HashSet<*const Dictionary>, depth: u32) -> Vec<(String, &'a Object)> {
        let mut out = Vec::new();
        if depth > 32 || !seen.insert(node as *const _) {
            return out;
        }
        if let Ok(Object::Array(pairs)) = node.get(b"Names").map(|o| self.resolve(o)) {
            for pair in pairs.chunks(2) {
                if let [k, v] = pair {
                    let key = lopdf::decode_text_string(self.resolve(k)).unwrap_or_default();
                    out.push((key, v));
                }
            }
        }
        if let Ok(Object::Array(kids)) = node.get(b"Kids").map(|o| self.resolve(o)) {
            for k in kids {
                if let Some(d) = self.dict(k) {
                    out.extend(self.name_tree(d, seen, depth + 1));
                }
            }
        }
        out
    }

    // ── outline ─────────────────────────────────────────────────────────────────────────────

    fn outline_siblings(&self, first: &'a Object, seen: &mut HashSet<ObjectId>, depth: u32) -> Vec<OutlineItem> {
        let mut items = Vec::new();
        let mut cur = Some(first);
        while let Some(o) = cur {
            if let Object::Reference(id) = o
                && !seen.insert(*id)
            {
                break; // cycle
            }
            let Some(d) = self.dict(o) else { break };
            let page = d
                .get(b"Dest")
                .ok()
                .and_then(|dest| self.dest_page(dest, 0))
                .or_else(|| d.get(b"A").ok().and_then(|a| self.dict(a)).and_then(|a| a.get(b"D").ok()).and_then(|dest| self.dest_page(dest, 0)));
            let children = match (d.get(b"First").ok(), depth < 32) {
                (Some(f), true) => self.outline_siblings(f, seen, depth + 1),
                _ => Vec::new(),
            };
            let open = d.get(b"Count").ok().and_then(|c| c.as_i64().ok()).is_some_and(|c| c > 0);
            items.push(OutlineItem { title: self.text(d, b"Title").unwrap_or_default(), page, children, open });
            cur = d.get(b"Next").ok();
            if items.len() > 100_000 {
                break;
            }
        }
        items
    }

    // ── page labels (ISO 32000-2 §12.4.2) ───────────────────────────────────────────────────

    fn page_labels(&self, catalog: &Dictionary, pages: &mut [PageInfo]) {
        let Some(tree) = catalog.get(b"PageLabels").ok().and_then(|o| self.dict(o)) else { return };
        let mut ranges: BTreeMap<usize, &Dictionary> = BTreeMap::new();
        let mut stack = vec![(tree, 0u32)];
        while let Some((node, depth)) = stack.pop() {
            if let Ok(Object::Array(nums)) = node.get(b"Nums").map(|o| self.resolve(o)) {
                for pair in nums.chunks(2) {
                    if let [k, v] = pair
                        && let (Ok(start), Some(d)) = (self.resolve(k).as_i64(), self.dict(v))
                        && start >= 0
                    {
                        ranges.insert(start as usize, d);
                    }
                }
            }
            if depth < 32
                && let Ok(Object::Array(kids)) = node.get(b"Kids").map(|o| self.resolve(o))
            {
                stack.extend(kids.iter().filter_map(|k| self.dict(k)).map(|d| (d, depth + 1)));
            }
        }
        for (i, page) in pages.iter_mut().enumerate() {
            let Some((&start, d)) = ranges.range(..=i).next_back() else { continue };
            let first = d.get(b"St").ok().and_then(|o| o.as_i64().ok()).unwrap_or(1).max(1) as usize;
            let n = first + (i - start);
            let prefix = self.text(d, b"P").unwrap_or_default();
            let number = match self.name(d, b"S").as_deref() {
                Some("D") => n.to_string(),
                Some("R") => roman(n).to_uppercase(),
                Some("r") => roman(n),
                Some("A") => alpha(n).to_uppercase(),
                Some("a") => alpha(n),
                _ => String::new(),
            };
            page.label = format!("{prefix}{number}");
            if page.label.is_empty() {
                page.label = (i + 1).to_string();
            }
        }
    }

    // ── annotations ─────────────────────────────────────────────────────────────────────────

    fn annotations(&self, info: &mut DocInfo) {
        for (&id, &page) in &self.page_index {
            let Ok(page_dict) = self.doc.get_dictionary(id) else { continue };
            let Ok(Object::Array(annots)) = page_dict.get(b"Annots").map(|o| self.resolve(o)) else { continue };
            for (index, a) in annots.iter().enumerate() {
                let Some(d) = self.dict(a) else { continue };
                let Some(subtype) = self.name(d, b"Subtype") else { continue };
                if subtype == "FileAttachment"
                    && let Ok(fs) = d.get(b"FS")
                {
                    let name = self.text(d, b"Contents").unwrap_or_else(|| "attachment".into());
                    info.attachments.push(self.attachment(name, fs, AttachmentSource::Annotation { page, index }));
                }
                if subtype == "Link" {
                    if let Some(link) = self.link(page, d) {
                        if matches!(&link.target, LinkTarget::Other(s) if s == "JavaScript") {
                            info.has_javascript = true;
                        }
                        info.links.push(link);
                    }
                    continue;
                }
                if matches!(subtype.as_str(), "Widget" | "Popup") {
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
                info.annotations.push(Annotation {
                    page,
                    subtype,
                    author: self.text(d, b"T"),
                    contents: self.text(d, b"Contents"),
                    modified: self.text(d, b"M").map(|m| pretty_date(&m)),
                    name: self.text(d, b"NM"),
                    in_reply_to,
                    rect,
                    color,
                    index,
                    state: self.text(d, b"State"),
                    quads,
                    locked: d.get(b"F").ok().and_then(|f| self.resolve(f).as_i64().ok()).unwrap_or(0) & 128 != 0,
                    intent: self.name(d, b"IT"),
                });
            }
        }
        info.annotations.sort_by(|a, b| a.page.cmp(&b.page).then(b.rect[3].total_cmp(&a.rect[3])));
    }

    fn link(&self, page: usize, d: &Dictionary) -> Option<Link> {
        let rect = rect4(self.resolve(d.get(b"Rect").ok()?));
        let target = if let Ok(dest) = d.get(b"Dest") {
            LinkTarget::Page(self.dest_page(dest, 0)?)
        } else {
            let a = d.get(b"A").ok().and_then(|a| self.dict(a))?;
            match self.name(a, b"S").as_deref() {
                Some("GoTo") => LinkTarget::Page(self.dest_page(a.get(b"D").ok()?, 0)?),
                Some("URI") => {
                    LinkTarget::Uri(a.get(b"URI").ok().and_then(|u| self.resolve(u).as_str().ok()).map(|b| String::from_utf8_lossy(b).into_owned())?)
                }
                Some("SetOCGState") => self.layer_state(a),
                Some(other) => LinkTarget::Other(other.to_string()),
                None => return None,
            }
        };
        Some(Link { page, rect, target })
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

    fn field(
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
            Ok(v @ Object::String(..)) => lopdf::decode_text_string(v).ok(),
            Ok(Object::Array(a)) => Some(a.iter().filter_map(|x| lopdf::decode_text_string(self.resolve(x)).ok()).collect::<Vec<_>>().join(", ")),
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

    fn find_widget_page(&self, field: &Object, kids: &[Object]) -> Option<usize> {
        let targets: HashSet<ObjectId> =
            std::iter::once(field).chain(kids.iter()).filter_map(|o| if let Object::Reference(id) = o { Some(*id) } else { None }).collect();
        if targets.is_empty() {
            return None;
        }
        self.page_index.iter().find_map(|(&pid, &idx)| {
            let annots = self.doc.get_dictionary(pid).ok()?.get(b"Annots").ok()?;
            let arr = self.resolve(annots).as_array().ok()?;
            arr.iter().any(|a| matches!(a, Object::Reference(id) if targets.contains(id))).then_some(idx)
        })
    }

    // ── optional content ────────────────────────────────────────────────────────────────────

    fn layers(&self, catalog: &Dictionary, out: &mut Vec<Layer>) {
        let Some(props) = catalog.get(b"OCProperties").ok().and_then(|o| self.dict(o)) else { return };
        let config = props.get(b"D").ok().and_then(|o| self.dict(o));
        let off: HashSet<ObjectId> = config
            .and_then(|c| c.get(b"OFF").ok())
            .and_then(|o| self.resolve(o).as_array().ok())
            .map(|a| a.iter().filter_map(|x| x.as_reference().ok()).collect())
            .unwrap_or_default();
        let base_off = config.and_then(|c| self.name(c, b"BaseState")).as_deref() == Some("OFF");
        let on: HashSet<ObjectId> = config
            .and_then(|c| c.get(b"ON").ok())
            .and_then(|o| self.resolve(o).as_array().ok())
            .map(|a| a.iter().filter_map(|x| x.as_reference().ok()).collect())
            .unwrap_or_default();
        let Ok(Object::Array(ocgs)) = props.get(b"OCGs").map(|o| self.resolve(o)) else { return };
        for g in ocgs {
            let Some(d) = self.dict(g) else { continue };
            let id = g.as_reference().ok();
            let visible = match id {
                Some(id) if off.contains(&id) => false,
                Some(id) if on.contains(&id) => true,
                _ => !base_off,
            };
            let Some(id) = id else { continue };
            out.push(Layer { id, name: self.text(d, b"Name").unwrap_or_else(|| "Layer".into()), visible });
        }
    }

    /// The default configuration's radio-button groups (`/RBGroups`) of two or more layers.
    fn layer_groups(&self, catalog: &Dictionary) -> Vec<Vec<ObjectId>> {
        let groups = catalog
            .get(b"OCProperties")
            .ok()
            .and_then(|o| self.dict(o))
            .and_then(|p| p.get(b"D").ok())
            .and_then(|o| self.dict(o))
            .and_then(|c| c.get(b"RBGroups").ok())
            .and_then(|o| self.resolve(o).as_array().ok());
        let mut left = MAX_LAYER_GROUP_ENTRIES;
        let mut out = Vec::new();
        for g in groups.into_iter().flatten() {
            let Ok(members) = self.resolve(g).as_array() else { continue };
            let group: Vec<ObjectId> = members.iter().filter_map(|x| x.as_reference().ok()).take(left).collect();
            left = left.saturating_sub(group.len());
            if group.len() > 1 {
                out.push(group);
            }
            if left == 0 {
                break;
            }
        }
        out
    }

    fn fonts(&self, out: &mut Vec<FontInfo>) {
        let mut seen = HashSet::new();
        let mut pages: Vec<_> = self.page_index.iter().collect();
        pages.sort_by_key(|(_, i)| **i);
        for (&pid, _) in pages {
            let Ok(fonts) = self.doc.get_page_fonts(pid) else { continue };
            for (_, f) in fonts {
                let base = f
                    .get(b"BaseFont")
                    .ok()
                    .and_then(|o| o.as_name().ok())
                    .map(|n| String::from_utf8_lossy(n).into_owned())
                    .unwrap_or_else(|| "(unnamed)".into());
                let kind = self.name(f, b"Subtype").unwrap_or_default();
                let encoding = match f.get(b"Encoding").map(|o| self.resolve(o)) {
                    Ok(Object::Name(n)) => Some(String::from_utf8_lossy(n).into_owned()),
                    Ok(Object::Dictionary(_)) => Some("Custom".into()),
                    Ok(Object::Stream(_)) => Some("Embedded CMap".into()),
                    _ => None,
                };
                // Embedded: FontFile/2/3 in the descriptor (for Type0, in the descendant font).
                let descriptor_of = |d: &Dictionary| d.get(b"FontDescriptor").ok().and_then(|o| self.dict(o)).cloned();
                let desc = descriptor_of(f).or_else(|| {
                    f.get(b"DescendantFonts")
                        .ok()
                        .and_then(|o| self.resolve(o).as_array().ok())
                        .and_then(|a| a.first())
                        .and_then(|o| self.dict(o))
                        .and_then(descriptor_of)
                });
                let embedded =
                    kind == "Type3" || desc.is_some_and(|d| d.get(b"FontFile").is_ok() || d.get(b"FontFile2").is_ok() || d.get(b"FontFile3").is_ok());
                let subset = base.len() > 7 && base.as_bytes()[6] == b'+' && base[..6].bytes().all(|b| b.is_ascii_uppercase());
                let name = if subset { base[7..].to_string() } else { base };
                let info = FontInfo { name, kind, embedded, subset, encoding };
                if seen.insert((info.name.clone(), info.kind.clone(), info.embedded)) {
                    out.push(info);
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
    }

    fn attachment(&self, name: String, spec: &Object, source: AttachmentSource) -> Attachment {
        let d = self.dict(spec);
        let size =
            d.and_then(|d| d.get(b"EF").ok()).and_then(|o| self.dict(o)).and_then(|ef| ef.get(b"F").ok().or_else(|| ef.get(b"UF").ok())).and_then(
                |f| match self.resolve(f) {
                    Object::Stream(s) => s
                        .dict
                        .get(b"Params")
                        .ok()
                        .and_then(|p| self.dict(p))
                        .and_then(|p| p.get(b"Size").ok())
                        .and_then(|s| s.as_i64().ok())
                        .map(|s| s as usize)
                        .or(Some(s.content.len())),
                    _ => None,
                },
            );
        let display = d.and_then(|d| self.text(d, b"UF").or_else(|| self.text(d, b"F"))).unwrap_or(name);
        Attachment { name: display, description: d.and_then(|d| self.text(d, b"Desc")), size, source }
    }
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

fn roman(mut n: usize) -> String {
    const T: [(usize, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut s = String::new();
    for (v, r) in T {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

/// a..z, aa..zz, aaa.. (ISO 32000-2: "a to z for the first 26 pages, aa to zz for the next 26").
fn alpha(n: usize) -> String {
    let n = n.max(1) - 1;
    let letter = (b'a' + (n % 26) as u8) as char;
    std::iter::repeat_n(letter, n / 26 + 1).collect()
}

/// `D:20260930104512-04'00'` → `2026-09-30 10:45`.
/// A PDF date (`D:20261001123000Z`) as "2026-10-01 12:30"; other strings unchanged.
pub fn pretty_date(s: &str) -> String {
    let d = s.trim_start_matches("D:");
    if let Some(d) = d.get(..12).filter(|d| d.bytes().all(|b| b.is_ascii_digit())) {
        // Twelve ASCII digits make every slice below a UTF-8 character boundary.
        format!("{}-{}-{} {}:{}", &d[0..4], &d[4..6], &d[6..8], &d[8..10], &d[10..12])
    } else {
        s.to_string()
    }
}

/// Fetch an attachment's bytes (decoded). Capped at 1 GiB to defuse decompression bombs.
pub fn attachment_data(bytes: &[u8], password: Option<&str>, att: &Attachment) -> Result<Vec<u8>, String> {
    let run = || -> Result<Vec<u8>, String> {
        let options = load_options(password);
        let doc = Document::load_mem_with_options(bytes, options).map_err(|e| e.to_string())?;
        let insp = Inspector::new(&doc);
        let spec: &Object = match &att.source {
            AttachmentSource::Document { key } => {
                let names = doc.catalog().ok().and_then(|c| c.get(b"Names").ok()).and_then(|o| insp.dict(o)).ok_or("no /Names")?;
                let ef = names.get(b"EmbeddedFiles").ok().and_then(|o| insp.dict(o)).ok_or("no /EmbeddedFiles")?;
                let mut seen = HashSet::new();
                insp.name_tree(ef, &mut seen, 0).into_iter().find(|(k, _)| k == key).map(|(_, v)| v).ok_or("attachment not found")?
            }
            AttachmentSource::Annotation { page, index } => {
                let (&pid, _) = insp.page_index.iter().find(|(_, i)| **i == *page).ok_or("page not found")?;
                let annots = doc.get_dictionary(pid).map_err(|e| e.to_string())?.get(b"Annots").map_err(|e| e.to_string())?;
                let a = insp.resolve(annots).as_array().map_err(|e| e.to_string())?.get(*index).ok_or("annotation not found")?;
                insp.dict(a).and_then(|d| d.get(b"FS").ok()).ok_or("annotation has no file")?
            }
        };
        let fs = insp.dict(spec).ok_or("bad file specification")?;
        let ef = fs.get(b"EF").ok().and_then(|o| insp.dict(o)).ok_or("the file is not embedded (external reference)")?;
        let stream = ef.get(b"UF").ok().or_else(|| ef.get(b"F").ok()).map(|o| insp.resolve(o)).ok_or("no embedded stream")?;
        let stream = stream.as_stream().map_err(|e| e.to_string())?;
        if stream.dict.get(b"Filter").is_ok() {
            stream.decompressed_content_with_limit(1 << 30).map_err(|e| e.to_string())
        } else {
            Ok(stream.content.clone())
        }
    };
    catch_unwind(AssertUnwindSafe(run)).unwrap_or_else(|p| Err(format!("reading the attachment crashed: {}", crate::raster::panic_message(&p))))
}

#[cfg(test)]
mod tests {
    #[test]
    fn png_predictor_rows_longer_than_the_data_are_refused_before_allocating() {
        // Vendored lopdf patch: a fuzzed `/Columns 4294967295` allocated two 4 GiB rows before
        // reading any data. A row that cannot fit in the data is refused up front.
        let e = lopdf::filters::png::decode_frame(&[2, 0, 0, 0], 1, 64 << 20).unwrap_err();
        assert!(e.to_string().contains("longer than the data"), "{e}");
        // A real frame still decodes: two 3-byte rows, the second Up-filtered.
        assert_eq!(lopdf::filters::png::decode_frame(&[0, 1, 2, 3, 2, 1, 1, 1], 1, 3).unwrap(), [1, 2, 3, 2, 3, 4]);
    }

    #[test]
    fn view_and_user_space_round_trip_for_every_rotation() {
        for rotation in [0u16, 90, 180, 270] {
            let (w, h) = if rotation % 180 == 0 { (200.0, 300.0) } else { (300.0, 200.0) };
            let p = super::PageInfo { width: w, height: h, label: String::new(), crop: [10.0, 20.0, 210.0, 320.0], rotation };
            for (x, y) in [(0.0, 0.0), (15.0, 40.0), (w, h)] {
                let [ux, uy] = p.view_to_user(x, y);
                let [vx, vy] = p.user_to_view(ux, uy);
                assert!((vx - x).abs() < 1e-3 && (vy - y).abs() < 1e-3, "{rotation}: {x},{y} → {ux},{uy} → {vx},{vy}");
            }
        }
        // Unrotated: the view's top-left is the crop box's top-left.
        let p = super::PageInfo { width: 200.0, height: 300.0, label: String::new(), crop: [10.0, 20.0, 210.0, 320.0], rotation: 0 };
        assert_eq!(p.view_to_user(0.0, 0.0), [10.0, 320.0]);
        assert_eq!(p.view_rect_to_quad([0.0, 0.0, 10.0, 5.0]), [10.0, 320.0, 20.0, 320.0, 10.0, 315.0, 20.0, 315.0]);
    }

    use super::*;

    #[test]
    fn roman_and_alpha_labels() {
        assert_eq!(roman(4), "iv");
        assert_eq!(roman(1994), "mcmxciv");
        assert_eq!(alpha(1), "a");
        assert_eq!(alpha(27), "aa");
        assert_eq!(alpha(53), "aaa");
    }

    const ATTACHMENTS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(notes.txt) 6 0 R] >> >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [4 0 R] /Resources << /Font << /F1 9 0 R >> >> >> endobj
4 0 obj << /Type /Annot /Subtype /FileAttachment /Rect [10 10 30 30] /Contents (data.csv) /FS 8 0 R >> endobj
5 0 obj << /Type /EmbeddedFile /Length 11 >> stream
hello notes
endstream endobj
6 0 obj << /Type /Filespec /F (notes.txt) /UF (notes.txt) /EF << /F 5 0 R >> /Desc (Doc-level) >> endobj
7 0 obj << /Type /EmbeddedFile /Length 5 >> stream
a,b,c
endstream endobj
8 0 obj << /Type /Filespec /F (data.csv) /EF << /F 7 0 R >> >> endobj
9 0 obj << /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF";

    #[test]
    fn attachments_of_both_kinds_are_listed_and_extracted() {
        let bytes = Arc::new(ATTACHMENTS.to_vec());
        let info = inspect(bytes.clone(), None).expect("opens");
        let names: Vec<_> = info.attachments.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"notes.txt") && names.contains(&"data.csv"), "{names:?}");
        for a in &info.attachments {
            let data = attachment_data(&bytes, None, a).expect("extracts");
            let expected: &[u8] = if a.name == "notes.txt" { b"hello notes" } else { b"a,b,c" };
            assert_eq!(data, expected, "{}", a.name);
        }
        assert_eq!(info.fonts, vec![FontInfo { name: "Helvetica".into(), kind: "Type1".into(), embedded: false, subset: true, encoding: None }]);
    }

    const LAYERS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R 7 0 R] /D << /OFF [6 0 R] /RBGroups [[5 0 R 6 0 R] [7 0 R] 8 0 R] >> >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [10 0 R 11 0 R] >> endobj
5 0 obj << /Type /OCG /Name (Red) >> endobj
6 0 obj << /Type /OCG /Name (Green) >> endobj
7 0 obj << /Type /OCG /Name (Blue) >> endobj
8 0 obj [6 0 R 7 0 R] endobj
9 0 obj [/OFF 5 0 R] endobj
10 0 obj << /Type /Annot /Subtype /Link /Rect [10 10 50 30] /A << /S /SetOCGState /State [7 0 R /ON 6 0 R /Bogus 5 0 R /Toggle 5 0 R 1 7 0 R] /PreserveRB false >> >> endobj
11 0 obj << /Type /Annot /Subtype /Link /Rect [60 10 100 30] /A << /S /SetOCGState /State 9 0 R >> >> endobj
trailer << /Root 1 0 R >>
%%EOF";

    #[test]
    fn radio_button_layer_groups_are_read() {
        let info = inspect(Arc::new(LAYERS.to_vec()), None).expect("opens");
        let layers: Vec<_> = info.layers.iter().map(|l| (l.id, l.name.as_str(), l.visible)).collect();
        assert_eq!(layers, [((5, 0), "Red", true), ((6, 0), "Green", false), ((7, 0), "Blue", true)]);
        // A group of one constrains nothing; an indirect group is read.
        assert_eq!(info.layer_groups, [vec![(5, 0), (6, 0)], vec![(6, 0), (7, 0)]]);
    }

    #[test]
    fn links_read_set_layer_actions() {
        let info = inspect(Arc::new(LAYERS.to_vec()), None).expect("opens");
        let targets: Vec<_> = info.links.iter().map(|l| l.target.clone()).collect();
        use LayerOp::{Off, On, Toggle};
        assert_eq!(
            targets,
            [
                // Groups before the first name or after an unknown one are skipped, and so is
                // anything that isn't a group.
                LinkTarget::SetLayers { changes: vec![(On, (6, 0)), (Toggle, (5, 0)), (Toggle, (7, 0))], preserve_rb: false },
                // An indirect /State; /PreserveRB defaults to true.
                LinkTarget::SetLayers { changes: vec![(Off, (5, 0))], preserve_rb: true },
            ]
        );
    }

    #[test]
    fn structure_failure_preserves_renderer_pages() {
        for crash in [true, false] {
            let mut info = DocInfo {
                file_size: 321,
                pdf_version: "1.7".into(),
                pages: vec![PageInfo { width: 300.0, height: 200.0, label: "1".into(), crop: [10.0, 20.0, 210.0, 320.0], rotation: 90 }],
                ..Default::default()
            };
            inspect_structure(&mut info, |tmp| {
                tmp.pages.clear();
                tmp.title = Some("partially inspected".into());
                if crash {
                    panic!("synthetic structure inspection failure");
                }
                Err("synthetic structure inspection failure".into())
            });

            assert_eq!(info.pages.len(), 1);
            let page = &info.pages[0];
            assert_eq!((page.width, page.height, page.label.as_str(), page.crop, page.rotation), (300.0, 200.0, "1", [10.0, 20.0, 210.0, 320.0], 90));
            assert_eq!(info.file_size, 321);
            assert_eq!(info.pdf_version, "1.7");
            assert!(info.title.is_none());
            assert_eq!(info.warnings.len(), 1);
            assert!(info.warnings[0].contains("synthetic structure inspection failure"));
            assert_eq!(info.warnings[0].contains("crashed"), crash);
        }
    }

    #[test]
    fn invalid_dates_with_unicode_are_unchanged() {
        for input in ["", "D:", "D:20260930104", "D:202609x01045", "yesterday"] {
            assert_eq!(pretty_date(input), input);
        }
        // Cover every position before the 12-byte prefix, including characters that
        // straddle its end. None of these strings is an ASCII PDF date.
        for character in ['é', '€', '😀'] {
            for prefix_len in 0..12 {
                let input = format!("D:{}{character}123456789012", "1".repeat(prefix_len));
                assert_eq!(pretty_date(&input), input, "{character} after {prefix_len} digits");
            }
        }
    }

    #[test]
    fn unicode_annotation_date_does_not_prevent_opening() {
        use lopdf::dictionary;

        let date = "D:12345678901éX";
        let mut encoded_date = vec![0xFE, 0xFF];
        encoded_date.extend(date.encode_utf16().flat_map(u16::to_be_bytes));
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let annot_id = doc.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Text",
            "Rect" => vec![10.into(), 10.into(), 30.into(), 30.into()],
            "M" => Object::String(encoded_date, lopdf::StringFormat::Hexadecimal),
        });
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 300.into()],
            "Annots" => vec![annot_id.into()],
        });
        doc.objects.insert(
            pages_id,
            dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }
            .into(),
        );
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).expect("write synthetic fixture");

        let info = inspect(Arc::new(bytes), None).expect("opens despite a non-date /M string");
        assert_eq!(info.pages.len(), 1);
        assert_eq!((info.pages[0].width, info.pages[0].height), (200.0, 300.0));
        assert_eq!(info.annotations.len(), 1);
        assert_eq!(info.annotations[0].modified.as_deref(), Some(date));
        assert!(info.warnings.is_empty(), "{:?}", info.warnings);
    }

    #[test]
    fn dates_are_prettified() {
        assert_eq!(pretty_date("D:20260930104512-04'00'"), "2026-09-30 10:45");
        assert_eq!(pretty_date("yesterday"), "yesterday");
    }
}

/// "1.7" from the parser's version name ("Pdf17", "V1_7", …).
fn version_label(name: &str) -> String {
    let digits: Vec<char> = name.chars().filter(char::is_ascii_digit).collect();
    match digits[..] {
        [a, b] => format!("{a}.{b}"),
        _ => name.to_owned(),
    }
}

#[cfg(test)]
mod version_tests {
    #[test]
    fn version_labels_read_as_numbers() {
        assert_eq!(super::version_label("Pdf17"), "1.7");
        assert_eq!(super::version_label("V2_0"), "2.0");
        assert_eq!(super::version_label("Unknown"), "Unknown");
    }
}
