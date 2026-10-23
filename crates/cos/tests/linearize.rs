//! Linearized saves (Fast Web View, ISO 32000-2 Annex F): the parameter dictionary, the two
//! cross-reference sections and the hint tables, read back by our parser, by hayro and by
//! `qpdf --check-linearization`, and detected on open.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use pdfcraft_cos::{
    Algorithm, CosError, Dict, Document, Linearization, Linearized, NewEncryption, ObjRef, Object, SaveOptions, StaleReason, XrefEntry,
    parse_indirect, write_full, write_incremental, write_linearized,
};
use pdfcraft_testkit::Pdf;

/// Three pages under a page tree that passes `/MediaBox` and `/Resources` down; a font only the
/// first page and the second use, a font the second and third share, an image only the third
/// page draws (also named by an unknown trailer entry, so it is not that page's alone), a third
/// page without `/Type`, a form field on page 2, a thumbnail on page 3, bookmarks and document
/// info.
fn fixture(page_mode: Option<&str>) -> Vec<u8> {
    let mode = page_mode.map(|m| format!("/PageMode /{m}")).unwrap_or_default();
    Pdf::new()
        .version("1.4")
        .obj(format!("<< /Type /Catalog /Pages 2 0 R /Outlines 10 0 R {mode} /AcroForm << /Fields [12 0 R] >> >>")) // 1
        .obj("<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 200 300] /Resources << /Font << /F1 6 0 R >> >> >>") // 2
        .obj("<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>") // 3
        .obj("<< /Type /Page /Parent 2 0 R /Contents 8 0 R /Resources << /Font << /F1 6 0 R /F2 11 0 R >> >> /Annots [12 0 R] >>") // 4
        .obj("<< /Parent 2 0 R /Contents 9 0 R /Resources << /Font << /F2 11 0 R >> /XObject << /Im 16 0 R >> >> /Thumb 13 0 R >>") // 5
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>") // 6
        .stream("", b"BT /F1 24 Tf 20 150 Td (First page) Tj ET") // 7
        .stream("", b"BT /F1 12 Tf 20 150 Td (Second) Tj /F2 12 Tf (page) Tj ET") // 8
        .stream("", b"q 20 0 0 20 10 10 cm /Im Do Q BT /F2 12 Tf 20 150 Td (Third page) Tj ET") // 9
        .obj("<< /Type /Outlines /First 14 0 R /Last 14 0 R /Count 1 >>") // 10
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >>") // 11
        .obj("<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Ada) /Rect [10 10 150 30] /P 4 0 R >>") // 12
        .stream("/Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8", [0u8, 255, 255, 0]) // 13
        .obj("<< /Title (Chapter one) /Parent 10 0 R /Dest [3 0 R /Fit] >>") // 14
        .obj("<< /Title (Linearized fixture) /Producer (PdfCraft tests) >>") // 15
        .stream("/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8", [0u8, 128, 128, 255]) // 16
        .obj("<< /Note (kept by every save) /Image 16 0 R >>") // 17
        .trailer_entry("/Info 15 0 R")
        .trailer_entry("/Custom 17 0 R")
        .build()
}

fn single_page() -> Vec<u8> {
    Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Contents 4 0 R >>")
        .stream("", b"0 0 m 100 100 l S")
        .build()
}

/// Twelve pages sharing one font, each with its own content: page runs of different lengths.
fn many_pages() -> Vec<u8> {
    let kids: Vec<String> = (0..12).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    let mut pdf = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj(format!("<< /Type /Pages /Kids [{}] /Count 12 /MediaBox [0 0 200 300] >>", kids.join(" ")))
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>");
    for i in 0..12 {
        let text = "x".repeat(10 + 37 * i);
        pdf = pdf
            .obj(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i))
            .stream("", format!("BT /F1 9 Tf 10 150 Td ({text}) Tj ET"));
    }
    pdf.build()
}

fn linearize(bytes: Vec<u8>) -> Linearized {
    let doc = Document::open(Arc::new(bytes)).unwrap();
    write_linearized(&doc, &SaveOptions::default()).unwrap()
}

/// The parameter dictionary: the first object after the header.
fn parameters(bytes: &[u8]) -> (usize, Dict) {
    let mut lx = pdfcraft_cos::Lexer::new(bytes, 0);
    lx.skip_ws();
    let at = lx.pos;
    let (_, object) = parse_indirect(bytes, at, &|_| None).unwrap();
    (at, object.as_dict().cloned().unwrap())
}

