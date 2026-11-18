//! Fields a signature locks: its field's `/Lock` (ISO 32000-2 §12.7.5.5, Table 236) and the
//! FieldMDP transform in its value's `/Reference` (§12.8.2.4). Signers write either or both. Once
//! the field is signed, a change to a field the lock names breaks the signature, whatever the
//! change would count as otherwise (#6757).
//!
//! The locks are read from the signed revision, and the locked fields are compared between that
//! revision and the document as it is now: each field's dictionary and the ones above it (an
//! inherited `/V` is its value too), and its widgets. A field that is gone, a second field under
//! its name, or a new field under a locked one is a change as well, and so is a change to the
//! form-wide appearance inputs a locked field is drawn from. A lock that is there but can't be
//! read, or a form too large to compare, is unchecked: never "no lock".

use std::collections::{BTreeMap, HashSet as NameSet};

use super::listing::{Field, MAX_ENTRIES, MAX_FIELD_NAME_BYTES, MAX_FIELDS, MAX_NAME_BYTES};
use super::{Dict, Document, HashSet, ObjRef, Object};

/// What a lock covers (`/Action`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    All,
    Include,
    Exclude,
}

/// One lock: its action and the fully qualified names it lists, exactly as written.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Rule {
    action: Action,
    names: NameSet<String>,
}

impl Rule {
    /// Which of `form`'s nodes the rule locks, by node. A listed name also covers the fields below
    /// it (`address` covers `address.city`): a node is listed when its name is, or its parent is.
    /// Parents come before their kids in a walk, so one pass decides every node, each by one look
    /// at its own name.
    fn locks(&self, form: &Form) -> Vec<bool> {
        let mut listed: Vec<bool> = Vec::with_capacity(form.nodes.len());
        for n in &form.nodes {
            let above = n.parent.and_then(|p| listed.get(p).copied()).unwrap_or(false);
            listed.push(above || self.names.contains(&n.name));
        }
        match self.action {
            Action::All => vec![true; listed.len()],
            Action::Include => listed,
            Action::Exclude => listed.into_iter().map(|l| !l).collect(),
        }
    }
}

/// Most names one lock lists; real locks name a handful.
const MAX_LOCK_NAMES: usize = MAX_FIELDS;
/// Most locks read for one signature: its `/Lock` and its FieldMDP references. Real signatures
/// carry one or two; each one costs a pass over the form's names.
const MAX_RULES: usize = 16;
/// Deepest field tree walked, and deepest nesting (references included) compared. Real forms nest
/// a few levels.
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

/// A field name or a name in a lock's `/Fields`, exactly as written: a text string decoded, or a
/// name's bytes. Nothing is trimmed, so `address ` and `address` stay two names, as the file has
/// them.
fn exact_name(o: &Object) -> Option<String> {
    match o {
        Object::String(s) => Some(s.to_text()),
        Object::Name(n) => Some(String::from_utf8_lossy(n).into_owned()),
        _ => None,
    }
}

/// A lock dictionary (`/Lock`) or FieldMDP transform parameters. `Err` when it is there but can't
/// be read: its action is not one of the three, an `Include` or `Exclude` has no readable
/// `/Fields` array, a name in it is not a string, or it lists more names than are read.
fn rule(doc: &Document, d: &Dict) -> Result<Rule, ()> {
    let action = match d.name(b"Action") {
        Some(b"All") => Action::All,
        Some(b"Include") => Action::Include,
        Some(b"Exclude") => Action::Exclude,
        _ => return Err(()),
    };
    if action == Action::All {
        return Ok(Rule { action, names: NameSet::new() });
    }
    // An Include read as naming nothing would lock nothing, and an Exclude read so would lock
    // everything: neither is what an unreadable list says.
    let listed = d.get(b"Fields").map(|f| doc.resolve(f)).ok_or(())?;
    let listed = listed.as_array().ok_or(())?;
    if listed.len() > MAX_LOCK_NAMES {
        return Err(());
    }
    let names = listed.iter().map(|n| exact_name(&doc.resolve(n)).ok_or(())).collect::<Result<_, _>>()?;
    Ok(Rule { action, names })
}

