//! The document's headings (H, H1–H6 after the role map) with the text they mark: what New
//! Bookmarks from Structure turns into bookmarks.

use std::collections::{BTreeSet, HashMap, HashSet};

use pdfcraft_content::Matrix;
use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object};
use pdfcraft_fonts::pdf::Metrics;
use pdfcraft_model::Page;

use crate::alt::{page_content_within, standard, tree_root};

const MAX_HEADINGS: usize = 10_000;
const MAX_VISITS: usize = 1_000_000;
const MAX_DEPTH: usize = 256;
/// Characters kept of a heading's title.
const MAX_TITLE: usize = 256;
const MAX_TAGGED_TEXT: usize = 32 * 1024 * 1024;
const MAX_TAGGED_WARNINGS: usize = 100;
const MAX_TAGGED_TASKS: usize = 100_000;
const MAX_TAGGED_FORM_DEPTH: usize = 12;

/// Text in structure-tree order and repairs or omissions found while reading it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaggedText {
    pub text: String,
    pub warnings: Vec<String>,
}

#[derive(Clone, Default)]
pub(crate) struct McidTexts {
    pub(crate) by_id: HashMap<i64, String>,
    pub(crate) form_texts: HashMap<ObjRef, McidTexts>,
    bytes: usize,
    pub(crate) limited: bool,
    pub(crate) unmarked_text: bool,
}

fn tagged_warning(out: &mut TaggedText, warning: String) {
    if out.warnings.contains(&warning) {
        return;
    }
    if out.warnings.len() < MAX_TAGGED_WARNINGS {
        out.warnings.push(warning);
    } else if out.warnings.len() == MAX_TAGGED_WARNINGS {
        out.warnings.push("additional tagged-text warnings were omitted".into());
    }
}

/// Read reachable marked content in tree order. Artifacts are absent because they have no
/// structure-tree reference. Cycles, missing objects, and limits are reported instead of guessed.
pub fn tagged_text(doc: &Document, selected_pages: &[usize]) -> TaggedText {
    tagged_text_with_limit(doc, selected_pages, MAX_TAGGED_TEXT)
}

pub(crate) fn tagged_text_with_limit(doc: &Document, selected_pages: &[usize], text_limit: usize) -> TaggedText {
    let Some(root) = tree_root(doc) else {
        return TaggedText { text: String::new(), warnings: vec!["the document has no structure tree".into()] };
    };
    let pages = pdfcraft_model::pages(doc);
    let page_index: HashMap<ObjRef, usize> = pages.iter().enumerate().map(|(i, p)| (p.obj, i)).collect();
    let selected: HashSet<usize> = selected_pages.iter().copied().collect();
    let mark_priorities = structure_mark_priorities(doc, &root, &page_index, &selected);
    let mut walker = TaggedWalker::new(doc, &pages, page_index, selected_pages, &root, mark_priorities, text_limit);
    let mut tasks = root.get(b"K").cloned().map(|k| vec![(k, None, 0, None, None)]).unwrap_or_default();
    if tasks.is_empty() {
        tagged_warning(&mut walker.out, "the structure tree has no root content".into());
    }
    while let Some((item, page, depth, element, stream)) = tasks.pop() {
        walker.visit(item, page, depth, element, stream, &mut tasks);
    }
    walker.finish(&pages)
}

type TaggedTask = (Object, Option<ObjRef>, usize, Option<usize>, Option<ObjRef>);
type MarkPriorities = HashMap<(usize, Option<ObjRef>), HashMap<i64, usize>>;

fn structure_mark_priorities(doc: &Document, root: &Dict, page_index: &HashMap<ObjRef, usize>, selected: &HashSet<usize>) -> MarkPriorities {
    let mut priorities = MarkPriorities::new();
    let mut tasks = root.get(b"K").cloned().map(|item| vec![(item, None, 0usize)]).unwrap_or_default();
    let mut seen = HashSet::new();
    let objects: HashSet<u32> = doc.object_numbers().into_iter().collect();
    let mut visits = 0usize;
    let mut order = 0usize;
    while let Some((item, inherited_page, depth)) = tasks.pop() {
        visits = visits.saturating_add(1);
        if visits > MAX_VISITS || depth > MAX_DEPTH {
            break;
        }
        let item = match item {
            Object::Ref(reference)
                if objects.contains(&reference.num) && reference.generation == doc.generation(reference.num) && seen.insert(reference) =>
            {
                (*doc.get(reference)).clone()
            }
            Object::Ref(_) => continue,
            other => other,
        };
        match item {
            Object::Array(items) => {
                let count = items.len().min(MAX_TAGGED_TASKS.saturating_sub(tasks.len()));
                tasks.extend(items.into_iter().take(count).rev().map(|item| (item, inherited_page, depth + 1)));
            }
            Object::Int(mcid) => {
                if let Some(page) = inherited_page.and_then(|reference| page_index.get(&reference).copied()).filter(|page| selected.contains(page)) {
                    let ranks = priorities.entry((page, None)).or_default();
                    if let std::collections::hash_map::Entry::Vacant(entry) = ranks.entry(mcid) {
                        entry.insert(order);
                        order = order.saturating_add(1);
                    }
                }
            }
            Object::Dict(dict) => {
                let page = dict.get(b"Pg").and_then(Object::as_ref).or(inherited_page);
                if dict.name(b"Type") == Some(b"MCR") {
                    if let Some((page, mcid)) = dict
                        .get(b"MCID")
                        .and_then(Object::as_int)
                        .and_then(|mcid| page.and_then(|reference| page_index.get(&reference).copied()).map(|page| (page, mcid)))
                        .filter(|(page, _)| selected.contains(page))
                    {
                        let stream = dict.get(b"Stm").and_then(Object::as_ref);
                        let ranks = priorities.entry((page, stream)).or_default();
                        if let std::collections::hash_map::Entry::Vacant(entry) = ranks.entry(mcid) {
                            entry.insert(order);
                            order = order.saturating_add(1);
                        }
                    }
                } else if dict.name(b"S") != Some(b"Artifact")
                    && let Some(kids) = dict.get(b"K")
                {
                    tasks.push((kids.clone(), page, depth + 1));
                }
            }
            _ => {}
        }
    }
    priorities
}

