//! Which fonts a document draws visibly.
//!
//! PDF/A-2 and -3 require the programs of the fonts *used for rendering* to be embedded (ISO
//! 19005-2 §6.2.11.4.1). Text in rendering mode 3 (neither filled nor stroked: the invisible
//! layer OCR puts over a scan) renders nothing, so a font used only that way needs no program.
//! This walks what the pages draw (their content, the forms in it and the annotations'
//! appearances), following the text rendering mode through the graphics state, and records how
//! each font is used.
//!
//! The walk is the evidence for an exemption, so it errs towards "visible": a font is excused
//! only if every use found is invisible, every content stream was read and parsed cleanly, and
//! no font list the walk doesn't follow (patterns, Type 3 glyphs, form fields' default
//! resources) names it.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object, Stream};

/// Forms nested deeper than this aren't followed (the walk is then incomplete).
const MAX_DEPTH: usize = 12;
/// Form walks over the whole document before the walk gives up as incomplete.
const MAX_FORM_WALKS: usize = 100_000;

/// How the document's fonts are drawn.
#[derive(Debug, Default)]
pub(crate) struct FontUse {
    /// Fonts that are indirect objects: (drawn visibly, drawn invisibly).
    by_ref: HashMap<ObjRef, (bool, bool)>,
    /// Fonts written directly into a resource dictionary (not indirect objects, so the object
    /// scan never sees them) that are drawn visibly without an embedded program: their name and
    /// the 0-based page.
    pub(crate) direct_unembedded: Vec<(Option<String>, Option<usize>)>,
    /// False once some content couldn't be read or a limit was reached.
    complete: bool,
}

impl FontUse {
    /// Whether the font object `font` is only ever drawn invisibly (rendering mode 3).
    pub(crate) fn invisible_only(&self, font: ObjRef) -> bool {
        self.complete && self.by_ref.get(&font) == Some(&(false, true))
    }
}

/// Whether the font dictionary `d` has its program embedded; `None` for a Type 3 font, whose
/// glyphs are content streams and need none.
pub(crate) fn embedded(doc: &Document, d: &Dict) -> Option<bool> {
    let sub = d.name(b"Subtype");
    if sub == Some(b"Type3") {
        return None;
    }
    let descriptor = if sub == Some(b"Type0") {
        d.get(b"DescendantFonts")
            .map(|x| doc.resolve(x))
            .and_then(|a| a.as_array().and_then(|a| a.first().cloned()))
            .map(|f| doc.resolve(&f))
            .and_then(|f| f.as_dict().and_then(|fd| fd.get(b"FontDescriptor").cloned()))
    } else {
        d.get(b"FontDescriptor").cloned()
    };
    Some(
        descriptor
            .map(|fd| doc.resolve(&fd))
            .and_then(|fd| fd.as_dict().cloned())
            .is_some_and(|fd| fd.get(b"FontFile").is_some() || fd.get(b"FontFile2").is_some() || fd.get(b"FontFile3").is_some()),
    )
}

/// Text state that `q`/`Q` save and restore: the current font (as its resource entry) and the
/// rendering mode.
#[derive(Clone)]
struct State {
    font: Option<Object>,
    mode: i64,
}

struct Walk<'a> {
    doc: &'a Document,
    out: FontUse,
    /// Forms being walked, outermost first: drawing one of these again is a cycle.
    path: Vec<ObjRef>,
    form_walks: usize,
    budget: DecodeBudget,
}

/// How `doc`'s pages draw their fonts.
pub(crate) fn font_use(doc: &Document) -> FontUse {
    let mut w = Walk { doc, out: FontUse { complete: true, ..FontUse::default() }, path: Vec::new(), form_walks: 0, budget: DecodeBudget::page() };
    for (i, page) in pdfcraft_model::pages(doc).iter().enumerate() {
        w.budget = DecodeBudget::page();
        let res = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
        if let Some(contents) = page.dict.get(b"Contents") {
            match w.budget.decode_contents(doc, contents).into_iter().collect::<Result<Vec<_>, _>>() {
                Ok(pieces) => {
                    // Content split over streams is one stream (§7.8.2): the state carries over.
                    let mut state = State { font: None, mode: 0 };
                    let mut stack = Vec::new();
                    for data in pieces {
                        w.content(&data, &res, &mut state, &mut stack, Some(i));
                    }
                }
                Err(_) => w.out.complete = false,
            }
        }
        let annots = page.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        for annot in annots {
            let Some(a) = doc.resolve(&annot).as_dict().cloned() else { continue };
            let Some(ap) = a.get(b"AP").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()) else { continue };
            for (_, entry) in ap.iter() {
                // An appearance is a stream, or a dictionary of streams by state.
                let streams: Vec<Object> = match &*doc.resolve(entry) {
                    Object::Dict(states) => states.iter().map(|(_, s)| s.clone()).collect(),
                    _ => vec![entry.clone()],
                };
                for s in streams {
                    w.appearance(&s, Some(i));
                }
            }
        }
    }
    w.unwalked_font_lists();
    w.out
}

