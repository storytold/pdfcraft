//! The Document, Page Content and Forms rules: catalog settings, page content (tagged or
//! artifact, text, images, fonts) and annotations.
//!
//! Every sentence a rule here can say is listed in [`crate::WORDINGS`], so interfaces can say it in
//! their own language; a test in this crate keeps that list complete.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object};
use pdfcraft_model::Page;

use crate::{Finding, Rule};

/// What the page content holds, per checked page.
#[derive(Debug, Default)]
pub(crate) struct Scan {
    pub has_text: bool,
    pub has_images: bool,
    /// Page → painting operations outside tagged or artifact content.
    pub untagged: BTreeMap<usize, usize>,
    /// Page → fonts used, in its content or in its annotations' appearances.
    pub fonts: BTreeMap<usize, BTreeSet<ObjRef>>,
    /// Pages read only in part: content or forms past the page's decode budget are missing.
    pub incomplete: BTreeSet<usize>,
    /// Pages whose annotation appearances were read only in part, past their own decode budget:
    /// fonts may be missing, but the page content was read.
    pub incomplete_appearances: BTreeSet<usize>,
}

const MAX_FORM_DEPTH: usize = 12;

impl Scan {
    /// Scan the pages in `list`; with `appearances`, also the fonts of their annotations'
    /// appearance streams, which only the Character Encoding rule reads.
    pub fn run(doc: &Document, pages: &[Page], list: &[usize], appearances: bool) -> Scan {
        let mut s = Scan::default();
        // An appearance stream shared by several annotations, or pages, is read once: once per
        // page when it has no resources of its own and so reads its fonts from each page's.
        let mut appearances_seen: HashSet<(ObjRef, Option<usize>)> = HashSet::new();
        for &p in list {
            let Some(page) = pages.get(p) else { continue };
            let res = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
            let mut budget = DecodeBudget::page();
            let data = crate::alt::page_content_within(doc, &page.dict, &mut budget);
            if budget.remaining() == 0 {
                s.incomplete.insert(p);
            }
            let mut w = Walk { doc, scan: &mut s, page: p, forms: HashSet::new(), budget, appearance: false, cut: false };
            w.content(&data, &res, false, 0);
            if !appearances {
                continue;
            }
            // Text an annotation shows is drawn by its appearance streams, whose fonts must map
            // to Unicode like any other (ISO 32000-2 §14.8.2.6; ISO 14289-1 7.21.7 names all
            // fonts): a check box's ZapfDingbats check mark is the common case. They have a
            // budget of their own, so they never cut the page content's reading short.
            let mut w = Walk { doc, scan: &mut s, page: p, forms: HashSet::new(), budget: DecodeBudget::page(), appearance: true, cut: false };
            for r in appearance_streams(doc, &page.dict) {
                let own_resources = matches!(&*doc.get(r), Object::Stream(x) if x.dict.contains(b"Resources"));
                let key = (r, (!own_resources).then_some(p));
                if appearances_seen.contains(&key) {
                    continue;
                }
                // Only a walk read to the end is kept: one cut short is read again where it
                // appears next, with that page's budget.
                w.cut = false;
                w.appearance_stream(r, &res);
                if !w.cut {
                    appearances_seen.insert(key);
                }
            }
        }
        s
    }
}

/// The appearance streams of a page's annotations (ISO 32000-2 §12.5.5): the normal, rollover
/// and down appearances, each a stream or a dictionary of streams by appearance state.
fn appearance_streams(doc: &Document, page: &Dict) -> Vec<ObjRef> {
    let annots = page.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
    let mut out = Vec::new();
    for annot in &annots {
        let annot = doc.resolve(annot);
        let Some(ap) = annot.as_dict().and_then(|a| a.get(b"AP")).map(|ap| doc.resolve(ap)) else { continue };
        let Some(ap) = ap.as_dict() else { continue };
        for key in [b"N".as_slice(), b"R", b"D"] {
            let Some(entry) = ap.get(key) else { continue };
            if let Some(r) = entry.as_ref()
                && matches!(&*doc.get(r), Object::Stream(_))
            {
                out.push(r);
                continue;
            }
            // A dictionary of states, given directly or by reference; its streams are references.
            if let Some(states) = doc.resolve(entry).as_dict() {
                out.extend(states.iter().filter_map(|(_, s)| s.as_ref()));
            }
        }
    }
    out
}

struct Walk<'a> {
    doc: &'a Document,
    scan: &'a mut Scan,
    page: usize,
    forms: HashSet<ObjRef>,
    /// Decoded bytes and form entries left for this page.
    budget: DecodeBudget,
    /// Walking annotation appearances: only their fonts are recorded, since what they paint is
    /// not page content (it is tagged through the annotation, not by marked content).
    appearance: bool,
    /// The appearance stream being walked was read only in part.
    cut: bool,
}

