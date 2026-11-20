use super::*;
use crate::{ObjectKind, Session};

/// A PDF of 300 × 300 pages, each drawing its `content` with `resources` and objects `extra`
/// (numbered after the pages' content streams).
fn pdf(pages: &[(&str, &str)], extra: &[String]) -> Arc<Vec<u8>> {
    let n = pages.len();
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + i)).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 300 300] >>", kids.join(" ")),
    ];
    for (i, (_, resources)) in pages.iter().enumerate() {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources {resources} >>", 3 + n + i));
    }
    for (content, _) in pages {
        objs.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
    }
    objs.extend(extra.iter().cloned());
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Arc::new(out)
}

const HELVETICA: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>";
const COURIER: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>";
/// A 2 × 2 RGB picture of the twelve bytes `data` (ASCII, so the file's lengths are its bytes).
fn picture(data: &str) -> String {
    assert!(data.is_ascii() && data.len() == 12);
    format!(
        "<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 12 >>\nstream\n{data}\nendstream"
    )
}

/// A one-page document whose paragraph "Copied" (blue, half opaque, Helvetica /F1 at objects 5)
/// and picture /Im0 (object 6) are copied; "Left" is not.
fn source() -> Arc<Vec<u8>> {
    let content = "0 0 1 rg /GS0 gs BT /F1 24 Tf 20 200 Td (Copied) Tj ET q 60 0 0 40 20 100 cm /Im0 Do Q BT /F1 12 Tf 200 40 Td (Left) Tj ET";
    pdf(
        &[(content, "<< /Font << /F1 5 0 R >> /XObject << /Im0 6 0 R >> /ExtGState << /GS0 << /ca 0.5 >> >> >>")],
        &[HELVETICA.into(), picture("\u{7f}\u{0}\u{0}\u{0}\u{7f}\u{0}\u{0}\u{0}\u{7f}\u{7f}\u{7f}\u{0}")],
    )
}

/// A one-page document that already draws with Courier as /F1 and has a picture of its own as /Im0.
fn target() -> Arc<Vec<u8>> {
    pdf(
        &[("BT /F1 10 Tf 250 280 Td (x) Tj ET q 10 0 0 10 280 5 cm /Im0 Do Q", "<< /Font << /F1 5 0 R >> /XObject << /Im0 6 0 R >> >>")],
        &[COURIER.into(), picture("@@@@@@@@@@@@")],
    )
}

fn copy_all_but_left(s: &Session, id: DocId) -> ObjectClip {
    let doc = s.get(id).unwrap();
    let targets: Vec<_> = doc.editable_objects(0).unwrap().into_iter().filter(|o| o.text.as_deref() != Some("Left")).map(|o| o.target).collect();
    assert_eq!(targets.len(), 2);
    doc.copy_objects(0, &targets).unwrap()
}

fn render(s: &Session, id: DocId) -> pdfcraft_render::RenderedPage {
    let doc = s.get(id).unwrap();
    let mut renderer = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), doc.config.clone());
    renderer.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() })
}