fn int(d: &Dict, key: &[u8]) -> u64 {
    u64::try_from(d.int(key).unwrap_or_else(|| panic!("/{}", String::from_utf8_lossy(key)))).unwrap()
}

fn offset(doc: &Document, num: u32) -> u64 {
    match doc.xref_entry(num) {
        Some(XrefEntry::InFile { offset, .. }) => offset,
        other => panic!("object {num} is not in the file: {other:?}"),
    }
}

fn pages(doc: &Document) -> Vec<ObjRef> {
    let catalog = doc.get(doc.root().unwrap()).as_dict().cloned().unwrap();
    let mut out = Vec::new();
    let mut stack = vec![catalog.reference(b"Pages").unwrap()];
    while let Some(r) = stack.pop() {
        let d = doc.get(r).as_dict().cloned().unwrap();
        // `/Kids` may be a reference to the array.
        let kids = match d.get(b"Kids") {
            Some(Object::Ref(k)) => doc.get(*k).as_array().cloned(),
            other => other.and_then(Object::as_array).cloned(),
        };
        match kids {
            Some(kids) => kids.iter().rev().for_each(|k| stack.push(k.as_ref().unwrap())),
            None => out.push(r),
        }
    }
    out
}

/// Reads hint table bit fields (Annex F.4) independently of the writer.
struct BitReader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl BitReader<'_> {
    fn get(&mut self, n: u64) -> u64 {
        let mut v = 0;
        for _ in 0..n {
            let byte = self.data[self.bit / 8];
            v = (v << 1) | u64::from((byte >> (7 - self.bit % 8)) & 1);
            self.bit += 1;
        }
        v
    }

    fn align(&mut self) {
        self.bit = self.bit.div_ceil(8) * 8;
    }
}

struct PageHint {
    objects: u64,
    length: u64,
    shared: Vec<u64>,
    content_offset: u64,
    content_length: u64,
}

struct Hints {
    first_page_offset: u64,
    pages: Vec<PageHint>,
    first_shared_obj: u64,
    first_shared_offset: u64,
    shared_first_page: u64,
    shared_lengths: Vec<u64>,
    outline: Option<[u64; 4]>,
}

fn hints(bytes: &[u8], doc: &Document, npages: usize) -> Hints {
    let (_, lin) = parameters(bytes);
    let h = lin.get(b"H").unwrap().as_array().unwrap();
    let (_, stream) = parse_indirect(bytes, h[0].as_int().unwrap() as usize, &|_| None).unwrap();
    let Object::Stream(stream) = stream else { panic!("the hint stream is a stream") };
    let num = parse_indirect(bytes, h[0].as_int().unwrap() as usize, &|_| None).unwrap().0;
    // Decrypt through the document when the file is encrypted.
    let data = match doc.get(num).as_ref() {
        Object::Stream(s) => s.decoded().unwrap(),
        _ => stream.decoded().unwrap(),
    };
    let mut r = BitReader { data: &data, bit: 0 };
    let min_objects = r.get(32);
    let first_page_offset = r.get(32);
    let object_bits = r.get(16);
    let min_length = r.get(32);
    let length_bits = r.get(16);
    let min_content_offset = r.get(32);
    let content_offset_bits = r.get(16);
    let min_content_length = r.get(32);
    let content_length_bits = r.get(16);
    let shared_bits = r.get(16);
    let id_bits = r.get(16);
    let numerator_bits = r.get(16);
    let denominator = r.get(16);
    assert!(denominator > 0, "a fraction needs a denominator");
    let objects: Vec<u64> = (0..npages).map(|_| min_objects + r.get(object_bits)).collect();
    r.align();
    let lengths: Vec<u64> = (0..npages).map(|_| min_length + r.get(length_bits)).collect();
    r.align();
    let counts: Vec<u64> = (0..npages).map(|_| r.get(shared_bits)).collect();
    r.align();
    let shared: Vec<Vec<u64>> = counts.iter().map(|&n| (0..n).map(|_| r.get(id_bits)).collect()).collect();
    r.align();
    for &n in &counts {
        for _ in 0..n {
            r.get(numerator_bits);
        }
    }
    r.align();
    let content_offsets: Vec<u64> = (0..npages).map(|_| min_content_offset + r.get(content_offset_bits)).collect();
    r.align();
    let content_lengths: Vec<u64> = (0..npages).map(|_| min_content_length + r.get(content_length_bits)).collect();
    let pages = (0..npages)
        .map(|i| PageHint {
            objects: objects[i],
            length: lengths[i],
            shared: shared[i].clone(),
            content_offset: content_offsets[i],
            content_length: content_lengths[i],
        })
        .collect();

    let s = stream.dict.int(b"S").unwrap() as usize;
    let mut r = BitReader { data: &data[s..], bit: 0 };
    let first_shared_obj = r.get(32);
    let first_shared_offset = r.get(32);
    let shared_first_page = r.get(32);
    let total = r.get(32);
    let group_object_bits = r.get(16);
    let min_group = r.get(32);
    let group_bits = r.get(16);
    let shared_lengths: Vec<u64> = (0..total).map(|_| min_group + r.get(group_bits)).collect();
    r.align();
    let signatures: Vec<u64> = (0..total).map(|_| r.get(1)).collect();
    assert!(signatures.iter().all(|&s| s == 0));
    r.align();
    assert!((0..total).all(|_| r.get(group_object_bits) == 0), "one object per group");

    let outline = stream.dict.int(b"O").map(|o| {
        let mut r = BitReader { data: &data[o as usize..], bit: 0 };
        [r.get(32), r.get(32), r.get(32), r.get(32)]
    });
    Hints { first_page_offset, pages, first_shared_obj, first_shared_offset, shared_first_page, shared_lengths, outline }
}

