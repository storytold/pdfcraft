//! Discard Objects: form submission, import and reset actions (`/SubmitForm`, `/ImportData`,
//! `/ResetForm`). The fields, buttons, links, bookmarks and pages that ran them stay; a removed
//! action's `/Next` actions take its place, so the rest of a chain still runs in order.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::OptimizeError;

const FORM_ACTIONS: [&[u8]; 3] = [b"SubmitForm", b"ImportData", b"ResetForm"];

/// Keys whose value is one action (or, for `/OpenAction`, an action or a destination).
const ACTION_KEYS: [&[u8]; 2] = [b"A", b"OpenAction"];

/// How deep direct objects and action chains are followed.
const MAX_DEPTH: usize = 64;

/// How many action values one object may make the walk look at.
const MAX_STEPS: usize = 100_000;

/// Remove every form submission, import and reset action. Returns how many the document held in
/// action entries, each counted once: an indirect action however many entries refer to it, and a
/// direct one in the object that holds it. Fails with [`OptimizeError::ActionLimit`] when an
/// object holds a form action more than [`MAX_DEPTH`] direct objects deep, chains form actions
/// more than [`MAX_DEPTH`] deep, or makes the walk look at more than [`MAX_STEPS`] action values.
pub(crate) fn discard(doc: &mut Document) -> Result<usize, OptimizeError> {
    let mut count = Count::default();
    for num in doc.object_numbers() {
        let r = ObjRef { num, generation: doc.generation(num) };
        let mut obj = match &*doc.get(r) {
            o @ (Object::Dict(_) | Object::Array(_)) => o.clone(),
            _ => continue,
        };
        count.object(doc, &obj, 0);
        let mut walk = Walk { changed: false, steps: 0, exhausted: false, chain: HashSet::new() };
        walk.clean(doc, &mut obj, 0);
        if walk.exhausted {
            return Err(OptimizeError::ActionLimit(r));
        }
        if walk.changed {
            doc.set(r, obj);
        }
    }
    Ok(count.direct + count.indirect.len())
}

/// Whether `d` is a form action; its `/S` may be stored as an indirect name.
fn is_form_action(doc: &Document, d: &Dict) -> bool {
    d.get(b"S").is_some_and(|s| doc.resolve(s).as_name().is_some_and(|s| FORM_ACTIONS.contains(&s)))
}

/// The form actions in action entries, counted before they are removed. Each object's pass looks
/// only at its own direct objects, so a direct action is counted once, in the object that holds
/// it; references are counted by identity.
#[derive(Default)]
struct Count {
    direct: usize,
    indirect: HashSet<ObjRef>,
    /// Indirect `/AA` dictionaries and `/Next` arrays already counted.
    shared: HashSet<ObjRef>,
}

impl Count {
    fn object(&mut self, doc: &Document, o: &Object, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        match o {
            Object::Dict(d) => {
                for key in [b"A".as_slice(), b"OpenAction", b"Next"] {
                    if let Some(v) = d.get(key) {
                        self.entry(doc, v, depth);
                    }
                }
                match d.get(b"AA") {
                    Some(Object::Ref(r)) if self.shared.insert(*r) => {
                        if let Object::Dict(t) = &*doc.get(*r) {
                            for (_, v) in t.iter() {
                                self.entry(doc, v, depth);
                            }
                        }
                    }
                    Some(Object::Dict(t)) => {
                        for (_, v) in t.iter() {
                            self.entry(doc, v, depth);
                        }
                    }
                    _ => {}
                }
                for (_, v) in d.iter() {
                    self.object(doc, v, depth + 1);
                }
            }
            Object::Array(items) => {
                for item in items {
                    self.object(doc, item, depth + 1);
                }
            }
            _ => {}
        }
    }

