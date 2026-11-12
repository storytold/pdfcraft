//! What a later revision did, explained as the operations a signature permits (stage B of #7032).
//!
//! Every object a later revision changed or added must be claimed by an operation the signature
//! permits, and each operation claims only the objects it may touch:
//!
//! | Operation | Levels | May touch |
//! |---|---|---|
//! | Fill an unlocked field | 2, 3, approval | the field's `/V` (and a list box's `/I`); its widgets' `/AS` and appearance, when the value changed; `/NeedAppearances`; `/DR` entries only added |
//! | Sign an empty field, or add a signature field | 2, 3, approval | a field or widget gaining a new signature dictionary whose `/ByteRange` signs the revision that writes it (`/V`, and a widget's appearance, nothing else of one that was there), the form's `/Fields` and `/SigFlags` |
//! | Add or change a comment | 3, approval | a non-widget annotation added to a page's `/Annots` (with its appearance and pop-up), an existing one changed, removed or restacked |
//! | Document timestamp | all | an invisible field, or a standalone dictionary, holding a `/DocTimeStamp` that signs the revision that writes it |
//! | Validation data | all | the document security store, its data and `/VRI` |
//! | Housekeeping | all | the signed `/Info` and XMP, the catalog's `/Metadata`, `/NeedsRendering` and developer extensions, cross-reference and object streams |
//!
//! A new object is claimed by the operation that claims what refers to it, through the entries
//! that operation changed, unless it plays a role of its own (an annotation, a field, a
//! signature, a page): those are claimed only by an operation on them. Whatever no operation
//! claims, or claims at a level the signature does not permit, is a change the signature does not
//! permit. A miss is a visible false warning, never a silent pass.

use super::changes::{
    Change, Observed, acroform, annot_refs, gains_signature, inherited, is_signature_field, lost_entries, note, observe, only_grew, perms_changed,
    refs, same_entries, same_entry, same_object, signed_revision, stored_at,
};
use super::locks::{LockScope, Locked};
use super::types::{Modification, kind};
use super::{Dict, Document, HashMap, HashSet, ObjRef, Object};

/// What a later revision may do after a signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    /// Fill an unlocked field.
    Fill,
    /// Sign an empty field, or add a signature field with its signature.
    Sign,
    /// Add a document timestamp that draws nothing.
    Timestamp,
    /// Add, change, remove or restack comments.
    Annotate,
    /// Add to the document security store.
    ValidationData,
    /// Update what a save always updates: the document information and XMP, file plumbing.
    Housekeeping,
    /// Write the document information as a new object. As before this recogniser, a no-changes
    /// certification does not permit it.
    NewInfo,
    /// Write cross-reference and object streams: how a save is stored, which shows nothing and is
    /// not listed.
    Plumbing,
    /// Restate what was signed: a signed page's annotations listed again, the same ones in the
    /// same order, in an array of their own. Not listed either.
    Restated,
}

impl Op {
    /// Whether a signature with DocMDP `p` (`None`: an approval signature) permits it.
    fn permitted(self, p: Option<u8>) -> bool {
        match self {
            Op::Fill | Op::Sign => matches!(p, Some(2 | 3) | None),
            Op::Annotate => matches!(p, Some(3) | None),
            Op::NewInfo => p != Some(1),
            Op::Timestamp | Op::ValidationData | Op::Housekeeping | Op::Plumbing | Op::Restated => true,
        }
    }

    /// The word of [`kind`] it is listed by, if it is listed.
    fn word(self) -> Option<&'static str> {
        match self {
            Op::Fill => Some(kind::FORM_FILL),
            Op::Sign | Op::Timestamp => Some(kind::SIGNATURE),
            Op::Annotate => Some(kind::COMMENTS),
            Op::ValidationData => Some(kind::SECURITY_STORE),
            Op::Housekeeping | Op::NewInfo => Some(kind::METADATA),
            Op::Plumbing | Op::Restated => None,
        }
    }

    /// The operation a change described by `word` would be, if one claimed it.
    fn named(word: &str) -> Option<Op> {
        match word {
            kind::FORM_FILL => Some(Op::Fill),
            kind::SIGNATURE => Some(Op::Sign),
            kind::COMMENTS => Some(Op::Annotate),
            kind::SECURITY_STORE => Some(Op::ValidationData),
            kind::METADATA => Some(Op::Housekeeping),
            _ => None,
        }
    }
}