/// Each in-file object's length: up to the next object or cross-reference section.
fn lengths(bytes: &[u8], doc: &Document) -> std::collections::BTreeMap<u32, (u64, u64)> {
    let mut starts: Vec<(u64, u32)> = doc
        .object_numbers()
        .into_iter()
        .filter_map(|n| match doc.xref_entry(n) {
            Some(XrefEntry::InFile { offset, .. }) => Some((offset, n)),
            _ => None,
        })
        .collect();
    starts.sort();
    let text = bytes;
    let ends: BTreeSet<u64> = (0..text.len()).filter(|&i| text[i..].starts_with(b"xref\n")).map(|i| i as u64).collect();
    starts
        .iter()
        .map(|&(at, n)| {
            let next_obj = starts.iter().map(|s| s.0).filter(|&s| s > at).min();
            let next_xref = ends.iter().copied().find(|&e| e > at);
            let end = [next_obj, next_xref].into_iter().flatten().min().unwrap();
            (n, (at, end - at))
        })
        .collect()
}

/// Objects a page uses: everything reachable from it except through `/Parent`, `/Thumb` or
/// another page.
fn used_by(doc: &Document, page: ObjRef) -> HashSet<u32> {
    let mut seen = HashSet::new();
    let mut stack = vec![page];
    while let Some(r) = stack.pop() {
        if !seen.insert(r.num) {
            continue;
        }
        let o = doc.get(r);
        if r != page && o.as_dict().is_some_and(|d| d.name(b"Type") == Some(b"Page")) {
            seen.remove(&r.num);
            continue;
        }
        fn refs(o: &Object, out: &mut Vec<ObjRef>, top: bool) {
            match o {
                Object::Ref(r) => out.push(*r),
                Object::Array(a) => a.iter().for_each(|x| refs(x, out, false)),
                Object::Dict(d) => {
                    d.iter().filter(|(k, _)| !top || (k.as_slice() != b"Parent" && k.as_slice() != b"Thumb")).for_each(|(_, v)| refs(v, out, false))
                }
                Object::Stream(s) => s.dict.iter().for_each(|(_, v)| refs(v, out, false)),
                _ => {}
            }
        }
        refs(&o, &mut stack, r == page);
    }
    seen
}

