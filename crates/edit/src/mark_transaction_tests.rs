//! The public mark APIs either commit the complete operation or leave the document untouched.
use super::{cx, fixture, reopen, streams};
use crate::place::MARK_PLACEMENT_FAILURE;
use crate::*;
use pdfcraft_cos::{ObjRef, SaveOptions, write_incremental};

const SELECTED: &[usize] = &[0, 1, 2];

struct PlacementFailure;

impl PlacementFailure {
    fn after_two_pages(kind: MarkKind) -> Self {
        MARK_PLACEMENT_FAILURE.with(|failure| {
            assert!(failure.get().is_none());
            failure.set(Some((kind, 2, 0)));
        });
        Self
    }

    fn completed(&self) -> usize {
        MARK_PLACEMENT_FAILURE.with(|failure| failure.get().unwrap().2)
    }
}

impl Drop for PlacementFailure {
    fn drop(&mut self) {
        MARK_PLACEMENT_FAILURE.with(|failure| failure.set(None));
    }
}

/// Four pages share an original stream and resources, both inherited and directly referenced.
/// All have all three existing marks, including a picture background with a replaced white fill.
fn marked_document() -> (Document, MarkSource) {
    let mut doc = fixture();
    let parent = ObjRef::new(2, 0);
    let extra = doc.add(Object::Dict(Dict::from_iter([
        (b"Type".to_vec(), Object::name("Page")),
        (b"Parent".to_vec(), Object::Ref(parent)),
        (b"Contents".to_vec(), Object::Ref(ObjRef::new(7, 0))),
    ])));
    doc.update_dict(parent, |d| {
        d.set(b"Kids".to_vec(), Object::Array([3, 4, 5].map(|n| Object::Ref(ObjRef::new(n, 0))).into_iter().chain([Object::Ref(extra)]).collect()));
        d.set(b"Count".to_vec(), Object::Int(4));
    })
    .unwrap();
    doc.set(ObjRef::new(7, 0), Object::Stream(Stream::from_raw(Dict::new(), b"1 g 0 0 600 800 re f 0 g BT /F1 9 Tf (Original page) Tj ET".to_vec())));
    let picture = |doc: &mut Document, gray| {
        let image = doc.add(Object::Stream(Stream::from_raw(
            Dict::from_iter([
                (b"Type".to_vec(), Object::name("XObject")),
                (b"Subtype".to_vec(), Object::name("Image")),
                (b"Width".to_vec(), Object::Int(1)),
                (b"Height".to_vec(), Object::Int(1)),
                (b"ColorSpace".to_vec(), Object::name("DeviceGray")),
                (b"BitsPerComponent".to_vec(), Object::Int(8)),
            ]),
            vec![gray],
        )));
        MarkSource { xobject: image, size: (1.0, 1.0), image: true }
    };
    let old_picture = picture(&mut doc, 170);
    let replacement_picture = picture(&mut doc, 80);
    let all = [0, 1, 2, 3];
    let hf = HeaderFooter {
        text: ["Old header".into(), String::new(), String::new(), String::new(), String::new(), String::new()],
        font: added::Family::Times,
        ..Default::default()
    };
    add_header_footer(&mut doc, &all, &hf, false, &cx()).unwrap();
    add_watermark(&mut doc, &all, &Watermark { text: "Old watermark".into(), opacity: 0.45, ..Default::default() }, false).unwrap();
    add_background(&mut doc, &all, &Background { source: Some(old_picture), opacity: 0.6, replace_backdrop: true, ..Default::default() }, false)
        .unwrap();
    let pages = page_list(&doc);
    let shared = ObjRef::new(6, 0);
    let resources = (*doc.resolve(pages[0].dict.get(b"Resources").unwrap())).clone();
    doc.set(shared, resources);
    for (i, page) in pages.iter().enumerate() {
        doc.update_dict(page.obj, |d| {
            if i % 2 == 0 {
                d.set(b"Resources".to_vec(), Object::Ref(shared));
            } else {
                d.remove(b"Resources");
            }
        })
        .unwrap();
    }
    let doc = reopen(&doc);
    assert!(!doc.is_modified());
    let pages = page_list(&doc);
    assert_eq!(pages.len(), 4);
    assert!(doc.get(pages[1].obj).as_dict().unwrap().get(b"Resources").is_none(), "a selected page inherits its resources");
    assert_eq!(pages[0].dict.get(b"Resources").and_then(Object::as_ref), Some(shared));
    assert_eq!(pages[2].dict.get(b"Resources").and_then(Object::as_ref), Some(shared));
    for i in 0..4 {
        for kind in [MarkKind::HeaderFooter, MarkKind::Watermark, MarkKind::Background] {
            assert_eq!(mark_streams(&doc, i, kind).len(), 1, "page {i} has existing {kind:?}");
        }
    }
    (doc, replacement_picture)
}

fn mark_streams(doc: &Document, page: usize, kind: MarkKind) -> Vec<String> {
    streams(doc, page).into_iter().filter(|s| s.contains(&format!("/PCMark /{}", kind.tag()))).collect()
}

