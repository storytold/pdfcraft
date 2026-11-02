//! Remove a signature (Acrobat: Clear Signature; `parity/` `sign.clear-signature`).
//!
//! Two kinds of signature can be taken out of a document:
//! - **Digital** ones, the value (`/V`) of a signature field (ISO 32000-2 §12.7.5.5, §12.8): the
//!   value goes, and with it either the whole field (its widgets leave their pages) or only the
//!   appearance, leaving an empty field to sign again. The catalog's certification entry
//!   (`/Perms /DocMDP`) goes when it names the removed signature, and `/SigFlags` loses the bits
//!   that no longer hold.
//! - **Visual** ones: a Fill & Sign signature or initials, an annotation PdfCraft recognises
//!   with certainty ([`pdfcraft_annot::is_fill_sign`]). They are deleted like any comment.
//!
//! A signature drawn into a page's content (a scanned page, say) is neither, and is never
//! listed: telling it apart from the rest of the page would mean guessing, and removing it would
//! mean erasing part of the page.
//!
//! Nothing here makes a document look signed, or valid, when it isn't. The other signatures
//! of the document are not touched, so they will report the change; the edit is refused when a
//! remaining certification or field lock does not allow it, rather than worked around.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use pdfcraft_forms::FieldKind;

use crate::SignatureInfo;

/// `/SigFlags` bit 1: the document has signature fields.
const SIGNATURES_EXIST: i64 = 1;
/// `/SigFlags` bit 2: the document has signatures that saving must not invalidate.
const APPEND_ONLY: i64 = 2;
/// Deepest field tree walked (field hierarchies are a handful of levels in practice).
const MAX_DEPTH: usize = 64;

/// What a listed signature is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A digital signature (an approval signature).
    Digital,
    /// A certification signature with its DocMDP level (1: no changes, 2: form filling and
    /// signing, 3: also comments).
    Certification(u8),
    /// A document timestamp: a signature over the whole file, in a field or not.
    DocumentTimestamp,
    /// A Fill & Sign signature (or initials): an annotation, with no cryptography behind it.
    Visual { initials: bool },
}

/// Where a listed signature is in the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A signature field, by its fully qualified name.
    Field(String),
    /// A page's annotation: page (0-based) and its position in the page's `/Annots`.
    Annotation { page: usize, index: usize },
    /// Not removable on its own (a document timestamp).
    None,
}

/// Something the removal will change besides the signature itself, to say before it's done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Other signatures in the document will report that it changed after they signed. Nothing
    /// says they will still be valid: they must be checked again.
    OtherSignatures(Vec<String>),
    /// Of those, the ones signed after this one: their signed bytes include it.
    SignedAfter(Vec<String>),
    /// The document stops being certified.
    EndsCertification(u8),
    /// The fields this signature locked (FieldMDP) can be changed again.
    Unlocks(Vec<String>),
    /// Other signatures remain, so the file is saved incrementally: the removed signature is
    /// still inside it, in the earlier revision, and can be recovered from there.
    EarlierRevisionKept,
    /// The document is a certified one and this change is not one its certification permits;
    /// the certification will report it.
    AgainstCertification(String),
}

/// Why a listed signature can't be removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blocker {
    /// A document timestamp, in a field or not: it covers the whole file and is no one's signature.
    DocumentTimestamp,
    /// Another signature certifies the document, and removing a signature is not a change any
    /// certification level permits.
    CertifiedBy(String),
    /// Another signature locks this field (FieldMDP).
    LockedBy(String),
    /// The document can't be changed (read-only, or its security doesn't allow it).
    ReadOnly(String),
}

impl Effect {
    /// The sentence that says it, in English with a `{names}` hole, and what fills the hole. An
    /// interface in another language translates the wording and fills it the same way.
    pub fn wording(&self) -> (&'static str, String) {
        match self {
            Effect::OtherSignatures(n) => (
                "The other signatures ({names}) will report that the document changed after they signed. Check them again: nothing says they stay valid.",
                n.join(", "),
            ),
            Effect::SignedAfter(n) => ("Signed after this signature, so what they signed includes it: {names}.", n.join(", ")),
            Effect::EndsCertification(_) => ("The document will no longer be certified.", String::new()),
            Effect::Unlocks(n) => ("The fields this signature locked ({names}) can be changed again.", n.join(", ")),
            Effect::EarlierRevisionKept => (
                "Other signatures remain, so the copy is saved as an update of the signed file: the removed signature stays in its earlier revision, where it can be recovered.",
                String::new(),
            ),
            Effect::AgainstCertification(n) => ("{names} certifies this document without allowing comments, and will report this change.", n.clone()),
        }
    }
}

impl Blocker {
    /// As [`Effect::wording`].
    pub fn wording(&self) -> (&'static str, String) {
        match self {
            Blocker::DocumentTimestamp => {
                ("A document timestamp covers the whole file and is no one's signature, so it can't be removed on its own.", String::new())
            }
            Blocker::CertifiedBy(n) => {
                ("{names} certifies this document, and removing a signature is not a change its certification permits.", n.clone())
            }
            Blocker::LockedBy(n) => ("The signature {names} locks this field.", n.clone()),
            Blocker::ReadOnly(why) => ("This document can't be changed: {names}", why.clone()),
        }
    }
}

