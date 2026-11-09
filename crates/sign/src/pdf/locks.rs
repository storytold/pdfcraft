//! Fields a signature locks: its field's `/Lock` (ISO 32000-2 §12.7.5.5, Table 236) and the
//! FieldMDP transform in its value's `/Reference` (§12.8.2.4). Signers write either or both. Once
//! the field is signed, a change to a field the lock names breaks the signature, whatever the
//! change would count as otherwise (#6757).
//!
//! The locks are read from the signed revision, and the locked fields are compared between that
//! revision and the document as it is now: each field's dictionary and the ones above it (an
//! inherited `/V` is its value too), and its widgets. A field that is gone, or a second field
//! under its name, is a change as well.

use std::collections::BTreeMap;

use super::listing::{Field, MAX_ENTRIES, MAX_FIELD_NAME_BYTES, MAX_FIELDS, MAX_NAME_BYTES, text};
use super::{Dict, Document, HashSet, ObjRef, Object};

/// What a lock covers (`/Action`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    All,
    Include,
    Exclude,
}

/// One lock: its action and the fully qualified names it lists.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Rule {
    action: Action,
    names: Vec<String>,
}

impl Rule {
    /// Whether the rule locks field `name`. A listed name also covers the fields below it
    /// (`address` covers `address.city`).
    fn locks(&self, name: &str) -> bool {
        let listed = || self.names.iter().any(|x| name == x || name.strip_prefix(x.as_str()).is_some_and(|r| r.starts_with('.')));
        match self.action {
            Action::All => true,
            Action::Include => listed(),
            Action::Exclude => !listed(),
        }
    }
}

/// Most names one lock lists; real locks name a handful.
const MAX_LOCK_NAMES: usize = MAX_FIELDS;
/// Deepest field tree walked. Real forms nest a few levels.
const MAX_FIELD_DEPTH: usize = 64;
/// Most objects one comparison visits, for all the locked fields of one signature together.
const MAX_COMPARED: usize = 1 << 18;

/// What comparing a signature's locked fields found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Locked {
    /// The signature locks no field, or none it locks changed.
    Kept,
    /// A field it locks changed.
    Changed,
    /// It locks fields, but a cap or a lock that can't be read kept them from being compared.
    Unchecked,
}

