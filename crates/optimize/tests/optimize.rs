//! The optimizer on documents with images drawn at known sizes.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};
use pdfcraft_optimize::{Compression, ImageSettings, Settings, SpaceCategory, audit_space, effective_resolutions, optimize};

fn image(doc: &mut Document, w: u32, h: u32, n: usize, jpeg: bool, smask: Option<ObjRef>) -> ObjRef {
    // A smooth gradient with a little texture (photo-like).
    let mut px = Vec::with_capacity((w * h) as usize * n);
    for y in 0..h {
        for x in 0..w {
            let v = ((x * 255 / w.max(1)) as u8).wrapping_add(((x ^ y) & 7) as u8);
            px.push(v);
            if n == 3 {
                px.push((y * 255 / h.max(1)) as u8);
                px.push(128);
            }
        }
    }
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(w as i64));
    d.set(b"Height".to_vec(), Object::Int(h as i64));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    d.set(b"ColorSpace".to_vec(), Object::name(if n == 1 { "DeviceGray" } else { "DeviceRGB" }));
    if let Some(m) = smask {
        d.set(b"SMask".to_vec(), Object::Ref(m));
    }
    let s = if jpeg {
        let mut out = Vec::new();
        let ty = if n == 1 { image::ExtendedColorType::L8 } else { image::ExtendedColorType::Rgb8 };
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 98).encode(&px, w, h, ty).unwrap();
        d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
        Stream::from_raw(d, out)
    } else {
        Stream::flate(d, &px)
    };
    doc.add(Object::Stream(s))
}

fn page(doc: &mut Document, xobjects: &[(&str, ObjRef)], content: &str) -> ObjRef {
    let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
    let mut x = Dict::new();
    for (n, r) in xobjects {
        x.set(n.as_bytes().to_vec(), Object::Ref(*r));
    }
    let mut res = Dict::new();
    res.set(b"XObject".to_vec(), Object::Dict(x));
    let c = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content.as_bytes().to_vec())));
    let mut p = Dict::new();
    p.set(b"Type".to_vec(), Object::name("Page"));
    p.set(b"Parent".to_vec(), Object::Ref(pages));
    p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]));
    p.set(b"Resources".to_vec(), Object::Dict(res));
    p.set(b"Contents".to_vec(), Object::Ref(c));
    p.set(b"Thumb".to_vec(), Object::Ref(c));
    let r = doc.add(Object::Dict(p));
    doc.update_dict(pages, |d| {
        let mut kids = d.get(b"Kids").and_then(|k| k.as_array().cloned()).unwrap_or_default();
        kids.push(Object::Ref(r));
        d.set(b"Count".to_vec(), Object::Int(kids.len() as i64));
        d.set(b"Kids".to_vec(), Object::Array(kids));
    })
    .unwrap();
    r
}

fn stream(doc: &Document, r: ObjRef) -> Stream {
    match &*doc.get(r) {
        Object::Stream(s) => s.clone(),
        _ => panic!("not a stream"),
    }
}

#[test]
fn images_are_measured_where_drawn_and_downsampled() {
    let mut doc = Document::new_empty();
    // 1200 px drawn 144 pt (2 in) wide → 600 ppi.
    let big = image(&mut doc, 1200, 1200, 3, false, None);
    // 100 px at 144 pt → 50 ppi: left at its size.
    let small = image(&mut doc, 100, 100, 1, false, None);
    // With a soft mask, 800 px at 72 pt → 800 ppi.
    let mask = image(&mut doc, 800, 800, 1, false, None);
    let masked = image(&mut doc, 800, 800, 3, false, Some(mask));
    // A high-quality JPEG at 72 ppi: recompressed, not resized.
    let photo = image(&mut doc, 600, 400, 3, true, None);
    // Drawn twice: 300 ppi and 75 ppi → effective 75 (never downsampled below what a use needs).
    let twice = image(&mut doc, 300, 300, 3, false, None);
    page(&mut doc, &[("Im1", big), ("Im2", small)], "q 144 0 0 144 36 600 cm /Im1 Do Q q 144 0 0 144 300 600 cm /Im2 Do Q");
    page(
        &mut doc,
        &[("A", masked), ("B", photo), ("C", twice)],
        "q 72 0 0 72 36 36 cm /A Do Q q 1 0 0 1 100 100 cm 600 0 0 400 0 0 cm /B Do Q q 72 0 0 72 0 0 cm /C Do Q q 288 0 0 288 300 300 cm /C Do Q",
    );
    let pages = pdfcraft_annot::page_refs(&doc).unwrap();
    let ppi = effective_resolutions(&doc, &pages);
    assert_eq!(ppi[&big].round(), 600.0);
    assert_eq!(ppi[&small].round(), 50.0);
    assert_eq!(ppi[&masked].round(), 800.0);
    assert_eq!(ppi[&photo].round(), 72.0);
    assert_eq!(ppi[&twice].round(), 75.0, "the largest use decides");

    let before = write_full(&doc, &SaveOptions::default()).unwrap().len();
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(report.images, 5, "{report:?}");
    assert_eq!(report.images_resampled, 2, "{report:?}");
    assert!(report.thumbnails == 2);
    let b = stream(&doc, big);
    assert_eq!((b.dict.int(b"Width"), b.dict.int(b"Height"), b.dict.name(b"Filter")), (Some(300), Some(300), Some(&b"DCTDecode"[..])));
    let m = stream(&doc, masked);
    let sm = stream(&doc, mask);
    assert_eq!((m.dict.int(b"Width"), sm.dict.int(b"Width")), (Some(150), Some(150)), "the soft mask follows its image");
    assert_eq!(sm.dict.name(b"Filter"), Some(&b"FlateDecode"[..]));
    assert_eq!(stream(&doc, small).dict.int(b"Width"), Some(100));
    assert_eq!(stream(&doc, twice).dict.int(b"Width"), Some(300), "75 ppi is below the threshold");
    let p = stream(&doc, photo);
    assert_eq!(p.dict.int(b"Width"), Some(600));
    let after_bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(after_bytes.len() * 3 < before, "{} → {}", before, after_bytes.len());

    // The optimized file opens, and the page still shows the picture where it was.
    let reopened = Document::open(Arc::new(after_bytes.clone())).unwrap();
    assert_eq!(pdfcraft_annot::page_refs(&reopened).unwrap().len(), 2);
    let mut r = pdfcraft_render::PageRenderer::new(Arc::new(after_bytes), pdfcraft_render::RenderConfig::default());
    let out = r.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    // Top-left image area (36..180 x 600..744 user → y from the top 48..192): drawn, not white.
    let i = ((100 * out.width + 100) * 4) as usize;
    assert_ne!(&out.rgba[i..i + 3], &[255, 255, 255]);
}