    /// One action entry's value: an action, or (in `/Next`) a list of them.
    fn entry(&mut self, doc: &Document, v: &Object, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        match v {
            Object::Dict(a) if is_form_action(doc, a) => self.direct += 1,
            Object::Array(items) => {
                for item in items {
                    self.entry(doc, item, depth + 1);
                }
            }
            Object::Ref(r) => match &*doc.get(*r) {
                Object::Dict(a) if is_form_action(doc, a) => {
                    self.indirect.insert(*r);
                }
                // An indirect `/Next` array: its direct actions are counted from the first entry
                // that refers to it, and their own entries by the array's own pass.
                Object::Array(items) if self.shared.insert(*r) => {
                    for item in items {
                        match item {
                            Object::Dict(a) if is_form_action(doc, a) => self.direct += 1,
                            Object::Ref(_) => self.entry(doc, item, depth + 1),
                            _ => {}
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

struct Walk {
    /// Whether the object changed.
    changed: bool,
    steps: usize,
    /// Whether a limit stopped the walk before it looked at everything.
    exhausted: bool,
    /// Indirect form actions on the path of the chain being followed.
    chain: HashSet<ObjRef>,
}

impl Walk {
    /// Take form actions out of the action entries of `o` and of the direct objects inside it.
    fn clean(&mut self, doc: &Document, o: &mut Object, depth: usize) {
        if depth > MAX_DEPTH {
            if holds_form_action(doc, o) {
                self.exhausted = true;
            }
            return;
        }
        match o {
            Object::Dict(d) => self.clean_dict(doc, d, depth),
            Object::Array(items) => {
                for item in items {
                    self.clean(doc, item, depth + 1);
                }
            }
            _ => {}
        }
    }

    fn clean_dict(&mut self, doc: &Document, d: &mut Dict, depth: usize) {
        for key in ACTION_KEYS {
            if let Some(v) = d.get(key)
                && let Some(rest) = self.take(doc, v)
            {
                match chain(doc, rest) {
                    Some(a) => d.set(key.to_vec(), a),
                    None => {
                        d.remove(key);
                    }
                }
            }
        }
        if let Some(v) = d.get(b"Next")
            && let Some(mut rest) = self.take(doc, v)
        {
            match rest.len() {
                0 => {
                    d.remove(b"Next");
                }
                1 => d.set(b"Next".to_vec(), rest.remove(0)),
                _ => d.set(b"Next".to_vec(), Object::Array(rest)),
            }
        }
        // Additional actions: each trigger holds one action. An indirect dictionary is copied in
        // when it changes, so other objects that share it keep theirs.
        if let Some(aa) = d.get(b"AA")
            && let Object::Dict(t) = &*doc.resolve(aa)
        {
            let mut triggers = t.clone();
            let mut changed = false;
            let keys: Vec<Vec<u8>> = triggers.iter().map(|(k, _)| k.to_vec()).collect();
            for key in keys {
                if let Some(v) = triggers.get(&key)
                    && let Some(rest) = self.take(doc, v)
                {
                    changed = true;
                    match chain(doc, rest) {
                        Some(a) => triggers.set(key, a),
                        None => {
                            triggers.remove(&key);
                        }
                    }
                }
            }
            if changed {
                if triggers.is_empty() {
                    d.remove(b"AA");
                } else {
                    d.set(b"AA".to_vec(), Object::Dict(triggers));
                }
            }
        }
        let keys: Vec<Vec<u8>> = d.iter().map(|(k, _)| k.to_vec()).collect();
        for key in keys {
            if let Some(v) = d.get_mut(&key) {
                self.clean(doc, v, depth + 1);
            }
        }
    }

    /// [`Walk::without`] for one action entry; marks the object changed when it changes.
    fn take(&mut self, doc: &Document, v: &Object) -> Option<Vec<Object>> {
        self.chain.clear();
        let rest = self.without(doc, v, 0)?;
        self.changed = true;
        Some(rest)
    }

    /// The actions an action value stands for once the form actions at the head of its chain are
    /// taken out: a form action gives way to the actions of its `/Next`. `None` when nothing
    /// changes (the value is kept as it is, references included).
    fn without(&mut self, doc: &Document, v: &Object, depth: usize) -> Option<Vec<Object>> {
        self.steps += 1;
        if depth > MAX_DEPTH || self.steps > MAX_STEPS {
            self.exhausted = true;
            return None;
        }
        let resolved = doc.resolve(v);
        match &*resolved {
            Object::Dict(a) if is_form_action(doc, a) => {
                let r = v.as_ref();
                // A chain that comes back to an action it is still inside ends there.
                if let Some(r) = r
                    && !self.chain.insert(r)
                {
                    return Some(Vec::new());
                }
                let rest = match a.get(b"Next") {
                    None => Vec::new(),
                    Some(next) => match self.without(doc, next, depth + 1) {
                        Some(rest) => rest,
                        None => match &*doc.resolve(next) {
                            Object::Array(items) => items.clone(),
                            _ => vec![next.clone()],
                        },
                    },
                };
                if let Some(r) = r {
                    self.chain.remove(&r);
                }
                Some(rest)
            }
            Object::Array(items) => {
                let mut out = Vec::new();
                let mut changed = false;
                for item in items {
                    match self.without(doc, item, depth + 1) {
                        Some(rest) => {
                            changed = true;
                            out.extend(rest);
                        }
                        None => out.push(item.clone()),
                    }
                }
                changed.then_some(out)
            }
            _ => None,
        }
    }
}

/// Whether the walk would take a form action out of `o` or the direct objects inside it: a form
/// action, or an action entry (or a `/Next` array or `/AA` dictionary, direct or indirect) that
/// refers to one. Looks through `o` without recursion.
fn holds_form_action(doc: &Document, o: &Object) -> bool {
    let mut stack = vec![o];
    while let Some(o) = stack.pop() {
        match o {
            Object::Dict(d) => {
                if is_form_action(doc, d) {
                    return true;
                }
                for key in [b"A".as_slice(), b"OpenAction", b"Next"] {
                    let refers = match d.get(key) {
                        Some(Object::Array(items)) => items.iter().any(|item| refers_to_form_action(doc, item)),
                        Some(v) => refers_to_form_action(doc, v),
                        None => false,
                    };
                    if refers {
                        return true;
                    }
                }
                if let Some(aa) = d.get(b"AA")
                    && let Object::Dict(t) = &*doc.resolve(aa)
                    && t.iter().any(|(_, v)| is_or_refers_to_form_action(doc, v))
                {
                    return true;
                }
                stack.extend(d.iter().map(|(_, v)| v));
            }
            Object::Array(items) => stack.extend(items.iter()),
            _ => {}
        }
    }
    false
}

/// Whether `v` is a direct form action or a reference to one.
fn is_or_refers_to_form_action(doc: &Document, v: &Object) -> bool {
    match v {
        Object::Dict(a) => is_form_action(doc, a),
        Object::Ref(r) => matches!(&*doc.get(*r), Object::Dict(a) if is_form_action(doc, a)),
        _ => false,
    }
}

/// Whether `v` refers to a form action, or to an indirect array that lists one.
fn refers_to_form_action(doc: &Document, v: &Object) -> bool {
    let Object::Ref(r) = v else { return false };
    match &*doc.get(*r) {
        Object::Dict(a) => is_form_action(doc, a),
        Object::Array(items) => items.iter().any(|item| is_or_refers_to_form_action(doc, item)),
        _ => false,
    }
}

/// One action value for a list of actions run in order: the first action, with the others
/// after its own `/Next` actions. `None` when no action remains.
fn chain(doc: &Document, actions: Vec<Object>) -> Option<Object> {
    let mut actions = actions.into_iter();
    loop {
        let first = actions.next()?;
        let rest: Vec<Object> = actions.as_slice().to_vec();
        if rest.is_empty() {
            return Some(first);
        }
        // A list needs a dictionary at its head to carry the rest; anything else is dropped.
        let Some(mut head) = doc.resolve(&first).as_dict().cloned() else { continue };
        let mut next = match head.get(b"Next") {
            None => Vec::new(),
            Some(n) => match &*doc.resolve(n) {
                Object::Array(items) => items.clone(),
                _ => vec![n.clone()],
            },
        };
        next.extend(rest);
        head.set(b"Next".to_vec(), Object::Array(next));
        return Some(Object::Dict(head));
    }
}
