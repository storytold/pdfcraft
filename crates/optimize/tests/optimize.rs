//! The optimizer on documents with images drawn at known sizes.

use std::sync::Arc;

use printcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};
use printcraft_optimize::{Compression, ImageSettings, Settings, SpaceCategory, audit_space, effective_resolutions, optimize};

fn image(doc: &mut Document, w: u32, h: u32, components: usize, jpeg: bool, smask: Option<ObjRef>) -> ObjRef {
    // A smooth gradient with a little texture (photo-like).
    let mut px = Vec::with_capacity((w * h) as usize * components);
    for y in 0..h {
        for x in 0..w {
            let sample = ((x * 255 / w.max(1)) as u8).wrapping_add(((x ^ y) & 7) as u8);
            px.push(sample);
            if components == 3 {
                px.push((y * 255 / h.max(1)) as u8);
                px.push(128);
            }
        }
    }
    let mut dict = Dict::new();
    dict.set(b"Type".to_vec(), Object::name("XObject"));
    dict.set(b"Subtype".to_vec(), Object::name("Image"));
    dict.set(b"Width".to_vec(), Object::Int(i64::from(w)));
    dict.set(b"Height".to_vec(), Object::Int(i64::from(h)));
    dict.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    dict.set(b"ColorSpace".to_vec(), Object::name(if components == 1 { "DeviceGray" } else { "DeviceRGB" }));
    if let Some(m) = smask {
        dict.set(b"SMask".to_vec(), Object::Ref(m));
    }
    let img = if jpeg {
        let mut out = Vec::new();
        let ty = if components == 1 { image::ExtendedColorType::L8 } else { image::ExtendedColorType::Rgb8 };
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 98).encode(&px, w, h, ty).unwrap();
        dict.set(b"Filter".to_vec(), Object::name("DCTDecode"));
        Stream::from_raw(dict, out)
    } else {
        Stream::flate(dict, &px)
    };
    doc.add(Object::Stream(img))
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
    let pages = printcraft_annot::page_refs(&doc).unwrap();
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
    assert_eq!(report.thumbnails, 2);
    let bs = stream(&doc, big);
    assert_eq!((bs.dict.int(b"Width"), bs.dict.int(b"Height"), bs.dict.name(b"Filter")), (Some(300), Some(300), Some(&b"DCTDecode"[..])));
    let ms = stream(&doc, masked);
    let sm = stream(&doc, mask);
    assert_eq!((ms.dict.int(b"Width"), sm.dict.int(b"Width")), (Some(150), Some(150)), "the soft mask follows its image");
    assert_eq!(sm.dict.name(b"Filter"), Some(&b"FlateDecode"[..]));
    assert_eq!(stream(&doc, small).dict.int(b"Width"), Some(100));
    assert_eq!(stream(&doc, twice).dict.int(b"Width"), Some(300), "75 ppi is below the threshold");
    let ps = stream(&doc, photo);
    assert_eq!(ps.dict.int(b"Width"), Some(600));
    let after_bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(after_bytes.len() * 3 < before, "{} → {}", before, after_bytes.len());

    // The optimized file opens, and the page still shows the picture where it was.
    let reopened = Document::open(Arc::new(after_bytes.clone())).unwrap();
    assert_eq!(printcraft_annot::page_refs(&reopened).unwrap().len(), 2);
    let mut renderer = printcraft_render::PageRenderer::new(Arc::new(after_bytes), printcraft_render::RenderConfig::default());
    let out = renderer.render(printcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    // Top-left image area (36..180 x 600..744 user → y from the top 48..192): drawn, not white.
    let at = ((100 * out.width + 100) * 4) as usize;
    assert_ne!(&out.rgba[at..at + 3], &[255, 255, 255]);
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
    let s = |t: &str| Object::String(printcraft_cos::PdfString::literal(t.as_bytes().to_vec()));
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
