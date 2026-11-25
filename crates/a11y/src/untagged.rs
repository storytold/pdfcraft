//! Mark a tagged page's untagged content as artifacts (ISO 32000-1 §14.8.2.2.2; ISO 14289-1
//! §7.1): content that is neither in marked content the structure tree claims nor an artifact
//! already is wrapped in `/Artifact BMC … EMC`. Calling it decorative is an inference, so the
//! result says so.
//!
//! Adapted from pdfcer (MIT, same author) at commit debd0d63, `edit/pdfua_artifacts.rs`.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::alt::{DecodedStream, Read, decode_page_streams, tree_root, write_streams};
use crate::untagged_wraps::{Planner, Wraps};

/// Structure-tree nodes read before giving up.
const MAX_NODES: usize = 1_000_000;

/// Why nothing was marked.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum UntaggedError {
    #[error("the document has no tags")]
    Untagged,
    #[error("page {0} is out of range")]
    PageOutOfRange(usize),
    #[error("the structure tree is too large to read")]
    TooLarge,
    #[error(transparent)]
    Cos(#[from] pdfcraft_cos::CosError),
}

/// A page whose untagged content became artifacts.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkedPage {
    /// 0-based.
    pub page: usize,
    /// `/Artifact` sequences added.
    pub sequences: usize,
    /// Content items they hold.
    pub items: usize,
}

/// A page left as it is, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct SkippedPage {
    /// 0-based.
    pub page: usize,
    pub reason: String,
}

/// What marking untagged content as artifacts did, or would do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UntaggedArtifacts {
    pub marked: Vec<MarkedPage>,
    pub sequences: usize,
    pub items: usize,
    pub skipped: Vec<SkippedPage>,
    /// What was left untouched on the pages that were read, and what the change means.
    pub notes: Vec<String>,
}

/// The marked content the structure tree claims: `(page, MCID)`, and MCIDs with no page.
#[derive(Debug, Default)]
pub(crate) struct Claims {
    on_page: HashSet<(ObjRef, i64)>,
    anywhere: HashSet<i64>,
}

impl Claims {
    pub(crate) fn claims(&self, page: ObjRef, mcid: i64) -> bool {
        self.on_page.contains(&(page, mcid)) || self.anywhere.contains(&mcid)
    }

    fn any_on(&self, page: ObjRef) -> bool {
        self.on_page.iter().any(|(p, _)| *p == page)
    }

    fn add(&mut self, page: Option<ObjRef>, mcid: i64) {
        match page {
            Some(p) => self.on_page.insert((p, mcid)),
            None => self.anywhere.insert(mcid),
        };
    }

    /// Every MCID the tree's elements own, direct elements too. Content an MCR places in a
    /// form (`/Stm`) is not on a page's own content, so it is not counted.
    fn read(doc: &Document, root: &Dict) -> Result<Self, UntaggedError> {
        let mut claims = Claims::default();
        let mut seen = HashSet::new();
        let mut stack: Vec<(Object, Option<ObjRef>)> = root.get(b"K").map(|k| vec![(k.clone(), None)]).unwrap_or_default();
        let mut visits = 0usize;
        while let Some((k, pg)) = stack.pop() {
            visits = visits.saturating_add(1);
            if visits > MAX_NODES {
                return Err(UntaggedError::TooLarge);
            }
            match k {
                Object::Int(n) => claims.add(pg, n),
                Object::Array(a) => stack.extend(a.into_iter().map(|x| (x, pg))),
                Object::Ref(r) => {
                    if seen.insert(r) {
                        stack.push(((*doc.get(r)).clone(), pg));
                    }
                }
                Object::Dict(d) if d.name(b"Type") == Some(b"MCR") => {
                    if !d.contains(b"Stm")
                        && let Some(n) = d.get(b"MCID").map(|m| doc.resolve(m)).and_then(|m| m.as_int())
                    {
                        claims.add(d.get(b"Pg").and_then(Object::as_ref).or(pg), n);
                    }
                }
                Object::Dict(d) if d.contains(b"S") => {
                    let pg = d.get(b"Pg").and_then(Object::as_ref).or(pg);
                    if let Some(kids) = d.get(b"K") {
                        stack.push((kids.clone(), pg));
                    }
                }
                _ => {}
            }
        }
        Ok(claims)
    }
}

/// What [`mark_untagged_artifacts`] would do to `pages` (0-based; empty means every page),
/// without changing the document.
///
/// # Errors
///
/// The document has no tags, a page is out of range, or the structure tree is too large.
pub fn plan_untagged_artifacts(doc: &Document, pages: &[usize]) -> Result<UntaggedArtifacts, UntaggedError> {
    plan(doc, pages).map(|(out, _)| out)
}