#[test]
fn an_identity_decode_array_does_not_stop_reduce() {
    // Office scanners write /Decode [0 1 0 1 0 1] (no change) on their page images (#490).
    let with_decode = |doc: &mut Document, r: ObjRef, decode: Vec<Object>| {
        let mut s = stream(doc, r);
        s.dict.set(b"Decode".to_vec(), Object::Array(decode));
        doc.set(r, Object::Stream(s));
    };
    let mut doc = Document::new_empty();
    // 900 px drawn 216 pt (3 in) wide → 300 ppi, like a scanned page.
    let scan = image(&mut doc, 900, 900, 3, true, None);
    with_decode(&mut doc, scan, [0, 1, 0, 1, 0, 1].map(Object::Int).to_vec());
    let gray = image(&mut doc, 900, 900, 1, false, None);
    with_decode(&mut doc, gray, vec![Object::Real(0.0), Object::Real(1.0)]);
    // A decode array that inverts the colours, and one of the wrong length: left alone.
    let inverted = image(&mut doc, 900, 900, 3, true, None);
    with_decode(&mut doc, inverted, [1, 0, 1, 0, 1, 0].map(Object::Int).to_vec());
    let odd = image(&mut doc, 900, 900, 3, true, None);
    with_decode(&mut doc, odd, [0, 1].map(Object::Int).to_vec());
    page(
        &mut doc,
        &[("S", scan), ("G", gray), ("I", inverted), ("O", odd)],
        "q 216 0 0 216 0 0 cm /S Do Q q 216 0 0 216 216 0 cm /G Do Q q 216 0 0 216 0 300 cm /I Do Q q 216 0 0 216 216 300 cm /O Do Q",
    );
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(report.images_resampled, 2, "{report:?}");
    assert_eq!(stream(&doc, scan).dict.int(b"Width"), Some(450));
    assert_eq!(stream(&doc, gray).dict.int(b"Width"), Some(450));
    assert!(!stream(&doc, scan).dict.contains(b"Decode"), "the default decode array isn't carried over");
    assert_eq!(stream(&doc, inverted).dict.int(b"Width"), Some(900));
    assert_eq!(stream(&doc, odd).dict.int(b"Width"), Some(900));
}

#[test]
fn settings_choose_what_happens() {
    let mut doc = Document::new_empty();
    let big = image(&mut doc, 1200, 600, 1, false, None);
    page(&mut doc, &[("Im1", big)], "q 144 0 0 72 36 600 cm /Im1 Do Q");
    let mut keep = doc.clone();
    // Off: nothing changes (but the thumbnail goes).
    let off = ImageSettings { downsample: false, target_ppi: 150.0, above_ppi: 225.0, compression: Compression::Retain };
    let r = optimize(&mut keep, &Settings { color: off, gray: off, ..Settings::default() }).unwrap();
    assert_eq!((r.images_resampled, r.images_recompressed), (0, 0));
    // Lossless: resampled to 300 ppi, Flate.
    let lossless = ImageSettings { downsample: true, target_ppi: 300.0, above_ppi: 450.0, compression: Compression::Flate };
    let r = optimize(&mut doc, &Settings { gray: lossless, ..Settings::default() }).unwrap();
    assert_eq!(r.images_resampled, 1);
    let s = stream(&doc, big);
    assert_eq!((s.dict.int(b"Width"), s.dict.int(b"Height"), s.dict.name(b"Filter")), (Some(600), Some(300), Some(&b"FlateDecode"[..])));
}

#[test]
fn discards_and_clean_up() {
    let mut doc = Document::new_empty();
    let pages = page(&mut doc, &[], "BT /F1 12 Tf 72 720 Td (Hello world, hello world, hello world, hello world) Tj ET");
    let root = doc.root().unwrap();
    let mut vp = Dict::new();
    vp.set(b"PrintScaling".to_vec(), Object::name("None"));
    vp.set(b"Duplex".to_vec(), Object::name("Simplex"));
    vp.set(b"HideToolbar".to_vec(), Object::Bool(true));
    let st = doc.add(Object::Dict(Dict::new()));
    doc.update_dict(root, |c| {
        c.set(b"ViewerPreferences".to_vec(), Object::Dict(vp));
        c.set(b"StructTreeRoot".to_vec(), Object::Ref(st));
        c.set(b"MarkInfo".to_vec(), Object::Dict(Dict::new()));
    })
    .unwrap();
    doc.update_dict(pages, |p| p.set(b"StructParents".to_vec(), Object::Int(0))).unwrap();
    let settings = Settings { discard_tags: true, discard_print_settings: true, ..Settings::default() };
    let r = optimize(&mut doc, &settings).unwrap();
    assert!(r.tags_removed);
    assert_eq!((r.print_settings, r.thumbnails, r.streams_compressed), (2, 1, 1));
    let c = doc.get(root).as_dict().cloned().unwrap();
    assert!(!c.contains(b"StructTreeRoot") && !c.contains(b"MarkInfo"));
    let vp = c.get(b"ViewerPreferences").and_then(|v| v.as_dict().cloned()).unwrap();
    assert!(vp.contains(b"HideToolbar") && !vp.contains(b"Duplex"), "other preferences stay");
}

#[test]
fn space_audit_shares_the_file_out_by_kind() {
    let mut doc = Document::new_empty();
    let img = image(&mut doc, 300, 300, 3, false, None);
    page(&mut doc, &[("Im0", img)], "q 300 0 0 300 0 0 cm /Im0 Do Q BT ET");
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    let doc = Document::open(Arc::new(bytes.clone())).unwrap();
    let audit = audit_space(&doc, bytes.len() as u64);
    assert_eq!(audit.len(), SpaceCategory::ALL.len());
    let get = |c: SpaceCategory| audit.iter().find(|u| u.category == c).unwrap().clone();
    let (images, content, overhead) = (get(SpaceCategory::Images), get(SpaceCategory::ContentStreams), get(SpaceCategory::DocumentOverhead));
    assert!(images.percent > 80.0, "{audit:?}");
    assert!(content.bytes > 0 && overhead.bytes > 0, "{audit:?}");
    assert_eq!(audit.iter().map(|u| u.bytes).sum::<u64>(), bytes.len() as u64);
    assert!((audit.iter().map(|u| u.percent).sum::<f64>() - 100.0).abs() < 1e-6);
}

#[test]
fn space_audit_counts_images_a_content_stream_dictionary_points_at_as_images() {
    // Content added with Edit ▸ Add content records its image in the content stream's
    // dictionary (/PCAdded); the image is still an image, not part of the content stream.
    let mut doc = Document::new_empty();
    let img = image(&mut doc, 300, 300, 3, false, None);
    let p = page(&mut doc, &[("Im0", img)], "q 300 0 0 300 0 0 cm /Im0 Do Q");
    let c = doc.get(p).as_dict().and_then(|d| d.reference(b"Contents")).unwrap();
    let mut added = Dict::new();
    added.set(b"Kind".to_vec(), Object::name("Image"));
    added.set(b"Image".to_vec(), Object::Ref(img));
    doc.update_dict(c, |d| d.set(b"PCAdded".to_vec(), Object::Dict(added))).unwrap();
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    let doc = Document::open(Arc::new(bytes.clone())).unwrap();
    let audit = audit_space(&doc, bytes.len() as u64);
    let images = audit.iter().find(|u| u.category == SpaceCategory::Images).unwrap();
    assert!(images.percent > 80.0, "{audit:?}");
}

