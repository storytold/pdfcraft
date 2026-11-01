//! The checks: each failure condition PdfCraft decides, and the findings that show where it fails.
//!
//! Every sentence a finding here can say is listed in [`crate::WORDINGS`], so interfaces can say it
//! in their own language; a test in this crate keeps that list complete.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use pdfcraft_model::Page;

use super::xmp::{metadata, xfa_requires_dynamic_render, xmp_has_language, xmp_value};
use super::{Collected, Part};
use crate::content::{self, Scan};
use crate::structure::{self, FIRST_HEADING, Found, Frame, Kind, Visitor};
use crate::{Finding, MAX_FINDINGS, Options, Rule, RuleResult};

/// The Accessibility Checker rules whose findings are a failure condition as they stand. The
/// Checker's Lbl and LBody rule is not among them: it also wants every Lbl in a list item, while
/// ISO 32000 lets a label stand in a table of contents entry or a heading too. [`Tree`] checks
/// LBody alone.
const FROM_CHECKER: [(Rule, &str); 8] = [
    (Rule::TaggedContent, "01-005"),
    (Rule::CharacterEncoding, "10-001"),
    (Rule::FiguresAltText, "13-004"),
    (Rule::OtherElementsAltText, "17-002"),
    (Rule::TableRows, "09-004"),
    (Rule::TableCells, "09-004"),
    (Rule::TableRegularity, "09-004"),
    (Rule::ListItems, "09-005"),
];

/// The structure types of ISO 32000-1 (PDF 1.7) §14.8.4: the standard types of PDF/UA-1. The
/// types PDF 2.0 added are not among them and must be role-mapped in a PDF/UA-1 file.
const STANDARD_1_7: &[&[u8]] = &[
    b"Document",
    b"Part",
    b"Art",
    b"Sect",
    b"Div",
    b"BlockQuote",
    b"Caption",
    b"TOC",
    b"TOCI",
    b"Index",
    b"NonStruct",
    b"Private",
    b"P",
    b"H",
    b"H1",
    b"H2",
    b"H3",
    b"H4",
    b"H5",
    b"H6",
    b"L",
    b"LI",
    b"Lbl",
    b"LBody",
    b"Table",
    b"TR",
    b"TH",
    b"TD",
    b"THead",
    b"TBody",
    b"TFoot",
    b"Span",
    b"Quote",
    b"Note",
    b"Reference",
    b"BibEntry",
    b"Code",
    b"Link",
    b"Annot",
    b"Ruby",
    b"RB",
    b"RT",
    b"RP",
    b"Warichu",
    b"WT",
    b"WP",
    b"Figure",
    b"Formula",
    b"Form",
];

/// Distinct structure types, annotation owners and Note ids kept at most (guards against absurd files).
const MAX_KEPT: usize = 1 << 20;

/// Name-tree nodes read at most, and how deep.
const MAX_NAME_NODES: usize = 100_000;
const MAX_NAME_DEPTH: usize = 32;

/// What the document sets no language for, when neither it nor any of its tags sets one.
pub(super) const NO_LANGUAGE: &str = "The document sets no language (Lang), and neither do its tags";

/// Run every check of `part` on `doc`, into `c`. Returns the part the metadata declares.
pub(super) fn run(doc: &Document, part: Part, c: &mut Collected) -> Option<u8> {
    let pages = pdfcraft_model::pages(doc);
    let all: Vec<usize> = (0..pages.len()).collect();
    let scan = Scan::run(doc, &pages, &all);
    let cat = content::catalog(doc);
    let cat_lang = content::text_of(doc, &cat, b"Lang");
    let doc_lang = cat_lang.as_deref().is_some_and(content::is_language_tag);
    let xmp = metadata(doc, &cat);
    let declared = xmp.as_deref().and_then(|x| xmp_value(x, "pdfuaid:part")).and_then(|p| p.trim().parse::<u8>().ok());

    // The checker's rules, read as failure conditions.
    let mut rules: BTreeSet<Rule> = FROM_CHECKER.iter().map(|(rule, _)| *rule).collect();
    rules.insert(Rule::HeadingNesting);
    let report = crate::check_scanned(doc, &Options { rules, pages: None }, &pages, &all, &scan);
    for RuleResult { rule, findings, .. } in report.results {
        if rule == Rule::HeadingNesting {
            for f in findings {
                let first = f.values.iter().any(|(hole, what)| *hole == "{what}" && what == FIRST_HEADING);
                c.extend_rule(if first { "14-002" } else { "14-003" }, vec![f]);
            }
        } else if let Some(&(_, id)) = FROM_CHECKER.iter().find(|(r, _)| *r == rule) {
            c.extend_rule(id, findings);
        }
    }

    // Marked content is tagged only when a structure tree is there to hold it.
    if !structure::has_struct_tree(doc) && scan.painted && !c.failed("01-005") {
        c.add("01-005", Finding::plain(None, "The document isn't tagged, so none of its content is"));
    }
    document(doc, part, &cat, xmp.as_deref(), declared, c);
    let Tree { found, raw_types, owners, any_lang, .. } = Tree::run(doc, &pages, &scan, doc_lang);
    if !doc_lang {
        // A language that isn't a language tag is no language; the finding says which it is.
        if let Some(l) = cat_lang.as_ref().filter(|_| scan.unlabelled_text) {
            let shown: String = l.chars().take(64).collect();
            c.add("11-001", Finding::worded(None, "The document's language (Lang) {} isn't a language tag", vec![("{}", shown)]));
        }
        language(doc, &cat, xmp.as_deref(), scan.unlabelled_text && cat_lang.is_none(), any_lang, &found, c);
    }
    role_map(doc, &raw_types, c);
    for (id, found) in found {
        // With no language anywhere, one finding in `language` stands for all of these.
        if id.starts_with("11-") && !any_lang {
            continue;
        }
        let (findings, total) = found.into_result();
        c.extend(id, findings, total);
    }
    annotations(doc, &pages, &owners, doc_lang, c);
    page_content(doc, &scan, c);
    document_objects(doc, &cat, c);
    declared
}