struct TaggedWalker<'a> {
    doc: &'a Document,
    pages: &'a [Page],
    role_map: Dict,
    page_index: HashMap<ObjRef, usize>,
    selected: HashSet<usize>,
    objects: HashSet<u32>,
    seen: HashSet<ObjRef>,
    texts: HashMap<usize, McidTexts>,
    form_texts: HashMap<(usize, ObjRef), McidTexts>,
    decode_budgets: HashMap<usize, DecodeBudget>,
    mark_priorities: MarkPriorities,
    text_limit: usize,
    used_marks: HashSet<(usize, Option<ObjRef>, i64)>,
    extracted_text: usize,
    visits: usize,
    next_element: usize,
    last_page: Option<usize>,
    last_element: Option<usize>,
    out: TaggedText,
}

impl<'a> TaggedWalker<'a> {
    fn new(
        doc: &'a Document,
        pages: &'a [Page],
        page_index: HashMap<ObjRef, usize>,
        selected: &[usize],
        root: &Dict,
        mark_priorities: MarkPriorities,
        text_limit: usize,
    ) -> Self {
        Self {
            doc,
            pages,
            role_map: root.get(b"RoleMap").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default(),
            page_index,
            selected: selected.iter().copied().collect(),
            objects: doc.object_numbers().into_iter().collect(),
            seen: HashSet::new(),
            texts: HashMap::new(),
            form_texts: HashMap::new(),
            decode_budgets: HashMap::new(),
            mark_priorities,
            text_limit,
            used_marks: HashSet::new(),
            extracted_text: 0,
            visits: 0,
            next_element: 0,
            last_page: None,
            last_element: None,
            out: TaggedText::default(),
        }
    }

    fn visit(
        &mut self,
        item: Object,
        page: Option<ObjRef>,
        depth: usize,
        element: Option<usize>,
        stream: Option<ObjRef>,
        tasks: &mut Vec<TaggedTask>,
    ) {
        self.visits = self.visits.saturating_add(1);
        if self.visits > MAX_VISITS || depth > MAX_DEPTH {
            tagged_warning(&mut self.out, "structure traversal stopped at its safety limit".into());
            return;
        }
        let item = match item {
            Object::Ref(reference) => {
                if !self.objects.contains(&reference.num) || reference.generation != self.doc.generation(reference.num) {
                    tagged_warning(&mut self.out, format!("missing structure reference {}", reference.num));
                    return;
                }
                if !self.seen.insert(reference) {
                    tagged_warning(&mut self.out, format!("cyclic or repeated structure reference {}", reference.num));
                    return;
                }
                (*self.doc.get(reference)).clone()
            }
            other => other,
        };
        match item {
            Object::Array(items) => self.visit_array(items, page, depth, element, stream, tasks),
            Object::Int(mcid) => self.visit_mcid(mcid, page, stream, element),
            Object::Dict(dict) => self.visit_dict(dict, page, depth, element, tasks),
            _ => tagged_warning(&mut self.out, "unsupported structure-tree content was skipped".into()),
        }
    }

    fn visit_array(
        &mut self,
        items: Vec<Object>,
        page: Option<ObjRef>,
        depth: usize,
        element: Option<usize>,
        stream: Option<ObjRef>,
        tasks: &mut Vec<TaggedTask>,
    ) {
        let count = items.len().min(MAX_TAGGED_TASKS.saturating_sub(tasks.len()));
        if count < items.len() {
            tagged_warning(&mut self.out, "structure traversal stopped at its pending-item limit".into());
        }
        tasks.extend(items.into_iter().take(count).rev().map(|item| (item, page, depth + 1, element, stream)));
    }

    fn visit_dict(&mut self, dict: Dict, inherited_page: Option<ObjRef>, depth: usize, element: Option<usize>, tasks: &mut Vec<TaggedTask>) {
        let page = dict.get(b"Pg").and_then(Object::as_ref).or(inherited_page);
        if dict.name(b"Type") == Some(b"MCR") {
            self.visit_mcr(&dict, page, depth, element, tasks);
            return;
        }
        if dict.contains(b"S") {
            if dict.name(b"S") == Some(b"Artifact") {
                return;
            }
            let element = Some(self.next_element);
            self.next_element = self.next_element.saturating_add(1);
            if let Some(replacement) = structure_replacement(self.doc, &self.role_map, &dict) {
                self.visit_replacement(replacement, page, element);
                if let Some(kids) = dict.get(b"K") {
                    let mut state = CoverState {
                        page_index: &self.page_index,
                        selected: &self.selected,
                        objects: &self.objects,
                        used_marks: &mut self.used_marks,
                        warnings: &mut self.out,
                    };
                    cover_struct_content(self.doc, kids, page, &mut state);
                }
                return;
            }
            if dict.get(b"K").is_none() {
                tagged_warning(&mut self.out, "structure element has no content or alternate text".into());
            }
            if let Some(kids) = dict.get(b"K") {
                tasks.push((kids.clone(), page, depth + 1, element, None));
            }
        }
    }

    fn visit_mcr(&mut self, dict: &Dict, page: Option<ObjRef>, depth: usize, element: Option<usize>, tasks: &mut Vec<TaggedTask>) {
        let Some(mcid) = dict.get(b"MCID").and_then(Object::as_int) else {
            tagged_warning(&mut self.out, "marked-content reference has no MCID".into());
            return;
        };
        let stream = dict.get(b"Stm").and_then(Object::as_ref);
        tasks.push((Object::Int(mcid), dict.get(b"Pg").and_then(Object::as_ref).or(page), depth + 1, element, stream));
        if let Some(stream) = stream
            && page.and_then(|reference| self.page_index.get(&reference).copied()).is_some_and(|page| self.selected.contains(&page))
        {
            let priority_page = page.and_then(|reference| self.page_index.get(&reference).copied());
            let priorities = priority_page.and_then(|page| self.mark_priorities.get(&(page, Some(stream))));
            if let Some(page_index) = priority_page {
                let cache_key = (page_index, stream);
                if !self.form_texts.contains_key(&cache_key) && self.texts.get(&page_index).and_then(|texts| find_form_text(texts, stream)).is_none()
                {
                    let remaining = self.text_limit.saturating_sub(self.extracted_text);
                    let budget = self.decode_budgets.entry(page_index).or_insert_with(DecodeBudget::page);
                    let parsed = form_mcid_texts(self.doc, stream, remaining, priorities, budget).unwrap_or_default();
                    self.extracted_text = self.extracted_text.saturating_add(parsed.bytes);
                    self.form_texts.insert(cache_key, parsed);
                }
            }
        }
    }

