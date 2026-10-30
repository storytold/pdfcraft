//! Planned, page-scoped comment membership removal, without changing shared arrays or objects.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::types::FLAG_LOCKED;
use super::{AnnotError, Dict, Document, ObjRef, Object};

/// Removed `/Annots` memberships, including repeated references and hidden thread entries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClearCommentsReport {
    pub comments_removed: usize,
    pub popups_removed: usize,
    pub pages_changed: usize,
}

impl ClearCommentsReport {
    pub fn changed(self) -> bool {
        self.pages_changed != 0
    }
}

/// Clear comments and their associated pop-ups on one zero-based page, or every page.
///
/// Links, widgets, other non-comment types, unclassified entries and orphan pop-ups stay.
/// Unknown nonempty subtype names follow the existing Comments taxonomy. All selected locks,
/// memberships and traversal limits are checked before any write. Changed pages receive their
/// own direct arrays; annotation objects and old indirect arrays are never modified or freed.
pub fn clear_comments(doc: &mut Document, page: Option<usize>) -> Result<ClearCommentsReport, AnnotError> {
    clear_with_limits(doc, page, Limits::default())
}

#[derive(Clone, Copy)]
struct Limits {
    work: usize,
    entries: usize,
    alias_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { work: 4_000_000, entries: 1_000_000, alias_depth: 64 }
    }
}

fn invalid(reason: &str) -> AnnotError {
    AnnotError::Invalid(format!("could not clear comments: {reason}; the document was not changed"))
}

#[derive(Clone)]
struct Resolved {
    object: Arc<Object>,
    identity: Option<ObjRef>,
    hops: usize,
}

struct Reader<'a> {
    doc: &'a Document,
    limits: Limits,
    aliases: HashMap<ObjRef, Resolved>,
}

impl<'a> Reader<'a> {
    fn new(doc: &'a Document, limits: Limits) -> Self {
        Self { doc, limits, aliases: HashMap::new() }
    }

    fn charge(&mut self, count: usize) -> Result<(), AnnotError> {
        self.limits.work = self.limits.work.checked_sub(count).ok_or_else(|| invalid("the traversal work limit was reached"))?;
        Ok(())
    }

    fn membership_budget(&mut self, count: usize) -> Result<(), AnnotError> {
        self.limits.entries = self.limits.entries.checked_sub(count).ok_or_else(|| invalid("the annotation membership limit was reached"))?;
        self.charge(count)
    }

    fn resolve(&mut self, entry: &Object) -> Result<Resolved, AnnotError> {
        self.charge(1)?;
        let Some(mut reference) = entry.as_ref() else {
            return Ok(Resolved { object: Arc::new(entry.clone()), identity: None, hops: 0 });
        };
        let mut path = Vec::new();
        let mut resolved = loop {
            self.charge(1)?;
            if path.contains(&reference) {
                return Err(invalid("a reference cycle prevents identifying an annotation or page"));
            }
            if let Some(known) = self.aliases.get(&reference) {
                break known.clone();
            }
            if path.len() >= self.limits.alias_depth {
                return Err(invalid("the reference-chain limit was reached"));
            }
            path.push(reference);
            let object = self.doc.try_get(reference.num)?;
            if let Object::Ref(next) = &*object {
                reference = *next;
            } else {
                break Resolved { object, identity: Some(reference), hops: 0 };
            }
        };
        if path.len().checked_add(resolved.hops).is_none_or(|depth| depth > self.limits.alias_depth) {
            return Err(invalid("the reference-chain limit was reached"));
        }
        for alias in path.into_iter().rev() {
            resolved.hops = resolved.hops.checked_add(1).ok_or_else(|| invalid("the reference-chain depth overflowed"))?;
            self.aliases.insert(alias, resolved.clone());
        }
        Ok(resolved)
    }

    fn kind(&mut self, object: &Object) -> Result<Kind, AnnotError> {
        let Object::Dict(dict) = object else { return Ok(Kind::Other) };
        let Some(subtype) = dict.get(b"Subtype") else { return Ok(Kind::Other) };
        let subtype = self.resolve(subtype)?;
        match &*subtype.object {
            Object::Name(name) if name == b"Popup" => Ok(Kind::Popup),
            Object::Name(name) if !name.is_empty() && super::info::is_comment_name(name) => Ok(Kind::Comment),
            _ => Ok(Kind::Other),
        }
    }

    fn unlocked(&mut self, object: &Object) -> Result<(), AnnotError> {
        let Some(flags) = object.as_dict().and_then(|d| d.get(b"F")) else { return Ok(()) };
        if self.resolve(flags)?.object.as_int().unwrap_or(0) & FLAG_LOCKED != 0 {
            return Err(invalid("a selected comment or pop-up is locked"));
        }
        Ok(())
    }
}

