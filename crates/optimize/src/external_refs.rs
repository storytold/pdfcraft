//! Discard User Data ▸ Discard external cross-references: links and bookmarks that open another
//! document.
//!
//! An action opens another document when its `/S` is `GoToR` (another PDF file), `GoToE` (a
//! document embedded in this one, or the one this is embedded in), `Launch` (a file or an
//! application), `ImportData` (form data read from another file) or `Thread` with an `/F` (an
//! article in another file). Such an action is taken out of the `/A` of a link annotation
//! (`/Subtype /Link`) or a bookmark, and the actions chained after it through `/Next` take its
//! place in order. Other annotations, such as form fields, keep their actions. A link left with no
//! action and no `/Dest` is removed from its page; when the link is stored as its own object, that
//! object keeps the link without its action for anything else that refers to it (the structure
//! tree, for example). A bookmark stays, with its title and children. Links that jump within the
//! document (`/Dest`, `GoTo`) and web links (`URI`) are kept. An `/A` that takes more than
//! [`MAX_STEPS`] values to check, or whose actions open another document and take more than that
//! to rewrite, stops the optimization with an error, and so does an outline of more than
//! [`MAX_OUTLINE`] items or with items stored inside each other more than [`MAX_DEPTH`] levels
//! deep.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::OptimizeError;

/// Action types that open or read another document (`Thread` does when it has an `/F`).
const EXTERNAL: [&[u8]; 4] = [b"GoToR", b"GoToE", b"Launch", b"ImportData"];

/// How many levels (actions, `/Next` arrays and references) a chain is rewritten. Past it, the rest
/// of the chain is kept when it opens no other document, and removed when it does.
const MAX_DEPTH: usize = 512;

/// How many values each pass over one `/A` looks at: the check for actions to other documents,
/// then the rewrite with its check past [`MAX_DEPTH`]. A chain that needs more in either pass
/// stops the optimization with [`OptimizeError::ExternalRefLimit`].
const MAX_STEPS: usize = 100_000;

/// How many bookmarks the outline walk looks at before it stops the optimization with
/// [`OptimizeError::ExternalRefLimit`].
const MAX_OUTLINE: usize = 100_000;

/// What is left of an action or a list of actions.
enum Kept {
    /// Nothing changed: keep the object as it is stored.
    Same,
    /// These actions, in order, replace it.
    Now(Vec<Object>),
}

#[derive(Default)]
struct Walk {
    /// References on the current chain, to end a chain that comes back to itself.
    path: Vec<ObjRef>,
    /// Indirect objects from which an action to another document can be reached.
    dirty: HashSet<ObjRef>,
    /// Actions to another document taken out.
    removed: usize,
    /// A chain that opens another document past [`MAX_DEPTH`] was cut there.
    cut: bool,
    /// Values looked at so far, against [`MAX_STEPS`].
    steps: usize,
    /// The walk ran out of steps; its result is not used.
    exhausted: bool,
}

fn is_external(doc: &Document, d: &Dict) -> bool {
    let Some(s) = d.get(b"S").map(|s| doc.resolve(s)) else { return false };
    match s.as_name() {
        Some(b"Thread") => d.get(b"F").is_some_and(|f| !matches!(doc.resolve(f).as_ref(), Object::Null)),
        Some(n) => EXTERNAL.contains(&n),
        None => false,
    }
}

/// The actions an unchanged `/Next` value stands for, in order.
fn listed(doc: &Document, next: &Object) -> Vec<Object> {
    match doc.resolve(next).as_ref() {
        Object::Array(a) => a.clone(),
        Object::Dict(_) => vec![next.clone()],
        _ => Vec::new(),
    }
}

impl Walk {
    /// Count one value; false once the walk has run out of steps.
    fn step(&mut self) -> bool {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            self.exhausted = true;
        }
        !self.exhausted
    }

    /// Whether the actions stored directly in `o` include one to another document; the references
    /// they hold are added to `refs`.
    fn scan(&mut self, doc: &Document, o: &Object, refs: &mut Vec<ObjRef>) -> bool {
        let mut stack = vec![o];
        let mut external = false;
        while let Some(o) = stack.pop() {
            if !self.step() {
                return false;
            }
            match o {
                Object::Ref(r) => refs.push(*r),
                Object::Array(items) => stack.extend(items),
                Object::Dict(d) => {
                    external |= is_external(doc, d);
                    if let Some(n) = d.get(b"Next") {
                        stack.push(n);
                    }
                }
                _ => {}
            }
        }
        external
    }

    /// Mark the indirect objects reachable from `a` that lead to an action to another document.
    /// Each indirect object is looked at once.
    fn reach(&mut self, doc: &Document, a: &Object) {
        let mut todo = Vec::new();
        self.scan(doc, a, &mut todo);
        let mut seen = HashSet::new();
        let mut parents: HashMap<ObjRef, Vec<ObjRef>> = HashMap::new();
        let mut found = Vec::new();
        while let Some(r) = todo.pop() {
            if self.exhausted {
                return;
            }
            if !seen.insert(r) {
                continue;
            }
            let target = doc.get(r);
            let mut refs = Vec::new();
            if self.scan(doc, &target, &mut refs) {
                found.push(r);
            }
            for c in refs {
                parents.entry(c).or_default().push(r);
                todo.push(c);
            }
        }
        while let Some(r) = found.pop() {
            if self.dirty.insert(r)
                && let Some(p) = parents.get(&r)
            {
                found.extend(p);
            }
        }
    }

    /// Whether `o` or an action chained after it opens another document. Needs [`Walk::reach`]
    /// to have run on an `/A` that holds `o`.
    fn holds_external(&mut self, doc: &Document, o: &Object) -> bool {
        let mut refs = Vec::new();
        let external = self.scan(doc, o, &mut refs);
        external || refs.iter().any(|r| self.dirty.contains(r))
    }
}

