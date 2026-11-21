//! Edit ▸ Copy, Cut and Paste of objects selected together in Edit a PDF (#5989).
//!
//! A copy ([`ObjectClip`]) holds what the objects draw and the resources that drawing names
//! ([`pdfcraft_edit::copy_objects`]), with a snapshot of the document they came from, so it can
//! be pasted into any open document, also after the original changes or closes. Within the same
//! document, while the objects it refers to are unchanged, a paste draws them again from the same
//! fonts and images; otherwise it first copies them over ([`pdfcraft_organize::import_value`]):
//! each once, through cycles, sharing what the target already holds, and within a size limit.

use std::sync::Arc;

use pdfcraft_cos::Object;

use crate::{DocId, Document, Edit, EditError, EditableObject, ObjectTarget};

/// The most objects a paste copies into another document.
const MAX_IMPORT_OBJECTS: usize = 20_000;
/// The most stream data (fonts, images…) a paste copies into another document.
const MAX_IMPORT_BYTES: usize = 256 << 20;
/// How far each further paste onto the same page steps down and to the right, in points, as
/// pasted added content steps ([`pdfcraft_edit::added::PASTE_STEP`]).
const STEP: f64 = pdfcraft_edit::added::PASTE_STEP;

/// Objects copied from one page of a document.
#[derive(Clone)]
pub struct ObjectClip {
    copied: pdfcraft_edit::CopiedObjects,
    /// The document as it was when copied: what the copy's resources refer to.
    source: pdfcraft_cos::Document,
    doc: DocId,
    page: usize,
}

impl PartialEq for ObjectClip {
    fn eq(&self, other: &Self) -> bool {
        self.copied == other.copied
            && self.doc == other.doc
            && self.page == other.page
            && self.source.source_identity() == other.source.source_identity()
    }
}

impl std::fmt::Debug for ObjectClip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectClip").field("doc", &self.doc).field("page", &self.page).field("objects", &self.copied.objects.len()).finish()
    }
}

impl ObjectClip {
    /// The objects as they were listed when copied ([`Document::editable_objects`]).
    pub fn objects(&self) -> &[EditableObject] {
        &self.copied.objects
    }

    /// The text of the copied paragraphs and text boxes, one per line.
    pub fn text(&self) -> String {
        self.copied.text()
    }

    /// The union of the objects' boxes, in the user space of the page they came from.
    pub fn rect(&self) -> [f64; 4] {
        self.copied.rect
    }

    /// The document and page the objects were copied from.
    pub fn source(&self) -> (DocId, usize) {
        (self.doc, self.page)
    }
}

impl Document {
    /// Edit ▸ Copy of `targets` (references from one [`Document::editable_objects`] inventory of
    /// `page`). Refused when the document doesn't allow copying. Changes nothing.
    pub fn copy_objects(&self, page: usize, targets: &[ObjectTarget]) -> Result<ObjectClip, EditError> {
        self.check_copying()?;
        let editor = self.editor.as_ref().ok_or_else(|| EditError::ReadOnly("the document can't be read".into()))?;
        let copied = pdfcraft_edit::copy_objects(&editor.cos, page, targets)?;
        Ok(ObjectClip { copied, source: editor.cos.clone(), doc: self.id, page })
    }

    /// Where Paste puts `clip` on `page`: the user-space offset that moves it `steps` × 18 pt down
    /// and to the right as displayed, or, when that leaves nothing of it on the page's crop box,
    /// the offset that centres it there.
    pub fn object_paste_offset(&self, page: usize, clip: &ObjectClip, steps: u32) -> Result<[f64; 2], String> {
        let step = STEP * f64::from(steps);
        let offset = self.object_move_offset(page, [step, step])?;
        let p = self.info.pages.get(page).ok_or("the page no longer exists")?;
        let crop = p.crop.map(f64::from);
        let r = clip.rect();
        let moved = [r[0] + offset[0], r[1] + offset[1], r[2] + offset[0], r[3] + offset[1]];
        if moved[0] < crop[2] && moved[2] > crop[0] && moved[1] < crop[3] && moved[3] > crop[1] {
            return Ok(offset);
        }
        let centre = |a: f64, b: f64| (a + b) / 2.0;
        Ok([centre(crop[0], crop[2]) - centre(r[0], r[2]), centre(crop[1], crop[3]) - centre(r[1], r[3])])
    }
}

/// The objects copied in an application or a script, and how many times each page has had them
/// pasted, which sets how far the next paste there steps ([`Document::object_paste_offset`]).
#[derive(Clone, Debug)]
pub struct ObjectClipboard {
    clip: Arc<ObjectClip>,
    pastes: Vec<(DocId, usize, u32)>,
}

impl ObjectClipboard {
    pub fn new(clip: ObjectClip) -> Self {
        ObjectClipboard { clip: Arc::new(clip), pastes: Vec::new() }
    }

    pub fn clip(&self) -> &ObjectClip {
        &self.clip
    }

