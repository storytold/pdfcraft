use super::*;
use std::error::Error;
use std::io;

mod reference;
use reference::{OldPage, interpret};

#[test]
fn rtl_marked_text_limit_fails_without_appending_partial_content() {
    let mut doc = PdfDoc::new();
    let page = doc.add_page(600.0, 300.0);
    page.content = " ".repeat(MAX_PAGE_CONTENT - 8);
    let before = page.content.len();
    page.text_with_direction(Point::new(10.0, 20.0), 12.0, Font::Helvetica, [0, 0, 0], "ABC !", TextDirection::RightToLeft);
    assert_eq!(page.content.len(), before, "the marked-content block must append atomically");
    assert!(doc.finish().unwrap_err().to_string().contains("size limit"));
}

#[test]
fn a_paragraph_separator_keeps_right_to_left_latin_on_one_line() {
    let visual = |text| arabic::latin_visual(text, TextDirection::RightToLeft).unwrap().into_owned();
    assert_eq!(visual("ABC\r"), visual("ABC "));
    assert_eq!(visual("ABC\u{2029}DEF"), visual("ABC DEF"));
}

#[test]
fn cjk_in_a_right_to_left_cell_keeps_the_cid_font() {
    let mut doc = PdfDoc::new();
    doc.add_page(600.0, 300.0).text_with_direction(Point::new(10.0, 20.0), 12.0, Font::Helvetica, [0, 0, 0], "编号!", TextDirection::RightToLeft);
    let pdf = doc.finish().unwrap();
    check_xref(&pdf).unwrap();
    let cjk = find(&pdf, b"<7F1653F7> Tj").expect("CJK run set in the CID font");
    assert!(find(&pdf, b"/Encoding /UniGB-UTF16-H").is_some());
    assert!(find(&pdf, b"(!) Tj").is_some_and(|bang| bang < cjk), "the trailing ! is drawn left of the CJK run");
    assert_eq!(utf16_entry(&pdf, "/ActualText").unwrap(), "编号!");
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}
fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

/// Parses the xref table back and checks each in-use entry points at `N 0 obj`.
fn check_xref(pdf: &[u8]) -> Result<usize, Box<dyn Error>> {
    let sx = rfind(pdf, b"startxref\n").ok_or_else(|| io::Error::other("missing startxref"))?;
    let rest = std::str::from_utf8(&pdf[sx + 10..])?;
    let off: usize = rest.lines().next().ok_or_else(|| io::Error::other("missing xref offset"))?.trim().parse()?;
    assert!(pdf[off..].starts_with(b"xref\n"), "startxref points at xref");
    let table = std::str::from_utf8(&pdf[off..sx])?;
    let mut lines = table.lines();
    assert_eq!(lines.next(), Some("xref"));
    let hdr: Vec<usize> =
        lines.next().ok_or_else(|| io::Error::other("missing xref header"))?.split(' ').map(str::parse).collect::<Result<_, _>>()?;
    assert_eq!(hdr.first(), Some(&0));
    let count = *hdr.get(1).ok_or_else(|| io::Error::other("missing xref count"))?;
    let mut checked = 0;
    for i in 0..count {
        let e = lines.next().ok_or_else(|| io::Error::other("missing xref entry"))?;
        assert_eq!(e.len(), 19, "entry `{e}` (20 bytes with newline)");
        let o: usize = e.get(..10).ok_or_else(|| io::Error::other("short xref offset"))?.parse()?;
        if e.trim_end().ends_with('n') {
            let want = format!("{i} 0 obj");
            assert!(pdf[o..].starts_with(want.as_bytes()), "object {i} at {o}");
            checked += 1;
        }
    }
    assert!(table.contains("trailer"));
    assert!(table.contains(&format!("/Size {count}")));
    Ok(checked)
}

#[test]
fn minimal_document() -> Result<(), Box<dyn Error>> {
    let pdf = PdfDoc::new().finish()?;
    assert!(pdf.starts_with(b"%PDF-1.7"));
    assert!(pdf.ends_with(b"%%EOF\n"));
    assert!(check_xref(&pdf)? >= 5);
    Ok(())
}

#[test]
fn drawing_and_text() -> Result<(), Box<dyn Error>> {
    let mut d = PdfDoc::new();
    d.info.title = "Report (Q1)".into();
    let p = d.add_page(612.0, 792.0);
    p.fill_rect(10.0, 10.0, 100.0, 20.0, [255, 255, 0]);
    p.rect_stroke(10.0, 10.0, 100.0, 20.0, 0.5, [0, 0, 0]);
    p.stroke_line(0.0, 0.0, 612.0, 792.0, 1.0, [255, 0, 0]);
    p.stroke_line_dashed(Point::new(0.0, 5.0), Point::new(100.0, 5.0), 1.0, [0, 0, 0], &[3.0, 2.0]);
    p.polygon(&[[0.0, 0.0], [10.0, 0.0], [5.0, 8.0]], Some([0, 0, 255]), Some(([0, 0, 0], 1.0)));
    p.circle(300.0, 300.0, 20.0, Some([0, 128, 0]), None);
    p.save();
    p.clip_rect(0.0, 0.0, 50.0, 50.0);
    p.text(72.0, 72.0, 12.0, Font::Helvetica, [0, 0, 0], "Hello (world) \\ café €5");
    p.restore();
    p.text_rotated(Point::new(100.0, 100.0), 9.0, Font::TimesBold, [0, 0, 0], "Up", -std::f32::consts::FRAC_PI_2);
    let p2 = d.add_page(842.0, 595.0);
    p2.text(10.0, 20.0, 10.0, Font::CourierBold, [0, 0, 0], "Second");
    assert_eq!(d.page_count(), 2);
    let pdf = d.finish()?;
    check_xref(&pdf)?;
    assert!(find(&pdf, b"(Hello \\(world\\) \\\\ caf\\351 \\2005) Tj").is_some());
    assert!(find(&pdf, b"(Second) Tj").is_some());
    assert!(find(&pdf, b"/BaseFont /Helvetica /Encoding /WinAnsiEncoding").is_some());
    assert!(find(&pdf, b"/BaseFont /Courier-Bold").is_some());
    assert!(find(&pdf, b"/BaseFont /Times-Bold").is_some());
    assert!(find(&pdf, b"/Count 2").is_some());
    assert!(find(&pdf, b"/MediaBox [0 0 842 595]").is_some());
    assert!(find(&pdf, b"/Title (Report \\(Q1\\))").is_some());
    // Top-left origin: a line from (0,0) starts at the top of the page.
    assert!(find(&pdf, b"0 792 m 612 0 l S").is_some());
    Ok(())
}

#[test]
fn uri_and_page_links_are_real_pdf_annotations() -> Result<(), Box<dyn Error>> {
    let mut d = PdfDoc::new();
    let first = d.add_page(200.0, 300.0);
    first.link_uri(10.0, 20.0, 80.0, 30.0, "https://example.org/a(b)?q=£");
    first.link_page(Rect::new(100.0, 40.0, 50.0, 20.0), 1, Point::new(35.0, 60.0));
    first.link_uri(-20.0, -20.0, 40.0, 40.0, "mailto:a@example.org");
    first.link_uri(0.0, 0.0, f32::NAN, 10.0, "https://bad.example");
    first.link_uri(0.0, 0.0, 10.0, 10.0, "javascript:alert(1)");
    let second = d.add_page(100.0, 200.0);
    second.link_page(Rect::new(0.0, 0.0, 10.0, 10.0), 99, Point::new(0.0, 0.0));
    let pdf = d.finish()?;
    check_xref(&pdf)?;
    let s = String::from_utf8_lossy(&pdf);
    assert_eq!(s.matches("/Subtype /Link").count(), 3);
    assert!(s.contains("/Annots ["));
    assert!(s.contains("/Rect [10 250 90 280]"));
    assert!(s.contains("/Rect [0 280 20 300]"));
    assert!(s.contains("/URI (https://example.org/a\\(b\\)?q=%C2%A3)"));
    assert!(s.contains("/XYZ 35 140 null"));
    assert!(!s.contains("https://bad.example"));
    Ok(())
}