/// The locks of signature field `field`, whose value is `value`, both as the signed revision `old`
/// has them. `Err` when one is there but can't be read: a `/Lock` or `/Reference` of the wrong
/// type, a reference that is not a dictionary, a FieldMDP reference without parameters, or more
/// locks than are read.
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
    let refs: &[Object] = match refs.as_deref() {
        None | Some(Object::Null) => &[],
        Some(Object::Array(a)) => a,
        // A single reference dictionary where an array belongs may still be a FieldMDP lock.
        Some(_) => return Err(()),
    };
    for r in refs {
        let r = old.resolve(r);
        let rd = r.as_dict().ok_or(())?;
        if rd.name(b"TransformMethod") != Some(b"FieldMDP") {
            continue;
        }
        // A FieldMDP reference that does not say what it locks is a lock that can't be read.
        let params = rd.get(b"TransformParams").map(|p| old.resolve(p)).ok_or(())?;
        out.push(rule(old, params.as_dict().ok_or(())?)?);
        if out.len() > MAX_RULES {
            return Err(());
        }
    }
    Ok(out)
}

/// Whether any field the signature in `field` (signed with `value`) locks in the signed revision
/// `old` is different in `doc`, the document as it is now. The signature's own field is one of
/// them when its lock covers it: its value is the signature, which is checked anyway, but its
/// widgets are what a reader sees of it.
pub(super) fn locked_fields(doc: &Document, old: &Document, field: &Field, value: &Dict) -> Locked {
    let Ok(rules) = rules(old, field, value) else { return Locked::Unchecked };
    if rules.is_empty() {
        return Locked::Kept;
    }
    let Some(before) = form_fields(old) else { return Locked::Unchecked };
    let mut locked_nodes = vec![false; before.nodes.len()];
    for rule in &rules {
        for (l, r) in locked_nodes.iter_mut().zip(rule.locks(&before)) {
            *l |= r;
        }
    }
    // A name is locked when a node under it is; nodes sharing a name share their ancestors' names.
    let is_locked = |entries: &[Entry]| entries.iter().any(|e| locked_nodes.get(e.node).copied().unwrap_or(false));
    let locked: Vec<&String> = before.fields.iter().filter(|(_, e)| is_locked(e)).map(|(n, _)| n).collect();
    if locked.is_empty() {
        return Locked::Kept;
    }
    let Some(now) = form_fields(doc) else { return Locked::Unchecked };
    let mut same = Same { old, doc, left: MAX_COMPARED, assumed: HashSet::new() };
    let mut drawn_from_form = false;
    for name in locked {
        let (Some(was), Some(is)) = (before.fields.get(name), now.fields.get(name)) else { return Locked::Changed };
        if was.len() != is.len() {
            return Locked::Changed;
        }
        for (w, i) in was.iter().zip(is) {
            drawn_from_form |= before.lacks_appearance(w);
            match same.field(&before, w, &now, i) {
                Some(true) => {}
                Some(false) => return Locked::Changed,
                None => return Locked::Unchecked,
            }
        }
    }
    // A field added since, below a field that was locked, changes that field: its kids are part
    // of it. A field added anywhere else is not one the signature saw, so it locks nothing.
    for n in &now.nodes {
        if before.fields.contains_key(&n.name) {
            continue;
        }
        let mut up = n.parent;
        while let Some(p) = up.and_then(|p| now.nodes.get(p)) {
            if let Some(entries) = before.fields.get(&p.name) {
                if is_locked(entries) {
                    return Locked::Changed;
                }
                break;
            }
            up = p.parent;
        }
    }
    // What the form says about drawing fields: whether a reader redraws every field
    // (/NeedAppearances), and, for a locked field with no appearance of its own, the defaults it
    // is drawn with.
    let mut keys: Vec<&[u8]> = vec![b"NeedAppearances"];
    if drawn_from_form {
        keys.extend([b"DA".as_slice(), b"DR", b"Q"]);
    }
    for k in keys {
        match same.entry(before.acroform.as_ref().and_then(|a| a.get(k)), now.acroform.as_ref().and_then(|a| a.get(k))) {
            Some(true) => {}
            Some(false) => return Locked::Changed,
            None => return Locked::Unchecked,
        }
    }
    Locked::Kept
}