/// Words for changes no operation can claim, whatever the level.
const NEVER: &[&str] = &[kind::PAGE_CONTENT, kind::PAGES, kind::STRUCTURE, kind::LINKS, kind::OTHER];

/// Most references followed from claimed objects into new ones, for one judgement. Past it the
/// rest is not claimed, and the change is refused.
const MAX_CLAIM_STEPS: usize = 1 << 20;
/// Deepest nesting of direct objects followed for references.
const MAX_NESTING: u8 = 32;
/// Longest array compared element by element with what it was (quadratic); a longer one counts
/// every element as changed.
const MAX_COMPARED: usize = 10_000;

/// What later revisions changed since the signed revision `old`, judged under DocMDP `p` (`None`:
/// an approval signature) and the signature's locks: permitted only when every changed or new
/// object is claimed by an operation `p` and the locks permit.
pub(super) fn judge_changes(doc: &Document, old: Option<Document>, p: Option<u8>, locks: &LockScope) -> Modification {
    let Some(old) = old else {
        return Modification::Disallowed(vec![kind::UNREADABLE]);
    };
    let seen = observe(doc, &old);
    let mut r = Recogniser {
        doc,
        old: &old,
        changed: seen.changes.iter().map(|c| (c.num, c)).collect(),
        seen: &seen,
        p,
        locks,
        claims: HashMap::new(),
        blocked: HashSet::new(),
        form: FormChange::Unchanged,
        steps: MAX_CLAIM_STEPS,
        exhausted: false,
    };
    r.sign_and_stamp();
    r.fill();
    r.form();
    r.pages();
    r.comments();
    r.validation_data();
    r.housekeeping();
    r.catalog();
    r.propagate();
    r.verdict()
}

/// What the interactive form dictionary gained since signing, as operations.
enum FormChange {
    Unchanged,
    /// Explained by these operations.
    By(Vec<Op>),
    /// Something no operation explains.
    Unexplained,
}

struct Recogniser<'a> {
    doc: &'a Document,
    old: &'a Document,
    seen: &'a Observed,
    changed: HashMap<u32, &'a Change>,
    p: Option<u8>,
    locks: &'a LockScope,
    /// The operations that claim each changed or new object.
    claims: HashMap<u32, Vec<Op>>,
    /// Objects a fill or a signature would claim but the signature's locks forbid.
    blocked: HashSet<u32>,
    form: FormChange,
    steps: usize,
    exhausted: bool,
}