#[test]
fn the_parameter_dictionary_and_both_sections_follow_annex_f() {
    for mode in [None, Some("UseOutlines")] {
        let out = linearize(fixture(mode));
        assert_eq!(out.pages_given_type, 1, "the third page had no /Type");
        assert_eq!(out.pages_given_inherited, 3, "every page inherited /MediaBox");
        let bytes = out.bytes;
        let doc = Document::open(Arc::new(bytes.clone())).unwrap();
        assert!(doc.repair_log().is_empty(), "{:?}", doc.repair_log());
        assert_eq!(doc.linearization(), Linearization::Yes, "{mode:?}");
        let (at, lin) = parameters(&bytes);
        let lin_num = parse_indirect(&bytes, at, &|_| None).unwrap().0.num;
        assert!(at + 200 < 1024, "the dictionary sits in the first kilobyte");
        assert_eq!(lin.get(b"Linearized").and_then(Object::as_f64), Some(1.0));
        assert_eq!(int(&lin, b"L"), bytes.len() as u64, "/L is the file length");
        assert_eq!(int(&lin, b"N"), 3, "/N is the page count");
        let page_refs = pages(&doc);
        assert_eq!(page_refs.len(), 3);
        assert_eq!(int(&lin, b"O"), u64::from(page_refs[0].num), "/O is the first page");

        // Two sections: the first-page one is newest (startxref names it), its trailer's /Prev
        // names the main one, whose entry for object 0 /T points just in front of.
        let revisions = doc.revisions();
        assert_eq!(revisions.len(), 2);
        let (main, first) = (revisions[0].xref_offset, revisions[1].xref_offset);
        assert!(!revisions[0].is_stream && !revisions[1].is_stream);
        assert!((at as u64) < first && first < 1024, "the first-page table follows the dictionary");
        let t = int(&lin, b"T");
        assert!(bytes[t as usize].is_ascii_whitespace());
        assert_eq!(&bytes[t as usize + 1..t as usize + 19], b"0000000000 65535 f");
        assert!(t > main && t < main + 16);
        let first_trailer = &bytes[first as usize..];
        let prev = String::from_utf8_lossy(first_trailer).split("/Prev ").nth(1).unwrap().split_whitespace().next().unwrap().parse::<u64>().unwrap();
        assert_eq!(prev, main);
        assert!(bytes.ends_with(format!("startxref\n{first}\n%%EOF\n").as_bytes()));
        let first_section = String::from_utf8_lossy(&bytes[first as usize..first as usize + 40]).into_owned();
        assert!(first_section.starts_with(&format!("xref\n{lin_num} ")), "{first_section}");
        assert!(String::from_utf8_lossy(&bytes[..main as usize]).contains("startxref\n0\n%%EOF\n"), "the first-page trailer's startxref is a dummy");
        assert!(String::from_utf8_lossy(&bytes[main as usize..]).contains(&format!("trailer\n<</Size {lin_num}>>")));

        // The primary hint stream sits right before the first page; /E ends the first page.
        let h = lin.get(b"H").unwrap().as_array().unwrap();
        assert_eq!(h.len(), 2, "no overflow hint stream");
        let (h0, h1) = (h[0].as_int().unwrap() as u64, h[1].as_int().unwrap() as u64);
        assert!(matches!(parse_indirect(&bytes, h0 as usize, &|_| None).unwrap().1, Object::Stream(_)));
        assert_eq!(h0 + h1, offset(&doc, page_refs[0].num), "the first page follows the hint stream");
        let hint = hints(&bytes, &doc, 3);
        let e = int(&lin, b"E");
        assert_eq!(e, offset(&doc, page_refs[0].num) + hint.pages[0].length, "/E ends the first page section");

        // Everything the first page needs comes before /E; nothing of the other pages does.
        for num in used_by(&doc, page_refs[0]) {
            assert!(offset(&doc, num) < e, "object {num} of page 1 is in the first page section");
        }
        for page in &page_refs[1..] {
            assert!(offset(&doc, page.num) >= e);
        }
        // Bookmarks come with the first page only when the document opens showing them.
        let outlines = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Outlines").unwrap();
        assert_eq!(offset(&doc, outlines.num) < e, mode.is_some(), "{mode:?}");
    }
}

