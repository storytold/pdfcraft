//! The Document, Page Content and Forms rules: catalog settings, page content (tagged or
//! artifact, text, images, fonts) and annotations.
//!
//! Every sentence a rule here can say is listed in [`crate::WORDINGS`], so interfaces can say it in
//! their own language; a test in this crate keeps that list complete.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object};
use pdfcraft_model::Page;

use crate::parent_tree::{Holdings, ParentTree, Place, both};
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
    /// Pages read only in part: content or forms past the page's decode budget are missing.
    pub incomplete: BTreeSet<usize>,
    /// Some marked content was counted neither as tagged nor as untagged: the parent tree can't
    /// tell whether an element owns it ([`ParentTree::owns`]).
    pub undecided: bool,
    /// The structure parent tree: which element owns each tagged item.
    pub tree: ParentTree,
    /// What the elements the parent tree names hold, read once each.
    holdings: Holdings,
}

const MAX_FORM_DEPTH: usize = 12;

impl Scan {
    pub fn run(doc: &Document, pages: &[Page], list: &[usize]) -> Scan {
        let mut s = Scan { tree: ParentTree::read(doc), ..Scan::default() };
        for &p in list {
            let Some(page) = pages.get(p) else { continue };
            let res = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
            let mut budget = DecodeBudget::page();
            let data = crate::alt::page_content_within(doc, &page.dict, &mut budget);
            if budget.remaining() == 0 {
                s.incomplete.insert(p);
            }
            let key = struct_parents(doc, &page.dict);
            let place = Place { page: page.obj, stm: None };
            let mut w = Walk { doc, scan: &mut s, page: p, forms: HashSet::new(), budget, key, place };
            w.content(&data, &res, false, 0);
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
    /// The parent-tree key (/StructParents) of the content stream being walked: the page's, or a
    /// form XObject's own.
    key: Option<i64>,
    /// Where the content stream being walked is: the page's own, or a form XObject's.
    place: Place,
}

/// The /StructParents of a page or form XObject: the key of its entry in the parent tree.
fn struct_parents(doc: &Document, d: &Dict) -> Option<i64> {
    d.get(b"StructParents").and_then(|k| doc.resolve(k).as_int())
}

impl Walk<'_> {
    /// Whether a BDC opens an artifact, or tagged content: marked content whose MCID belongs to a
    /// structure element through the parent tree entry `self.key` names, an element that holds it
    /// among its kids (ISO 32000-2 §14.7.5.4, [`ParentTree::owns`]). An MCID that no element owns
    /// is not tagged: no tag in the tree answers it. One the parent tree can't decide for (its
    /// entry may be in the part of a truncated tree not read, its element's kids were read only in
    /// part, or its parents run past the ancestor limit) is not counted either way, and noted in
    /// [`Scan::undecided`]: the rule is then left to a manual check.
    fn bdc_marks(&mut self, op: &pdfcraft_content::Op, props: &Dict) -> bool {
        if op.name(0) == Some(b"Artifact") {
            return true;
        }
        let mcid_of = |d: &Dict| d.get(b"MCID").and_then(|m| self.doc.resolve(m).as_int());
        let mcid = match op.operands.get(1) {
            Some(Object::Dict(d)) => mcid_of(d),
            Some(Object::Name(n)) => props.get(n).map(|x| self.doc.resolve(x)).and_then(|x| x.as_dict().and_then(mcid_of)),
            _ => None,
        };
        match (mcid, self.key) {
            (Some(mcid), Some(key)) => self.scan.tree.owns(self.doc, &mut self.scan.holdings, key, mcid, self.place).unwrap_or_else(|| {
                self.scan.undecided = true;
                true
            }),
            _ => false,
        }
    }
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
        // One operator at a time: the walk keeps counts, never the stream's operators.
        for op in pdfcraft_content::ops(data) {
            let inside = covered || stack.last().copied().unwrap_or(false);
            match op.op.as_slice() {
                b"BMC" => stack.push(inside || op.name(0) == Some(b"Artifact")),
                b"BDC" => stack.push(inside || self.bdc_marks(&op, &props)),
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
                            // The form's marked content is keyed by its own /StructParents.
                            let key = std::mem::replace(&mut self.key, struct_parents(self.doc, &x.dict));
                            let page = self.place.page;
                            let place = std::mem::replace(&mut self.place, Place { page, stm: Some(r) });
                            let inner = x
                                .dict
                                .get(b"Resources")
                                .map(|o| self.doc.resolve(o))
                                .and_then(|o| o.as_dict().cloned())
                                .unwrap_or_else(|| res.clone());
                            self.content(&data, &inner, inside, depth + 1);
                            self.key = key;
                            self.place = place;
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

/// Whether annotation `r` (`a`, of subtype `st`) is tagged: its /StructParent names, in the parent
/// tree, the structure element that holds it through an object reference (/OBJR) among its kids
/// (ISO 32000-2 §14.7.5.3, §14.7.5.4), and that element is of the type that holds annotations of
/// its kind (§14.8.2.5.2, §14.8.4.7.2 Table 368): Form for a widget, Link for a link, Annot for any
/// other. An element of another type can't hold it, wherever its parents lead. `None` when the
/// parent tree can't tell ([`ParentTree::entry`], [`Holdings::holds_object`],
/// [`ParentTree::in_hierarchy`]): the annotation is then not counted against the rule, which is
/// left to a manual check.
fn annotation_tagged(doc: &Document, tree: &ParentTree, owners: &mut Owners, r: ObjRef, a: &Dict, st: &[u8]) -> Option<bool> {
    let Some(key) = a.get(b"StructParent").and_then(|k| doc.resolve(k).as_int()) else { return Some(false) };
    let Some((at, element)) = tree.entry(doc, key)? else { return Some(false) };
    let want: &[u8] = match st {
        b"Widget" => b"Form",
        b"Link" => b"Link",
        _ => b"Annot",
    };
    // The type may be given by reference (ISO 32000-2 §7.3.10).
    let role = element.as_dict().and_then(|e| e.get(b"S")).and_then(|s| doc.resolve(s).as_name().map(<[u8]>::to_vec));
    if !role.is_some_and(|s| owners.roles.standard(doc, &s).is_some_and(|t| t == want)) {
        return Some(false);
    }
    both(owners.holdings.holds_object(doc, at, &element, r), || tree.in_hierarchy(doc, &element))
}

/// What one rule's annotations look up in their elements, each read once: the standard type each
/// element type reaches, and what each element holds.
struct Owners<'a> {
    roles: Roles<'a>,
    holdings: Holdings,
}

/// The standard type each structure type reaches through the tree root's /RoleMap, kept for one
/// rule's annotations: the elements of many annotations share types, and a type on a chain
/// reaches what the whole chain reaches, so each mapping is followed once.
struct Roles<'a> {
    role_map: &'a Dict,
    /// Type → the standard type it reaches, or `None` for none.
    known: HashMap<Vec<u8>, Option<Vec<u8>>>,
}

impl<'a> Roles<'a> {
    fn new(role_map: &'a Dict) -> Self {
        Roles { role_map, known: HashMap::new() }
    }

    /// The standard type `s` is, or maps to through the role map. Every mapping is followed until
    /// a standard type, a type the map doesn't hold, or one already passed (a cycle, ISO 32000-2
    /// §14.7.3): a long chain that ends in a standard type still reaches it.
    fn standard(&mut self, doc: &Document, s: &[u8]) -> Option<Vec<u8>> {
        let mut path: Vec<Vec<u8>> = Vec::new();
        let mut seen: HashSet<Vec<u8>> = HashSet::new();
        let mut t = s.to_vec();
        let end = loop {
            if crate::structure::is_standard(&t) {
                break Some(t);
            }
            if let Some(known) = self.known.get(&t) {
                break known.clone();
            }
            let next = self.role_map.get(&t).map(|o| doc.resolve(o)).and_then(|o| o.as_name().map(<[u8]>::to_vec));
            if !seen.insert(t.clone()) {
                break None;
            }
            path.push(t);
            let Some(next) = next else { break None };
            t = next;
        };
        // Every type passed reaches the same end: the role map gives each type one mapping.
        for t in path {
            self.known.insert(t, end.clone());
        }
        end
    }
}

/// The findings of page rule `rule` on the pages in `list`, and whether an item it applies to was
/// counted neither way because the parent tree can't tell whether it is tagged.
pub(crate) fn page_rule(doc: &Document, rule: Rule, pages: &[Page], list: &[usize], scan: &Scan, tagged: bool) -> (Vec<Finding>, bool) {
    let mut out = Vec::new();
    let mut undecided = false;
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
                return (doc_finding("The document isn't tagged, so none of its content is"), false);
            }
            for (p, n) in &scan.untagged {
                // One item and several items are separate wordings, so a language can agree the
                // noun with the number its own way.
                let pattern = if *n == 1 { "Page {p}: {n} untagged content item" } else { "Page {p}: {n} untagged content items" };
                out.push(Finding::worded(Some(*p), pattern, vec![("{p}", (p + 1).to_string()), ("{n}", n.to_string())]));
            }
        }
        Rule::TaggedAnnotations | Rule::TaggedMultimedia | Rule::TaggedFormFields => {
            let role_map = crate::alt::tree_root(doc)
                .and_then(|r| r.get(b"RoleMap").map(|o| doc.resolve(o)))
                .and_then(|o| o.as_dict().cloned())
                .unwrap_or_default();
            let mut owners = Owners { roles: Roles::new(&role_map), holdings: Holdings::default() };
            for &p in list {
                let Some(page) = pages.get(p) else { continue };
                for (r, obj) in annots(page) {
                    let Some(a) = obj.as_dict() else { continue };
                    let st = a.name(b"Subtype").unwrap_or(b"");
                    let applies = match rule {
                        Rule::TaggedFormFields => st == b"Widget",
                        Rule::TaggedMultimedia => MULTIMEDIA.contains(&st),
                        _ => st != b"Widget" && !MULTIMEDIA.contains(&st) && st != b"PrinterMark",
                    };
                    let held = applies
                        && tagged
                        && annotation_tagged(doc, &scan.tree, &mut owners, r, a, st).unwrap_or_else(|| {
                            undecided = true;
                            true
                        });
                    if applies && !held {
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
    (out, undecided)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_role_mapping_is_followed_once_for_a_rule() {
        // A chain of 10,000 mappings to Link, as many links' elements might share: the first
        // lookup follows it once and keeps every type on it, so later ones, from any point on the
        // chain, look up one type. A chain that goes round reaches nothing, from any point on it.
        let n = 10_000;
        let mut map = Dict::new();
        for i in 0..n {
            let to = if i + 1 == n { b"Link".to_vec() } else { format!("R{}", i + 1).into_bytes() };
            map.set(format!("R{i}").into_bytes(), Object::Name(to));
        }
        map.set(b"C0".to_vec(), Object::Name(b"C1".to_vec()));
        map.set(b"C1".to_vec(), Object::Name(b"C0".to_vec()));
        let doc = Document::new_empty();
        let mut roles = Roles::new(&map);
        assert_eq!(roles.standard(&doc, b"R0"), Some(b"Link".to_vec()));
        assert_eq!(roles.known.len(), n);
        assert_eq!(roles.standard(&doc, b"R5000"), Some(b"Link".to_vec()));
        assert_eq!(roles.standard(&doc, b"C1"), None);
        assert_eq!(roles.known.len(), n + 2);
        assert_eq!(roles.standard(&doc, b"Span"), Some(b"Span".to_vec()));
    }
}