impl Recogniser<'_> {
    fn claim(&mut self, num: u32, op: Op) {
        let ops = self.claims.entry(num).or_default();
        if !ops.contains(&op) {
            ops.push(op);
        }
    }

    fn claimed_by(&self, num: u32, op: Op) -> bool {
        self.claims.get(&num).is_some_and(|ops| ops.contains(&op))
    }

    /// The signing operation this revision carries out, if it signs anything.
    fn signing(&self) -> Option<Op> {
        let any = |op| self.claims.values().any(|ops| ops.contains(&op));
        if any(Op::Sign) {
            Some(Op::Sign)
        } else if any(Op::Timestamp) {
            Some(Op::Timestamp)
        } else {
            None
        }
    }

    fn is_new(&self, num: u32) -> bool {
        self.changed.get(&num).is_some_and(|c| c.before.is_none())
    }

    /// Signing: a field or widget whose signature is new and signs the revision that writes it
    /// (#6771's rule). An existing one may gain only its value and, for a widget, the appearance
    /// that shows it: a name, an action or any other key changed with it is no signing (review of
    /// #6771). A document timestamp in a widget that draws nothing, or standing alone, is
    /// validation data every level permits (#6364).
    fn sign_and_stamp(&mut self) {
        let (doc, old) = (self.doc, self.old);
        for c in &self.seen.changes {
            let Some(d) = c.now.as_dict() else { continue };
            let before = c.before.as_deref().and_then(Object::as_dict);
            if is_signature_field(doc, d) && gains_signature(doc, old, &self.seen.ends, c.num, d, before) {
                if before.is_some_and(|b| !same_entries(d, b, &[b"V", b"AP"])) {
                    continue;
                }
                if self.locks.objects.contains(&c.num) {
                    self.blocked.insert(c.num);
                    continue;
                }
                let value = inherited(doc, d, b"V");
                let stamp = value.as_ref().and_then(|(_, v)| doc.resolve(v).as_dict().map(super::validate::is_doc_timestamp)).unwrap_or(false);
                let widget = d.name(b"Subtype") == Some(b"Widget");
                let op = if stamp && !(widget && super::listing::widget_shown(doc, d)) { Op::Timestamp } else { Op::Sign };
                self.claim(c.num, op);
                if let Some(v) = value.and_then(|(_, v)| v.as_ref()).filter(|v| self.is_new(v.num)) {
                    self.claim(v.num, op);
                }
            } else if c.before.is_none() && super::validate::is_doc_timestamp(d) && self.signs_its_own_revision(c.num, d) {
                self.claim(c.num, Op::Timestamp);
            }
        }
    }

    /// Whether the signature dictionary `v`, object `num`, signs the revision that stores it.
    fn signs_its_own_revision(&self, num: u32, v: &Dict) -> bool {
        signed_revision(self.doc, self.old, &self.seen.ends, v).is_some_and(|range| stored_at(self.doc, num).iter().any(|at| range.contains(at)))
    }

    /// Filling: a widget given a new value and the appearance that shows it, a field given a new
    /// value (a radio group's, or one shown by several widgets), the XFA data a fill rewrites. A
    /// field the signature locks is not filled (#6757).
    fn fill(&mut self) {
        let (old, snapshot) = (self.old, &self.seen.snapshot);
        let root = old.root().map(|r| r.num);
        for c in &self.seen.changes {
            if c.label != Some(kind::FORM_FILL) || Some(c.num) == snapshot.old_form || Some(c.num) == root {
                continue;
            }
            let fills = match (&*c.now, c.before.as_deref()) {
                (Object::Stream(_), Some(Object::Stream(_))) => snapshot.old_xfa_data.contains(&c.num),
                (Object::Dict(d), Some(Object::Dict(b))) if !is_signature_field(old, b) => {
                    if b.name(b"Subtype") == Some(b"Widget") {
                        // Judged already: a new value, and only the value and its appearance changed.
                        true
                    } else {
                        (b.contains(b"FT") || (b.contains(b"T") && b.contains(b"Kids"))) && same_entries(d, b, &[b"V", b"I"])
                    }
                }
                _ => false,
            };
            if !fills {
                continue;
            }
            if self.locks.objects.contains(&c.num) {
                self.blocked.insert(c.num);
            } else {
                self.claim(c.num, Op::Fill);
            }
        }
    }

    /// The interactive form dictionary: fields only added, each one signing; `/SigFlags` only
    /// with a signature; `/NeedAppearances` with a fill, or dropped when signing; `/DR` entries
    /// only added. Anything else is unexplained.
    fn form(&mut self) {
        let (doc, old) = (self.doc, self.old);
        let (before, now) = (acroform(old), acroform(doc));
        if same_entry(before.clone().map(Object::Dict).as_ref(), now.clone().map(Object::Dict).as_ref()) {
            return;
        }
        let (b, n) = (before.unwrap_or_default(), now.unwrap_or_default());
        let signing = self.signing();
        let mut ops = Vec::new();
        let mut explained = same_entries(&n, &b, &[b"Fields", b"SigFlags", b"NeedAppearances", b"DR"]);
        if !same_entry(n.get(b"Fields"), b.get(b"Fields")) {
            let old_fields = refs(old, b.get(b"Fields"));
            let added: Vec<ObjRef> = refs(doc, n.get(b"Fields")).into_iter().filter(|r| !old_fields.contains(r)).collect();
            // A field dropped, or the same fields only reordered or respelled.
            explained &= !lost_entries(doc, old, b.get(b"Fields"), n.get(b"Fields")) && !added.is_empty();
            for r in added {
                match [Op::Sign, Op::Timestamp].into_iter().find(|op| self.claimed_by(r.num, *op)) {
                    Some(op) => ops.push(op),
                    None => explained = false,
                }
            }
        }
        if !same_entry(n.get(b"SigFlags"), b.get(b"SigFlags")) {
            // Only signing sets the signature flags: a lone change signs nothing (review of #6771).
            match signing {
                Some(op) => ops.push(op),
                None => explained = false,
            }
        }
        if !same_entry(n.get(b"NeedAppearances"), b.get(b"NeedAppearances")) {
            let dropped = n.get(b"NeedAppearances").is_none_or(|v| matches!(&*doc.resolve(v), Object::Bool(false)));
            ops.push(signing.filter(|_| dropped).unwrap_or(Op::Fill));
        }
        if !same_entry(n.get(b"DR"), b.get(b"DR")) {
            let grew = match (b.get(b"DR"), n.get(b"DR")) {
                (None, Some(_)) => true,
                (Some(was), Some(is)) => only_grew(doc, old, &old.resolve(was), &doc.resolve(is), 0),
                _ => false,
            };
            if grew {
                ops.push(Op::Fill);
            } else {
                explained = false;
            }
        }
        self.form = if explained { FormChange::By(ops) } else { FormChange::Unexplained };
        // The form as an object of its own: the signed one rewritten, or a new one.
        let FormChange::By(ops) = &self.form else { return };
        let ops = ops.clone();
        let held = old.root().and_then(|c| doc.get(c).as_dict().and_then(|c| c.get(b"AcroForm").and_then(Object::as_ref))).map(|r| r.num);
        for num in [self.seen.snapshot.old_form, held].into_iter().flatten() {
            if self.changed.contains_key(&num) {
                for op in &ops {
                    self.claim(num, *op);
                }
            }
        }
    }

    /// A signed page's annotations (the page itself, or its `/Annots` array of its own): each
    /// one added is a signature's widget, or a comment; one removed or restacked is a comment
    /// change. A widget added for no signature, or one moved from another page, is unexplained.
    /// The same annotations listed again restate what was signed.
    fn pages(&mut self) {
        let (doc, old, snapshot) = (self.doc, self.old, &self.seen.snapshot);
        for c in &self.seen.changes {
            if !matches!(c.label, Some(kind::COMMENTS | kind::SIGNATURE) | None) {
                continue;
            }
            let Some(before) = c.before.as_deref() else { continue };
            let (was, now) = if snapshot.signed_pages.contains(&c.num) {
                match (before.as_dict(), c.now.as_dict()) {
                    (Some(b), Some(d)) => (b.get(b"Annots"), d.get(b"Annots")),
                    _ => continue,
                }
            } else if snapshot.signed_annots.contains(&c.num) && before.as_array().is_some() {
                (Some(before), Some(&*c.now))
            } else {
                continue;
            };
            // Read again from the budget the observation spent: a file whose lists used it up
            // there is not explained here either, and the work stays within one budget
            // (`shared_annotation_arrays_are_judged_in_bounded_time`).
            let (Some(was), Some(now)) = (annot_refs(old, was, &snapshot.annot_budget), annot_refs(doc, now, &snapshot.annot_budget)) else {
                continue;
            };
            let live =
                |d: &Document, list: Vec<ObjRef>| -> Vec<ObjRef> { list.into_iter().filter(|r| !matches!(*d.get(*r), Object::Null)).collect() };
            let (was, now) = (live(old, was), live(doc, now));
            let had: HashSet<u32> = was.iter().map(|r| r.num).collect();
            let kept: HashSet<u32> = now.iter().map(|r| r.num).collect();
            let mut ops = Vec::new();
            let restacked = !now.iter().map(|r| r.num).filter(|n| had.contains(n)).eq(was.iter().map(|r| r.num));
            if had.iter().any(|n| !kept.contains(n)) || restacked {
                ops.push(Op::Annotate);
            }
            let mut explained = true;
            for added in now.iter().filter(|r| !had.contains(&r.num)) {
                if let Some(op) = [Op::Sign, Op::Timestamp].into_iter().find(|op| self.claimed_by(added.num, *op)) {
                    ops.push(op);
                } else if !snapshot.placed_annots.contains(&added.num) && doc.get(*added).as_dict().is_some_and(is_comment) {
                    self.claim(added.num, Op::Annotate);
                    ops.push(Op::Annotate);
                } else {
                    explained = false;
                }
            }
            if explained {
                if ops.is_empty() {
                    ops.push(Op::Restated);
                }
                for op in ops {
                    self.claim(c.num, op);
                }
            }
        }
    }

    /// An existing comment changed (its kind unchanged, which the label already checks).
    fn comments(&mut self) {
        let snapshot = &self.seen.snapshot;
        let changed: Vec<u32> = self
            .seen
            .changes
            .iter()
            .filter(|c| c.label == Some(kind::COMMENTS))
            .filter(|c| !snapshot.signed_pages.contains(&c.num) && !snapshot.signed_annots.contains(&c.num))
            .filter(|c| c.before.as_deref().and_then(Object::as_dict).is_some_and(is_comment) && c.now.as_dict().is_some_and(is_comment))
            .map(|c| c.num)
            .collect();
        for num in changed {
            self.claim(num, Op::Annotate);
        }
    }

    /// The document security store's own objects, as `dss_parts` recognises them.
    fn validation_data(&mut self) {
        let parts: Vec<u32> = self.seen.changes.iter().filter(|c| self.seen.dss.contains(&c.num)).map(|c| c.num).collect();
        for num in parts {
            self.claim(num, Op::ValidationData);
        }
    }

    /// The signed `/Info` and XMP rewritten, cross-reference and object streams written, and a
    /// new `/Info` object. A dictionary that plays a role of its own (a signature value, an
    /// annotation, a field) is not document information because the trailer names it: rewriting
    /// a signature value that is also `/Info`, or filling a signed page's dangling `/Annots`
    /// entry with a widget named as `/Info`, is no housekeeping. Nor is the signed catalog's
    /// `/Perms`, which names the certification: rewritten in place it keeps the catalog's
    /// reference, so only [`perms_changed`] (at P=1) would see it otherwise.
    fn housekeeping(&mut self) {
        let (doc, old, snapshot) = (self.doc, self.old, &self.seen.snapshot);
        let info = doc.trailer().get(b"Info").and_then(Object::as_ref).map(|r| r.num);
        let perms = old.root().and_then(|c| old.get(c).as_dict().and_then(|c| c.get(b"Perms").and_then(Object::as_ref))).map(|r| r.num);
        let no_role = |o: &Object| o.as_dict().is_none_or(|d| !has_role(d));
        let mut found = Vec::new();
        for c in &self.seen.changes {
            let op = match (&c.before, &*c.now) {
                (Some(b), _)
                    if c.label == Some(kind::METADATA)
                        && (Some(c.num) == snapshot.old_metadata
                            || (Some(c.num) == snapshot.old_info && Some(c.num) != perms && no_role(b) && no_role(&c.now))) =>
                {
                    Some(Op::Housekeeping)
                }
                (None, Object::Stream(s)) if matches!(s.dict.name(b"Type"), Some(b"XRef" | b"ObjStm")) => Some(Op::Plumbing),
                (None, Object::Dict(d)) if Some(c.num) == info && !has_role(d) => Some(Op::NewInfo),
                _ => None,
            };
            found.extend(op.map(|op| (c.num, op)));
        }
        for (num, op) in found {
            self.claim(num, op);
        }
    }

    /// The signed catalog: each entry that changed must be explained, the form by [`Self::form`],
    /// `/Perms` only by the signature it now names being signed here.
    fn catalog(&mut self) {
        let (doc, old) = (self.doc, self.old);
        let Some(root) = old.root() else { return };
        let Some(c) = self.changed.get(&root.num) else { return };
        if c.label.is_some_and(|l| NEVER.contains(&l)) {
            return;
        }
        let (Some(b), Some(d)) = (c.before.as_deref().and_then(Object::as_dict), c.now.as_dict()) else { return };
        let mut ops = vec![];
        for key in [b"AcroForm".as_slice(), b"DSS", b"Perms", b"Metadata", b"NeedsRendering", b"Extensions"] {
            if same_entry(d.get(key), b.get(key)) {
                continue;
            }
            let op = match key {
                b"AcroForm" => match &self.form {
                    FormChange::Unchanged => continue,
                    FormChange::By(by) => {
                        ops.extend(by.iter().copied());
                        continue;
                    }
                    FormChange::Unexplained => None,
                },
                b"DSS" => Some(Op::ValidationData),
                b"Perms" => {
                    let perms = d.get(b"Perms").map(|p| doc.resolve(p));
                    let named = perms.as_deref().and_then(Object::as_dict).and_then(|p| p.get(b"DocMDP").and_then(Object::as_ref));
                    named.filter(|r| self.claimed_by(r.num, Op::Sign)).map(|_| Op::Sign)
                }
                _ => Some(Op::Housekeeping),
            };
            match op {
                Some(op) => ops.push(op),
                None => return,
            }
        }
        if ops.is_empty() {
            ops.push(Op::Housekeeping);
        }
        for op in ops {
            self.claim(root.num, op);
        }
    }

    /// New objects reached from what a claimed object changed (or from all of a new one) are
    /// claimed by the same operations, unless they play a role of their own; a comment's
    /// pop-up, another comment, is the comment's. A part of the store's structure that does not
    /// have the shape of its role is not validation data however it is reached.
    fn propagate(&mut self) {
        let structure = super::changes::dss_structure(self.doc);
        let mut work: Vec<u32> = self.claims.keys().copied().collect();
        work.sort_unstable();
        while let Some(num) = work.pop() {
            let Some(c) = self.changed.get(&num).copied() else { continue };
            let ops = self.claims.get(&num).cloned().unwrap_or_default();
            let mut found = Vec::new();
            match c.before.as_deref() {
                None => collect_refs(&c.now, 0, &mut found),
                Some(before) => changed_refs(before, &c.now, &mut found),
            }
            for r in found {
                let Some(left) = self.steps.checked_sub(1) else {
                    self.exhausted = true;
                    return;
                };
                self.steps = left;
                let Some(t) = self.changed.get(&r.num).copied() else { continue };
                if t.before.is_some() || self.claims.contains_key(&r.num) || (structure.contains(&r.num) && !self.seen.dss.contains(&r.num)) {
                    continue;
                }
                let own = match t.now.as_dict() {
                    Some(d) if has_role(d) => ops.contains(&Op::Annotate) && is_comment(d),
                    _ => true,
                };
                if own {
                    self.claims.insert(r.num, ops.clone());
                    work.push(r.num);
                }
            }
        }
    }

    fn verdict(self) -> Modification {
        let p = self.p;
        let (mut allowed, mut disallowed): (Vec<&'static str>, Vec<&'static str>) = (Vec::new(), self.seen.hard.clone());
        if self.exhausted {
            note(&mut disallowed, kind::OTHER);
        }
        let mut unexplained = Vec::new();
        for c in &self.seen.changes {
            // A new object can carry one too: a signed page drew from its number (#1787).
            if let Some(label) = c.label.filter(|l| NEVER.contains(l)) {
                note(&mut disallowed, label);
                continue;
            }
            if let Some(ops) = self.claims.get(&c.num) {
                for op in ops {
                    let list = if op.permitted(p) { &mut allowed } else { &mut disallowed };
                    if let Some(word) = op.word() {
                        note(list, word);
                    }
                }
                continue;
            }
            if self.blocked.contains(&c.num) {
                note(&mut disallowed, kind::LOCKED_FIELD);
                continue;
            }
            // What the unclaimed change looks like, said as it was before this recogniser; but a
            // change that only looks like what the signature permits is "other changes".
            let word = match (&c.before, c.label) {
                (Some(_), None) => continue,
                (Some(_), Some(label)) => Some(label),
                (None, _) => c.now.as_dict().and_then(describe),
            };
            match word {
                Some(w) => note(&mut disallowed, if Op::named(w).is_some_and(|op| op.permitted(p)) { kind::OTHER } else { w }),
                None => unexplained.push(c.num),
            }
        }
        for what in &self.seen.freed {
            note(&mut disallowed, what);
        }
        if self.locks.state == Locked::Changed {
            note(&mut disallowed, kind::LOCKED_FIELD);
        }
        if p == Some(1) && perms_changed(self.doc, self.old) {
            note(&mut disallowed, kind::STRUCTURE);
        }
        // A new object of no role of its own that nothing claims: data no operation brought.
        // Said only when nothing else is, as it is reached from a change listed already.
        if !unexplained.is_empty() && disallowed.is_empty() && !unreferenced(self.doc, self.old, &unexplained) {
            disallowed.push(kind::OTHER);
        }
        if disallowed.is_empty() { Modification::Allowed(allowed) } else { Modification::Disallowed(disallowed) }
    }
}