#[test]
fn hint_tables_describe_where_every_page_and_shared_object_is() {
    for input in [fixture(None), fixture(Some("UseOutlines")), single_page(), many_pages()] {
        let bytes = linearize(input).bytes;
        let doc = Document::open(Arc::new(bytes.clone())).unwrap();
        let page_refs = pages(&doc);
        let hint = hints(&bytes, &doc, page_refs.len());
        let (_, lin) = parameters(&bytes);
        let h = lin.get(b"H").unwrap().as_array().unwrap();
        let hint_len = h[1].as_int().unwrap() as u64;
        let len = lengths(&bytes, &doc);
        let run = |first: u32, count: u64| -> u64 {
            (0..count as u32).map(|k| len.get(&(first + k)).unwrap_or_else(|| panic!("object {} is in the run", first + k))).map(|(_, l)| l).sum()
        };
        // Offsets leave the hint stream out (F.4).
        assert_eq!(hint.first_page_offset + hint_len, offset(&doc, page_refs[0].num));
        for (i, (page, p)) in page_refs.iter().zip(&hint.pages).enumerate() {
            assert!(p.objects >= 1);
            assert_eq!(p.length, run(page.num, p.objects), "page {} length", i + 1);
            // The run is contiguous in the file, starting with the page object.
            let mut at = offset(&doc, page.num);
            for k in 0..p.objects as u32 {
                assert_eq!(len[&(page.num + k)].0, at, "page {} object {k} follows the previous one", i + 1);
                at += len[&(page.num + k)].1;
            }
            assert_eq!((p.content_offset, p.content_length), (0, p.length), "the page run stands for its content");
            if i == 0 {
                assert!(p.shared.is_empty(), "the first page lists no shared objects (F.4.1)");
            }
            for &id in &p.shared {
                assert!(id < hint.shared_lengths.len() as u64);
            }
        }
        // Shared object groups: the first page's objects from its page object, then part 8.
        let n_first = hint.shared_first_page;
        assert_eq!(n_first, hint.pages[0].objects, "every first-page object is a group");
        let group_object =
            |id: u64| -> u32 { if id < n_first { page_refs[0].num + id as u32 } else { hint.first_shared_obj as u32 + (id - n_first) as u32 } };
        for (id, length) in hint.shared_lengths.iter().enumerate() {
            assert_eq!(*length, len[&group_object(id as u64)].1, "group {id}");
        }
        if hint.shared_lengths.len() as u64 > n_first {
            assert_eq!(hint.first_shared_offset + hint_len, offset(&doc, hint.first_shared_obj as u32));
        }
        // Every shared object a page lists is one it uses, and every object a later page uses
        // that more than one page uses is either listed, the first page's, or document-level.
        for (page, p) in page_refs.iter().zip(&hint.pages).skip(1) {
            let used = used_by(&doc, *page);
            for &id in &p.shared {
                assert!(used.contains(&group_object(id)), "page object {} lists object {} it does not use", page.num, group_object(id));
            }
            for k in 0..p.objects as u32 {
                assert!(used.contains(&(page.num + k)), "page object {} holds object {} it does not use", page.num, page.num + k);
            }
        }
        // The outline table names the outline's run.
        let outlines = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Outlines");
        match (outlines, hint.outline) {
            (Some(o), Some([first, at, count, length])) => {
                assert_eq!(first, u64::from(o.num));
                assert_eq!(at + if offset(&doc, o.num) > h[0].as_int().unwrap() as u64 { hint_len } else { 0 }, offset(&doc, o.num));
                assert_eq!(length, run(o.num, count));
                assert_eq!(count, 2, "the outline root and its one item");
            }
            (None, None) => {}
            other => panic!("outline table and catalog disagree: {other:?}"),
        }
    }
}

