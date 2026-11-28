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
    pub has_images: bool,
    /// Page → painting operations outside tagged or artifact content.
    pub untagged: BTreeMap<usize, usize>,
    /// Page → fonts used.
    pub fonts: BTreeMap<usize, BTreeSet<ObjRef>>,
    /// Font → the one-byte codes its text shows, on any checked page: bit `c` of the set is code `c`.
    pub codes: HashMap<ObjRef, [u64; 4]>,
    /// Pages read only in part: content or forms past the page's decode budget are missing.
    pub incomplete: BTreeSet<usize>,
    /// Pages whose text the scan may not have read in full, or may have given the wrong font: a
    /// content stream or form that stopped at damage or didn't decode, a form past the nesting
    /// limit, or a font set where the `q` limit can't restore it. [`codes`](Scan::codes) is then
    /// not the whole story, so no font is judged by the codes it shows.
    pub partial_text: BTreeSet<usize>,
}

const MAX_FORM_DEPTH: usize = 12;

/// `q` saves nested deeper than this keep no font of their own to restore. Their `Q`s are counted
/// so they don't take the saves kept below them; a font set inside them makes the page's text
/// partial ([`Scan::partial_text`]).
const MAX_SAVES: usize = 4096;

impl Scan {
    pub fn run(doc: &Document, pages: &[Page], list: &[usize]) -> Scan {
        let mut s = Scan::default();
        for &p in list {
            let Some(page) = pages.get(p) else { continue };
            let res = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
            let mut budget = DecodeBudget::page();
            let (data, damaged) = crate::alt::page_content_with_damage(doc, &page.dict, &mut budget);
            if budget.remaining() == 0 {
                s.incomplete.insert(p);
            }
            if damaged {
                s.partial_text.insert(p);
            }
            let mut w = Walk { doc, scan: &mut s, page: p, page_res: res.clone(), forms: HashSet::new(), budget, font: None };
            w.content(&data, &res, false, 0);
        }
        s
    }
}