pub(super) fn dict_at(doc: &Document, d: &Dict, key: &[u8]) -> Option<Dict> {
    d.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned())
}

/// The document-level conditions: metadata, identification, viewer preferences, Tagged PDF.
fn document(doc: &Document, part: Part, cat: &Dict, xmp: Option<&str>, declared: Option<u8>, c: &mut Collected) {
    if xmp.is_none() {
        c.add("06-001", Finding::plain(None, "The catalog has no metadata stream (XMP)"));
    }
    let identification = match part {
        Part::Ua1 => "06-002",
        Part::Ua2 => "ua2-identification",
    };
    match declared {
        None => c.add(identification, Finding::plain(None, "The metadata doesn't identify the file as PDF/UA (pdfuaid:part)")),
        Some(n) if n != part.number() => c.add(
            identification,
            Finding::worded(
                None,
                "The metadata identifies the file as PDF/UA-{n}, not PDF/UA-{want}",
                vec![("{n}", n.to_string()), ("{want}", part.number().to_string())],
            ),
        ),
        Some(_) => {}
    }
    if part == Part::Ua2 {
        // PDF/UA-2 also names the revision of the standard, as a year.
        let rev = xmp.and_then(|x| xmp_value(x, "pdfuaid:rev"));
        if !rev.as_deref().map(str::trim).is_some_and(|r| r.len() == 4 && r.bytes().all(|b| b.is_ascii_digit())) {
            c.add("ua2-identification", Finding::plain(None, "The metadata has no PDF/UA revision year (pdfuaid:rev)"));
        }
        let version = pdf_version(doc, cat);
        if version < (2, 0) {
            c.add(
                "ua2-pdf-version",
                Finding::worded(None, "The file is PDF {v}, not PDF 2.0", vec![("{v}", format!("{}.{}", version.0, version.1))]),
            );
        }
    }
    if xmp.and_then(|x| xmp_value(x, "dc:title")).is_none_or(|t| t.trim().is_empty()) {
        c.add("06-003", Finding::plain(None, "The metadata has no title (dc:title)"));
    }
    match dict_at(doc, cat, b"ViewerPreferences").and_then(|v| v.get(b"DisplayDocTitle").map(|x| doc.resolve(x))) {
        None => c.add("07-001", Finding::plain(None, "The viewer preferences have no DisplayDocTitle entry")),
        Some(v) if matches!(&*v, Object::Bool(true)) => {}
        Some(_) => c.add("07-002", Finding::plain(None, "DisplayDocTitle isn't true: the window shows the file name")),
    }
    if !structure::has_struct_tree(doc) {
        c.add("tagged-pdf", Finding::plain(None, "The document has no tags (structure tree)"));
    }
    let mark_info = dict_at(doc, cat, b"MarkInfo").unwrap_or_default();
    let flag = |key: &[u8]| mark_info.get(key).is_some_and(|v| matches!(&*doc.resolve(v), Object::Bool(true)));
    if !flag(b"Marked") {
        c.add("tagged-pdf", Finding::plain(None, "The document isn't marked as tagged (MarkInfo Marked)"));
    }
    if flag(b"Suspects") {
        c.add("08-003", Finding::plain(None, "MarkInfo Suspects is true: the tags are marked as unreliable"));
    }
}

/// The PDF version: the header's, or the catalog's /Version when that is later.
fn pdf_version(doc: &Document, cat: &Dict) -> (u32, u32) {
    let parse = |v: &str| -> Option<(u32, u32)> {
        let (major, minor) = v.trim().split_once('.')?;
        Some((major.parse().ok()?, minor.parse().ok()?))
    };
    let header = parse(doc.version()).unwrap_or((1, 0));
    let catalog = cat.name(b"Version").and_then(|v| std::str::from_utf8(v).ok()).and_then(parse);
    catalog.map_or(header, |c| c.max(header))
}