    fn visit_mcid(&mut self, mcid: i64, page_ref: Option<ObjRef>, stream: Option<ObjRef>, element: Option<usize>) {
        let Some(page) = page_ref.and_then(|reference| self.page_index.get(&reference).copied()) else {
            let message = if page_ref.is_some() {
                format!("marked content {mcid} references a missing page")
            } else {
                format!("marked content {mcid} has no page association")
            };
            tagged_warning(&mut self.out, message);
            return;
        };
        if !self.selected.contains(&page) {
            return;
        }
        let key = (page, stream, mcid);
        if !self.used_marks.insert(key) {
            tagged_warning(&mut self.out, format!("marked content {mcid} on page {} is referenced more than once", page + 1));
            return;
        }
        let remaining = self.text_limit.saturating_sub(self.extracted_text);
        let text = if let Some(stream) = stream {
            let Some(texts) = self.form_texts.get(&(page, stream)).or_else(|| self.texts.get(&page).and_then(|texts| find_form_text(texts, stream)))
            else {
                tagged_warning(&mut self.out, format!("missing marked content {mcid} on page {}", page + 1));
                return;
            };
            if texts.limited {
                tagged_warning(&mut self.out, format!("text extraction reached its size limit on page {}", page + 1));
            }
            texts.by_id.get(&mcid).cloned()
        } else {
            if !self.texts.contains_key(&page) {
                let priorities = self.mark_priorities.get(&(page, None));
                let skipped_forms: HashSet<_> =
                    self.used_marks.iter().filter_map(|(used_page, source, _)| (*used_page == page).then_some(*source).flatten()).collect();
                let budget = self.decode_budgets.entry(page).or_insert_with(DecodeBudget::page);
                let texts = self
                    .pages
                    .get(page)
                    .map(|p| {
                        mcid_texts_limited_skipping_with_budget(self.doc, &p.dict, MAX_TAGGED_TEXT, remaining, priorities, &skipped_forms, budget)
                    })
                    .unwrap_or_default();
                self.extracted_text = self.extracted_text.saturating_add(texts.bytes);
                self.texts.insert(page, texts);
            }
            let Some(texts) = self.texts.get(&page) else { return };
            let found = texts.by_id.get(&mcid).cloned();
            if texts.limited {
                tagged_warning(&mut self.out, format!("text extraction reached its size limit on page {}", page + 1));
            }
            found
        };
        if let Some(text) = text {
            if !append_tagged_text(&mut self.out, &text, page, element, &mut self.last_page, &mut self.last_element) {
                tagged_warning(&mut self.out, "tagged text stopped at its size limit".into());
            }
        } else {
            tagged_warning(&mut self.out, format!("missing marked content {mcid} on page {}", page + 1));
        }
    }

    fn visit_replacement(&mut self, text: String, page_ref: Option<ObjRef>, element: Option<usize>) {
        if let Some(page) = page_ref.and_then(|reference| self.page_index.get(&reference).copied()) {
            if self.selected.contains(&page) && !append_tagged_text(&mut self.out, &text, page, element, &mut self.last_page, &mut self.last_element)
            {
                tagged_warning(&mut self.out, "tagged text stopped at its size limit".into());
            }
        } else {
            let warning = if page_ref.is_some() { "alternate text references a missing page" } else { "alternate text has no page association" };
            tagged_warning(&mut self.out, warning.into());
        }
    }

    fn finish(mut self, pages: &[Page]) -> TaggedText {
        self.warn_unstructured_pages(pages);
        self.out.warnings.sort();
        self.out.warnings.dedup();
        self.out
    }
}

impl TaggedWalker<'_> {
    fn warn_unstructured_pages(&mut self, pages: &[Page]) {
        for page in &self.selected {
            if !self.texts.contains_key(page) {
                let remaining = self.text_limit.saturating_sub(self.extracted_text);
                let page_priorities = self.mark_priorities.get(&(*page, None));
                let skipped_forms: HashSet<_> =
                    self.used_marks.iter().filter_map(|(used_page, source, _)| (*used_page == *page).then_some(*source).flatten()).collect();
                let budget = self.decode_budgets.entry(*page).or_insert_with(DecodeBudget::page);
                let text = pages
                    .get(*page)
                    .map(|p| {
                        mcid_texts_limited_skipping_with_budget(
                            self.doc,
                            &p.dict,
                            MAX_TAGGED_TEXT,
                            remaining,
                            page_priorities,
                            &skipped_forms,
                            budget,
                        )
                    })
                    .unwrap_or_default();
                self.extracted_text = self.extracted_text.saturating_add(text.bytes);
                self.texts.insert(*page, text);
            }
            let Some(page_text) = self.texts.get(page) else { continue };
            if page_text.limited {
                tagged_warning(&mut self.out, format!("text extraction reached its size limit on page {}", page + 1));
            }
            if page_text.unmarked_text
                || page_text.by_id.iter().any(|(mcid, text)| !text.trim().is_empty() && !self.used_marks.contains(&(*page, None, *mcid)))
                || has_unstructured_form_text(*page, page_text, &self.form_texts, &self.used_marks)
            {
                tagged_warning(&mut self.out, format!("page {} contains text outside the structure tree", page + 1));
            }
        }
    }
}

fn append_tagged_text(
    out: &mut TaggedText,
    text: &str,
    page: usize,
    element: Option<usize>,
    last_page: &mut Option<usize>,
    last_element: &mut Option<usize>,
) -> bool {
    if text.is_empty() {
        return true;
    }
    let separator = if last_page.is_some_and(|previous| previous != page) {
        "\u{c}"
    } else if !out.text.is_empty() && !out.text.ends_with('\n') && *last_element != element {
        "\n"
    } else {
        ""
    };
    if out.text.len().saturating_add(separator.len()).saturating_add(text.len()) > MAX_TAGGED_TEXT {
        return false;
    }
    out.text.push_str(separator);
    out.text.push_str(text);
    *last_page = Some(page);
    *last_element = element;
    true
}