#[test]
fn images_with_alpha() -> Result<(), Box<dyn Error>> {
    let mut d = PdfDoc::new();
    let p = d.add_page(200.0, 200.0);
    let mut px = Vec::new();
    for i in 0..16u8 {
        px.extend_from_slice(&[i * 10, 0, 0, if i < 8 { 255 } else { 128 }]);
    }
    p.image_rgba(Rect::new(10.0, 10.0, 40.0, 40.0), &px, [4, 4]);
    p.image_rgba(Rect::new(10.0, 10.0, 40.0, 40.0), &[255; 16], [2, 2]); // opaque: no mask
    p.image_rgba(Rect::new(0.0, 0.0, 1.0, 1.0), &[1, 2, 3], [4, 4]); // too short: ignored
    let pdf = d.finish()?;
    check_xref(&pdf)?;
    assert!(find(&pdf, b"/SMask").is_some());
    assert!(find(&pdf, b"/Im1 Do").is_some());
    assert!(find(&pdf, b"/Im2 Do").is_some());
    assert!(find(&pdf, b"/Im3").is_none());
    Ok(())
}

#[test]
fn run_length_roundtrip() {
    fn decode(d: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < d.len() {
            let l = d[i];
            i += 1;
            if l == 128 {
                break;
            } else if l < 128 {
                out.extend_from_slice(&d[i..i + l as usize + 1]);
                i += l as usize + 1;
            } else {
                out.extend(std::iter::repeat_n(d[i], 257 - l as usize));
                i += 1;
            }
        }
        out
    }
    let cases: Vec<Vec<u8>> = vec![
        vec![],
        vec![1],
        vec![1, 1],
        vec![1, 2, 3, 3, 3, 3, 4, 5, 5],
        (0..1000).map(|i| (i / 7) as u8).collect(),
        (0..1000).map(|i| (i * 31 % 251) as u8).collect(),
        vec![9; 300],
    ];
    for c in cases {
        assert_eq!(decode(&run_length(&c)), c);
    }
}

#[test]
fn widths() {
    assert!((text_width(Font::Helvetica, 10.0, "Hello") - 22.78).abs() < 0.01);
    assert!(text_width(Font::HelveticaBold, 10.0, "Hello") > text_width(Font::Helvetica, 10.0, "Hello"));
    assert_eq!(text_width(Font::Courier, 10.0, "abc"), 18.0);
    assert!(text_width(Font::TimesRoman, 12.0, "é") > 0.0);
    assert_eq!(text_width(Font::Helvetica, 10.0, ""), 0.0);
    assert_eq!(Font::family_for("Calibri"), Family::Sans);
    assert_eq!(Font::family_for("Times New Roman"), Family::Serif);
    assert_eq!(Font::family_for("Consolas"), Family::Mono);
    assert_eq!(Font::styled(Family::Serif, true, true), Font::TimesBoldItalic);
}

#[test]
fn hostile_inputs_never_panic() -> Result<(), Box<dyn Error>> {
    let mut d = PdfDoc::new();
    let p = d.add_page(f32::NAN, -5.0);
    p.text(f32::INFINITY, f32::NAN, 1e30, Font::Helvetica, [0, 0, 0], "x");
    p.fill_rect(0.0, 0.0, -1.0, 5.0, [0, 0, 0]);
    p.polygon(&[], None, None);
    p.circle(0.0, 0.0, -3.0, Some([0, 0, 0]), None);
    p.restore();
    p.restore();
    p.save();
    p.image_rgba(Rect::new(0.0, 0.0, 10.0, 10.0), &[], [u32::MAX, u32::MAX]);
    let pdf = d.finish()?;
    check_xref(&pdf)?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Writer cleanup: equivalence with the previous content writer, structure, and one test per
// behaviour change.
// ---------------------------------------------------------------------------------------------

/// Offsets of the in-use objects, by id, from the xref table.
fn xref_offsets(pdf: &[u8]) -> Vec<usize> {
    let sx = rfind(pdf, b"startxref\n").unwrap();
    let rest = std::str::from_utf8(&pdf[sx + 10..]).unwrap();
    let off: usize = rest.lines().next().unwrap().trim().parse().unwrap();
    let table = std::str::from_utf8(&pdf[off..sx]).unwrap();
    table.lines().skip(2).take_while(|l| l.len() == 18 || l.len() == 19).map(|e| e[..10].parse().unwrap()).collect()
}

struct Obj {
    dict: String,
    stream: Option<Vec<u8>>,
}

/// Parses every object of a document written by this crate, checking that each stream's
/// `/Length` matches its data and is followed by `endstream`/`endobj`.
fn parse_objects(pdf: &[u8]) -> Vec<Obj> {
    let offsets = xref_offsets(pdf);
    let mut objs = Vec::new();
    for (id, off) in offsets.iter().enumerate() {
        if id == 0 {
            objs.push(Obj { dict: String::new(), stream: None });
            continue;
        }
        let tail = &pdf[*off..];
        let head = format!("{id} 0 obj\n");
        assert!(tail.starts_with(head.as_bytes()), "object {id}");
        let body = &tail[head.len()..];
        let end = find(body, b"\nendobj\n").unwrap();
        match find(body, b"\nstream\n").filter(|s| *s < end) {
            Some(s) => {
                let dict = String::from_utf8(body[..s].to_vec()).unwrap();
                let l = dict.find("/Length ").unwrap() + 8;
                let len: usize = dict[l..].split(' ').next().unwrap().parse().unwrap();
                let data = &body[s + 8..];
                assert!(data[len..].starts_with(b"\nendstream\nendobj\n"), "object {id}: /Length {len} is wrong");
                objs.push(Obj { dict, stream: Some(data[..len].to_vec()) });
            }
            None => objs.push(Obj { dict: String::from_utf8(body[..end].to_vec()).unwrap(), stream: None }),
        }
    }
    objs
}

fn ref_of(dict: &str, key: &str) -> Option<usize> {
    let i = dict.find(key)? + key.len();
    dict[i..].trim_start().split(' ').next()?.parse().ok()
}

struct ParsedPage {
    media: String,
    content: String,
    /// `(Im name, object id)`.
    xobjects: Vec<(String, usize)>,
}

fn parse_pages(pdf: &[u8]) -> (Vec<Obj>, Vec<ParsedPage>) {
    let objs = parse_objects(pdf);
    let mut pages = Vec::new();
    for o in &objs {
        if !o.dict.contains("/Type /Page ") {
            continue;
        }
        let m = o.dict.find("/MediaBox [").unwrap() + 10;
        let media = o.dict[m..m + o.dict[m..].find(']').unwrap() + 1].to_string();
        let content_id = ref_of(&o.dict, "/Contents").unwrap();
        let content = String::from_utf8(objs[content_id].stream.clone().unwrap()).unwrap();
        let mut xobjects = Vec::new();
        if let Some(x) = o.dict.find("/XObject << ") {
            let rest = &o.dict[x + 11..];
            let rest = &rest[..rest.find(">>").unwrap()];
            let toks: Vec<&str> = rest.split_whitespace().collect();
            for c in toks.chunks(4) {
                xobjects.push((c[0].to_string(), c[1].parse().unwrap()));
            }
        }
        pages.push(ParsedPage { media, content, xobjects });
    }
    (objs, pages)
}

fn rle_decode(d: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < d.len() {
        let l = d[i];
        i += 1;
        if l == 128 {
            break;
        } else if l < 128 {
            out.extend_from_slice(&d[i..i + l as usize + 1]);
            i += l as usize + 1;
        } else {
            out.extend(std::iter::repeat_n(d[i], 257 - l as usize));
            i += 1;
        }
    }
    out
}

/// Decodes an image XObject: `(width, height, rgb, alpha)`.
fn decode_image(objs: &[Obj], id: usize) -> (u32, u32, Vec<u8>, Option<Vec<u8>>) {
    let o = &objs[id];
    let w = ref_of(&o.dict, "/Width").unwrap() as u32;
    let h = ref_of(&o.dict, "/Height").unwrap() as u32;
    let rgb = rle_decode(o.stream.as_ref().unwrap());
    let alpha = ref_of(&o.dict, "/SMask").map(|m| rle_decode(objs[m].stream.as_ref().unwrap()));
    (w, h, rgb, alpha)
}

/// Deterministic pseudo-random numbers for the corpus.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn f(&mut self) -> f32 {
        self.below(600) as f32 + self.below(4) as f32 * 0.25
    }
}