/// The pixels in user-space box `r` of a 300 pt page rendered at scale 1.
fn pixels(page: &pdfcraft_render::RenderedPage, r: [f64; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    for y in (300.0 - r[3]) as u32..(300.0 - r[1]) as u32 {
        for x in r[0] as u32..r[2] as u32 {
            let i = ((y * page.width + x) * 4) as usize;
            out.extend_from_slice(&page.rgba[i..i + 4]);
        }
    }
    out
}

fn images(s: &Session, id: DocId) -> Vec<Option<pdfcraft_cos::ObjRef>> {
    s.get(id).unwrap().page_images(0).into_iter().map(|i| i.object).collect()
}

#[test]
fn objects_copied_from_one_document_paste_into_another_and_draw_the_same() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let a = s.open("a.pdf", None, source(), None).unwrap();
    let b = s.open("b.pdf", None, target(), None).unwrap();
    let clip = copy_all_but_left(&s, a);
    assert_eq!(clip.text(), "Copied");
    let mut clipboard = ObjectClipboard::new(clip);
    // The source can change, or close: the copy is a snapshot.
    s.apply(a, Edit::delete_objects(0, vec![ObjectTarget { kind: ObjectKind::Image, index: 0 }]).unwrap()).unwrap();
    s.close(a);
    let a = s.open("a.pdf", None, source(), None).unwrap();

    let paste = clipboard.paste(s.get(b).unwrap(), 0).unwrap();
    assert!(matches!(&paste, Edit::PasteObjects { offset, .. } if *offset == [0.0, 0.0]), "another document: where it was");
    s.apply(b, paste).unwrap();
    assert_eq!(s.get(b).unwrap().can_undo(), Some("Paste"));
    let area = clipboard.clip().rect().map(|v| v.round());
    let area = [area[0] - 3.0, area[1] - 3.0, area[2] + 3.0, area[3] + 3.0];
    let (original, pasted) = (pixels(&render(&s, a), area), pixels(&render(&s, b), area));
    assert!(original.chunks(4).any(|p| p[2] > 200 && p[0] < 200), "the blue paragraph is drawn");
    assert_eq!(original, pasted, "the pasted paragraph and picture draw the same pixels");

    // Its own /F1 and /Im0 are left alone; what the copy uses came over under other names.
    let doc = s.get(b).unwrap();
    let texts: Vec<_> = doc.editable_objects(0).unwrap().into_iter().filter_map(|o| o.text).collect();
    assert!(texts.contains(&"Copied".to_string()) && !texts.contains(&"Left".to_string()), "{texts:?}");
    let pictures = images(&s, b);
    assert_eq!(pictures.len(), 2);
    assert_ne!(pictures[0], pictures[1]);

    // A second paste steps down and to the right, and shares the fonts and pictures the first
    // brought instead of copying them again.
    let paste = clipboard.paste(s.get(b).unwrap(), 0).unwrap();
    assert!(matches!(&paste, Edit::PasteObjects { offset, .. } if *offset == [18.0, -18.0]), "{paste:?}");
    s.apply(b, paste).unwrap();
    let pictures = images(&s, b);
    assert_eq!(pictures.len(), 3);
    assert_eq!(pictures[1], pictures[2], "one picture object for both pastes");

    // Saved and reopened, the pastes are there; undone, the page is as it was.
    let saved = s.save_bytes(b).unwrap();
    let mut again = Session::new();
    let reopened = again.open("b.pdf", None, saved, None).unwrap();
    assert_eq!(again.get(reopened).unwrap().page_images(0).len(), 3);
    s.undo(b).unwrap();
    s.undo(b).unwrap();
    assert_eq!(images(&s, b).len(), 1);
    assert_eq!(s.get(b).unwrap().can_undo(), None);
}

#[test]
fn a_paste_onto_the_page_it_came_from_steps_and_draws_from_the_same_objects() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let a = s.open("a.pdf", None, source(), None).unwrap();
    let mut clipboard = ObjectClipboard::new(copy_all_but_left(&s, a));
    let paste = clipboard.paste(s.get(a).unwrap(), 0).unwrap();
    assert!(matches!(&paste, Edit::PasteObjects { offset, .. } if *offset == [18.0, -18.0]), "one step down and right: {paste:?}");
    s.apply(a, paste).unwrap();
    let pictures = images(&s, a);
    assert_eq!(pictures.len(), 2);
    assert_eq!(pictures[0], pictures[1], "the same picture object, not a copy");
    // Far off the page (a copy from a larger page), a paste is centred instead.
    let doc = s.get(a).unwrap();
    let mut far = clipboard.clip().clone();
    far.copied.rect = [1000.0, 1000.0, 1100.0, 1050.0];
    assert_eq!(doc.object_paste_offset(0, &far, 0).unwrap(), [150.0 - 1050.0, 150.0 - 1025.0]);
}

