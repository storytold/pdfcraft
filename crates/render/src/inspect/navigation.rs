#[cfg(test)]
use super::core::inspect;
use super::core::spend;
use super::model::{DestView, OutlineItem, PageInfo};
use super::*;

/// `DocInfo::outline` holds the first this many bookmarks in outline order. Opening a document
/// reads only this window; the rest are listed by page through `pdfcraft_organize`.
pub(super) const OUTLINE_WINDOW: usize = 10_000;

/// Deepest level `DocInfo::outline` nests (the top level is 0); deeper bookmarks are left out.
pub(super) const OUTLINE_DEPTH: usize = 32;

impl<'a> Inspector<'a> {
    /// The destination `/OpenAction` opens at (ISO 32000-2 §7.7.2 Table 29): an explicit
    /// destination array, a `/GoTo` action's `/D`, or (leniently, since Table 29 asks for an array
    /// or an action) a destination name or string; a name or string is looked up as a named
    /// destination (§12.3.2.4). Any other action (a script, a remote go-to) has none here, and
    /// neither has a page number past the end of the document.
    pub(super) fn open_dest(&self, action: &Object) -> Option<(usize, DestView)> {
        let dest = match self.resolve(action) {
            Object::Dictionary(d) => {
                if d.get(b"S").ok().and_then(|o| self.resolve(o).as_name().ok()) != Some(&b"GoTo"[..]) {
                    return None;
                }
                d.get(b"D").ok()?
            }
            other => other,
        };
        self.dest(dest, 0).filter(|(page, _)| *page < self.page_ids.len())
    }

    pub(super) fn page_of(&self, o: &Object) -> Option<usize> {
        match o {
            Object::Reference(id) => self.page_index.get(id).copied(),
            // Some producers write page *numbers* in remote-style destinations.
            Object::Integer(n) => usize::try_from(*n).ok(),
            _ => None,
        }
    }

    // ── destinations ────────────────────────────────────────────────────────────────────────