/// The language conditions, when the document itself sets no language (catalog /Lang): then every
/// text needs a language of its own, from its tag or one around it, or from a marked-content
/// property list. `found` holds the tags without one; when no tag sets a language either, one
/// finding says so instead of one per tag, if `unlabelled_text` (some text has no language from a
/// property list either).
fn language(
    doc: &Document,
    cat: &Dict,
    xmp: Option<&str>,
    unlabelled_text: bool,
    any_lang: bool,
    found: &BTreeMap<&'static str, Found>,
    c: &mut Collected,
) {
    if !any_lang {
        if unlabelled_text {
            c.add("11-001", Finding::plain(None, NO_LANGUAGE));
        }
        if found.contains_key("11-002") {
            c.add("11-002", Finding::plain(None, NO_LANGUAGE));
        }
    }
    let bookmarks = dict_at(doc, cat, b"Outlines").is_some_and(|o| o.contains(b"First"));
    if bookmarks {
        c.add("11-003", Finding::plain(None, "The bookmarks have no language: the document sets none (Lang)"));
    }
    if xmp.is_some_and(|x| !xmp_has_language(x)) {
        c.add("11-006", Finding::plain(None, "The metadata has no language: the document sets none (Lang) and the metadata no xml:lang"));
    }
}

/// The role map, for PDF/UA-1: every non-standard type in use reaches a PDF 1.7 standard type, no
/// entry leads round in a circle, and no standard type is remapped.
fn role_map(doc: &Document, used: &BTreeSet<Vec<u8>>, c: &mut Collected) {
    let entries = structure::role_map(doc);
    // The map read once into a table, so following a chain costs a lookup a step.
    let map: HashMap<Vec<u8>, Vec<u8>> = entries.iter().filter_map(|(k, v)| doc.resolve(v).as_name().map(|v| (k.clone(), v.to_vec()))).collect();
    let name = |t: &[u8]| String::from_utf8_lossy(t).into_owned();
    let mut ends = HashMap::new();
    for (key, value) in entries.iter() {
        if STANDARD_1_7.contains(&key.as_slice()) {
            let to = doc.resolve(value).as_name().map(name).unwrap_or_default();
            c.add("02-004", Finding::worded(None, "Standard structure type {} is remapped to {to}", vec![("{}", name(key)), ("{to}", to)]));
        } else if chain_end(key, &map, &mut ends) == End::Circle {
            c.add("02-003", Finding::worded(None, "Role map entry {} leads round in a circle", vec![("{}", name(key))]));
        }
    }
    // A type caught in a circle doesn't reach a standard type either.
    for t in used {
        if chain_end(t, &map, &mut ends) != End::Standard {
            c.add("02-001", Finding::worded(None, "Structure type {} doesn't map to a standard type", vec![("{}", name(t))]));
        }
    }
}

/// Where a chain of role-map entries ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    Standard,
    Nowhere,
    /// The entry is part of a circle.
    Circle,
    /// The entry is not in a circle itself, but its chain runs into one.
    IntoCircle,
}

/// Where the chain from `start` ends. Every type passed on the way is remembered in `ends`, so the
/// whole map is followed once however many chains share it: the types that form a circle as
/// [`End::Circle`], those that only lead into one as [`End::IntoCircle`], the others with the end
/// their chain reaches.
fn chain_end(start: &[u8], map: &HashMap<Vec<u8>, Vec<u8>>, ends: &mut HashMap<Vec<u8>, End>) -> End {
    let mut path: Vec<Vec<u8>> = Vec::new();
    let mut on_path: HashSet<Vec<u8>> = HashSet::new();
    let mut t = start.to_vec();
    // The end of the chain, and where on `path` a circle closes, if it does.
    let (end, circle_from) = loop {
        if STANDARD_1_7.contains(&t.as_slice()) {
            break (End::Standard, None);
        }
        if let Some(&end) = ends.get(&t) {
            // A type followed before. Reached from another type, a circle is one that type only
            // leads into; `start` itself keeps what it was found to be.
            let end = if end == End::Circle && !path.is_empty() { End::IntoCircle } else { end };
            break (end, None);
        }
        if !on_path.insert(t.clone()) {
            break (End::IntoCircle, path.iter().position(|p| *p == t));
        }
        path.push(t.clone());
        match map.get(&t) {
            Some(next) => t = next.clone(),
            None => break (End::Nowhere, None),
        }
    };
    let mut result = end;
    for (i, t) in path.into_iter().enumerate() {
        let e = if circle_from.is_some_and(|from| i >= from) { End::Circle } else { end };
        if i == 0 {
            result = e;
        }
        ends.insert(t, e);
    }
    result
}

/// The element enclosing an annotation's object reference.
#[derive(Clone, Debug)]
struct Owner {
    /// Its standard type.
    ty: Vec<u8>,
    alt: bool,
    /// A language applies to it: its own or an enclosing element's.
    lang: bool,
}

/// An open element, as the PDF/UA walk keeps it.
#[derive(Default)]
struct Open {
    /// A language applies: its own /Lang or an enclosing element's.
    lang: bool,
    /// H (unnumbered heading) children so far.
    plain_headings: usize,
    /// Some of its marked content shows text that no language covers: none from the tags, and
    /// none from a marked-content property list around the text.
    unlabelled_text: bool,
    /// Some of its marked content wasn't found in its page's content stream.
    unread: bool,
    /// Marked content of its page's own stream it holds, as the walk told it.
    told: usize,
    /// Its kids so far.
    kids: usize,
    /// In a Table: a Caption kid after the first kid, which then has to be the last.
    late_caption: Option<(usize, Finding)>,
    /// In a Table: THead and TFoot kids so far, and whether it holds rows directly and rows in
    /// THead, TBody or TFoot.
    heads: usize,
    foots: usize,
    bare_rows: bool,
    row_groups: bool,
}

