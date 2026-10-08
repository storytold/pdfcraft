//! The structure tree (tags) as the checker needs it, and the rules that read it: alternate
//! text, tables, lists and headings (ISO 32000-2 §14.7–14.8).

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use pdfcraft_model::Page;

use crate::{Finding, Rule};

/// Standard structure types (PDF 1.7 and 2.0); anything else goes through the role map.
const STANDARD: &[&[u8]] = &[
    b"Document",
    b"DocumentFragment",
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
    b"Aside",
    b"Title",
    b"FENote",
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
    b"Sub",
    b"Em",
    b"Strong",
    b"Artifact",
];

pub(crate) fn is_standard(t: &[u8]) -> bool {
    STANDARD.contains(&t)
}

/// Elements beyond this are not read (a guard against absurd trees).
const MAX_ELEMENTS: usize = 2_000_000;

#[derive(Clone, Debug, Default)]
pub(crate) struct Elem {
    /// The standard type after the role map (or the type as written if it maps to nothing).
    pub ty: Vec<u8>,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// Has /Alt (or /ActualText, which also replaces the content).
    pub alt: bool,
    /// Marked content or an object reference directly under it.
    pub content: bool,
    /// Annotations it references (OBJR).
    pub annots: Vec<ObjRef>,
    pub page: Option<usize>,
    pub attrs: Vec<Dict>,
}

#[derive(Debug, Default)]
pub(crate) struct Tree {
    /// A /StructTreeRoot exists.
    pub exists: bool,
    pub elems: Vec<Elem>,
    /// Top-level elements in order.
    pub roots: Vec<usize>,
}