#[test]
fn pasting_into_another_document_follows_cycles_and_refuses_past_its_limit() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    // A font that refers to itself and to an object that refers back.
    let cyclic = pdf(
        &[("BT /F1 20 Tf 20 200 Td (Loop) Tj ET", "<< /Font << /F1 5 0 R >> >>")],
        &[
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding /Self 5 0 R /Next [6 0 R] >>".into(),
            "<< /Back 5 0 R /Again 6 0 R >>".into(),
        ],
    );
    let a = s.open("cyclic.pdf", None, cyclic, None).unwrap();
    let b = s.open("b.pdf", None, target(), None).unwrap();
    let all: Vec<_> = s.get(a).unwrap().editable_objects(0).unwrap().into_iter().map(|o| o.target).collect();
    let mut clipboard = ObjectClipboard::new(s.get(a).unwrap().copy_objects(0, &all).unwrap());
    s.apply(b, clipboard.paste(s.get(b).unwrap(), 0).unwrap()).unwrap();
    assert!(s.get(b).unwrap().editable_objects(0).unwrap().iter().any(|o| o.text.as_deref() == Some("Loop")));

    // An ExtGState that drags in more objects than a paste copies is refused with a reason.
    let many = 20_001;
    let refs: Vec<String> = (0..many).map(|i| format!("{} 0 R", 6 + i)).collect();
    let mut extra = vec![format!("<< /ca 0.5 /Junk [{}] >>", refs.join(" "))];
    extra.extend((0..many).map(|i| format!("<< /N {i} >>")));
    let resources = format!("<< /Font << /F1 {HELVETICA} >> /ExtGState << /GS0 5 0 R >> >>");
    let big = pdf(&[("/GS0 gs BT /F1 20 Tf 20 200 Td (Big) Tj ET", &resources)], &extra);
    let c = s.open("big.pdf", None, big, None).unwrap();
    let all: Vec<_> = s.get(c).unwrap().editable_objects(0).unwrap().into_iter().map(|o| o.target).collect();
    assert_eq!(all.len(), 1);
    let mut clipboard = ObjectClipboard::new(s.get(c).unwrap().copy_objects(0, &all).unwrap());
    let before = s.get(b).unwrap().can_undo().map(str::to_string);
    let err = s.apply(b, clipboard.paste(s.get(b).unwrap(), 0).unwrap()).unwrap_err().to_string();
    assert!(err.contains("copy fewer objects"), "{err}");
    assert_eq!(s.get(b).unwrap().can_undo().map(str::to_string), before, "a refused paste changes nothing");
}

#[test]
fn copying_refuses_a_stale_selection_and_deleting_several_objects_is_one_step() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let a = s.open("a.pdf", None, source(), None).unwrap();
    let doc = s.get(a).unwrap();
    assert!(doc.copy_objects(0, &[ObjectTarget { kind: ObjectKind::Text, index: 7 }]).is_err());
    assert!(doc.copy_objects(3, &[ObjectTarget { kind: ObjectKind::Text, index: 0 }]).is_err());
    let delete = Edit::delete_objects(0, vec![ObjectTarget { kind: ObjectKind::Text, index: 0 }, ObjectTarget { kind: ObjectKind::Image, index: 0 }]);
    assert!(matches!(delete, Some(Edit::DeleteObjects { .. })));
    assert!(Edit::delete_objects(0, Vec::new()).is_none());
    s.apply(a, delete.unwrap()).unwrap();
    assert_eq!(s.get(a).unwrap().can_undo(), Some("Delete content"));
    assert!(s.get(a).unwrap().editable_objects(0).unwrap().iter().all(|o| o.text.as_deref() == Some("Left")));
}