enum Walk {
    Visit(ObjRef),
    Kids(Arc<Object>, usize),
    Leave(ObjRef),
}

/// Unlike the ordinary lenient page reader, a destructive operation must not accept a prefix.
fn checked_pages(reader: &mut Reader<'_>) -> Result<Vec<ObjRef>, AnnotError> {
    let root = reader.doc.root().ok_or(AnnotError::NoPageTree)?;
    let root = reader.resolve(&Object::Ref(root))?;
    // The page model reads one reference hop. Following aliases here could assign a visible
    // page index to another page, so refuse them only in page-tree structure, not annotations.
    if root.hops > 1 {
        return Err(invalid("a catalog alias makes page indices ambiguous"));
    }
    let top = root.object.as_dict().and_then(|d| d.reference(b"Pages")).ok_or(AnnotError::NoPageTree)?;
    let mut pending = vec![Walk::Visit(top)];
    let mut active = HashSet::new();
    let mut seen = HashSet::new();
    let mut pages = Vec::new();
    while let Some(step) = pending.pop() {
        reader.charge(1)?;
        match step {
            Walk::Leave(node) => {
                active.remove(&node);
            }
            Walk::Kids(kids, index) => {
                let Some(entry) = kids.as_array().and_then(|a| a.get(index)) else { continue };
                let child = entry.as_ref().ok_or_else(|| invalid("a page-tree child is not a reference"))?;
                let next = index.checked_add(1).ok_or_else(|| invalid("the page-tree index overflowed"))?;
                pending.push(Walk::Kids(kids, next));
                pending.push(Walk::Visit(child));
            }
            Walk::Visit(reference) => visit_page(reader, reference, &mut active, &mut seen, &mut pages, &mut pending)?,
        }
    }
    Ok(pages)
}

