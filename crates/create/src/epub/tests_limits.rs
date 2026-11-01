//! Books built to exhaust the converter: huge list numbers, deep nesting, SVG pictures that name
//! local files, and tags cut off mid-character. Each must convert (or be refused) without
//! running out of memory or stack, and without reading anything outside the book.

use super::dom::{Block, Builder, Cascade, MAX_DEPTH};
use super::*;

fn blocks(html: &str) -> (Vec<Block>, Vec<String>) {
    let doc = package::parse(html).unwrap();
    let cascade = Cascade::new(Vec::new());
    let mut b = Builder::new(&cascade, "OEBPS/c.xhtml");
    b.node(doc.root_element());
    b.finish()
}

fn texts(blocks: &[Block]) -> Vec<String> {
    blocks
        .iter()
        .filter_map(|b| match b {
            Block::Text { runs, .. } => Some(runs.iter().map(|r| r.text.as_str()).collect()),
            _ => None,
        })
        .collect()
}

#[test]
fn huge_list_start_is_bounded() {
    // Roman numerals for a start this large would be about a terabyte of "m"s.
    let (b, _) = blocks(
        r#"<html xmlns="http://www.w3.org/1999/xhtml"><body>
           <ol start="999999999999999" style="list-style-type: upper-roman"><li>Big</li></ol>
           <ol start="18446744073709551615"><li>Max</li></ol>
           <ol start="3998" style="list-style-type: lower-roman"><li>a</li><li>b</li><li>c</li></ol></body></html>"#,
    );
    let t = texts(&b);
    assert_eq!(t[0], format!("{}.\u{a0}Big", super::dom::MAX_LIST_START));
    assert_eq!(t[1], format!("{}.\u{a0}Max", super::dom::MAX_LIST_START));
    // Roman up to 3999, decimal past it.
    assert_eq!(t[2..], ["mmmcmxcviii.\u{a0}a", "mmmcmxcix.\u{a0}b", "4000.\u{a0}c"]);
}

