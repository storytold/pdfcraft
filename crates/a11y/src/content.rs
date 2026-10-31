//! The Document, Page Content and Forms rules: catalog settings, page content (tagged or
//! artifact, text, images, fonts) and annotations.
//!
//! Every sentence a rule here can say is listed in [`crate::WORDINGS`], so interfaces can say it in
//! their own language; a test in this crate keeps that list complete.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object};
use pdfcraft_model::Page;

use crate::{Finding, Rule};

/// What the page content holds, per checked page.
#[derive(Debug, Default)]
pub(crate) struct Scan {
    pub has_text: bool,
    /// Some text has no language from a marked-content property list around it.
    pub unlabelled_text: bool,
    pub has_images: bool,
    /// Some page paints something: text, a path, a shading or an image.
    pub painted: bool,
    /// Page → painting operations outside tagged or artifact content.
    pub untagged: BTreeMap<usize, usize>,
    /// Page → fonts used.
    pub fonts: BTreeMap<usize, BTreeSet<ObjRef>>,
    /// Page → fonts that draw text: shown in a render mode other than 3 (invisible).
    pub rendered_fonts: BTreeMap<usize, BTreeSet<ObjRef>>,
    /// Page → artifacts that begin inside tagged (MCID) content.
    pub artifacts_in_tagged: BTreeMap<usize, usize>,
    /// Page → tagged (MCID) content that begins inside an artifact.
    pub tagged_in_artifacts: BTreeMap<usize, usize>,
    /// Form XObject → how many times a content stream draws it.
    pub form_uses: HashMap<ObjRef, usize>,
    /// Form XObjects whose own content has marked content with an MCID.
    pub forms_with_mcid: HashSet<ObjRef>,
    /// Page → the marked content (MCID) of its own content stream, and the text in each.
    pub mcid_text: BTreeMap<usize, HashMap<i64, McidText>>,
    /// Pages read only in part: content or forms past the page's decode budget are missing.
    pub incomplete: BTreeSet<usize>,
}

/// The text in one marked-content sequence (MCID) of a page.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct McidText {
    /// It shows text.
    pub text: bool,
    /// Some of that text has no language from a marked-content property list: no /Lang on its
    /// own sequence or on one around it (ISO 32000-1 §14.9.2.1).
    pub unlabelled: bool,
}

const MAX_FORM_DEPTH: usize = 12;

/// `q` saves nested deeper than this are not followed (the text state stays as it is).
const MAX_SAVES: usize = 4096;

/// What encloses a point in a content stream.
#[derive(Clone, Copy, Debug, Default)]
struct Within {
    /// Marked content with an MCID: tagged content.
    tagged: bool,
    /// Marked content tagged /Artifact.
    artifact: bool,
    /// The MCID of the page's own marked content around it, if any.
    mcid: Option<i64>,
    /// A marked-content property list around it sets a language (/Lang).
    lang: bool,
}

impl Within {
    /// Tagged or an artifact: not untagged content.
    fn covered(self) -> bool {
        self.tagged || self.artifact
    }
}

/// The part of the text state the scan follows. `q` saves it and `Q` restores it.
#[derive(Clone, Copy, Debug, Default)]
struct TextState {
    font: Option<ObjRef>,
    /// The text rendering mode (Tr); 3 draws nothing.
    mode: i64,
}

impl Scan {
    pub fn run(doc: &Document, pages: &[Page], list: &[usize]) -> Scan {
        let mut s = Scan::default();
        for &p in list {
            let Some(page) = pages.get(p) else { continue };
            let res = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
            let mut budget = DecodeBudget::page();
            let data = crate::alt::page_content_within(doc, &page.dict, &mut budget);
            if budget.remaining() == 0 {
                s.incomplete.insert(p);
            }
            let mut w = Walk { doc, scan: &mut s, page: p, forms: HashSet::new(), budget };
            w.content(&data, &res, Within::default(), TextState::default(), None, 0);
        }
        s
    }
}

struct Walk<'a> {
    doc: &'a Document,
    scan: &'a mut Scan,
    page: usize,
    forms: HashSet<ObjRef>,
    /// Decoded bytes and form entries left for this page.
    budget: DecodeBudget,
}