#[test]
fn space_audit_counts_what_fields_and_comments_draw_with_by_kind() {
    // A font or image a form field or comment draws with is still a font or image. The form
    // was claimed first, so its fonts made a fillable form look like mostly "Forms".
    let mut doc = Document::new_empty();
    let p = page(&mut doc, &[], "BT ET");
    let font = |doc: &mut Document| {
        let file = doc.add(Object::Stream(Stream::from_raw(Dict::new(), vec![7; 40_000])));
        let mut desc = Dict::new();
        desc.set(b"Type".to_vec(), Object::name("FontDescriptor"));
        desc.set(b"FontFile2".to_vec(), Object::Ref(file));
        let desc = doc.add(Object::Dict(desc));
        let mut f = Dict::new();
        f.set(b"Type".to_vec(), Object::name("Font"));
        f.set(b"Subtype".to_vec(), Object::name("TrueType"));
        f.set(b"FontDescriptor".to_vec(), Object::Ref(desc));
        doc.add(Object::Dict(f))
    };
    let (field_font, comment_font) = (font(&mut doc), font(&mut doc));
    let icon = image(&mut doc, 200, 200, 3, false, None);
    let appearance = |doc: &mut Document, kind: &str, name: &str, r: ObjRef| {
        let mut entries = Dict::new();
        entries.set(name.as_bytes().to_vec(), Object::Ref(r));
        let mut res = Dict::new();
        res.set(kind.as_bytes().to_vec(), Object::Dict(entries));
        let mut d = Dict::new();
        d.set(b"Subtype".to_vec(), Object::name("Form"));
        d.set(b"Resources".to_vec(), Object::Dict(res));
        Object::Ref(doc.add(Object::Stream(Stream::from_raw(d, b"q Q".to_vec()))))
    };
    let annot = |doc: &mut Document, subtype: &str, ap: Object| {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Annot"));
        d.set(b"Subtype".to_vec(), Object::name(subtype));
        d.set(b"Rect".to_vec(), Object::Array(vec![0.into(), 0.into(), 10.into(), 10.into()]));
        d.set(b"P".to_vec(), Object::Ref(p));
        let mut aps = Dict::new();
        aps.set(b"N".to_vec(), ap);
        d.set(b"AP".to_vec(), Object::Dict(aps));
        Object::Ref(doc.add(Object::Dict(d)))
    };
    // A text field's appearance uses the form's default font; a check box's "on" state (one of
    // a dictionary of states) draws an image; a free-text comment has a font of its own.
    let text_ap = appearance(&mut doc, "Font", "F1", field_font);
    let text = annot(&mut doc, "Widget", text_ap);
    let mut states = Dict::new();
    states.set(b"On".to_vec(), appearance(&mut doc, "XObject", "Im0", icon));
    let check = annot(&mut doc, "Widget", Object::Dict(states));
    let comment_ap = appearance(&mut doc, "Font", "F2", comment_font);
    let comment = annot(&mut doc, "FreeText", comment_ap);
    doc.update_dict(p, |d| d.set(b"Annots".to_vec(), Object::Array(vec![text.clone(), check.clone(), comment]))).unwrap();
    let mut fonts = Dict::new();
    fonts.set(b"F1".to_vec(), Object::Ref(field_font));
    let mut dr = Dict::new();
    dr.set(b"Font".to_vec(), Object::Dict(fonts));
    let mut form = Dict::new();
    form.set(b"Fields".to_vec(), Object::Array(vec![text, check]));
    form.set(b"DR".to_vec(), Object::Dict(dr));
    let root = doc.root().unwrap();
    doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(form))).unwrap();
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    let doc = Document::open(Arc::new(bytes.clone())).unwrap();
    let audit = audit_space(&doc, bytes.len() as u64);
    let get = |c: SpaceCategory| audit.iter().find(|u| u.category == c).unwrap().bytes;
    assert!(get(SpaceCategory::Fonts) > 80_000, "both fonts are fonts: {audit:?}");
    assert!(get(SpaceCategory::Images) > 2_000, "the check box's image is an image: {audit:?}");
    assert!(get(SpaceCategory::Forms) < 2_000 && get(SpaceCategory::Comments) < 1_000, "{audit:?}");
    assert!(get(SpaceCategory::Forms) > 0 && get(SpaceCategory::Comments) > 0, "the fields and the comment are still counted: {audit:?}");
}

#[test]
fn space_audit_survives_deeply_nested_forms() {
    // Each form draws the next; sorting their resources by kind recursed once per form, so a
    // long enough chain overflowed the stack.
    let mut doc = Document::new_empty();
    let mut next: Option<ObjRef> = None;
    for _ in 0..20_000 {
        let mut d = Dict::new();
        d.set(b"Subtype".to_vec(), Object::name("Form"));
        if let Some(n) = next {
            let mut x = Dict::new();
            x.set(b"X".to_vec(), Object::Ref(n));
            let mut res = Dict::new();
            res.set(b"XObject".to_vec(), Object::Dict(x));
            d.set(b"Resources".to_vec(), Object::Dict(res));
        }
        next = Some(doc.add(Object::Stream(Stream::from_raw(d, b"/X Do".to_vec()))));
    }
    page(&mut doc, &[("X", next.unwrap())], "/X Do");
    let audit = audit_space(&doc, 0);
    let forms = audit.iter().find(|u| u.category == SpaceCategory::XObjectForms).unwrap();
    assert!(forms.bytes > 20_000 * 40, "every form is counted, however deep: {audit:?}");
}

#[test]
fn invalid_links_and_unreferenced_destinations_go() {
    let mut doc = Document::new_empty();
    let p = page(&mut doc, &[], "BT ET");
    let link = |doc: &mut Document, dest: Object| {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Annot"));
        d.set(b"Subtype".to_vec(), Object::name("Link"));
        d.set(b"Rect".to_vec(), Object::Array(vec![0.into(), 0.into(), 10.into(), 10.into()]));
        d.set(b"Dest".to_vec(), dest);
        Object::Ref(doc.add(Object::Dict(d)))
    };
    let s = |t: &str| Object::String(pdfcraft_cos::PdfString::literal(t.as_bytes().to_vec()));
    let annots = vec![
        link(&mut doc, Object::Array(vec![Object::Ref(p), Object::name("Fit")])),
        link(&mut doc, Object::Array(vec![Object::Ref(ObjRef::new(999, 0)), Object::name("Fit")])),
        link(&mut doc, s("gone")),
        link(&mut doc, s("kept")),
    ];
    doc.update_dict(p, |d| d.set(b"Annots".to_vec(), Object::Array(annots))).unwrap();
    // Named destinations "kept" (used) and "unused"; a bookmark to a missing name.
    let fit = Object::Array(vec![Object::Ref(p), Object::name("Fit")]);
    let mut tree = Dict::new();
    tree.set(b"Names".to_vec(), Object::Array(vec![s("kept"), fit.clone(), s("unused"), fit]));
    let tree = doc.add(Object::Dict(tree));
    let mut names = Dict::new();
    names.set(b"Dests".to_vec(), Object::Ref(tree));
    let outlines = doc.add(Object::Null);
    let mut item = Dict::new();
    item.set(b"Title".to_vec(), s("Lost"));
    item.set(b"Parent".to_vec(), Object::Ref(outlines));
    item.set(b"Dest".to_vec(), s("nowhere"));
    let item = doc.add(Object::Dict(item));
    let mut o = Dict::new();
    o.set(b"First".to_vec(), Object::Ref(item));
    o.set(b"Last".to_vec(), Object::Ref(item));
    doc.set(outlines, Object::Dict(o));
    let root = doc.root().unwrap();
    doc.update_dict(root, |c| {
        c.set(b"Names".to_vec(), Object::Dict(names));
        c.set(b"Outlines".to_vec(), Object::Ref(outlines));
    })
    .unwrap();
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!((report.invalid_links, report.invalid_bookmarks, report.unreferenced_dests), (2, 1, 1));
    let left = doc.get(p).as_dict().unwrap().get(b"Annots").unwrap().as_array().unwrap().len();
    assert_eq!(left, 2);
    assert!(doc.get(item).as_dict().unwrap().get(b"Dest").is_none(), "the bookmark stays, without its broken destination");
    // Off: nothing changes.
    let mut again = doc.clone();
    let off = Settings { remove_invalid_links: false, remove_unreferenced_dests: false, ..Settings::default() };
    assert_eq!(optimize(&mut again, &off).unwrap().invalid_links, 0);
}