/// A lock dictionary (`/Lock`) or FieldMDP transform parameters. `Err` when it is there but can't
/// be read: its action is not one of the three, or it lists more names than are read.
fn rule(doc: &Document, d: &Dict) -> Result<Rule, ()> {
    let action = match d.name(b"Action") {
        Some(b"All") => Action::All,
        Some(b"Include") => Action::Include,
        Some(b"Exclude") => Action::Exclude,
        _ => return Err(()),
    };
    let listed = d.get(b"Fields").map(|f| doc.resolve(f));
    let listed = listed.as_deref().and_then(Object::as_array).map(Vec::as_slice).unwrap_or_default();
    if listed.len() > MAX_LOCK_NAMES {
        return Err(());
    }
    let names = listed
        .iter()
        .filter_map(|n| match &*doc.resolve(n) {
            Object::String(s) => Some(s.to_text()),
            Object::Name(n) => Some(String::from_utf8_lossy(n).into_owned()),
            _ => None,
        })
        .map(|s| s.trim_matches('\0').trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    Ok(Rule { action, names })
}

/// The locks of signature field `field`, whose value is `value`, both as the signed revision `old`
/// has them. `Err` when one is there but can't be read.
fn rules(old: &Document, field: &Field, value: &Dict) -> Result<Vec<Rule>, ()> {
    let mut out = Vec::new();
    if let Some(lock) = field.dict.get(b"Lock").map(|l| old.resolve(l)) {
        match &*lock {
            Object::Dict(d) => out.push(rule(old, d)?),
            Object::Null => {}
            _ => return Err(()),
        }
    }
    let refs = value.get(b"Reference").map(|r| old.resolve(r));
    for r in refs.as_deref().and_then(Object::as_array).into_iter().flatten() {
        let r = old.resolve(r);
        let Some(rd) = r.as_dict() else { continue };
        if rd.name(b"TransformMethod") != Some(b"FieldMDP") {
            continue;
        }
        let params = rd.get(b"TransformParams").map(|p| old.resolve(p));
        match params.as_deref().and_then(Object::as_dict) {
            Some(p) => out.push(rule(old, p)?),
            // A FieldMDP reference that does not say what it locks is a lock that can't be read.
            None => return Err(()),
        }
    }
    Ok(out)
}

/// Whether any field the signature in `field` (signed with `value`) locks in the signed revision
/// `old` is different in `doc`, the document as it is now.
pub(super) fn locked_fields(doc: &Document, old: &Document, field: &Field, value: &Dict) -> Locked {
    let Ok(rules) = rules(old, field, value) else { return Locked::Unchecked };
    if rules.is_empty() {
        return Locked::Kept;
    }
    let Some(before) = form_fields(old) else { return Locked::Unchecked };
    let locked: Vec<&String> = before.fields.keys().filter(|name| **name != field.name && rules.iter().any(|r| r.locks(name))).collect();
    if locked.is_empty() {
        return Locked::Kept;
    }
    let Some(now) = form_fields(doc) else { return Locked::Unchecked };
    let mut same = Same { old, doc, left: MAX_COMPARED, assumed: HashSet::new() };
    for name in locked {
        let (Some(was), Some(is)) = (before.fields.get(name), now.fields.get(name)) else { return Locked::Changed };
        if was.len() != is.len() {
            return Locked::Changed;
        }
        for (w, i) in was.iter().zip(is) {
            match same.field(&before, w, &now, i) {
                Some(true) => {}
                Some(false) => return Locked::Changed,
                None => return Locked::Unchecked,
            }
        }
    }
    Locked::Kept
}

/// One field dictionary in a walk of the form, and the one above it.
struct Node {
    dict: Dict,
    parent: Option<usize>,
}

/// One field under its fully qualified name: its node, and its widgets apart from it.
struct Entry {
    node: usize,
    widgets: Vec<Dict>,
}

/// Every field of a form by fully qualified name, terminal or not, in walk order.
struct Form {
    nodes: Vec<Node>,
    fields: BTreeMap<String, Vec<Entry>>,
}

/// The fields of `doc`'s form, or `None` when a cap stopped the walk (a field it did not read may
/// be one a lock names). The caps are the signature listing's.
fn form_fields(doc: &Document) -> Option<Form> {
    let mut form = Form { nodes: Vec::new(), fields: BTreeMap::new() };
    let Some(root) = doc.root() else { return Some(form) };
    let Some(af) = doc.get(root).as_dict().and_then(|c| c.get(b"AcroForm")).map(|a| doc.resolve(a)) else { return Some(form) };
    let af = match &*af {
        Object::Dict(d) => d.clone(),
        Object::Null => return Some(form),
        _ => return None,
    };
    let top = af.get(b"Fields").map(|f| doc.resolve(f));
    let top: Vec<Object> = match top.as_deref() {
        None | Some(Object::Null) => return Some(form),
        Some(Object::Array(a)) => a.clone(),
        Some(_) => return None,
    };
    let (mut fields_left, mut entries_left, mut names_left) = (MAX_FIELDS, MAX_ENTRIES, MAX_NAME_BYTES);
    let mut seen: HashSet<ObjRef> = HashSet::new();
    let mut stack: Vec<(Object, String, Option<usize>, usize)> = Vec::new();
    if top.len() > MAX_ENTRIES {
        return None;
    }
    entries_left -= top.len();
    for f in top.into_iter().rev() {
        stack.push((f, String::new(), None, 0));
    }
    while let Some((o, prefix, parent, depth)) = stack.pop() {
        if depth > MAX_FIELD_DEPTH {
            return None;
        }
        if let Some(r) = o.as_ref()
            && !seen.insert(r)
        {
            continue;
        }
        let Some(d) = doc.resolve(&o).as_dict().cloned() else { continue };
        fields_left = fields_left.checked_sub(1)?;
        let name = match (text(doc, &d, b"T"), prefix.is_empty()) {
            (Some(p), true) => p,
            (Some(p), false) => format!("{prefix}.{p}"),
            (None, _) => prefix,
        };
        if name.len() > MAX_FIELD_NAME_BYTES {
            return None;
        }
        names_left = names_left.checked_sub(name.len())?;
        let node = form.nodes.len();
        let kids = d.get(b"Kids").map(|k| doc.resolve(k));
        let kids: Vec<Object> = kids.as_deref().and_then(Object::as_array).cloned().unwrap_or_default();
        entries_left = entries_left.checked_sub(kids.len())?;
        form.nodes.push(Node { dict: d, parent });
        let mut widgets = Vec::new();
        let mut sub = Vec::new();
        for k in kids {
            let kd = doc.resolve(&k);
            let Some(kd) = kd.as_dict() else { continue };
            // Kids with a /T are fields, and so are those with kids of their own; the others are
            // this field's widgets.
            if kd.contains(b"T") || kd.contains(b"Kids") {
                sub.push(k);
            } else {
                widgets.push(kd.clone());
            }
        }
        for k in sub.into_iter().rev() {
            stack.push((k, name.clone(), Some(node), depth + 1));
        }
        form.fields.entry(name).or_default().push(Entry { node, widgets });
    }
    Some(form)
}

/// Entries a field's own dictionary is compared without: the tree's links, which change when a
/// field is added beside it or a widget's page gains an annotation, and say nothing of the field.
const FIELD_LINKS: &[&[u8]] = &[b"Kids", b"Parent", b"P"];

/// Compares objects of the signed revision with those of the document now, resolving references
/// in each, within [`MAX_COMPARED`] objects.
struct Same<'a> {
    old: &'a Document,
    doc: &'a Document,
    left: usize,
    /// Pairs of references being compared or found the same, so a cycle ends.
    assumed: HashSet<(ObjRef, ObjRef)>,
}