/// Two pastes of a paragraph and a picture exactly over each other: deleting a paragraph also
/// removes a copy drawn exactly over it, so deleting all four objects together found the second
/// paragraph's number gone and refused the whole delete. Both copies go, in one step.
#[test]
fn objects_pasted_exactly_over_each_other_delete_together_in_one_step() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let a = s.open("a.pdf", None, source(), None).unwrap();
    let blank = s.create_blank(300.0, 300.0, 1).unwrap();
    let b = s.open_new("b.pdf", blank).unwrap();
    let mut clipboard = ObjectClipboard::new(copy_all_but_left(&s, a));
    let Edit::PasteObjects { clip, .. } = clipboard.paste(s.get(b).unwrap(), 0).unwrap() else { panic!("a paste") };
    for _ in 0..2 {
        s.apply(b, Edit::PasteObjects { page: 0, clip: Arc::clone(&clip), offset: [0.0, 0.0] }).unwrap();
    }
    let pasted = s.get(b).unwrap().editable_objects(0).unwrap();
    assert_eq!(pasted.iter().filter(|o| o.text.as_deref() == Some("Copied")).count(), 2, "{pasted:?}");
    assert_eq!(pasted.len(), 4);
    let delete = Edit::delete_objects(0, pasted.iter().map(|o| o.target).collect()).unwrap();
    s.apply(b, delete).unwrap();
    assert!(s.get(b).unwrap().editable_objects(0).unwrap().is_empty());
    assert_eq!(s.get(b).unwrap().can_undo(), Some("Delete content"));
    s.undo(b).unwrap();
    assert_eq!(s.get(b).unwrap().editable_objects(0).unwrap(), pasted, "one undo brings both copies back");
}

/// Deleting renumbers what follows in the same inventory, so paragraphs and pictures go from
/// the last to the first, and added content (outside those inventories) after them.
#[test]
fn several_objects_delete_last_first() {
    let content =
        "BT /F1 12 Tf 20 250 Td (A) Tj ET BT /F1 12 Tf 20 180 Td (B) Tj ET BT /F1 12 Tf 20 110 Td (C) Tj ET BT /F1 12 Tf 20 40 Td (D) Tj ET";
    let mut doc = pdfcraft_cos::Document::open(pdf(&[(content, "<< /Font << /F1 5 0 R >> >>")], &[HELVETICA.into()])).unwrap();
    let t = |kind, index| ObjectTarget { kind, index };
    let targets = [t(ObjectKind::Added, 0), t(ObjectKind::Text, 1), t(ObjectKind::Image, 0), t(ObjectKind::Text, 3), t(ObjectKind::Added, 2)];
    let mut order = Vec::new();
    delete(&mut doc, 0, &targets, |_, e| {
        order.push(match e {
            Edit::EditTextBlock { block, text, .. } if text.is_empty() => ('t', *block),
            Edit::EditPageImage { index, change: crate::ImageEdit::Delete, .. } => ('i', *index),
            Edit::DeleteContent { index, .. } => ('a', *index),
            other => panic!("unexpected {other:?}"),
        });
        Ok(())
    })
    .unwrap();
    assert_eq!(order, [('t', 3), ('t', 1), ('i', 0), ('a', 2), ('a', 0)]);
}