impl Walk {
    /// Indirect actions that change are written as new indirect objects (the old ones stay for
    /// whatever else uses them), so a rewritten chain nests no deeper than the original.
    fn strip(&mut self, doc: &mut Document, o: &Object, depth: usize) -> Kept {
        if !self.step() {
            return Kept::Same;
        }
        if depth > MAX_DEPTH {
            if !self.holds_external(doc, o) {
                return Kept::Same;
            }
            self.cut = true;
            return Kept::Now(Vec::new());
        }
        match o {
            Object::Ref(r) => {
                if !self.dirty.contains(r) {
                    return Kept::Same;
                }
                if self.path.contains(r) {
                    return Kept::Now(Vec::new());
                }
                self.path.push(*r);
                let target = doc.get(*r);
                let kept = self.strip(doc, &target, depth + 1);
                self.path.pop();
                match kept {
                    Kept::Now(mut v) if v.len() == 1 && matches!(v.first(), Some(Object::Dict(_))) => {
                        let d = v.remove(0);
                        Kept::Now(vec![Object::Ref(doc.add(d))])
                    }
                    other => other,
                }
            }
            Object::Array(items) => {
                let mut out = Vec::new();
                let mut changed = false;
                for item in items {
                    match self.strip(doc, item, depth + 1) {
                        Kept::Same => out.push(item.clone()),
                        Kept::Now(v) => {
                            changed = true;
                            out.extend(v);
                        }
                    }
                }
                if changed { Kept::Now(out) } else { Kept::Same }
            }
            Object::Dict(d) => {
                let next = d.get(b"Next");
                let rest = next.map(|n| self.strip(doc, n, depth + 1));
                if is_external(doc, d) {
                    self.removed += 1;
                    return Kept::Now(match (rest, next) {
                        (Some(Kept::Now(v)), _) => v,
                        (Some(Kept::Same), Some(n)) => listed(doc, n),
                        _ => Vec::new(),
                    });
                }
                match rest {
                    Some(Kept::Now(v)) => {
                        let mut d = d.clone();
                        set_next(&mut d, v);
                        Kept::Now(vec![Object::Dict(d)])
                    }
                    _ => Kept::Same,
                }
            }
            _ => Kept::Same,
        }
    }
}

fn set_next(d: &mut Dict, mut v: Vec<Object>) {
    match v.len() {
        0 => {
            d.remove(b"Next");
        }
        1 => d.set(b"Next".to_vec(), v.remove(0)),
        _ => d.set(b"Next".to_vec(), Object::Array(v)),
    }
}

/// `/A` of `owner` with its actions to other documents taken out: `None` when nothing changed,
/// `Some(None)` when no action is left, `Some(Some(action))` otherwise (one action, the rest after
/// it).
fn strip_action(doc: &mut Document, a: &Object, owner: ObjRef) -> Result<Option<Option<Object>>, OptimizeError> {
    let mut walk = Walk::default();
    // A quick check first, which looks at each indirect object once: a chain with no action to
    // another document is left alone however its actions are shared, unless it holds more than
    // `MAX_STEPS` values.
    walk.reach(doc, a);
    let external = walk.holds_external(doc, a);
    if walk.exhausted {
        return Err(OptimizeError::ExternalRefLimit(owner));
    }
    if !external {
        return Ok(None);
    }
    walk.steps = 0;
    let kept = walk.strip(doc, a, 0);
    if walk.exhausted {
        return Err(OptimizeError::ExternalRefLimit(owner));
    }
    let Kept::Now(mut v) = kept else { return Ok(None) };
    if walk.removed == 0 && !walk.cut {
        return Ok(None);
    }
    // A null or a reference to something other than an action cannot lead the chain.
    let head = v.iter().position(|o| doc.resolve(o).as_dict().is_some()).unwrap_or(v.len());
    v.drain(..head);
    if v.is_empty() {
        return Ok(Some(None));
    }
    let first = v.remove(0);
    if v.is_empty() {
        return Ok(Some(Some(first)));
    }
    let Some(mut d) = doc.resolve(&first).as_dict().cloned() else { return Ok(Some(None)) };
    let mut next = d.get(b"Next").map(|n| listed(doc, n)).unwrap_or_default();
    next.extend(v);
    set_next(&mut d, next);
    Ok(Some(Some(Object::Dict(d))))
}