fn bdc_marks(doc: &Document, op: &pdfcraft_content::Op, props: &Dict) -> bool {
    op.name(0) == Some(b"Artifact")
        || match op.operands.get(1) {
            Some(Object::Dict(d)) => d.contains(b"MCID"),
            Some(Object::Name(n)) => props.get(n).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().map(|d| d.contains(b"MCID"))).unwrap_or(false),
            _ => false,
        }
}

impl Walk<'_> {
    fn sub(&self, res: &Dict, key: &[u8]) -> Dict {
        res.get(key).map(|x| self.doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default()
    }

    /// This page was read only in part: its content, or (walking appearances) its annotations'.
    fn cut_short(&mut self) {
        self.cut = true;
        let pages = if self.appearance { &mut self.scan.incomplete_appearances } else { &mut self.scan.incomplete };
        pages.insert(self.page);
    }

    /// Form XObject `x` decoded against the budget, its /DecodeParms given by reference read too.
    /// Walking appearances, a stream that decodes only up to damage, or not at all, leaves the
    /// walk cut short: what went unread may select a font.
    fn decode(&mut self, x: &pdfcraft_cos::Stream) -> Result<Vec<u8>, pdfcraft_cos::CosError> {
        let decoded = self.budget.decode_with_damage(self.doc, x, usize::MAX);
        if self.appearance && !matches!(decoded, Ok((_, false))) {
            self.cut_short();
        }
        decoded.map(|(data, _)| data)
    }

    /// Walk one annotation appearance stream for its fonts. `res` stands in for resources the
    /// stream doesn't give, as for a form XObject.
    fn appearance_stream(&mut self, r: ObjRef, res: &Dict) {
        let obj = self.doc.get(r);
        let Object::Stream(x) = &*obj else { return };
        // An appearance stream is a form XObject (ISO 32000-2 §12.5.5). One that says it is
        // something else, an image say, is not decoded as content.
        if x.dict.name(b"Subtype").is_some_and(|s| s != b"Form") {
            return;
        }
        if !self.budget.enter_form() {
            self.cut_short();
            return;
        }
        let data = match self.decode(x) {
            Ok(data) => data,
            Err(_) => {
                if self.budget.exhausted() {
                    self.cut_short();
                }
                return;
            }
        };
        let inner = x.dict.get(b"Resources").map(|o| self.doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_else(|| res.clone());
        self.forms.insert(r);
        // Covered: nothing an appearance paints is counted as untagged page content.
        self.content(&data, &inner, true, 1);
        self.forms.remove(&r);
    }

    /// Walk a content stream; `covered`: already inside tagged or artifact content.
    fn content(&mut self, data: &[u8], res: &Dict, covered: bool, depth: usize) {
        let (fonts, xobjects, props) = (self.sub(res, b"Font"), self.sub(res, b"XObject"), self.sub(res, b"Properties"));
        let states = self.sub(res, b"ExtGState");
        // Marked-content stack: whether each level is tagged or an artifact.
        let mut stack: Vec<bool> = Vec::new();
        let mut untagged = 0;
        // One operator at a time: the walk keeps counts, never the stream's operators.
        for op in pdfcraft_content::ops(data) {
            let inside = covered || stack.last().copied().unwrap_or(false);
            match op.op.as_slice() {
                b"BMC" => stack.push(inside || op.name(0) == Some(b"Artifact")),
                b"BDC" => stack.push(inside || bdc_marks(self.doc, &op, &props)),
                b"EMC" => {
                    stack.pop();
                }
                b"Tj" | b"TJ" | b"'" | b"\"" => {
                    self.scan.has_text |= !self.appearance;
                    untagged += usize::from(!inside);
                }
                b"Tf" => {
                    if let Some(r) = op.name(0).and_then(|n| fonts.get(n)).and_then(Object::as_ref) {
                        self.scan.fonts.entry(self.page).or_default().insert(r);
                    }
                }
                b"gs" => {
                    // An ExtGState's /Font entry, an array of the font and its size, selects the
                    // font as Tf does (ISO 32000-2 §8.4.5, Table 57).
                    let font = op
                        .name(0)
                        .and_then(|n| states.get(n))
                        .map(|g| self.doc.resolve(g))
                        .and_then(|g| g.as_dict().and_then(|g| g.get(b"Font")).map(|f| self.doc.resolve(f)))
                        .and_then(|f| f.as_array().and_then(|a| a.first()).and_then(Object::as_ref));
                    if let Some(r) = font {
                        self.scan.fonts.entry(self.page).or_default().insert(r);
                    }
                }
                b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" | b"sh" => untagged += usize::from(!inside),
                b"BI" => {
                    self.scan.has_images |= !self.appearance;
                    untagged += usize::from(!inside);
                }
                b"Do" => {
                    let Some(r) = op.name(0).and_then(|n| xobjects.get(n)).and_then(Object::as_ref) else { continue };
                    let obj = self.doc.get(r);
                    let Object::Stream(x) = &*obj else { continue };
                    match x.dict.name(b"Subtype") {
                        Some(b"Image") => {
                            self.scan.has_images |= !self.appearance;
                            untagged += usize::from(!inside);
                        }
                        Some(b"Form") if depth < MAX_FORM_DEPTH && !self.forms.contains(&r) => {
                            if !self.budget.enter_form() {
                                self.cut_short();
                                continue;
                            }
                            let data = match self.decode(x) {
                                Ok(data) => data,
                                Err(_) => {
                                    if self.budget.exhausted() {
                                        self.cut_short();
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
                            self.content(&data, &inner, inside, depth + 1);
                            self.forms.remove(&r);
                        }
                        // An appearance's form past the depth limit may hold the fonts not read.
                        // (A form already being walked is a cycle: nothing more to read there.)
                        Some(b"Form") if self.appearance && depth >= MAX_FORM_DEPTH && !self.forms.contains(&r) => self.cut_short(),
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

fn text_of(doc: &Document, d: &Dict, key: &[u8]) -> Option<String> {
    d.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_string().map(|s| s.to_text())).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
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
fn annotation_rect(doc: &Document, annot: &Dict) -> Option<[f64; 4]> {
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
    // The annotation objects are shared, never deep-copied: a reference repeated across a page's
    // Annots array (or its pages) must not copy the dictionary each time.
    let annots = |p: &Page| -> Vec<(ObjRef, std::sync::Arc<Object>)> {
        let a = p.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        a.iter()
            .filter_map(|x| x.as_ref().map(|r| (r, doc.get(r))))
            .filter(|(_, o)| o.as_dict().is_some_and(|d| d.name(b"Subtype") != Some(b"Popup")))
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
                for (_, obj) in annots(page) {
                    let Some(a) = obj.as_dict() else { continue };
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
                            values.push(("{}", subtype_name(a)));
                            "{} annotation on page {p} isn't tagged"
                        };
                        let mut finding = Finding::worded(Some(p), pattern, values);
                        finding.bbox = annotation_rect(doc, a);
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
                for (r, obj) in annots(page) {
                    let Some(a) = obj.as_dict() else { continue };
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
                        finding.bbox = annotation_rect(doc, a);
                        out.push(finding);
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
        // An encoding dictionary with neither /BaseEncoding nor /Differences leaves the font's
        // built-in encoding as it is (ISO 32000-2 §9.6.5.1, Table 112): as if there were none.
        subtype => match enc.as_deref() {
            Some(Object::Dict(d)) if !d.contains(b"BaseEncoding") && !d.contains(b"Differences") => built_in_maps(doc, f, subtype),
            Some(Object::Name(_) | Object::Dict(_)) => true,
            _ => built_in_maps(doc, f, subtype),
        },
    }
}

/// Whether simple font `f`, of `subtype`, maps to Unicode through its built-in encoding, which is
/// standard unless the font is symbolic.
fn built_in_maps(doc: &Document, f: &Dict, subtype: Option<&[u8]>) -> bool {
    let base = f.name(b"BaseFont").unwrap_or(b"");
    // ZapfDingbats' built-in encoding names its glyphs a1 to a191 (ISO 32000-2 Annex D.6), which
    // no glyph list maps to Unicode, so as a Type 1 font it needs a ToUnicode map whatever its
    // descriptor says (§9.10.2; ISO 14289-1 7.21.7). It is the font of a check box's check mark,
    // usually without a descriptor at all.
    let type1 = matches!(subtype, Some(b"Type1" | b"MMType1"));
    if type1 && base == b"ZapfDingbats" {
        return false;
    }
    // Symbol's built-in names are those of Annex D.5, which ISO 14289-1 7.21.7 accepts for a Type 1
    // font. A symbolic TrueType font has no names to go by.
    let flags = f.get(b"FontDescriptor").map(|d| doc.resolve(d)).and_then(|d| d.as_dict().and_then(|d| d.int(b"Flags"))).unwrap_or(32);
    flags & 4 == 0 || (type1 && base == b"Symbol")
}