/// #4648: Copy/Paste and Cut/Undo of objects on pages with live layer toggles follow the active
fn layered_clip_fixture(initially_on: bool, marked: bool, form: bool) -> Arc<Vec<u8>> {
    let state = if initially_on { "ON" } else { "OFF" };
    let wrapped = if marked { "/OC /L BDC q 20 0 0 20 10 10 cm /First Do Q EMC" } else { "q 20 0 0 20 10 10 cm /First Do Q" };
    let oc = if marked { "" } else { "/OC 7 0 R" };
    let first_obj = if form {
        let body = "1 0 0 rg 0 0 1 1 re f";
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << >> {oc} /Length {} >>\nstream\n{body}\nendstream", body.len())
    } else {
        format!(
            "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 {oc} /Length 3 >>\nstream\n\u{7f}\u{0}\u{0}\nendstream"
        )
    };
    let second_obj = "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 3 >>\nstream\n\u{0}\u{7f}\u{0}\nendstream";
    let bytes = pdf(
        &[
            (&format!("{wrapped} q 20 0 0 20 60 60 cm /Second Do Q"), "<< /Properties << /L 7 0 R >> /XObject << /First 8 0 R /Second 9 0 R >> >>"),
            ("", "<< >>"),
        ],
        &["<< /Type /OCG /Name (Switchable) >>".into(), first_obj, second_obj.into()],
    );
    // Attach /OCProperties to the catalog (object 1).
    let mut cos = pdfcraft_cos::Document::open(bytes).unwrap();
    cos.update_dict(pdfcraft_cos::ObjRef::new(1, 0), |cat| {
        let ocg = pdfcraft_cos::Object::Ref(pdfcraft_cos::ObjRef::new(7, 0));
        let d = pdfcraft_cos::Dict::from_iter([
            (state.as_bytes().to_vec(), pdfcraft_cos::Object::Array(vec![ocg.clone()])),
            (b"Order".to_vec(), pdfcraft_cos::Object::Array(vec![ocg.clone()])),
        ]);
        let ocp = pdfcraft_cos::Dict::from_iter([
            (b"OCGs".to_vec(), pdfcraft_cos::Object::Array(vec![ocg])),
            (b"D".to_vec(), pdfcraft_cos::Object::Dict(d)),
        ]);
        cat.set(b"OCProperties".to_vec(), pdfcraft_cos::Object::Dict(ocp));
    })
    .unwrap();
    Arc::new(pdfcraft_cos::write_full(&cos, &pdfcraft_cos::SaveOptions::default()).unwrap())
}

fn verify_initially_on_hidden_clip(marked: bool, form: bool) {
    let raw = layered_clip_fixture(true, marked, form);
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("layers.pdf", None, raw, None).unwrap();
    let initial_p0 = s.get(id).unwrap().page_images(0);
    assert_eq!(initial_p0.len(), 2);
    let second_ref = initial_p0[1].object.unwrap();
    assert!(s.set_layer_visible(id, 0, false));

    // Preceding layer was turned OFF: index 0 is /Second, index 1 does not exist.
    let img0 = ObjectTarget { kind: ObjectKind::Image, index: 0 };
    let img1 = ObjectTarget { kind: ObjectKind::Image, index: 1 };
    let objects = s.get(id).unwrap().editable_objects(0).unwrap();
    assert_eq!(objects.len(), 1);
    let clip = s.get(id).unwrap().copy_objects(0, &[img0]).unwrap();
    assert_eq!(clip.rect(), [60.0, 60.0, 80.0, 80.0]);
    assert!(s.get(id).unwrap().copy_objects(0, &[img1]).is_err());

    // Copy/Paste captures /Second onto blank page 1.
    let mut cb = ObjectClipboard::new(clip);
    let paste_edit = cb.paste(s.get(id).unwrap(), 1).unwrap();
    s.apply(id, paste_edit).unwrap();
    let p1_imgs: Vec<_> = s.get(id).unwrap().page_images(1).into_iter().map(|i| (i.object, i.rect)).collect();
    assert_eq!(p1_imgs, [(Some(second_ref), [60.0, 60.0, 80.0, 80.0])]);

    // Cut (/Second) + Undo must remove /Second and keep hidden /First untouched.
    s.apply(id, Edit::delete_objects(0, vec![img0]).unwrap()).unwrap();
    assert!(s.get(id).unwrap().page_images(0).is_empty());
    assert!(s.set_layer_visible(id, 0, true));
    assert_eq!(s.get(id).unwrap().page_images(0).into_iter().map(|i| i.name).collect::<Vec<_>>(), ["First"]);
    assert!(s.set_layer_visible(id, 0, false));
    s.undo(id).unwrap();
    assert!(s.reset_layers(id));
    assert_eq!(s.get(id).unwrap().page_images(0).into_iter().map(|i| i.name).collect::<Vec<_>>(), ["First", "Second"]);
}