/// Wrap the untagged content of each tagged page in `pages` (0-based; empty means every page) in
/// `/Artifact` marked content, rewriting only the content streams that change. A page without
/// tags, one whose content is damaged or doesn't pair up, marked content with an MCID nothing
/// claims, forms holding tagged content and annotation appearances are left as they are.
///
/// # Errors
///
/// The document has no tags, a page is out of range, or the structure tree is too large.
pub fn mark_untagged_artifacts(doc: &mut Document, pages: &[usize]) -> Result<UntaggedArtifacts, UntaggedError> {
    let (out, writes) = plan(doc, pages)?;
    for (page, streams, pieces, wraps) in writes {
        write_streams(doc, page, streams, pieces.splice(wraps.edits))?;
    }
    Ok(out)
}

type Write = (ObjRef, Vec<DecodedStream>, pdfcraft_content::Pieces, Wraps);

fn plan(doc: &Document, pages: &[usize]) -> Result<(UntaggedArtifacts, Vec<Write>), UntaggedError> {
    let root = tree_root(doc).ok_or(UntaggedError::Untagged)?;
    let claims = Claims::read(doc, &root)?;
    let all = pdfcraft_model::pages(doc);
    if let Some(bad) = pages.iter().find(|&&p| p >= all.len()) {
        return Err(UntaggedError::PageOutOfRange(*bad));
    }
    let chosen: Vec<usize> = if pages.is_empty() { (0..all.len()).collect() } else { pages.to_vec() };
    let mut out = UntaggedArtifacts::default();
    let mut writes = Vec::new();
    let mut done = HashSet::new();
    let mut left = Left::default();
    for index in chosen {
        let Some(page) = all.get(index).filter(|_| done.insert(index)) else { continue };
        match page_wraps(doc, page, &claims) {
            Err(reason) => out.skipped.push(SkippedPage { page: index, reason }),
            Ok(None) => {}
            Ok(Some((streams, pieces, wraps))) => {
                left.add(&wraps);
                if wraps.sequences == 0 {
                    continue;
                }
                out.sequences = out.sequences.saturating_add(wraps.sequences);
                out.items = out.items.saturating_add(wraps.items);
                out.marked.push(MarkedPage { page: index, sequences: wraps.sequences, items: wraps.items });
                writes.push((page.obj, streams, pieces, wraps));
            }
        }
    }
    out.notes = left.notes(&out);
    Ok((out, writes))
}

type PageWraps = (Vec<DecodedStream>, pdfcraft_content::Pieces, Wraps);

/// The wraps for one page; `None` when it has nothing to read, the reason when it is skipped.
fn page_wraps(doc: &Document, page: &pdfcraft_model::Page, claims: &Claims) -> Result<Option<PageWraps>, String> {
    if !page.dict.contains(b"StructParents") && !claims.any_on(page.obj) {
        return Err("the page has no tags".into());
    }
    let streams = decode_page_streams(doc, &page.dict).ok_or("the page has more content than can be edited")?;
    if streams.iter().any(|(_, s)| s.as_ref().is_some_and(|(_, _, read)| *read != Read::Whole)) {
        return Err("a content stream is damaged or can't be read".into());
    }
    let decoded: Vec<&[u8]> = streams.iter().filter_map(|(_, s)| s.as_ref().map(|(_, bytes, _)| bytes.as_slice())).collect();
    if decoded.is_empty() {
        return Ok(None);
    }
    let pieces = pdfcraft_content::Pieces::join(&decoded);
    let parsed = pieces.parse();
    if parsed.skipped > 0 || parsed.ambiguous_images > 0 {
        return Err("its content has bytes or inline images that can't be read reliably".into());
    }
    let wraps = Planner::new(doc, page.obj, &page.dict, claims).plan(&pieces, &parsed.ops).map_err(|e| e.to_string())?;
    Ok(Some((streams, pieces, wraps)))
}

/// What was left on the pages that were read.
#[derive(Default)]
struct Left {
    orphans: usize,
    mixed: HashSet<ObjRef>,
    unread: HashSet<ObjRef>,
    split: usize,
}

impl Left {
    fn add(&mut self, wraps: &Wraps) {
        self.orphans = self.orphans.saturating_add(wraps.orphans);
        self.mixed.extend(wraps.mixed_forms.iter().copied());
        self.unread.extend(wraps.unread_forms.iter().copied());
        self.split = self.split.saturating_add(wraps.split_items);
    }

    fn notes(&self, out: &UntaggedArtifacts) -> Vec<String> {
        let mut notes = Vec::new();
        if out.items > 0 {
            notes.push(format!(
                "Treated {} untagged content item(s) as decorative and marked them as artifacts; assistive technology will skip them. Tag any that carry meaning.",
                out.items
            ));
        }
        if self.orphans > 0 {
            notes.push(format!(
                "Left {} marked-content sequence(s) whose MCID the structure tree doesn't claim; tag them or mark them as artifacts.",
                self.orphans
            ));
        }
        if !self.mixed.is_empty() {
            notes.push(format!(
                "Left {} form(s) that hold tagged and untagged content together; their untagged content is still untagged.",
                self.mixed.len()
            ));
        }
        if !self.unread.is_empty() {
            notes.push(format!("Left {} form(s) that could not be read in full.", self.unread.len()));
        }
        if self.split > 0 {
            notes.push(format!("Left {} untagged item(s) whose operator is split across two content streams.", self.split));
        }
        notes
    }
}