/// Whether the Checker's rules (or [`Tree`]'s LBody check) already report `kid` for standing in
/// `parent`, so the nesting check need not.
fn reported_elsewhere(parent: Kind, kid: Kind) -> bool {
    match kid {
        Kind::Tr => !matches!(parent, Kind::Table | Kind::Section),
        Kind::Th | Kind::Td => parent != Kind::Tr,
        Kind::Li => parent != Kind::L,
        Kind::LBody => !matches!(parent, Kind::Li | Kind::NoteOrToc),
        _ => false,
    }
}

/// An open table: whether its cells use Headers, and its TH cells without Scope until it closes.
struct Heads {
    depth: usize,
    headers: bool,
    unscoped: Vec<(usize, Finding)>,
    more: usize,
}

/// The structure tree conditions, read in one walk.
struct Tree<'a> {
    scan: &'a Scan,
    doc_lang: bool,
    found: BTreeMap<&'static str, Found>,
    /// Every structure type in use, as written.
    raw_types: BTreeSet<Vec<u8>>,
    /// Annotation → the element its object reference sits in.
    owners: HashMap<ObjRef, Owner>,
    /// Some element sets a language.
    any_lang: bool,
    open: Vec<Open>,
    tables: Vec<Heads>,
    note_ids: HashSet<Vec<u8>>,
    plain_heading: bool,
    numbered_heading: bool,
}