#[test]
fn a_linearized_save_keeps_the_document() {
    let input = fixture(Some("UseOutlines"));
    let source = Document::open(Arc::new(input.clone())).unwrap();
    let bytes = linearize(input).bytes;
    let doc = Document::open(Arc::new(bytes.clone())).unwrap();
    let content = |d: &Document| -> Vec<Vec<u8>> {
        pages(d)
            .iter()
            .map(|p| {
                let page = d.get(*p).as_dict().cloned().unwrap();
                match d.get(page.reference(b"Contents").unwrap()).as_ref() {
                    Object::Stream(s) => s.decoded().unwrap(),
                    _ => panic!("content stream"),
                }
            })
            .collect()
    };
    assert_eq!(content(&doc), content(&source));
    for p in pages(&doc) {
        let page = doc.get(p).as_dict().cloned().unwrap();
        assert_eq!(page.name(b"Type"), Some(b"Page".as_slice()));
        assert!(page.get(b"MediaBox").is_some(), "inherited attributes are on every page");
        assert!(page.get(b"Resources").is_some());
    }
    let tree = doc.get(doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap());
    assert!(tree.as_dict().unwrap().get(b"MediaBox").is_none() && tree.as_dict().unwrap().get(b"Resources").is_none());
    // The first page used to inherit the Helvetica /Resources and still has them.
    let first = doc.get(pages(&doc)[0]).as_dict().cloned().unwrap();
    let res = doc.resolve(first.get(b"Resources").unwrap());
    let font = doc.resolve(res.as_dict().unwrap().get(b"Font").unwrap());
    let f1 = doc.get(font.as_dict().unwrap().reference(b"F1").unwrap());
    assert_eq!(f1.as_dict().unwrap().name(b"BaseFont"), Some(b"Helvetica".as_slice()));
    let info = doc.resolve(doc.trailer().get(b"Info").unwrap());
    assert_eq!(info.as_dict().unwrap().get(b"Title").unwrap().as_string().unwrap().bytes, b"Linearized fixture");
    let custom = doc.resolve(doc.trailer().get(b"Custom").unwrap());
    assert_eq!(custom.as_dict().unwrap().get(b"Note").unwrap().as_string().unwrap().bytes, b"kept by every save");
    let third = doc.get(pages(&doc)[2]).as_dict().cloned().unwrap();
    assert!(matches!(doc.get(third.reference(b"Thumb").unwrap()).as_ref(), Object::Stream(_)), "the thumbnail is kept");
    assert!(doc.trailer().get(b"ID").is_some());
    assert_eq!(hayro_syntax::Pdf::new(Arc::new(bytes)).expect("hayro opens").pages().len(), 3);
}

#[test]
fn a_page_tree_node_whose_kids_array_is_indirect_is_a_node() {
    // `/Kids 3 0 R`: the node's two pages are pages, the node is not one.
    let input = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids 3 0 R /Count 2 /MediaBox [0 0 200 300] >>")
        .obj("[4 0 R 5 0 R]")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>")
        .stream("", b"0 0 m 100 100 l S")
        .build();
    let bytes = linearize(input).bytes;
    let (_, lin) = parameters(&bytes);
    assert_eq!(int(&lin, b"N"), 2);
    let doc = Document::open(Arc::new(bytes.clone())).unwrap();
    assert_eq!(doc.linearization(), Linearization::Yes);
    assert_eq!(pages(&doc).len(), 2);
    assert_eq!(hayro_syntax::Pdf::new(Arc::new(bytes)).expect("hayro opens").pages().len(), 2);
}

#[test]
fn inputs_with_object_streams_and_linearized_inputs_linearize_again() {
    let packed = write_full(&Document::open(Arc::new(fixture(None))).unwrap(), &SaveOptions::default()).unwrap();
    assert!(String::from_utf8_lossy(&packed).contains("/ObjStm"));
    let once = linearize(packed).bytes;
    let twice = linearize(once.clone()).bytes;
    for bytes in [&once, &twice] {
        let doc = Document::open(Arc::new(bytes.clone())).unwrap();
        assert_eq!(doc.linearization(), Linearization::Yes);
        assert!(!String::from_utf8_lossy(bytes).contains("/ObjStm"), "hint tables locate objects by offset");
        assert_eq!(pages(&doc).len(), 3);
    }
    // The old parameter dictionary and hint stream are not carried over.
    assert_eq!(String::from_utf8_lossy(&twice).matches("/Linearized").count(), 1);
}

#[test]
fn an_incremental_update_makes_a_linearized_file_stale() {
    let bytes = linearize(fixture(None)).bytes;
    let mut doc = Document::open(Arc::new(bytes.clone())).unwrap();
    assert_eq!(doc.linearization(), Linearization::Yes);
    let page = pages(&doc)[0];
    doc.update_dict(page, |d| d.set(b"Rotate".to_vec(), Object::Int(90))).unwrap();
    assert_eq!(doc.linearization(), Linearization::Yes, "edits that are not saved do not change the file");
    let updated = write_incremental(&doc, &SaveOptions::default()).unwrap();
    assert!(updated.starts_with(&bytes));
    let back = Document::open(Arc::new(updated.clone())).unwrap();
    assert!(back.repair_log().is_empty(), "{:?}", back.repair_log());
    assert_eq!(back.linearization(), Linearization::Stale(StaleReason::Updated));
    assert_eq!(back.get(page).as_dict().unwrap().int(b"Rotate"), Some(90), "the update chains to the first-page section");
    assert!(!back.linearization().is_fast_web_view());
    // Linearizing again restores it.
    let again = write_linearized(&back, &SaveOptions::default()).unwrap().bytes;
    assert_eq!(Document::open(Arc::new(again)).unwrap().linearization(), Linearization::Yes);
}