const PALETTE: [Rgb; 4] = [[0, 0, 0], [255, 0, 0], [12, 200, 99], [255, 255, 255]];
const WIDTHS: [f32; 3] = [0.5, 1.0, 2.5];
const DASHES: [&[f32]; 3] = [&[], &[3.0, 2.0], &[1.0]];

/// A picture used by the corpus: `(rgba, w, h)`.
fn pictures() -> Vec<(Vec<u8>, u32, u32)> {
    let mut with_alpha = Vec::new();
    for i in 0..16u8 {
        with_alpha.extend_from_slice(&[i * 10, 0, 7, if i < 8 { 255 } else { 128 }]);
    }
    let opaque: Vec<u8> = (0..4 * 9).map(|i| if i % 4 == 3 { 255 } else { (i * 7) as u8 }).collect();
    let mut last_px = vec![200u8; 4 * 6];
    for px in last_px.chunks_mut(4) {
        px[3] = 255;
    }
    last_px[4 * 5 + 3] = 3; // only the very last pixel is translucent
    vec![(with_alpha, 4, 4), (opaque, 3, 3), (last_px, 3, 2)]
}

/// Applies a random operation to both writers.
fn random_op(rng: &mut Lcg, new: &mut Page, old: &mut OldPage, pics: &[(Vec<u8>, u32, u32)], depth: &mut usize) {
    let color = PALETTE[rng.below(4) as usize];
    let width = WIDTHS[rng.below(3) as usize];
    let dash = DASHES[rng.below(3) as usize];
    let (x, y, w, h) = (rng.f(), rng.f(), rng.f() / 4.0 + 1.0, rng.f() / 4.0 + 1.0);
    let pts = [[x, y], [x + w, y], [x + w, y + h]];
    let stroke = if rng.below(2) == 0 { Some((PALETTE[rng.below(4) as usize], WIDTHS[rng.below(3) as usize])) } else { None };
    let fill = if rng.below(2) == 0 { Some(color) } else { None };
    match rng.below(14) {
        0 | 1 => {
            new.fill_rect(x, y, w, h, color);
            old.fill_rect(x, y, w, h, color);
        }
        2 => {
            new.rect_stroke(x, y, w, h, width, color);
            old.rect_stroke(x, y, w, h, width, color);
        }
        3 => {
            new.stroke_line_dashed(Point::new(x, y), Point::new(x + w, y + h), width, color, dash);
            old.stroke_line_dashed(Point::new(x, y), Point::new(x + w, y + h), width, color, dash);
        }
        4 => {
            new.polyline(&pts, width, color, dash);
            old.polyline(&pts, width, color, dash);
        }
        5 => {
            new.polygon(&pts, fill, stroke);
            old.polygon(&pts, fill, stroke);
        }
        6 => {
            new.circle(x, y, w, fill, stroke);
            old.circle(x, y, w, fill, stroke);
        }
        7 | 8 => {
            let font = Font::ALL[rng.below(12) as usize];
            let angle = if rng.below(3) == 0 { 0.5 } else { 0.0 };
            new.text_rotated(Point::new(x, y), 10.0, font, color, "Hi (there)", angle);
            old.text_rotated(Point::new(x, y), 10.0, font, color, "Hi (there)", angle);
        }
        9 => {
            new.clip_rect(x, y, w, h);
            old.clip_rect(x, y, w, h);
        }
        10 if *depth < 20 => {
            new.save();
            old.save();
            *depth += 1;
        }
        11 if *depth > 0 => {
            new.restore();
            old.restore();
            *depth -= 1;
        }
        12 => {
            let (px, pw, ph) = &pics[rng.below(3) as usize];
            new.image_rgba(Rect::new(x, y, w, h), px, [*pw, *ph]);
            old.image(x, y, w, h);
        }
        _ => {
            new.text(x, y, 9.0, Font::Helvetica, color, "Plain");
            old.text_rotated(Point::new(x, y), 9.0, Font::Helvetica, color, "Plain", 0.0);
        }
    }
}

/// Builds the same random multi-page document with the new writer and the old content writer.
fn corpus_doc(seed: u64) -> (Vec<u8>, Vec<String>) {
    let mut rng = Lcg(seed);
    let pics = pictures();
    let mut doc = PdfDoc::new();
    let mut old_pages = Vec::new();
    for _ in 0..(1 + rng.below(4)) {
        let mut old = OldPage::new(792.0);
        let mut depth = 0usize;
        let page = doc.add_page(612.0, 792.0);
        for _ in 0..120 {
            random_op(&mut rng, page, &mut old, &pics, &mut depth);
        }
        while depth > 0 {
            page.restore();
            old.restore();
            depth -= 1;
        }
        old_pages.push(old.content);
    }
    (doc.finish().unwrap(), old_pages)
}

#[test]
fn new_writer_is_equivalent_to_the_old_one() -> Result<(), Box<dyn Error>> {
    let mut style_new = 0;
    let mut style_old = 0;
    for seed in 1..=40u64 {
        let (pdf, old_pages) = corpus_doc(seed);
        check_xref(&pdf)?;
        let (_, pages) = parse_pages(&pdf);
        assert_eq!(pages.len(), old_pages.len());
        for (p, old) in pages.iter().zip(&old_pages) {
            assert_eq!(p.media, "[0 0 612 792]");
            let new = interpret(&p.content);
            let old = interpret(old);
            assert!(new.balanced && old.balanced, "q/Q balanced (seed {seed})");
            assert_eq!(new.paints.len(), old.paints.len(), "seed {seed}");
            for (i, (a, b)) in new.paints.iter().zip(&old.paints).enumerate() {
                // Round joins leaking from polylines into later rectangles is the old bug (B7),
                // compared separately in `join_does_not_leak_from_polylines`.
                let mut a = a.clone();
                let mut b = b.clone();
                a.join.clear();
                b.join.clear();
                assert_eq!(a, b, "seed {seed}, painting op {i}");
            }
            style_new += new.style_ops;
            style_old += old.style_ops;
        }
    }
    // Sanity: the corpus really contains style changes, and the new writer emits fewer.
    assert!(style_old > 1000, "corpus too small: {style_old}");
    assert!(style_new < style_old, "{style_new} vs {style_old}");
    Ok(())
}

#[test]
fn structure_is_valid_for_all_documents() -> Result<(), Box<dyn Error>> {
    for seed in 1..=40u64 {
        let (pdf, _) = corpus_doc(seed);
        check_xref(&pdf)?;
        parse_objects(&pdf); // every /Length matches its data
    }
    let empty = PdfDoc::new().finish()?;
    check_xref(&empty)?;
    parse_objects(&empty);
    Ok(())
}

#[test]
fn images_decode_to_their_pixels() {
    let pics = pictures();
    let mut d = PdfDoc::new();
    for _ in 0..2 {
        let p = d.add_page(300.0, 300.0);
        for (px, w, h) in &pics {
            p.image_rgba(Rect::new(1.0, 1.0, 20.0, 20.0), px, [*w, *h]);
        }
    }
    let pdf = d.finish().unwrap();
    let (objs, pages) = parse_pages(&pdf);
    assert_eq!(pages.len(), 2);
    for page in &pages {
        assert_eq!(page.xobjects.len(), 3);
        for ((px, w, h), (_, id)) in pics.iter().zip(&page.xobjects) {
            let (dw, dh, rgb, alpha) = decode_image(&objs, *id);
            assert_eq!((dw, dh), (*w, *h));
            let want_rgb: Vec<u8> = px.chunks(4).flat_map(|c| [c[0], c[1], c[2]]).collect();
            assert_eq!(rgb, want_rgb);
            let want_alpha: Vec<u8> = px.chunks(4).map(|c| c[3]).collect();
            match alpha {
                Some(a) => assert_eq!(a, want_alpha),
                None => assert!(want_alpha.iter().all(|a| *a == 255)),
            }
        }
    }
}