fn verify_initially_off_shown_clip(marked: bool, form: bool) {
    let raw = layered_clip_fixture(false, marked, form);
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("layers.pdf", None, raw, None).unwrap();
    assert!(s.set_layer_visible(id, 0, true));
    let initial_p0 = s.get(id).unwrap().page_images(0);
    assert_eq!(initial_p0.len(), 2);
    let first_ref = initial_p0[0].object.unwrap();
    let second_ref = initial_p0[1].object.unwrap();

    // Initially-off layer was turned ON: index 0 is /First, index 1 is /Second.
    let img0 = ObjectTarget { kind: ObjectKind::Image, index: 0 };
    let img1 = ObjectTarget { kind: ObjectKind::Image, index: 1 };
    let objects = s.get(id).unwrap().editable_objects(0).unwrap();
    assert_eq!(objects.len(), 2);
    let clip0 = s.get(id).unwrap().copy_objects(0, &[img0]).unwrap();
    assert_eq!(clip0.rect(), [10.0, 10.0, 30.0, 30.0]);
    let clip1 = s.get(id).unwrap().copy_objects(0, &[img1]).unwrap();
    assert_eq!(clip1.rect(), [60.0, 60.0, 80.0, 80.0]);

    // Copy/Paste /First onto blank page 1 draws red artwork at [10, 10, 30, 30], while /Second pastes /Second.
    let mut cb0 = ObjectClipboard::new(clip0);
    let paste0 = cb0.paste(s.get(id).unwrap(), 1).unwrap();
    s.apply(id, paste0).unwrap();
    let p1_imgs: Vec<_> = s.get(id).unwrap().page_images(1).into_iter().map(|i| (i.object, i.rect)).collect();
    assert_eq!(p1_imgs, [(Some(first_ref), [10.0, 10.0, 30.0, 30.0])]);
    if form {
        let doc = s.get(id).unwrap();
        let mut renderer = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), doc.config.clone());
        let page1 = renderer.render(pdfcraft_render::RenderRequest { page: 1, scale: 1.0, ..Default::default() });
        assert!(pixels(&page1, [15.0, 15.0, 25.0, 25.0]).as_chunks::<4>().0.iter().all(|p| p[0] > 200 && p[1] < 30 && p[2] < 30));
    }
    let mut cb1 = ObjectClipboard::new(clip1);
    let paste1 = cb1.paste(s.get(id).unwrap(), 1).unwrap();
    s.apply(id, paste1).unwrap();
    assert_eq!(
        s.get(id).unwrap().page_images(1).into_iter().map(|i| (i.object, i.rect)).collect::<Vec<_>>(),
        [(Some(first_ref), [10.0, 10.0, 30.0, 30.0]), (Some(second_ref), [60.0, 60.0, 80.0, 80.0])]
    );

    // Cut /First (index 0) leaves /Second intact, and Undo restores /First before /Second.
    s.apply(id, Edit::delete_objects(0, vec![img0]).unwrap()).unwrap();
    assert_eq!(s.get(id).unwrap().page_images(0).into_iter().map(|i| i.name).collect::<Vec<_>>(), ["Second"]);
    s.undo(id).unwrap();
    assert_eq!(s.get(id).unwrap().page_images(0).into_iter().map(|i| i.name).collect::<Vec<_>>(), ["First", "Second"]);
}

/// Regression: copy, paste, and cut/undo preserve live-index targeting according to the active
/// session layer visibility (not stale `/OCProperties /D`), across BDC and XObject `/OC` as well
/// as raster images and Form artwork.
#[test]
fn copy_paste_and_cut_undo_follow_live_layer_visibility() {
    for marked in [false, true] {
        for form in [false, true] {
            verify_initially_on_hidden_clip(marked, form);
            verify_initially_off_shown_clip(marked, form);
        }
    }
}