impl Same<'_> {
    /// Whether field `was` of the signed revision's form `before` is unchanged as `is` in `now`:
    /// its dictionary and each above it, and its widgets. `None` when the comparison ran past its cap.
    fn field(&mut self, before: &Form, was: &Entry, now: &Form, is: &Entry) -> Option<bool> {
        let (mut a, mut b) = (Some(was.node), Some(is.node));
        loop {
            match (a.and_then(|i| before.nodes.get(i)), b.and_then(|i| now.nodes.get(i))) {
                (None, None) => break,
                (Some(x), Some(y)) => {
                    if !self.dicts(&x.dict, &y.dict, FIELD_LINKS, 0)? {
                        return Some(false);
                    }
                    (a, b) = (x.parent, y.parent);
                }
                _ => return Some(false),
            }
        }
        if was.widgets.len() != is.widgets.len() {
            return Some(false);
        }
        for (x, y) in was.widgets.iter().zip(&is.widgets) {
            if !self.dicts(x, y, FIELD_LINKS, 0)? {
                return Some(false);
            }
        }
        Some(true)
    }

    fn dicts(&mut self, a: &Dict, b: &Dict, skip: &[&[u8]], depth: usize) -> Option<bool> {
        fn keys<'d>(d: &'d Dict, skip: &[&[u8]]) -> Vec<&'d [u8]> {
            let mut k: Vec<&[u8]> = d.iter().map(|(k, _)| k.as_slice()).filter(|k| !skip.contains(k)).collect();
            k.sort_unstable();
            k
        }
        if keys(a, skip) != keys(b, skip) {
            return Some(false);
        }
        for (k, v) in a.iter().filter(|(k, _)| !skip.contains(&k.as_slice())) {
            let Some(w) = b.get(k) else { return Some(false) };
            if !self.objects(v, w, depth + 1)? {
                return Some(false);
            }
        }
        Some(true)
    }

    /// Object equality across the two revisions: references resolved in their own revision,
    /// dictionaries regardless of key order, a string by its bytes, a number by its value, and
    /// stream data byte for byte as stored. A page is compared by reference only, since every
    /// annotation added to it rewrites it.
    fn objects(&mut self, a: &Object, b: &Object, depth: usize) -> Option<bool> {
        self.left = self.left.checked_sub(1)?;
        if depth > MAX_FIELD_DEPTH {
            return None;
        }
        match (a, b) {
            (Object::Ref(x), Object::Ref(y)) => {
                if !self.assumed.insert((*x, *y)) {
                    return Some(true);
                }
                let (was, is) = (self.old.get(*x), self.doc.get(*y));
                if x == y && was.as_dict().is_some_and(|d| matches!(d.name(b"Type"), Some(b"Page" | b"Pages"))) {
                    return Some(true);
                }
                self.objects(&was, &is, depth)
            }
            (Object::Ref(x), _) => {
                let was = self.old.get(*x);
                self.objects(&was, b, depth)
            }
            (_, Object::Ref(y)) => {
                let is = self.doc.get(*y);
                self.objects(a, &is, depth)
            }
            (Object::String(x), Object::String(y)) => Some(x.bytes == y.bytes),
            (Object::Int(i), Object::Real(r)) | (Object::Real(r), Object::Int(i)) => Some(r.fract() == 0.0 && *r as i128 == i128::from(*i)),
            (Object::Dict(x), Object::Dict(y)) => self.dicts(x, y, &[], depth),
            (Object::Stream(x), Object::Stream(y)) => Some(x.raw == y.raw && self.dicts(&x.dict, &y.dict, &[], depth)?),
            (Object::Array(x), Object::Array(y)) => {
                if x.len() != y.len() {
                    return Some(false);
                }
                for (v, w) in x.iter().zip(y) {
                    if !self.objects(v, w, depth + 1)? {
                        return Some(false);
                    }
                }
                Some(true)
            }
            _ => Some(a == b),
        }
    }
}