fn structure_replacement(doc: &Document, role_map: &Dict, dict: &Dict) -> Option<String> {
    if let Some(actual_text) = dict.get(b"ActualText").map(|value| doc.resolve(value)).and_then(|value| value.as_string().map(|s| s.to_text())) {
        return Some(actual_text);
    }
    if dict.name(b"S").is_some_and(|s| standard(doc, role_map, s) == b"Figure") {
        return dict
            .get(b"Alt")
            .map(|value| doc.resolve(value))
            .and_then(|value| value.as_string().map(|s| s.to_text()))
            .filter(|text| !text.is_empty());
    }
    None
}

pub(crate) fn form_mcid_texts(
    doc: &Document,
    reference: ObjRef,
    max_total: usize,
    priorities: Option<&HashMap<i64, usize>>,
    budget: &mut DecodeBudget,
) -> Option<McidTexts> {
    let object = doc.get(reference);
    let Object::Stream(stream) = &*object else { return None };
    if stream.dict.name(b"Subtype") != Some(b"Form") {
        return None;
    }
    if !budget.enter_form() {
        return Some(McidTexts { limited: true, ..McidTexts::default() });
    }
    let Ok(data) = budget.decode(stream) else { return Some(McidTexts { limited: true, ..McidTexts::default() }) };
    let mut forms = HashSet::from([reference]);
    Some(mcid_texts_within(doc, &stream.dict, Some(&data), None, MAX_TAGGED_TEXT, max_total, priorities, &HashSet::new(), budget, &mut forms, 0))
}

struct CoverState<'a> {
    page_index: &'a HashMap<ObjRef, usize>,
    selected: &'a HashSet<usize>,
    objects: &'a HashSet<u32>,
    used_marks: &'a mut HashSet<(usize, Option<ObjRef>, i64)>,
    warnings: &'a mut TaggedText,
}

fn cover_struct_content(doc: &Document, root: &Object, page: Option<ObjRef>, state: &mut CoverState<'_>) {
    let mut tasks = vec![(root.clone(), page, 0usize)];
    let mut seen = HashSet::new();
    let mut visits = 0usize;
    while let Some((item, inherited_page, depth)) = tasks.pop() {
        visits = visits.saturating_add(1);
        if visits > MAX_VISITS || depth > MAX_DEPTH {
            tagged_warning(state.warnings, "structure replacement traversal stopped at its safety limit".into());
            break;
        }
        let item = match item {
            Object::Ref(reference) => {
                if !state.objects.contains(&reference.num) || reference.generation != doc.generation(reference.num) {
                    tagged_warning(state.warnings, format!("missing structure reference {}", reference.num));
                    continue;
                }
                if !seen.insert(reference) {
                    tagged_warning(state.warnings, format!("cyclic or repeated structure reference {}", reference.num));
                    continue;
                }
                (*doc.get(reference)).clone()
            }
            item => item,
        };
        match item {
            Object::Int(mcid) => {
                if let Some(index) = inherited_page.and_then(|page| state.page_index.get(&page).copied()).filter(|page| state.selected.contains(page))
                {
                    state.used_marks.insert((index, None, mcid));
                }
            }
            Object::Array(items) => {
                let count = items.len().min(MAX_TAGGED_TASKS.saturating_sub(tasks.len()));
                if count < items.len() {
                    tagged_warning(state.warnings, "structure replacement traversal stopped at its pending-item limit".into());
                }
                tasks.extend(items.into_iter().take(count).rev().map(|item| (item, inherited_page, depth + 1)));
            }
            Object::Dict(dict) if dict.name(b"Type") == Some(b"MCR") => {
                if let Some(mcid) = dict.get(b"MCID").and_then(Object::as_int)
                    && let Some(index) = dict
                        .get(b"Pg")
                        .and_then(Object::as_ref)
                        .or(inherited_page)
                        .and_then(|page| state.page_index.get(&page).copied())
                        .filter(|page| state.selected.contains(page))
                {
                    state.used_marks.insert((index, dict.get(b"Stm").and_then(Object::as_ref), mcid));
                }
            }
            Object::Dict(dict) if dict.name(b"S") != Some(b"Artifact") => {
                if let Some(kids) = dict.get(b"K") {
                    tasks.push((kids.clone(), dict.get(b"Pg").and_then(Object::as_ref).or(inherited_page), depth + 1));
                }
            }
            _ => {}
        }
    }
}

fn has_unstructured_form_text(
    page: usize,
    root: &McidTexts,
    direct_forms: &HashMap<(usize, ObjRef), McidTexts>,
    used_marks: &HashSet<(usize, Option<ObjRef>, i64)>,
) -> bool {
    let mut forms: Vec<(ObjRef, &McidTexts)> = root.form_texts.iter().map(|(reference, text)| (*reference, text)).collect();
    forms.extend(direct_forms.iter().filter_map(|((form_page, reference), text)| (*form_page == page).then_some((*reference, text))));
    while let Some((reference, texts)) = forms.pop() {
        if texts.unmarked_text
            || texts.by_id.iter().any(|(mcid, text)| !text.trim().is_empty() && !used_marks.contains(&(page, Some(reference), *mcid)))
        {
            return true;
        }
        forms.extend(texts.form_texts.iter().map(|(reference, text)| (*reference, text)));
    }
    false
}

fn find_form_text(root: &McidTexts, wanted: ObjRef) -> Option<&McidTexts> {
    let mut forms: Vec<(ObjRef, &McidTexts)> = root.form_texts.iter().map(|(reference, text)| (*reference, text)).collect();
    while let Some((reference, texts)) = forms.pop() {
        if reference == wanted {
            return Some(texts);
        }
        forms.extend(texts.form_texts.iter().map(|(reference, text)| (*reference, text)));
    }
    None
}

/// A heading element.
#[derive(Clone, Debug, PartialEq)]
pub struct Heading {
    /// The structure element (`None` for a direct element, which can't be referenced).
    pub obj: Option<ObjRef>,
    /// 1 for H1 … 6 for H6; a plain H takes the number of sections (Sect, Part, Art) around it.
    pub level: u8,
    pub title: String,
    /// 0-based page of its content.
    pub page: usize,
}