/// Fill a wording's `{names}` hole, for English output (tools, errors).
pub fn said((wording, names): (&'static str, String)) -> String {
    wording.replace("{names}", &names)
}

/// One signature that can be listed for removal, with what removing it would do.
#[derive(Clone, Debug, PartialEq)]
pub struct Removable {
    pub kind: Kind,
    pub target: Target,
    /// The field, for digital signatures.
    pub field: Option<String>,
    /// 0-based page of its appearance, if it has one on a page.
    pub page: Option<usize>,
    /// Who signed: the certificate's common name or the signature's `/Name`, as validation read
    /// it. Never guessed.
    pub signer: Option<String>,
    /// The signing time as written (`/M`), for digital signatures.
    pub date: Option<String>,
    /// The verdict validation gave, for digital signatures. The existence of a signature says
    /// nothing about its validity.
    pub status: Option<crate::SignatureStatus>,
    /// 1-based revision the signature covers (0 for visual signatures).
    pub revision: usize,
    pub effects: Vec<Effect>,
    pub blocked: Option<Blocker>,
}

/// What a removal took out, to report after the save.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cleared {
    pub field: String,
    /// The field (and its widgets) left the form, not just its value.
    pub removed_field: bool,
    /// Widgets taken off their pages, or emptied of their appearance when the field stays.
    pub widgets: usize,
    /// Empty parent fields removed with it.
    pub parents: usize,
    /// The catalog's certification entry named this signature and was removed.
    pub certification: bool,
    /// `/SigFlags` was updated.
    pub sig_flags: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ClearError {
    #[error("there is no signature field named {0:?}")]
    NoSuchField(String),
    #[error("{0:?} is not signed")]
    NotSigned(String),
    #[error("{0:?} is a document timestamp, which can't be removed on its own")]
    DocumentTimestamp(String),
    #[error("{name:?} can't be removed: the signature {by:?} certifies this document, and removing a signature is not a change it permits")]
    CertifiedBy { name: String, by: String },
    #[error("{name:?} can't be removed: the signature {by:?} locks this field")]
    LockedBy { name: String, by: String },
    #[error("removing {name:?} would leave the document inconsistent ({problems}), so nothing was changed")]
    Inconsistent { name: String, problems: String },
    #[error("{0}")]
    Form(String),
}

/// A signed signature field as the structure shows it (no validation needed).
struct SignedField {
    name: String,
    obj: ObjRef,
    /// The signature dictionary's reference, when it is indirect (it nearly always is).
    value: Option<ObjRef>,
    value_dict: Dict,
    /// The certification level when this is a certification signature.
    certify: Option<u8>,
    /// What this signature locks once signed (FieldMDP).
    lock: Option<Lock>,
}

/// FieldMDP: the fields a signature locks (ISO 32000-2 Table 236).
#[derive(Clone, Debug)]
enum Lock {
    All,
    Include(Vec<String>),
    Exclude(Vec<String>),
}

impl Lock {
    fn covers(&self, field: &str) -> bool {
        // A listed name covers its descendants too (`a` locks `a.b`).
        let listed = |names: &[String]| names.iter().any(|n| n == field || field.strip_prefix(n.as_str()).is_some_and(|r| r.starts_with('.')));
        match self {
            Lock::All => true,
            Lock::Include(n) => listed(n),
            Lock::Exclude(n) => !listed(n),
        }
    }

    fn read(doc: &Document, d: &Dict) -> Option<Lock> {
        let action = d.name(b"Action")?;
        let names: Vec<String> = d
            .get(b"Fields")
            .map(|f| doc.resolve(f))
            .and_then(|f| f.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|o| match &*doc.resolve(o) {
                Object::String(s) => Some(s.to_text()),
                _ => None,
            })
            .collect();
        match action {
            b"All" => Some(Lock::All),
            b"Include" => Some(Lock::Include(names)),
            b"Exclude" => Some(Lock::Exclude(names)),
            _ => None,
        }
    }
}

/// A document timestamp's dictionary (ISO 32000-2 §12.8.5): `/SubFilter /ETSI.RFC3161`, typed
/// `/DocTimeStamp` or, as older writers have it, `/Sig`.
fn is_doc_timestamp(d: &Dict) -> bool {
    d.name(b"SubFilter") == Some(b"ETSI.RFC3161") && matches!(d.name(b"Type"), Some(b"DocTimeStamp" | b"Sig"))
}

fn catalog_doc_mdp(doc: &Document) -> Option<ObjRef> {
    let root = doc.root()?;
    let perms = doc.get(root).as_dict()?.get(b"Perms").map(|p| doc.resolve(p))?;
    perms.as_dict()?.reference(b"DocMDP")
}

/// The transform of `method` in a signature dictionary's `/Reference` array: its parameters.
fn transform_params(doc: &Document, sig: &Dict, method: &[u8]) -> Option<Dict> {
    let refs = sig.get(b"Reference").map(|r| doc.resolve(r))?;
    refs.as_array()?.iter().find_map(|r| {
        let r = doc.resolve(r);
        let d = r.as_dict()?;
        if d.name(b"TransformMethod") != Some(method) {
            return None;
        }
        Some(d.get(b"TransformParams").map(|p| doc.resolve(p)).and_then(|p| p.as_dict().cloned()).unwrap_or_default())
    })
}

fn signed_fields(doc: &Document) -> Vec<SignedField> {
    let doc_mdp = catalog_doc_mdp(doc);
    pdfcraft_forms::fields(doc)
        .into_iter()
        .filter(|f| f.kind == FieldKind::Signature)
        .filter_map(|f| {
            let field = doc.get(f.obj);
            let fd = field.as_dict()?;
            let v = fd.get(b"V")?;
            let value = v.as_ref();
            let value_dict = doc.resolve(v).as_dict().cloned()?;
            let mdp = transform_params(doc, &value_dict, b"DocMDP");
            let certify = match (&mdp, value.is_some() && value == doc_mdp) {
                (Some(p), _) => Some(p.int(b"P").unwrap_or(2).clamp(1, 3) as u8),
                (None, true) => Some(2),
                (None, false) => None,
            };
            let lock = transform_params(doc, &value_dict, b"FieldMDP")
                .and_then(|p| Lock::read(doc, &p))
                .or_else(|| fd.get(b"Lock").map(|l| doc.resolve(l)).and_then(|l| l.as_dict().and_then(|l| Lock::read(doc, l))));
            Some(SignedField { name: f.name, obj: f.obj, value, value_dict, certify, lock })
        })
        .collect()
}

/// The fields a lock covers, among the document's.
fn locked_names(doc: &Document, lock: &Lock, except: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    pdfcraft_forms::fields(doc).into_iter().map(|f| f.name).filter(|n| n != except && lock.covers(n) && seen.insert(n.clone())).collect()
}

/// The signatures that can be listed for removal, digital ones first in revision order, then
/// visual ones in page order. `signatures` is the document's validation ([`crate::Document::signatures`]).
pub fn removable(doc: &Document, signatures: &[SignatureInfo]) -> Vec<Removable> {
    let fields = signed_fields(doc);
    let mut out = Vec::new();
    let mut digital: Vec<&SignatureInfo> = signatures.iter().filter(|s| s.signed).collect();
    digital.sort_by_key(|s| s.revision);
    // Every signature keeps a saved copy incremental, document timestamps included.
    let signed_names: Vec<String> = digital.iter().map(|s| s.field.clone()).collect();
    for s in &digital {
        let field = fields.iter().find(|f| f.name == s.field);
        let others: Vec<String> = digital.iter().filter(|o| o.field != s.field).map(|o| o.field.clone()).collect();
        let (kind, target, blocked) = match field {
            None => (Kind::DocumentTimestamp, Target::None, Some(Blocker::DocumentTimestamp)),
            Some(f) => {
                let kind = match f.certify {
                    Some(p) => Kind::Certification(p),
                    None if s.doc_timestamp => Kind::DocumentTimestamp,
                    None => Kind::Digital,
                };
                // A document timestamp in a field (as ISO 32000-2 stores it) is never removable on
                // its own either.
                let blocked =
                    if kind == Kind::DocumentTimestamp { Some(Blocker::DocumentTimestamp) } else { blocker(&fields, &f.name).map(|(b, _)| b) };
                (kind, Target::Field(f.name.clone()), blocked)
            }
        };
        let mut effects = Vec::new();
        if !others.is_empty() {
            effects.push(Effect::OtherSignatures(others.clone()));
            let after: Vec<String> = digital.iter().filter(|o| o.field != s.field && o.revision > s.revision).map(|o| o.field.clone()).collect();
            if !after.is_empty() {
                effects.push(Effect::SignedAfter(after));
            }
        }
        if let Kind::Certification(p) = kind {
            effects.push(Effect::EndsCertification(p));
        }
        if let Some(lock) = field.and_then(|f| f.lock.as_ref()) {
            let names = locked_names(doc, lock, &s.field);
            if !names.is_empty() {
                effects.push(Effect::Unlocks(names));
            }
        }
        if signed_names.iter().any(|n| n != &s.field) {
            effects.push(Effect::EarlierRevisionKept);
        }
        out.push(Removable {
            kind,
            target,
            field: Some(s.field.clone()),
            page: s.page,
            signer: s.signer.clone(),
            date: s.date.clone(),
            status: Some(s.status),
            revision: s.revision,
            effects,
            blocked,
        });
    }
    let certified_by = fields.iter().find(|f| f.certify.is_some_and(|p| p < 3)).map(|f| f.name.clone());
    for (page, index, initials) in visual_signatures(doc) {
        let mut effects = Vec::new();
        if !signed_names.is_empty() {
            effects.push(Effect::OtherSignatures(signed_names.clone()));
        }
        if let Some(by) = &certified_by {
            effects.push(Effect::AgainstCertification(by.clone()));
        }
        out.push(Removable {
            kind: Kind::Visual { initials },
            target: Target::Annotation { page, index },
            field: None,
            page: Some(page),
            signer: None,
            date: None,
            status: None,
            revision: 0,
            effects,
            blocked: None,
        });
    }
    out
}

/// Fill & Sign signatures and initials: (page, index in its `/Annots`, initials).
fn visual_signatures(doc: &Document) -> Vec<(usize, usize, bool)> {
    let mut out = Vec::new();
    for (page, p) in pdfcraft_annot::page_refs(doc).unwrap_or_default().into_iter().enumerate() {
        let list =
            doc.get(p).as_dict().and_then(|d| d.get(b"Annots").map(|a| doc.resolve(a))).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        for (index, entry) in list.iter().enumerate() {
            let obj = doc.resolve(entry);
            let Some(d) = obj.as_dict() else { continue };
            if !pdfcraft_annot::is_fill_sign(doc, d) {
                continue;
            }
            let subject = |k: &[u8]| match d.get(k).map(|o| doc.resolve(o)).as_deref() {
                Some(Object::String(s)) => s.to_text(),
                _ => String::new(),
            };
            // Only the signature kinds: typed text, checks, crosses, dots and lines are not
            // signatures, even when they sit on a signature line.
            let initials = match (d.name(b"Subtype"), d.name(b"Name")) {
                (Some(b"Stamp"), Some(b"PCCustomInitials")) => true,
                (Some(b"Stamp"), Some(b"PCCustomSignature")) => false,
                (Some(b"Stamp"), Some(b"PCTypedSignature")) => subject(b"Contents") == "Initials",
                (Some(b"Ink"), _) => subject(b"Contents") == "Initials",
                _ => continue,
            };
            out.push((page, index, initials));
        }
    }
    out
}

/// Why `name` can't be removed while the other signatures stay: a certification or a lock.
fn blocker(fields: &[SignedField], name: &str) -> Option<(Blocker, ClearError)> {
    for f in fields.iter().filter(|f| f.name != name) {
        if f.certify.is_some() {
            return Some((Blocker::CertifiedBy(f.name.clone()), ClearError::CertifiedBy { name: name.into(), by: f.name.clone() }));
        }
        if f.lock.as_ref().is_some_and(|l| l.covers(name)) {
            return Some((Blocker::LockedBy(f.name.clone()), ClearError::LockedBy { name: name.into(), by: f.name.clone() }));
        }
    }
    None
}

/// Remove the signature in field `name`: the whole field when `remove_field`, otherwise only
/// its value and appearance, leaving an empty signature field. Refused, with the document
/// unchanged, when another signature's certification or lock forbids it, or when the result
/// would have references it didn't have before.
pub fn clear(doc: &mut Document, name: &str, remove_field: bool) -> Result<Cleared, ClearError> {
    // A failure part way (a parent that can't be updated, say) must not leave a half-removed
    // field: work on a copy (clones share all unchanged data) and keep it only on success.
    let mut work = doc.clone();
    let done = clear_in(&mut work, name, remove_field)?;
    *doc = work;
    Ok(done)
}

fn clear_in(doc: &mut Document, name: &str, remove_field: bool) -> Result<Cleared, ClearError> {
    let fields = signed_fields(doc);
    let target = match fields.iter().find(|f| f.name == name) {
        Some(f) => f,
        None if pdfcraft_forms::fields(doc).iter().any(|f| f.name == name && f.kind == FieldKind::Signature) => {
            return Err(ClearError::NotSigned(name.into()));
        }
        None => return Err(ClearError::NoSuchField(name.into())),
    };
    if is_doc_timestamp(&target.value_dict) {
        return Err(ClearError::DocumentTimestamp(name.into()));
    }
    if let Some((_, e)) = blocker(&fields, name) {
        return Err(e);
    }
    let (obj, value, value_dict) = (target.obj, target.value, target.value_dict.clone());
    let widgets: Vec<ObjRef> =
        pdfcraft_forms::fields(doc).into_iter().find(|f| f.name == name).map(|f| f.widgets.iter().map(|w| w.obj).collect()).unwrap_or_default();
    let before = problems(doc);
    let mut done = Cleared { field: name.into(), removed_field: remove_field, widgets: widgets.len(), ..Cleared::default() };
    if remove_field {
        let parent = doc.get(obj).as_dict().and_then(|d| d.reference(b"Parent"));
        pdfcraft_forms::delete_field(doc, name).map_err(|e| ClearError::Form(e.to_string()))?;
        done.parents = prune_empty_parents(doc, parent)?;
    } else {
        // The field stays, empty and ready to sign: no value, and no appearance showing the
        // old signature. Its own /Lock and seed values (/SV) are the field's, and stay.
        update(doc, obj, |d| {
            d.remove(b"V");
        })?;
        for w in &widgets {
            update(doc, *w, |d| {
                d.remove(b"AP");
                d.remove(b"AS");
            })?;
        }
    }
    done.certification = drop_certification(doc, value, &value_dict)?;
    done.sig_flags = update_sig_flags(doc)?;
    let after = problems(doc);
    let new: Vec<String> = after.into_iter().filter(|p| !before.contains(p)).collect();
    if !new.is_empty() {
        return Err(ClearError::Inconsistent { name: name.into(), problems: new.join("; ") });
    }
    Ok(done)
}

fn update(doc: &mut Document, r: ObjRef, f: impl FnOnce(&mut Dict)) -> Result<(), ClearError> {
    doc.update_dict(r, f).map_err(|e| ClearError::Form(e.to_string()))
}

/// After a field leaves its parent, parents left without kids leave theirs, up the tree.
fn prune_empty_parents(doc: &mut Document, mut node: Option<ObjRef>) -> Result<usize, ClearError> {
    let mut removed = 0;
    let mut seen = HashSet::new();
    while let Some(n) = node {
        if !seen.insert(n) || seen.len() > MAX_DEPTH {
            break;
        }
        let d = doc.get(n).as_dict().cloned().unwrap_or_default();
        let kids = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
        // A node that still has kids, or is itself a widget, stays.
        if !kids.is_empty() || d.name(b"Subtype") == Some(b"Widget") {
            break;
        }
        let up = d.reference(b"Parent");
        match up {
            Some(p) => detach(doc, p, b"Kids", n)?,
            None => detach_from_form(doc, n)?,
        }
        removed += 1;
        node = up;
    }
    Ok(removed)
}

/// Take `r` out of the array `key` of `holder` (written into the array object when it is
/// indirect, so other holders of it see the change).
fn detach(doc: &mut Document, holder: ObjRef, key: &[u8], r: ObjRef) -> Result<(), ClearError> {
    let entry = doc.get(holder).as_dict().and_then(|d| d.get(key).cloned());
    let list = entry.as_ref().map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
    let kept: Vec<Object> = list.into_iter().filter(|o| o.as_ref() != Some(r)).collect();
    match entry.as_ref().and_then(Object::as_ref) {
        Some(a) if doc.get(a).as_array().is_some() => {
            doc.set(a, Object::Array(kept));
            Ok(())
        }
        _ => update(doc, holder, |d| d.set(key.to_vec(), Object::Array(kept))),
    }
}

fn detach_from_form(doc: &mut Document, r: ObjRef) -> Result<(), ClearError> {
    let Some(root) = doc.root() else { return Ok(()) };
    match doc.get(root).as_dict().and_then(|d| d.get(b"AcroForm").cloned()) {
        Some(Object::Ref(af)) => detach(doc, af, b"Fields", r),
        Some(Object::Dict(mut af)) => {
            let list = af.get(b"Fields").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
            af.set(b"Fields".to_vec(), Object::Array(list.into_iter().filter(|o| o.as_ref() != Some(r)).collect()));
            update(doc, root, |d| d.set(b"AcroForm".to_vec(), Object::Dict(af)))
        }
        _ => Ok(()),
    }
}

/// Remove the catalog's `/Perms /DocMDP` when it names the removed signature: the document is no
/// longer certified by it. Other permissions (`/UR3`) stay.
fn drop_certification(doc: &mut Document, value: Option<ObjRef>, value_dict: &Dict) -> Result<bool, ClearError> {
    let Some(root) = doc.root() else { return Ok(false) };
    let Some(perms_entry) = doc.get(root).as_dict().and_then(|d| d.get(b"Perms").cloned()) else { return Ok(false) };
    let Some(mut perms) = doc.resolve(&perms_entry).as_dict().cloned() else { return Ok(false) };
    let names_it = match perms.get(b"DocMDP") {
        Some(Object::Ref(r)) => Some(*r) == value,
        Some(Object::Dict(d)) => value.is_none() && d == value_dict,
        _ => false,
    };
    if !names_it {
        return Ok(false);
    }
    perms.remove(b"DocMDP");
    match (perms_entry.as_ref(), perms.is_empty()) {
        (_, true) => update(doc, root, |d| {
            d.remove(b"Perms");
        })?,
        (Some(r), false) => doc.set(r, Object::Dict(perms)),
        (None, false) => update(doc, root, |d| d.set(b"Perms".to_vec(), Object::Dict(perms)))?,
    }
    Ok(true)
}

/// Clear the `/SigFlags` bits that no longer hold: AppendOnly once nothing is signed (a
/// standalone document timestamp, outside any field, still counts), and SignaturesExist once
/// there is no signature field. Returns whether it changed.
fn update_sig_flags(doc: &mut Document) -> Result<bool, ClearError> {
    let Some(root) = doc.root() else { return Ok(false) };
    let entry = doc.get(root).as_dict().and_then(|d| d.get(b"AcroForm").cloned());
    let Some(af) = entry.as_ref().map(|e| doc.resolve(e)).and_then(|a| a.as_dict().cloned()) else { return Ok(false) };
    let Some(flags) = af.get(b"SigFlags").and_then(|f| doc.resolve(f).as_int()) else { return Ok(false) };
    let any_field = pdfcraft_forms::fields(doc).iter().any(|f| f.kind == FieldKind::Signature);
    let any_signed = !signed_fields(doc).is_empty() || has_standalone_timestamp(doc);
    let mut next = flags;
    if !any_signed {
        next &= !APPEND_ONLY;
    }
    if !any_field {
        next &= !SIGNATURES_EXIST;
    }
    if next == flags {
        return Ok(false);
    }
    let set = |d: &mut Dict| {
        if next == 0 {
            d.remove(b"SigFlags");
        } else {
            d.set(b"SigFlags".to_vec(), Object::Int(next));
        }
    };
    match entry.as_ref().and_then(Object::as_ref) {
        Some(r) => update(doc, r, set)?,
        None => {
            let mut af = af;
            set(&mut af);
            update(doc, root, |d| d.set(b"AcroForm".to_vec(), Object::Dict(af)))?;
        }
    }
    Ok(true)
}

/// Whether the document holds a document timestamp outside any field (ISO 32000-2 §12.8.2.2),
/// which [`signed_fields`] doesn't see. Scans every object, so it runs only once no signed
/// field is left.
fn has_standalone_timestamp(doc: &Document) -> bool {
    doc.scan_objects().any(|(_, o)| o.as_dict().is_some_and(|d| is_doc_timestamp(d) && d.contains(b"ByteRange")))
}

/// Broken references in the parts of the document a removal touches: page annotations, the
/// field tree and the certification entry. Compared before and after, so a file that was
/// already damaged is not blamed on the removal.
pub fn problems(doc: &Document) -> Vec<String> {
    let mut out = Vec::new();
    for (i, p) in pdfcraft_annot::page_refs(doc).unwrap_or_default().into_iter().enumerate() {
        let list =
            doc.get(p).as_dict().and_then(|d| d.get(b"Annots").map(|a| doc.resolve(a))).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        for r in list.iter().filter_map(Object::as_ref) {
            if doc.get(r).as_dict().is_none() {
                out.push(format!("page {} lists a missing annotation {} {} R", i + 1, r.num, r.generation));
            }
        }
    }
    if let Some(root) = doc.root() {
        let af = doc.get(root).as_dict().and_then(|d| d.get(b"AcroForm").map(|a| doc.resolve(a))).and_then(|a| a.as_dict().cloned());
        let fields = af.and_then(|af| af.get(b"Fields").map(|f| doc.resolve(f))).and_then(|f| f.as_array().cloned()).unwrap_or_default();
        let mut stack: Vec<(Object, usize)> = fields.into_iter().map(|f| (f, 0)).collect();
        let mut seen = HashSet::new();
        while let Some((o, depth)) = stack.pop() {
            let Some(r) = o.as_ref() else { continue };
            if !seen.insert(r) || depth > MAX_DEPTH {
                continue;
            }
            let obj = doc.get(r);
            let Some(d) = obj.as_dict() else {
                out.push(format!("the form lists a missing field {} {} R", r.num, r.generation));
                continue;
            };
            let kids = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
            stack.extend(kids.into_iter().map(|k| (k, depth + 1)));
        }
        if let Some(r) = catalog_doc_mdp(doc)
            && doc.get(r).as_dict().is_none()
        {
            out.push("the certification entry names a missing signature".into());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    //! Structures the signer doesn't write (field locks, hierarchies, shared arrays, damaged
    //! files), built by hand. The `/V` dictionaries hold no cryptography: only the structure is
    //! under test here; `tests/it/clear_signature.rs` covers real signatures.

    use pdfcraft_cos::PdfString;

    use super::*;

    fn dict(entries: Vec<(&[u8], Object)>) -> Dict {
        let mut d = Dict::new();
        for (k, v) in entries {
            d.set(k.to_vec(), v);
        }
        d
    }

    fn arr(refs: &[ObjRef]) -> Object {
        Object::Array(refs.iter().map(|r| Object::Ref(*r)).collect())
    }

    /// One page and an empty form; returns (document, page, form).
    fn base() -> (Document, ObjRef, ObjRef) {
        let mut doc = Document::new_empty();
        let root = doc.root().unwrap();
        let pages = doc.get(root).as_dict().unwrap().reference(b"Pages").unwrap();
        let page = doc.add(dict(vec![
            (b"Type", Object::name("Page")),
            (b"Parent", Object::Ref(pages)),
            (b"MediaBox", Object::Array(vec![0.into(), 0.into(), 300.into(), 400.into()])),
        ]));
        doc.update_dict(pages, |d| {
            d.set(b"Kids".to_vec(), arr(&[page]));
            d.set(b"Count".to_vec(), Object::Int(1));
        })
        .unwrap();
        let form = doc.add(dict(vec![(b"Fields", Object::Array(Vec::new())), (b"SigFlags", Object::Int(3))]));
        doc.update_dict(root, |d| d.set(b"AcroForm".to_vec(), Object::Ref(form))).unwrap();
        (doc, page, form)
    }

    /// A signed signature field that is its own widget, on `page`, at the top of the form.
    fn signed(doc: &mut Document, page: ObjRef, form: ObjRef, name: &str, extra: Vec<(&[u8], Object)>) -> (ObjRef, ObjRef) {
        let v = doc.add(dict(vec![
            (b"Type", Object::name("Sig")),
            (b"Filter", Object::name("Adobe.PPKLite")),
            (b"ByteRange", Object::Array(vec![0.into(), 0.into(), 0.into(), 0.into()])),
            (b"Contents", PdfString::literal(vec![0u8; 8]).into()),
        ]));
        let mut entries = vec![
            (b"FT".as_slice(), Object::name("Sig")),
            (b"T", PdfString::text(name).into()),
            (b"V", Object::Ref(v)),
            (b"Type", Object::name("Annot")),
            (b"Subtype", Object::name("Widget")),
            (b"Rect", Object::Array(vec![10.into(), 10.into(), 110.into(), 40.into()])),
            (b"P", Object::Ref(page)),
        ];
        entries.extend(extra);
        let f = doc.add(dict(entries));
        add_to(doc, form, b"Fields", f);
        add_to(doc, page, b"Annots", f);
        (f, v)
    }

    fn add_to(doc: &mut Document, holder: ObjRef, key: &[u8], r: ObjRef) {
        let mut list = doc.get(holder).as_dict().unwrap().get(key).map(|o| doc.resolve(o).as_array().cloned().unwrap()).unwrap_or_default();
        list.push(Object::Ref(r));
        match doc.get(holder).as_dict().unwrap().reference(key) {
            Some(a) => doc.set(a, Object::Array(list)),
            None => doc.update_dict(holder, |d| d.set(key.to_vec(), Object::Array(list))).unwrap(),
        }
    }

    fn names(doc: &Document) -> Vec<String> {
        pdfcraft_forms::fields(doc).into_iter().map(|f| f.name).collect()
    }

    fn annots(doc: &Document, page: ObjRef) -> Vec<Object> {
        doc.get(page).as_dict().unwrap().get(b"Annots").map(|a| doc.resolve(a).as_array().cloned().unwrap()).unwrap_or_default()
    }

    #[test]
    fn a_field_lock_of_another_signature_refuses_the_removal_and_changes_nothing() {
        let (mut doc, page, form) = base();
        let (buyer, _) = signed(&mut doc, page, form, "Buyer", vec![]);
        let lock = dict(vec![(b"Action", Object::name("Include")), (b"Fields", Object::Array(vec![PdfString::text("Buyer").into()]))]);
        signed(&mut doc, page, form, "Seller", vec![(b"Lock", Object::Dict(lock))]);
        let before = doc.clone();
        assert_eq!(clear(&mut doc, "Buyer", true), Err(ClearError::LockedBy { name: "Buyer".into(), by: "Seller".into() }));
        assert_eq!(names(&doc), names(&before));
        assert_eq!((doc.get(buyer), annots(&doc, page)), (before.get(buyer), annots(&before, page)), "a refused removal changes nothing");
        assert_eq!(doc.get(form), before.get(form));
        // The lock is the locking signature's: removing that one is allowed, and unlocks Buyer.
        let list = removable(&doc, &[]);
        assert!(list.is_empty(), "nothing validated, nothing listed");
        assert_eq!(clear(&mut doc, "Seller", true).map(|c| c.removed_field), Ok(true));
        assert_eq!(names(&doc), ["Buyer"]);
    }

    #[test]
    fn a_document_timestamp_in_a_field_is_refused() {
        let (mut doc, page, form) = base();
        let (_, v) = signed(&mut doc, page, form, "Timestamp1", vec![]);
        doc.update_dict(v, |d| {
            d.set(b"Type".to_vec(), Object::name("DocTimeStamp"));
            d.set(b"SubFilter".to_vec(), Object::name("ETSI.RFC3161"));
        })
        .unwrap();
        let before = doc.clone();
        assert_eq!(clear(&mut doc, "Timestamp1", true), Err(ClearError::DocumentTimestamp("Timestamp1".into())));
        assert_eq!((doc.get(form), annots(&doc, page)), (before.get(form), annots(&before, page)));
        // Listed as a timestamp and blocked, as validation reports it.
        let info = pdfcraft_sign::pdf::list(&doc, &[], &pdfcraft_sign::pdf::TrustStore::default());
        let list = removable(&doc, &info);
        assert_eq!(list.len(), 1, "{list:?}");
        assert_eq!((&list[0].kind, &list[0].blocked), (&Kind::DocumentTimestamp, &Some(Blocker::DocumentTimestamp)));
    }

    #[test]
    fn a_standalone_document_timestamp_keeps_append_only_after_the_last_field_goes() {
        let sig_flags = |doc: &Document, form: ObjRef| doc.get(form).as_dict().unwrap().get(b"SigFlags").and_then(Object::as_int);
        // Without a timestamp, removing the only signature clears both bits.
        let (mut doc, page, form) = base();
        signed(&mut doc, page, form, "Buyer", vec![]);
        assert_eq!(clear(&mut doc, "Buyer", true).map(|c| c.sig_flags), Ok(true));
        assert_eq!(sig_flags(&doc, form), None);
        // A document timestamp in no field still covers the file: AppendOnly stays.
        let (mut doc, page, form) = base();
        signed(&mut doc, page, form, "Buyer", vec![]);
        doc.add(dict(vec![
            (b"Type", Object::name("DocTimeStamp")),
            (b"SubFilter", Object::name("ETSI.RFC3161")),
            (b"ByteRange", Object::Array(vec![0.into(), 0.into(), 0.into(), 0.into()])),
            (b"Contents", PdfString::literal(vec![0u8; 8]).into()),
        ]));
        assert_eq!(clear(&mut doc, "Buyer", true).map(|c| c.sig_flags), Ok(true));
        assert_eq!(sig_flags(&doc, form), Some(APPEND_ONLY));
    }

    #[test]
    fn an_exclude_lock_covers_every_other_field_and_its_children() {
        let lock = Lock::Exclude(vec!["notes".into()]);
        assert!(lock.covers("Buyer") && lock.covers("parties.buyer") && !lock.covers("notes") && !lock.covers("notes.page2"));
        let include = Lock::Include(vec!["parties".into()]);
        assert!(include.covers("parties.buyer") && !include.covers("partiesX") && !include.covers("other"));
    }

    #[test]
    fn a_hierarchical_field_takes_its_emptied_parents_and_leaves_its_siblings() {
        let (mut doc, page, form) = base();
        // parties ─ buyer (signed widget)
        //         └ seller ─ sig (signed widget)
        //                  └ text (a text field)
        let v = doc.add(dict(vec![(b"Type", Object::name("Sig")), (b"Contents", PdfString::literal(vec![0u8; 4]).into())]));
        let parties = doc.add(dict(vec![(b"T", PdfString::text("parties").into())]));
        let seller = doc.add(dict(vec![(b"T", PdfString::text("seller").into()), (b"Parent", Object::Ref(parties))]));
        let widget = |doc: &mut Document, name: &str, parent: ObjRef, extra: Vec<(&[u8], Object)>| {
            let mut e = vec![
                (b"T".as_slice(), PdfString::text(name).into()),
                (b"Parent", Object::Ref(parent)),
                (b"Type", Object::name("Annot")),
                (b"Subtype", Object::name("Widget")),
                (b"Rect", Object::Array(vec![10.into(), 10.into(), 110.into(), 40.into()])),
                (b"P", Object::Ref(page)),
            ];
            e.extend(extra);
            let r = doc.add(dict(e));
            add_to(doc, page, b"Annots", r);
            r
        };
        let buyer = widget(&mut doc, "buyer", parties, vec![(b"FT", Object::name("Sig")), (b"V", Object::Ref(v))]);
        let sig = widget(&mut doc, "sig", seller, vec![(b"FT", Object::name("Sig")), (b"V", Object::Ref(v))]);
        widget(&mut doc, "text", seller, vec![(b"FT", Object::name("Tx")), (b"V", PdfString::text("kept").into())]);
        let kids = annots(&doc, page).into_iter().filter_map(|o| o.as_ref()).collect::<Vec<_>>();
        doc.update_dict(parties, |d| d.set(b"Kids".to_vec(), arr(&[buyer, seller]))).unwrap();
        doc.update_dict(seller, |d| d.set(b"Kids".to_vec(), arr(&[sig, kids[2]]))).unwrap();
        doc.update_dict(form, |d| d.set(b"Fields".to_vec(), arr(&[parties]))).unwrap();
        assert_eq!(names(&doc), ["parties.buyer", "parties.seller.sig", "parties.seller.text"]);

        let done = clear(&mut doc, "parties.seller.sig", true).unwrap();
        assert_eq!((done.widgets, done.parents), (1, 0), "seller still has its text field");
        assert_eq!(names(&doc), ["parties.buyer", "parties.seller.text"]);
        let done = clear(&mut doc, "parties.buyer", true).unwrap();
        assert_eq!(done.parents, 0);
        assert_eq!(names(&doc), ["parties.seller.text"]);
        assert_eq!(annots(&doc, page).len(), 1, "only the text field's widget is left on the page");
        assert!(problems(&doc).is_empty());
    }

    #[test]
    fn the_last_child_takes_its_empty_parents_out_of_the_form() {
        let (mut doc, page, form) = base();
        let v = doc.add(dict(vec![(b"Type", Object::name("Sig"))]));
        let top = doc.add(dict(vec![(b"T", PdfString::text("a").into())]));
        let mid = doc.add(dict(vec![(b"T", PdfString::text("b").into()), (b"Parent", Object::Ref(top))]));
        let leaf = doc.add(dict(vec![
            (b"T", PdfString::text("c").into()),
            (b"Parent", Object::Ref(mid)),
            (b"FT", Object::name("Sig")),
            (b"V", Object::Ref(v)),
            (b"Subtype", Object::name("Widget")),
            (b"Rect", Object::Array(vec![0.into(), 0.into(), 10.into(), 10.into()])),
        ]));
        add_to(&mut doc, page, b"Annots", leaf);
        doc.update_dict(mid, |d| d.set(b"Kids".to_vec(), arr(&[leaf]))).unwrap();
        doc.update_dict(top, |d| d.set(b"Kids".to_vec(), arr(&[mid]))).unwrap();
        doc.update_dict(form, |d| d.set(b"Fields".to_vec(), arr(&[top]))).unwrap();
        let done = clear(&mut doc, "a.b.c", true).unwrap();
        assert_eq!(done.parents, 2);
        let fields = doc.get(form).as_dict().unwrap().get(b"Fields").unwrap().as_array().unwrap().clone();
        assert!(fields.is_empty(), "{fields:?}");
        assert!(done.sig_flags);
        assert_eq!(doc.get(form).as_dict().unwrap().get(b"SigFlags"), None, "no signature field is left");
    }

    #[test]
    fn a_shared_annots_array_is_updated_in_place() {
        let (mut doc, page, form) = base();
        let note = doc.add(dict(vec![(b"Type", Object::name("Annot")), (b"Subtype", Object::name("Text"))]));
        let shared = doc.add(arr(&[note]));
        doc.update_dict(page, |d| d.set(b"Annots".to_vec(), Object::Ref(shared))).unwrap();
        signed(&mut doc, page, form, "Signature1", vec![]);
        assert_eq!(doc.get(shared).as_array().unwrap().len(), 2);
        clear(&mut doc, "Signature1", true).unwrap();
        assert_eq!(doc.get(page).as_dict().unwrap().reference(b"Annots"), Some(shared), "the page still points at the shared array");
        assert_eq!(*doc.get(shared), arr(&[note]), "the comment stays");
    }

    #[test]
    fn keeping_the_field_drops_the_value_and_appearance_but_keeps_the_lock_and_seed_values() {
        let (mut doc, page, form) = base();
        let ap = doc.add(dict(vec![(b"Type", Object::name("XObject"))]));
        let lock = dict(vec![(b"Action", Object::name("All"))]);
        let (f, _) = signed(
            &mut doc,
            page,
            form,
            "Signature1",
            vec![
                (b"AP", Object::Dict(dict(vec![(b"N", Object::Ref(ap))]))),
                (b"Lock", Object::Dict(lock.clone())),
                (b"SV", Object::Dict(dict(vec![(b"Type", Object::name("SV"))]))),
            ],
        );
        let done = clear(&mut doc, "Signature1", false).unwrap();
        assert_eq!((done.removed_field, done.widgets), (false, 1));
        let d = doc.get(f).as_dict().unwrap().clone();
        assert!(!d.contains(b"V") && !d.contains(b"AP"));
        assert_eq!(d.get(b"Lock"), Some(&Object::Dict(lock)));
        assert!(d.contains(b"SV"));
        assert_eq!(annots(&doc, page).len(), 1, "the widget stays on its page");
        assert_eq!(doc.get(form).as_dict().unwrap().int(b"SigFlags"), Some(1), "a field exists, nothing is signed");
        assert_eq!(clear(&mut doc, "Signature1", false), Err(ClearError::NotSigned("Signature1".into())));
    }

    #[test]
    fn a_certification_entry_naming_the_signature_goes_and_usage_rights_stay() {
        let (mut doc, page, form) = base();
        let (_, v) = signed(&mut doc, page, form, "Signature1", vec![]);
        let ur = doc.add(dict(vec![(b"Type", Object::name("Sig"))]));
        let root = doc.root().unwrap();
        doc.update_dict(root, |d| d.set(b"Perms".to_vec(), Object::Dict(dict(vec![(b"DocMDP", Object::Ref(v)), (b"UR3", Object::Ref(ur))]))))
            .unwrap();
        let done = clear(&mut doc, "Signature1", true).unwrap();
        assert!(done.certification);
        let perms = doc.get(root).as_dict().unwrap().get(b"Perms").unwrap().as_dict().unwrap().clone();
        assert_eq!((perms.contains(b"DocMDP"), perms.reference(b"UR3")), (false, Some(ur)));
    }

    #[test]
    fn a_file_already_missing_objects_is_not_blamed_on_the_removal() {
        let (mut doc, page, form) = base();
        signed(&mut doc, page, form, "Signature1", vec![]);
        // A page annotation pointing at an object that doesn't exist, from before.
        let missing = ObjRef { num: 900, generation: 0 };
        add_to(&mut doc, page, b"Annots", missing);
        assert_eq!(problems(&doc).len(), 1);
        clear(&mut doc, "Signature1", true).unwrap();
        assert_eq!(problems(&doc).len(), 1, "the old problem is still there, and nothing was added");
    }

    #[test]
    fn a_failure_part_way_leaves_the_document_unchanged() {
        let (mut doc, page, form) = base();
        let v = doc.add(dict(vec![(b"Type", Object::name("Sig"))]));
        // An empty parent whose own parent is a missing object: pruning it fails after the
        // field itself has already been taken out.
        let missing = ObjRef { num: 900, generation: 0 };
        let parent = doc.add(dict(vec![(b"T", PdfString::text("a").into()), (b"Parent", Object::Ref(missing))]));
        let leaf = doc.add(dict(vec![
            (b"T", PdfString::text("b").into()),
            (b"Parent", Object::Ref(parent)),
            (b"FT", Object::name("Sig")),
            (b"V", Object::Ref(v)),
            (b"Subtype", Object::name("Widget")),
            (b"Rect", Object::Array(vec![0.into(), 0.into(), 10.into(), 10.into()])),
        ]));
        add_to(&mut doc, page, b"Annots", leaf);
        doc.update_dict(parent, |d| d.set(b"Kids".to_vec(), arr(&[leaf]))).unwrap();
        doc.update_dict(form, |d| d.set(b"Fields".to_vec(), arr(&[parent]))).unwrap();
        let before = doc.clone();
        assert!(clear(&mut doc, "a.b", true).is_err());
        assert_eq!((doc.get(parent), doc.get(leaf), annots(&doc, page)), (before.get(parent), before.get(leaf), annots(&before, page)));
    }

    #[test]
    fn a_cyclic_field_tree_does_not_hang() {
        let (mut doc, page, form) = base();
        let (f, _) = signed(&mut doc, page, form, "Signature1", vec![]);
        let loop_node = doc.add(dict(vec![(b"T", PdfString::text("loop").into())]));
        doc.update_dict(loop_node, |d| {
            d.set(b"Kids".to_vec(), arr(&[loop_node]));
            d.set(b"Parent".to_vec(), Object::Ref(loop_node));
        })
        .unwrap();
        add_to(&mut doc, form, b"Fields", loop_node);
        let _ = problems(&doc);
        assert_eq!(prune_empty_parents(&mut doc, Some(loop_node)), Ok(0), "a node that is its own kid isn't empty");
        clear(&mut doc, "Signature1", true).unwrap();
        assert!(doc.get(f).as_dict().is_some());
    }
}