/// O6: no alpha plane (and no SMask object) for an opaque image; a single translucent pixel
/// anywhere (first, middle, last) produces the full plane.
#[test]
fn opaque_images_have_no_soft_mask() {
    let mut d = PdfDoc::new();
    let p = d.add_page(100.0, 100.0);
    p.image_rgba(Rect::new(0.0, 0.0, 10.0, 10.0), &[255; 4 * 12], [4, 3]);
    assert!(p.images[0].alpha.is_none());
    let pdf = d.finish().unwrap();
    assert!(find(&pdf, b"/SMask").is_none());
    assert_eq!(pdf.windows(15).filter(|w| *w == b"/Subtype /Image").count(), 1);

    for translucent in [0usize, 5, 11] {
        let mut px = vec![255u8; 4 * 12];
        px[translucent * 4 + 3] = 7;
        let mut d = PdfDoc::new();
        d.add_page(100.0, 100.0).image_rgba(Rect::new(0.0, 0.0, 10.0, 10.0), &px, [4, 3]);
        let (objs, pages) = parse_pages(&d.finish().unwrap());
        let (_, _, _, alpha) = decode_image(&objs, pages[0].xobjects[0].1);
        let mut want = vec![255u8; 12];
        want[translucent] = 7;
        assert_eq!(alpha, Some(want));
    }
}

/// O4: the same picture on five pages is stored once (image + mask), by every page.
#[test]
fn identical_images_are_stored_once() -> Result<(), Box<dyn Error>> {
    let pics = pictures();
    let (px, w, h) = &pics[0]; // has alpha: image object + mask object
    let mut d = PdfDoc::new();
    for _ in 0..5 {
        let p = d.add_page(200.0, 200.0);
        p.image_rgba(Rect::new(1.0, 1.0, 20.0, 20.0), px, [*w, *h]);
        p.image_rgba(Rect::new(30.0, 1.0, 20.0, 20.0), px, [*w, *h]); // twice on one page
    }
    let pdf = d.finish()?;
    check_xref(&pdf)?;
    assert_eq!(pdf.windows(15).filter(|w| *w == b"/Subtype /Image").count(), 2, "one image + one soft mask");
    let (_, pages) = parse_pages(&pdf);
    let ids: Vec<usize> = pages.iter().flat_map(|p| p.xobjects.iter().map(|x| x.1)).collect();
    assert_eq!(ids.len(), 5, "one XObject entry per page");
    assert!(ids.iter().all(|i| *i == ids[0]));
    for p in &pages {
        assert_eq!(p.content.matches(" Do").count(), 2);
    }
    Ok(())
}

/// O4: a picture that differs in one byte, or only in size, is not merged.
#[test]
fn different_images_are_not_merged() {
    let a = vec![50u8; 4 * 4];
    let mut b = a.clone();
    b[5] = 51;
    let mut d = PdfDoc::new();
    let p = d.add_page(100.0, 100.0);
    p.image_rgba(Rect::new(0.0, 0.0, 5.0, 5.0), &a, [4, 1]);
    p.image_rgba(Rect::new(0.0, 0.0, 5.0, 5.0), &b, [4, 1]);
    p.image_rgba(Rect::new(0.0, 0.0, 5.0, 5.0), &a, [2, 2]);
    d.add_page(100.0, 100.0).image_rgba(Rect::new(0.0, 0.0, 5.0, 5.0), &a, [4, 1]);
    let pdf = d.finish().unwrap();
    assert_eq!(pdf.windows(15).filter(|w| *w == b"/Subtype /Image").count(), 6, "three distinct pictures, each with a soft mask (alpha 50)");
}

/// O4: many distinct pictures on one page are looked up by hash, not by comparing each new
/// picture with every earlier one (which made dense picture exports quadratic).
#[test]
fn many_distinct_images_on_one_page_are_not_compared_pairwise() {
    const N: u32 = 3000;
    let mut d = PdfDoc::new();
    let p = d.add_page(612.0, 792.0);
    for i in 0..N {
        let [a, b, c, _] = i.to_le_bytes();
        p.image_rgba(Rect::new(0.0, 0.0, 1.0, 1.0), &[a, b, c, 255], [1, 1]);
    }
    // A repeat of the first picture still shares its XObject.
    p.image_rgba(Rect::new(5.0, 5.0, 1.0, 1.0), &[0, 0, 0, 255], [1, 1]);
    assert_eq!(p.images.len(), N as usize);
    assert!(p.content.ends_with("/Im1 Do Q\n"), "the repeated picture reuses /Im1");
    assert!(p.image_comparisons <= 1, "{} pixel comparisons for {N} distinct pictures", p.image_comparisons);
    let pdf = d.finish().unwrap();
    assert_eq!(pdf.windows(15).filter(|w| *w == b"/Subtype /Image").count(), N as usize);
}

/// O4: equal hashes alone never merge images; the pixels are compared.
#[test]
fn hash_collisions_do_not_merge_images() {
    let mut d = PdfDoc::new();
    for v in [10u8, 20] {
        d.add_page(100.0, 100.0).image_rgba(Rect::new(0.0, 0.0, 5.0, 5.0), &[v; 16], [2, 2]);
    }
    for p in 0..2 {
        d.page_mut(p).unwrap().images[0].hash = 42; // forced collision
    }
    let (objs, pages) = parse_pages(&d.finish().unwrap());
    let ids: Vec<usize> = pages.iter().map(|p| p.xobjects[0].1).collect();
    assert_ne!(ids[0], ids[1]);
    assert_eq!(decode_image(&objs, ids[0]).2, vec![10; 12]);
    assert_eq!(decode_image(&objs, ids[1]).2, vec![20; 12]);
}

/// S5: the colour component table equals the old float formatting for every byte.
#[test]
fn colour_components_match_the_float_formatting() {
    for v in 0..=255u8 {
        let mut s = String::new();
        push_unit(&mut s, v);
        assert_eq!(s, n(v as f32 / 255.0), "byte {v}");
    }
    let mut s = String::new();
    push_rgb(&mut s, [255, 128, 0], true);
    assert_eq!(s, "1 0.502 0 RG");
}

/// S3: the obsolete /ProcSet entry is gone.
#[test]
fn no_procset() {
    let (pdf, _) = corpus_doc(3);
    assert!(find(&pdf, b"/ProcSet").is_none());
}

/// O3: a style that is already in effect is not emitted again.
#[test]
fn unchanged_style_is_emitted_once() {
    let mut d = PdfDoc::new();
    let p = d.add_page(200.0, 200.0);
    for i in 0..50 {
        p.rect_stroke(i as f32, 0.0, 5.0, 5.0, 1.5, [10, 20, 30]);
    }
    for i in 0..50 {
        p.stroke_line_dashed(Point::new(0.0, i as f32), Point::new(9.0, 9.0), 1.5, [10, 20, 30], &[2.0, 1.0]);
    }
    for _ in 0..50 {
        p.fill_rect(0.0, 0.0, 4.0, 4.0, [1, 2, 3]);
        p.text(5.0, 5.0, 9.0, Font::Helvetica, [1, 2, 3], "x");
    }
    let s = interpret(&p.content);
    assert_eq!(p.content.matches(" RG\n").count(), 1);
    assert_eq!(p.content.matches(" rg\n").count(), 1);
    assert_eq!(p.content.matches(" w\n").count(), 1);
    assert_eq!(p.content.matches(" d\n").count(), 2, "solid, then [2 1]");
    assert_eq!(s.style_ops, 5);
    assert_eq!(s.paints.len(), 50 * 4);
}

