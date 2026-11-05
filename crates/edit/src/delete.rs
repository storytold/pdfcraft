//! Delete several page objects at once: vector paths and images (including Form artwork).

use std::collections::BTreeSet;

use pdfcraft_cos::Document;

use crate::{EditError, MAX_MOVE_OBJECTS, ObjectKind, ObjectTarget, editable_objects};

/// Delete `targets` (references into [`editable_objects`]) as one atomic edit. Text and added
/// content are refused. A refusal leaves the document untouched.
pub fn delete_objects(doc: &mut Document, page: usize, targets: &[ObjectTarget]) -> Result<(), EditError> {
    if targets.is_empty() || targets.len() > MAX_MOVE_OBJECTS {
        return Err(EditError::Invalid(format!("select between 1 and {MAX_MOVE_OBJECTS} objects to delete")));
    }
    if targets.iter().any(|t| matches!(t.kind, ObjectKind::Text | ObjectKind::Added)) {
        return Err(EditError::Invalid("only images and vector paths can be deleted with this selection, not text or added content".into()));
    }
    if targets.iter().collect::<BTreeSet<_>>().len() != targets.len() {
        return Err(EditError::Invalid("an object was selected more than once".into()));
    }
    // The inventory leaves out the images added content draws, which page_images still counts.
    let inventory: BTreeSet<_> = editable_objects(doc, page)?.into_iter().map(|o| o.target).collect();
    if !targets.iter().all(|t| inventory.contains(t)) {
        return Err(EditError::Invalid("a selected object no longer exists; select the objects again".into()));
    }
    let indexes = |kind| targets.iter().filter(|t| t.kind == kind).map(|t| t.index).collect::<Vec<_>>();
    let mut work = doc.clone();
    // One splice for both kinds, so each touched stream becomes one new object.
    crate::path_edit::delete_paths_and_images(&mut work, page, &indexes(ObjectKind::Path), &indexes(ObjectKind::Image))?;
    editable_objects(&work, page)?;
    *doc = work;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::image_page_with;

    #[test]
    fn paths_and_images_go_in_one_splice_that_keeps_every_other_byte() {
        let mut doc = image_page_with(
            "% kept\nq 50 0 0 30 100 100 cm /Im0 Do Q 0.50 0 0 rg 10 10 20 20 re f q 80 0 0 40 300 200 cm /Im0 Do Q .5 w 100 300 m 150 300 l S",
        );
        let t = |kind, index| ObjectTarget { kind, index };
        let (image, path) = (ObjectKind::Image, ObjectKind::Path);
        let targets: Vec<_> = editable_objects(&doc, 0).unwrap().into_iter().map(|o| o.target).collect();
        assert_eq!(targets, [t(image, 0), t(image, 1), t(path, 0), t(path, 1)]);
        delete_objects(&mut doc, 0, &[t(path, 0), t(image, 1), t(image, 0)]).unwrap();
        let content = crate::objects::checked_streams(&doc, 0).unwrap().remove(0).1;
        assert_eq!(
            String::from_utf8(content).unwrap(),
            "% kept\nq 50 0 0 30 100 100 cm  Q 0.50 0 0 rg   q 80 0 0 40 300 200 cm  Q .5 w 100 300 m 150 300 l S"
        );
        // The fixture has six objects: the only new one is the content stream.
        assert_eq!(doc.modified_objects().into_iter().filter(|&n| n > 6).count(), 1);
        let left: Vec<_> = editable_objects(&doc, 0).unwrap().into_iter().map(|o| o.target).collect();
        assert_eq!(left, [t(path, 0)]);
    }
}