struct Walker<'a> {
    doc: &'a Document,
    role_map: Dict,
    pages: &'a [Page],
    page_index: HashMap<ObjRef, usize>,
    seen: HashSet<ObjRef>,
    visits: usize,
    /// Per page: the text each MCID shows (read once per page).
    texts: HashMap<usize, HashMap<i64, String>>,
    out: Vec<Heading>,
}

/// The headings, in document order. Headings without text or a page are left out.
pub fn headings(doc: &Document) -> Vec<Heading> {
    let Some(root) = tree_root(doc) else { return Vec::new() };
    let pages = pdfcraft_model::pages(doc);
    let mut w = Walker {
        doc,
        role_map: root.get(b"RoleMap").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default(),
        pages: &pages,
        page_index: pages.iter().enumerate().map(|(i, p)| (p.obj, i)).collect(),
        seen: HashSet::new(),
        visits: 0,
        texts: HashMap::new(),
        out: Vec::new(),
    };
    if let Some(k) = root.get(b"K") {
        w.walk(k, None, 0, 0);
    }
    w.out
}

impl Walker<'_> {
    /// Whether `k` may be visited (bounded depth and size, each object once).
    fn enter(&mut self, k: &Object, depth: usize) -> bool {
        self.visits += 1;
        depth <= MAX_DEPTH && self.visits <= MAX_VISITS && self.out.len() < MAX_HEADINGS && !matches!(k, Object::Ref(r) if !self.seen.insert(*r))
    }

    fn element_of(&self, k: &Object) -> Option<(Option<ObjRef>, Dict)> {
        let d = match k {
            Object::Ref(r) => self.doc.get(*r).as_dict().cloned()?,
            Object::Dict(d) => d.clone(),
            _ => return None,
        };
        d.contains(b"S").then(|| (k.as_ref(), d))
    }

    fn walk(&mut self, k: &Object, pg: Option<ObjRef>, sections: u8, depth: usize) {
        if !self.enter(k, depth) {
            return;
        }
        if let Object::Array(a) = k {
            a.iter().for_each(|x| self.walk(x, pg, sections, depth + 1));
            return;
        }
        let Some((obj, d)) = self.element_of(k) else { return };
        let pg = d.get(b"Pg").and_then(Object::as_ref).or(pg);
        let ty = d.name(b"S").map(|s| standard(self.doc, &self.role_map, s)).unwrap_or_default();
        let level = match ty.as_slice() {
            [b'H', n @ b'1'..=b'6'] => Some(n - b'0'),
            b"H" => Some(sections.max(1)),
            _ => None,
        };
        if let Some(level) = level {
            self.heading(obj, &d, pg, level, depth);
            return;
        }
        let sections = if matches!(ty.as_slice(), b"Sect" | b"Part" | b"Art") { sections.saturating_add(1) } else { sections };
        if let Some(kids) = d.get(b"K") {
            self.walk(kids, pg, sections, depth + 1);
        }
    }

    /// The marked content under `k`: (page, MCID) pairs in order.
    fn marks(&mut self, k: &Object, pg: Option<ObjRef>, depth: usize, out: &mut Vec<(Option<ObjRef>, i64)>) {
        if !self.enter(k, depth) {
            return;
        }
        match k {
            Object::Int(n) => out.push((pg, *n)),
            Object::Array(a) => a.iter().for_each(|x| self.marks(x, pg, depth + 1, out)),
            Object::Dict(m) if m.name(b"Type") == Some(b"MCR") => {
                if let Some(n) = m.get(b"MCID").and_then(Object::as_int) {
                    out.push((m.get(b"Pg").and_then(Object::as_ref).or(pg), n));
                }
            }
            _ => {
                if let Some((_, d)) = self.element_of(k)
                    && let Some(kids) = d.get(b"K")
                {
                    self.marks(kids, d.get(b"Pg").and_then(Object::as_ref).or(pg), depth + 1, out);
                }
            }
        }
    }

    fn heading(&mut self, obj: Option<ObjRef>, d: &Dict, pg: Option<ObjRef>, level: u8, depth: usize) {
        let mut marks = Vec::new();
        if let Some(kids) = d.get(b"K") {
            self.marks(kids, pg, depth + 1, &mut marks);
        }
        let Some(page) = pg.or_else(|| marks.iter().find_map(|m| m.0)).and_then(|p| self.page_index.get(&p).copied()) else { return };
        let doc = self.doc;
        let attr = |key: &[u8]| d.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_string().map(|s| s.to_text())).filter(|t| !t.trim().is_empty());
        let title = match attr(b"ActualText") {
            Some(t) => Some(t),
            None => self.marked_text(&marks, page).or_else(|| attr(b"Alt")).or_else(|| attr(b"T")),
        };
        let Some(title) = title else { return };
        let title: String = title.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_TITLE).collect();
        self.out.push(Heading { obj, level, title, page });
    }

    /// The text the marked content shows (`page`: where marks without a page of their own are).
    fn marked_text(&mut self, marks: &[(Option<ObjRef>, i64)], page: usize) -> Option<String> {
        let mut parts = Vec::new();
        // The title is cut to MAX_TITLE characters later: stop gathering well before a heading
        // that repeats one mark many times can pile up gigabytes.
        let mut gathered = 0usize;
        for (p, id) in marks {
            if gathered >= MAX_TITLE * 4 {
                break;
            }
            let page = p.and_then(|p| self.page_index.get(&p).copied()).unwrap_or(page);
            if let Some(text) = self.texts_of(page).get(id) {
                gathered = gathered.saturating_add(text.len()).saturating_add(1);
                parts.push(text.clone());
            }
        }
        Some(parts.join(" ")).filter(|t| !t.trim().is_empty())
    }

    fn texts_of(&mut self, page: usize) -> &HashMap<i64, String> {
        let (doc, pages) = (self.doc, self.pages);
        self.texts.entry(page).or_insert_with(|| pages.get(page).map(|p| mcid_texts(doc, &p.dict)).unwrap_or_default())
    }
}

fn sub(doc: &Document, d: &Dict, key: &[u8]) -> Dict {
    d.get(key).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default()
}