#[test]
fn progress_is_reported_per_image_and_a_refusal_cancels() {
    use pdfcraft_optimize::{OptimizeError, Stage, optimize_with_progress};
    let build = || {
        let mut doc = Document::new_empty();
        let a = image(&mut doc, 300, 300, 3, false, None);
        let b = image(&mut doc, 300, 300, 1, false, None);
        let c = image(&mut doc, 300, 300, 3, true, None);
        page(&mut doc, &[("A", a), ("B", b), ("C", c)], "q 72 0 0 72 0 0 cm /A Do Q q 72 0 0 72 100 0 cm /B Do Q q 72 0 0 72 200 0 cm /C Do Q");
        doc
    };
    let mut seen = Vec::new();
    let report = optimize_with_progress(&mut build(), &Settings::default(), &mut |s| {
        seen.push(s);
        true
    })
    .unwrap();
    assert_eq!(report.images, 3);
    assert_eq!(
        seen,
        [
            Stage::Images { done: 0, total: 3 },
            Stage::Images { done: 1, total: 3 },
            Stage::Images { done: 2, total: 3 },
            Stage::Images { done: 3, total: 3 },
            Stage::CleanUp
        ]
    );

    // Stopping after the first image.
    let mut calls = 0;
    let r = optimize_with_progress(&mut build(), &Settings::default(), &mut |_| {
        calls += 1;
        calls < 2
    });
    assert!(matches!(r, Err(OptimizeError::Cancelled)), "{r:?}");
    assert_eq!(calls, 2, "nothing runs after the refusal");
}

/// A page with `resources` and, if given, `contents`.
fn raw_page(doc: &mut Document, resources: Object, contents: Option<ObjRef>) -> ObjRef {
    let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
    let mut p = Dict::new();
    p.set(b"Type".to_vec(), Object::name("Page"));
    p.set(b"Parent".to_vec(), Object::Ref(pages));
    p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]));
    p.set(b"Resources".to_vec(), resources);
    if let Some(c) = contents {
        p.set(b"Contents".to_vec(), Object::Ref(c));
    }
    let r = doc.add(Object::Dict(p));
    doc.update_dict(pages, |d| {
        let mut kids = d.get(b"Kids").and_then(|k| k.as_array().cloned()).unwrap_or_default();
        kids.push(Object::Ref(r));
        d.set(b"Count".to_vec(), Object::Int(kids.len() as i64));
        d.set(b"Kids".to_vec(), Object::Array(kids));
    })
    .unwrap();
    r
}

/// Builders for the tests below.
fn xobjects(pairs: &[(&str, ObjRef)]) -> Dict {
    let mut x = Dict::new();
    for (n, r) in pairs {
        x.set(n.as_bytes().to_vec(), Object::Ref(*r));
    }
    x
}

fn resources(x: Dict) -> Dict {
    let mut res = Dict::new();
    res.set(b"XObject".to_vec(), Object::Dict(x));
    res
}

fn content(doc: &mut Document, c: &str) -> ObjRef {
    doc.add(Object::Stream(Stream::flate(Dict::new(), c.as_bytes())))
}

fn form(doc: &mut Document, resources: Option<Object>, c: &str) -> ObjRef {
    let mut d = Dict::new();
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 1.into(), 1.into()]));
    if let Some(r) = resources {
        d.set(b"Resources".to_vec(), r);
    }
    doc.add(Object::Stream(Stream::flate(d, c.as_bytes())))
}

fn xobject_names(doc: &Document, resources: &Object) -> Vec<String> {
    let res = doc.resolve(resources);
    let mut n: Vec<String> =
        res.as_dict().unwrap().get(b"XObject").unwrap().as_dict().unwrap().iter().map(|(k, _)| String::from_utf8_lossy(k).into_owned()).collect();
    n.sort();
    n
}

fn page_xobjects(doc: &Document, page: ObjRef) -> Vec<String> {
    let res = doc.get(page).as_dict().unwrap().get(b"Resources").unwrap().clone();
    xobject_names(doc, &res)
}