/// O3: the tracked style follows q/Q, and a new page starts from scratch.
#[test]
fn style_tracking_follows_save_restore_and_pages() {
    let red = [255, 0, 0];
    let blue = [0, 0, 255];
    let mut d = PdfDoc::new();
    let p = d.add_page(100.0, 100.0);
    p.fill_rect(0.0, 0.0, 5.0, 5.0, red);
    p.save();
    p.fill_rect(0.0, 0.0, 5.0, 5.0, red); // still red inside: nothing to emit
    p.fill_rect(0.0, 0.0, 5.0, 5.0, blue);
    p.restore();
    p.fill_rect(0.0, 0.0, 5.0, 5.0, red); // red again after Q without a new operator
    p.fill_rect(0.0, 0.0, 5.0, 5.0, blue); // but blue must be re-emitted
    let fills: Vec<String> = interpret(&p.content).paints.iter().map(|p| p.fill.clone()).collect();
    assert_eq!(fills, ["1 0 0", "1 0 0", "0 0 1", "1 0 0", "0 0 1"]);
    assert_eq!(p.content.matches(" rg\n").count(), 3, "red once, blue twice");
    let p2 = d.add_page(100.0, 100.0);
    p2.fill_rect(0.0, 0.0, 5.0, 5.0, red);
    assert_eq!(p2.content.matches(" rg\n").count(), 1, "a new page re-emits the colour");
}

/// B6: only 28 nested saves are written, the matching extra restores are no-ops.
#[test]
fn save_nesting_is_limited_to_28() -> Result<(), Box<dyn Error>> {
    let mut d = PdfDoc::new();
    let p = d.add_page(100.0, 100.0);
    for _ in 0..40 {
        p.save();
    }
    p.fill_rect(0.0, 0.0, 5.0, 5.0, [1, 1, 1]);
    assert_eq!(p.content.matches("q\n").count(), 28);
    // The 12 restores that match refused saves do nothing...
    for _ in 0..12 {
        p.restore();
    }
    assert_eq!(p.content.matches("Q\n").count(), 0);
    assert_eq!(interpret(&p.content).max_depth, 28);
    // ...the next 28 undo the accepted ones, and further restores are ignored.
    for _ in 0..40 {
        p.restore();
    }
    assert_eq!(p.content.matches("Q\n").count(), 28);
    let i = interpret(&p.content);
    assert!(i.balanced);
    // An image needs its own q: dropped at the limit, drawn otherwise.
    for _ in 0..28 {
        p.save();
    }
    p.image_rgba(Rect::new(0.0, 0.0, 5.0, 5.0), &[9; 16], [2, 2]);
    assert!(!p.content.contains(" Do"));
    p.restore();
    p.image_rgba(Rect::new(0.0, 0.0, 5.0, 5.0), &[9; 16], [2, 2]);
    assert!(p.content.contains(" Do"));
    let pdf = d.finish()?;
    check_xref(&pdf)?;
    Ok(())
}

/// B6: `finish()` closes what is still open, without exceeding the limit.
#[test]
fn finish_closes_open_saves() -> Result<(), Box<dyn Error>> {
    let mut d = PdfDoc::new();
    let p = d.add_page(100.0, 100.0);
    for _ in 0..33 {
        p.save();
    }
    let (_, pages) = parse_pages(&d.finish()?);
    let i = interpret(&pages[0].content);
    assert!(i.balanced);
    assert_eq!(i.qs, 28);
    Ok(())
}

/// B7: round joins set for a polyline do not leak into later rectangles, lines and circles.
#[test]
fn join_does_not_leak_from_polylines() {
    let mut d = PdfDoc::new();
    let p = d.add_page(200.0, 200.0);
    let pts = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]];
    p.rect_stroke(0.0, 0.0, 5.0, 5.0, 1.0, [0, 0, 0]);
    p.polyline(&pts, 1.0, [0, 0, 0], &[]);
    p.polyline(&pts, 1.0, [0, 0, 0], &[]);
    p.rect_stroke(0.0, 0.0, 5.0, 5.0, 1.0, [0, 0, 0]);
    p.stroke_line(0.0, 0.0, 9.0, 9.0, 1.0, [0, 0, 0]);
    p.polygon(&pts, None, Some(([0, 0, 0], 1.0)));
    p.circle(50.0, 50.0, 5.0, None, Some(([0, 0, 0], 1.0)));
    let joins: Vec<String> = interpret(&p.content).paints.iter().map(|p| p.join.clone()).collect();
    assert_eq!(joins, ["0", "1", "1", "0", "0", "1", "0"]);
    assert_eq!(p.content.matches(" j\n").count(), 4, "1, 0, 1, 0: only on change");
}

fn page_content(d: &PdfDoc) -> &str {
    &d.pages[0].content
}

#[test]
fn text_width_does_not_overflow() {
    // 8,000,000 x 556 units exceeds u32::MAX: a u32 sum panics in debug builds.
    let long = "a".repeat(8_000_000);
    let w = text_width(Font::Helvetica, 10.0, &long);
    assert!(w.is_finite() && w > 0.0);
    assert!(w > text_width(Font::Helvetica, 10.0, &"a".repeat(7_000_000)));
}

#[test]
fn all_zero_dash_is_drawn_solid() {
    let mut d = PdfDoc::new();
    let p = d.add_page(100.0, 100.0);
    p.stroke_line_dashed(Point::new(0.0, 0.0), Point::new(50.0, 0.0), 1.0, [0, 0, 0], &[0.0, 0.0]);
    p.polyline(&[[0.0, 0.0], [5.0, 5.0]], 1.0, [0, 0, 0], &[0.0, 0.0001, f32::NAN]);
    p.stroke_line_dashed(Point::new(0.0, 0.0), Point::new(50.0, 0.0), 1.0, [0, 0, 0], &[3.0, 2.0]);
    let c = page_content(&d);
    // The writer emits style operators only on change: the two solid lines share one `[] 0 d`.
    let dashes: Vec<String> = interpret(c).paints.iter().map(|p| p.dash.clone()).collect();
    assert_eq!(dashes, ["", "", "3 2"]);
    assert_eq!(c.matches("[] 0 d").count(), 1);
    assert!(!c.contains("[0 0]"));
    assert!(c.contains("[3 2] 0 d"));
}

#[test]
fn negative_and_nan_stroke_widths_become_zero() {
    let mut d = PdfDoc::new();
    let p = d.add_page(100.0, 100.0);
    p.rect_stroke(1.0, 1.0, 10.0, 10.0, -3.0, [0, 0, 0]);
    p.stroke_line(0.0, 0.0, 5.0, 5.0, f32::NAN, [0, 0, 0]);
    p.polygon(&[[0.0, 0.0], [5.0, 0.0], [5.0, 5.0]], None, Some(([0, 0, 0], -1.0)));
    p.circle(20.0, 20.0, 5.0, None, Some(([0, 0, 0], f32::NEG_INFINITY)));
    let c = page_content(&d);
    // Style operators are emitted only on change, so judge the width in effect for each stroke.
    let widths: Vec<String> = interpret(c).paints.iter().map(|p| p.width.clone()).collect();
    assert_eq!(widths, ["0", "0", "0", "0"]);
    assert!(!c.contains("-1 w") && !c.contains("-3 w"));
}

#[test]
fn non_finite_image_size_stores_nothing() {
    let mut d = PdfDoc::new();
    let p = d.add_page(200.0, 200.0);
    let px = [255u8; 16];
    for (w, h) in [(f32::NAN, 10.0), (10.0, f32::NAN), (f32::INFINITY, 10.0), (-1.0, 10.0)] {
        p.image_rgba(Rect::new(0.0, 0.0, w, h), &px, [2, 2]);
    }
    p.image_rgba(Rect::new(f32::NAN, 0.0, 10.0, 10.0), &px, [2, 2]);
    assert!(p.images.is_empty());
    assert!(p.content.is_empty());
}