impl Tree {
    pub fn read(doc: &Document, pages: &[Page]) -> Tree {
        let mut t = Tree::default();
        let Some(cat) = doc.root().and_then(|r| doc.get(r).as_dict().cloned()) else { return t };
        let Some(root) = cat.get(b"StructTreeRoot").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()) else { return t };
        t.exists = true;
        let role_map = root.get(b"RoleMap").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
        let page_index: HashMap<ObjRef, usize> = pages.iter().enumerate().map(|(i, p)| (p.obj, i)).collect();
        let mut seen = HashSet::new();
        let mut reader = Reader { doc, role_map: &role_map, page_index: &page_index, seen: &mut seen };
        if let Some(k) = root.get(b"K") {
            let mut roots = Vec::new();
            reader.kids(&mut t, k, None, None, &mut roots, 0);
            t.roots = roots;
        }
        t
    }

    /// Elements in document (pre-)order.
    fn preorder(&self) -> Vec<usize> {
        let mut out = Vec::with_capacity(self.elems.len());
        let mut stack: Vec<usize> = self.roots.iter().rev().copied().collect();
        while let Some(i) = stack.pop() {
            out.push(i);
            stack.extend(self.elems[i].children.iter().rev().copied());
        }
        out
    }

    fn ancestors(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        std::iter::successors(self.elems[i].parent, move |p| self.elems[*p].parent)
    }

    /// `i` and everything under it.
    fn subtree(&self, i: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = vec![i];
        while let Some(j) = stack.pop() {
            out.push(j);
            stack.extend(self.elems[j].children.iter().copied());
        }
        out
    }

    fn is(&self, i: usize, ty: &[u8]) -> bool {
        self.elems[i].ty == ty
    }

    fn parent_is(&self, i: usize, types: &[&[u8]]) -> bool {
        self.elems[i].parent.is_some_and(|p| types.contains(&self.elems[p].ty.as_slice()))
    }

    fn finding(&self, i: usize, what: &str) -> Finding {
        let e = &self.elems[i];
        let ty = String::from_utf8_lossy(&e.ty);
        let at = e.page.map(|p| format!(" on page {}", p + 1)).unwrap_or_default();
        Finding { page: e.page, message: format!("{ty} element{at}: {what}") }
    }

    pub fn rule(&self, rule: Rule) -> Vec<Finding> {
        let order = self.preorder();
        let mut out = Vec::new();
        match rule {
            Rule::FiguresAltText => {
                for &i in &order {
                    if self.is(i, b"Figure") && !self.elems[i].alt && !self.ancestors(i).any(|a| self.elems[a].alt) {
                        out.push(self.finding(i, "no alternate text"));
                    }
                }
            }
            Rule::OtherElementsAltText => {
                for &i in &order {
                    if self.is(i, b"Formula") && !self.elems[i].alt && !self.ancestors(i).any(|a| self.elems[a].alt) {
                        out.push(self.finding(i, "no alternate text"));
                    }
                }
            }
            Rule::NestedAltText => {
                for &i in &order {
                    if self.elems[i].alt && self.ancestors(i).any(|a| self.elems[a].alt) {
                        out.push(self.finding(i, "its alternate text is inside another element's and will never be read"));
                    }
                }
            }
            Rule::AltTextAssociated => {
                for &i in &order {
                    if self.elems[i].alt && !self.subtree(i).iter().any(|j| self.elems[*j].content) {
                        out.push(self.finding(i, "alternate text on an element with no page content"));
                    }
                }
            }
            Rule::AltTextHidesAnnotation => {
                for &i in &order {
                    if self.elems[i].alt && self.subtree(i).iter().any(|j| !self.elems[*j].annots.is_empty()) {
                        out.push(self.finding(i, "alternate text hides an annotation inside it"));
                    }
                }
            }
            Rule::TableRows => {
                for &i in &order {
                    if self.is(i, b"TR") && !self.parent_is(i, &[b"Table", b"THead", b"TBody", b"TFoot"]) {
                        out.push(self.finding(i, "not in a Table, THead, TBody or TFoot"));
                    }
                }
            }
            Rule::TableCells => {
                for &i in &order {
                    if (self.is(i, b"TH") || self.is(i, b"TD")) && !self.parent_is(i, &[b"TR"]) {
                        out.push(self.finding(i, "not in a TR"));
                    }
                }
            }
            Rule::TableHeaders => {
                for &i in &order {
                    if self.is(i, b"Table") && !self.table_cells(i).iter().any(|(_, c)| self.is(*c, b"TH")) {
                        out.push(self.finding(i, "no header cells (TH)"));
                    }
                }
            }
            Rule::TableRegularity => {
                for &i in &order {
                    if self.is(i, b"Table")
                        && let Some(why) = self.irregular(i)
                    {
                        out.push(self.finding(i, &why));
                    }
                }
            }
            Rule::TableSummary => {
                for &i in &order {
                    if self.is(i, b"Table") && self.attr(i, b"Summary").is_none() {
                        out.push(self.finding(i, "no summary"));
                    }
                }
            }
            Rule::ListItems => {
                for &i in &order {
                    if self.is(i, b"LI") && !self.parent_is(i, &[b"L"]) {
                        out.push(self.finding(i, "not in an L (list)"));
                    }
                }
            }
            Rule::LblLBody => {
                for &i in &order {
                    if (self.is(i, b"Lbl") || self.is(i, b"LBody")) && !self.parent_is(i, &[b"LI"]) && !self.inside_toc_or_note(i) {
                        out.push(self.finding(i, "not in an LI (list item)"));
                    }
                }
            }
            Rule::HeadingNesting => {
                let mut prev = 0u8;
                for &i in &order {
                    let level = match self.elems[i].ty.as_slice() {
                        [b'H', d @ b'1'..=b'6'] => d - b'0',
                        _ => continue,
                    };
                    if level > prev + 1 {
                        let what = if prev == 0 { format!("the first heading is an H{level}") } else { format!("H{level} follows an H{prev}") };
                        out.push(self.finding(i, &format!("{what}; headings should not skip levels")));
                    }
                    prev = level;
                }
            }
            _ => {}
        }
        out
    }

    /// Lbl is also allowed in TOCI and Note-like contexts (PDF 2.0); only list misuse is flagged.
    fn inside_toc_or_note(&self, i: usize) -> bool {
        self.parent_is(i, &[b"TOCI", b"BibEntry", b"Note", b"FENote"])
    }

    /// An attribute from the element's attribute objects (/A), any owner.
    fn attr(&self, i: usize, key: &[u8]) -> Option<&Object> {
        self.elems[i].attrs.iter().find_map(|d| d.get(key))
    }

    fn span(&self, i: usize, key: &[u8]) -> usize {
        self.attr(i, key).and_then(Object::as_int).filter(|n| *n >= 1).map_or(1, |n| n.min(10_000) as usize)
    }

    /// The rows of a table (through THead/TBody/TFoot, not into nested tables) and their cells.
    fn table_cells(&self, table: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (r, row) in self.table_rows(table).into_iter().enumerate() {
            for &c in &self.elems[row].children {
                if self.is(c, b"TH") || self.is(c, b"TD") {
                    out.push((r, c));
                }
            }
        }
        out
    }

    fn table_rows(&self, table: usize) -> Vec<usize> {
        let mut rows = Vec::new();
        for &k in &self.elems[table].children {
            match self.elems[k].ty.as_slice() {
                b"TR" => rows.push(k),
                b"THead" | b"TBody" | b"TFoot" => rows.extend(self.elems[k].children.iter().copied().filter(|r| self.is(*r, b"TR"))),
                _ => {}
            }
        }
        rows
    }

    /// Why the table's rows don't all have the same width, counting row and column spans.
    fn irregular(&self, table: usize) -> Option<String> {
        let rows = self.table_rows(table);
        if rows.len() < 2 {
            return None;
        }
        // carry[c] = rows still covered at column c by a cell above.
        let mut carry: Vec<usize> = Vec::new();
        let mut widths = Vec::with_capacity(rows.len());
        for &row in &rows {
            let mut col = 0;
            let mut next = carry.iter().map(|n| n.saturating_sub(1)).collect::<Vec<_>>();
            for &c in self.elems[row].children.iter().filter(|c| self.is(**c, b"TH") || self.is(**c, b"TD")) {
                while col < carry.len() && carry[col] > 0 {
                    col += 1;
                }
                let (cs, rs) = (self.span(c, b"ColSpan"), self.span(c, b"RowSpan"));
                if next.len() < col + cs {
                    next.resize(col + cs, 0);
                }
                for n in next.iter_mut().skip(col).take(cs) {
                    *n = (*n).max(rs - 1);
                }
                col += cs;
            }
            while col < carry.len() && carry[col] > 0 {
                col += 1;
            }
            widths.push(col);
            carry = next;
        }
        let (min, max) = (widths.iter().min()?, widths.iter().max()?);
        (min != max).then(|| format!("rows have between {min} and {max} columns"))
    }
}