fn count_images(bytes: &[u8]) -> usize {
    let doc = Document::open(Arc::new(bytes.to_vec())).unwrap();
    doc.object_numbers()
        .into_iter()
        .filter(|n| matches!(&*doc.get(ObjRef::new(*n, doc.generation(*n))), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image")))
        .count()
}

#[test]
fn images_and_forms_nothing_draws_are_dropped() {
    let mut doc = Document::new_empty();
    let img = |doc: &mut Document| image(doc, 64, 64, 3, false, None);

    // Edit a PDF deleting an image: the page gets a new content stream without the `Do`; the
    // old one, which still names the image, is no longer used.
    let t1 = img(&mut doc);
    let old = content(&mut doc, "q 72 0 0 72 0 0 cm /ImT Do Q");
    let t = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("ImT", t1)]))), Some(old));
    let new = content(&mut doc, "");
    doc.update_dict(t, |p| p.set(b"Contents".to_vec(), Object::Ref(new))).unwrap();
    // Dropped: a JPEG a page no longer draws, beside one it does.
    let a1 = img(&mut doc);
    let a2 = image(&mut doc, 64, 64, 3, true, None);
    let c = content(&mut doc, "q 72 0 0 72 0 0 cm /Im1 Do Q");
    let a = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("Im1", a1), ("Im2", a2)]))), Some(c));
    // Dropped from resources the page alone refers to, written as an object of their own.
    let s1 = img(&mut doc);
    let own = doc.add(Object::Dict(resources(xobjects(&[("Unused", s1)]))));
    let c = content(&mut doc, "");
    let s = raw_page(&mut doc, Object::Ref(own), Some(c));
    // An image drawn only inside a form stays; one beside it goes.
    let (d3, d4) = (img(&mut doc), img(&mut doc));
    let f = form(&mut doc, None, "/Im3 Do");
    let c = content(&mut doc, "/F Do");
    let d = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("F", f), ("Im3", d3), ("Im4", d4)]))), Some(c));

    // Kept, though the page's own content doesn't name them:
    // - drawn by a form with resources of its own that lack the name (renderers fall back to
    //   the page's);
    let k1 = img(&mut doc);
    let mut procset = Dict::new();
    procset.set(b"ProcSet".to_vec(), Object::Array(vec![Object::name("PDF")]));
    let fk = form(&mut doc, Some(Object::Dict(procset)), "/ImK Do");
    let c = content(&mut doc, "/Fk Do");
    let k = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("Fk", fk), ("ImK", k1)]))), Some(c));
    // - drawn by a Type 3 glyph named like an inert key;
    let g6 = img(&mut doc);
    let glyph = content(&mut doc, "/Im6 Do");
    let mut procs = Dict::new();
    procs.set(b"P".to_vec(), Object::Ref(glyph));
    let mut t3 = Dict::new();
    t3.set(b"Type".to_vec(), Object::name("Font"));
    t3.set(b"Subtype".to_vec(), Object::name("Type3"));
    t3.set(b"CharProcs".to_vec(), Object::Dict(procs));
    let mut fonts = Dict::new();
    fonts.set(b"T3".to_vec(), Object::Dict(t3));
    let mut res = resources(xobjects(&[("Im6", g6)]));
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    let c = content(&mut doc, "BT /T3 1 Tf (P) Tj ET");
    let g = raw_page(&mut doc, Object::Dict(res), Some(c));
    // - after an inline image whose data runs into the operators (no space before `EI`), or
    //   given as the second of two operands;
    let (l1, l2) = (img(&mut doc), img(&mut doc));
    let c = content(&mut doc, "BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 00>EI Q q /ImL Do Q /Other /ImL2 Do");
    let l = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("ImL", l1), ("ImL2", l2)]))), Some(c));
    // - drawn by a form field that is on no page;
    let z1 = img(&mut doc);
    let c = content(&mut doc, "");
    let z = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("Z1", z1)]))), Some(c));
    let wap = form(&mut doc, None, "/Z1 Do");
    let mut appearance = Dict::new();
    appearance.set(b"N".to_vec(), Object::Ref(wap));
    let mut widget = Dict::new();
    widget.set(b"Subtype".to_vec(), Object::name("Widget"));
    widget.set(b"AP".to_vec(), Object::Dict(appearance));
    let widget = doc.add(Object::Dict(widget));
    let mut acroform = Dict::new();
    acroform.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(widget)]));
    let root = doc.root().unwrap();
    doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(acroform))).unwrap();
    // - in resources shared by pages (or with an annotation's appearance), or inherited from the
    //   page tree: left alone;
    let (b1, b2) = (img(&mut doc), img(&mut doc));
    let shared = doc.add(Object::Dict(resources(xobjects(&[("X1", b1), ("X2", b2)]))));
    let c = content(&mut doc, "0 0 m");
    raw_page(&mut doc, Object::Ref(shared), Some(c));
    let c = content(&mut doc, "/X1 Do");
    raw_page(&mut doc, Object::Ref(shared), Some(c));
    let (inh, unused_inh) = (img(&mut doc), img(&mut doc));
    let tree = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
    doc.update_dict(tree, |d| d.set(b"Resources".to_vec(), Object::Dict(resources(xobjects(&[("Inh", inh), ("Old", unused_inh)]))))).unwrap();
    // - named by another page for its own image.
    let (r1, r2) = (img(&mut doc), img(&mut doc));
    let c = content(&mut doc, "");
    let r = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("Im9", r1)]))), Some(c));
    let c = content(&mut doc, "/Im9 Do");
    raw_page(&mut doc, Object::Dict(resources(xobjects(&[("Im9", r2)]))), Some(c));

    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(report.unused_xobjects, 4, "{report:?}");
    assert!(page_xobjects(&doc, t).is_empty(), "the deleted image goes");
    assert_eq!(page_xobjects(&doc, a), ["Im1"]);
    assert!(xobject_names(&doc, &Object::Ref(own)).is_empty());
    assert!(page_xobjects(&doc, s).is_empty());
    assert_eq!(page_xobjects(&doc, d), ["F", "Im3"]);
    assert_eq!(page_xobjects(&doc, k), ["Fk", "ImK"]);
    assert_eq!(page_xobjects(&doc, g), ["Im6"]);
    assert_eq!(page_xobjects(&doc, l), ["ImL", "ImL2"]);
    assert_eq!(page_xobjects(&doc, z), ["Z1"]);
    assert_eq!(xobject_names(&doc, &Object::Ref(shared)), ["X1", "X2"]);
    assert_eq!(page_xobjects(&doc, tree), ["Inh", "Old"]);
    assert_eq!(page_xobjects(&doc, r), ["Im9"]);
    let after = write_full(&doc, &SaveOptions::default()).unwrap();
    assert_eq!((count_images(&before), count_images(&after)), (17, 13), "the dropped images are gone from the file");
}

#[test]
fn nothing_is_dropped_when_a_stream_cannot_be_checked() {
    // A page with an image nothing draws, and an appearance stream on it.
    let build = |stream: Stream| {
        let mut doc = Document::new_empty();
        let unused = image(&mut doc, 64, 64, 3, false, None);
        let c = content(&mut doc, "");
        let page = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("Im1", unused)]))), Some(c));
        let other = doc.add(Object::Stream(stream));
        let mut appearance = Dict::new();
        appearance.set(b"N".to_vec(), Object::Ref(other));
        let mut annot = Dict::new();
        annot.set(b"AP".to_vec(), Object::Dict(appearance));
        doc.update_dict(page, |p| p.set(b"Annots".to_vec(), Object::Array(vec![Object::Dict(annot)]))).unwrap();
        let report = optimize(&mut doc, &Settings::default()).unwrap();
        (report.unused_xobjects, page_xobjects(&doc, page))
    };
    let (gone, kept) = ((1, Vec::<String>::new()), (0, vec!["Im1".to_string()]));
    // Valid compressed data with extra keys.
    let flate = |extra: &[(&str, Object)]| {
        let mut s = Stream::flate(Dict::new(), b"0 0 m");
        s.dict.set(b"Filter".to_vec(), Object::name("FlateDecode"));
        for (k, v) in extra {
            s.dict.set(k.as_bytes().to_vec(), v.clone());
        }
        s
    };
    let parms = |k: &str, v: Object| {
        let mut p = Dict::new();
        p.set(k.as_bytes().to_vec(), v);
        Object::Dict(p)
    };
    // Readable: the image goes.
    assert_eq!(build(flate(&[])), gone);
    assert_eq!(build(flate(&[("DecodeParms", parms("Predictor", Object::Int(1)))])), gone);
    // The name written with an escape still counts.
    assert_eq!(build(Stream::flate(Dict::new(), b"/Im#31 Do")), kept);
    // Read differently by viewers, or not at all: nothing goes.
    let dangling = Object::Ref(ObjRef::new(999, 0));
    let cases = [
        Stream::from_raw(flate(&[]).dict, b"not zlib data".to_vec()),
        flate(&[("Filter", dangling.clone())]),
        flate(&[("Filter", Object::Array(vec![Object::name("FlateDecode"), dangling.clone()]))]),
        flate(&[("Filter", Object::Array(vec![Object::name("Crypt"), Object::name("FlateDecode")]))]),
        flate(&[("DP", parms("Predictor", Object::Int(1)))]),
        flate(&[("DecodeParms", Object::Array(vec![Object::Dict(Dict::new()), Object::Dict(Dict::new())]))]),
        flate(&[("DecodeParms", parms("Predictor", dangling))]),
        flate(&[("DecodeParms", parms("Predictor", Object::Real(12.5)))]),
        flate(&[("F", Object::name("external.dat"))]),
        // An image codec on a stream drawn as an appearance: viewers decode it as content.
        Stream::from_raw(
            {
                let mut d = Dict::new();
                d.set(b"Filter".to_vec(), Object::name("CCITTFaxDecode"));
                d
            },
            vec![0; 8],
        ),
    ];
    for s in cases {
        let dict = s.dict.clone();
        assert_eq!(build(s), kept, "{dict:?}");
    }
}