/// The text each marked-content id shows on a page (text drawn directly in the page content).
///
/// Word breaks come from where the glyphs are, as on the page: a glyph that starts on another
/// line, more than 0.2 em past the end of the previous glyph of the same id, or back over more
/// than an em from it, starts a new word (the em is the font size times |`Tz`|; see [`new_word`]).
/// Text-positioning operators alone don't break words: producers that place every glyph with its
/// own `Td` (Chromium) would otherwise read as "C h a p t e r".
pub(crate) fn mcid_texts(doc: &Document, page: &Dict) -> HashMap<i64, String> {
    mcid_texts_limited(doc, page, MAX_TITLE * 4, usize::MAX, None).by_id
}

struct MarkedFrame {
    id: Option<i64>,
    artifact: bool,
    suppresses_text: bool,
    local_actual_text: Option<(i64, String)>,
    unmarked_actual_text: Option<String>,
    apply_replacement: bool,
}

pub(crate) fn mcid_texts_limited(
    doc: &Document,
    page: &Dict,
    max_text: usize,
    max_total: usize,
    priorities: Option<&HashMap<i64, usize>>,
) -> McidTexts {
    mcid_texts_limited_skipping(doc, page, max_text, max_total, priorities, &HashSet::new())
}

fn mcid_texts_limited_skipping(
    doc: &Document,
    page: &Dict,
    max_text: usize,
    max_total: usize,
    priorities: Option<&HashMap<i64, usize>>,
    skipped_forms: &HashSet<ObjRef>,
) -> McidTexts {
    let mut budget = DecodeBudget::page();
    mcid_texts_limited_skipping_with_budget(doc, page, max_text, max_total, priorities, skipped_forms, &mut budget)
}

fn mcid_texts_limited_skipping_with_budget(
    doc: &Document,
    page: &Dict,
    max_text: usize,
    max_total: usize,
    priorities: Option<&HashMap<i64, usize>>,
    skipped_forms: &HashSet<ObjRef>,
    budget: &mut DecodeBudget,
) -> McidTexts {
    let mut forms = HashSet::new();
    mcid_texts_within(doc, page, None, None, max_text, max_total, priorities, skipped_forms, budget, &mut forms, 0)
}