/// One field dictionary in a walk of the form: its fully qualified name, and the one above it.
struct Node {
    name: String,
    dict: Dict,
    parent: Option<usize>,
}

/// One field under its fully qualified name: its node, and its widgets apart from it.
struct Entry {
    node: usize,
    widgets: Vec<Dict>,
}

/// Every field of a form by fully qualified name, terminal or not, in walk order (a parent before
/// its kids), and the form dictionary itself.
struct Form {
    acroform: Option<Dict>,
    nodes: Vec<Node>,
    fields: BTreeMap<String, Vec<Entry>>,
}

impl Form {
    /// Whether field `e` has a widget with no appearance of its own, which a reader draws from the
    /// form's defaults: a separate widget without `/AP`, or a field that is its own widget.
    fn lacks_appearance(&self, e: &Entry) -> bool {
        let has_ap = |d: &Dict| d.get(b"AP").is_some_and(|ap| !matches!(ap, Object::Null));
        let merged = self.nodes.get(e.node).map(|n| &n.dict).filter(|d| e.widgets.is_empty() && !d.contains(b"Kids"));
        merged.is_some_and(|d| !has_ap(d)) || e.widgets.iter().any(|w| !has_ap(w))
    }
}

/// The fields of `doc`'s form, or `None` when a cap stopped the walk (a field it did not read may
/// be one a lock names). The caps are the signature listing's.
fn form_fields(doc: &Document) -> Option<Form> {
    let mut form = Form { acroform: None, nodes: Vec::new(), fields: BTreeMap::new() };
    let Some(root) = doc.root() else { return Some(form) };
    let Some(af) = doc.get(root).as_dict().and_then(|c| c.get(b"AcroForm")).map(|a| doc.resolve(a)) else { return Some(form) };
    let af = match &*af {
        Object::Dict(d) => d.clone(),
        Object::Null => return Some(form),
        _ => return None,
    };
    let top = af.get(b"Fields").map(|f| doc.resolve(f));
    form.acroform = Some(af);
    let top: Vec<Object> = match top.as_deref() {
        None | Some(Object::Null) => return Some(form),
        Some(Object::Array(a)) => a.clone(),
        Some(_) => return None,
    };
    let (mut fields_left, mut entries_left, mut names_left) = (MAX_FIELDS, MAX_ENTRIES, MAX_NAME_BYTES);
    let mut seen: HashSet<ObjRef> = HashSet::new();
    let mut stack: Vec<(Object, Option<usize>, usize)> = Vec::new();
    entries_left = entries_left.checked_sub(top.len())?;
    for f in top.into_iter().rev() {
        stack.push((f, None, 0));
    }
    while let Some((o, parent, depth)) = stack.pop() {
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
        let prefix = parent.and_then(|p| form.nodes.get(p)).map_or("", |p| p.name.as_str());
        let part = d.get(b"T").and_then(|t| exact_name(&doc.resolve(t)));
        let name = match (part, prefix.is_empty()) {
            (Some(p), true) => p,
            (Some(p), false) => format!("{prefix}.{p}"),
            (None, _) => prefix.to_owned(),
        };
        if name.len() > MAX_FIELD_NAME_BYTES {
            return None;
        }
        // Counted twice: each name is kept by its node and as a key.
        names_left = names_left.checked_sub(name.len().saturating_mul(2))?;
        let node = form.nodes.len();
        let kids = d.get(b"Kids").map(|k| doc.resolve(k));
        let kids: Vec<Object> = kids.as_deref().and_then(Object::as_array).cloned().unwrap_or_default();
        entries_left = entries_left.checked_sub(kids.len())?;
        form.nodes.push(Node { name: name.clone(), dict: d, parent });
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
            stack.push((k, Some(node), depth + 1));
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

    /// [`Same::objects`] for an entry that may be absent: both absent, or both present and the
    /// same.
    fn entry(&mut self, a: Option<&Object>, b: Option<&Object>) -> Option<bool> {
        match (a, b) {
            (None, None) => Some(true),
            (Some(a), Some(b)) => self.objects(a, b, 0),
            _ => Some(false),
        }
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
    /// stream data byte for byte as stored. A page, the page tree and the catalog are compared by
    /// reference only, since every annotation or field added rewrites them.
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
                // The document's own structure is compared by reference: a page, the page tree, and
                // the catalog, which a FieldMDP reference's /Data names (ISO 32000-2 Table 256) and
                // which holds the whole form, so a field added anywhere would otherwise read as a
                // change to a locked one. The catalog is the signed revision's /Root, with or
                // without its /Type.
                let structural =
                    Some(*x) == self.old.root() || was.as_dict().is_some_and(|d| matches!(d.name(b"Type"), Some(b"Page" | b"Pages" | b"Catalog")));
                if x == y && structural {
                    return Some(true);
                }
                // Each reference followed counts as a level, so a chain of references (or a
                // cycle of them on one side only) ends at the depth cap.
                self.objects(&was, &is, depth + 1)
            }
            (Object::Ref(x), _) => {
                let was = self.old.get(*x);
                self.objects(&was, b, depth + 1)
            }
            (_, Object::Ref(y)) => {
                let is = self.doc.get(*y);
                self.objects(a, &is, depth + 1)
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

    /// A form of fields named `names`, each below the one named by its name up to the last dot.
    fn form_of(names: &[&str]) -> Form {
        let mut form = Form { acroform: None, nodes: Vec::new(), fields: BTreeMap::new() };
        for n in names {
            let parent = n.rsplit_once('.').and_then(|(up, _)| names.iter().position(|x| *x == up));
            form.nodes.push(Node { name: (*n).to_owned(), dict: Dict::new(), parent });
        }
        form
    }

    /// The names `rules` lock among `names`.
    fn locked_names<'a>(rules: &[Rule], names: &[&'a str]) -> Vec<&'a str> {
        let form = form_of(names);
        let by_rule: Vec<Vec<bool>> = rules.iter().map(|r| r.locks(&form)).collect();
        names.iter().enumerate().filter(|(i, _)| by_rule.iter().any(|l| l[*i])).map(|(_, n)| *n).collect()
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

    fn lock(entries: Vec<(&str, Object)>) -> Option<Object> {
        Some(Object::Dict(dict(entries)))
    }

    fn rules_of(doc: &Document, lock: Option<Object>, value: &Dict) -> Vec<Rule> {
        rules(doc, &sig_field(lock), value).unwrap()
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
        let l = lock(vec![("Type", Object::name("SigFieldLock")), ("Action", Object::name("Include")), ("Fields", names(&["address"]))]);
        let rules = rules_of(&doc, l, &dict(vec![]));
        assert_eq!(locked_names(&rules, ALL_NAMES), ["address", "address.city"]);
    }

    /// Names are matched exactly as written: a lock on `address ` (with its space) covers the field
    /// of that name and its kids, and not `address`.
    #[test]
    fn names_match_as_written() {
        let doc = Document::new_empty();
        let l = lock(vec![("Action", Object::name("Include")), ("Fields", names(&["address "]))]);
        let rules = rules_of(&doc, l, &dict(vec![]));
        assert_eq!(locked_names(&rules, &["address", "address.city", "address ", "address .city"]), ["address ", "address .city"]);
    }

    /// A lock that is there but can't be read leaves the signature's locks unknown, never "none".
    #[test]
    fn an_unreadable_lock_is_not_no_lock() {
        let doc = Document::new_empty();
        let err = |lock: Option<Object>, value: Dict| rules(&doc, &sig_field(lock), &value).is_err();
        assert!(err(lock(vec![("Action", Object::name("Some"))]), dict(vec![])));
        assert!(err(Some(Object::Int(1)), dict(vec![])));
        // An Include or Exclude whose names can't be read: missing, not an array, or not strings.
        for action in ["Include", "Exclude"] {
            assert!(err(lock(vec![("Action", Object::name(action))]), dict(vec![])), "{action} without /Fields");
            let one = dict(vec![("amount", Object::Int(1))]);
            assert!(err(lock(vec![("Action", Object::name(action)), ("Fields", Object::Dict(one))]), dict(vec![])), "{action} /Fields not an array");
            let mixed = Object::Array(vec![Object::String(PdfString::text("amount")), Object::Int(7)]);
            assert!(err(lock(vec![("Action", Object::name(action)), ("Fields", mixed)]), dict(vec![])), "{action} with a name that is not one");
        }
        // A FieldMDP reference without parameters, a reference that is not a dictionary, and a
        // single reference dictionary where the array belongs.
        let bare = dict(vec![("TransformMethod", Object::name("FieldMDP"))]);
        assert!(err(None, dict(vec![("Reference", Object::Array(vec![Object::Dict(bare)]))])));
        assert!(err(None, dict(vec![("Reference", Object::Array(vec![Object::Int(3)]))])));
        let one = field_mdp("All", &[]).get(b"Reference").and_then(Object::as_array).cloned().unwrap();
        assert!(err(None, dict(vec![("Reference", one[0].clone())])));
        assert_eq!(locked_fields(&doc, &doc, &sig_field(Some(Object::Int(1))), &dict(vec![])), Locked::Unchecked);
        // No lock at all, an All lock without /Fields, and a DocMDP reference read fine.
        assert!(!err(lock(vec![("Action", Object::name("All"))]), dict(vec![])));
        let docmdp = dict(vec![("TransformMethod", Object::name("DocMDP"))]);
        assert_eq!(rules(&doc, &sig_field(None), &dict(vec![("Reference", Object::Array(vec![Object::Dict(docmdp)]))])), Ok(Vec::new()));
    }

    /// A classic-xref PDF of `objs`, numbered from 1, with object 1 as the catalog.
    fn write_pdf(objs: &[&str]) -> Vec<u8> {
        let mut out = b"%PDF-1.7
"
        .to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(
                format!(
                    "{} 0 obj
{o}
endobj
",
                    i + 1
                )
                .as_bytes(),
            );
        }
        let xref = out.len();
        out.extend_from_slice(
            format!(
                "xref
0 {}
0000000000 65535 f 
",
                objs.len() + 1
            )
            .as_bytes(),
        );
        for o in offsets {
            out.extend_from_slice(
                format!(
                    "{o:010} 00000 n 
"
                )
                .as_bytes(),
            );
        }
        out.extend_from_slice(
            format!(
                "trailer
<< /Size {} /Root 1 0 R >>
startxref
{xref}
%%EOF
",
                objs.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    fn open(bytes: Vec<u8>) -> Document {
        Document::open(std::sync::Arc::new(bytes)).unwrap()
    }

    /// `doc` with `edit` saved as one more revision, read back.
    fn revised(doc: &Document, edit: impl FnOnce(&mut Document)) -> Document {
        let mut doc = doc.clone();
        edit(&mut doc);
        let options = pdfcraft_cos::SaveOptions { object_streams: false, ..pdfcraft_cos::SaveOptions::default() };
        open(pdfcraft_cos::write_incremental(&doc, &options).unwrap())
    }

    fn at(num: u32) -> ObjRef {
        ObjRef { num, generation: 0 }
    }

    /// From review on #6763: a FieldMDP reference's `/Data` points at the catalog (ISO 32000-2
    /// Table 256). A lock that covers the signature's own field compares its value, reference and
    /// all, and the catalog holds the form: adding a signature in a new field afterwards rewrites
    /// the form, which is not a change to the locked field. The catalog is compared by reference,
    /// as pages are. Its own field's appearance is still compared.
    #[test]
    fn a_reference_to_the_catalog_is_not_followed_into_the_form() {
        let lock = "/Action /Include /Fields [(Signature2)]";
        let old = open(write_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm 7 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Annots [4 0 R] >>",
            &format!(
                "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Signature2) /Rect [0 0 100 50] /P 3 0 R /F 4 /V 5 0 R /AP << /N 6 0 R >> /Lock << /Type /SigFieldLock {lock} >> >>"
            ),
            &format!(
                "<< /Type /Sig /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 10] /Contents <00> /Reference [<< /Type /SigRef /TransformMethod /FieldMDP /TransformParams << /Type /TransformParams {lock} /V /1.2 >> /Data 1 0 R >>] >>"
            ),
            "<< /Type /XObject /Subtype /Form /BBox [0 0 100 50] /Length 0 >>
stream

endstream",
            "<< /Fields [4 0 R] /SigFlags 3 >>",
        ]));
        let field = super::super::listing::sig_fields(&old).into_iter().find(|f| f.name == "Signature2").unwrap();
        let value = field.signature(&old).unwrap();
        // A second signature in a new top-level field, and the form's resources rewritten.
        let signed_again = revised(&old, |doc| {
            let mut v = Dict::new();
            v.set(b"Type".to_vec(), Object::name("Sig"));
            v.set(b"Contents".to_vec(), Object::String(PdfString::literal(vec![0])));
            let v = doc.add(Object::Dict(v));
            let mut w = Dict::new();
            w.set(b"Type".to_vec(), Object::name("Annot"));
            w.set(b"Subtype".to_vec(), Object::name("Widget"));
            w.set(b"FT".to_vec(), Object::name("Sig"));
            w.set(b"T".to_vec(), Object::String(PdfString::text("Signature1")));
            w.set(b"Rect".to_vec(), Object::Array(vec![Object::Int(0); 4]));
            w.set(b"P".to_vec(), Object::Ref(at(3)));
            w.set(b"V".to_vec(), Object::Ref(v));
            let w = doc.add(Object::Dict(w));
            doc.update_dict(at(7), |form| {
                form.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(at(4)), Object::Ref(w)]));
                form.set(b"DR".to_vec(), Object::Dict(dict(vec![("Font", Object::Dict(Dict::new()))])));
            })
            .unwrap();
            doc.update_dict(at(3), |page| page.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(at(4)), Object::Ref(w)]))).unwrap();
        });
        assert_eq!(locked_fields(&signed_again, &old, &field, &value), Locked::Kept);
        // The locked field's own widget repainted is still a change.
        let repainted = revised(&old, |doc| {
            doc.update_dict(at(4), |w| w.set(b"AP".to_vec(), Object::Dict(dict(vec![("N", Object::Null)])))).unwrap();
        });
        assert_eq!(locked_fields(&repainted, &old, &field, &value), Locked::Changed);
    }

    /// More locks than are read is a lock that can't be read.
    #[test]
    fn too_many_locks_are_unchecked() {
        let doc = Document::new_empty();
        let one = field_mdp("All", &[]).get(b"Reference").and_then(Object::as_array).cloned().unwrap();
        let many = vec![one[0].clone(); MAX_RULES + 1];
        assert!(rules(&doc, &sig_field(None), &dict(vec![("Reference", Object::Array(many))])).is_err());
    }
}