#[test]
fn every_draw_method_survives_hostile_numbers() -> Result<(), Box<dyn Error>> {
    let bad = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -7.0, 0.0, 1.0e30, -1.0e30];
    let px = [200u8; 16];
    let mut d = PdfDoc::new();
    let p = d.add_page(300.0, 300.0);
    for &a in &bad {
        for &b in &bad {
            p.clip_rect(a, b, a, b);
            p.fill_rect(a, b, b, a, [1, 2, 3]);
            p.rect_stroke(a, b, a, b, a, [1, 2, 3]);
            p.stroke_line(a, b, b, a, a, [1, 2, 3]);
            p.stroke_line_dashed(Point::new(a, b), Point::new(b, a), b, [1, 2, 3], &[a, b, 0.0]);
            p.polyline(&[[a, b], [b, a], [1.0, 1.0]], a, [1, 2, 3], &[b, a]);
            p.polygon(&[[a, b], [b, a], [1.0, 1.0]], Some([1, 2, 3]), Some(([4, 5, 6], a)));
            p.circle(a, b, b, Some([1, 2, 3]), Some(([4, 5, 6], a)));
            p.text(a, b, a, Font::Helvetica, [0, 0, 0], "x");
            p.text_rotated(Point::new(a, b), 10.0, Font::TimesRoman, [0, 0, 0], "y", b);
            p.image_rgba(Rect::new(a, b, b, a), &px, [2, 2]);
        }
    }
    p.stroke_line_dashed(Point::new(0.0, 0.0), Point::new(1.0, 1.0), 1.0, [0, 0, 0], &[0.0; 100]);
    let c = page_content(&d);
    assert!(!c.contains("NaN") && !c.contains("inf"));
    let pdf = d.finish()?;
    check_xref(&pdf)?;
    Ok(())
}

/// Decodes the `<FEFF…>` hex string following `key` in the file bytes.
fn utf16_entry(pdf: &[u8], key: &str) -> Result<String, Box<dyn Error>> {
    let at = find(pdf, key.as_bytes()).ok_or_else(|| io::Error::other("missing PDF info key"))? + key.len();
    let end = at + 9000.min(pdf.len() - at);
    let rest = std::str::from_utf8(pdf.get(at..end).ok_or_else(|| io::Error::other("short PDF info value"))?)?;
    let hex = rest.trim_start().strip_prefix("<FEFF").ok_or_else(|| io::Error::other("missing UTF-16 marker"))?;
    let end = hex.find('>').ok_or_else(|| io::Error::other("missing UTF-16 terminator"))?;
    let hex = hex.get(..end).ok_or_else(|| io::Error::other("invalid UTF-16 range"))?;
    let units: Vec<u16> = (0..hex.len()).step_by(4).map(|i| u16::from_str_radix(hex.get(i..i + 4).unwrap_or(""), 16)).collect::<Result<_, _>>()?;
    Ok(String::from_utf16(&units)?)
}

#[test]
fn info_strings_with_non_ascii_are_utf16be() -> Result<(), Box<dyn Error>> {
    let mut doc = PdfDoc::new();
    doc.add_page(200.0, 200.0);
    doc.info.title = "Übersicht €5 – “Q1” 日本 😀".into();
    doc.info.author = "Müller & Søn".into();
    let pdf = doc.finish()?;
    assert_eq!(utf16_entry(&pdf, "/Title")?, "Übersicht €5 – “Q1” 日本 😀");
    assert_eq!(utf16_entry(&pdf, "/Author")?, "Müller & Søn");
    Ok(())
}

#[test]
fn ascii_info_strings_stay_literal_and_escaped() {
    let mut doc = PdfDoc::new();
    doc.add_page(200.0, 200.0);
    doc.info.title = r"Q1 (draft) a\b".into();
    let pdf = doc.finish().unwrap();
    assert!(find(&pdf, br"/Title (Q1 \(draft\) a\\b)").is_some());
    assert!(find(&pdf, b"/Producer (GridCraft PDF)").is_some());
}

#[test]
fn info_strings_are_capped() -> Result<(), Box<dyn Error>> {
    let mut doc = PdfDoc::new();
    doc.add_page(200.0, 200.0);
    doc.info.title = "日".repeat(100_000);
    let pdf = doc.finish()?;
    assert_eq!(utf16_entry(&pdf, "/Title")?.chars().count(), MAX_INFO_CHARS);
    Ok(())
}

#[test]
fn cjk_text_is_not_replaced_by_question_marks() -> Result<(), Box<dyn Error>> {
    let mut doc = PdfDoc::new();
    let p = doc.add_page(612.0, 792.0);
    p.text(10.0, 20.0, 10.0, Font::Helvetica, [0, 0, 0], "签字清单");
    p.text(10.0, 40.0, 10.0, Font::Helvetica, [0, 0, 0], "No. 编号 2");
    let pdf = doc.finish()?;
    check_xref(&pdf)?;
    assert!(find(&pdf, b"(????) Tj").is_none(), "CJK text written as question marks");
    assert!(find(&pdf, b"<7B7E5B576E055355> Tj").is_some());
    assert!(find(&pdf, b"(No. ) Tj").is_some());
    assert!(find(&pdf, b"<7F1653F7> Tj").is_some());
    assert!(find(&pdf, b"( 2) Tj").is_some());
    assert!(find(&pdf, b"/Encoding /UniGB-UTF16-H").is_some());
    // Measured as full-width glyphs, matching the CID font's default width.
    assert_eq!(text_width(Font::Helvetica, 10.0, "签字"), 20.0);
    Ok(())
}

#[test]
fn arabic_is_shaped_and_preserved_or_falls_back_without_a_font() {
    let text = "مَرْحَبًا 123 (ABC)";
    let mut doc = PdfDoc::new();
    let page = doc.add_page(600.0, 300.0);
    page.text(30.0, 70.0, 20.0, Font::Helvetica, [0, 0, 0], text);
    let layout = arabic::layout(Font::Helvetica, text, TextDirection::Auto);
    let Ok(layout) = layout else {
        assert!(doc.missing_arabic_font());
        check_xref(&doc.finish().unwrap()).unwrap();
        return;
    };
    assert!(layout.width > 0.0);
    assert!((text_width(Font::Helvetica, 20.0, text) - layout.width * 0.02).abs() < 0.001);
    let pdf = doc.finish().unwrap();
    check_xref(&pdf).unwrap();
    assert_eq!(utf16_entry(&pdf, "/ActualText").unwrap(), text);
    assert!(find(&pdf, b"/Subtype /Type3").is_some());
    assert!(find(&pdf, b"/ToUnicode").is_some());
    assert!(find(&pdf, b"beginbfchar").is_some());
    assert!(find(&pdf, b"(?) Tj").is_none());
    assert!(find(&pdf, b"/UniGB-UTF16-H").is_none(), "Arabic is never set in the CJK font");
}

#[test]
fn latin_inside_arabic_text_declares_its_page_font() {
    if arabic::layout(Font::Helvetica, "مرحبا", TextDirection::Auto).is_err() && arabic::ARABIC_FONTS.is_empty() {
        return;
    }
    let mut doc = PdfDoc::new();
    doc.add_page(600.0, 300.0).text(30.0, 70.0, 20.0, Font::Helvetica, [0, 0, 0], "مرحبا\u{2028}عالم");
    let pdf = doc.finish().unwrap();
    check_xref(&pdf).unwrap();
    assert!(find(&pdf, b"/F1 20 Tf").is_some(), "the line break is drawn as a Helvetica space");
    assert!(find(&pdf, b"/Font << /F1 ").is_some(), "the page declares the font its Latin run uses");
}

#[test]
fn cjk_inside_arabic_text_uses_the_cid_font() {
    if arabic::layout(Font::Helvetica, "مرحبا", TextDirection::Auto).is_err() && arabic::ARABIC_FONTS.is_empty() {
        return;
    }
    let text = "مرحبا 编号";
    let mut doc = PdfDoc::new();
    doc.add_page(600.0, 300.0).text(30.0, 70.0, 20.0, Font::Helvetica, [0, 0, 0], text);
    let pdf = doc.finish().unwrap();
    check_xref(&pdf).unwrap();
    assert!(find(&pdf, b"<7F1653F7> Tj").is_some(), "the CJK run is set in the CID font");
    assert!(find(&pdf, b"/Encoding /UniGB-UTF16-H").is_some(), "the page declares the CID font");
    assert_eq!(utf16_entry(&pdf, "/ActualText").unwrap(), text);
    assert!(text_width(Font::Helvetica, 20.0, text) >= text_width(Font::Helvetica, 20.0, "مرحبا") + 40.0, "two full-width ideographs");
    for (text, cid_run) in [("مرحبا 😀", &b"<D83DDE00> Tj"[..]), ("مرحبا Привет", &b"<041F04400438043204350442> Tj"[..])] {
        let mut uncovered = PdfDoc::new();
        uncovered.add_page(600.0, 300.0).text(30.0, 70.0, 20.0, Font::Helvetica, [0, 0, 0], text);
        assert!(!uncovered.missing_arabic_font());
        let pdf = uncovered.finish().unwrap();
        check_xref(&pdf).unwrap();
        assert!(find(&pdf, cid_run).is_some(), "{text}: characters the Arabic face lacks fall back like non-Arabic text");
        assert_eq!(utf16_entry(&pdf, "/ActualText").unwrap(), text);
    }
}

