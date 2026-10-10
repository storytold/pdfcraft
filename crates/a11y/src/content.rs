//! The Document, Page Content and Forms rules: catalog settings, page content (tagged or
//! artifact, text, images, fonts) and annotations.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use pdfcraft_model::Page;

use crate::{Finding, Rule};

/// What the page content holds, per checked page.
#[derive(Debug, Default)]
pub(crate) struct Scan {
    pub has_text: bool,
    pub has_images: bool,
    /// Page → painting operations outside tagged or artifact content.
    pub untagged: BTreeMap<usize, usize>,
    /// Page → fonts used.
    pub fonts: BTreeMap<usize, BTreeSet<ObjRef>>,
}

const MAX_FORM_DEPTH: usize = 12;

impl Scan {
    pub fn run(doc: &Document, pages: &[Page], list: &[usize]) -> Scan {
        let mut s = Scan::default();
        for &p in list {
            let Some(page) = pages.get(p) else { continue };
            let res = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
            let data = page_content(doc, &page.dict);
            let mut w = Walk { doc, scan: &mut s, page: p, forms: HashSet::new() };
            w.content(&data, &res, false, 0);
        }
        s
    }
}

fn page_content(doc: &Document, page: &Dict) -> Vec<u8> {
    let Some(c) = page.get(b"Contents") else { return Vec::new() };
    match &*doc.resolve(c) {
        Object::Stream(s) => s.decoded().unwrap_or_default(),
        Object::Array(a) => a
            .iter()
            .flat_map(|x| match &*doc.resolve(x) {
                Object::Stream(s) => {
                    let mut v = s.decoded().unwrap_or_default();
                    v.push(b'\n');
                    v
                }
                _ => Vec::new(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

struct Walk<'a> {
    doc: &'a Document,
    scan: &'a mut Scan,
    page: usize,
    forms: HashSet<ObjRef>,
}

impl Walk<'_> {
    fn sub(&self, res: &Dict, key: &[u8]) -> Dict {
        res.get(key).map(|x| self.doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default()
    }

    /// Walk a content stream; `covered`: already inside tagged or artifact content.
    fn content(&mut self, data: &[u8], res: &Dict, covered: bool, depth: usize) {
        let (fonts, xobjects, props) = (self.sub(res, b"Font"), self.sub(res, b"XObject"), self.sub(res, b"Properties"));
        // Marked-content stack: whether each level is tagged or an artifact.
        let mut stack: Vec<bool> = Vec::new();
        let mut untagged = 0;
        for op in pdfcraft_content::parse(data).ops {
            let inside = covered || stack.last().copied().unwrap_or(false);
            match op.op.as_slice() {
                b"BMC" => stack.push(inside || op.name(0) == Some(b"Artifact")),
                b"BDC" => {
                    let marks = op.name(0) == Some(b"Artifact")
                        || match op.operands.get(1) {
                            Some(Object::Dict(d)) => d.contains(b"MCID"),
                            Some(Object::Name(n)) => {
                                props.get(n).map(|x| self.doc.resolve(x)).and_then(|x| x.as_dict().map(|d| d.contains(b"MCID"))).unwrap_or(false)
                            }
                            _ => false,
                        };
                    stack.push(inside || marks);
                }
                b"EMC" => {
                    stack.pop();
                }
                b"Tj" | b"TJ" | b"'" | b"\"" => {
                    self.scan.has_text = true;
                    untagged += usize::from(!inside);
                }
                b"Tf" => {
                    if let Some(r) = op.name(0).and_then(|n| fonts.get(n)).and_then(Object::as_ref) {
                        self.scan.fonts.entry(self.page).or_default().insert(r);
                    }
                }
                b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" | b"sh" => untagged += usize::from(!inside),
                b"BI" => {
                    self.scan.has_images = true;
                    untagged += usize::from(!inside);
                }
                b"Do" => {
                    let Some(r) = op.name(0).and_then(|n| xobjects.get(n)).and_then(Object::as_ref) else { continue };
                    let obj = self.doc.get(r);
                    let Object::Stream(x) = &*obj else { continue };
                    match x.dict.name(b"Subtype") {
                        Some(b"Image") => {
                            self.scan.has_images = true;
                            untagged += usize::from(!inside);
                        }
                        Some(b"Form") if depth < MAX_FORM_DEPTH && self.forms.insert(r) => {
                            let inner = x
                                .dict
                                .get(b"Resources")
                                .map(|o| self.doc.resolve(o))
                                .and_then(|o| o.as_dict().cloned())
                                .unwrap_or_else(|| res.clone());
                            let data = x.decoded().unwrap_or_default();
                            self.content(&data, &inner, inside, depth + 1);
                            self.forms.remove(&r);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        if untagged > 0 {
            *self.scan.untagged.entry(self.page).or_default() += untagged;
        }
    }
}

fn catalog(doc: &Document) -> Dict {
    doc.root().and_then(|r| doc.get(r).as_dict().cloned()).unwrap_or_default()
}

fn text_of(doc: &Document, d: &Dict, key: &[u8]) -> Option<String> {
    d.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_string().map(|s| s.to_text())).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

fn doc_finding(message: impl Into<String>) -> Vec<Finding> {
    vec![Finding { page: None, message: message.into() }]
}

pub(crate) fn document_rule(doc: &Document, rule: Rule, pages: &[Page], scan: &Scan) -> Vec<Finding> {
    let cat = catalog(doc);
    match rule {
        Rule::PermissionFlag => match doc.permissions() {
            // The file's own setting, whichever password opened it.
            Some(p) if (p.bits >> 9) & 1 == 0 => doc_finding("The security settings don't allow screen readers to read the text"),
            _ => Vec::new(),
        },
        Rule::ImageOnly => {
            if !scan.has_text && scan.has_images {
                doc_finding("No page has text: the document appears to be images of pages")
            } else {
                Vec::new()
            }
        }
        Rule::TaggedPdf => {
            let marked = cat.get(b"MarkInfo").map(|m| doc.resolve(m)).and_then(|m| m.as_dict().and_then(|d| d.get(b"Marked").cloned()));
            let mut out = Vec::new();
            if !cat.contains(b"StructTreeRoot") {
                out.push(Finding { page: None, message: "The document has no tags (structure tree)".into() });
            }
            if !matches!(marked, Some(Object::Bool(true))) {
                out.push(Finding { page: None, message: "The document isn't marked as tagged (MarkInfo Marked)".into() });
            }
            out
        }
        Rule::PrimaryLanguage => {
            if text_of(doc, &cat, b"Lang").is_none() {
                doc_finding("No document language is set")
            } else {
                Vec::new()
            }
        }
        Rule::Title => {
            let mut out = Vec::new();
            let info = doc.trailer().get(b"Info").map(|i| doc.resolve(i)).and_then(|i| i.as_dict().cloned()).unwrap_or_default();
            if text_of(doc, &info, b"Title").is_none() {
                out.push(Finding { page: None, message: "The document has no title".into() });
            }
            let shows = cat
                .get(b"ViewerPreferences")
                .map(|v| doc.resolve(v))
                .and_then(|v| v.as_dict().and_then(|d| d.get(b"DisplayDocTitle").map(|x| matches!(&*doc.resolve(x), Object::Bool(true)))))
                .unwrap_or(false);
            if !shows {
                out.push(Finding { page: None, message: "The window shows the file name, not the title (Initial View > Show)".into() });
            }
            out
        }
        Rule::Bookmarks => {
            let has = cat.get(b"Outlines").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().map(|d| d.contains(b"First"))).unwrap_or(false);
            if pages.len() > 20 && !has { doc_finding(format!("{} pages and no bookmarks", pages.len())) } else { Vec::new() }
        }
        _ => Vec::new(),
    }
}

const MULTIMEDIA: &[&[u8]] = &[b"Screen", b"Movie", b"Sound", b"RichMedia", b"3D"];

fn subtype_name(d: &Dict) -> String {
    String::from_utf8_lossy(d.name(b"Subtype").unwrap_or(b"Unknown")).into_owned()
}

pub(crate) fn page_rule(doc: &Document, rule: Rule, pages: &[Page], list: &[usize], scan: &Scan, tagged: bool) -> Vec<Finding> {
    let mut out = Vec::new();
    let annots = |p: &Page| -> Vec<(ObjRef, Dict)> {
        let a = p.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        a.iter()
            .filter_map(|x| x.as_ref().and_then(|r| doc.get(r).as_dict().cloned().map(|d| (r, d))))
            .filter(|(_, d)| d.name(b"Subtype") != Some(b"Popup"))
            .collect()
    };
    match rule {
        Rule::TaggedContent => {
            if !tagged && !scan.untagged.is_empty() {
                return doc_finding("The document isn't tagged, so none of its content is");
            }
            for (p, n) in &scan.untagged {
                out.push(Finding { page: Some(*p), message: format!("Page {}: {n} untagged content item{}", p + 1, if *n == 1 { "" } else { "s" }) });
            }
        }
        Rule::TaggedAnnotations | Rule::TaggedMultimedia | Rule::TaggedFormFields => {
            for &p in list {
                let Some(page) = pages.get(p) else { continue };
                for (_, a) in annots(page) {
                    let st = a.name(b"Subtype").unwrap_or(b"");
                    let applies = match rule {
                        Rule::TaggedFormFields => st == b"Widget",
                        Rule::TaggedMultimedia => MULTIMEDIA.contains(&st),
                        _ => st != b"Widget" && !MULTIMEDIA.contains(&st) && st != b"PrinterMark",
                    };
                    if applies && !(tagged && a.contains(b"StructParent")) {
                        let what = if st == b"Widget" { "Form field".to_string() } else { format!("{} annotation", subtype_name(&a)) };
                        out.push(Finding { page: Some(p), message: format!("{what} on page {} isn't tagged", p + 1) });
                    }
                }
            }
        }
        Rule::TabOrder => {
            for &p in list {
                let Some(page) = pages.get(p) else { continue };
                if !annots(page).is_empty() && page.dict.name(b"Tabs") != Some(b"S") {
                    out.push(Finding { page: Some(p), message: format!("Page {} doesn't tab in structure order", p + 1) });
                }
            }
        }
        Rule::CharacterEncoding => {
            let mut seen = HashSet::new();
            for (p, fonts) in &scan.fonts {
                for r in fonts {
                    if seen.insert(*r)
                        && let Some(f) = doc.get(*r).as_dict()
                        && !maps_to_unicode(doc, f)
                    {
                        let name = f.name(b"BaseFont").map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_else(|| "(unnamed)".into());
                        out.push(Finding { page: Some(*p), message: format!("Font {name} on page {} doesn't map its characters to Unicode", p + 1) });
                    }
                }
            }
        }
        Rule::FieldDescriptions => {
            let mut seen = HashSet::new();
            for &p in list {
                let Some(page) = pages.get(p) else { continue };
                for (r, a) in annots(page) {
                    if a.name(b"Subtype") != Some(b"Widget") {
                        continue;
                    }
                    // The field is the widget itself when it has a name, else its parent.
                    let field = if a.contains(b"T") { Some(r) } else { a.get(b"Parent").and_then(Object::as_ref) };
                    let Some(field) = field else { continue };
                    if !seen.insert(field) {
                        continue;
                    }
                    let fd = doc.get(field).as_dict().cloned().unwrap_or_default();
                    if text_of(doc, &fd, b"TU").is_none() {
                        out.push(Finding {
                            page: Some(p),
                            message: format!("Field \"{}\" on page {} has no description (tooltip)", full_name(doc, field), p + 1),
                        });
                    }
                }
            }
        }
        _ => {}
    }
    out
}

fn full_name(doc: &Document, field: ObjRef) -> String {
    let mut parts = Vec::new();
    let mut node = Some(field);
    for _ in 0..64 {
        let Some(r) = node else { break };
        let Some(d) = doc.get(r).as_dict().cloned() else { break };
        if let Some(t) = text_of(doc, &d, b"T") {
            parts.push(t);
        }
        node = d.get(b"Parent").and_then(Object::as_ref);
    }
    parts.reverse();
    parts.join(".")
}

/// Whether text in this font can be mapped to Unicode (ToUnicode, a standard encoding, or a
/// predefined CJK character collection).
fn maps_to_unicode(doc: &Document, f: &Dict) -> bool {
    if f.contains(b"ToUnicode") {
        return true;
    }
    let enc = f.get(b"Encoding").map(|e| doc.resolve(e));
    match f.name(b"Subtype") {
        Some(b"Type0") => {
            let identity = matches!(enc.as_deref(), Some(Object::Name(n)) if n.starts_with(b"Identity"));
            let ordering = f
                .get(b"DescendantFonts")
                .map(|d| doc.resolve(d))
                .and_then(|d| d.as_array().and_then(|a| a.first().cloned()))
                .and_then(|d| doc.resolve(&d).as_dict().cloned())
                .and_then(|d| d.get(b"CIDSystemInfo").map(|c| doc.resolve(c)).and_then(|c| c.as_dict().cloned()))
                .and_then(|c| c.get(b"Ordering").map(|o| doc.resolve(o)).and_then(|o| o.as_string().map(|s| s.bytes.clone())));
            let known = matches!(ordering.as_deref(), Some(b"Japan1" | b"GB1" | b"CNS1" | b"Korea1" | b"KR"));
            !identity && known
        }
        Some(b"Type3") => enc.is_some(),
        _ => match enc.as_deref() {
            Some(Object::Name(_) | Object::Dict(_)) => true,
            _ => {
                // No encoding: the font's built-in one, which is standard unless it is symbolic.
                let flags = f.get(b"FontDescriptor").map(|d| doc.resolve(d)).and_then(|d| d.as_dict().and_then(|d| d.int(b"Flags"))).unwrap_or(32);
                let base = f.name(b"BaseFont").unwrap_or(b"");
                flags & 4 == 0 || matches!(base, b"Symbol" | b"ZapfDingbats")
            }
        },
    }
}