/// Whether nothing refers to any of the objects `nums`, in `doc` or in the signed revision `old`:
/// written but not part of either document, they show nothing (writers leave such objects
/// behind). One the signed revision referred to fills a reference that was signed, and one the
/// document refers to is reached from a change. `false` when the references can't all be read.
fn unreferenced(doc: &Document, old: &Document, nums: &[u32]) -> bool {
    let referred = |d: &Document| super::changes::references(d).map(|found| found.into_iter().map(|r| r.num).collect::<HashSet<u32>>());
    match (referred(doc), referred(old)) {
        (Some(now), Some(then)) => nums.iter().all(|n| !now.contains(n) && !then.contains(n)),
        _ => false,
    }
}

/// An annotation that is neither a widget nor a link: a comment or its pop-up. A link goes
/// somewhere when clicked, which no level permits changing ([`kind::LINKS`]).
fn is_comment(d: &Dict) -> bool {
    d.name(b"Subtype").is_some_and(|s| s != b"Widget" && s != b"Link") && (d.name(b"Type") == Some(b"Annot") || d.contains(b"Rect"))
}

/// Whether a new dictionary plays a role of its own, so it is claimed only by an operation on it:
/// an annotation, a form field, a signature value, a page, the page tree or a catalog.
fn has_role(d: &Dict) -> bool {
    d.contains(b"FT")
        || d.contains(b"T")
        || d.contains(b"Kids")
        || d.contains(b"ByteRange")
        || (d.contains(b"Subtype") && d.contains(b"Rect"))
        || matches!(d.name(b"Type"), Some(b"Annot" | b"Page" | b"Pages" | b"Catalog" | b"Sig" | b"DocTimeStamp"))
}