impl<'a> Tree<'a> {
    fn run(doc: &Document, pages: &[Page], scan: &'a Scan, doc_lang: bool) -> Tree<'a> {
        let mut tree = Tree {
            scan,
            doc_lang,
            found: BTreeMap::new(),
            raw_types: BTreeSet::new(),
            owners: HashMap::new(),
            any_lang: false,
            open: Vec::new(),
            tables: Vec::new(),
            note_ids: HashSet::new(),
            plain_heading: false,
            numbered_heading: false,
        };
        structure::walk(doc, pages, &mut tree);
        if tree.plain_heading && tree.numbered_heading {
            tree.add("14-007", 0, || Finding::plain(None, "The document uses both H and numbered headings (H1–H6)"));
        }
        tree
    }

    fn add(&mut self, id: &'static str, index: usize, make: impl FnOnce() -> Finding) {
        self.found.entry(id).or_default().add(index, make);
    }

    /// The kids ISO 32000-1 Tables 336 and 337 allow in tables and lists, for `e` opening in
    /// `parent`, whose state is the innermost [`Open`].
    fn nesting(&mut self, parent: &Frame, e: &Frame) {
        let Some(open) = self.open.last_mut() else { return };
        let first = open.kids == 0;
        open.kids = open.kids.saturating_add(1);
        let late_caption = open.late_caption.take();
        let caption = e.ty.as_slice() == b"Caption";
        let misplaced = || structure::worded(e, "not allowed directly inside {in}", vec![("{in}", pdf_name(&parent.ty))]);
        let mut found: Vec<(&'static str, Finding)> = Vec::new();
        if let Some((index, f)) = late_caption {
            // A kid after a Caption that wasn't the first: the Caption isn't the last either.
            self.found.entry("09-004").or_default().add(index, || f);
        }
        let Some(open) = self.open.last_mut() else { return };
        if reported_elsewhere(parent.kind, e.kind) {
            return;
        }
        match parent.kind {
            Kind::Tr if !matches!(e.kind, Kind::Th | Kind::Td) => found.push(("09-004", misplaced())),
            Kind::Section if e.kind != Kind::Tr => found.push(("09-004", misplaced())),
            Kind::Table => match e.kind {
                Kind::Tr => open.bare_rows = true,
                Kind::Section => {
                    open.row_groups = true;
                    let count = match e.ty.as_slice() {
                        b"THead" => Some(&mut open.heads),
                        b"TFoot" => Some(&mut open.foots),
                        _ => None,
                    };
                    if let Some(n) = count {
                        *n = n.saturating_add(1);
                        if *n == 2 {
                            found.push(("09-004", structure::worded(e, "a second {} in its table", vec![("{}", pdf_name(&e.ty))])));
                        }
                    }
                }
                _ if caption => {
                    if !first {
                        open.late_caption = Some((e.index, structure::finding(e, "not the first or last child of its table")));
                    }
                }
                _ => found.push(("09-004", misplaced())),
            },
            Kind::L => match e.kind {
                Kind::Li | Kind::L => {}
                _ if caption => {
                    if !first {
                        found.push(("09-005", structure::finding(e, "not the first child of its list")));
                    }
                }
                _ => found.push(("09-005", misplaced())),
            },
            Kind::Li if !matches!(e.kind, Kind::Lbl | Kind::LBody) => found.push(("09-005", misplaced())),
            _ => {}
        }
        for (id, f) in found {
            self.add(id, e.index, || f);
        }
    }
}

impl Visitor for Tree<'_> {
    fn enter(&mut self, ancestors: &[Frame], e: &Frame) {
        if self.raw_types.len() < MAX_KEPT {
            self.raw_types.insert(e.raw.clone());
        }
        self.any_lang |= e.lang;
        let parent = ancestors.last();
        let parent_ty = parent.map(|p| p.ty.as_slice());
        if let Some(p) = parent {
            self.nesting(p, e);
        }
        if e.kind == Kind::LBody && !matches!(parent.map(|p| p.kind), Some(Kind::Li | Kind::NoteOrToc)) {
            self.add("09-005", e.index, || structure::finding(e, "not in an LI (list item)"));
        }
        let lang = e.lang || self.open.last().is_some_and(|o| o.lang);
        self.open.push(Open { lang, ..Open::default() });
        if !self.doc_lang && !lang && (e.alt || e.expansion) {
            self.add("11-002", e.index, || structure::finding(e, "its alternate text, actual text or expansion has no language (Lang)"));
        }
        match e.ty.as_slice() {
            b"H" => {
                self.plain_heading = true;
                let depth = self.open.len().saturating_sub(2);
                if let (Some(p), Some(open)) = (parent, self.open.get_mut(depth)) {
                    open.plain_headings = open.plain_headings.saturating_add(1);
                    if open.plain_headings == 2 {
                        self.add("14-006", p.index, || structure::finding(p, "more than one H heading in it"));
                    }
                }
            }
            b"TOCI" if parent_ty != Some(b"TOC") => self.add("09-006", e.index, || structure::finding(e, "not in a TOC")),
            b"RB" | b"RT" | b"RP" if parent_ty != Some(b"Ruby") => self.add("09-007", e.index, || structure::finding(e, "not in a Ruby")),
            b"WT" | b"WP" if parent_ty != Some(b"Warichu") => self.add("09-008", e.index, || structure::finding(e, "not in a Warichu")),
            b"Note" => match &e.id {
                None => self.add("19-003", e.index, || structure::finding(e, "no ID")),
                Some(id) if self.note_ids.len() < MAX_KEPT && !self.note_ids.insert(id.clone()) => {
                    self.add("19-004", e.index, || structure::finding(e, "its ID is another Note's too"))
                }
                Some(_) => {}
            },
            _ => {}
        }
        if parent_ty == Some(b"TOC") && !matches!(e.ty.as_slice(), b"TOC" | b"TOCI" | b"Caption") {
            self.add("09-006", e.index, || structure::finding(e, "not allowed directly in a TOC"));
        }
        match e.kind {
            Kind::Heading(_) => self.numbered_heading = true,
            Kind::Section if parent.map(|p| p.kind) != Some(Kind::Table) => {
                self.add("09-004", e.index, || structure::finding(e, "not in a Table"));
            }
            Kind::Table => self.tables.push(Heads { depth: ancestors.len(), headers: false, unscoped: Vec::new(), more: 0 }),
            Kind::Th | Kind::Td => {
                if let Some(table) = self.tables.last_mut() {
                    table.headers |= e.headers;
                    if e.kind == Kind::Th && !e.scope {
                        if table.unscoped.len() < MAX_FINDINGS {
                            table.unscoped.push((e.index, structure::finding(e, "a header cell with no Scope, in a table that doesn't use Headers")));
                        } else {
                            table.more = table.more.saturating_add(1);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn exit(&mut self, ancestors: &[Frame], e: &Frame) {
        let open = self.open.pop().unwrap_or_default();
        // Text in the page content needs a language: from the tags, or from a marked-content
        // property list around the text (ISO 32000-1 §14.9.2.1). Marked content without text
        // needs none, and a tag that only holds an annotation (OBJR) is checked under 11-004 and
        // 11-005, with the annotation. Marked content the walk couldn't look at (in another
        // stream, or not found) counts as text without a language.
        let unread = open.unread || (e.marked && open.told == 0);
        if !self.doc_lang && !open.lang && (open.unlabelled_text || unread) {
            let what = if e.bad_lang { "its language (Lang) isn't a language tag" } else { "no language (Lang) on it, above it or on the document" };
            self.add("11-001", e.index, || structure::finding(e, what));
        }
        if e.kind == Kind::Table && open.bare_rows && open.row_groups {
            self.add("09-004", e.index, || structure::finding(e, "rows both directly in it and in THead, TBody or TFoot"));
        }
        if e.kind == Kind::Table && self.tables.last().is_some_and(|t| t.depth == ancestors.len()) {
            let Some(table) = self.tables.pop() else { return };
            if !table.headers {
                let found = self.found.entry("15-003").or_default();
                for (index, finding) in table.unscoped {
                    found.add(index, || finding);
                }
                found.skip(table.more);
            }
        }
    }

    fn marked_content(&mut self, _open: &[Frame], page: Option<usize>, mcid: i64) {
        let seen = page.and_then(|p| self.scan.mcid_text.get(&p)).and_then(|m| m.get(&mcid)).copied();
        if let Some(o) = self.open.last_mut() {
            o.told = o.told.saturating_add(1);
            match seen {
                Some(t) => o.unlabelled_text |= t.text && t.unlabelled,
                None => o.unread = true,
            }
        }
    }

    fn object_reference(&mut self, open: &[Frame], obj: ObjRef) {
        let Some(e) = open.last() else { return };
        if self.owners.len() < MAX_KEPT && !self.owners.contains_key(&obj) {
            let lang = self.open.last().is_some_and(|o| o.lang);
            self.owners.insert(obj, Owner { ty: e.ty.clone(), alt: e.alt, lang });
        }
    }
}

/// Whether annotation `a` has some area inside `crop`. A rectangle of no width or height, or one
/// wholly outside the page, shows nothing; a missing or malformed one is taken to show.
fn shows_on(doc: &Document, a: &Dict, crop: [f64; 4]) -> bool {
    let Some(r) = a.get(b"Rect").map(|r| doc.resolve(r)) else { return true };
    let Some(v) = r.as_array().map(|v| v.iter().filter_map(|x| doc.resolve(x).as_f64()).collect::<Vec<f64>>()) else { return true };
    let [x0, y0, x1, y1] = v.as_slice() else { return true };
    if ![x0, y0, x1, y1].iter().all(|x| x.is_finite()) {
        return true;
    }
    let (left, right, bottom, top) = (x0.min(*x1), x0.max(*x1), y0.min(*y1), y0.max(*y1));
    left < right && bottom < top && left < crop[2] && right > crop[0] && bottom < crop[3] && top > crop[1]
}

fn pdf_name(n: &[u8]) -> String {
    String::from_utf8_lossy(n).into_owned()
}

/// The annotation conditions, page by page.
fn annotations(doc: &Document, pages: &[Page], owners: &HashMap<ObjRef, Owner>, doc_lang: bool, c: &mut Collected) {
    let mut fields_seen = HashSet::new();
    for (p, page) in pages.iter().enumerate() {
        let list = page.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        let annots: Vec<(Option<ObjRef>, Dict)> = list.iter().filter_map(|x| doc.resolve(x).as_dict().cloned().map(|d| (x.as_ref(), d))).collect();
        if annots.is_empty() {
            continue;
        }
        let on_page = ("{p}", (p + 1).to_string());
        match page.dict.get(b"Tabs").map(|t| doc.resolve(t)).as_deref().map(|t| t.as_name().map(<[u8]>::to_vec)) {
            None => c.add("28-008", Finding::worded(Some(p), "Page {p} has annotations but no tab order (Tabs)", vec![on_page.clone()])),
            Some(Some(t)) if t == b"S" => {}
            Some(t) => c.add(
                "28-009",
                Finding::worded(
                    Some(p),
                    "Page {p} has annotations and tabs in {} order, not structure order (S)",
                    vec![("{}", t.map(|t| pdf_name(&t)).unwrap_or_default()), on_page.clone()],
                ),
            ),
        }
        for (r, a) in annots {
            let subtype = a.name(b"Subtype").unwrap_or(b"").to_vec();
            let owner = r.and_then(|r| owners.get(&r));
            let hidden = a.get(b"F").and_then(|f| doc.resolve(f).as_int()).is_some_and(|f| f & 2 != 0) || !shows_on(doc, &a, page.crop(doc));
            let place = |pattern: &'static str, mut values: Vec<(&'static str, String)>| {
                values.push(on_page.clone());
                let mut f = Finding::worded(Some(p), pattern, values);
                f.object = r;
                f.bbox = content::annotation_rect(doc, &a);
                f
            };
            let described = content::text_of(doc, &a, b"Contents").is_some();
            let kind = ("{}", pdf_name(&subtype));
            // An empty F or UF names no file.
            let names_file = |fs: &Dict, key: &[u8]| content::text_of(doc, fs, key).is_some();
            if subtype == b"FileAttachment" && dict_at(doc, &a, b"FS").is_some_and(|fs| !(names_file(&fs, b"F") && names_file(&fs, b"UF"))) {
                c.add("28-016", place("File attachment annotation on page {p}: its file specification lacks F or UF", Vec::new()));
            }
            match subtype.as_slice() {
                b"TrapNet" => c.add("28-007", place("Page {p} has a trap network annotation (TrapNet)", Vec::new())),
                b"PrinterMark" => {
                    if owner.is_some() || a.contains(b"StructParent") {
                        c.add("28-017", place("Printer's mark annotation on page {p} is in the structure tree", Vec::new()));
                    }
                }
                // Popups belong to their parent annotation and are not tagged; hidden ones, and
                // ones with no area on the page, are not presented at all.
                b"Popup" => {}
                _ if hidden => {}
                b"Widget" => {
                    let want = b"Form".as_slice();
                    match owner {
                        None => c.add("28-010", place("Form field on page {p} isn't tagged", Vec::new())),
                        Some(o) if o.ty != want => c.add(
                            "28-010",
                            place(
                                "{} annotation on page {p} is in a {ty} tag, not {want}",
                                vec![kind.clone(), ("{ty}", pdf_name(&o.ty)), ("{want}", pdf_name(want))],
                            ),
                        ),
                        Some(_) => {}
                    }
                    // The field is the widget when it has a name, else its parent.
                    let field = if a.contains(b"T") { r } else { a.get(b"Parent").and_then(Object::as_ref) };
                    if field.is_some_and(|f| !fields_seen.insert(f)) {
                        continue;
                    }
                    let field_dict = field.and_then(|f| doc.get(f).as_dict().cloned()).unwrap_or_else(|| a.clone());
                    let tooltip = content::text_of(doc, &field_dict, b"TU").is_some();
                    let name = ("{}", field.map(|f| content::full_name(doc, f)).unwrap_or_default());
                    if !tooltip && !owner.is_some_and(|o| o.alt) {
                        c.add("28-005", place("Field \"{}\" on page {p} has no description (TU), and its tag no alternate text", vec![name.clone()]));
                    }
                    if tooltip && !doc_lang && !owner.is_some_and(|o| o.lang) {
                        c.add("11-005", place("Field \"{}\" on page {p}: its description has no language (Lang)", vec![name]));
                    }
                }
                b"Link" => {
                    let want = b"Link".as_slice();
                    match owner {
                        None => c.add("28-011", place("{} annotation on page {p} isn't tagged", vec![kind.clone()])),
                        Some(o) if o.ty != want => c.add(
                            "28-011",
                            place(
                                "{} annotation on page {p} is in a {ty} tag, not {want}",
                                vec![kind.clone(), ("{ty}", pdf_name(&o.ty)), ("{want}", pdf_name(want))],
                            ),
                        ),
                        Some(_) => {}
                    }
                    if !described {
                        c.add("28-012", place("Link annotation on page {p} has no description (Contents)", Vec::new()));
                    } else if !doc_lang && !owner.is_some_and(|o| o.lang) {
                        c.add("11-004", place("{} annotation on page {p}: its description has no language (Lang)", vec![kind.clone()]));
                    }
                }
                _ => {
                    let want = b"Annot".as_slice();
                    match owner {
                        None => c.add("28-002", place("{} annotation on page {p} isn't tagged", vec![kind.clone()])),
                        Some(o) if o.ty != want => c.add(
                            "28-002",
                            place(
                                "{} annotation on page {p} is in a {ty} tag, not {want}",
                                vec![kind.clone(), ("{ty}", pdf_name(&o.ty)), ("{want}", pdf_name(want))],
                            ),
                        ),
                        Some(_) => {}
                    }
                    if !described && !owner.is_some_and(|o| o.alt) {
                        c.add(
                            "28-004",
                            place("{} annotation on page {p} has no description (Contents), and its tag no alternate text", vec![kind.clone()]),
                        );
                    } else if described && !doc_lang && !owner.is_some_and(|o| o.lang) {
                        c.add("11-004", place("{} annotation on page {p}: its description has no language (Lang)", vec![kind.clone()]));
                    }
                }
            }
        }
    }
}

/// The conditions the page content decides: marked-content nesting, form XObjects and fonts.
fn page_content(doc: &Document, scan: &Scan, c: &mut Collected) {
    for (p, n) in &scan.artifacts_in_tagged {
        let pattern = if *n == 1 { "Page {p}: {n} artifact inside tagged content" } else { "Page {p}: {n} artifacts inside tagged content" };
        c.add("01-003", Finding::worded(Some(*p), pattern, vec![("{p}", (p + 1).to_string()), ("{n}", n.to_string())]));
    }
    for (p, n) in &scan.tagged_in_artifacts {
        let pattern =
            if *n == 1 { "Page {p}: {n} tagged content item inside an artifact" } else { "Page {p}: {n} tagged content items inside an artifact" };
        c.add("01-004", Finding::worded(Some(*p), pattern, vec![("{p}", (p + 1).to_string()), ("{n}", n.to_string())]));
    }
    let mut reused: Vec<(ObjRef, usize)> =
        scan.form_uses.iter().filter(|(form, uses)| **uses > 1 && scan.forms_with_mcid.contains(form)).map(|(form, uses)| (*form, *uses)).collect();
    reused.sort();
    for (form, uses) in reused {
        let mut f = Finding::worded(None, "A form XObject with marked content (MCIDs) is drawn {n} times", vec![("{n}", uses.to_string())]);
        f.object = Some(form);
        c.add("30-002", f);
    }
    let mut seen = HashSet::new();
    for (p, fonts) in &scan.rendered_fonts {
        for r in fonts {
            if !seen.insert(*r) {
                continue;
            }
            let Some(font) = doc.get(*r).as_dict().cloned() else { continue };
            if embedded(doc, &font) {
                continue;
            }
            let page = ("{p}", (p + 1).to_string());
            let mut f = match font.name(b"BaseFont") {
                Some(n) => Finding::worded(Some(*p), "Font {} on page {p} isn't embedded", vec![("{}", pdf_name(n)), page]),
                None => Finding::worded(Some(*p), "Font (unnamed) on page {p} isn't embedded", vec![page]),
            };
            f.object = Some(*r);
            c.add("31-009", f);
        }
    }
}

/// Whether a font's program is in the file. A Type 3 font's glyphs always are; a Type 0 font's
/// program is its descendant CIDFont's.
fn embedded(doc: &Document, font: &Dict) -> bool {
    let program = match font.name(b"Subtype") {
        Some(b"Type3") => return true,
        Some(b"Type0") => {
            let descendant = font.get(b"DescendantFonts").map(|d| doc.resolve(d)).and_then(|d| d.as_array().and_then(|a| a.first().cloned()));
            match descendant.and_then(|d| doc.resolve(&d).as_dict().cloned()) {
                Some(d) => d,
                None => return false,
            }
        }
        _ => font.clone(),
    };
    dict_at(doc, &program, b"FontDescriptor").is_some_and(|d| d.contains(b"FontFile") || d.contains(b"FontFile2") || d.contains(b"FontFile3"))
}

/// The conditions on objects anywhere in the file: optional content, embedded files, XFA, security
/// and reference XObjects.
fn document_objects(doc: &Document, cat: &Dict, c: &mut Collected) {
    if let Some(ocp) = dict_at(doc, cat, b"OCProperties") {
        let unnamed = |d: &Dict| content::text_of(doc, d, b"Name").is_none();
        if let Some(d) = dict_at(doc, &ocp, b"D") {
            if unnamed(&d) {
                c.add("20-002", Finding::plain(None, "The default optional content configuration (D) has no Name"));
            }
            if d.contains(b"AS") {
                c.add("20-003", Finding::plain(None, "An optional content configuration has an AS entry"));
            }
        }
        let configs = ocp.get(b"Configs").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
        for (i, config) in configs.iter().enumerate() {
            let Some(d) = doc.resolve(config).as_dict().cloned() else { continue };
            if unnamed(&d) {
                c.add("20-001", Finding::worded(None, "Optional content configuration {n} has no Name", vec![("{n}", (i + 1).to_string())]));
            }
            if d.contains(b"AS") {
                c.add("20-003", Finding::plain(None, "An optional content configuration has an AS entry"));
            }
        }
    }
    embedded_files(doc, cat, c);
    if xfa_requires_dynamic_render(doc, cat) {
        c.add("25-001", Finding::plain(None, "The XFA form requires dynamic rendering (dynamicRender)"));
    }
    if let Some(encrypt) = dict_at(doc, doc.trailer(), b"Encrypt") {
        security(doc, &encrypt, c);
    }
    for (r, obj) in doc.scan_objects() {
        if let Object::Stream(s) = &*obj
            && s.dict.name(b"Subtype") == Some(b"Form")
            && s.dict.contains(b"Ref")
        {
            let mut f = Finding::plain(None, "The file has a reference XObject (Ref)");
            f.object = Some(r);
            c.add("30-001", f);
        }
    }
}

/// The security conditions of an encrypted file, from its encryption dictionary: it must say what
/// is permitted (P), and permit assistive technology to read the text (bit 10).
pub(super) fn security(doc: &Document, encrypt: &Dict, c: &mut Collected) {
    match encrypt.get(b"P").and_then(|p| doc.resolve(p).as_int()) {
        None => c.add("26-001", Finding::plain(None, "The file is encrypted but its encryption dictionary has no permissions (P)")),
        Some(p) if p & (1 << 9) == 0 => c.add("26-002", Finding::plain(None, "The security settings don't allow screen readers to read the text")),
        Some(_) => {}
    }
}

/// The embedded files of the EmbeddedFiles name tree: each file specification needs F and UF.
fn embedded_files(doc: &Document, cat: &Dict, c: &mut Collected) {
    let Some(root) = dict_at(doc, cat, b"Names").and_then(|n| dict_at(doc, &n, b"EmbeddedFiles")) else { return };
    let mut stack = vec![(root, 0usize)];
    let mut seen: HashSet<ObjRef> = HashSet::new();
    let mut visited = 0usize;
    while let Some((node, depth)) = stack.pop() {
        visited = visited.saturating_add(1);
        if visited > MAX_NAME_NODES {
            break;
        }
        let names = node.get(b"Names").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
        for pair in names.chunks(2) {
            let [key, value] = pair else { continue };
            let Some(spec) = doc.resolve(value).as_dict().cloned() else { continue };
            if !(spec.contains(b"F") && spec.contains(b"UF")) {
                let name = doc.resolve(key).as_string().map(|s| s.to_text()).unwrap_or_default();
                let mut f = Finding::worded(None, "Embedded file \"{}\": its file specification lacks F or UF", vec![("{}", name)]);
                f.object = value.as_ref();
                c.add("21-001", f);
            }
        }
        if depth >= MAX_NAME_DEPTH {
            continue;
        }
        let kids = node.get(b"Kids").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
        for kid in kids.iter().rev() {
            if kid.as_ref().is_some_and(|r| !seen.insert(r)) {
                continue;
            }
            if let Some(d) = doc.resolve(kid).as_dict().cloned() {
                stack.push((d, depth + 1));
            }
        }
    }
}