fn apply_replacement(doc: &mut Document, kind: MarkKind, source: MarkSource, replace: bool) -> Result<(), EditError> {
    match kind {
        MarkKind::HeaderFooter => add_header_footer(
            doc,
            SELECTED,
            &HeaderFooter {
                text: ["New header".into(), String::new(), String::new(), String::new(), String::new(), String::new()],
                font: added::Family::Courier,
                shrink: true,
                ..Default::default()
            },
            replace,
            &cx(),
        ),
        MarkKind::Watermark => add_watermark(doc, SELECTED, &Watermark { source: Some(source), opacity: 0.25, ..Default::default() }, replace),
        MarkKind::Background => {
            add_background(doc, SELECTED, &Background { color: [0.1, 0.3, 0.7], replace_backdrop: true, ..Default::default() }, replace)
        }
    }
}

fn unchanged_after_later_page_failure(kind: MarkKind) {
    // Replacement and addition share the same operation boundary; exercise replacement first.
    for (replace, pending_edit) in [(true, false), (true, true), (false, false), (false, true)] {
        let (mut doc, source) = marked_document();
        if pending_edit {
            let pending = doc.add(Object::String(pdfcraft_cos::PdfString::literal(b"unsaved caller edit".to_vec())));
            doc.update_dict(page_list(&doc)[0].obj, |d| d.set(b"CallerEdit".to_vec(), Object::Ref(pending))).unwrap();
        }
        let modified = doc.is_modified();
        let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
        let objects = doc.object_numbers().len();
        let mut allocation_probe = doc.clone();
        let next_object = allocation_probe.add(Object::Null);
        let failure = PlacementFailure::after_two_pages(kind);
        let result = apply_replacement(&mut doc, kind, source, replace);
        assert_eq!(result, Err(EditError::Invalid("injected mark placement failure".into())));
        assert_eq!(failure.completed(), 2, "the error followed two real page placements");
        assert_eq!(doc.object_numbers().len(), objects, "{kind:?}, replace={replace}: no allocated objects remain");
        assert_eq!(doc.is_modified(), modified, "{kind:?}, replace={replace}: the dirty state is preserved");
        assert!(write_incremental(&doc, &SaveOptions::default()).unwrap() == before, "{kind:?}, replace={replace}: every byte stays unchanged");
        assert_eq!(doc.add(Object::Null), next_object, "the allocation cursor is unchanged too");
    }
}

#[test]
fn header_footer_changes_are_atomic_after_a_later_page_failure() {
    unchanged_after_later_page_failure(MarkKind::HeaderFooter);
}

#[test]
fn watermark_changes_are_atomic_after_a_later_page_failure() {
    unchanged_after_later_page_failure(MarkKind::Watermark);
}

#[test]
fn background_changes_are_atomic_after_a_later_page_failure() {
    unchanged_after_later_page_failure(MarkKind::Background);
}

#[test]
fn replacements_commit_all_selected_pages_and_preserve_other_marks_and_shared_resources() {
    for kind in [MarkKind::HeaderFooter, MarkKind::Watermark, MarkKind::Background] {
        let (mut doc, source) = marked_document();
        let before = doc.clone();
        let expected = match kind {
            MarkKind::HeaderFooter => "(New header)".to_string(),
            MarkKind::Watermark => format!("/PCPic{} Do", source.xobject.num),
            MarkKind::Background => "0.1 0.3 0.7 rg".to_string(),
        };
        apply_replacement(&mut doc, kind, source, true).unwrap();
        let doc = reopen(&doc);
        assert_eq!(streams(&doc, 3), streams(&before, 3), "the unselected page keeps its content");
        let unselected = page_list(&before)[3].obj;
        assert_eq!(doc.get(unselected), before.get(unselected), "the unselected page keeps its resources and entries");
        for reference in [ObjRef::new(6, 0), ObjRef::new(7, 0)] {
            assert_eq!(
                format!("{:?}", doc.get(reference)),
                format!("{:?}", before.get(reference)),
                "shared resources and original content stay unchanged"
            );
        }
        for page in SELECTED {
            let marks = mark_streams(&doc, *page, kind);
            assert_eq!(marks.len(), 1, "one replacement on selected page {page}");
            assert!(marks[0].contains(&expected), "the new mark is present: {}", marks[0]);
            let old_page = &page_list(&before)[*page];
            let new_page = &page_list(&doc)[*page];
            let old_resources = before.resolve(old_page.dict.get(b"Resources").unwrap());
            let new_resources = doc.resolve(new_page.dict.get(b"Resources").unwrap());
            for category in [&b"Font"[..], &b"ExtGState"[..], &b"XObject"[..]] {
                let old = before.resolve(old_resources.as_dict().unwrap().get(category).unwrap());
                let new = doc.resolve(new_resources.as_dict().unwrap().get(category).unwrap());
                for (name, value) in old.as_dict().unwrap().iter() {
                    assert_eq!(new.as_dict().unwrap().get(name), Some(value), "old resources remain usable on page {page}");
                    assert_eq!(doc.resolve(value), before.resolve(value), "referenced resource contents are unchanged on page {page}");
                }
            }
            for other in [MarkKind::HeaderFooter, MarkKind::Watermark, MarkKind::Background] {
                if other != kind {
                    assert_eq!(mark_streams(&doc, *page, other), mark_streams(&before, *page, other), "unrelated {other:?} on page {page}");
                }
            }
        }
    }
}