struct Walk<'a> {
    doc: &'a Document,
    scan: &'a mut Scan,
    page: usize,
    /// The page's resources, which a form without its own inherits (ISO 32000-2 §7.8.3).
    page_res: Dict,
    forms: HashSet<ObjRef>,
    /// Decoded bytes and form entries left for this page.
    budget: DecodeBudget,
    /// The current font (Tf), part of the graphics state: `q` saves it and `Q` restores it, and a
    /// form XObject starts with the one in effect where it is drawn.
    font: Option<ObjRef>,
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

    /// Notes the codes a text-showing operator shows in the current font, one byte each: what a
    /// simple font's encoding reads (ISO 32000-2 §9.6.5).
    fn shown(&mut self, op: &pdfcraft_content::Op) {
        let Some(font) = self.font else { return };
        let strings: Vec<&[u8]> = match (op.op.as_slice(), op.operands.as_slice()) {
            (b"Tj" | b"'", [Object::String(s), ..]) => vec![&s.bytes],
            (b"\"", [_, _, Object::String(s), ..]) => vec![&s.bytes],
            (b"TJ", [Object::Array(items), ..]) => items.iter().filter_map(|x| x.as_string().map(|s| s.bytes.as_slice())).collect(),
            _ => return,
        };
        let set = self.scan.codes.entry(font).or_default();
        for &b in strings.iter().flat_map(|s| s.iter()) {
            if let Some(word) = set.get_mut(usize::from(b >> 6)) {
                *word |= 1 << (b & 63);
            }
        }
    }

    /// Makes `font` the current font, as `Tf` does and an ExtGState's /Font entry does through `gs`.
    /// `over`: `q` saves past [`MAX_SAVES`] are open, so no `Q` can bring back the font this
    /// replaces.
    fn set_font(&mut self, font: Option<ObjRef>, over: usize) {
        if let Some(r) = font {
            self.scan.fonts.entry(self.page).or_default().insert(r);
        }
        if over > 0 {
            self.scan.partial_text.insert(self.page);
        }
        self.font = font;
    }

    /// Walk a content stream; `covered`: already inside tagged or artifact content.
    fn content(&mut self, data: &[u8], res: &Dict, covered: bool, depth: usize) {
        let (fonts, xobjects, props) = (self.sub(res, b"Font"), self.sub(res, b"XObject"), self.sub(res, b"Properties"));
        let states = self.sub(res, b"ExtGState");
        // Marked-content stack: whether each level is tagged or an artifact.
        let mut stack: Vec<bool> = Vec::new();
        let mut untagged = 0;
        let mut saved: Vec<Option<ObjRef>> = Vec::new();
        // `q` saves past MAX_SAVES still open: their `Q`s restore nothing kept.
        let mut over = 0usize;
        // One operator at a time: the walk keeps counts, never the stream's operators.
        let mut ops = pdfcraft_content::ops(data);
        for op in ops.by_ref() {
            let inside = covered || stack.last().copied().unwrap_or(false);
            match op.op.as_slice() {
                b"q" => {
                    if saved.len() < MAX_SAVES {
                        saved.push(self.font);
                    } else {
                        over = over.saturating_add(1);
                    }
                }
                b"Q" => {
                    if over > 0 {
                        over -= 1;
                    } else if let Some(font) = saved.pop() {
                        self.font = font;
                    }
                }
                b"BMC" => stack.push(inside || op.name(0) == Some(b"Artifact")),
                b"BDC" => stack.push(inside || bdc_marks(self.doc, &op, &props)),
                b"EMC" => {
                    stack.pop();
                }
                b"Tj" | b"TJ" | b"'" | b"\"" => {
                    self.scan.has_text = true;
                    untagged += usize::from(!inside);
                    self.shown(&op);
                }
                b"Tf" => {
                    let font = op.name(0).and_then(|n| fonts.get(n)).and_then(Object::as_ref);
                    self.set_font(font, over);
                }
                b"gs" => {
                    // An ExtGState's /Font entry, an array of the font and its size, sets the font
                    // as Tf does (ISO 32000-2 §8.4.5, Table 57). Without one the font stays.
                    // A direct dictionary is read in place, so many `gs` don't copy it many times.
                    // An ExtGState or /Font the file holds but can't read may set a font: which
                    // one the later text is in is not known.
                    let doc = self.doc;
                    let mut lost = false;
                    let font = op.name(0).and_then(|n| states.get(n)).and_then(|g| {
                        let held;
                        let g = if let Object::Ref(r) = g {
                            lost |= crate::alt::unreadable(doc, *r);
                            held = doc.resolve(g);
                            &*held
                        } else {
                            g
                        };
                        let entry = g.as_dict()?.get(b"Font")?;
                        let held;
                        let entry = if let Object::Ref(r) = entry {
                            lost |= crate::alt::unreadable(doc, *r);
                            held = doc.resolve(entry);
                            &*held
                        } else {
                            entry
                        };
                        entry.as_array().map(|a| a.first().and_then(Object::as_ref))
                    });
                    if lost {
                        self.scan.partial_text.insert(self.page);
                    }
                    if let Some(font) = font {
                        self.set_font(font, over);
                    }
                }
                b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" | b"sh" => untagged += usize::from(!inside),
                b"BI" => {
                    self.scan.has_images = true;
                    untagged += usize::from(!inside);
                }
                b"Do" => {
                    let Some(r) = op.name(0).and_then(|n| xobjects.get(n)).and_then(Object::as_ref) else { continue };
                    // An XObject the file holds but can't read may be a form showing text.
                    if crate::alt::unreadable(self.doc, r) {
                        self.scan.partial_text.insert(self.page);
                        continue;
                    }
                    let obj = self.doc.get(r);
                    let Object::Stream(x) = &*obj else { continue };
                    match x.dict.name(b"Subtype") {
                        Some(b"Image") => {
                            self.scan.has_images = true;
                            untagged += usize::from(!inside);
                        }
                        // Text in a form nested past the limit is not read.
                        Some(b"Form") if depth >= MAX_FORM_DEPTH && !self.forms.contains(&r) => {
                            self.scan.partial_text.insert(self.page);
                        }
                        Some(b"Form") if !self.forms.contains(&r) => {
                            if !self.budget.enter_form() {
                                self.scan.incomplete.insert(self.page);
                                continue;
                            }
                            let data = match self.budget.decode_with_damage(self.doc, x, usize::MAX) {
                                Ok((data, damaged)) => {
                                    if damaged {
                                        self.scan.partial_text.insert(self.page);
                                    }
                                    data
                                }
                                Err(_) => {
                                    if self.budget.exhausted() {
                                        self.scan.incomplete.insert(self.page);
                                    } else {
                                        self.scan.partial_text.insert(self.page);
                                    }
                                    continue;
                                }
                            };
                            self.forms.insert(r);
                            let font = self.font;
                            let inner = x
                                .dict
                                .get(b"Resources")
                                .map(|o| self.doc.resolve(o))
                                .and_then(|o| o.as_dict().cloned())
                                .unwrap_or_else(|| self.page_res.clone());
                            self.content(&data, &inner, inside, depth + 1);
                            self.font = font;
                            self.forms.remove(&r);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        // Bytes the parser skipped, or an inline image whose end it guessed, may hold text shown
        // in some font: the codes the walk saw are not all there are.
        if ops.skipped() > 0 || ops.ambiguous_images() > 0 {
            self.scan.partial_text.insert(self.page);
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
            // What the embedded font programs read here may decode, all together.
            let mut programs = DecodeBudget::new(MAX_FONT_PROGRAMS, 0);
            // A font passes on the codes its text shows only when the scan read all of that text.
            let codes_known = scan.incomplete.is_empty() && scan.partial_text.is_empty();
            for (p, fonts) in &scan.fonts {
                for r in fonts {
                    if seen.insert(*r)
                        && let Some(f) = doc.get(*r).as_dict()
                        && !maps_to_unicode(doc, f)
                        && !(codes_known && built_in_names_map(doc, f, scan.codes.get(r), &mut programs))
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

/// The most bytes one embedded Type 1 font program may decode to when its built-in encoding is
/// read, and the most all of them may decode to in one check.
const MAX_FONT_PROGRAM: usize = 4 << 20;
const MAX_FONT_PROGRAMS: usize = 64 << 20;

/// The clear-text part of a Type 1 font program read for its encoding, at most: the encoding comes
/// before `eexec`, a few kilobytes in.
const MAX_CLEAR_TEXT: usize = 256 << 10;

/// Whether a Type 1 font without an /Encoding entry, which reads its codes through the built-in
/// encoding of its embedded program, names a glyph for every code `codes` shows whose name gives
/// its Unicode value: ISO 32000-2 §9.10.2's route through glyph names, open to a simple font
/// whose glyph selection uses a glyph name (§9.6.5.2). A symbolic font's built-in names are often
/// standard ones (`plus`, `minus`), and are then read like those of any other encoding. A name
/// counts when [`pdfcraft_fonts::pdf::glyph_text`] reads it. A program that can't be read, or
/// codes without a name, leave the font without a route.
fn built_in_names_map(doc: &Document, f: &Dict, codes: Option<&[u64; 4]>, budget: &mut DecodeBudget) -> bool {
    // An /Encoding of null is no /Encoding (ISO 32000-2 §7.3.7).
    let encoding = f.get(b"Encoding").map(|e| doc.resolve(e));
    if !matches!(f.name(b"Subtype"), Some(b"Type1" | b"MMType1")) || encoding.is_some_and(|e| !matches!(*e, Object::Null)) {
        return false;
    }
    let descriptor = f.get(b"FontDescriptor").map(|d| doc.resolve(d));
    let program = descriptor.as_deref().and_then(Object::as_dict).and_then(|d| d.get(b"FontFile")).map(|x| doc.resolve(x));
    let Some(Object::Stream(program)) = program.as_deref() else { return false };
    // Through the document, so /DecodeParms given by reference are read too.
    let Ok((data, damaged)) = budget.decode_with_damage(doc, program, MAX_FONT_PROGRAM) else { return false };
    let Some(encoding) = built_in_encoding(&data, !damaged) else { return false };
    let Some(codes) = codes else { return true };
    (0..=u8::MAX).filter(|&c| codes.get(usize::from(c >> 6)).is_some_and(|w| w & (1 << (c & 63)) != 0)).all(|c| match &encoding {
        BuiltIn::Standard => pdfcraft_fonts::pdf::standard_encoding_char(c).is_some(),
        BuiltIn::Names(names) => names
            .get(usize::from(c))
            .and_then(|n| n.as_deref())
            .and_then(|n| std::str::from_utf8(n).ok())
            .is_some_and(|n| pdfcraft_fonts::pdf::glyph_text(n).is_some()),
    })
}

/// A Type 1 font program's built-in encoding.
enum BuiltIn {
    /// `/Encoding StandardEncoding def`: Adobe's standard Latin names (ISO 32000-2 Annex D.2).
    Standard,
    /// The glyph name of each code, from `dup code /name put`; `None` for a code it doesn't name.
    Names(Vec<Option<Vec<u8>>>),
}

/// The built-in encoding of a Type 1 font program, from the `/Encoding` its clear text defines
/// before `eexec` (Adobe Type 1 Font Format, 2.3): StandardEncoding, or an array filled with
/// `dup code /name put` and closed by `def`. `None` when the program defines neither, when the
/// definition isn't closed within the clear text read, when an assignment's code is not an
/// integer from 0 to 255, when StandardEncoding is only where the definition starts (a copy it
/// then changes), or, for a program that stopped at damage (`whole` false), when its clear text
/// doesn't reach `eexec`: what the damage cut off may define the encoding again. Comments and strings are not read as code (PostScript Language
/// Reference, 3.2.2 to 3.2.4), nor are procedure bodies, `{` to `}`, which are not run where they
/// are written (3.5.3).
fn built_in_encoding(program: &[u8], whole: bool) -> Option<BuiltIn> {
    let clear = program.get(..program.len().min(MAX_CLEAR_TEXT))?;
    let tokens = ps_tokens(clear);
    if !whole && !tokens.contains(&b"eexec".as_slice()) {
        return None;
    }
    let tokens = tokens.split(|t| *t == b"eexec").next().unwrap_or_default();
    // Each token with how many procedures it is inside.
    let mut depth = 0usize;
    let tokens: Vec<(&[u8], usize)> = tokens
        .iter()
        .map(|&t| {
            let at = depth;
            match t {
                b"{" => depth = depth.saturating_add(1),
                b"}" => depth = depth.saturating_sub(1),
                _ => {}
            }
            (t, at.min(depth))
        })
        .collect();
    let top = |i: usize, word: &[u8]| tokens.get(i).is_some_and(|&(t, d)| d == 0 && t == word);
    let at = (0..tokens.len()).find(|&i| top(i, b"/Encoding"))?;
    let start = at.saturating_add(1);
    // `dup 33 /arrowright put`, until the array is defined (`readonly def` or `def`).
    let end = (start..tokens.len()).find(|&i| top(i, b"def"))?;
    let body = tokens.get(start..end)?;
    // `/Encoding StandardEncoding def` (or `readonly def`) is the standard table itself; any more
    // after StandardEncoding (`256 array copy`, then `put`s) changes a copy of it.
    match body {
        [(b"StandardEncoding", _)] | [(b"StandardEncoding", _), (b"readonly", _)] => return Some(BuiltIn::Standard),
        [(b"StandardEncoding", _), ..] => return None,
        _ => {}
    }
    let mut names: Vec<Option<Vec<u8>>> = vec![None; 256];
    let mut any = false;
    for w in body.windows(4) {
        if let [(b"dup", 0), (code, 0), (name, 0), (b"put", 0)] = w
            && let Some(name) = name.strip_prefix(b"/")
        {
            // A code this reader can't place may be the one that undoes another assignment.
            let slot = ps_integer(code).and_then(|c| usize::try_from(c).ok()).and_then(|c| names.get_mut(c))?;
            *slot = Some(name.to_vec());
            any = true;
        }
    }
    any.then_some(BuiltIn::Names(names))
}

/// A PostScript integer: decimal with an optional sign, or `base#digits` with a base from 2 to 36
/// (PostScript Language Reference, 3.2.2). `None` for anything else, a real number included.
fn ps_integer(token: &[u8]) -> Option<i64> {
    let text = std::str::from_utf8(token).ok()?;
    match text.split_once('#') {
        Some((base, digits)) => {
            let base = base.parse::<u32>().ok().filter(|b| (2..=36).contains(b))?;
            // Radix digits are unsigned (`from_str_radix` would take a sign).
            if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_alphanumeric()) {
                return None;
            }
            i64::from_str_radix(digits, base).ok()
        }
        None => text.parse::<i64>().ok(),
    }
}

/// The tokens of PostScript clear text, in order: names, numbers and operators, split at white
/// space and the delimiters `[ ] { } < > ( ) / %`, a literal name keeping its `/`, and the braces
/// `{` and `}` that open and close a procedure. Comments (`%`
/// to the end of the line) and strings (`(...)` with nested parentheses and backslash escapes, or
/// `<...>`) are left out (PostScript Language Reference, 3.2.2 to 3.2.4).
fn ps_tokens(text: &[u8]) -> Vec<&[u8]> {
    let space = |b: u8| b.is_ascii_whitespace() || b == 0;
    let delimiter = |b: u8| matches!(b, b'[' | b']' | b'{' | b'}' | b'<' | b'>' | b'(' | b')' | b'/' | b'%');
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(&b) = text.get(i) {
        match b {
            b'%' => {
                while text.get(i).is_some_and(|&c| c != b'\n' && c != b'\r') {
                    i += 1;
                }
            }
            b'(' => {
                let mut level = 0usize;
                while let Some(&c) = text.get(i) {
                    i += 1;
                    match c {
                        b'\\' => i += 1,
                        b'(' => level += 1,
                        b')' => {
                            level = level.saturating_sub(1);
                            if level == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            // `<<` opens a dictionary; a lone `<` a hexadecimal string, to its `>`.
            b'<' if text.get(i.saturating_add(1)) == Some(&b'<') => i += 2,
            b'<' => {
                while text.get(i).is_some_and(|&c| c != b'>') {
                    i += 1;
                }
                i += 1;
            }
            b'{' | b'}' => {
                if let Some(brace) = text.get(i..=i) {
                    out.push(brace);
                }
                i += 1;
            }
            _ if space(b) || (delimiter(b) && b != b'/') => i += 1,
            _ => {
                // A name, number or operator; a literal name keeps its `/`.
                let start = i;
                i += 1;
                while text.get(i).is_some_and(|&c| !space(c) && !delimiter(c)) {
                    i += 1;
                }
                if let Some(token) = text.get(start..i) {
                    out.push(token);
                }
            }
        }
    }
    out
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