#[allow(clippy::cognitive_complexity, clippy::too_many_arguments)]
fn mcid_texts_within(
    doc: &Document,
    page: &Dict,
    content: Option<&[u8]>,
    parent_resources: Option<&Dict>,
    max_text: usize,
    max_total: usize,
    priorities: Option<&HashMap<i64, usize>>,
    skipped_forms: &HashSet<ObjRef>,
    budget: &mut DecodeBudget,
    forms: &mut HashSet<ObjRef>,
    depth: usize,
) -> McidTexts {
    let res = page
        .get(b"Resources")
        .map(|value| doc.resolve(value))
        .and_then(|value| value.as_dict().cloned())
        .or_else(|| parent_resources.cloned())
        .unwrap_or_default();
    let (fonts, props) = (sub(doc, &res, b"Font"), sub(doc, &res, b"Properties"));
    let mut metrics: HashMap<Vec<u8>, Metrics> = HashMap::new();
    // The id each open marked-content sequence is in: its own, else the enclosing one's.
    let mut stack = Vec::<MarkedFrame>::new();
    let mut out = McidTexts::default();
    let mut total_text = 0usize;
    let mut retained = BTreeSet::<(usize, i64)>::new();
    let mut truncated = HashSet::<i64>::new();
    let mut replacements = HashMap::<i64, String>::new();
    // Where each id's last glyph ended, in user space.
    let mut ends: HashMap<i64, (f64, f64)> = HashMap::new();
    let mut gs = TextState::default();
    // With each saved state, the text and text line matrices: inside a text object `q`/`Q`
    // save and restore them too (ISO 32000-2 §9.4.1, as corrected by the PDF Association's
    // errata, issue 368), as the renderer does.
    let mut saved: Vec<(TextState, Matrix, Matrix)> = Vec::new();
    let mut unsaved = 0usize;
    let mut form_cache = HashSet::<ObjRef>::new();
    let (mut tm, mut tlm) = (Matrix::IDENTITY, Matrix::IDENTITY);
    let xobjects = sub(doc, &res, b"XObject");
    let decoded_page_data = content.is_none().then(|| page_content_within(doc, page, budget));
    let page_data = content.or(decoded_page_data.as_deref()).unwrap_or_default();
    for op in pdfcraft_content::parse(page_data).ops {
        crate::count_mark_reads(1);
        let inherited_id = stack.last().and_then(|frame| frame.id);
        let artifact = stack.last().is_some_and(|frame| frame.artifact);
        let suppressed = stack.iter().any(|frame| frame.suppresses_text);
        let id = (!artifact && !suppressed).then_some(inherited_id).flatten();
        match op.op.as_slice() {
            b"Do" => {
                let Some(reference) = op.name(0).and_then(|name| xobjects.get(name)).and_then(Object::as_ref) else { continue };
                if skipped_forms.contains(&reference) {
                    continue;
                }
                let object = doc.get(reference);
                let Object::Stream(stream) = &*object else { continue };
                if stream.dict.name(b"Subtype") != Some(b"Form") {
                    continue;
                }
                if form_cache.contains(&reference) {
                    continue;
                }
                if depth >= MAX_TAGGED_FORM_DEPTH || forms.contains(&reference) || !budget.enter_form() {
                    out.limited = true;
                    continue;
                }
                let Ok(data) = budget.decode(stream) else {
                    out.limited = true;
                    continue;
                };
                forms.insert(reference);
                let remaining = max_total.saturating_sub(total_text);
                let form_resources = stream.dict.get(b"Resources").map(|value| doc.resolve(value)).and_then(|value| value.as_dict().cloned());
                let form_dict = stream.dict.clone();
                let form = mcid_texts_within(
                    doc,
                    &form_dict,
                    Some(&data),
                    form_resources.as_ref().or(Some(&res)),
                    max_text,
                    remaining,
                    None,
                    skipped_forms,
                    budget,
                    forms,
                    depth + 1,
                );
                forms.remove(&reference);
                total_text = total_text.saturating_add(form.bytes);
                out.limited |= form.limited;
                out.unmarked_text |= form.unmarked_text;
                form_cache.insert(reference);
                out.form_texts.insert(reference, form);
                continue;
            }
            b"BMC" => stack.push(MarkedFrame {
                id: inherited_id,
                artifact: artifact || op.name(0) == Some(b"Artifact"),
                suppresses_text: suppressed,
                local_actual_text: None,
                unmarked_actual_text: None,
                apply_replacement: false,
            }),
            b"BDC" => {
                let properties = match op.operands.get(1) {
                    Some(Object::Dict(d)) => Some(d.clone()),
                    Some(Object::Name(n)) => props.get(n).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()),
                    _ => None,
                }
                .unwrap_or_default();
                let own_id = properties.get(b"MCID").and_then(Object::as_int);
                let mcid = own_id.or(inherited_id);
                let opens_artifact = artifact || op.name(0) == Some(b"Artifact");
                let has_outer_replacement = stack.iter().any(|frame| frame.suppresses_text);
                let actual_text =
                    properties.get(b"ActualText").map(|value| doc.resolve(value)).and_then(|value| value.as_string().map(|s| s.to_text()));
                let has_actual_text = !opens_artifact && actual_text.is_some();
                let mut local_actual_text = None;
                let mut unmarked_actual_text = None;
                let mut apply_replacement = false;
                if has_actual_text
                    && !has_outer_replacement
                    && let Some(actual_text) = actual_text
                {
                    match (own_id, mcid) {
                        (Some(_), Some(id)) => {
                            replacements.insert(id, actual_text);
                        }
                        (None, Some(id)) => {
                            local_actual_text = Some((id, actual_text));
                            apply_replacement = true;
                        }
                        (None, None) => {
                            unmarked_actual_text = Some(actual_text);
                            apply_replacement = true;
                        }
                        _ => {}
                    }
                }
                stack.push(MarkedFrame {
                    id: mcid,
                    artifact: opens_artifact,
                    suppresses_text: suppressed || has_actual_text,
                    local_actual_text,
                    unmarked_actual_text,
                    apply_replacement,
                });
            }
            b"EMC" => {
                if let Some(frame) = stack.pop()
                    && frame.apply_replacement
                    && !stack.iter().any(|outer| outer.suppresses_text)
                {
                    if let Some((id, replacement)) = frame.local_actual_text {
                        let rank = priorities.and_then(|items| items.get(&id)).copied().unwrap_or(usize::MAX);
                        let text_len = out.by_id.get(&id).map_or(0, String::len);
                        if text_len.saturating_add(replacement.len()) <= max_text {
                            while total_text.saturating_add(replacement.len()) > max_total
                                && let Some((worst_rank, worst_id)) = retained.iter().next_back().copied()
                                && worst_id != id
                                && worst_rank > rank
                            {
                                if let Some(removed) = out.by_id.remove(&worst_id) {
                                    total_text = total_text.saturating_sub(removed.len());
                                    ends.remove(&worst_id);
                                    retained.remove(&(worst_rank, worst_id));
                                    truncated.insert(worst_id);
                                    out.limited = true;
                                }
                            }
                        }
                        let text = out.by_id.entry(id).or_default();
                        let updated = text.len().saturating_add(replacement.len());
                        if updated <= max_text && total_text.saturating_add(replacement.len()) <= max_total {
                            text.push_str(&replacement);
                            total_text = total_text.saturating_add(replacement.len());
                            retained.insert((rank, id));
                        } else {
                            out.limited = true;
                        }
                    }
                    if frame.unmarked_actual_text.is_some_and(|text| !text.trim().is_empty()) {
                        out.unmarked_text = true;
                    }
                }
            }
            b"q" if saved.len() < MAX_SAVED => saved.push((gs.clone(), tm, tlm)),
            b"q" => unsaved += 1,
            b"Q" if unsaved > 0 => unsaved -= 1,
            b"Q" => {
                if let Some(saved) = saved.pop() {
                    (gs, tm, tlm) = saved;
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    gs.ctm = Matrix(m).then(&gs.ctm);
                }
            }
            b"Tf" => {
                gs.size = op.num(1).unwrap_or(gs.size);
                gs.font = op.name(0).map(<[u8]>::to_vec);
                if let Some(name) = op.name(0)
                    && !metrics.contains_key(name)
                    && let Some(f) = fonts.get(name).map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned())
                {
                    metrics.insert(name.to_vec(), Metrics::from_dict(doc, &f));
                }
            }
            b"Tc" => gs.char_spacing = op.num(0).unwrap_or(0.0),
            b"Tw" => gs.word_spacing = op.num(0).unwrap_or(0.0),
            b"Tz" => gs.scale = op.num(0).unwrap_or(100.0) / 100.0,
            b"TL" => gs.leading = op.num(0).unwrap_or(0.0),
            b"BT" => (tm, tlm) = (Matrix::IDENTITY, Matrix::IDENTITY),
            b"Td" | b"TD" => {
                if let Some([x, y]) = op.nums::<2>() {
                    if op.op == b"TD" {
                        gs.leading = -y;
                    }
                    tlm = Matrix::translate(x, y).then(&tlm);
                    tm = tlm;
                }
            }
            b"Tm" => {
                if let Some(m) = op.nums::<6>() {
                    (tm, tlm) = (Matrix(m), Matrix(m));
                }
            }
            b"T*" => {
                tlm = Matrix::translate(0.0, -gs.leading).then(&tlm);
                tm = tlm;
            }
            _ => {}
        }
        let shows = matches!(op.op.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"");
        let Some(m) = gs.font.as_ref().and_then(|f| metrics.get(f)).filter(|_| shows) else { continue };
        if op.op != b"Tj" && op.op != b"TJ" {
            if op.op == b"\"" {
                gs.word_spacing = op.num(0).unwrap_or(gs.word_spacing);
                gs.char_spacing = op.num(1).unwrap_or(gs.char_spacing);
            }
            tlm = Matrix::translate(0.0, -gs.leading).then(&tlm);
            tm = tlm;
        }
        let pieces: Vec<&Object> = match op.op.as_slice() {
            // A surplus array (`[(A)] [(B)] TJ`) is malformed, but the operator still takes the
            // one immediately before it, as the renderer and every other reader here do (#3569).
            b"TJ" => op.operands.last().and_then(Object::as_array).map(|a| a.iter().collect()).unwrap_or_default(),
            _ => op.operands.last().into_iter().collect(),
        };
        for piece in pieces {
            let s = match piece {
                Object::String(s) => s,
                other => {
                    // A TJ adjustment moves the next glyph; whether it breaks a word is decided there.
                    let dx = -other.as_f64().unwrap_or(0.0) / 1000.0 * gs.size * gs.scale;
                    tm = Matrix::translate(dx, 0.0).then(&tm);
                    continue;
                }
            };
            for (code, len) in m.codes(&s.bytes) {
                let trm = tm.then(&gs.ctm);
                // A glyph's text is only built while its id still takes more.
                if let Some(glyph) = m.text_of(code) {
                    if let Some(id) = id {
                        let text_len = out.by_id.get(&id).map_or(0, String::len);
                        let ends_word = out.by_id.get(&id).is_some_and(|text| !text.is_empty() && !text.ends_with(' '));
                        let breaks = ends.get(&id).is_some_and(|end| new_word(&trm, *end, gs.size, gs.scale));
                        let separator = breaks && ends_word && !glyph.starts_with(' ');
                        let added = glyph.len().saturating_add(usize::from(separator));
                        let rank = priorities.and_then(|items| items.get(&id)).copied().unwrap_or(usize::MAX);
                        while text_len < max_text
                            && !truncated.contains(&id)
                            && total_text.saturating_add(added) > max_total
                            && let Some((worst_rank, worst_id)) = retained.iter().next_back().copied()
                            && worst_id != id
                            && worst_rank > rank
                        {
                            if let Some(removed) = out.by_id.remove(&worst_id) {
                                total_text = total_text.saturating_sub(removed.len());
                                ends.remove(&worst_id);
                                retained.remove(&(worst_rank, worst_id));
                                truncated.insert(worst_id);
                                out.limited = true;
                            }
                        }
                        if text_len < max_text && !truncated.contains(&id) && total_text.saturating_add(added) <= max_total {
                            let text = out.by_id.entry(id).or_default();
                            if separator {
                                text.push(' ');
                            }
                            text.push_str(&glyph);
                            total_text = total_text.saturating_add(added);
                            retained.insert((rank, id));
                        } else {
                            out.limited = true;
                            truncated.insert(id);
                        }
                    } else if !artifact && !glyph.trim().is_empty() {
                        out.unmarked_text = true;
                    }
                }
                let advance = m.width(code) * gs.size + gs.char_spacing + if m.is_space(code, len) { gs.word_spacing } else { 0.0 };
                tm = Matrix::translate(advance * gs.scale, 0.0).then(&tm);
                if let Some(id) = id.filter(|id| out.by_id.contains_key(id) && !truncated.contains(id)) {
                    ends.insert(id, tm.then(&gs.ctm).apply(0.0, 0.0));
                }
            }
        }
    }
    let mut replacements: Vec<(i64, String)> = replacements.into_iter().collect();
    replacements.sort_by_key(|(id, replacement)| {
        (replacement.len() > out.by_id.get(id).map_or(0, String::len), priorities.and_then(|items| items.get(id)).copied().unwrap_or(usize::MAX), *id)
    });
    for (id, replacement) in replacements {
        let previous = out.by_id.get(&id).map_or(0, String::len);
        let rank = priorities.and_then(|items| items.get(&id)).copied().unwrap_or(usize::MAX);
        while replacement.len() <= max_text
            && total_text.saturating_sub(previous).saturating_add(replacement.len()) > max_total
            && let Some((worst_rank, worst_id)) = retained.iter().next_back().copied()
            && worst_id != id
            && worst_rank > rank
        {
            if let Some(removed) = out.by_id.remove(&worst_id) {
                total_text = total_text.saturating_sub(removed.len());
                ends.remove(&worst_id);
                retained.remove(&(worst_rank, worst_id));
                truncated.insert(worst_id);
                out.limited = true;
            }
        }
        let bytes = total_text.saturating_sub(previous).saturating_add(replacement.len());
        if replacement.len() <= max_text && bytes <= max_total {
            total_text = bytes;
            if previous > 0 {
                retained.remove(&(rank, id));
            }
            if replacement.is_empty() {
                out.by_id.insert(id, replacement);
            } else {
                retained.insert((rank, id));
                out.by_id.insert(id, replacement);
            }
        } else {
            out.limited = true;
        }
    }
    out.bytes = total_text;
    out
}