/// The outline walk: bookmarks stored as their own objects wait on `stack`; those stored inside
/// another bookmark are edited where they are.
#[derive(Default)]
struct Outline {
    stack: Vec<ObjRef>,
    items: usize,
    changed: usize,
}

impl Outline {
    /// Take actions to other documents out of the bookmark `d` (or, when `item` is false, the
    /// outline dictionary) and of the bookmarks stored inside it. `owner` is the object that holds
    /// `d`. Returns whether `d` changed.
    fn visit(&mut self, doc: &mut Document, d: &mut Dict, owner: ObjRef, depth: usize, item: bool) -> Result<bool, OptimizeError> {
        let mut changed = false;
        if item {
            self.items += 1;
            if self.items > MAX_OUTLINE {
                return Err(OptimizeError::ExternalRefLimit(owner));
            }
            let stripped = match d.get(b"A") {
                Some(a) => strip_action(doc, a, owner)?,
                None => None,
            };
            if let Some(action) = stripped {
                match action {
                    Some(a) => d.set(b"A".to_vec(), a),
                    None => {
                        d.remove(b"A");
                    }
                }
                self.changed += 1;
                changed = true;
            }
        }
        let keys: &[&[u8]] = if item { &[b"Next", b"First"] } else { &[b"First"] };
        for k in keys {
            match d.get_mut(k) {
                Some(Object::Ref(r)) => self.stack.push(*r),
                Some(Object::Dict(c)) => {
                    if depth >= MAX_DEPTH {
                        return Err(OptimizeError::ExternalRefLimit(owner));
                    }
                    changed |= self.visit(doc, c, owner, depth + 1, true)?;
                }
                _ => {}
            }
        }
        Ok(changed)
    }
}

/// Take actions to other documents out of bookmarks, depth-first. Returns the bookmarks changed.
fn discard_bookmarks(doc: &mut Document) -> Result<usize, OptimizeError> {
    let Some(root) = doc.root() else { return Ok(0) };
    let catalog = doc.get(root);
    let mut walk = Outline::default();
    let mut seen = HashSet::new();
    match catalog.as_dict().and_then(|c| c.get(b"Outlines")) {
        Some(Object::Ref(o)) => {
            seen.insert(*o);
            if let Some(mut d) = doc.get(*o).as_dict().cloned()
                && walk.visit(doc, &mut d, *o, 0, false)?
            {
                doc.set(*o, Object::Dict(d));
            }
        }
        Some(Object::Dict(o)) => {
            let mut d = o.clone();
            if walk.visit(doc, &mut d, root, 0, false)? {
                doc.update_dict(root, |c| c.set(b"Outlines".to_vec(), Object::Dict(d)))?;
            }
        }
        _ => {}
    }
    while let Some(r) = walk.stack.pop() {
        if !seen.insert(r) {
            continue;
        }
        let Some(mut d) = doc.get(r).as_dict().cloned() else { continue };
        if walk.visit(doc, &mut d, r, 0, true)? {
            doc.set(r, Object::Dict(d));
        }
    }
    Ok(walk.changed)
}

/// Take actions to other documents out of links and bookmarks. Returns (links, bookmarks)
/// changed; a link counts whether it was removed or kept for its other actions.
pub(crate) fn discard(doc: &mut Document, pages: &[ObjRef]) -> Result<(usize, usize), OptimizeError> {
    let mut links = 0;
    let mut removed_links = HashSet::new();
    for p in pages {
        let Some(pd) = doc.get(*p).as_dict().cloned() else { continue };
        let Some(annots) = pd.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()) else { continue };
        let mut keep = Vec::with_capacity(annots.len());
        let mut changed = false;
        for x in &annots {
            let Some(mut d) = doc.resolve(x).as_dict().cloned() else {
                keep.push(x.clone());
                continue;
            };
            let owner = match x {
                Object::Ref(r) => *r,
                _ => *p,
            };
            let link = d.get(b"Subtype").map(|s| doc.resolve(s)).is_some_and(|s| s.as_name().is_some_and(|n| n == b"Link"));
            let stripped = match d.get(b"A") {
                Some(a) if link => strip_action(doc, a, owner)?,
                _ => None,
            };
            let Some(action) = stripped else {
                if !matches!(x, Object::Ref(r) if removed_links.contains(r)) {
                    keep.push(x.clone());
                } else {
                    changed = true;
                }
                continue;
            };
            links += 1;
            match action {
                Some(a) => d.set(b"A".to_vec(), a),
                None => {
                    d.remove(b"A");
                }
            }
            let removed = d.get(b"A").is_none() && d.get(b"Dest").is_none();
            match x {
                Object::Ref(r) => {
                    doc.set(*r, Object::Dict(d));
                    if removed {
                        removed_links.insert(*r);
                        changed = true;
                    } else {
                        keep.push(x.clone());
                    }
                }
                _ => {
                    changed = true;
                    if !removed {
                        keep.push(Object::Dict(d));
                    }
                }
            }
        }
        if changed {
            doc.update_dict(*p, |d| d.set(b"Annots".to_vec(), Object::Array(keep)))?;
        }
    }
    let bookmarks = discard_bookmarks(doc)?;
    Ok((links, bookmarks))
}
