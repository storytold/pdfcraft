use super::annotations::MAX_LINK_ATTACHMENT_TEXT_BYTES;
use super::model::{AttachmentSource, PageInfo, Xfa};
use super::navigation::text_string;
use super::{
    Arc, AssertUnwindSafe, DecryptionError, Dictionary, DocInfo, Document, HashMap, HashSet, LazyStructure, LoadOptions, LoadPdfError, Object,
    ObjectId, OnceCell, OpenError, Pdf, Structure, catch_unwind,
};

/// lopdf decodes object and cross-reference streams while it loads, with no limit unless one is
/// set: a few hundred bytes of nested FlateDecode then inflate to gigabytes. Real object and
/// xref streams are far below this.
const LOAD_STREAM_LIMIT: usize = 256 << 20;

pub(super) fn load_options(password: Option<&str>) -> LoadOptions {
    LoadOptions { password: password.map(str::to_owned), max_decompressed_size: Some(LOAD_STREAM_LIMIT), ..LoadOptions::default() }
}

/// Check that `bytes` still open as the same document: the parser reads them and the page
/// count matches. Content-only edits skip the full [`inspect`] (seconds on very large files)
/// but must still catch an edit that produced something the viewer cannot display, so the
/// caller can roll it back. Never panics: parser crashes are caught and reported.
pub fn check_pages(bytes: Arc<Vec<u8>>, password: Option<&str>, expected: usize) -> Result<(), OpenError> {
    let pdf = catch_unwind(AssertUnwindSafe(|| Pdf::new_with_password(bytes, password.unwrap_or(""))))
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
    let pages = catch_unwind(AssertUnwindSafe(|| pdf.pages().len()))
        .map_err(|p| OpenError::Invalid(format!("the page tree could not be read: {}", crate::raster::panic_message(&p))))?;
    if pages != expected {
        return Err(OpenError::Invalid(format!("the edit changed the page count from {expected} to {pages}")));
    }
    Ok(())
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
    // Geometry no longer needs the renderer parser. Do not overlap its allocations with
    // structural inspection, including the compatibility fallback for damaged files.
    drop(pdf);
    inspect_structure(&mut info, |tmp| {
        // The lazy reader first; lopdf's repair/tolerance path when it can't expose the same
        // page tree or an object a panel needs. No partial result is published.
        let lazy = catch_unwind(AssertUnwindSafe(|| {
            let doc = LazyStructure::new(bytes.clone(), password).ok()?;
            let mut t = DocInfo { pages: tmp.pages.clone(), ..Default::default() };
            let inspector = Inspector::new(&doc);
            if inspector.page_index.len() != t.pages.len() {
                return None;
            }
            inspector.fill(&mut t);
            (!doc.failed()).then_some(t)
        }))
        .ok()
        .flatten();
        if let Some(t) = lazy {
            *tmp = t;
            return Ok(());
        }
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
pub(super) fn inspect_structure(info: &mut DocInfo, fill: impl FnOnce(&mut DocInfo) -> Result<(), String>) {
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

pub(super) struct Inspector<'a> {
    pub(super) doc: &'a dyn Structure,
    pub(super) page_ids: Vec<ObjectId>,
    pub(super) page_index: HashMap<ObjectId, usize>,
    /// Named destinations (`/Names /Dests` tree), keyed by raw string bytes, built once.
    /// Looking each name up by walking the tree was O(links × names): minutes on manuals.
    pub(super) named: HashMap<Vec<u8>, &'a Object>,
    /// The page of each annotation (the first page that lists it), built when a field's widget
    /// has no `/P`. Searching every page's `/Annots` for each such field was O(fields ×
    /// annotations): a second on a form of 8,000 fields.
    pub(super) annotation_pages: OnceCell<HashMap<ObjectId, usize>>,
}

impl<'a> Inspector<'a> {
    pub(super) fn new(doc: &'a dyn Structure) -> Self {
        let page_ids = doc.page_ids();
        let page_index = page_ids.iter().copied().enumerate().map(|(index, id)| (id, index)).collect();
        let mut me = Self { doc, page_ids, page_index, named: HashMap::new(), annotation_pages: OnceCell::new() };
        let mut named = HashMap::new();
        if let Some(tree) =
            doc.catalog().ok().and_then(|c| c.get(b"Names").ok()).and_then(|o| me.dict(o)).and_then(|n| n.get(b"Dests").ok()).and_then(|o| me.dict(o))
        {
            me.name_tree_raw(tree, &mut HashSet::new(), 0, &mut named);
        }
        me.named = named;
        me
    }

    pub(super) fn fill(&self, info: &mut DocInfo) {
        // lopdf drops /Encrypt from the trailer once it has decrypted the file.
        info.encrypted = self.doc.encrypted();
        if let Some(d) = self.doc.trailer().get(b"Info").ok().and_then(|o| self.dict(o)) {
            info.title = self.text(d, b"Title");
            info.author = self.text(d, b"Author");
            info.subject = self.text(d, b"Subject");
            info.keywords = self.text(d, b"Keywords");
            info.creator = self.text(d, b"Creator");
            info.producer = self.text(d, b"Producer");
            const STANDARD: [&[u8]; 9] =
                [b"Title", b"Author", b"Subject", b"Keywords", b"Creator", b"Producer", b"CreationDate", b"ModDate", b"Trapped"];
            // The same rule as the engine's list after an edit (`pdfcraft_organize::custom_info`,
            // trimmed and without empty values): text strings only, so a name value is not a
            // custom property, and the cap counts listed entries.
            info.custom = d
                .iter()
                .filter(|(k, _)| !STANDARD.contains(&k.as_slice()))
                .filter_map(|(k, v)| match self.resolve(v) {
                    Object::String(bytes, _) => {
                        let v = text_string(bytes).trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string();
                        (!v.is_empty()).then(|| (String::from_utf8_lossy(k).into_owned(), v))
                    }
                    _ => None,
                })
                .take(256)
                .collect();
        }
        let Ok(catalog) = self.doc.catalog() else { return };
        info.tagged = catalog
            .get(b"MarkInfo")
            .ok()
            .and_then(|o| self.dict(o))
            .and_then(|m| m.get(b"Marked").ok())
            // Any value may be written as an indirect reference, `/Marked 9 0 R` included, so the
            // flag is resolved before it is read as a boolean (#3671).
            .map(|o| self.resolve(o))
            .and_then(|o| o.as_bool().ok())
            .unwrap_or(false);
        match self.page_labels(catalog, &mut info.pages) {
            Ok(notes) => info.warnings.extend(notes.into_iter().map(|note| format!("Page labels: {note}."))),
            Err(reason) => info.warnings.push(format!("Page labels were skipped: {reason}; using physical page numbers.")),
        }
        if let Some(first) = catalog.get(b"Outlines").ok().and_then(|o| self.dict(o)).and_then(|d| d.get(b"First").ok()) {
            (info.outline, info.outline_more) = self.outline_window(first);
        }
        let mut text_budget = MAX_LINK_ATTACHMENT_TEXT_BYTES;
        self.annotations(info, &mut text_budget);
        self.transitions(info);
        let mut field_seen = HashSet::new();
        if let Some(form) = catalog.get(b"AcroForm").ok().and_then(|o| self.dict(o))
            && let Ok(fields) = form.get(b"Fields").and_then(|o| self.resolve(o).as_array())
        {
            for f in fields {
                self.field(f, None, &mut info.fields, &mut field_seen, 0);
            }
        }
        // Widgets no `/Fields` entry reaches are fields too (Acrobat and the browsers fill them);
        // each is adopted through its topmost unlisted `/Parent`, so a split field stays one.
        for &pid in &self.page_ids {
            let Ok(page_dict) = self.doc.get_dictionary(pid) else { continue };
            let Ok(Object::Array(annots)) = page_dict.get(b"Annots").map(|o| self.resolve(o)) else { continue };
            for a in annots {
                let Some(d) = self.dict(a) else { continue };
                if self.name(d, b"Subtype").as_deref() != Some("Widget")
                    || (d.get(b"FT").is_err() && d.get(b"T").is_err() && d.get(b"Parent").is_err())
                {
                    continue;
                }
                if let Object::Reference(id) = a
                    && let Some(root) = self.unlisted_field(*id, &field_seen)
                {
                    self.field(&Object::Reference(root), None, &mut info.fields, &mut field_seen, 0);
                }
            }
        }
        info.has_javascript |= info.fields.iter().any(|f| f.has_actions);
        let needs_rendering = catalog.get(b"NeedsRendering").ok().and_then(|o| self.resolve(o).as_bool().ok()).unwrap_or(false);
        if let Some(xfa) = catalog.get(b"AcroForm").ok().and_then(|o| self.dict(o)).and_then(|form| form.get(b"XFA").ok()) {
            // A form with no fields is taken to be laid out from its template, unless its
            // configuration says it is a static form (#3180): Designer writes
            // `<dynamicRender>forbidden</dynamicRender>` for one, whose pages are the form.
            let static_form = self.xfa_dynamic_render(xfa).as_deref() == Some("forbidden");
            info.xfa = Some(if needs_rendering || (info.fields.is_empty() && !static_form) { Xfa::Dynamic } else { Xfa::Static });
        }
        self.layers(catalog, &mut info.layers);
        info.layer_groups = self.layer_groups(catalog);
        self.fonts(&mut info.fonts);
        if let Some(names) = catalog.get(b"Names").ok().and_then(|o| self.dict(o)) {
            if let Some(ef) = names.get(b"EmbeddedFiles").ok().and_then(|o| self.dict(o)) {
                let mut seen = HashSet::new();
                for (name, spec) in self.name_tree(ef, &mut seen, 0, &mut text_budget) {
                    // The key is kept a second time when it is also the name.
                    let fallback = |budget: &mut usize| spend(name.as_bytes(), budget, |_| name.clone());
                    info.attachments.push(self.attachment(spec, AttachmentSource::Document { key: name.clone() }, &mut text_budget, fallback));
                }
            }
            info.has_javascript |= names.get(b"JavaScript").is_ok();
        }
        if let Ok(action) = catalog.get(b"OpenAction")
            && let Some(dict) = self.dict(action)
            && dict.get(b"S").ok().and_then(|o| self.resolve(o).as_name().ok()) == Some(&b"JavaScript"[..])
        {
            info.has_javascript = true;
        }
        info.open_dest = catalog.get(b"OpenAction").ok().and_then(|o| self.open_dest(o));
    }

    pub(super) fn resolve(&self, o: &'a Object) -> &'a Object {
        self.doc.dereference(o).map(|(_, o)| o).unwrap_or(o)
    }

    /// The object a reference ends at after following any chain of references; `None` for a
    /// direct object or a broken reference.
    pub(super) fn resolved_id(&self, o: &Object) -> Option<ObjectId> {
        self.doc.dereference(o).ok().and_then(|(id, _)| id)
    }

    pub(super) fn dict(&self, o: &'a Object) -> Option<&'a Dictionary> {
        match self.resolve(o) {
            Object::Dictionary(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }

    pub(super) fn text(&self, d: &Dictionary, key: &[u8]) -> Option<String> {
        let s = trimmed_text(self.resolve(d.get(key).ok()?))?;
        (!s.is_empty()).then_some(s)
    }

    /// [`Self::text`], converted and spent from `budget` as [`spend`] does, so text that trims
    /// short or to nothing still spends its raw length.
    pub(super) fn budgeted_text(&self, d: &Dictionary, key: &[u8], budget: &mut usize) -> Option<String> {
        let o = self.resolve(d.get(key).ok()?);
        let (Object::String(bytes, _) | Object::Name(bytes)) = o else { return None };
        let text = spend(bytes, budget, |_| trimmed_text(o).unwrap_or_default())?;
        (!text.is_empty()).then_some(text)
    }

    pub(super) fn name(&self, d: &Dictionary, key: &[u8]) -> Option<String> {
        self.resolve(d.get(key).ok()?).as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned())
    }
}

/// A string or name value as text, without surrounding NULs and white space.
fn trimmed_text(o: &Object) -> Option<String> {
    let s = match o {
        Object::String(bytes, _) => text_string(bytes),
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        _ => return None,
    };
    Some(s.trim_matches('\0').trim().to_string())
}

/// `convert(bytes)` when the result fits in `budget`, which it then spends by the longer of the raw
/// and converted lengths. Bytes longer than the budget are not converted. A conversion that comes
/// out too long spends the raw length, so a string many entries share is converted only until the
/// budget runs out.
pub(super) fn spend(bytes: &[u8], budget: &mut usize, convert: impl FnOnce(&[u8]) -> String) -> Option<String> {
    if bytes.len() > *budget {
        return None;
    }
    let text = convert(bytes);
    if text.len() > *budget {
        *budget = budget.saturating_sub(bytes.len());
        return None;
    }
    // The conversion's work follows the raw length, even when the text it keeps is shorter.
    *budget = budget.saturating_sub(text.len().max(bytes.len()));
    Some(text)
}

pub(super) fn limited_pdf_string(bytes: &[u8], max_bytes: usize) -> Option<String> {
    if bytes.len() > max_bytes {
        return None;
    }
    let text = text_string(bytes);
    (text.len() <= max_bytes).then_some(text)
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

#[cfg(test)]
mod open_action_javascript_tests {
    use std::sync::Arc;

    use super::inspect;

    /// One-page PDF whose catalog points its /OpenAction at object 4 (written by the caller).
    fn pdf_with_open_action(action: &[u8]) -> Vec<u8> {
        let objs: Vec<&[u8]> = vec![
            b"<< /Type /Catalog /Pages 2 0 R /OpenAction 4 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>",
            action,
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    #[test]
    fn open_action_javascript_sets_has_javascript() {
        let bytes = pdf_with_open_action(b"<< /S /JavaScript /JS (app.alert('hi')) >>");
        let info = inspect(Arc::new(bytes), None).expect("inspects");
        assert!(info.has_javascript, "an /OpenAction /JavaScript is JavaScript");
    }

    #[test]
    fn open_action_goto_does_not_set_has_javascript() {
        let bytes = pdf_with_open_action(b"<< /S /GoTo /D [3 0 R /Fit] >>");
        let info = inspect(Arc::new(bytes), None).expect("inspects");
        assert!(!info.has_javascript, "a /GoTo /OpenAction is not JavaScript");
    }
}