/// How deep `q` nests before the text state stops being saved (a hostile stream can nest millions).
const MAX_SAVED: usize = 256;

/// The graphics state that places glyphs: the CTM, the font and the text state parameters.
#[derive(Clone)]
struct TextState {
    ctm: Matrix,
    font: Option<Vec<u8>>,
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    scale: f64,
    leading: f64,
}

impl Default for TextState {
    fn default() -> Self {
        TextState { ctm: Matrix::IDENTITY, font: None, size: 0.0, char_spacing: 0.0, word_spacing: 0.0, scale: 1.0, leading: 0.0 }
    }
}

/// Does a glyph drawn with text rendering matrix `trm` (text space → user space) start a new word
/// after a glyph that ended at `end` (user space)? Yes on another line (more than half the absolute
/// font size up or down), more than 0.2 horizontal em ahead, or back over more than a horizontal em.
/// The horizontal em is the absolute value of font size times horizontal scale (`Tz`); "ahead" follows
/// the glyph advance, backwards when the size or `Tz` is negative. `Tz` never scales the vertical threshold.
fn new_word(trm: &Matrix, end: (f64, f64), font_size: f64, horizontal_scale: f64) -> bool {
    let advance = font_size * horizontal_scale;
    let em = advance.abs();
    let start = trm.apply(0.0, 0.0);
    let [a, b, c, d, _, _] = trm.0;
    let Some(back) = Matrix([a, b, c, d, 0.0, 0.0]).invert() else { return true };
    let (dx, dy) = back.apply(start.0 - end.0, start.1 - end.1);
    let ahead = if advance < 0.0 { -dx } else { dx };
    !(ahead.is_finite() && dy.is_finite()) || dy.abs() > font_size.abs() * 0.5 || ahead > em * 0.2 || ahead < -em
}