    /// The page and view of a destination: an explicit array (`[page /XYZ left top zoom]`, …),
    /// a dictionary with `/D`, or a named destination that resolves to one.
    pub(super) fn dest(&self, dest: &Object, depth: u32) -> Option<(usize, DestView)> {
        if depth > 8 {
            return None;
        }
        match self.resolve(dest) {
            Object::Array(a) => {
                let page = a.first().and_then(|p| self.page_of(p))?;
                let kind = a.get(1).map(|k| self.resolve(k)).and_then(|k| k.as_name().ok());
                let operands: Vec<Option<f32>> = a.iter().skip(2).take(4).map(|o| self.resolve(o).as_float().ok()).collect();
                Some((page, DestView::from_operands(kind, &operands)))
            }
            Object::Dictionary(d) => d.get(b"D").ok().and_then(|d| self.dest(d, depth + 1)),
            Object::String(key, _) | Object::Name(key) => self.named_dest(key).and_then(|d| self.dest(d, depth + 1)),
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
    pub(super) fn name_tree_raw(
        &self,
        node: &'a Dictionary,
        seen: &mut HashSet<*const Dictionary>,
        depth: u32,
        out: &mut HashMap<Vec<u8>, &'a Object>,
    ) {
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

    /// The entries of an embedded-files name tree. Each string key is spent from `text_budget`
    /// (see [`MAX_LINK_ATTACHMENT_TEXT_BYTES`]), and an entry whose key does not fit is left out.
    pub(super) fn name_tree(
        &self,
        node: &'a Dictionary,
        seen: &mut HashSet<*const Dictionary>,
        depth: u32,
        text_budget: &mut usize,
    ) -> Vec<(String, &'a Object)> {
        let mut out = Vec::new();
        if depth > 32 || !seen.insert(node as *const _) {
            return out;
        }
        if let Ok(Object::Array(pairs)) = node.get(b"Names").map(|o| self.resolve(o)) {
            for pair in pairs.chunks(2) {
                if let [k, v] = pair {
                    let key = match self.resolve(k) {
                        Object::String(bytes, _) => match spend(bytes, text_budget, text_string) {
                            Some(key) => key,
                            None => continue,
                        },
                        _ => String::new(),
                    };
                    out.push((key, v));
                }
            }
        }
        if let Ok(Object::Array(kids)) = node.get(b"Kids").map(|o| self.resolve(o)) {
            for k in kids {
                if let Some(d) = self.dict(k) {
                    out.extend(self.name_tree(d, seen, depth + 1, text_budget));
                }
            }
        }
        out
    }

    /// The first entry of an embedded-files name tree whose key is `key`, in the order
    /// [`Self::name_tree`] lists them, with no key budget. It keeps no key, and converts a key
    /// object or `/Names` array that several places reference only once (a second visit cannot
    /// match, since the first would have returned).
    pub(super) fn name_tree_find(&self, node: &'a Dictionary, key: &str, seen: &mut NameTreeSeen, depth: u32) -> Option<&'a Object> {
        if depth > 32 || !seen.nodes.insert(node as *const _) {
            return None;
        }
        let names = node.get(b"Names").ok();
        if let Some(names) = names
            && self.resolved_id(names).is_none_or(|id| seen.objects.insert(id))
            && let Object::Array(pairs) = self.resolve(names)
        {
            for pair in pairs.chunks(2) {
                if let [k, v] = pair {
                    if self.resolved_id(k).is_some_and(|id| !seen.objects.insert(id)) {
                        continue;
                    }
                    let found = match self.resolve(k) {
                        Object::String(bytes, _) => text_string(bytes) == key,
                        _ => key.is_empty(),
                    };
                    if found {
                        return Some(v);
                    }
                }
            }
        }
        if let Ok(Object::Array(kids)) = node.get(b"Kids").map(|o| self.resolve(o)) {
            for k in kids {
                if let Some(d) = self.dict(k)
                    && let Some(v) = self.name_tree_find(d, key, seen, depth + 1)
                {
                    return Some(v);
                }
            }
        }
        None
    }

    // ── outline ─────────────────────────────────────────────────────────────────────────────

    /// The window `DocInfo::outline` keeps: the first `OUTLINE_WINDOW` bookmarks in outline order,
    /// nested no deeper than `OUTLINE_DEPTH`, and whether any bookmark was left out. Iterative, and
    /// each bookmark is read once, so the work and memory stay within the window.
    pub(super) fn outline_window(&self, first: &'a Object) -> (Vec<OutlineItem>, bool) {
        let mut seen = HashSet::new();
        // The listed bookmarks in outline order, with their depth.
        let mut listed: Vec<(usize, OutlineItem)> = Vec::new();
        // One entry per open level: its next sibling, or `None` once that run of siblings has ended.
        let mut levels: Vec<Option<&'a Object>> = vec![Some(first)];
        let mut more = false;
        while let Some(depth) = levels.len().checked_sub(1) {
            let Some(slot) = levels.get_mut(depth) else { break };
            let Some(o) = slot.take() else {
                levels.pop();
                continue;
            };
            if let Object::Reference(id) = o
                && !seen.insert(*id)
            {
                continue; // cycle: this run of siblings ends here
            }
            let Some(d) = self.dict(o) else { continue };
            if listed.len() >= OUTLINE_WINDOW {
                more = true;
                break;
            }
            *slot = d.get(b"Next").ok();
            let dest = d
                .get(b"Dest")
                .ok()
                .and_then(|dest| self.dest(dest, 0))
                .or_else(|| d.get(b"A").ok().and_then(|a| self.dict(a)).and_then(|a| a.get(b"D").ok()).and_then(|dest| self.dest(dest, 0)));
            let (page, view) = dest.map_or((None, DestView::Top), |(p, v)| (Some(p), v));
            match d.get(b"First").ok() {
                Some(child) if depth < OUTLINE_DEPTH => levels.push(Some(child)),
                Some(_) => more = true,
                None => {}
            }
            let open = d.get(b"Count").ok().and_then(|c| c.as_i64().ok()).is_some_and(|c| c > 0);
            let title = self.text(d, b"Title").unwrap_or_default();
            let color = match d.get(b"C").map(|o| self.resolve(o)) {
                Ok(Object::Array(c)) => outline_color(c.iter().map(|v| self.resolve(v).as_float().ok())),
                _ => None,
            };
            let flags = d.get(b"F").ok().and_then(|f| self.resolve(f).as_i64().ok()).unwrap_or(0);
            let (italic, bold) = (flags & 1 != 0, flags & 2 != 0);
            listed.push((depth, OutlineItem { title, page, view, children: Vec::new(), open, color, italic, bold }));
        }
        (nest_outline(listed), more)
    }

    // ── page labels (ISO 32000-2 §12.4.2) ───────────────────────────────────────────────────

    fn label_object(&self, object: &'a Object) -> Result<&'a Object, &'static str> {
        self.doc.dereference(object).map(|(_, o)| o).map_err(|_| "a number tree reference is missing or cyclic")
    }

    fn label_dict(&self, object: &'a Object) -> Result<Option<&'a Dictionary>, &'static str> {
        Ok(match self.label_object(object)? {
            Object::Dictionary(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        })
    }

    /// Apply `/PageLabels` to `pages`. `Err`: the tree is unusable and no label is applied.
    /// `Ok(notes)`: labels applied, with what was repaired on the way (a duplicated start page,
    /// a range whose labels are too long to show).
    pub(super) fn page_labels(&self, catalog: &Dictionary, pages: &mut [PageInfo]) -> Result<Vec<String>, &'static str> {
        let mut notes = Vec::new();
        let Ok(tree) = catalog.get(b"PageLabels") else { return Ok(notes) };
        let Some(tree) = self.label_dict(tree)? else { return Ok(notes) };
        let mut ranges: BTreeMap<usize, &Dictionary> = BTreeMap::new();
        let mut stack = vec![(tree, 0usize)];
        let mut seen = HashSet::new();
        let mut left = MAX_LABEL_TREE_WORK;
        while let Some((node, depth)) = stack.pop() {
            label_work(&mut left, 1)?;
            if depth > MAX_LABEL_TREE_DEPTH {
                return Err("the number tree is too deep");
            }
            if !seen.insert(node as *const Dictionary) {
                return Err("the number tree repeats a node or contains a cycle");
            }
            if let Ok(nums) = node.get(b"Nums")
                && let Object::Array(nums) = self.label_object(nums)?
            {
                label_work(&mut left, nums.len().div_ceil(2))?;
                for pair in nums.chunks(2) {
                    if let [k, v] = pair
                        && let (Ok(start), Some(d)) = (self.label_object(k)?.as_i64(), self.label_dict(v)?)
                        && let Ok(start) = usize::try_from(start)
                        && ranges.insert(start, d).is_some()
                    {
                        // The later range wins, as in readers that overwrite.
                        notes.push(format!("a range starts on page {} twice; the later one is used", start.saturating_add(1)));
                    }
                }
            }
            if let Ok(kids) = node.get(b"Kids")
                && let Object::Array(kids) = self.label_object(kids)?
            {
                label_work(&mut left, kids.len())?;
                if kids.len() > MAX_LABEL_TREE_WORK.saturating_sub(stack.len()) {
                    return Err("the number tree has too many pending nodes");
                }
                if !kids.is_empty() && depth == MAX_LABEL_TREE_DEPTH {
                    return Err("the number tree is too deep");
                }
                for kid in kids {
                    if let Some(d) = self.label_dict(kid)? {
                        stack.push((d, depth + 1));
                    }
                }
            }
        }
        // Decode each bounded prefix once, rather than once for every page in its range.
        let mut specs = BTreeMap::new();
        let mut prefix_total = 0usize;
        for (start, d) in ranges {
            let first = d.get(b"St").ok().and_then(|o| self.resolve(o).as_i64().ok()).unwrap_or(1).max(1) as u64;
            let prefix = match d.get(b"P").ok().map(|o| self.resolve(o)) {
                Some(Object::String(raw, _)) => {
                    if raw.len() > MAX_PREFIX_BYTES {
                        return Err("a raw prefix exceeds 2050 bytes");
                    }
                    // A label prefix is literal text, not trimmed document metadata.
                    text_string(raw).trim_matches('\0').to_string()
                }
                Some(Object::Name(raw)) => {
                    if raw.len() > MAX_PREFIX_BYTES {
                        return Err("a raw prefix exceeds 2050 bytes");
                    }
                    // Retain the existing name compatibility and boundary-NUL handling.
                    String::from_utf8_lossy(raw).trim_matches('\0').to_string()
                }
                _ => String::new(),
            };
            if prefix.len() > MAX_LABEL_BYTES {
                return Err("a decoded prefix exceeds 1024 UTF-8 bytes");
            }
            // A prefix object shared by many ranges is decoded once per range: count every copy.
            prefix_total = prefix_total.saturating_add(prefix.len());
            if prefix_total > MAX_PREFIX_TOTAL_BYTES {
                return Err("decoded prefixes exceed 4 MiB in total");
            }
            let style = d.get(b"S").ok().and_then(|o| self.resolve(o).as_name().ok());
            specs.insert(start, (first, prefix, style));
        }
        let mut staged = Vec::new();
        let mut total = 0usize;
        for (i, _) in pages.iter().enumerate() {
            let Some((&start, (first, prefix, style))) = specs.range(..=i).next_back() else { continue };
            let n = if matches!(*style, Some(b"D" | b"R" | b"r" | b"A" | b"a")) {
                let offset = u64::try_from(i - start).map_err(|_| "a page number is too large")?;
                first.checked_add(offset).ok_or("a page number overflows")?
            } else {
                0 // /St is unused when the label consists only of its prefix.
            };
            let remaining = MAX_LABEL_BYTES - prefix.len();
            let number = match *style {
                Some(b"D") => Some(n.to_string()),
                Some(b"R" | b"r") => roman(n, remaining),
                Some(b"A" | b"a") => alpha(n, remaining),
                _ => Some(String::new()),
            }
            .filter(|number| number.len() <= remaining);
            // A label too long to show keeps that page's physical number; other ranges stay.
            let Some(mut number) = number else {
                let note = format!(
                    "labels from page {} exceed {MAX_LABEL_BYTES} UTF-8 bytes; those pages show their physical page numbers",
                    start.saturating_add(1)
                );
                if !notes.contains(&note) {
                    notes.push(note);
                }
                continue;
            };
            if matches!(*style, Some(b"R" | b"A")) {
                number.make_ascii_uppercase();
            }
            let mut label = prefix.clone();
            label.push_str(&number);
            if label.is_empty() {
                continue; // Keep the existing physical number for an empty custom label.
            }
            total = total.checked_add(label.len()).ok_or("the total label size overflows")?;
            if total > MAX_LABEL_TOTAL_BYTES {
                return Err("custom labels exceed 4 MiB in total");
            }
            staged.push((i, label));
        }
        // A malformed later range must not leave a partially applied custom label sequence.
        for (i, label) in staged {
            if let Some(page) = pages.get_mut(i) {
                page.label = label;
            }
        }
        Ok(notes)
    }
}

/// A text string's value, decoded by the same rules as the engine's object model
/// ([`pdfcraft_cos::PdfString::to_text`]) so the panels and outline editing agree.
pub(super) fn text_string(bytes: &[u8]) -> String {
    pdfcraft_cos::PdfString::literal(bytes).to_text()
}

/// What [`Inspector::name_tree_find`] has visited: node dictionaries, and the key and `/Names`
/// array objects it reached by reference.
#[derive(Default)]
pub(super) struct NameTreeSeen {
    nodes: HashSet<*const Dictionary>,
    objects: HashSet<ObjectId>,
}

/// A bookmark title's colour from its `/C` entries, each kept within 0–1 (ISO 32000-2 §12.3.3,
/// Table 151: "An array of three numbers in the range 0.0 to 1.0, representing the components in
/// the DeviceRGB colour space"). Black, the default, and anything that isn't three numbers give
/// `None`.
pub(crate) fn outline_color(values: impl Iterator<Item = Option<f32>>) -> Option<[f32; 3]> {
    let v: Vec<Option<f32>> = values.take(4).collect();
    let [Some(r), Some(g), Some(b)] = v.as_slice() else { return None };
    // A number past the range, however large, is kept at its edge.
    let rgb = [*r, *g, *b].map(|c| c.clamp(0.0, 1.0));
    (rgb != [0.0; 3]).then_some(rgb)
}

/// Nests a pre-order list of `(depth, item)` into the outline tree.
fn nest_outline(listed: Vec<(usize, OutlineItem)>) -> Vec<OutlineItem> {
    let mut roots = Vec::new();
    // The open ancestors, outermost first; each collects its children until it closes.
    let mut open: Vec<(usize, OutlineItem)> = Vec::new();
    for (depth, item) in listed {
        while open.last().is_some_and(|(d, _)| *d >= depth) {
            close_outline(&mut open, &mut roots);
        }
        open.push((depth, item));
    }
    while !open.is_empty() {
        close_outline(&mut open, &mut roots);
    }
    roots
}

/// Closes the innermost open bookmark: it joins its parent's children, or the top level.
fn close_outline(open: &mut Vec<(usize, OutlineItem)>, roots: &mut Vec<OutlineItem>) {
    if let Some((_, item)) = open.pop() {
        match open.last_mut() {
            Some((_, parent)) => parent.children.push(item),
            None => roots.push(item),
        }
    }
}

fn label_work(left: &mut usize, count: usize) -> Result<(), &'static str> {
    *left = left.checked_sub(count).ok_or("the number tree has too many entries")?;
    Ok(())
}

#[cfg(test)]
mod open_dest_tests {
    use std::sync::Arc;

    use super::{DestView, inspect};

    /// Three 612×792 pages (objects 3, 4, 5); `catalog` is added to the catalog dictionary and
    /// `extra` become objects 6, 7, ….
    fn pdf(catalog: &str, extra: &[String]) -> Vec<u8> {
        let mut objs =
            vec![format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>"), "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".to_string()];
        for _ in 0..3 {
            objs.push("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_string());
        }
        objs.extend(extra.iter().cloned());
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    fn open_dest(catalog: &str, extra: &[String]) -> Option<(usize, DestView)> {
        inspect(Arc::new(pdf(catalog, extra)), None).expect("inspects").open_dest
    }

    const XYZ: DestView = DestView::Xyz { left: Some(100.0), top: Some(500.0), zoom: Some(2.0) };

    #[test]
    fn an_explicit_destination_opens_at_its_page_and_position() {
        assert_eq!(open_dest("/OpenAction [5 0 R /XYZ 100 500 2]", &[]), Some((2, XYZ)));
        assert_eq!(open_dest("/OpenAction [4 0 R /FitH 400]", &[]), Some((1, DestView::FitH { top: Some(400.0) })));
        // Indirect, and a zoom of 0 keeps the zoom (Table 149).
        assert_eq!(
            open_dest("/OpenAction 6 0 R", &["[4 0 R /XYZ 0 792 0]".into()]),
            Some((1, DestView::Xyz { left: Some(0.0), top: Some(792.0), zoom: None }))
        );
    }

    #[test]
    fn a_go_to_action_opens_at_its_destination_explicit_or_named() {
        assert_eq!(open_dest("/OpenAction << /S /GoTo /D [5 0 R /XYZ 100 500 2] >>", &[]), Some((2, XYZ)));
        // A name, through the PDF 1.1 catalog /Dests dictionary (§12.3.2.4).
        assert_eq!(open_dest("/OpenAction << /S /GoTo /D /there >> /Dests << /there [5 0 R /XYZ 100 500 2] >>", &[]), Some((2, XYZ)));
        // A byte string, through the /Names /Dests name tree, whose value is a dictionary with /D.
        assert_eq!(
            open_dest(
                "/OpenAction << /S /GoTo /D (there) >> /Names << /Dests 6 0 R >>",
                &["<< /Kids [7 0 R] >>".into(), "<< /Names [(here) [3 0 R /Fit] (there) << /D [5 0 R /XYZ 100 500 2] >>] >>".into()]
            ),
            Some((2, XYZ))
        );
    }

    #[test]
    fn an_open_action_given_as_a_name_or_string_is_looked_up() {
        assert_eq!(open_dest("/OpenAction /there /Dests << /there [4 0 R /Fit] >>", &[]), Some((1, DestView::Fit)));
        assert_eq!(
            open_dest("/OpenAction (there) /Names << /Dests << /Names [(there) [5 0 R /FitV 30]] >> >>", &[]),
            Some((2, DestView::FitV { left: Some(30.0) }))
        );
    }

    #[test]
    fn an_open_action_that_goes_nowhere_here_has_no_destination() {
        for catalog in [
            "",
            "/OpenAction << /S /JavaScript /JS (app.alert(1)) >>",
            // A remote go-to's /D is a page of another file.
            "/OpenAction << /S /GoToR /F (other.pdf) /D [1 /Fit] >>",
            "/OpenAction << /D [4 0 R /Fit] >>",
            // An unknown name, and no name dictionaries at all.
            "/OpenAction << /S /GoTo /D /missing >> /Dests << /there [4 0 R /Fit] >>",
            "/OpenAction /missing",
            // A page number past the end, and a page reference that is no page.
            "/OpenAction [7 /Fit]",
            "/OpenAction [2 0 R /Fit]",
            "/OpenAction 42",
        ] {
            assert_eq!(open_dest(catalog, &[]), None, "{catalog}");
        }
    }

    #[test]
    fn cyclic_and_deep_named_destinations_end() {
        // Names that refer to each other.
        assert_eq!(open_dest("/OpenAction /a /Dests << /a /b /b /a >>", &[]), None);
        assert_eq!(open_dest("/OpenAction /a /Dests << /a << /D /a >> >>", &[]), None);
        // A name tree whose node is its own kid.
        assert_eq!(open_dest("/OpenAction (x) /Names << /Dests 6 0 R >>", &["<< /Kids [6 0 R] >>".into()]), None);
        // A name tree deeper than the depth bound: the leaf past it isn't read.
        let depth = 40;
        let mut nodes: Vec<String> = (0..depth).map(|i| format!("<< /Kids [{} 0 R] >>", 7 + i)).collect();
        nodes.push("<< /Names [(x) [4 0 R /Fit]] >>".into());
        assert_eq!(open_dest("/OpenAction (x) /Names << /Dests 6 0 R >>", &nodes), None);
        // A shallow one is read.
        let shallow = ["<< /Kids [7 0 R] >>".to_string(), "<< /Kids [8 0 R] >>".into(), "<< /Names [(x) [4 0 R /Fit]] >>".into()];
        assert_eq!(open_dest("/OpenAction (x) /Names << /Dests 6 0 R >>", &shallow), Some((1, DestView::Fit)));
    }
}