fn visit_page(
    reader: &mut Reader<'_>,
    reference: ObjRef,
    active: &mut HashSet<ObjRef>,
    seen: &mut HashSet<ObjRef>,
    pages: &mut Vec<ObjRef>,
    pending: &mut Vec<Walk>,
) -> Result<(), AnnotError> {
    let node = reader.resolve(&Object::Ref(reference))?;
    if node.hops > 1 {
        return Err(invalid("a page-tree node alias makes page indices ambiguous"));
    }
    let identity = node.identity.ok_or_else(|| invalid("a page has no reference identity"))?;
    if active.contains(&identity) {
        return Err(invalid("the page tree contains a cycle"));
    }
    if !seen.insert(identity) {
        return Ok(());
    }
    let Object::Dict(dict) = &*node.object else { return Err(invalid("a page-tree node is not a dictionary")) };
    match dict.name(b"Type") {
        Some(b"Page") => pages.push(identity),
        Some(b"Pages") => {
            let kids = dict.get(b"Kids").ok_or_else(|| invalid("a page-tree branch has no Kids array"))?;
            if kids.as_array().is_some_and(|a| a.len() > reader.limits.work) {
                return Err(invalid("the page-tree allocation limit was reached"));
            }
            let kids = reader.resolve(kids)?;
            if kids.hops > 1 {
                return Err(invalid("a page-tree Kids alias makes page indices ambiguous"));
            }
            if kids.object.as_array().is_none() {
                return Err(invalid("a page-tree Kids entry is not an array"));
            }
            active.insert(identity);
            pending.push(Walk::Leave(identity));
            pending.push(Walk::Kids(kids.object, 0));
        }
        _ => return Err(invalid("a page-tree node has an unknown Type")),
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Comment,
    Popup,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Owner {
    Reference(ObjRef),
    Inline(ObjRef, usize),
}

struct Member {
    resolved: Resolved,
    kind: Kind,
}

struct Membership {
    page: ObjRef,
    dict: Dict,
    original: Arc<Object>,
    members: Vec<Member>,
}

#[derive(Default)]
struct Owners {
    comments: HashSet<ObjRef>,
    forward: HashMap<ObjRef, Owner>,
}

impl Owners {
    fn add(&mut self, reader: &mut Reader<'_>, comment: &Resolved, page: ObjRef, index: usize) -> Result<(), AnnotError> {
        let owner = comment.identity.map_or(Owner::Inline(page, index), Owner::Reference);
        if let Some(identity) = comment.identity {
            self.comments.insert(identity);
        }
        let Some(popup) = comment.object.as_dict().and_then(|d| d.get(b"Popup")) else { return Ok(()) };
        let popup = reader.resolve(popup)?;
        if reader.kind(&popup.object)? == Kind::Popup
            && let Some(identity) = popup.identity
        {
            if self.forward.get(&identity).is_some_and(|previous| *previous != owner) {
                return Err(invalid("a pop-up is claimed by different comments"));
            }
            self.forward.insert(identity, owner);
        }
        Ok(())
    }

    fn removes_popup(&self, reader: &mut Reader<'_>, popup: &Resolved) -> Result<bool, AnnotError> {
        let forward = popup.identity.and_then(|r| self.forward.get(&r)).copied();
        let parent = popup.object.as_dict().and_then(|d| d.get(b"Parent"));
        let parent = parent.map(|p| reader.resolve(p)).transpose()?;
        let identity = parent.as_ref().and_then(|p| p.identity);
        if let Some(owner) = forward {
            if parent.is_some() && identity.map(Owner::Reference) != Some(owner) {
                return Err(invalid("a pop-up's Parent contradicts its comment's Popup reference"));
            }
            return Ok(true);
        }
        Ok(identity.is_some_and(|r| self.comments.contains(&r)))
    }
}

fn read_membership(reader: &mut Reader<'_>, page: ObjRef, owners: &mut Owners) -> Result<Membership, AnnotError> {
    let object = reader.resolve(&Object::Ref(page))?;
    let Object::Dict(dict) = &*object.object else { return Err(invalid("a selected page is not a dictionary")) };
    let original = match dict.get(b"Annots") {
        Some(entry) => {
            // A direct array would be cloned by resolve, so account for it first.
            if let Some(entries) = entry.as_array() {
                reader.membership_budget(entries.len())?;
            }
            let original = reader.resolve(entry)?.object;
            if entry.as_ref().is_some()
                && let Some(entries) = original.as_array()
            {
                reader.membership_budget(entries.len())?;
            }
            original
        }
        None => Arc::new(Object::Array(Vec::new())),
    };
    let entries = original.as_array().ok_or_else(|| invalid("a selected page's Annots membership is not an array"))?;
    let mut members = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let resolved = reader.resolve(entry)?;
        let kind = reader.kind(&resolved.object)?;
        if kind == Kind::Comment {
            reader.unlocked(&resolved.object)?;
            owners.add(reader, &resolved, page, index)?;
        }
        members.push(Member { resolved, kind });
    }
    Ok(Membership { page, dict: dict.clone(), original, members })
}

fn replacement(
    reader: &mut Reader<'_>,
    membership: Membership,
    owners: &Owners,
    report: &mut ClearCommentsReport,
) -> Result<Option<(ObjRef, Dict)>, AnnotError> {
    let mut kept = Vec::new();
    let mut changed = false;
    let entries = membership.original.as_array().ok_or_else(|| invalid("the planned annotation membership is not an array"))?;
    for (entry, member) in entries.iter().zip(membership.members) {
        reader.charge(1)?;
        let remove = member.kind == Kind::Comment || (member.kind == Kind::Popup && owners.removes_popup(reader, &member.resolved)?);
        if remove {
            reader.unlocked(&member.resolved.object)?;
            changed = true;
            if member.kind == Kind::Comment {
                report.comments_removed = report.comments_removed.checked_add(1).ok_or_else(|| invalid("the comment count overflowed"))?;
            } else {
                report.popups_removed = report.popups_removed.checked_add(1).ok_or_else(|| invalid("the pop-up count overflowed"))?;
            }
        } else {
            kept.push(entry.clone());
        }
    }
    if !changed {
        return Ok(None);
    }
    let mut dict = membership.dict;
    if kept.is_empty() {
        dict.remove(b"Annots");
    } else {
        dict.set(b"Annots".to_vec(), Object::Array(kept));
    }
    report.pages_changed = report.pages_changed.checked_add(1).ok_or_else(|| invalid("the changed-page count overflowed"))?;
    Ok(Some((membership.page, dict)))
}

fn clear_with_limits(doc: &mut Document, page: Option<usize>, limits: Limits) -> Result<ClearCommentsReport, AnnotError> {
    let mut reader = Reader::new(doc, limits);
    let pages = checked_pages(&mut reader)?;
    let selected = match page {
        Some(index) => vec![pages.get(index).copied().ok_or(AnnotError::NoSuchPage(index))?],
        None => pages,
    };
    let mut owners = Owners::default();
    let mut memberships = Vec::new();
    for page in selected {
        memberships.push(read_membership(&mut reader, page, &mut owners)?);
    }
    let mut report = ClearCommentsReport::default();
    let mut writes = Vec::new();
    for membership in memberships {
        if let Some(write) = replacement(&mut reader, membership, &owners, &mut report)? {
            writes.push(write);
        }
    }
    // Every fallible read, ownership/lock check and allocation above precedes these writes.
    // Use the planned dictionaries, so even the final stage needs no fallible object lookup.
    for (page, dict) in writes {
        doc.set(page, Object::Dict(dict));
    }
    Ok(report)
}

#[cfg(test)]
mod tests;