/// `data` as ASCII85 text with the `~>` end marker, as ASCII85Decode reads it.
fn ascii85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for chunk in data.chunks(4) {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        let mut v = u32::from_be_bytes(word);
        let mut digits = [0u8; 5];
        for d in digits.iter_mut().rev() {
            *d = (v % 85) as u8 + b'!';
            v /= 85;
        }
        out.extend_from_slice(&digits[..chunk.len() + 1]);
    }
    out.extend_from_slice(b"~>");
    out
}

/// `data` as hex digits with the `>` end marker, as ASCIIHexDecode reads it.
fn ascii_hex(data: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = data.iter().flat_map(|b| format!("{b:02X}").into_bytes()).collect();
    out.push(b'>');
    out
}

fn filtered(names: &[&str], raw: Vec<u8>) -> Stream {
    let mut d = Dict::new();
    d.set(b"Filter".to_vec(), Object::Array(names.iter().map(|n| Object::name(n)).collect()));
    Stream::from_raw(d, raw)
}

/// A one-page document whose page content is `content`, with the content's reference.
fn with_content(content: Stream) -> (Document, ObjRef) {
    let mut doc = Document::new_empty();
    let pg = page(&mut doc, &[], "");
    let c = doc.add(Object::Stream(content));
    doc.update_dict(pg, |d| d.set(b"Contents".to_vec(), Object::Ref(c))).unwrap();
    (doc, c)
}

fn vector_ops() -> Vec<u8> {
    "0 0 m 612 792 l S\n".repeat(400).into_bytes()
}

#[test]
fn ascii_wrappers_come_off_filtered_streams_and_the_inner_data_is_unchanged() {
    let ops = vector_ops();
    let inner = Stream::flate(Dict::new(), &ops).raw.to_vec();
    for (outer, encoded) in [("ASCII85Decode", ascii85(&inner)), ("ASCIIHexDecode", ascii_hex(&inner))] {
        let source = filtered(&[outer, "FlateDecode"], encoded);
        let (mut doc, c) = with_content(source.clone());
        let r = optimize(&mut doc, &Settings::default()).unwrap();
        assert_eq!(r.streams_compressed, 1, "{outer}");
        let after = stream(&doc, c);
        assert_eq!(after.dict.get(b"Filter"), Some(&Object::Array(vec![Object::name("FlateDecode")])), "{outer}");
        assert_eq!(after.raw.to_vec(), inner, "{outer}: the inner Flate bytes are kept exactly");
        assert_eq!(after.decoded_strict().unwrap(), ops, "{outer}");
        assert!(after.raw.len() < source.raw.len(), "{outer}");
    }
}

#[test]
fn streams_the_wrapper_stripping_does_not_touch_stay_unchanged() {
    let inner = Stream::flate(Dict::new(), &vector_ops()).raw.to_vec();
    let wrapped = ascii85(&inner);
    let mut with_parms = filtered(&["ASCII85Decode", "FlateDecode"], wrapped.clone());
    with_parms.dict.set(b"DecodeParms".to_vec(), Object::Dict(Dict::new()));
    let mut image = filtered(&["ASCII85Decode", "FlateDecode"], wrapped.clone());
    image.dict.set(b"Subtype".to_vec(), Object::name("Image"));
    let mut objstm = filtered(&["ASCII85Decode", "FlateDecode"], wrapped.clone());
    objstm.dict.set(b"Type".to_vec(), Object::name("ObjStm"));
    let mut metadata = filtered(&["ASCII85Decode", "FlateDecode"], wrapped.clone());
    metadata.dict.set(b"Type".to_vec(), Object::name("Metadata"));
    let cases = [
        ("malformed ASCII85", filtered(&["ASCII85Decode", "FlateDecode"], b"{{ not ASCII85 ~>".to_vec())),
        ("malformed ASCIIHex", filtered(&["ASCIIHexDecode", "FlateDecode"], b"G0>".to_vec())),
        ("DecodeParms", with_parms),
        ("single ASCII filter", filtered(&["ASCII85Decode"], wrapped.clone())),
        ("image", image),
        ("object stream", objstm),
        ("metadata", metadata),
        ("crypt after the wrapper", filtered(&["ASCII85Decode", "Crypt", "FlateDecode"], wrapped.clone())),
        ("tiny", filtered(&["ASCII85Decode", "FlateDecode"], ascii85(b"ab"))),
        ("single Flate", Stream::flate(Dict::new(), &vector_ops())),
    ];
    for (label, s) in cases {
        let (mut doc, c) = with_content(s.clone());
        let r = optimize(&mut doc, &Settings::default()).unwrap();
        assert_eq!(r.streams_compressed, 0, "{label}");
        assert_eq!(stream(&doc, c), s, "{label}");
    }
}

#[test]
fn a_second_run_leaves_the_stripped_stream_alone() {
    let inner = Stream::flate(Dict::new(), &vector_ops()).raw.to_vec();
    let (mut doc, c) = with_content(filtered(&["ASCII85Decode", "FlateDecode"], ascii85(&inner)));
    optimize(&mut doc, &Settings::default()).unwrap();
    let once = stream(&doc, c);
    let r = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(r.streams_compressed, 0);
    assert_eq!(stream(&doc, c), once);
}

/// A grey image `w` px wide and one row high, every pixel `v`; JPEG or Flate.
fn strip(doc: &mut Document, w: u32, v: u8, jpeg: bool) -> ObjRef {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(w as i64));
    d.set(b"Height".to_vec(), Object::Int(1));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
    let px = vec![v; w as usize];
    let s = if jpeg {
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95).encode(&px, w, 1, image::ExtendedColorType::L8).unwrap();
        d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
        Stream::from_raw(d, out)
    } else {
        Stream::flate(d, &px)
    };
    doc.add(Object::Stream(s))
}

/// A page drawn the way some PDF printers write a page picture: `rows` images one pixel row
/// high, 1000 units wide, scaled into a 24 pt square (so 600 ppi across at 200 px). Row `r`
/// from the top is grey `r`; `edit` changes the content of each row.
fn printed_page(rows: usize, jpeg: impl Fn(usize) -> bool, edit: impl Fn(usize, String) -> String) -> (Document, Vec<ObjRef>) {
    let mut doc = Document::new_empty();
    let mut names = Vec::new();
    let mut refs = Vec::new();
    let mut content = String::from("q 0.024 0 0 0.024 0 0 cm\n");
    for r in 0..rows {
        let img = strip(&mut doc, 200, r as u8, jpeg(r));
        refs.push(img);
        let y = (rows - 1 - r) as f64;
        content += &edit(r, format!("q 1000 0 0 1 0 {y} cm\n/R{r} Do\nQ\n"));
        names.push(format!("R{r}"));
    }
    content += "Q\n";
    let xo: Vec<(&str, ObjRef)> = names.iter().map(String::as_str).zip(refs.iter().copied()).collect();
    let p = page(&mut doc, &xo, &content);
    // No thumbnail: it would be the old content stream, which still names the strips.
    doc.update_dict(p, |d| {
        d.remove(b"Thumb");
    })
    .unwrap();
    (doc, refs)
}