impl Walk<'_> {
    fn sub(&self, res: &Dict, key: &[u8]) -> Dict {
        res.get(key).map(|x| self.doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default()
    }

    /// Walk a content stream. `outer` is what encloses it (for a form XObject, what encloses the
    /// `Do` that draws it), `text` the text state it starts in, and `form` the form XObject it is.
    fn content(&mut self, data: &[u8], res: &Dict, outer: Within, text: TextState, form: Option<ObjRef>, depth: usize) {
        let (fonts, xobjects, props) = (self.sub(res, b"Font"), self.sub(res, b"XObject"), self.sub(res, b"Properties"));
        // Marked-content stack: what encloses each level.
        let mut stack: Vec<Within> = Vec::new();
        let mut saved: Vec<TextState> = Vec::new();
        let mut text = text;
        let (mut untagged, mut has_mcid) = (0usize, false);
        let (mut artifacts_in_tagged, mut tagged_in_artifacts) = (0usize, 0usize);
        for op in pdfcraft_content::parse(data).ops {
            let within = stack.last().copied().unwrap_or(outer);
            let inside = within.covered();
            match op.op.as_slice() {
                b"q" if saved.len() < MAX_SAVES => saved.push(text),
                b"Q" => text = saved.pop().unwrap_or(text),
                b"Tr" => text.mode = op.operands.first().and_then(Object::as_int).unwrap_or(text.mode),
                b"BMC" => {
                    let artifact = op.name(0) == Some(b"Artifact");
                    artifacts_in_tagged += usize::from(artifact && within.tagged);
                    stack.push(Within { artifact: within.artifact || artifact, ..within });
                }
                b"BDC" => {
                    let artifact = op.name(0) == Some(b"Artifact");
                    let list = match op.operands.get(1) {
                        Some(Object::Dict(d)) => Some(d.clone()),
                        Some(Object::Name(n)) => props.get(n).map(|x| self.doc.resolve(x)).and_then(|x| x.as_dict().cloned()),
                        _ => None,
                    };
                    let list = list.unwrap_or_default();
                    let mcid = !artifact && list.contains(b"MCID");
                    has_mcid |= mcid;
                    artifacts_in_tagged += usize::from(artifact && within.tagged);
                    tagged_in_artifacts += usize::from(mcid && within.artifact);
                    // An MCID in a form XObject belongs to the form's own stream, not the page's.
                    let page_mcid = match (mcid, form) {
                        (false, _) => within.mcid,
                        (true, None) => list.get(b"MCID").map(|x| self.doc.resolve(x)).and_then(|x| x.as_int()),
                        (true, Some(_)) => None,
                    };
                    if mcid
                        && form.is_none()
                        && let Some(m) = page_mcid
                    {
                        self.scan.mcid_text.entry(self.page).or_default().entry(m).or_default();
                    }
                    let lang = text_of(self.doc, &list, b"Lang").is_some_and(|l| is_language_tag(&l));
                    stack.push(Within {
                        tagged: within.tagged || mcid,
                        artifact: within.artifact || artifact,
                        mcid: page_mcid,
                        lang: within.lang || lang,
                    });
                }
                b"EMC" => {
                    stack.pop();
                }
                b"Tj" | b"TJ" | b"'" | b"\"" => {
                    self.scan.has_text = true;
                    self.scan.painted = true;
                    untagged += usize::from(!inside);
                    self.scan.unlabelled_text |= !within.lang;
                    if let Some(m) = within.mcid {
                        let seen = self.scan.mcid_text.entry(self.page).or_default().entry(m).or_default();
                        seen.text = true;
                        seen.unlabelled |= !within.lang;
                    }
                    if text.mode != 3
                        && let Some(font) = text.font
                    {
                        self.scan.rendered_fonts.entry(self.page).or_default().insert(font);
                    }
                }
                b"Tf" => {
                    text.font = op.name(0).and_then(|n| fonts.get(n)).and_then(Object::as_ref);
                    if let Some(r) = text.font {
                        self.scan.fonts.entry(self.page).or_default().insert(r);
                    }
                }
                b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" | b"sh" => {
                    self.scan.painted = true;
                    untagged += usize::from(!inside);
                }
                b"BI" => {
                    self.scan.has_images = true;
                    self.scan.painted = true;
                    untagged += usize::from(!inside);
                }
                b"Do" => {
                    let Some(r) = op.name(0).and_then(|n| xobjects.get(n)).and_then(Object::as_ref) else { continue };
                    let obj = self.doc.get(r);
                    let Object::Stream(x) = &*obj else { continue };
                    if x.dict.name(b"Subtype") == Some(b"Form") {
                        let uses = self.scan.form_uses.entry(r).or_default();
                        *uses = uses.saturating_add(1);
                    }
                    match x.dict.name(b"Subtype") {
                        Some(b"Image") => {
                            self.scan.has_images = true;
                            self.scan.painted = true;
                            untagged += usize::from(!inside);
                        }
                        Some(b"Form") if depth < MAX_FORM_DEPTH && !self.forms.contains(&r) => {
                            if !self.budget.enter_form() {
                                self.scan.incomplete.insert(self.page);
                                continue;
                            }
                            let data = match self.budget.decode(x) {
                                Ok(data) => data,
                                Err(_) => {
                                    if self.budget.exhausted() {
                                        self.scan.incomplete.insert(self.page);
                                    }
                                    continue;
                                }
                            };
                            self.forms.insert(r);
                            let inner = x
                                .dict
                                .get(b"Resources")
                                .map(|o| self.doc.resolve(o))
                                .and_then(|o| o.as_dict().cloned())
                                .unwrap_or_else(|| res.clone());
                            self.content(&data, &inner, within, text, Some(r), depth + 1);
                            self.forms.remove(&r);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        let page = self.page;
        for (count, map) in [
            (untagged, &mut self.scan.untagged),
            (artifacts_in_tagged, &mut self.scan.artifacts_in_tagged),
            (tagged_in_artifacts, &mut self.scan.tagged_in_artifacts),
        ] {
            if count > 0 {
                let n = map.entry(page).or_default();
                *n = n.saturating_add(count);
            }
        }
        if has_mcid && let Some(form) = form {
            self.scan.forms_with_mcid.insert(form);
        }
    }
}

pub(crate) fn catalog(doc: &Document) -> Dict {
    doc.root().and_then(|r| doc.get(r).as_dict().cloned()).unwrap_or_default()
}

/// The most references one value may be written through. A chain is unusual but legal, and the
/// render inspector follows them (lopdf's `dereference` has a limit of its own), so the two
/// readers of a flag must agree about one.
const MAX_REFERENCE_CHAIN: usize = 32;

/// The value a chain of indirect references ends at. [`Document::resolve`] follows one reference,
/// and an indirect object's value may itself be a reference (ISO 32000-2 §7.3.10): `/Marked
/// 8 0 R` with object 8 holding `9 0 R` holding `true` is a tagged document, which this rule read
/// as untagged while `doc_info` read it as tagged. Bounded and cycle-safe, since a damaged file
/// can point a reference at itself; a chain longer than that, or one that loops, is no value.
fn resolved(doc: &Document, o: &Object) -> std::sync::Arc<Object> {
    let mut seen: HashSet<ObjRef> = HashSet::new();
    let mut value = doc.resolve(o);
    while let Object::Ref(r) = &*value {
        if seen.len() >= MAX_REFERENCE_CHAIN || !seen.insert(*r) {
            return std::sync::Arc::new(Object::Null);
        }
        value = doc.get(*r);
    }
    value
}

pub(crate) fn text_of(doc: &Document, d: &Dict, key: &[u8]) -> Option<String> {
    d.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_string().map(|s| s.to_text())).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// Whether `s` is a language tag in the form RFC 3066 gives it, which ISO 32000-1 §14.9.2.1
/// names: a primary subtag of 1 to 8 letters, then any number of subtags of 1 to 8 letters or
/// digits, joined by hyphens. Every BCP 47 tag, which PDF 2.0 names, has this form too.
pub(crate) fn is_language_tag(s: &str) -> bool {
    let subtag = |p: &str, letters_only: bool| {
        (1..=8).contains(&p.len()) && p.bytes().all(|b| if letters_only { b.is_ascii_alphabetic() } else { b.is_ascii_alphanumeric() })
    };
    let mut parts = s.split('-');
    parts.next().is_some_and(|p| subtag(p, true)) && parts.all(|p| subtag(p, false))
}

fn doc_finding(sentence: &'static str) -> Vec<Finding> {
    vec![Finding::plain(None, sentence)]
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
            // `/Marked` may be an indirect reference like any other value -- or a chain of
            // them -- so it is followed to the value before the test below, which looks for the
            // boolean itself (#3671).
            let marked =
                cat.get(b"MarkInfo").map(|m| resolved(doc, m)).and_then(|m| m.as_dict().and_then(|d| d.get(b"Marked").map(|v| resolved(doc, v))));
            let mut out = Vec::new();
            if !cat.contains(b"StructTreeRoot") {
                out.push(Finding::plain(None, "The document has no tags (structure tree)"));
            }
            if !matches!(marked.as_deref(), Some(Object::Bool(true))) {
                out.push(Finding::plain(None, "The document isn't marked as tagged (MarkInfo Marked)"));
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
                out.push(Finding::plain(None, "The document has no title"));
            }
            let shows = cat
                .get(b"ViewerPreferences")
                .map(|v| doc.resolve(v))
                .and_then(|v| v.as_dict().and_then(|d| d.get(b"DisplayDocTitle").map(|x| matches!(&*doc.resolve(x), Object::Bool(true)))))
                .unwrap_or(false);
            if !shows {
                out.push(Finding::plain(None, "The window shows the file name, not the title (Initial View > Show)"));
            }
            out
        }
        Rule::Bookmarks => {
            let has = cat.get(b"Outlines").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().map(|d| d.contains(b"First"))).unwrap_or(false);
            if pages.len() > 20 && !has {
                vec![Finding::worded(None, "{n} pages and no bookmarks", vec![("{n}", pages.len().to_string())])]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

const MULTIMEDIA: &[&[u8]] = &[b"Screen", b"Movie", b"Sound", b"RichMedia", b"3D"];

fn subtype_name(d: &Dict) -> String {
    String::from_utf8_lossy(d.name(b"Subtype").unwrap_or(b"Unknown")).into_owned()
}

/// An annotation's rectangle is already in page user space. Ignore missing, malformed and
/// degenerate rectangles rather than sending the checker to an arbitrary place on the page.
pub(crate) fn annotation_rect(doc: &Document, annot: &Dict) -> Option<[f64; 4]> {
    let rect = annot.get(b"Rect").map(|r| doc.resolve(r))?;
    let values = rect.as_array()?;
    let [x0, y0, x1, y1] = values.as_slice() else { return None };
    let [x0, y0, x1, y1] = [x0, y0, x1, y1].map(|v| doc.resolve(v).as_f64());
    let (x0, y0, x1, y1) = (x0?, y0?, x1?, y1?);
    if ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) || x0 == x1 || y0 == y1 {
        return None;
    }
    Some([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)])
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
                // One item and several items are separate wordings, so a language can agree the
                // noun with the number its own way.
                let pattern = if *n == 1 { "Page {p}: {n} untagged content item" } else { "Page {p}: {n} untagged content items" };
                out.push(Finding::worded(Some(*p), pattern, vec![("{p}", (p + 1).to_string()), ("{n}", n.to_string())]));
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
                        // A whole sentence each way, rather than a noun put in front of one: a form
                        // field and an annotation of some subtype read differently in other
                        // languages. The subtype itself is the PDF's own name and goes in as it is.
                        let mut values = vec![("{p}", (p + 1).to_string())];
                        let pattern = if st == b"Widget" {
                            "Form field on page {p} isn't tagged"
                        } else {
                            values.push(("{}", subtype_name(&a)));
                            "{} annotation on page {p} isn't tagged"
                        };
                        let mut finding = Finding::worded(Some(p), pattern, values);
                        finding.bbox = annotation_rect(doc, &a);
                        out.push(finding);
                    }
                }
            }
        }
        Rule::TabOrder => {
            for &p in list {
                let Some(page) = pages.get(p) else { continue };
                if !annots(page).is_empty() && page.dict.name(b"Tabs") != Some(b"S") {
                    out.push(Finding::worded(Some(p), "Page {p} doesn't tab in structure order", vec![("{p}", (p + 1).to_string())]));
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
                        // A font with no name gets its own whole sentence, so another language can
                        // word that rather than translate a parenthesis. The name itself is the
                        // document's own and goes in as it stands.
                        let page = ("{p}", (p + 1).to_string());
                        out.push(match f.name(b"BaseFont") {
                            Some(n) => Finding::worded(
                                Some(*p),
                                "Font {} on page {p} doesn't map its characters to Unicode",
                                vec![("{}", String::from_utf8_lossy(n).into_owned()), page],
                            ),
                            None => Finding::worded(Some(*p), "Font (unnamed) on page {p} doesn't map its characters to Unicode", vec![page]),
                        });
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
                        let mut finding = Finding::worded(
                            Some(p),
                            "Field \"{}\" on page {p} has no description (tooltip)",
                            vec![("{}", full_name(doc, field)), ("{p}", (p + 1).to_string())],
                        );
                        finding.bbox = annotation_rect(doc, &a);
                        out.push(finding);
                    }
                }
            }
        }
        _ => {}
    }
    out
}