impl Walk<'_> {
    /// Walk one content stream with `res`, starting in `state` (a form starts in its caller's
    /// graphics state, §8.10.1).
    fn content(&mut self, data: &[u8], res: &Dict, state: &mut State, stack: &mut Vec<State>, page: Option<usize>) {
        let doc = self.doc;
        let parsed = pdfcraft_content::parse(data);
        // Bytes the parser couldn't read, or an inline image whose end is a guess, leave the
        // operators after them uncertain: one of them may draw a font visibly.
        if parsed.skipped != 0 || parsed.ambiguous_images != 0 {
            self.out.complete = false;
        }
        for op in parsed.ops {
            match op.op.as_slice() {
                b"q" => stack.push(state.clone()),
                b"Q" => {
                    if let Some(s) = stack.pop() {
                        *state = s;
                    }
                }
                b"Tr" => {
                    if let Some(m) = op.operands.last().and_then(Object::as_f64).filter(|m| m.is_finite()) {
                        // Out-of-range modes render nothing sensible either way; anything but
                        // exactly 3 counts as visible.
                        state.mode = if m == 3.0 { 3 } else { 0 };
                    }
                }
                b"Tf" => {
                    // `name size Tf`: the name is the second-to-last operand.
                    let name = op.operands.len().checked_sub(2).and_then(|k| op.operands.get(k)).and_then(Object::as_name);
                    // A font the resources don't name has nothing to embed or excuse.
                    state.font =
                        name.and_then(|n| res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().and_then(|f| f.get(n).cloned())));
                }
                b"Tj" | b"TJ" | b"'" | b"\"" => self.used(state, page),
                b"Do" => {
                    let Some(name) = op.last_name() else { continue };
                    let Some(entry) = res.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().and_then(|x| x.get(name).cloned())) else {
                        continue;
                    };
                    let Some(r) = entry.as_ref() else {
                        // Streams are always indirect; anything else draws nothing.
                        continue;
                    };
                    let obj = doc.get(r);
                    if let Object::Stream(s) = &*obj
                        && s.dict.name(b"Subtype") == Some(b"Form")
                    {
                        let mut inner = state.clone();
                        self.form(r, s, res, &mut inner, page);
                    }
                }
                _ => {}
            }
        }
    }

    /// Record that the current font draws text in the current mode.
    fn used(&mut self, state: &State, page: Option<usize>) {
        let doc = self.doc;
        let Some(font) = &state.font else { return };
        let visible = state.mode != 3;
        match font {
            Object::Ref(r) => {
                let e = self.out.by_ref.entry(*r).or_insert((false, false));
                if visible {
                    e.0 = true;
                } else {
                    e.1 = true;
                }
                // A composite font's descendant is a font object of its own: it is used the
                // same way.
                let kids = doc.get(*r).as_dict().and_then(|d| d.get(b"DescendantFonts").map(|k| (*doc.resolve(k)).clone()));
                if let Some(Object::Array(kids)) = kids {
                    for kid in kids.iter().filter_map(Object::as_ref) {
                        let e = self.out.by_ref.entry(kid).or_insert((false, false));
                        if visible {
                            e.0 = true;
                        } else {
                            e.1 = true;
                        }
                    }
                }
            }
            Object::Dict(d) if visible && embedded(doc, d) == Some(false) => {
                let name = d.name(b"BaseFont").map(|n| String::from_utf8_lossy(n).into_owned());
                if !self.out.direct_unembedded.contains(&(name.clone(), page)) {
                    self.out.direct_unembedded.push((name, page));
                }
            }
            _ => {}
        }
    }

    /// Walk form `r` drawn from content whose resources are `res`.
    fn form(&mut self, r: ObjRef, s: &Stream, res: &Dict, state: &mut State, page: Option<usize>) {
        if self.path.contains(&r) {
            return;
        }
        if self.path.len() >= MAX_DEPTH || self.form_walks >= MAX_FORM_WALKS || !self.budget.enter_form() {
            self.out.complete = false;
            return;
        }
        self.form_walks += 1;
        let Ok(data) = self.budget.decode(s) else {
            self.out.complete = false;
            return;
        };
        let own = s.dict.get(b"Resources").map(|x| self.doc.resolve(x)).and_then(|x| x.as_dict().cloned());
        self.path.push(r);
        let mut stack = Vec::new();
        self.content(&data, own.as_ref().unwrap_or(res), state, &mut stack, page);
        self.path.pop();
    }

    /// Walk an annotation appearance stream (a form drawn in the default graphics state).
    fn appearance(&mut self, entry: &Object, page: Option<usize>) {
        let Some(r) = entry.as_ref() else { return };
        let obj = self.doc.get(r);
        let Object::Stream(s) = &*obj else { return };
        let mut state = State { font: None, mode: 0 };
        self.form(r, s, &Dict::new(), &mut state, page);
    }

    /// Fonts named in font lists this walk doesn't follow count as drawn visibly: tiling
    /// patterns and Type 3 glyphs draw with their own resources, and form fields' default
    /// resources are what a viewer draws field text with.
    fn unwalked_font_lists(&mut self) {
        let doc = self.doc;
        let mut lists: Vec<Dict> = Vec::new();
        let mut seen = HashSet::new();
        for n in doc.object_numbers() {
            let r = ObjRef::new(n, doc.generation(n));
            let obj = doc.get(r);
            let d = match &*obj {
                Object::Stream(s) => &s.dict,
                Object::Dict(d) => d,
                _ => continue,
            };
            let unwalked = d.int(b"PatternType") == Some(1) || d.name(b"Subtype") == Some(b"Type3");
            let res = if unwalked { d.get(b"Resources") } else { d.get(b"DR") };
            if let Some(res) = res.map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned())
                && let Some(fonts) = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned())
            {
                lists.push(fonts);
            }
            // The interactive form's default resources sit in the catalog's /AcroForm, which is
            // often a direct dictionary.
            if let Some(af) = d.get(b"AcroForm").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned())
                && let Some(dr) = af.get(b"DR").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned())
                && let Some(fonts) = dr.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned())
            {
                lists.push(fonts);
            }
        }
        for fonts in lists {
            for (_, f) in fonts.iter() {
                if let Some(r) = f.as_ref()
                    && seen.insert(r)
                    && let Some(e) = self.out.by_ref.get_mut(&r)
                {
                    e.0 = true;
                }
            }
        }
    }
}