/// The images the first page's content stream draws, by name, with their streams.
fn drawn_images(doc: &Document) -> Vec<Stream> {
    let pages = pdfcraft_annot::page_refs(doc).unwrap();
    let p = doc.get(pages[0]).as_dict().cloned().unwrap();
    let res = doc.resolve(p.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
    let c = match &*doc.resolve(p.get(b"Contents").unwrap()) {
        Object::Stream(s) => s.decoded().unwrap(),
        _ => panic!("contents"),
    };
    pdfcraft_content::parse(&c)
        .ops
        .iter()
        .filter(|o| o.is("Do"))
        .map(|o| match &*doc.resolve(xo.get(o.name(0).unwrap()).unwrap()) {
            Object::Stream(s) => s.clone(),
            _ => panic!("image"),
        })
        .collect()
}

#[test]
fn a_page_picture_printed_as_one_pixel_strips_is_merged_into_one_image() {
    // 120 strips, every other one JPEG and half as wide (as one PDF printer writes them).
    let (mut doc, _) = printed_page(120, |r| r % 2 == 1, |_, s| s);
    let keep = Settings {
        gray: ImageSettings { downsample: false, target_ppi: 150.0, above_ppi: 225.0, compression: Compression::Retain },
        ..Settings::default()
    };
    let r = optimize(&mut doc, &keep).unwrap();
    assert_eq!((r.merged_images, r.fragments_merged), (1, 120), "{r:?}");
    let drawn = drawn_images(&doc);
    assert_eq!(drawn.len(), 1, "one image instead of 120");
    let img = &drawn[0];
    assert_eq!((img.dict.int(b"Width"), img.dict.int(b"Height")), (Some(200), Some(120)), "the finest resolution both ways");
    let px = img.decoded().unwrap();
    // Each row is its strip's grey, top row first; JPEG rows within a few levels.
    for (row, chunk) in px.chunks(200).enumerate() {
        assert!(chunk.iter().all(|&v| (v as i32 - row as i32).abs() <= 3), "row {row}: {:?}", &chunk[..4]);
    }
    assert_eq!(r.unused_xobjects, 120, "the strips are dropped");
}

#[test]
fn the_mobile_preset_makes_a_printed_page_small() {
    use pdfcraft_optimize::Preset;
    let (doc, _) = printed_page(200, |_| true, |_, s| s);
    let bytes = |d: &Document| write_full(d, &SaveOptions::default()).unwrap().len();
    let before = bytes(&doc);
    let mut mobile = doc.clone();
    let r = optimize(&mut mobile, &Preset::Mobile.settings()).unwrap();
    assert_eq!((r.merged_images, r.images_resampled), (1, 1), "{r:?}");
    let img = &drawn_images(&mobile)[0];
    // 24 pt = 1/3 in at 96 ppi.
    assert_eq!((img.dict.int(b"Width"), img.dict.int(b"Height")), (Some(32), Some(32)));
    assert_eq!(img.dict.name(b"Filter"), Some(&b"DCTDecode"[..]));
    assert!(bytes(&mobile) * 10 < before, "{} → {}", before, bytes(&mobile));
    assert_eq!(Preset::of(&Preset::Mobile.settings()), Some(Preset::Mobile));
    assert_eq!(Preset::from_name("mobile"), Some(Preset::Mobile));
}

#[test]
fn images_that_do_not_tile_a_picture_are_left_alone() {
    let unmerged = |doc: &mut Document, settings: &Settings| {
        let r = optimize(doc, settings).unwrap();
        assert_eq!(r.merged_images, 0, "{r:?}");
    };
    let all = Settings::default();
    // A gap between two rows.
    let (mut gap, _) =
        printed_page(40, |_| false, |r, s| if r < 20 { s.replace(&format!(" 0 {} cm", 39 - r), &format!(" 0 {} cm", 41 - r)) } else { s });
    let r = optimize(&mut gap, &all).unwrap();
    assert_eq!((r.merged_images, r.fragments_merged), (2, 40), "one picture each side of the gap: {r:?}");
    // Rotated rows.
    let (mut turned, _) = printed_page(40, |_| false, |_, s| s.replace("q 1000 0 0 1", "q 0 1 -1000 0"));
    unmerged(&mut turned, &all);
    // Something else drawn between the rows.
    let (mut between, _) = printed_page(40, |_| false, |r, s| if r == 10 { format!("0 0 10 10 re f\n{s}") } else { s });
    let r = optimize(&mut between, &all).unwrap();
    assert_eq!((r.merged_images, r.fragments_merged), (2, 40), "two runs, each side of the fill: {r:?}");
    // Too few rows, and the option off.
    let (mut few, _) = printed_page(3, |_| false, |_, s| s);
    unmerged(&mut few, &all);
    let (mut off, _) = printed_page(40, |_| false, |_, s| s);
    unmerged(&mut off, &Settings { merge_fragments: false, ..Settings::default() });
}

/// `levels` nested forms with their own resources, the innermost drawing `img` over the unit square.
fn nested_forms(doc: &mut Document, img: ObjRef, levels: usize) -> ObjRef {
    let mut inner = form(doc, Some(Object::Dict(resources(xobjects(&[("Im", img)])))), "/Im Do");
    for _ in 1..levels {
        inner = form(doc, Some(Object::Dict(resources(xobjects(&[("F", inner)])))), "/F Do");
    }
    inner
}

#[test]
fn a_deeply_nested_form_is_measured_at_its_largest_use() {
    // 1200 px drawn 1 in wide (1200 ppi) and then 8 in wide (150 ppi): 150 ppi, so it isn't resampled.
    let small = "q 72 0 0 72 36 36 cm /F Do Q";
    let large = "q 576 0 0 576 18 100 cm /F Do Q";
    for levels in [1, 6, 12] {
        for (pages, label) in [(vec![format!("{small} {large}")], "one page"), (vec![small.into(), large.into()], "two pages")] {
            let mut doc = Document::new_empty();
            let img = image(&mut doc, 1200, 1200, 3, false, None);
            let outer = nested_forms(&mut doc, img, levels);
            for c in &pages {
                page(&mut doc, &[("F", outer)], c);
            }
            let refs = pdfcraft_annot::page_refs(&doc).unwrap();
            assert_eq!(effective_resolutions(&doc, &refs)[&img].round(), 150.0, "{levels} levels, {label}");
            optimize(&mut doc, &Settings::default()).unwrap();
            assert_eq!(stream(&doc, img).dict.int(b"Width"), Some(1200), "{levels} levels, {label}");
        }
    }
}

#[test]
fn a_form_drawing_itself_is_walked_once_per_use() {
    let mut doc = Document::new_empty();
    let img = image(&mut doc, 1200, 1200, 3, false, None);
    let f = form(&mut doc, None, "/Im Do q 2 0 0 2 0 0 cm /F Do Q");
    let mut x = xobjects(&[("Im", img)]);
    x.set(b"F".to_vec(), Object::Ref(f));
    let mut d = match &*doc.get(f) {
        Object::Stream(s) => s.clone(),
        _ => unreachable!(),
    };
    d.dict.set(b"Resources".to_vec(), Object::Dict(resources(x)));
    doc.set(f, Object::Stream(d));
    page(&mut doc, &[("F", f)], "q 72 0 0 72 36 36 cm /F Do Q");
    let refs = pdfcraft_annot::page_refs(&doc).unwrap();
    // Only the outer use: the cycle isn't followed, and doesn't count as a missed use.
    assert_eq!(effective_resolutions(&doc, &refs)[&img].round(), 1200.0);
}

#[test]
fn a_shared_form_without_resources_draws_each_callers_image() {
    // G has no /Resources, so its /Im is whichever image the form drawing it names.
    let mut doc = Document::new_empty();
    let a = image(&mut doc, 1200, 1200, 3, false, None);
    let b = image(&mut doc, 1200, 1200, 3, false, None);
    let g = form(&mut doc, None, "/Im Do");
    let mut outers = Vec::new();
    for img in [a, b] {
        let mut inner = form(&mut doc, Some(Object::Dict(resources(xobjects(&[("Im", img), ("G", g)])))), "/G Do");
        for _ in 1..6 {
            inner = form(&mut doc, Some(Object::Dict(resources(xobjects(&[("F", inner)])))), "/F Do");
        }
        outers.push(inner);
    }
    page(&mut doc, &[("A", outers[0]), ("B", outers[1])], "q 72 0 0 72 36 36 cm /A Do Q q 576 0 0 576 18 100 cm /B Do Q");
    let refs = pdfcraft_annot::page_refs(&doc).unwrap();
    let ppi = effective_resolutions(&doc, &refs);
    assert_eq!(ppi[&a].round(), 1200.0);
    assert_eq!(ppi.get(&b).map(|p| p.round()), Some(150.0));
}

#[test]
fn images_are_not_resampled_when_some_uses_could_not_be_measured() {
    // Too deep: the 13th form isn't followed, so its use of the image is unknown.
    let mut doc = Document::new_empty();
    let img = image(&mut doc, 1200, 1200, 3, false, None);
    let other = image(&mut doc, 1200, 1200, 3, false, None);
    let deep = nested_forms(&mut doc, img, 13);
    page(
        &mut doc,
        &[("F", deep), ("Im", img), ("O", other)],
        "q 72 0 0 72 36 36 cm /Im Do Q q 72 0 0 72 200 36 cm /O Do Q q 576 0 0 576 18 100 cm /F Do Q",
    );
    let refs = pdfcraft_annot::page_refs(&doc).unwrap();
    let ppi = effective_resolutions(&doc, &refs);
    assert_eq!((ppi[&img], ppi[&other]), (f64::INFINITY, f64::INFINITY));
    optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(stream(&doc, img).dict.int(b"Width"), Some(1200));

    // Too much work: forms without resources (so nothing can be skipped), each drawing the next
    // three times, twelve deep, would be (3^12 - 1) / 2 walks.
    let mut doc = Document::new_empty();
    let img = image(&mut doc, 1200, 1200, 3, false, None);
    let mut names = vec![("Im", img)];
    let mut next = "Im";
    let labels = ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12"];
    for (i, label) in labels.iter().enumerate().rev() {
        let s = 1.0 + i as f64 / 100.0;
        let f = form(&mut doc, None, &format!("q {s} 0 0 {s} 0 0 cm /{next} Do Q /{next} Do q 1 0 0 1 1 0 cm /{next} Do Q"));
        names.push((label, f));
        next = label;
    }
    page(&mut doc, &names, "q 72 0 0 72 36 36 cm /F1 Do Q");
    let refs = pdfcraft_annot::page_refs(&doc).unwrap();
    assert_eq!(effective_resolutions(&doc, &refs)[&img], f64::INFINITY);
}

#[test]
fn image_resolution_keeps_comments_across_content_pieces() {
    for ending in ["", "\n", "\r", "\r\n"] {
        let mut doc = Document::new_empty();
        let visible = image(&mut doc, 20, 20, 3, false, None);
        let hidden = image(&mut doc, 20, 20, 3, false, None);
        let page = page(&mut doc, &[("Visible", visible), ("Hidden", hidden)], "");
        let first = doc
            .add(Object::Stream(Stream::from_raw(Dict::new(), format!("q 72 0 0 72 0 0 cm /Visible Do Q\n% trailing comment{ending}").into_bytes())));
        let second = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"q 144 0 0 144 0 0 cm /Hidden Do Q\n".to_vec())));
        doc.update_dict(page, |dict| {
            dict.set(b"Contents".to_vec(), Object::Array(vec![Object::Ref(first), Object::Int(17), Object::Ref(second)]));
        })
        .unwrap();
        let measured = effective_resolutions(&doc, &[page]);
        assert_eq!(measured.get(&visible), Some(&20.0));
        if ending.is_empty() {
            assert_eq!(measured.len(), 1);
            assert_eq!(measured.get(&hidden), None, "a comment must hide the next piece's first line");
        } else {
            assert_eq!(measured.len(), 2);
            assert_eq!(measured.get(&hidden), Some(&10.0), "a real line ending exposes the image");
        }
    }
}