    /// The edit that pastes the copy onto `page` of `doc`: where it was, or on the page it came
    /// from one step down and to the right (as Acrobat offsets a paste onto the same page), and
    /// one step further for each earlier paste onto that page.
    pub fn paste(&mut self, doc: &Document, page: usize) -> Result<Edit, String> {
        let at = self.pastes.iter().position(|(d, p, _)| *d == doc.id && *p == page);
        let earlier = at.and_then(|i| self.pastes.get(i)).map_or(0, |(_, _, n)| *n);
        let own = u32::from(self.clip.source() == (doc.id, page));
        let offset = doc.object_paste_offset(page, &self.clip, earlier.saturating_add(own))?;
        match at.and_then(|i| self.pastes.get_mut(i)) {
            Some(entry) => entry.2 = entry.2.saturating_add(1),
            None => self.pastes.push((doc.id, page, 1)),
        }
        Ok(Edit::PasteObjects { page, clip: Arc::clone(&self.clip), offset })
    }
}

impl Edit {
    /// The edit that deletes `targets` (indexes into one inventory of `page`) as one undo step:
    /// what Delete and Cut do to objects selected together. One object is deleted by its own
    /// edit, several by [`Edit::DeleteObjects`]; none is no edit.
    pub fn delete_objects(page: usize, mut targets: Vec<ObjectTarget>) -> Option<Edit> {
        targets.sort();
        targets.dedup();
        match targets.as_slice() {
            [] => None,
            [t] => Some(delete_one(page, *t)),
            _ => Some(Edit::DeleteObjects { page, objects: targets }),
        }
    }
}

fn delete_one(page: usize, t: ObjectTarget) -> Edit {
    use crate::ObjectKind;
    match t.kind {
        ObjectKind::Text => Edit::EditTextBlock { page, block: t.index, text: String::new(), style: crate::BlockStyle::default() },
        ObjectKind::Image => Edit::EditPageImage { page, index: t.index, change: crate::ImageEdit::Delete },
        ObjectKind::Added => Edit::DeleteContent { page, index: t.index },
    }
}

/// [`Edit::DeleteObjects`], each object by its own edit (`run`). A deletion renumbers what follows
/// it in its own inventory, so pictures go from the last to the first, and added content, which
/// the other inventories leave out, after them. Deleting a paragraph also removes a copy drawn
/// exactly over it (fake bold, a paste onto itself), which renumbers paragraphs before it too: so
/// each paragraph is found again by its text and box, and one already gone with another is done.
pub(crate) fn delete(
    doc: &mut pdfcraft_cos::Document,
    page: usize,
    objects: &[ObjectTarget],
    mut run: impl FnMut(&mut pdfcraft_cos::Document, &Edit) -> Result<(), EditError>,
) -> Result<(), EditError> {
    use crate::ObjectKind;
    let mut targets = objects.to_vec();
    targets.sort_by_key(|t| (t.kind == ObjectKind::Added, std::cmp::Reverse(t.index)));
    targets.dedup();
    let blocks = pdfcraft_edit::text_blocks(doc, page)?;
    for t in targets {
        let edit = match t.kind {
            ObjectKind::Text => {
                let key = blocks.get(t.index).map(crate::BlockKey::of).ok_or_else(|| {
                    EditError::Edit(pdfcraft_edit::EditError::Invalid("a selected paragraph no longer exists; select the objects again".into()))
                })?;
                let Some(block) = key.find(&pdfcraft_edit::text_blocks(doc, page)?, t.index) else { continue };
                delete_one(page, ObjectTarget { kind: t.kind, index: block })
            }
            _ => delete_one(page, t),
        };
        run(doc, &edit)?;
    }
    Ok(())
}

/// [`Edit::PasteObjects`]: the copy's resources as `doc` holds them, then the paste.
pub(crate) fn paste(doc: &mut pdfcraft_cos::Document, page: usize, clip: &ObjectClip, offset: [f64; 2]) -> Result<(), EditError> {
    let resources = Object::Dict(clip.copied.resources.clone());
    let refused =
        |e: pdfcraft_organize::OrganizeError| EditError::Edit(pdfcraft_edit::EditError::Invalid(format!("the copied objects can't be pasted: {e}")));
    let used = pdfcraft_organize::reachable(&clip.source, &resources, MAX_IMPORT_OBJECTS, MAX_IMPORT_BYTES).map_err(refused)?;
    // Still the objects the copy was made from: the same document, and each of them unchanged.
    let same = doc.source_identity() == clip.source.source_identity()
        && used.iter().all(|r| {
            let (here, there) = (doc.get(*r), clip.source.get(*r));
            Arc::ptr_eq(&here, &there) || here == there
        });
    let resources = if same { resources } else { pdfcraft_organize::import_value(doc, &clip.source, &resources).map_err(refused)? };
    let Object::Dict(resources) = resources else {
        return Err(EditError::Edit(pdfcraft_edit::EditError::Invalid("the copied objects' resources could not be copied".into())));
    };
    let copied = pdfcraft_edit::CopiedObjects { resources, ..clip.copied.clone() };
    pdfcraft_edit::paste_objects(doc, page, &copied, offset)?;
    Ok(())
}

#[cfg(test)]
#[path = "object_clip_tests.rs"]
mod tests;