/// What a new dictionary no operation claims looks like: `None` when it plays no role of its own.
fn describe(d: &Dict) -> Option<&'static str> {
    if d.name(b"Subtype") == Some(b"Link") {
        Some(kind::LINKS)
    } else if d.contains(b"Subtype") && (d.name(b"Type") == Some(b"Annot") || d.contains(b"Rect")) {
        Some(kind::COMMENTS)
    } else if matches!(d.name(b"Type"), Some(b"Sig" | b"DocTimeStamp" | b"SigRef" | b"TransformParams")) || d.name(b"FT") == Some(b"Sig") {
        Some(kind::SIGNATURE)
    } else if matches!(d.name(b"Type"), Some(b"Page" | b"Pages" | b"Catalog")) {
        Some(kind::STRUCTURE)
    } else if has_role(d) {
        Some(kind::FORM_FILL)
    } else {
        None
    }
}

/// Every reference in `o`, through its direct nesting.
fn collect_refs(o: &Object, depth: u8, found: &mut Vec<ObjRef>) {
    if depth > MAX_NESTING {
        return;
    }
    match o {
        Object::Ref(r) => found.push(*r),
        Object::Dict(d) => d.iter().for_each(|(_, v)| collect_refs(v, depth + 1, found)),
        Object::Stream(s) => s.dict.iter().for_each(|(_, v)| collect_refs(v, depth + 1, found)),
        Object::Array(a) => a.iter().for_each(|v| collect_refs(v, depth + 1, found)),
        _ => {}
    }
}

/// The references in what changed from `before` to `now`: entries of a dictionary (or a stream's)
/// that are new or different, elements of an array that it did not hold; everything, when the
/// object changed type.
fn changed_refs(before: &Object, now: &Object, found: &mut Vec<ObjRef>) {
    let dicts = |b: &Dict, n: &Dict, found: &mut Vec<ObjRef>| {
        for (k, v) in n.iter() {
            if !same_entry(Some(v), b.get(k)) {
                collect_refs(v, 1, found);
            }
        }
    };
    match (before, now) {
        (Object::Dict(b), Object::Dict(n)) => dicts(b, n, found),
        (Object::Stream(b), Object::Stream(n)) => dicts(&b.dict, &n.dict, found),
        (Object::Array(b), Object::Array(n)) => {
            let held: HashSet<ObjRef> = b.iter().filter_map(Object::as_ref).collect();
            let compared = b.len() <= MAX_COMPARED && n.len() <= MAX_COMPARED;
            for v in n {
                let had = match v {
                    Object::Ref(r) => held.contains(r),
                    direct => compared && b.iter().any(|w| same_object(direct, w)),
                };
                if !had {
                    collect_refs(v, 1, found);
                }
            }
        }
        _ => collect_refs(now, 0, found),
    }
}