#[test]
fn deep_nesting_does_not_overflow() {
    // Past the converter's own cap (256) but inside what the parser accepts (512).
    let depth = 400;
    let html =
        format!(r#"<html xmlns="http://www.w3.org/1999/xhtml"><body>{}deep words{}</body></html>"#, "<div>".repeat(depth), "</div>".repeat(depth));
    let (b, notes) = blocks(&html);
    assert_eq!(texts(&b), ["deep words"]);
    assert!(notes.iter().any(|n| n.contains(&format!("{MAX_DEPTH} deep"))), "{notes:?}");
}

#[test]
fn deep_contents_list_does_not_overflow() {
    // Each list level is two elements (ol, li), so 200 stay inside the parser's 512.
    let depth = 200;
    let nav = format!(
        r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc">{}{}</nav></body></html>"#,
        r#"<ol><li><a href="c.xhtml">x</a>"#.repeat(depth),
        "</li></ol>".repeat(depth)
    );
    let ncx = format!(
        r#"<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><navMap>{}{}</navMap></ncx>"#,
        r#"<navPoint><navLabel><text>x</text></navLabel><content src="c.xhtml"/>"#.repeat(depth),
        "</navPoint>".repeat(depth)
    );
    let container: &[u8] = br#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
    // The navigation document is read first, the NCX only without one: a book each.
    for (nav_item, name, list) in [
        (r#"<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>"#, "OEBPS/nav.xhtml", &nav),
        ("", "OEBPS/toc.ncx", &ncx),
    ] {
        let opf = format!(
            r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata/>
            <manifest>{nav_item}<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
            <item id="c" href="c.xhtml" media-type="application/xhtml+xml"/></manifest><spine toc="ncx"><itemref idref="c"/></spine></package>"#
        );
        let bytes = zip::tests::zip(&[
            ("mimetype", b"application/epub+zip".as_slice(), false),
            ("META-INF/container.xml", container, true),
            ("OEBPS/content.opf", opf.as_bytes(), true),
            (name, list.as_bytes(), true),
        ]);
        let z = zip::Zip::open(&bytes).unwrap();
        let pkg = package::read(&z).unwrap();
        assert!(!pkg.toc.is_empty() && pkg.toc.len() <= 32, "{name}: {}", pkg.toc.len());
    }
}

#[test]
fn an_svg_past_its_limits_is_too_large() {
    assert!(!svg_too_large(b"<svg/>"));
    assert!(svg_too_large("<g/>".repeat(MAX_SVG_TAGS + 1).as_bytes()));
    assert!(svg_too_large(&vec![b' '; MAX_SVG_BYTES + 1]));
}

#[test]
fn svg_image_href_never_reads_disk() {
    // A real PNG on disk, named by absolute path from inside a book's SVG.
    let dir = std::env::temp_dir().join(format!("epub-svg-href-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("secret.png");
    let mut png_bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png_bytes, 2, 2);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().unwrap();
        wr.write_image_data(&[255u8; 12]).unwrap();
    }
    std::fs::write(&file, &png_bytes).unwrap();
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10"><image width="10" height="10" xlink:href="{}"/></svg>"#,
        file.display()
    );
    // The default options would load it: proof the file is reachable.
    let loud = resvg::usvg::Tree::from_data(svg.as_bytes(), &resvg::usvg::Options::default()).unwrap();
    assert!(loud.root().has_children());
    let tree = resvg::usvg::Tree::from_data(svg.as_bytes(), &svg_options()).unwrap();
    assert!(!tree.root().has_children(), "the SVG picture read a file from disk");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unclosed_tag_after_a_multibyte_char_keeps_the_words() {
    // Each ends inside a tag whose last character is more than one byte.
    for text in ["<body><p>Before</p><p class=\"caf\u{e9}", "<body>words <\u{e9}", "<body>end <"] {
        let t = texts(&loose_blocks(text, "OEBPS/c.xhtml"));
        assert!(t.iter().any(|p| p.contains("Before") || p.contains("words") || p.contains("end")), "{text}: {t:?}");
    }
}

#[test]
fn a_stored_entry_that_understates_its_size_is_refused() {
    // Without the check, a stored entry declaring 1 byte would be handed out whole, past the
    // per-entry and total caps that are checked against the declared size.
    let mut bytes = zip::tests::zip(&[("big.xhtml", &[b'x'; 4096], false)]);
    let dir = bytes.windows(4).rposition(|w| w == b"PK\x01\x02").unwrap();
    bytes[dir + 24..dir + 28].copy_from_slice(&1u32.to_le_bytes());
    let z = zip::Zip::open(&bytes).unwrap();
    assert!(matches!(z.read("big.xhtml"), Err(EpubError::Damaged(m)) if m.contains("larger than the container says")));
}

#[test]
fn xml_nested_past_the_parser_limit_is_refused_not_fatal() {
    // roxmltree recurses per level: 100,000 nested elements overflow the stack and abort.
    let depth = 100_000;
    let deep = format!(r#"<html xmlns="http://www.w3.org/1999/xhtml"><body>{}x{}</body></html>"#, "<div>".repeat(depth), "</div>".repeat(depth));
    assert!(package::parse(&deep).is_err());
    // An ordinary document, and one at the limit, still parse.
    let ok = format!("<a>{}x{}</a>", "<b>".repeat(package::MAX_XML_DEPTH - 1), "</b>".repeat(package::MAX_XML_DEPTH - 1));
    assert!(package::parse(&ok).is_ok());
    // Self-closing tags, comments, CDATA and quoted '>' do not count as nesting.
    let flat = format!("<a>{}</a>", r#"<br/><!-- <x><y> --><![CDATA[<z><z>]]><i t="<q><q>"/>"#.repeat(2000));
    assert!(!package::nests_deeper_than(&flat, 3));
}

/// `parse` on another thread: whether it refused `text`, or `None` if it took past the limit.
fn refused_in_time(text: String) -> Option<bool> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(package::parse(&text).is_err());
    });
    rx.recv_timeout(std::time::Duration::from_secs(5)).ok()
}

#[test]
fn billion_laughs_is_refused_quickly() {
    // Ten levels of ten references each: a thousand million copies of "lol".
    let mut dtd = String::from(r#"<!ENTITY lol0 "lol">"#);
    for level in 1..=9 {
        let refs = format!("&lol{};", level - 1).repeat(10);
        dtd.push_str(&format!(r#"<!ENTITY lol{level} "{refs}">"#));
    }
    assert_eq!(refused_in_time(format!("<!DOCTYPE a [{dtd}]><a>&lol9;</a>")), Some(true));
    // And in an attribute.
    assert_eq!(refused_in_time(format!(r#"<!DOCTYPE a [{dtd}]><a b="&lol9;"/>"#)), Some(true));
}

#[test]
fn many_references_to_a_large_entity_are_refused_quickly() {
    // No nesting, so no limit on references applies: each is a megabyte of text, all in one text
    // node that is copied whole at each reference.
    let value = "x".repeat(1 << 20);
    let text = format!(r#"<!DOCTYPE a [<!ENTITY big "{value}">]><a>{}</a>"#, "&big;".repeat(500));
    assert_eq!(refused_in_time(text), Some(true));
}

#[test]
fn a_book_s_own_small_entities_still_expand() {
    let text = r#"<!DOCTYPE a [<!ENTITY hi "hello"><!ENTITY both '&hi; there'>]><a>&both; &amp; &hi;</a>"#;
    assert_eq!(package::parse(text).unwrap().root_element().text(), Some("hello there & hello"));
}

const CONTAINER: &[u8] = br#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;

/// A book of one content file, `c`, listed `times` in the spine, and any other `files`.
fn book(media: &str, times: usize, c: &[u8], files: &[(&str, &[u8])]) -> Vec<u8> {
    let opf = format!(
        r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata/>
        <manifest><item id="c" href="c" media-type="{media}"/></manifest><spine>{}</spine></package>"#,
        r#"<itemref idref="c"/>"#.repeat(times)
    );
    let mut entries: Vec<(&str, &[u8], bool)> = vec![
        ("mimetype", b"application/epub+zip", false),
        ("META-INF/container.xml", CONTAINER, true),
        ("OEBPS/content.opf", opf.as_bytes(), true),
        ("OEBPS/c", c, false),
    ];
    entries.extend(files.iter().map(|(name, bytes)| (*name, *bytes, false)));
    zip::tests::zip(&entries)
}

const XHTML: &str = "application/xhtml+xml";

fn page(body: &str) -> Vec<u8> {
    format!(r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head><body>{body}</body></html>"#).into_bytes()
}

fn too_large(bytes: &[u8]) -> bool {
    matches!(from_epub(bytes, A4, "t"), Err(EpubError::TooLarge(_)))
}

#[test]
fn a_chapter_of_too_many_pages_is_refused() {
    let turn = r#"<div style="page-break-before: always">x</div>"#;
    assert!(too_large(&book(XHTML, 1, &page(&turn.repeat(MAX_PAGES + 1)), &[])));
    assert!(!too_large(&book(XHTML, 1, &page(&turn.repeat(3)), &[])), "an ordinary book converts");
}

#[test]
fn chapters_past_the_page_limit_are_refused() {
    assert!(too_large(&book(XHTML, MAX_PAGES + 1, &page("<p>x</p>"), &[])));
}

/// A PNG's signature and a header saying `w` × `h`, with no pixels at all.
fn png_header(w: u32, h: u32) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend_from_slice(&w.to_be_bytes());
    png.extend_from_slice(&h.to_be_bytes());
    png.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
    png
}

#[test]
fn a_picture_past_the_pixel_limit_is_left_out_with_a_note() {
    let (fits, over) = (png_header(10_000, 10_000), png_header(10_001, 10_000));
    let bytes = zip::tests::zip(&[("fits.png", fits.as_slice(), false), ("over.png", over.as_slice(), false)]);
    let z = zip::Zip::open(&bytes).unwrap();
    let mut pictures = Pictures { zip: &z, cache: HashMap::new(), notes: BTreeSet::new() };
    assert!(pictures.load("fits.png").is_some());
    assert!(pictures.notes.is_empty());
    assert!(pictures.load("over.png").is_none());
    assert!(pictures.notes.iter().any(|n| n.starts_with("over.png:")), "{:?}", pictures.notes);

    // The largest header there is, in a whole book: the book still converts.
    let huge = png_header(u32::MAX, u32::MAX);
    let bytes = book(XHTML, 1, &page(r#"<p>Words</p><img src="big.png" alt=""/>"#), &[("OEBPS/big.png", &huge)]);
    let converted = from_epub(&bytes, A4, "t").unwrap();
    assert!(converted.notes.iter().any(|n| n.contains("big.png") && n.contains("this large")), "{:?}", converted.notes);
}

#[test]
fn a_loop_of_entities_ends() {
    let text = r#"<!DOCTYPE a [<!ENTITY x "&y;"><!ENTITY y "&x;">]><a>&x;</a>"#;
    assert!(package::parse(text).is_err());
}