/// `bytes` with the first `from` replaced by `to` (the header's binary comment is not UTF-8).
fn replace_once(bytes: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let at = bytes.windows(from.len()).position(|w| w == from).unwrap();
    [&bytes[..at], to, &bytes[at + from.len()..]].concat()
}

#[test]
fn ordinary_and_inconsistent_files_are_told_apart() {
    assert_eq!(Document::open(Arc::new(fixture(None))).unwrap().linearization(), Linearization::No);
    let full = write_full(&Document::open(Arc::new(fixture(None))).unwrap(), &SaveOptions::default()).unwrap();
    assert_eq!(Document::open(Arc::new(full)).unwrap().linearization(), Linearization::No);
    let bytes = linearize(fixture(None)).bytes;
    // Same length, wrong page count.
    let wrong_n = replace_once(&bytes, b"/N 3", b"/N 4");
    assert_eq!(wrong_n.len(), bytes.len());
    assert_eq!(Document::open(Arc::new(wrong_n)).unwrap().linearization(), Linearization::Stale(StaleReason::Pages));
    // Same length, a hint stream offset that points nowhere.
    let (_, lin) = parameters(&bytes);
    let h0 = lin.get(b"H").unwrap().as_array().unwrap()[0].as_int().unwrap();
    let wrong_h = replace_once(&bytes, format!("/H [{h0} ").as_bytes(), format!("/H [{} ", h0 - 1).as_bytes());
    assert_eq!(wrong_h.len(), bytes.len());
    assert_eq!(Document::open(Arc::new(wrong_h)).unwrap().linearization(), Linearization::Stale(StaleReason::Parameters));
    // Same length, a hint stream length that stops inside its `endobj` (one byte short only
    // drops the end-of-line after it, which readers accept).
    let h1 = lin.get(b"H").unwrap().as_array().unwrap()[1].as_int().unwrap();
    let wrong_len = replace_once(&bytes, format!("/H [{h0} {h1}]").as_bytes(), format!("/H [{h0} {}]", h1 - 3).as_bytes());
    assert_eq!(wrong_len.len(), bytes.len(), "h1 = {h1}");
    assert_eq!(Document::open(Arc::new(wrong_len)).unwrap().linearization(), Linearization::Stale(StaleReason::Parameters));
    // A different length.
    let mut longer = bytes.clone();
    longer.extend_from_slice(b"\n% trailing comment\n");
    assert_eq!(Document::open(Arc::new(longer)).unwrap().linearization(), Linearization::Stale(StaleReason::Updated));
}

#[test]
fn documents_that_cannot_be_linearized_are_refused_with_a_reason() {
    let no_pages = Pdf::new().obj("<< /Type /Catalog /Pages 2 0 R >>").obj("<< /Type /Pages /Kids [] /Count 0 >>").build();
    let twice = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R 3 0 R] /Count 2 >>")
        .obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>")
        .build();
    let direct = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [<< /Type /Page /MediaBox [0 0 10 10] >>] /Count 1 >>")
        .build();
    let looped = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R 2 0 R] /Count 1 >>")
        .obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>")
        .build();
    for (bytes, why) in [(no_pages, "no pages"), (twice, "more than once"), (direct, "not an indirect object"), (looped, "more than once")] {
        let doc = Document::open(Arc::new(bytes)).unwrap();
        match write_linearized(&doc, &SaveOptions::default()) {
            Err(CosError::Linearize(reason)) => assert!(reason.contains(why), "{reason}"),
            other => panic!("{why}: {other:?}"),
        }
    }
}

const ALGORITHMS: [Algorithm; 4] = [Algorithm::Rc4_40, Algorithm::Rc4_128, Algorithm::Aes128, Algorithm::Aes256];

fn encrypted(alg: Algorithm) -> Vec<u8> {
    let mut doc = Document::open(Arc::new(fixture(Some("UseOutlines")))).unwrap();
    doc.set_encryption(&NewEncryption {
        algorithm: alg,
        user_password: "user",
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [5; 32],
    })
    .unwrap();
    write_linearized(&doc, &SaveOptions::default()).unwrap().bytes
}