struct Reader<'a> {
    doc: &'a Document,
    role_map: &'a Dict,
    page_index: &'a HashMap<ObjRef, usize>,
    seen: &'a mut HashSet<ObjRef>,
}

impl Reader<'_> {
    fn standard(&self, ty: &[u8]) -> Vec<u8> {
        let mut t = ty.to_vec();
        for _ in 0..12 {
            if STANDARD.contains(&t.as_slice()) {
                return t;
            }
            match self.role_map.get(&t).map(|o| self.doc.resolve(o)).and_then(|o| o.as_name().map(<[u8]>::to_vec)) {
                Some(n) if n != t => t = n,
                _ => break,
            }
        }
        t
    }

    fn page_of(&self, d: &Dict) -> Option<usize> {
        d.get(b"Pg").and_then(Object::as_ref).and_then(|r| self.page_index.get(&r).copied())
    }

    /// Read the kids `k` of `parent` (or of the root), appending element indexes to `out`.
    fn kids(&mut self, t: &mut Tree, k: &Object, parent: Option<usize>, page: Option<usize>, out: &mut Vec<usize>, depth: usize) {
        if depth > 256 || t.elems.len() >= MAX_ELEMENTS {
            return;
        }
        match k {
            Object::Array(a) => {
                for x in a {
                    self.kids(t, x, parent, page, out, depth + 1);
                }
            }
            Object::Int(_) => {
                if let Some(p) = parent {
                    t.elems[p].content = true;
                }
            }
            Object::Ref(r) => {
                if !self.seen.insert(*r) {
                    return;
                }
                let obj = self.doc.get(*r);
                if let Some(d) = obj.as_dict() {
                    self.dict(t, d, parent, page, out, depth);
                }
            }
            Object::Dict(d) => self.dict(t, d, parent, page, out, depth),
            _ => {}
        }
    }

    fn dict(&mut self, t: &mut Tree, d: &Dict, parent: Option<usize>, page: Option<usize>, out: &mut Vec<usize>, depth: usize) {
        match d.name(b"Type") {
            Some(b"MCR") => {
                if let Some(p) = parent {
                    t.elems[p].content = true;
                }
                return;
            }
            Some(b"OBJR") => {
                if let Some(p) = parent {
                    t.elems[p].content = true;
                    if let Some(o) = d.get(b"Obj").and_then(Object::as_ref)
                        && self.doc.get(o).as_dict().is_some_and(|a| a.contains(b"Subtype") && a.contains(b"Rect"))
                    {
                        t.elems[p].annots.push(o);
                    }
                }
                return;
            }
            _ => {}
        }
        let Some(s) = d.name(b"S") else { return };
        let page = self.page_of(d).or(page);
        let text =
            |k: &[u8]| d.get(k).map(|o| self.doc.resolve(o)).and_then(|o| o.as_string().map(|s| !s.to_text().trim().is_empty())).unwrap_or(false);
        let mut attrs = Vec::new();
        if let Some(a) = d.get(b"A").map(|o| self.doc.resolve(o)) {
            let list = match &*a {
                Object::Array(v) => v.clone(),
                other => vec![other.clone()],
            };
            for x in list {
                if let Some(ad) = self.doc.resolve(&x).as_dict() {
                    attrs.push(ad.clone());
                }
            }
        }
        let i = t.elems.len();
        t.elems.push(Elem { ty: self.standard(s), parent, alt: text(b"Alt") || text(b"ActualText"), page, attrs, ..Elem::default() });
        if let Some(p) = parent {
            t.elems[p].children.push(i);
        }
        out.push(i);
        if let Some(k) = d.get(b"K") {
            let mut ignored = Vec::new();
            self.kids(t, k, Some(i), page, &mut ignored, depth + 1);
        }
    }
}