#[test]
fn oversized_arabic_text_is_an_error_instead_of_partial_pdf() {
    let mut doc = PdfDoc::new();
    doc.add_page(600.0, 300.0).text(0.0, 50.0, 10.0, Font::Helvetica, [0, 0, 0], &"ب".repeat(65_537));
    if doc.missing_arabic_font() {
        check_xref(&doc.finish().unwrap()).unwrap();
        return;
    }
    assert!(doc.finish().unwrap_err().to_string().contains("128 KiB"));
}

#[test]
fn arabic_joining_marks_styles_and_rotation() {
    if arabic::layout(Font::Helvetica, "مرحبا", TextDirection::Auto).is_err() && arabic::ARABIC_FONTS.is_empty() {
        return;
    }
    for (text, expected) in [("مرحبا 123 (ABC)", "(ABC)"), ("مرحبا [ABC]", "[ABC]"), ("مرحبا <ABC>", "<ABC>")] {
        let mixed = arabic::layout(Font::Helvetica, text, TextDirection::Auto).unwrap();
        let latin: String = mixed
            .items
            .iter()
            .filter_map(|item| match item {
                arabic::Item::Latin { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        if !latin.is_empty() {
            assert_eq!(latin, expected);
        }
    }
    let mut metrics = PdfDoc::new();
    let (ascent, descent) = metrics.add_page(100.0, 100.0).text_vertical_extents(Font::Helvetica, 20.0, "كمثرى").unwrap();
    assert!(ascent > 0.0 && descent > 20.0 * 0.22, "Arabic descenders exceed the old Latin baseline allowance");
    assert!(
        metrics.page_mut(0).unwrap().text_vertical_extents(Font::Helvetica, 20.0, "مرحبا 123 (ABC)").is_some(),
        "spaces in variable fonts have empty outlines"
    );
    metrics.finish().unwrap();
    let joined = arabic::layout(Font::Helvetica, "سلام", TextDirection::Auto).unwrap();
    let separate_width: f32 = "سلام".chars().map(|c| text_width(Font::Helvetica, 1000.0, &c.to_string())).sum();
    assert!((joined.width - separate_width).abs() > 10.0, "joining changes advances");
    let marked = arabic::layout(Font::Helvetica, "مَرْحَبًا", TextDirection::Auto).unwrap();
    let plain = arabic::layout(Font::Helvetica, "مرحبا", TextDirection::Auto).unwrap();
    assert!(marked.items.len() > plain.items.len());
    assert!(marked.items.iter().any(|item| matches!(item, arabic::Item::Glyph { y, .. } if *y != 0.0)));
    let bold_available = match arabic::layout(Font::HelveticaBold, "مرحبا", TextDirection::Auto) {
        Ok(bold) if arabic::bold_face_is_synthetic() => {
            assert!(bold.width >= plain.width, "synthetic bold keeps the regular advances");
            true
        }
        Ok(bold) => {
            assert!(bold.width > plain.width, "bold metrics use the bold font instance");
            true
        }
        Err(e) => {
            assert!(arabic::ARABIC_FONTS.is_empty(), "a bundled face always serves bold text: {e:?}");
            false
        }
    };
    let mut regular = PdfDoc::new();
    regular.add_page(500.0, 300.0).text(30.0, 60.0, 20.0, Font::Helvetica, [0, 0, 0], "مرحبا");
    let regular = regular.finish().unwrap();
    let mut italic = PdfDoc::new();
    italic.add_page(500.0, 300.0).text(30.0, 60.0, 20.0, Font::HelveticaOblique, [0, 0, 0], "مرحبا");
    let italic = italic.finish().unwrap();
    assert_ne!(regular, italic, "italic shears the actual outlines");
    let rotated_font = if bold_available { Font::HelveticaBoldOblique } else { Font::HelveticaOblique };
    let mut rotated = PdfDoc::new();
    rotated.add_page(500.0, 300.0).text_rotated(Point::new(100.0, 100.0), 20.0, rotated_font, [0, 0, 0], "مرحبا 123", -std::f32::consts::FRAC_PI_2);
    let rotated = rotated.finish().unwrap();
    check_xref(&rotated).unwrap();
    assert_eq!(utf16_entry(&rotated, "/ActualText").unwrap(), "مرحبا 123");
    assert!(find(&rotated, b"0 1 -1 0 100 200 cm").is_some());
}

#[test]
fn line_breaks_in_arabic_text_draw_as_spaces() {
    if arabic::layout(Font::Helvetica, "مرحبا", TextDirection::Auto).is_err() && arabic::ARABIC_FONTS.is_empty() {
        return;
    }
    for (text, direction) in ["مرحبا\r\nعالم", "مرحبا\u{2028}عالم", "مرحبا\u{2029}ABC\r"]
        .into_iter()
        .flat_map(|text| [TextDirection::Auto, TextDirection::LeftToRight, TextDirection::RightToLeft].map(|direction| (text, direction)))
    {
        let layout = arabic::layout(Font::Helvetica, text, direction).unwrap();
        for item in &layout.items {
            if let arabic::Item::Latin { text: drawn, .. } = item {
                assert!(!drawn.chars().any(|ch| matches!(ch, '\n' | '\r' | '\u{2028}' | '\u{2029}')), "{drawn:?} from {text:?}");
            }
        }
    }
    let drawn = |text: &str| format!("{:?}", arabic::layout(Font::Helvetica, text, TextDirection::RightToLeft).unwrap().items);
    assert_eq!(drawn("مرحبا\u{2029}عالم"), drawn("مرحبا عالم"), "a paragraph separator keeps right-to-left reading order");
}

#[test]
fn arabic_text_respects_the_save_nesting_limit() -> Result<(), Box<dyn Error>> {
    if arabic::layout(Font::Helvetica, "مرحبا", TextDirection::Auto).is_err() && arabic::ARABIC_FONTS.is_empty() {
        return Ok(());
    }
    let mut d = PdfDoc::new();
    let p = d.add_page(100.0, 100.0);
    for _ in 0..28 {
        p.save();
    }
    p.text(10.0, 50.0, 12.0, Font::Helvetica, [0, 0, 0], "مرحبا");
    assert_eq!(interpret(&p.content).max_depth, 28);
    p.restore();
    p.text(10.0, 50.0, 12.0, Font::Helvetica, [0, 0, 0], "مرحبا");
    assert!(p.content.contains("/ActualText"));
    check_xref(&d.finish()?)?;
    Ok(())
}

#[test]
fn arabic_subsets_split_after_255_character_codes() -> Result<(), Box<dyn Error>> {
    if arabic::layout(Font::Helvetica, "مرحبا", TextDirection::Auto).is_err() && arabic::ARABIC_FONTS.is_empty() {
        return Ok(());
    }
    let mut doc = PdfDoc::new();
    let page = doc.add_page(600.0, 300.0);
    let marks = ['َ', 'ُ', 'ِ', 'ّ', 'ْ', 'ً', 'ٌ', 'ٍ'];
    for i in 0..260 {
        let text = format!("ب{}{}{}", marks[i % 8], marks[(i / 8) % 8], marks[(i / 64) % 8]);
        page.text(20.0, 50.0, 10.0, Font::Helvetica, [0, 0, 0], &text);
    }
    let pdf = doc.finish().unwrap();
    check_xref(&pdf).unwrap();
    assert!(find(&pdf, b"/A2 ").is_some());
    assert!(find(&pdf, b"/LastChar 255").is_some());
    assert!(find(&pdf, b"<100> Tj").is_none(), "Type3 codes are always one byte");
    Ok(())
}

#[test]
fn spreadsheet_context_is_first_character_not_first_strong() {
    use TextDirection::{Auto, Context, LeftToRight as Ltr, RightToLeft as Rtl};
    for (text, expected) in [("  مرحبا", Rtl), ("123 مرحبا", Ltr), ("(مرحبا)", Ltr), ("  ١٢٣ مرحبا", Ltr), ("\t\n", Ltr)] {
        assert_eq!(Context.resolve(text), expected, "{text}");
    }
    assert_eq!(Auto.resolve("123 مرحبا"), Rtl);
    assert_eq!(Auto.resolve("(مرحبا)"), Rtl);
}

#[test]
fn explicit_rtl_standard_text_reorders_without_an_arabic_font() {
    let source = "ABC !";
    let mut doc = PdfDoc::new();
    let page = doc.add_page(300.0, 100.0);
    page.text_with_direction(Point::new(10.0, 30.0), 12.0, Font::Helvetica, [0, 0, 0], source, TextDirection::RightToLeft);
    assert!(page.content.contains("(! ABC) Tj"), "{}", page.content);
    let mut objects = craft_pdf::classic::Writer::with_capacity(0).unwrap();
    let mut next = 4;
    assert!(page.arabic.write_objects(&mut next, &mut objects).unwrap().is_empty());
    assert_eq!(next, 4, "Latin-only RTL text must not allocate Arabic font objects");
    assert_eq!(text_width_with_direction(Font::Helvetica, 12.0, source, TextDirection::RightToLeft), text_width(Font::Helvetica, 12.0, source));
    let pdf = doc.finish().unwrap();
    assert_eq!(utf16_entry(&pdf, "/ActualText").unwrap(), source);
    assert!(find(&pdf, b"/Subtype /Type3").is_none());

    let mut huge = PdfDoc::new();
    huge.add_page(300.0, 100.0).text_with_direction(
        Point::new(10.0, 30.0),
        12.0,
        Font::Helvetica,
        [0, 0, 0],
        &"A".repeat(128 * 1024 + 1),
        TextDirection::RightToLeft,
    );
    assert!(huge.finish().unwrap_err().to_string().contains("128 KiB"));
}

#[test]
fn arabic_explicit_base_changes_visual_order_and_preserves_logical_text() {
    if arabic::layout(Font::Helvetica, "مرحبا", TextDirection::Auto).is_err() && arabic::ARABIC_FONTS.is_empty() {
        return;
    }
    let sources = |text: &str, direction| {
        arabic::layout(Font::Helvetica, text, direction)
            .unwrap()
            .items
            .into_iter()
            .map(|item| match item {
                arabic::Item::Glyph { source, .. } => source,
                arabic::Item::Latin { text, .. } | arabic::Item::Cid { text, .. } => text,
            })
            .collect::<String>()
    };
    let text = "مرحبا ABC";
    let ltr = sources(text, TextDirection::LeftToRight);
    let rtl = sources(text, TextDirection::RightToLeft);
    assert!(ltr.ends_with("ABC"), "{ltr}");
    assert!(rtl.starts_with("ABC"), "{rtl}");
    assert_ne!(ltr, rtl);
    assert_eq!(sources("123 مرحبا", TextDirection::Context), sources("123 مرحبا", TextDirection::LeftToRight));
    let mut documents = Vec::new();
    for direction in [TextDirection::LeftToRight, TextDirection::RightToLeft] {
        let mut doc = PdfDoc::new();
        doc.add_page(300.0, 100.0).text_rotated_with_direction(
            Baseline { origin: Point::new(30.0, 70.0), angle: 0.3 },
            20.0,
            Font::Helvetica,
            [0, 0, 0],
            text,
            direction,
        );
        let pdf = doc.finish().unwrap();
        assert_eq!(utf16_entry(&pdf, "/ActualText").unwrap(), text);
        documents.push(pdf);
    }
    assert_ne!(documents[0], documents[1]);
}

#[test]
fn many_alternating_runs_share_one_text_matrix() {
    let mut doc = PdfDoc::new();
    let p = doc.add_page(612.0, 792.0);
    p.text(10.0, 20.0, 10.0, Font::Helvetica, [0, 0, 0], &"a\u{4e2d}".repeat(2000));
    let pdf = doc.finish().unwrap();
    let count = |needle: &[u8]| pdf.windows(needle.len()).filter(|w| *w == needle).count();
    assert_eq!(count(b" Tm\n"), 1, "the position is set once, before the first run");
    assert_eq!(count(b" Tf\n"), 4000, "every run selects its font");
    assert_eq!(count(b" Tj\n"), 4000);
}

#[test]
fn symbol_font_draws_maths_in_its_own_encoding() -> Result<(), Box<dyn Error>> {
    let mut doc = PdfDoc::new();
    let p = doc.add_page(200.0, 200.0);
    p.text(10.0, 50.0, 12.0, Font::Symbol, [0, 0, 0], "\u{2211}\u{03B1}\u{2264}\u{221E}x");
    p.text(10.0, 80.0, 12.0, Font::TimesItalic, [0, 0, 0], "x");
    let pdf = doc.finish()?;
    let s = String::from_utf8_lossy(&pdf);
    assert!(s.contains("/BaseFont /Symbol >>"), "Symbol has no WinAnsi encoding");
    assert!(s.contains("/BaseFont /Times-Italic /Encoding /WinAnsiEncoding"));
    // The sum, alpha, less-or-equal and infinity codes; x is not in Symbol and becomes `?`.
    assert!(s.contains("(\\345a\\243\\245?) Tj"), "{s}");
    check_xref(&pdf)?;
    assert!(Font::Symbol.can_encode('\u{2211}') && !Font::Symbol.can_encode('x'));
    assert!(Font::TimesRoman.can_encode('x') && !Font::TimesRoman.can_encode('\u{2211}'));
    assert_eq!(text_width(Font::Symbol, 10.0, "\u{2211}"), 7.13);
    Ok(())
}

#[test]
fn symbol_text_is_never_shaped_as_arabic() -> Result<(), Box<dyn Error>> {
    let mut doc = PdfDoc::new();
    let page = doc.add_page(200.0, 200.0);
    assert_eq!(page.text_vertical_extents(Font::Symbol, 12.0, "\u{03B1}ب"), None);
    page.text(10.0, 50.0, 12.0, Font::Symbol, [0, 0, 0], "\u{03B1}ب");
    let pdf = doc.finish()?;
    check_xref(&pdf)?;
    assert!(find(&pdf, b"(a?) Tj").is_some());
    assert!(find(&pdf, b"/ActualText").is_none());
    assert_eq!(text_width(Font::Symbol, 10.0, "\u{03B1}ب"), f32::from(Font::Symbol.char_width('\u{03B1}') + Font::Symbol.char_width('ب')) / 100.0);
    Ok(())
}

#[test]
fn shared_run_length_preserves_pixels_at_packet_boundaries() {
    for len in [0, 1, 2, 3, 127, 128, 129, 255, 256, 257, 1024] {
        for seed in [0u8, 127, 128, 255] {
            let repeats = vec![seed; len];
            let literals: Vec<u8> = (0..len).map(|i| seed.wrapping_add(i as u8)).collect();
            let pairs: Vec<u8> = (0..len).map(|i| (i / 2) as u8).collect();
            for pixels in [repeats, literals, pairs] {
                let encoded = run_length(&pixels);
                assert_eq!(encoded.last(), Some(&128));
                assert_eq!(rle_decode(&encoded), pixels);
            }
        }
    }
    // Two equal bytes may now terminate a literal packet. PDF pixel values stay exact.
    assert_eq!(run_length(&[1, 2, 2, 3]), [0, 1, 255, 2, 0, 3, 128]);
}