/// The names `rules` would lock among `names`, for the tests below.
#[cfg(test)]
fn locked_names<'a>(rules: &[Rule], names: &[&'a str]) -> Vec<&'a str> {
    names.iter().copied().filter(|n| rules.iter().any(|r| r.locks(n))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_cos::PdfString;

    fn dict(entries: Vec<(&str, Object)>) -> Dict {
        let mut d = Dict::new();
        for (k, v) in entries {
            d.set(k.as_bytes().to_vec(), v);
        }
        d
    }

    fn names(n: &[&str]) -> Object {
        Object::Array(n.iter().map(|s| Object::String(PdfString::text(s))).collect())
    }

    fn sig_field(lock: Option<Object>) -> Field {
        let mut d = dict(vec![("FT", Object::name("Sig")), ("T", Object::String(PdfString::text("Approval")))]);
        if let Some(l) = lock {
            d.set(b"Lock".to_vec(), l);
        }
        Field { name: "Approval".into(), r: None, dict: d, widgets: Vec::new() }
    }

    fn field_mdp(action: &str, fields: &[&str]) -> Dict {
        let params = dict(vec![("Type", Object::name("TransformParams")), ("Action", Object::name(action)), ("Fields", names(fields))]);
        let reference =
            dict(vec![("Type", Object::name("SigRef")), ("TransformMethod", Object::name("FieldMDP")), ("TransformParams", Object::Dict(params))]);
        dict(vec![("Type", Object::name("Sig")), ("Reference", Object::Array(vec![Object::Dict(reference)]))])
    }

    const ALL_NAMES: &[&str] = &["amount", "note", "address", "address.city", "addressee"];

    /// A FieldMDP reference in the signature value locks as a `/Lock` on its field does: the
    /// signers that write only one of them are read either way.
    #[test]
    fn a_fieldmdp_reference_in_the_value_locks_fields() {
        let doc = Document::new_empty();
        let rules = rules(&doc, &sig_field(None), &field_mdp("Include", &["amount"])).unwrap();
        assert_eq!(locked_names(&rules, ALL_NAMES), ["amount"]);
        let rules = rules_of(&doc, None, &field_mdp("Exclude", &["note", "address"]));
        assert_eq!(locked_names(&rules, ALL_NAMES), ["amount", "addressee"]);
        let rules = rules_of(&doc, None, &field_mdp("All", &[]));
        assert_eq!(locked_names(&rules, ALL_NAMES), ALL_NAMES);
    }

    /// A lock's names cover the fields below them, never a field that merely starts the same.
    #[test]
    fn a_listed_name_covers_its_kids_only() {
        let doc = Document::new_empty();
        let lock = dict(vec![("Type", Object::name("SigFieldLock")), ("Action", Object::name("Include")), ("Fields", names(&["address"]))]);
        let rules = rules_of(&doc, Some(Object::Dict(lock)), &dict(vec![]));
        assert_eq!(locked_names(&rules, ALL_NAMES), ["address", "address.city"]);
    }

    /// A lock that is there but can't be read leaves the signature's locks unknown, never "none".
    #[test]
    fn an_unreadable_lock_is_not_no_lock() {
        let doc = Document::new_empty();
        let odd = dict(vec![("Action", Object::name("Some"))]);
        assert!(rules(&doc, &sig_field(Some(Object::Dict(odd))), &dict(vec![])).is_err());
        assert!(rules(&doc, &sig_field(Some(Object::Int(1))), &dict(vec![])).is_err());
        let bare = dict(vec![("TransformMethod", Object::name("FieldMDP"))]);
        assert!(rules(&doc, &sig_field(None), &dict(vec![("Reference", Object::Array(vec![Object::Dict(bare)]))])).is_err());
        assert_eq!(locked_fields(&doc, &doc, &sig_field(Some(Object::Int(1))), &dict(vec![])), Locked::Unchecked);
        // No lock at all, and a DocMDP reference, lock nothing.
        let docmdp = dict(vec![("TransformMethod", Object::name("DocMDP"))]);
        assert_eq!(rules(&doc, &sig_field(None), &dict(vec![("Reference", Object::Array(vec![Object::Dict(docmdp)]))])), Ok(Vec::new()));
    }

    fn rules_of(doc: &Document, lock: Option<Object>, value: &Dict) -> Vec<Rule> {
        rules(doc, &sig_field(lock), value).unwrap()
    }
}