pub(crate) fn full_name(doc: &Document, field: ObjRef) -> String {
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

/// Predefined CJK CMaps (ISO 32000-2 §9.7.5.2, Table 116), excluding Identity-H/V.
/// Test complete names: neither a custom CMap nor a nonexistent vertical variant is predefined.
fn predefined_cjk_cmap(name: &[u8]) -> bool {
    if matches!(name, b"H" | b"V" | b"83pv-RKSJ-H" | b"90pv-RKSJ-H" | b"KSCpc-EUC-H" | b"UniAKR-UTF8-H" | b"UniAKR-UTF16-H" | b"UniAKR-UTF32-H") {
        return true;
    }
    let Some(base) = name.strip_suffix(b"-H").or_else(|| name.strip_suffix(b"-V")) else { return false };
    matches!(
        base,
        b"GB-EUC"
            | b"GBpc-EUC"
            | b"GBK-EUC"
            | b"GBKp-EUC"
            | b"GBK2K"
            | b"UniGB-UCS2"
            | b"UniGB-UTF16"
            | b"B5pc"
            | b"HKscs-B5"
            | b"ETen-B5"
            | b"ETenms-B5"
            | b"CNS-EUC"
            | b"UniCNS-UCS2"
            | b"UniCNS-UTF16"
            | b"90ms-RKSJ"
            | b"90msp-RKSJ"
            | b"Add-RKSJ"
            | b"EUC"
            | b"Ext-RKSJ"
            | b"UniJIS-UCS2"
            | b"UniJIS-UCS2-HW"
            | b"UniJIS-UTF16"
            | b"KSC-EUC"
            | b"KSCms-UHC"
            | b"KSCms-UHC-HW"
            | b"UniKS-UCS2"
            | b"UniKS-UTF16"
    )
}

/// Whether text in this font declares a Unicode mapping route (ToUnicode, a standard encoding,
/// a predefined CJK CMap, or an Adobe CJK character collection). This does not validate every glyph.
fn maps_to_unicode(doc: &Document, f: &Dict) -> bool {
    if f.contains(b"ToUnicode") {
        return true;
    }
    let enc = f.get(b"Encoding").map(|e| doc.resolve(e));
    match f.name(b"Subtype") {
        Some(b"Type0") => {
            // ISO 32000-2 §9.10.2: either a predefined non-Identity CMap or the descendant's
            // Adobe CJK collection supplies the route. Identity-H/V do not rule out the latter.
            if matches!(enc.as_deref(), Some(Object::Name(n)) if predefined_cjk_cmap(n)) {
                return true;
            }
            let system = f
                .get(b"DescendantFonts")
                .map(|d| doc.resolve(d))
                .and_then(|d| d.as_array().and_then(|a| a.first().cloned()))
                .and_then(|d| doc.resolve(&d).as_dict().cloned())
                .and_then(|d| d.get(b"CIDSystemInfo").map(|c| doc.resolve(c)).and_then(|c| c.as_dict().cloned()));
            let Some(system) = system else { return false };
            let registry = system.get(b"Registry").map(|r| doc.resolve(r));
            let ordering = system.get(b"Ordering").map(|o| doc.resolve(o));
            matches!(registry.as_deref(), Some(Object::String(s)) if s.bytes == b"Adobe")
                && matches!(ordering.as_deref(), Some(Object::String(s)) if matches!(s.bytes.as_slice(), b"Japan1" | b"GB1" | b"CNS1" | b"Korea1" | b"KR"))
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