#[test]
fn encrypted_documents_are_linearized_with_their_encryption() {
    for alg in ALGORITHMS {
        let bytes = encrypted(alg);
        assert!(!String::from_utf8_lossy(&bytes).contains("First page"), "{alg:?}: plaintext leaked");
        assert!(matches!(Document::open(Arc::new(bytes.clone())), Err(CosError::NeedsPassword)));
        let doc = Document::open_with_password(Arc::new(bytes.clone()), Some("user")).unwrap();
        assert_eq!(doc.linearization(), Linearization::Yes, "{alg:?}");
        let page = doc.get(pages(&doc)[0]).as_dict().cloned().unwrap();
        let Object::Stream(s) = doc.get(page.reference(b"Contents").unwrap()).as_ref().clone() else { panic!() };
        assert_eq!(s.decoded().unwrap(), b"BT /F1 24 Tf 20 150 Td (First page) Tj ET");
        // The hint stream is encrypted like any stream and decrypts to valid tables.
        let hint = hints(&bytes, &doc, 3);
        assert_eq!(hint.pages.len(), 3);
        // Opened again with the encryption it was saved with, it linearizes again.
        let again = write_linearized(&doc, &SaveOptions::default()).unwrap().bytes;
        let again = Document::open_with_password(Arc::new(again), Some("owner")).unwrap();
        assert_eq!(again.linearization(), Linearization::Yes);
    }
}

fn qpdf_report(qpdf: &pdfcraft_oracle::Oracle, bytes: &[u8], args: &[&str]) -> (bool, String) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("pdfcraft-linearized-{}-{n}.pdf", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let out = qpdf.command().args(args).arg(&path).output().unwrap();
    let _ = std::fs::remove_file(&path);
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// The independent oracle: qpdf checks the parameter dictionary, both cross-reference sections
/// and every hint table entry against the objects it finds for each page.
#[test]
fn qpdf_finds_no_linearization_errors() {
    let qpdf = pdfcraft_oracle::or_skip!(pdfcraft_oracle::qpdf());
    let packed = write_full(&Document::open(Arc::new(fixture(None))).unwrap(), &SaveOptions::default()).unwrap();
    let inputs = [
        ("outlines closed", fixture(None)),
        ("outlines open", fixture(Some("UseOutlines"))),
        ("single page", single_page()),
        ("twelve pages", many_pages()),
        ("object streams", packed),
    ];
    for (name, input) in inputs {
        let bytes = linearize(input).bytes;
        let again = linearize(bytes.clone()).bytes;
        for bytes in [bytes, again] {
            let (ok, report) = qpdf_report(&qpdf, &bytes, &["--check-linearization"]);
            assert!(ok && report.contains("no linearization errors") && !report.contains("WARNING"), "{name}: {report}");
            let (ok, report) = qpdf_report(&qpdf, &bytes, &["--check"]);
            assert!(ok && report.contains("File is linearized") && !report.contains("WARNING"), "{name}: {report}");
        }
    }
    for alg in ALGORITHMS {
        let bytes = encrypted(alg);
        let (ok, report) = qpdf_report(&qpdf, &bytes, &["--password=user", "--check-linearization"]);
        assert!(ok && report.contains("no linearization errors") && !report.contains("WARNING"), "{alg:?}: {report}");
        let (ok, report) = qpdf_report(&qpdf, &bytes, &["--password=owner", "--check"]);
        assert!(ok && !report.contains("WARNING"), "{alg:?}: {report}");
    }
}

/// qpdf agrees that an incremental update leaves a valid file that is no longer linearized.
#[test]
fn qpdf_sees_an_updated_linearized_file_as_no_longer_linearized() {
    let qpdf = pdfcraft_oracle::or_skip!(pdfcraft_oracle::qpdf());
    let mut doc = Document::open(Arc::new(linearize(fixture(None)).bytes)).unwrap();
    let page = pages(&doc)[1];
    doc.update_dict(page, |d| d.set(b"Rotate".to_vec(), Object::Int(180))).unwrap();
    let updated = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let (ok, report) = qpdf_report(&qpdf, &updated, &["--check"]);
    assert!(ok && !report.contains("WARNING") && !report.contains("File is linearized"), "{report}");
}