#[test]
fn a_hostile_image_size_is_left_alone_not_multiplied_into_overflow() {
    // /Width and /Height are file-controlled: 2^32 + 100 wraps to 100 when read as u32, while the
    // real values overflow `w * h * n`. The image must be skipped, not sized by a wrapped product.
    let mut doc = Document::new_empty();
    let side = i64::from(u32::MAX) + 101;
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(side));
    d.set(b"Height".to_vec(), Object::Int(side));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
    let hostile = doc.add(Object::Stream(Stream::flate(d, &[7u8; 256])));
    page(&mut doc, &[("Im", hostile)], "q 1 0 0 1 10 10 cm /Im Do Q");
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(report.images_resampled, 0, "{report:?}");
    assert_eq!(stream(&doc, hostile).dict.int(b"Width"), Some(side), "the image is left as it was");
}

#[test]
fn a_hostile_soft_mask_leaves_its_image_unchanged() {
    let mut doc = Document::new_empty();
    let mask = image(&mut doc, 400, 400, 1, false, None);
    let mut hostile = stream(&doc, mask);
    hostile.dict.set(b"Width".to_vec(), Object::Int(i64::from(u32::MAX) + 401));
    doc.set(mask, Object::Stream(hostile.clone()));
    let photo = image(&mut doc, 400, 400, 3, false, Some(mask));
    let before = stream(&doc, photo);
    page(&mut doc, &[("Photo", photo)], "q 72 0 0 72 10 10 cm /Photo Do Q");
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(report.images_resampled, 0, "{report:?}");
    assert_eq!(stream(&doc, photo), before);
    assert_eq!(stream(&doc, mask), hostile);
}
