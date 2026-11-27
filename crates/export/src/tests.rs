use super::*;
use craft_wordmodel::para::{Float, ShapeKind, Wrap};
use craft_wordmodel::{Align, ControlKind, ControlLock, Rgb};

/// A page of `n` one-word blocks, each in its own size and colour.
fn varied_page(n: usize) -> Page {
    let blocks = (0..n)
        .map(|i| Block {
            text: "word".into(),
            rect: [72.0, i as f64, 100.0, i as f64 + 1.0],
            size: 4.0 + i as f64 * 0.5,
            bold: false,
            italic: false,
            color: [(i % 256) as f64 / 255.0, (i / 256 % 256) as f64 / 255.0, 0.5],
            rotation: 0.0,
            space_around: [false; 2],
            font: String::new(),
        })
        .collect();
    Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), ..Page::default() }
}

fn lookup_steps(f: impl FnOnce()) -> usize {
    LOOKUP_STEPS.with(|c| c.set(0));
    f();
    LOOKUP_STEPS.with(|c| c.get())
}

#[test]
fn many_distinct_sizes_are_counted_in_constant_steps_each() {
    let pages = [varied_page(3000)];
    let steps = lookup_steps(|| {
        body_size(&pages);
    });
    assert!(steps <= 2 * 3000, "{steps} steps");
}

/// The step counter measures page-item and body-size lookups in the mapping; the last colour's
/// slot checks the output. craft-rtf separately tests colour-table lookup complexity.
#[test]
fn many_distinct_colours_are_exported_with_bounded_page_mapping() {
    let pages = [varied_page(3000)];
    let mut out = String::new();
    let steps = lookup_steps(|| out = rtf(&pages));
    assert!(steps <= 4 * 3000, "{steps} steps");
    assert!(out.contains("\\cf3000 ") || out.contains("\\cf3000\\"), "the last colour has the last slot");
}

fn page() -> Page {
    let b = |t: &str, y: f64, size: f64, bold: bool| Block {
        text: t.into(),
        rect: [72.0, y, 500.0, y + size],
        size,
        bold,
        italic: false,
        color: [0.0; 3],
        rotation: 0.0,
        space_around: [false; 2],
        font: String::new(),
    };
    Page {
        width: 612.0,
        height: 792.0,
        blocks: vec![
            b("Body text after the picture, which is long enough to be the main text size of the page.", 400.0, 11.0, false),
            b("Annual Report", 700.0, 24.0, true),
            b("Overview", 650.0, 14.0, true),
            b("Some <body> text & more of it so that eleven points is the body size here.", 620.0, 11.0, false),
        ],
        images: vec![Image {
            ext: "png",
            bytes: b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x02\0\0\0\x03".as_slice().into(),
            rect: [72.0, 450.0, 272.0, 600.0],
            size: [200.0, 150.0],
            rotation: 0.0,
            flip: false,
        }],
        background: None,
        ..Page::default()
    }
}

#[test]
fn html_has_headings_paragraphs_and_images_in_order() {
    let h = html(&[page(), page()], "Report");
    let order: Vec<usize> = ["<h1>Annual Report</h1>", "<h2>Overview</h2>", "<p>Some &lt;body&gt; text &amp;", "<img", "<p>Body text after"]
        .iter()
        .map(|x| h.find(x).unwrap_or_else(|| panic!("{x} missing in {h}")))
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
    assert_eq!(h.matches("<hr>").count(), 1);
    assert_eq!(base64(b"Man"), "TWFu");
    assert_eq!(base64(b"Ma"), "TWE=");
}

#[test]
fn simple_html_has_no_css_or_layout_elements() {
    let mut p = page();
    let mut unicode = block("naïve 日本語.", 72.0, 300.0, 300.0, 311.0);
    unicode.bold = true;
    unicode.italic = true;
    p.blocks.push(unicode);
    let source = html_simple(&[p, table_page()], "Simple export");
    for banned in ["<style", "style=", "<div", "<span"] {
        assert!(!source.contains(banned), "{banned} in {source}");
    }
    for wanted in ["<h1", "<p>", "<strong>", "<em>", "<table", "naïve 日本語."] {
        assert!(source.contains(wanted), "no {wanted} in {source}");
    }
    // The rich export of the same pages keeps its stylesheet: only simple mode drops it.
    assert!(html(&[page()], "Rich").contains("<style"));
}

#[test]
fn docx_is_a_word_package() {
    let d = docx(&[page()], "Report");
    assert!(d.starts_with(b"PK"));
    let names: Vec<&str> =
        ["[Content_Types].xml", "word/document.xml", "word/styles.xml", "word/_rels/document.xml.rels", "word/media/image1.png", "_rels/.rels"]
            .to_vec();
    for n in names {
        assert!(d.windows(n.len()).any(|w| w == n.as_bytes()), "{n}");
    }
}

#[test]
fn word_does_not_invent_borders_for_aligned_invoice_fields() {
    let tables = docx_tables(&docx(&[table_page()], "Invoice"));
    assert_eq!(tables.len(), 1);
    let single = |b: &Option<craft_wordmodel::props::Border>| b.is_some_and(|b| b.style == craft_wordmodel::props::BorderStyle::Single);
    let cells = tables[0].rows.iter().flat_map(|r| &r.cells).filter_map(|c| c.props.borders);
    let table = tables[0].props.borders.into_iter();
    for b in table.chain(cells) {
        assert!(![b.top, b.left, b.bottom, b.right, b.between, b.inside_v].iter().any(single), "text alignment is not evidence of a border: {b:?}");
    }
}

#[test]
fn word_table_cell_widths_follow_the_columns_they_span() {
    let cell = |text: &str, span| Cell { text: text.into(), size: 11.0, bold: false, italic: false, color: [0.0; 3], span, font: String::new() };
    let table = Table {
        rect: [0.0, 0.0, 300.0, 100.0],
        cols: vec![0.0, 50.0, 150.0],
        rows: vec![
            vec![cell("First", 1), cell("Second", 1), cell("Third", 1)],
            vec![cell("First", 1), cell("Last two", 2)],
            vec![cell("First two", 2), cell("Third", 1)],
            vec![cell("All", 3)],
            vec![cell("First", 1)],
        ],
        ruled: None,
    };
    let t = crate::as_word::table(&table, true);
    assert_eq!(t.grid, [50.0, 100.0, 150.0]);
    let widths: Vec<Vec<f32>> = t.rows.iter().map(|r| r.cells.iter().map(|c| c.props.width.unwrap_or(0.0)).collect()).collect();
    // A short row is padded to the grid with cells as wide as the columns they fill.
    assert_eq!(widths, vec![vec![50.0, 100.0, 150.0], vec![50.0, 250.0], vec![150.0, 150.0], vec![300.0], vec![50.0, 100.0, 150.0]]);
}

/// The tables a Word document holds, read back through craft-docx.
fn docx_tables(d: &[u8]) -> Vec<craft_wordmodel::Table> {
    craft_docx::read(d)
        .unwrap()
        .body
        .iter()
        .filter_map(|b| if let craft_wordmodel::Block::Table(t) = &**b { Some(t.clone()) } else { None })
        .collect()
}

/// The paragraphs of a document's body (not inside tables).
fn body_paragraphs(doc: &craft_wordmodel::Document) -> Vec<&craft_wordmodel::Paragraph> {
    doc.body.iter().filter_map(|b| if let craft_wordmodel::Block::Para(p) = &**b { Some(p) } else { None }).collect()
}

/// The objects in the body's paragraphs, in order.
fn body_objects(doc: &craft_wordmodel::Document) -> Vec<&craft_wordmodel::InlineObject> {
    body_paragraphs(doc).into_iter().flat_map(|p| &p.objects).collect()
}

/// A shape's text, from the story it holds.
fn shape_text(doc: &craft_wordmodel::Document, o: &craft_wordmodel::InlineObject) -> String {
    let craft_wordmodel::InlineObject::Shape { story: Some(id), .. } = o else { return String::new() };
    doc.parts
        .get(id)
        .map(|part| part.blocks.iter().filter_map(|b| if let craft_wordmodel::Block::Para(p) = &**b { Some(p.plain_text()) } else { None }).collect())
        .unwrap_or_default()
}

/// The Word document a package reads back as.
fn word(d: &[u8]) -> craft_wordmodel::Document {
    craft_docx::read(d).unwrap()
}

/// A drawn shape (not a text box) floating on a page: its kind, its box from the page's top left
/// ([x, y, width, height]), where it stacks, and its paint.
#[derive(Debug)]
struct Drawn {
    kind: ShapeKind,
    rect: [f32; 4],
    z: u32,
    behind: bool,
    fill: Option<Rgb>,
    stroke: Option<Rgb>,
    width: f32,
    dashed: bool,
    /// A freeform's paths in its own frame, y down, and whether each is closed.
    paths: Vec<(Vec<[f32; 2]>, bool)>,
}

/// The shapes drawn among `objects`, in the order they are written.
fn drawn_of(objects: &[&craft_wordmodel::InlineObject]) -> Vec<Drawn> {
    objects
        .iter()
        .filter_map(|o| {
            let craft_wordmodel::InlineObject::Shape { kind, w, h, fill, stroke, stroke_width, float, story: None, freeform, extra, .. } = o else {
                return None;
            };
            Some(Drawn {
                kind: *kind,
                rect: [float.x, float.y, *w, *h],
                z: float.z,
                behind: float.wrap == Wrap::BehindText,
                fill: fill.as_ref().map(|f| match f {
                    craft_wordmodel::fill::Fill::Solid { color, .. } => *color,
                    other => panic!("not a solid fill: {other:?}"),
                }),
                stroke: *stroke,
                width: *stroke_width,
                dashed: !extra.dash.is_solid(),
                paths: freeform.as_ref().map(|f| f.paths.iter().map(|p| (p.pts.clone(), p.closed)).collect()).unwrap_or_default(),
            })
        })
        .collect()
}

/// The shapes drawn in the document's body.
fn drawn(doc: &craft_wordmodel::Document) -> Vec<Drawn> {
    drawn_of(&body_objects(doc))
}

/// The body's text boxes: their text, box from the page's top left ([x, y, width, height]) and
/// where they stack.
fn text_boxes(doc: &craft_wordmodel::Document) -> Vec<(String, [f32; 4], u32)> {
    body_objects(doc)
        .into_iter()
        .filter_map(|o| match o {
            craft_wordmodel::InlineObject::Shape { story: Some(_), w, h, float, .. } => {
                Some((shape_text(doc, o), [float.x, float.y, *w, *h], float.z))
            }
            _ => None,
        })
        .collect()
}

/// The pictures placed in the document's body, with their placement.
fn pictures(doc: &craft_wordmodel::Document) -> Vec<&Float> {
    body_objects(doc)
        .into_iter()
        .filter_map(|o| if let craft_wordmodel::InlineObject::Image { float, .. } = o { Some(float) } else { None })
        .collect()
}

/// The document's content controls, each with the paragraph it stands in: those in the body's
/// paragraphs, then those in its text boxes.
fn controls(doc: &craft_wordmodel::Document) -> Vec<(craft_wordmodel::ContentControl, craft_wordmodel::Paragraph)> {
    let boxed = doc
        .parts
        .values()
        .flat_map(|part| part.blocks.iter())
        .filter_map(|b| if let craft_wordmodel::Block::Para(p) = &**b { Some(p) } else { None });
    body_paragraphs(doc)
        .into_iter()
        .chain(boxed)
        .flat_map(|p| {
            p.objects.iter().filter_map(move |o| match o {
                craft_wordmodel::InlineObject::ControlStart { control } => Some(((**control).clone(), p.clone())),
                _ => None,
            })
        })
        .collect()
}

/// The formatting of a paragraph's first run of text.
fn run_of(p: &craft_wordmodel::Paragraph) -> craft_wordmodel::CharProps {
    p.run_ranges()
        .find(|(range, _)| p.text.get(range.clone()).is_some_and(|t| t.chars().any(|c| c != craft_wordmodel::para::OBJ)))
        .map(|(_, props)| props.clone())
        .expect("a run of text")
}

/// A paragraph's text without its object marks.
fn text_of(p: &craft_wordmodel::Paragraph) -> String {
    p.plain_text().chars().filter(|c| *c != craft_wordmodel::para::OBJ).collect()
}

/// Whether two boxes agree to a hundredth of a point.
fn close(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.01)
}

/// Every part of a package written by [`Zip`], inflated, for searching what it holds.
fn package_text(zip: &[u8]) -> String {
    let at = |i: usize, n: usize| zip[i..i + n].iter().rev().fold(0usize, |v, b| v << 8 | usize::from(*b));
    let (mut i, mut out) = (0, String::new());
    while zip[i..].starts_with(b"PK\x03\x04") {
        let (size, name_len) = (at(i + 18, 4), at(i + 26, 2));
        let data = i + 30 + name_len;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::DeflateDecoder::new(&zip[data..data + size]), &mut bytes).unwrap();
        out.push_str(&String::from_utf8_lossy(&bytes));
        i = data + size;
    }
    out
}

/// A floating object's placement.
fn float_of(o: &craft_wordmodel::InlineObject) -> Option<&craft_wordmodel::para::Float> {
    match o {
        craft_wordmodel::InlineObject::Shape { float, .. } | craft_wordmodel::InlineObject::Image { float, .. } => Some(float),
        _ => None,
    }
}

#[test]
fn word_keeps_a_page_with_a_clear_image_and_big_title_in_flow() {
    // page() has a picture between paragraphs and a 24 pt title over 11 pt body text, but
    // nothing overlaps: it must stay flowing paragraphs with the image inline.
    let doc = craft_docx::read(&docx(&[page()], "Report")).unwrap();
    let objects = body_objects(&doc);
    assert!(objects.iter().all(|o| !matches!(o, craft_wordmodel::InlineObject::Shape { .. })), "nothing overlaps, so nothing floats: {objects:?}");
    assert!(
        objects.iter().any(|o| matches!(o, craft_wordmodel::InlineObject::Image { float, .. } if float.wrap == craft_wordmodel::para::Wrap::Inline)),
        "the picture stays inline: {objects:?}"
    );
    let paras = body_paragraphs(&doc);
    let title = paras.iter().position(|p| p.plain_text() == "Annual Report");
    let body = paras.iter().position(|p| p.plain_text().starts_with("Body text after the picture"));
    assert_eq!(title.and_then(|i| paras[i].props.style.as_deref()), Some("Heading1"), "the title stays a heading");
    assert!(title.is_some() && body.is_some() && title < body, "reading order kept");
    // The same page with the picture moved over the body text floats.
    let mut over = page();
    over.images[0].rect = [72.0, 380.0, 272.0, 530.0];
    let doc = craft_docx::read(&docx(&[over], "Report")).unwrap();
    assert!(body_objects(&doc).iter().any(|o| matches!(o, craft_wordmodel::InlineObject::Shape { .. })), "an image over text floats the page");
}

#[test]
fn word_overlap_does_not_consume_invoice_body_space() {
    let mut p = table_page();
    p.blocks.push(Block {
        text: "DRAFT".into(),
        rect: [100.0, 200.0, 500.0, 700.0],
        size: 100.0,
        bold: false,
        italic: false,
        color: [0.8; 3],
        rotation: 0.0,
        space_around: [false; 2],
        font: String::new(),
    });
    p.images = page().images;
    let d = docx(&[p], "Invoice");
    let doc = craft_docx::read(&d).unwrap();
    use craft_wordmodel::para::{Anchor, Wrap};
    assert!(doc.body.iter().all(|b| !matches!(&**b, craft_wordmodel::Block::Table(_))), "the source text boxes keep their positions");
    let objects = body_objects(&doc);
    let shape = |text: &str| objects.iter().copied().find(|o| shape_text(&doc, o) == text);
    let draft = shape("DRAFT").and_then(float_of).expect("the watermark is a text box");
    assert_eq!(objects.iter().filter(|o| shape_text(&doc, o) == "DRAFT").count(), 1);
    let meter = shape("Meter").and_then(float_of).expect("Meter is a text box");
    assert_eq!((meter.h_rel, meter.v_rel), (Anchor::Page, Anchor::Page));
    assert_eq!((meter.x, meter.y), (72.0, 160.0), "the Meter field keeps its source position");
    assert_eq!(meter.wrap, Wrap::InFrontOfText);
    assert!(!body_paragraphs(&doc).iter().any(|p| p.text.contains(craft_wordmodel::para::PAGE_BREAK)), "no page breaks");
    // The watermark is behind everything, and the picture, behind the text too, is drawn over it.
    let picture = objects.iter().copied().find(|o| matches!(o, craft_wordmodel::InlineObject::Image { .. })).and_then(float_of).expect("the picture");
    assert_eq!((draft.wrap, picture.wrap), (Wrap::BehindText, Wrap::BehindText), "images must not displace invoice text");
    assert!(draft.z < picture.z && picture.z < meter.z, "{} < {} < {}", draft.z, picture.z, meter.z);
}

#[test]
fn word_rotated_pages_keep_separate_page_sizes_and_unique_shapes() {
    let mut p = table_page();
    p.blocks[0].rotation = 45.0;
    let mut second = p.clone();
    second.width = 400.0;
    second.height = 500.0;
    let d = docx(&[p, second], "Two invoices");
    let doc = craft_docx::read(&d).unwrap();
    let meters: Vec<_> = body_objects(&doc).into_iter().filter(|o| shape_text(&doc, o) == "Meter").collect();
    assert_eq!(meters.len(), 2);
    assert!(meters.iter().all(|o| float_of(o).is_some_and(|f| (f.rot - 315.0).abs() < 0.01)), "PDF y-up becomes Word y-down");
    let sizes: Vec<(f32, f32)> = doc.sections().iter().map(|(_, s)| (s.page_w, s.page_h)).collect();
    assert_eq!(sizes, [(612.0, 792.0), (400.0, 500.0)]);
    // Every drawing has an id of its own.
    let xml = part(&d, "word/document.xml");
    let ids: Vec<&str> = xml.split("<wp:docPr id=\"").skip(1).filter_map(|s| s.split('"').next()).collect();
    let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
    assert!(!ids.is_empty() && unique.len() == ids.len(), "{ids:?}");
}

/// #2992: pages that share a background colour get it as the Word document's page colour
/// (with the setting that makes Word show it); differing or white backgrounds don't.
#[test]
fn docx_keeps_a_shared_page_background() {
    let teal = Page { background: Some([0.0, 0.5, 0.5]), ..page() };
    let zip = docx(&[teal.clone(), teal.clone()], "Teal");
    assert_eq!(craft_docx::read(&zip).unwrap().settings.page_color, Some(craft_wordmodel::Rgb(0, 128, 128)));
    // Word shows the colour only when the settings say so.
    assert!(part(&zip, "word/document.xml").contains("<w:background w:color=\"008080\""));
    assert!(part(&zip, "word/settings.xml").contains("<w:displayBackgroundShape/>"));
    let other = Page { background: Some([1.0, 0.0, 0.0]), ..page() };
    for (pages, why) in [
        (vec![teal.clone(), other], "different colours"),
        (vec![teal.clone(), page()], "a page without one"),
        (vec![Page { background: Some([1.0; 3]), ..page() }], "white"),
        (vec![page()], "none"),
    ] {
        let zip = docx(&pages, "Plain");
        assert_eq!(craft_docx::read(&zip).unwrap().settings.page_color, None, "{why}");
        assert!(!part(&zip, "word/document.xml").contains("<w:background"), "{why}");
    }
}

/// One part of a package written by [`Zip`], inflated.
pub(crate) fn part(zip: &[u8], name: &str) -> String {
    let at = |i: usize, n: usize| zip[i..i + n].iter().rev().fold(0usize, |v, b| v << 8 | *b as usize);
    let mut i = 0;
    while zip[i..].starts_with(b"PK\x03\x04") {
        let (size, name_len) = (at(i + 18, 4), at(i + 26, 2));
        let data = i + 30 + name_len;
        if &zip[i + 30..data] == name.as_bytes() {
            let mut s = String::new();
            std::io::Read::read_to_string(&mut flate2::read::DeflateDecoder::new(&zip[data..data + size]), &mut s).unwrap();
            return s;
        }
        i = data + size;
    }
    panic!("{name} missing")
}

/// The PDF's language (`/Lang`) is the default language of the Word document's text, in the
/// attribute Word keeps for its script, and the HTML page's. No language, or one that is not a
/// well formed tag, leaves both files as they were.
#[test]
fn word_and_html_take_the_documents_language() {
    let pages = [page()];
    // The document default languages Word reads back: (Latin, East Asian, right-to-left).
    let langs = |lang| {
        let c = craft_docx::read(&docx_with_language(&pages, "Report", lang)).unwrap().styles.default_chr.clone();
        (c.lang, c.lang_ea, c.lang_bidi)
    };
    let latin = |t: &str| (Some(t.to_string()), None, None);
    let east_asian = |t: &str| (None, Some(t.to_string()), None);
    let bidi = |t: &str| (None, None, Some(t.to_string()));
    assert_eq!(langs(Some("bg-BG")), latin("bg-BG"));
    assert_eq!(langs(Some(" en-GB ")), latin("en-GB"), "the tag is trimmed");
    assert_eq!(langs(Some("zh-Hant-TW")), east_asian("zh-Hant-TW"), "East Asian text");
    assert_eq!(langs(Some("ja")), east_asian("ja"), "East Asian text");
    assert_eq!(langs(Some("ar-EG")), bidi("ar-EG"), "right-to-left text");
    assert_eq!(langs(Some("HE")), bidi("HE"), "right-to-left text");
    // A script subtag decides over the language; private use names no script.
    assert_eq!(langs(Some("pa-Arab-PK")), bidi("pa-Arab-PK"), "Punjabi in Arabic script");
    assert_eq!(langs(Some("sr-Latn")), latin("sr-Latn"), "Serbian in Latin script");
    assert_eq!(langs(Some("zh-Latn")), latin("zh-Latn"), "Chinese in Latin script");
    assert_eq!(langs(Some("en-x-arab")), latin("en-x-arab"), "private use");
    assert_eq!(langs(Some("az-Latn-x-arab")), latin("az-Latn-x-arab"), "private use");
    assert!(html_with_language(&pages, "Report", Some("bg-BG")).starts_with("<!DOCTYPE html>\n<html lang=\"bg-BG\">\n"));
    // A tag longer than 35 characters is still a tag.
    let long = "en-US-u-ca-gregory-nu-latn-co-phonebk";
    assert_eq!(langs(Some(long)), latin(long), "{long}");
    assert!(html_with_language(&pages, "Report", Some(long)).contains(&format!("<html lang=\"{long}\">")), "{long}");
    let too_long = format!("en{}", "-abcdefgh".repeat(29));
    // Private use takes one-character subtags.
    assert!(html_with_language(&pages, "Report", Some("en-x-a-b")).contains("<html lang=\"en-x-a-b\">"));
    for lang in [
        None,
        Some(""),
        Some("  "),
        Some("bg_BG"),
        Some("b"),
        Some("bulg-BG"),
        Some("bg--BG"),
        Some("bg-"),
        Some("en-x"),
        Some("en-a"),
        Some("en-a-x-private"),
        Some("en-US-u-ca-x"),
        Some("en-US-abcdefghi"),
        Some(too_long.as_str()),
        Some("en-\"x\""),
        Some("1a-BG"),
        Some("\u{431}\u{433}-BG"),
    ] {
        assert_eq!(langs(lang), (None, None, None), "{lang:?}");
        assert_eq!(html_with_language(&pages, "Report", lang), html(&pages, "Report"), "{lang:?}");
        assert_eq!(html_simple_with_language(&pages, "Report", lang), html_simple(&pages, "Report"), "{lang:?}");
    }
    // The simple export leaves out styling, not the language.
    assert!(html_simple_with_language(&pages, "Report", Some("bg-BG")).starts_with("<!DOCTYPE html>\n<html lang=\"bg-BG\">\n"));
}

#[test]
fn word_opens_text_with_control_characters_and_long_pages() {
    // #72: text extracted without a usable /ToUnicode holds C0 controls, which XML forbids;
    // Word then refused the whole file. A till receipt is also taller than Word's 22 in.
    let mut p = page();
    p.height = 2400.0;
    p.blocks.push(Block {
        text: "Total\u{0}\u{3}\u{c} 4.50\u{FFFF}\tGBP".into(),
        rect: [72.0, 100.0, 300.0, 110.0],
        size: f64::NAN,
        bold: false,
        italic: false,
        color: [0.0; 3],
        rotation: 0.0,
        space_around: [false; 2],
        font: String::new(),
    });
    let d = docx(&[p.clone()], "Receipt\u{1}");
    let xml = part(&d, "word/document.xml");
    assert!(!xml.chars().any(|c| !allowed(c)));
    let doc = craft_docx::read(&d).unwrap();
    let total = body_paragraphs(&doc).into_iter().find(|p| p.plain_text().starts_with("Total")).expect("the receipt line");
    assert_eq!(total.plain_text(), "Total 4.50\tGBP");
    assert_eq!(total.runs.first().and_then(|r| r.props.size), Some(12.0), "a non-finite size falls back to 12 pt");
    assert_eq!(doc.last_section.page_h, 1584.0, "clamped to 22 in");
    assert!(rtf(&[p]).contains("Total 4.50\\tab GBP"));
    // A narrow page keeps room for text: margins shrink.
    let narrow = Page { width: 100.0, height: 200.0, ..page() };
    let s = craft_docx::read(&docx(&[narrow], "Narrow")).unwrap().last_section;
    assert_eq!([s.margin_top, s.margin_right, s.margin_bottom, s.margin_left], [25.0, 12.5, 25.0, 12.5]);
}

#[test]
fn text_colour_survives_every_format() {
    // #526: a dark green heading (0.05 0.23 0.18 rg) came out black in Word.
    let mut p = page();
    p.blocks[1].color = [0.05, 0.23, 0.18];
    p.blocks[3].color = [f64::NAN, 2.0, -1.0];
    let xml = part(&docx(&[p.clone()], "Colour"), "word/document.xml");
    assert!(xml.contains("<w:color w:val=\"0D3B2E\"/>"), "{xml}");
    assert!(xml.contains("<w:color w:val=\"00FF00\"/>"), "out-of-range values clamp: {xml}");
    // Black text keeps Word's default colour.
    assert_eq!(xml.matches("<w:color ").count(), 2, "{xml}");
    let h = html(&[p.clone()], "Colour");
    assert!(h.contains("<h1 style=\"color:#0D3B2E\">Annual Report</h1>"), "{h}");
    assert!(h.contains("<p style=\"color:#00FF00\">"), "{h}");
    let r = rtf(&[p.clone()]);
    assert!(r.contains("{\\colortbl;\\red13\\green59\\blue46;\\red0\\green255\\blue0;}"), "{r}");
    assert!(r.contains("\\cf1 Annual Report"), "{r}");
    let ppt = pptx(&[p], "Colour");
    let pxml = part(&ppt, "ppt/slides/slide1.xml");
    assert!(pxml.contains("<a:srgbClr val=\"0D3B2E\"/>"), "{pxml}");
    assert!(pxml.contains("<a:srgbClr val=\"00FF00\"/>"), "out-of-range values clamp: {pxml}");
    // A table cell keeps its colour too.
    let mut t = table_page();
    t.blocks[0].color = [1.0, 0.0, 0.0];
    let xml = part(&docx(&[t.clone()], "Grid"), "word/document.xml");
    assert!(xml.contains("<w:color w:val=\"FF0000\"/>"), "{xml}");
    assert!(html(&[t], "Grid").contains("style=\"color:#FF0000\""));
}

/// A run read back from RTF: its text, bold, italic and size in half points.
type RtfRun = (String, bool, bool, Option<u32>);

/// The paragraphs RTF holds, read back, as their runs.
fn rtf_paras(r: &str) -> Vec<Vec<RtfRun>> {
    let doc = craft_rtf::read(r.as_bytes()).unwrap();
    doc.blocks
        .iter()
        .filter_map(|b| if let craft_rtf::Block::Para(p) = b { Some(p) } else { None })
        .map(|p| p.runs.iter().map(|r| (r.text.clone(), r.fmt.bold, r.fmt.italic, r.fmt.half_points)).collect())
        .collect()
}

#[test]
fn rtf_escapes_and_sizes() {
    let mut p = page();
    p.blocks.push(Block {
        text: "Café {x}".into(),
        rect: [72.0, 100.0, 200.0, 110.0],
        size: 10.0,
        bold: false,
        italic: true,
        color: [0.0; 3],
        rotation: 0.0,
        space_around: [false; 2],
        font: String::new(),
    });
    let r = rtf(&[p]);
    assert!(r.starts_with("{\\rtf1") && r.trim_end().ends_with('}'));
    assert!(r.contains("Caf\\u233? \\{x\\}"), "escaped: {r}");
    let runs: Vec<_> = rtf_paras(&r).into_iter().flatten().collect();
    assert!(runs.contains(&("Annual Report".into(), true, false, Some(48))), "{runs:?}");
    assert!(runs.contains(&("Café {x}".into(), false, true, Some(20))), "{runs:?}");
}

/// The RTF read back into the word model: each section's page size (points) and the text of its
/// blocks, tables as `<table:first cell>`.
fn rtf_sections(r: &str) -> Vec<([f32; 2], Vec<String>)> {
    let doc = craft_rtf::doc::import(r.as_bytes()).unwrap();
    let mut out: Vec<([f32; 2], Vec<String>)> = Vec::new();
    let mut texts = Vec::new();
    for b in &doc.body {
        match &**b {
            craft_wordmodel::Block::Para(p) => {
                if !p.plain_text().is_empty() || !p.objects.is_empty() {
                    texts.push(if p.objects.is_empty() { p.plain_text() } else { "<picture>".into() });
                }
                if let Some(s) = &p.section {
                    out.push(([s.page_w, s.page_h], std::mem::take(&mut texts)));
                }
            }
            craft_wordmodel::Block::Table(t) => {
                let first = t
                    .rows
                    .first()
                    .and_then(|r| r.cells.first())
                    .map(|c| {
                        c.blocks
                            .iter()
                            .filter_map(|b| if let craft_wordmodel::Block::Para(p) = &**b { Some(p.plain_text()) } else { None })
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                texts.push(format!("<table:{}>", first.trim()));
            }
        }
    }
    out.push(([doc.last_section.page_w, doc.last_section.page_h], texts));
    out
}

/// Each page is a section of its own, so it starts a new page, and a table on a page is in that
/// page's section rather than after a break character in its first cell.
#[test]
fn rtf_pages_are_sections_and_tables_stay_on_theirs() {
    let r = rtf(&[page(), table_page(), page()]);
    let sections = rtf_sections(&r);
    assert_eq!(sections.len(), 3, "{sections:?}");
    assert!(sections[1].1.first().is_some_and(|t| t == "<table:Meter>"), "{sections:?}");
    assert!(!r.contains("\\page"), "sections break the pages: {r}");
}

/// Empty and image-only pages still occupy a page; each keeps its own size.
#[test]
fn rtf_keeps_pages_without_text() {
    let mut blank = page();
    blank.blocks.clear();
    blank.images.clear();
    blank.rules.clear();
    let mut image_only = page();
    image_only.blocks.clear();
    let mut small = blank.clone();
    small.width = 400.0;
    small.height = 500.0;
    for pages in [
        vec![blank.clone(), blank.clone(), page()],
        vec![page(), blank.clone(), blank.clone()],
        vec![page(), blank.clone(), blank.clone(), table_page()],
        vec![blank.clone(), blank.clone(), blank.clone()],
        vec![page(), image_only.clone(), image_only.clone(), page()],
        vec![page(), image_only],
        vec![page(), small, blank],
    ] {
        let r = rtf(&pages);
        let sizes: Vec<[f32; 2]> = rtf_sections(&r).iter().map(|s| s.0).collect();
        let want: Vec<[f32; 2]> = pages.iter().map(|p| [p.width as f32, p.height as f32]).collect();
        assert_eq!(sizes, want, "{r}");
    }
}

/// A 3-column table with a spanning header row and a hole in the last row.
fn table_page() -> Page {
    let cell = |t: &str, x: f64, y: f64, w: f64| Block {
        text: t.into(),
        rect: [x, y, x + w, y + 12.0],
        size: 11.0,
        bold: false,
        italic: false,
        color: [0.0; 3],
        rotation: 0.0,
        space_around: [false; 2],
        font: String::new(),
    };
    Page {
        width: 612.0,
        height: 792.0,
        blocks: vec![
            cell("Meter", 72.0, 620.0, 120.0),
            cell("Unit", 232.0, 620.0, 80.0),
            cell("Reading", 392.0, 620.0, 100.0),
            cell("A1", 72.0, 590.0, 120.0),
            cell("kWh", 232.0, 590.0, 80.0),
            cell("37414.00", 392.0, 590.0, 100.0),
            cell("A2", 72.0, 560.0, 120.0),
            cell("kW", 232.0, 560.0, 80.0),
            // A2's row is missing its third cell (an empty cell in the grid).
            cell("Totals", 72.0, 530.0, 240.0),
            Block {
                text: "Notes follow the table and are long enough to be the body text size of the page here.".into(),
                rect: [72.0, 460.0, 500.0, 471.0],
                size: 11.0,
                bold: false,
                italic: false,
                color: [0.0; 3],
                rotation: 0.0,
                space_around: [false; 2],
                font: String::new(),
            },
        ],
        images: Vec::new(),
        background: None,
        ..Page::default()
    }
}

#[test]
fn grid_text_becomes_a_real_table_in_every_format() {
    let p = table_page();
    let h = html(std::slice::from_ref(&p), "Bill");
    assert!(h.contains("<table>"), "{h}");
    assert!(h.contains("<td colspan=\"2\">Totals</td>"), "spanning cell: {h}");
    assert_eq!(h.matches("<tr>").count(), 4, "{h}");
    assert!(h.contains("<td></td>"), "the hole is an empty cell: {h}");
    assert!(h.contains("<p>Notes follow"), "the paragraph after the table stays: {h}");

    let d = docx(std::slice::from_ref(&p), "Bill");
    assert!(d.starts_with(b"PK"));
    assert!(d.windows(b"word/document.xml".len()).any(|w| w == b"word/document.xml"));
    let (tables, _) = tables(&p.blocks, &p.rules);
    assert_eq!(tables.len(), 1);
    let read = docx_tables(&d);
    assert_eq!(read.len(), 1, "{read:?}");
    assert_eq!(read[0].grid.len(), 3, "a three-column grid");
    assert_eq!(read[0].rows.len(), 4);
    assert!(read[0].rows.iter().any(|r| r.cells.first().is_some_and(|c| c.props.span == 2)), "Totals spans two columns: {:?}", read[0]);

    let r = rtf(std::slice::from_ref(&p));
    assert!(r.contains("\\trowd"), "{r}");
    assert_eq!(r.matches("\\row").count(), 4);
    // Each cell ends at its last column's right edge, measured from the left margin (`\cellx`):
    // the columns start at x = 72, 232 and 392 and the table ends at 496, so 160, 160 and 104 pt
    // wide. "Totals" spans two columns, so it ends at the second edge, and its row is padded to
    // the grid with an empty cell.
    let back = craft_rtf::read(r.as_bytes()).unwrap();
    let rows: Vec<Vec<(i32, String)>> = back
        .blocks
        .iter()
        .filter_map(|b| if let craft_rtf::Block::Table(t) = b { Some(t) } else { None })
        .flat_map(|t| &t.rows)
        .map(|row| row.iter().map(|c| (c.right, c.paras.iter().map(|p| p.text()).collect())).collect())
        .collect();
    assert_eq!(rows.first().map(|r| r.iter().map(|c| c.0).collect::<Vec<_>>()), Some(vec![3200, 6400, 8480]), "{rows:?}");
    assert!(rows.contains(&vec![(6400, "Totals".to_string()), (8480, String::new())]), "{rows:?}");

    let pt = pptx(std::slice::from_ref(&p), "Bill");
    assert!(pt.starts_with(b"PK"));
    assert!(pt.windows(b"ppt/slides/slide1.xml".len()).any(|w| w == b"ppt/slides/slide1.xml"));
    let slide_xml = part(&pt, "ppt/slides/slide1.xml");
    assert!(slide_xml.contains("<a:tbl>"));
    assert!(slide_xml.contains("gridSpan=\"2\""));
    assert_eq!(slide_xml.matches("<a:tr ").count(), 4);
}

#[test]
fn a_staircase_of_blocks_is_not_a_huge_table() {
    // Row k holds blocks in columns k and k + 1: every column has two members and every row
    // two columns, so without a cap this would be one table of (n / 2)² cells.
    let n = 4000;
    let blocks: Vec<Block> = (0..n)
        .map(|i| {
            let (row, col) = (i / 2, i / 2 + i % 2);
            let (x, y) = (10.0 + col as f64 * 20.0, 10_000.0 - row as f64 * 14.0);
            Block {
                text: "x".into(),
                rect: [x, y, x + 8.0, y + 12.0],
                size: 11.0,
                bold: false,
                italic: false,
                color: [0.0; 3],
                rotation: 0.0,
                space_around: [false; 2],
                font: String::new(),
            }
        })
        .collect();
    let started = std::time::Instant::now();
    let (tables, _) = tables(&blocks, &[]);
    assert!(tables.iter().all(|t| t.cols.len() <= MAX_COLS), "{} columns", tables.iter().map(|t| t.cols.len()).max().unwrap_or(0));
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "took {:?}", started.elapsed());
}

#[test]
fn word_opens_tables_up_to_its_column_limit() {
    // Rows of short numbers on a regular grid, n columns wide.
    let grid = |n: usize| Page {
        width: 1500.0,
        height: 792.0,
        blocks: (0..3)
            .flat_map(|row| {
                (0..n).map(move |col| {
                    let (x, y) = (20.0 + col as f64 * 22.0, 700.0 - row as f64 * 14.0);
                    Block {
                        text: (col + 1).to_string(),
                        rect: [x, y, x + 8.0, y + 8.0],
                        size: 8.0,
                        bold: false,
                        italic: false,
                        color: [0.0; 3],
                        rotation: 0.0,
                        space_around: [false; 2],
                        font: String::new(),
                    }
                })
            })
            .collect(),
        images: Vec::new(),
        background: None,
        ..Page::default()
    };
    let grid_cols = |p: &Page| part(&docx(std::slice::from_ref(p), "Grid"), "word/document.xml").matches("<w:gridCol ").count();
    assert_eq!(grid_cols(&grid(63)), 63, "63 columns is still a table");
    // Word refuses to open a document with a 64-column table, so that grid stays text.
    assert_eq!(grid_cols(&grid(64)), 0);
}

fn block(t: &str, x0: f64, y0: f64, x1: f64, y1: f64) -> Block {
    Block {
        text: t.into(),
        rect: [x0, y0, x1, y1],
        size: 12.0,
        bold: false,
        italic: false,
        color: [0.0; 3],
        rotation: 0.0,
        space_around: [false; 2],
        font: String::new(),
    }
}

/// #2806: a sentence whose link is set in another colour or font runs on as separate blocks
/// on one line. They are not table columns, even when several such lines share left edges.
#[test]
fn a_link_inside_a_sentence_is_not_a_table_column() {
    let blue = |t: &str, x0: f64, y0: f64, x1: f64, y1: f64| Block { color: [0.02, 0.388, 0.757], ..block(t, x0, y0, x1, y1) };
    let p = Page {
        width: 612.0,
        height: 792.0,
        blocks: vec![
            block("6.3. 2. All content shown at", 121.7, 710.3, 272.0, 724.9),
            blue("http", 273.5, 710.3, 294.1, 724.9),
            blue("s", 294.1, 709.8, 298.8, 725.8),
            blue("://women", 298.8, 710.3, 348.0, 724.9),
            blue("g", 348.1, 709.8, 353.7, 725.8),
            blue("randprix.fide.com", 353.7, 710.3, 443.2, 724.9),
            block("shall be reviewed and approved by the Press Officer.", 164.3, 622.5, 533.1, 724.9),
            block("6.3. 3. The Organiser shall warrant that", 121.7, 601.8, 345.1, 616.5),
            blue("https://womengrandprix.fide.com", 350.2, 601.3, 519.9, 617.4),
            block("is managed in a professional way and furthermore that it is capable of", 164.3, 557.9, 533.0, 616.5),
            block("6. 3. 4. The Press Officer shall report to the FIDE Head of PR.", 121.7, 516.6, 530.3, 551.9),
            block("conducted during a WGP tournament.", 164.3, 502.0, 352.5, 516.6),
        ],
        images: Vec::new(),
        background: None,
        ..Page::default()
    };
    let h = html(std::slice::from_ref(&p), "Regulations");
    assert!(!h.contains("<table>"), "running text became a table: {h}");
    // The link is a run of its sentence, in its own colour (#4158).
    assert!(h.contains("<p>6.3. 2. All content shown at <span style=\"color:#0563C1\">http</span>"), "{h}");
    for b in &p.blocks {
        assert!(h.contains(&b.text), "{} was lost: {h}", b.text);
    }
}

/// #2806: lines that each run on into a link starting at the same place are sentences, not a
/// two-column table, even three in a row.
#[test]
fn sentences_that_run_on_into_links_are_not_a_table() {
    let blue = |t: &str, x0: f64, y0: f64, x1: f64, y1: f64| Block { color: [0.02, 0.388, 0.757], ..block(t, x0, y0, x1, y1) };
    let mut blocks = Vec::new();
    for (n, lead) in ["Rules are published at", "Pairings are posted at", "Results are listed at"].iter().enumerate() {
        let y = 700.0 - 16.0 * n as f64;
        blocks.push(block(lead, 72.0, y, 211.0, y + 12.0));
        blocks.push(blue("https://example.org/event", 212.5, y, 360.0, y + 12.0));
    }
    let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), ..Page::default() };
    let h = html(std::slice::from_ref(&p), "Event");
    assert!(!h.contains("<table>"), "sentences became a table: {h}");
    assert_eq!(h.matches("https://example.org/event").count(), 3, "{h}");
}

/// A continuation can drift into a grid column covered by the preceding cell's span.
/// It must remain available as paragraph text if that slot never becomes an output cell.
#[test]
fn table_spans_do_not_consume_text_they_do_not_emit() {
    let mut blocks = vec![
        Block { size: 12.0, ..block("ABCDEFGHIJK", 72.1, 697.0, 151.3, 710.8) },
        Block { size: 12.0, color: [0.0, 0.0, 1.0], ..block("LOST", 150.4, 697.0, 179.2, 710.8) },
        Block { size: 12.0, ..block("R0", 250.0, 697.0, 264.4, 710.8) },
    ];
    for row in 1..=4 {
        let y = 697.0 - f64::from(row) * 50.0;
        for (text, left, right) in [(format!("L{row}"), 72.1, 86.5), (format!("M{row}"), 146.4, 160.8), (format!("R{row}"), 250.0, 264.4)] {
            blocks.push(Block { size: 12.0, ..block(&text, left, y, right, y + 13.8) });
        }
    }
    let p = Page { width: 612.0, height: 792.0, blocks, ..Page::default() };
    let (tables, consumed) = tables(&p.blocks, &p.rules);
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].rows[0][0].span, 2);
    assert!(!consumed[1], "a covered slot did not emit its text");
    assert!(consumed.iter().enumerate().all(|(index, used)| index == 1 || *used));
    let outputs = [
        html(std::slice::from_ref(&p), "Continuations"),
        part(&docx(std::slice::from_ref(&p), "Continuations"), "word/document.xml"),
        rtf(std::slice::from_ref(&p)),
        part(&pptx(std::slice::from_ref(&p), "Continuations"), "ppt/slides/slide1.xml"),
    ];
    for output in outputs {
        for b in &p.blocks {
            assert_eq!(output.matches(&b.text).count(), 1, "{} must survive exactly once", b.text);
        }
    }
}

/// #2806: a superscript right after a number ("1" "st") continues its cell; the prize table
/// around it stays a table.
#[test]
fn a_superscript_beside_a_number_keeps_its_table() {
    let mut blocks = vec![block("PLACE", 100.0, 700.0, 140.0, 712.0), block("PRIZE (euros)", 300.0, 700.0, 380.0, 712.0)];
    for (n, (row, sup)) in [("1", "st"), ("2", "nd"), ("3", "rd"), ("4", "th")].iter().enumerate() {
        let y = 680.0 - 20.0 * n as f64;
        blocks.push(block(row, 100.0, y, 106.0, y + 12.0));
        blocks.push(Block { size: 8.0, ..block(sup, 106.0, y + 5.0, 113.0, y + 13.0) });
        blocks.push(block(&format!("{},000", 30 - 5 * n), 300.0, y, 340.0, y + 12.0));
    }
    let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), ..Page::default() };
    let h = html(std::slice::from_ref(&p), "Prizes");
    assert!(h.contains("<table>"), "the prize table was lost: {h}");
    assert!(h.contains("30,000") && h.contains("15,000"), "{h}");
    let (tables, consumed) = tables(&p.blocks, &p.rules);
    assert!(consumed.iter().all(|used| *used), "ordinary table continuations stay in their cells");
    let cells: Vec<&str> = tables.iter().flat_map(|t| &t.rows).flatten().map(|c| c.text.as_str()).collect();
    assert!(["st", "nd", "rd", "th"].iter().all(|suffix| cells.contains(suffix)));
}

#[test]
fn two_column_body_text_is_not_mistaken_for_a_table() {
    let para = |t: &str, x: f64, y: f64| Block {
        text: t.into(),
        rect: [x, y, x + 200.0, y + 90.0], // nine lines tall: body text, not a cell
        size: 11.0,
        bold: false,
        italic: false,
        color: [0.0; 3],
        rotation: 0.0,
        space_around: [false; 2],
        font: String::new(),
    };
    let p = Page {
        width: 612.0,
        height: 792.0,
        blocks: vec![
            para("Left column text of the page, long enough to be a flowing paragraph.", 72.0, 500.0),
            para("Right column text of the page, long enough to be a flowing paragraph.", 320.0, 500.0),
            para("Second left block, still a tall flowing paragraph rather than a cell.", 72.0, 400.0),
            para("Second right block, still a tall flowing paragraph rather than a cell.", 320.0, 400.0),
        ],
        images: Vec::new(),
        background: None,
        ..Page::default()
    };
    let h = html(&[p], "Paper");
    assert!(!h.contains("<table>"), "tall two-column text must stay paragraphs: {h}");
}

/// #1936: every picture Word is told to draw (`r:embed`) names a relationship whose target is a
/// file in the package, with a content type; JPEG pictures too, inline or placed.
#[test]
fn word_pictures_resolve_to_media_in_the_package() {
    let jpeg = Image { ext: "jpg", bytes: [0xFF, 0xD8, 0xFF, 0xD9].as_slice().into(), ..page().images[0].clone() };
    for placed in [false, true] {
        let mut p = page();
        p.images = vec![jpeg.clone()];
        if placed {
            p.images[0].rect = [72.0, 640.0, 272.0, 790.0];
            p.blocks[0].rect = [72.0, 700.0, 300.0, 760.0];
        }
        let zip = docx(&[p], "T");
        let doc = craft_docx::read(&zip).unwrap();
        let pictures: Vec<_> = body_objects(&doc).into_iter().filter(|o| matches!(o, craft_wordmodel::InlineObject::Image { .. })).collect();
        assert_eq!(pictures.len(), 1);
        let craft_wordmodel::InlineObject::Image { media, float, .. } = pictures[0] else { panic!("not a picture") };
        assert_eq!(float.wrap != craft_wordmodel::para::Wrap::Inline, placed);
        // The picture resolves to the JPEG's bytes in the package, which has a type for it.
        assert_eq!(doc.media.get(media).map(|b| b.to_vec()), Some(vec![0xFF, 0xD8, 0xFF, 0xD9]), "{:?}", doc.media.keys().collect::<Vec<_>>());
        assert!(part(&zip, "[Content_Types].xml").contains("image/jpeg"));
    }
}

#[test]
fn word_export_bounds_the_number_of_pictures_and_skips_empty_ones() {
    let mut p = page();
    let one = p.images[0].clone();
    p.images = vec![Image { bytes: [].as_slice().into(), ..one.clone() }];
    p.images.extend(std::iter::repeat_n(one, MAX_MEDIA + 10));
    let zip = docx(&[p], "T");
    let doc = craft_docx::read(&zip).unwrap();
    let placed = body_objects(&doc).into_iter().filter(|o| matches!(o, craft_wordmodel::InlineObject::Image { .. })).count();
    assert_eq!(placed, MAX_MEDIA, "at most MAX_MEDIA pictures, and none for the empty one");
    // One picture drawn many times is one file in the package.
    assert_eq!(doc.media.len(), 1);
}

/// The flowing paragraphs of a Word document (table cells left out), each as its runs' text
/// and whether the run is italic.
fn word_paragraphs(xml: &str) -> Vec<Vec<(String, bool)>> {
    let body = xml.split("<w:tbl>").map(|s| s.split_once("</w:tbl>").map_or(s, |(_, after)| after)).collect::<String>();
    body.split("<w:p>")
        .skip(1)
        .map(|p| {
            let p = p.split_once("</w:p>").map_or(p, |(inside, _)| inside);
            p.split("<w:r>")
                .skip(1)
                .filter_map(|r| {
                    let text = r.split_once("<w:t xml:space=\"preserve\">")?.1.split_once("</w:t>")?.0;
                    Some((text.to_string(), r.contains("<w:i/>")))
                })
                .collect::<Vec<_>>()
        })
        .filter(|runs| !runs.is_empty())
        .collect()
}

fn runs(r: &[(&str, bool)]) -> Vec<(String, bool)> {
    r.iter().map(|(t, i)| (t.to_string(), *i)).collect()
}

/// "Before *Chevron* after." on one line: the roman text, the italic case name and the roman
/// text after it, the way reading blocks split a sentence at its style changes (#4158). The
/// italic face stands a little taller than the roman one around it.
fn italic_sentence() -> Vec<Block> {
    vec![
        block("Before", 72.0, 700.0, 110.0, 712.0),
        Block { italic: true, ..block("Chevron", 113.0, 700.0, 158.0, 712.6) },
        block("after.", 161.0, 700.0, 190.0, 712.0),
    ]
}

/// #4158: a run in another style inside a sentence stays in that sentence's paragraph, as a
/// run in its own style, in every Office format.
#[test]
fn an_italic_word_stays_in_its_sentence() {
    let mut blocks = italic_sentence();
    blocks.push(block("The next line is a paragraph of its own.", 72.0, 684.0, 300.0, 696.0));
    let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), ..Page::default() };
    let xml = part(&docx(std::slice::from_ref(&p), "Opinion"), "word/document.xml");
    assert_eq!(
        word_paragraphs(&xml),
        vec![runs(&[("Before ", false), ("Chevron ", true), ("after.", false)]), runs(&[("The next line is a paragraph of its own.", false)])],
        "{xml}"
    );
    let h = html(std::slice::from_ref(&p), "Opinion");
    assert!(h.contains("<p>Before <em>Chevron </em>after.</p>"), "{h}");
    let r = rtf(std::slice::from_ref(&p));
    let size = Some(24);
    assert_eq!(
        rtf_paras(&r).first(),
        Some(&vec![("Before ".into(), false, false, size), ("Chevron ".into(), false, true, size), ("after.".into(), false, false, size)]),
        "{r}"
    );
    // The longest line is the slide's title; the sentence is one paragraph of the body.
    let pxml = part(&pptx(std::slice::from_ref(&p), "Opinion"), "ppt/slides/slide1.xml");
    let slide = craft_xml::parse(pxml.as_bytes()).expect("slide XML");
    let paragraph = slide
        .descendants()
        .filter(|n| n.is("a:p"))
        .find(|p| p.children("a:r").any(|r| r.child_text("a:t").as_deref() == Some("Before ")))
        .expect("body paragraph");
    let actual: Vec<_> = paragraph
        .children("a:r")
        .map(|r| {
            let props = r.child("a:rPr").expect("run properties");
            (r.child_text("a:t").expect("run text").into_owned(), props.attr_u32("sz"), props.flag("i", false))
        })
        .collect();
    assert_eq!(actual, vec![("Before ".into(), Some(1200), false), ("Chevron ".into(), Some(1200), true), ("after.".into(), Some(1200), false)]);
}

/// #4158: the runs keep their order along the line whatever order the blocks come in, and the
/// paragraph keeps its place on the page, though the italic run stands taller than the first.
#[test]
fn a_sentence_keeps_its_reading_order() {
    let mut blocks = vec![block("Then the next line.", 72.0, 684.0, 200.0, 696.0)];
    blocks.extend(italic_sentence().into_iter().rev());
    let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), ..Page::default() };
    let xml = part(&docx(std::slice::from_ref(&p), "Opinion"), "word/document.xml");
    assert_eq!(
        word_paragraphs(&xml),
        vec![runs(&[("Before ", false), ("Chevron ", true), ("after.", false)]), runs(&[("Then the next line.", false)])],
        "{xml}"
    );
}

/// The text of a block leaves out the spaces at its ends, and the box of a run often takes in
/// its trailing space, so runs can touch. The source's own space still separates them; runs
/// that touch with no space between them (a superscript) stay joined.
#[test]
fn runs_keep_the_spaces_the_source_had_between_them() {
    let blocks = vec![
        Block { space_around: [false, true], ..block("Before", 72.0, 700.0, 113.0, 712.0) },
        Block { italic: true, ..block("Chevron", 113.0, 700.0, 158.0, 712.0) },
        Block { space_around: [true, false], ..block("after.", 158.0, 700.0, 190.0, 712.0) },
        block("E = mc", 72.0, 660.0, 110.0, 672.0),
        Block { size: 8.0, ..block("2", 110.0, 665.0, 115.0, 673.0) },
    ];
    let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), ..Page::default() };
    let xml = part(&docx(std::slice::from_ref(&p), "Spaces"), "word/document.xml");
    assert_eq!(
        word_paragraphs(&xml),
        vec![runs(&[("Before ", false), ("Chevron ", true), ("after.", false)]), runs(&[("E = mc", false), ("2", false)])],
        "{xml}"
    );
}
/// Text turned another way (a label up the margin) is not a run of the line it touches.
#[test]
fn turned_text_is_not_a_run_of_the_line_beside_it() {
    let blocks = vec![block("Before", 72.0, 700.0, 110.0, 712.0), Block { rotation: 90.0, ..block("SIDE", 111.0, 690.0, 123.0, 720.0) }];
    let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), ..Page::default() };
    let h = html(std::slice::from_ref(&p), "Turned");
    assert!(h.contains("<p>Before</p>") && h.contains("<p>SIDE</p>"), "{h}");
}
/// A block that starts where the line above ends is the next line, not a run of it.
#[test]
fn a_block_on_the_next_line_starts_a_paragraph() {
    let blocks = vec![
        block("The first line ends here", 72.0, 700.0, 190.0, 712.0),
        Block { italic: true, ..block("and the next starts there.", 190.5, 690.0, 320.0, 702.0) },
    ];
    let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), ..Page::default() };
    let xml = part(&docx(std::slice::from_ref(&p), "Lines"), "word/document.xml");
    assert_eq!(word_paragraphs(&xml), vec![runs(&[("The first line ends here", false)]), runs(&[("and the next starts there.", true)])], "{xml}");
}

/// A superscript beside a table cell stays in the table: table detection consumes it before
/// paragraphs are formed, and a sentence after the table still keeps its italic run.
#[test]
fn table_cells_are_not_runs_of_a_paragraph() {
    let mut blocks = vec![block("PLACE", 100.0, 700.0, 140.0, 712.0), block("PRIZE (euros)", 300.0, 700.0, 380.0, 712.0)];
    for (n, (row, sup)) in [("1", "st"), ("2", "nd"), ("3", "rd")].iter().enumerate() {
        let y = 680.0 - 20.0 * n as f64;
        blocks.push(block(row, 100.0, y, 106.0, y + 12.0));
        blocks.push(Block { size: 8.0, ..block(sup, 106.0, y + 5.0, 113.0, y + 13.0) });
        blocks.push(block(&format!("{},000", 30 - 5 * n), 300.0, y, 340.0, y + 12.0));
    }
    blocks.extend(italic_sentence().into_iter().map(|b| Block { rect: [b.rect[0], b.rect[1] - 200.0, b.rect[2], b.rect[3] - 200.0], ..b }));
    let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), ..Page::default() };
    let xml = part(&docx(std::slice::from_ref(&p), "Prizes"), "word/document.xml");
    assert_eq!(xml.matches("<w:tbl>").count(), 1, "{xml}");
    for cell in ["st", "nd", "rd", "30,000"] {
        assert_eq!(xml.matches(&format!("preserve\">{cell}</w:t>")).count(), 1, "{cell}: {xml}");
    }
    // The cells are all in the table: the only flowing paragraph is the sentence.
    assert_eq!(word_paragraphs(&xml), vec![runs(&[("Before ", false), ("Chevron ", true), ("after.", false)])], "{xml}");
}

/// A bold lead-in followed by body text on its line is body text, not a heading; a heading
/// with a smaller footnote marker after it stays a heading.
#[test]
fn a_paragraph_is_a_heading_only_when_its_text_is() {
    let blocks = vec![
        Block { size: 24.0, ..block("Annual Report", 72.0, 740.0, 250.0, 764.0) },
        Block { size: 14.0, ..block("1", 251.0, 752.0, 258.0, 766.0) },
        Block { bold: true, ..block("Summary", 72.0, 700.0, 125.0, 712.0) },
        block("the year closed with more members than it opened with, and fewer debts.", 128.0, 700.0, 540.0, 712.0),
    ];
    let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), ..Page::default() };
    let xml = part(&docx(std::slice::from_ref(&p), "Report"), "word/document.xml");
    assert_eq!(xml.matches("<w:pStyle w:val=\"Heading1\"/>").count(), 1, "{xml}");
    assert!(!xml.contains("Heading2"), "the lead-in made its paragraph a heading: {xml}");
    assert_eq!(word_paragraphs(&xml).len(), 2, "{xml}");
}

/// #1595: Markdown gets the headings, paragraphs and a rule between pages, in reading order;
/// images are left out.
#[test]
fn markdown_has_headings_paragraphs_and_page_rules_in_order() {
    let md = markdown(&[page(), page()]);
    let order: Vec<usize> = ["# Annual Report\n\n", "## Overview\n\n", "Some \\<body\\> text \\& more", "Body text after the picture", "\n\n---\n\n"]
        .iter()
        .map(|x| md.find(x).unwrap_or_else(|| panic!("{x:?} missing in {md}")))
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{md}");
    assert_eq!(md.matches("\n---\n").count(), 1, "one rule between two pages: {md}");
    assert_eq!(md.matches("# Annual Report").count(), 2, "{md}");
    assert!(!md.contains("PNG") && !md.contains("!["), "images are left out: {md}");
    assert!(md.ends_with(".\n") && !md.ends_with("\n\n"), "{md:?}");
}

/// #1595: text Markdown would read as markup stays text, emphasis wraps a run without its outer
/// spaces, and a detected table becomes a pipe table with its first row as the header.
#[test]
fn markdown_escapes_markup_and_keeps_emphasis_and_tables() {
    let b = |t: &str, y: f64, bold: bool, italic: bool| Block {
        text: t.into(),
        rect: [72.0, y, 500.0, y + 11.0],
        size: 11.0,
        bold,
        italic,
        color: [0.0; 3],
        rotation: 0.0,
        space_around: [false; 2],
        font: String::new(),
    };
    let p = Page {
        width: 612.0,
        height: 792.0,
        blocks: vec![
            b("- not a list item, and long enough to make eleven points the body size here", 700.0, false, false),
            b("1. not a numbered item either", 650.0, false, false),
            b("*stars* _under_ `tick` [link](x) a|b # #tag\u{1}\nnext", 600.0, false, false),
            b("Strong words in a sentence of their own.", 550.0, true, false),
            b("slanted", 500.0, false, true),
        ],
        images: Vec::new(),
        background: None,
        ..Page::default()
    };
    let md = markdown(&[p]);
    assert!(md.contains("\\- not a list item"), "{md}");
    assert!(md.contains("1\\. not a numbered item"), "{md}");
    assert!(md.contains("\\*stars\\* \\_under\\_ \\`tick\\` \\[link\\](x) a\\|b \\# \\#tag next"), "{md}");
    assert!(md.contains("**Strong words in a sentence of their own.**"), "{md}");
    // Markdown reads no emphasis where a marker sits next to a space: the outer spaces go outside.
    assert_eq!(emphasis(" two words ", true, false, false, false), " **two words** ");
    assert_eq!(emphasis("both", true, true, true, true), "***both***");
    assert_eq!(emphasis("  ", true, true, true, true), "  ", "nothing to mark");
    // Runs in one style are one: two bold runs side by side don't meet as `****`.
    assert_eq!(paragraph(&[("hello", true, false), ("world", true, false)]), "**helloworld**");
    // Markers inside a word would stay literal in CommonMark, so a run there gets HTML tags.
    assert_eq!(paragraph(&[("foo", false, false), (".", true, false), ("bar", false, false)]), "foo<strong>.</strong>bar");
    assert_eq!(paragraph(&[("x", true, false), ("y", false, true)]), "<strong>x</strong>*y*", "two styles side by side");
    // `*` may sit inside a word, and before punctuation that follows a letter.
    assert_eq!(paragraph(&[("foo", false, false), ("bar", true, false), ("baz", false, false)]), "foo**bar**baz");
    assert_eq!(paragraph(&[("a ", false, false), ("b", true, false), (" c", false, false)]), "a **b** c");
    assert_eq!(paragraph(&[("say ", false, false), ("this", false, true), (", then", false, false)]), "say *this*, then");
    assert!(md.contains("*slanted*"), "{md}");
    assert!(!md.chars().any(|c| !allowed(c)), "{md:?}");

    let md = markdown(&[table_page()]);
    let rows: Vec<&str> = md.lines().filter(|l| l.starts_with('|')).collect();
    assert_eq!(rows.first(), Some(&"| Meter | Unit | Reading |"), "{md}");
    assert_eq!(rows.get(1), Some(&"| --- | --- | --- |"), "{md}");
    assert!(rows.contains(&"| A1 | kWh | 37414.00 |"), "{md}");
    assert!(rows.iter().all(|r| r.matches(" | ").count() == 2), "every row has the header's three cells: {md}");
}

/// A stroked axis-aligned box around `r` ([x0, y0, x1, y1], y up).
fn boxed(r: [f64; 4]) -> Shape {
    Shape {
        even_odd: false,
        paths: vec![(vec![[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]], true)],
        stroke: Some(Stroke { color: [0.0; 3], width: 0.7, dashed: false }),
        fill: None,
    }
}

/// A form page like a pension office's refund claim: labels in boxes, comb cells, and fields.
fn form_page() -> Page {
    let labels = ["Name", "First name", "Birth name", "Date of birth", "Street", "Postcode", "Town"];
    let mut blocks = Vec::new();
    let mut shapes = Vec::new();
    for (i, t) in labels.iter().enumerate() {
        let y = 700.0 - 30.0 * i as f64;
        blocks.push(Block { size: 8.0, ..block(t, 60.0, y + 14.0, 120.0, y + 22.0) });
        let mut frame = boxed([56.0, y, 540.0, y + 24.0]);
        if i % 2 == 0 {
            // Drawn as this claim form's producer draws them: four segments back to the corner,
            // left open (`m l l l l S`).
            let (points, _) = &frame.paths[0];
            let mut open = points.clone();
            open.push(points[0]);
            frame.paths = vec![(open, false)];
        }
        shapes.push(frame);
    }
    // A comb cell's tick, a dashed rule and a free-form (curved) mark.
    shapes.push(Shape { paths: vec![(vec![[80.0, 400.0], [80.0, 404.0]], false)], ..boxed([0.0; 4]) });
    shapes.push(Shape {
        even_odd: false,
        stroke: Some(Stroke { color: [1.0, 0.0, 0.0], width: 0.0, dashed: true }),
        paths: vec![(vec![[60.0, 380.0], [300.0, 380.0]], false)],
        fill: None,
    });
    shapes.push(Shape {
        even_odd: false,
        paths: vec![(vec![[100.0, 300.0], [120.0, 320.0], [140.0, 300.0]], true)],
        stroke: None,
        fill: Some([0.5; 3]),
    });
    shapes.push(Shape { paths: vec![(vec![[60.0, 500.0], [70.0, 510.0]], false)], ..boxed([0.0; 4]) });
    let fields = vec![
        Field {
            name: "Name".into(),
            rect: [60.0, 700.0, 300.0, 712.0],
            size: 0.0,
            font: String::new(),
            read_only: false,
            color: [0.0; 3],
            alignment: 0,
            round: false,
            kind: FieldKind::Text { value: String::new(), multiline: false, comb: None, max_len: None },
            border: None,
            background: None,
        },
        Field {
            name: "Postcode".into(),
            rect: [60.0, 520.0, 160.0, 532.0],
            size: 10.0,
            font: String::new(),
            read_only: false,
            color: [0.0; 3],
            alignment: 0,
            round: false,
            kind: FieldKind::Text { value: "10115".into(), multiline: false, comb: Some(5), max_len: Some(5) },
            border: None,
            background: None,
        },
        Field {
            name: "Notes".into(),
            rect: [60.0, 200.0, 540.0, 260.0],
            size: 9.0,
            font: String::new(),
            read_only: false,
            color: [0.0; 3],
            alignment: 0,
            round: false,
            kind: FieldKind::Text { value: "a & b".into(), multiline: true, comb: None, max_len: None },
            border: None,
            background: None,
        },
        Field {
            name: "Yes".into(),
            rect: [60.0, 150.0, 70.0, 160.0],
            size: 0.0,
            font: String::new(),
            read_only: false,
            color: [0.0; 3],
            alignment: 0,
            round: false,
            kind: FieldKind::Check { checked: true, on_state: "Yes".into() },
            border: None,
            background: None,
        },
        Field {
            name: "No".into(),
            rect: [90.0, 150.0, 100.0, 160.0],
            size: 0.0,
            font: String::new(),
            read_only: false,
            color: [0.0; 3],
            alignment: 0,
            round: false,
            kind: FieldKind::Check { checked: false, on_state: "Yes".into() },
            border: None,
            background: None,
        },
        Field {
            name: "Fund".into(),
            rect: [120.0, 150.0, 300.0, 162.0],
            size: 0.0,
            font: String::new(),
            read_only: false,
            color: [0.0; 3],
            alignment: 0,
            round: false,
            kind: FieldKind::Choice { value: "Health".into(), options: vec!["Health".into(), "Pension".into()], editable: false },
            border: None,
            background: None,
        },
    ];
    Page { width: 595.0, height: 842.0, blocks, shapes, fields, ..Page::default() }
}

/// A form exported to Word keeps its layout: its boxes, rules and comb cells are shapes behind
/// the text, in painting order, and its labels stay where they were (gap 8: the boxes and
/// fields were all lost, and the labels reflowed onto twice as many pages).
#[test]
fn word_keeps_a_forms_boxes_rules_and_labels_in_place() {
    let zip = docx(&[form_page()], "Claim");
    assert!(docx_tables(&zip).is_empty() && !part(&zip, "word/document.xml").contains("<wp:inline"), "nothing flows");
    let doc = word(&zip);
    let art = drawn(&doc);
    assert_eq!(art.len(), 11, "{art:?}");
    // Boxes, closed or drawn back to their corner: a rectangle each, measured from the page's
    // top-left corner (a free-form path showed a box as a diagonal in LibreOffice).
    let boxes: Vec<&Drawn> = art.iter().filter(|s| s.kind == ShapeKind::Rectangle).collect();
    assert_eq!(boxes.len(), 7, "{art:?}");
    assert!(close(boxes[0].rect, [56.0, 118.0, 484.0, 24.0]), "{:?}", boxes[0]);
    assert!(boxes.iter().all(|s| s.stroke == Some(Rgb(0, 0, 0)) && (s.width - 0.7).abs() < 0.01 && s.fill.is_none()), "{boxes:?}");
    // A straight segment is an open path of two points, y down from its box's top: the comb
    // tick the form draws upwards, and a rising diagonal.
    let tick = &art[7];
    assert!(close(tick.rect, [80.0, 438.0, 0.01, 4.0]), "{tick:?}");
    assert_eq!(tick.paths, [(vec![[0.0, 4.0], [0.0, 0.0]], false)]);
    let diagonal = &art[10];
    assert!(close(diagonal.rect, [60.0, 332.0, 10.0, 10.0]), "{diagonal:?}");
    assert_eq!(diagonal.paths, [(vec![[0.0, 10.0], [10.0, 0.0]], false)]);
    // A 0 pt line is drawn at the thinnest visible width.
    let rule = &art[8];
    assert!(rule.dashed && rule.stroke == Some(Rgb(255, 0, 0)) && (rule.width - 0.25).abs() < 0.001, "{rule:?}");
    assert!(art.iter().filter(|s| s.dashed).count() == 1);
    // Anything else is a path of straight segments, closed when it is closed or filled.
    let mark = &art[9];
    assert_eq!(mark.paths, [(vec![[0.0, 20.0], [20.0, 0.0], [40.0, 20.0]], true)]);
    assert!(mark.fill == Some(Rgb(128, 128, 128)) && mark.stroke.is_none(), "{mark:?}");
    // Painting order: the first shape is furthest back, and every shape is behind the text.
    let labels = text_boxes(&doc);
    assert!(art.windows(2).all(|w| w[0].z < w[1].z) && art.iter().all(|s| s.behind), "{art:?}");
    assert!(labels.iter().all(|(_, _, z)| art.iter().all(|s| s.z < *z)));
    // The labels keep their places, one line each with room to spare for a reader whose font
    // runs wider: 60 × 1.25 + 2 points.
    let name = labels.iter().find(|(text, ..)| text == "Name").unwrap();
    assert!(close(name.1, [60.0, 120.0, 77.0, 8.0]), "{name:?}");
    // A long line's spare room ends at the page's edge: 60 + 480 × 1.25 + 2 would pass 595.
    let mut long = form_page();
    long.blocks.push(Block { size: 10.0, ..block("A line nearly as wide as the page", 60.0, 30.0, 540.0, 40.0) });
    let doc = word(&docx(&[long], "Long"));
    let line = text_boxes(&doc).into_iter().find(|(text, ..)| text.starts_with("A line")).unwrap();
    assert!(close(line.1, [60.0, 802.0, 535.0, 10.0]), "{line:?}");
}

/// A form's fields become content controls a Word user fills in over their boxes, without
/// protecting the document, so it can still be edited, cut and copied. A field's text is set in
/// its font, size and comb spacing; Word doesn't limit a control's length, so the PDF's maximum
/// length is named in the control's title.
#[test]
fn word_makes_a_forms_fields_content_controls_that_keep_their_look() {
    let zip = docx(&[form_page()], "Claim");
    let doc = word(&zip);
    let fields = controls(&doc);
    // Every field is a content control tagged with its name; none is a legacy form field.
    assert_eq!(fields.iter().map(|(c, _)| c.tag.as_str()).collect::<Vec<_>>(), ["Name", "Postcode", "Notes", "Yes", "No", "Fund"]);
    let xml = part(&zip, "word/document.xml");
    assert!(!xml.contains("FORMTEXT") && !xml.contains("<w:ffData>"), "{xml}");
    // Fields: framed paragraphs over the fields' boxes, which Word places out of the text flow.
    let frame = |i: usize| fields[i].1.props.frame.clone().expect("a frame");
    assert!((0..6).all(|i| frame(i).h_exact && frame(i).h_anchor == "page" && frame(i).v_anchor == "page"));
    // The comb field's frame reaches one cell (20 pt) past its box: Word spaces the last
    // character too, and in Word the last digit of a postcode wrapped out of sight.
    // Its line of 10 pt text (11.5 pt high) sits in the middle of the 12 pt box: the frame
    // starts a quarter point lower, rather than its paragraph having space before it.
    let comb = frame(1);
    assert!(close([comb.x, comb.y, comb.w.unwrap(), comb.h.unwrap()], [60.0, 310.25, 120.0, 11.75]), "{comb:?}");
    assert_eq!(fields[1].1.props.space_before, Some(0.0));
    // The page places everything itself: a quarter-inch top margin, so that Word doesn't move
    // a field framed near the top of the page down to a one-inch margin.
    let sec = &doc.last_section;
    assert!(close([sec.margin_top, sec.header, sec.margin_bottom, sec.margin_left], [18.0, 9.0, 72.0, 72.0]), "{sec:?}");
    // An empty field shows a placeholder as wide as its box, in the field's font and size.
    let (name, p) = &fields[0];
    assert!(name.showing_placeholder && name.title == "Name" && name.kind == ControlKind::Text { multi_line: false }, "{name:?}");
    assert!(!text_of(p).is_empty() && text_of(p).chars().all(|c| c == '\u{2002}'));
    let arial = run_of(p);
    assert_eq!(
        (arial.font.as_deref(), arial.font_ea.as_deref(), arial.font_cs.as_deref(), arial.size),
        (Some("Arial"), Some("Arial"), Some("Arial"), Some(8.0))
    );
    // The comb's 6 pt Courier advance plus 14 pt spacing is one 20 pt cell.
    let (postcode, p) = &fields[1];
    let courier = run_of(p);
    assert_eq!((courier.font.as_deref(), courier.spacing, courier.size), (Some("Courier New"), Some(14.0), Some(10.0)));
    assert_eq!((postcode.title.as_str(), text_of(p).as_str(), postcode.showing_placeholder), ("Postcode (max. 5)", "10115", false));
    let (notes, p) = &fields[2];
    assert_eq!((notes.kind.clone(), text_of(p).as_str()), (ControlKind::Text { multi_line: true }, "a & b"));
    // Check boxes and lists keep their look the same way: an X in Arial for a checked box.
    let checked = |i: usize| match &fields[i].0.kind {
        ControlKind::CheckBox { checked, checked_char: 'X', checked_font, .. } if checked_font == "Arial" => *checked,
        other => panic!("not a check box: {other:?}"),
    };
    assert!(checked(3) && !checked(4));
    assert_eq!(
        (text_of(&fields[3].1).as_str(), run_of(&fields[3].1).font.as_deref(), fields[3].1.props.align),
        ("X", Some("Arial"), Some(Align::Center))
    );
    let ControlKind::DropDown { items, .. } = &fields[5].0.kind else { panic!("not a drop-down: {:?}", fields[5].0.kind) };
    assert_eq!(items.iter().map(|i| (i.display.as_str(), i.value())).collect::<Vec<_>>(), [("Health", "Health"), ("Pension", "Pension")]);
    assert_eq!(text_of(&fields[5].1), "Health");
    // Not protected: a protected form could not be copied or cut in Word (it only lets the
    // fields be changed), which is how a reader reuses an exported form. No control is locked.
    assert!(doc.settings.protection.is_none() && !package_text(&zip).contains("documentProtection"));
    assert!(fields.iter().all(|(c, _)| c.lock == ControlLock::Unlocked));
}

/// A content control keeps its field's name as it is, as its title and tag (Word's legacy form
/// fields had to turn a PDF's dotted, accented or repeated names into bookmark names), escaped,
/// and a hostile name is cut to [`MAX_FIELD_NAME_CHARS`] characters.
#[test]
fn word_content_controls_keep_their_fields_names() {
    let mut p = form_page();
    let text = |name: &str, max_len| Field {
        name: name.into(),
        rect: [60.0, 50.0, 200.0, 62.0],
        size: 0.0,
        font: String::new(),
        read_only: false,
        color: [0.0; 3],
        alignment: 0,
        round: false,
        kind: FieldKind::Text { value: String::new(), multiline: false, comb: None, max_len },
        border: None,
        background: None,
    };
    let long = "\u{e4}".repeat(MAX_FIELD_NAME_CHARS + 400);
    p.fields = vec![
        text("Arbeitnehmeranteil.Betrag.1", None),
        text("1. Betrag (\u{20ac}) <\"&>", Some(8)),
        text("Name", None),
        text("Name", None),
        text(&long, None),
    ];
    let xml = part(&docx(&[p], "Names"), "word/document.xml");
    let tags: Vec<&str> = xml.split("<w:tag w:val=\"").skip(1).map(|s| s.split('"').next().unwrap()).collect();
    let cut = "\u{e4}".repeat(MAX_FIELD_NAME_CHARS);
    assert_eq!(tags, ["Arbeitnehmeranteil.Betrag.1", "1. Betrag (\u{20ac}) &lt;&quot;&amp;&gt;", "Name", "Name", cut.as_str()]);
    assert!(xml.contains("<w:alias w:val=\"1. Betrag (\u{20ac}) &lt;&quot;&amp;&gt; (max. 8)\"/>"), "{xml}");
    assert!(xml.contains(&format!("<w:alias w:val=\"{cut}\"/>")) && !xml.contains(&long), "{xml}");
}

/// A page whose text is framed by boxes (a bordered table, a ruled form) keeps them even without
/// form fields; a page of running text with a few horizontal rules (underlines, a header line)
/// still flows as paragraphs, and Word leaves the rules out.
#[test]
fn word_positions_ruled_pages_and_lets_underlined_text_flow() {
    let positioned = |p: Page| !text_boxes(&word(&docx(&[p], "Report"))).is_empty();
    let ruled = Page { fields: Vec::new(), ..form_page() };
    assert!(drawn(&word(&docx(&[ruled], "Ruled"))).iter().any(|s| s.kind == ShapeKind::Rectangle));

    let mut text = page();
    text.images.clear();
    for y in [395.0, 500.0, 600.0, 690.0] {
        text.shapes.push(Shape { paths: vec![(vec![[72.0, y], [500.0, y]], false)], ..boxed([0.0; 4]) });
    }
    let doc = word(&docx(&[text.clone()], "Report"));
    assert!(text_boxes(&doc).is_empty() && drawn(&doc).is_empty(), "underlines alone don't hold a page in place, and Word leaves them out");
    // Five upright boxes around text are not yet a form either; six are.
    let mut framed = text.clone();
    for i in 0..FRAMING_SHAPES - 1 {
        framed.shapes.push(boxed([70.0, 398.0 - i as f64, 502.0, 413.0 + i as f64]));
    }
    assert!(!positioned(framed.clone()));
    framed.shapes.push(boxed([70.0, 690.0, 502.0, 726.0]));
    assert!(positioned(framed));
}

/// Hostile shapes and fields (no points, non-finite or huge coordinates, no paint, too many)
/// write a package that still opens, within the page's shape budget.
#[test]
fn word_bounds_hostile_shapes_and_fields() {
    let mut p = form_page();
    p.shapes.push(Shape { paths: Vec::new(), ..boxed([0.0; 4]) });
    p.shapes.push(Shape { paths: vec![(vec![[f64::NAN, 1.0], [f64::INFINITY, 2.0]], false)], ..boxed([0.0; 4]) });
    p.shapes.push(Shape { stroke: None, fill: None, ..boxed([1.0, 1.0, 2.0, 2.0]) });
    p.shapes
        .push(Shape { stroke: Some(Stroke { color: [f64::NAN; 3], width: f64::INFINITY, dashed: false }), ..boxed([-1e300, -1e300, 1e300, 1e300]) });
    p.fields.push(Field {
        name: "<&>\u{0}".into(),
        rect: [f64::NAN, 0.0, 1.0, 1.0],
        size: f64::NAN,
        font: String::new(),
        read_only: false,
        color: [0.0; 3],
        alignment: 0,
        round: false,
        kind: FieldKind::Check { checked: true, on_state: "Yes".into() },
        border: None,
        background: None,
    });
    p.fields.push(Field {
        name: "x".into(),
        rect: [0.0, 0.0, 1e300, 1e300],
        size: -5.0,
        font: String::new(),
        read_only: false,
        color: [0.0; 3],
        alignment: 0,
        round: false,
        kind: FieldKind::Text { value: "1".into(), multiline: false, comb: Some(0), max_len: Some(0) },
        border: None,
        background: None,
    });
    for i in 0..MAX_DOCX_PAGE_SHAPES + 50 {
        p.shapes.push(Shape { paths: vec![(vec![[1.0, i as f64 / 100.0], [2.0, 3.0]], false)], ..boxed([0.0; 4]) });
    }
    let xml = part(&docx(&[p], "Hostile"), "word/document.xml");
    assert!(xml.matches("id=\"Shape").count() <= MAX_DOCX_PAGE_SHAPES);
    assert!(!xml.contains("NaN") && !xml.contains("inf"), "no non-finite number reaches the XML");
    assert!(!xml.contains('\u{0}'));
}

/// The PDF's font names become the families Word knows, so text keeps its face and width (the
/// claim form's Arial showed in Word's default serif and overran its boxes).
#[test]
fn word_sets_text_in_the_family_of_its_pdf_font() {
    for (base, family) in [
        ("BFPKIJ+Arial-BoldMT", Some("Arial")),
        ("ArialMT", Some("Arial")),
        ("Helvetica-Oblique", Some("Arial")),
        ("TimesNewRomanPSMT", Some("Times New Roman")),
        ("Times-Roman", Some("Times New Roman")),
        ("Courier", Some("Courier New")),
        ("SegoeUI,Bold", Some("Segoe UI")),
        ("ABCDEF+MinionPro-Regular", Some("Minion Pro")),
        ("ITCAvantGarde", Some("ITC Avant Garde")),
        // A hyphen before a family word stays in the name, as Copy with formatting reads it.
        ("MS-Gothic", Some("MS Gothic")),
        ("ABCDEF+MS-Mincho", Some("MS Mincho")),
        ("Courier-Bold", Some("Courier New")),
        ("Symbol", Some("Symbol")),
        ("", None),
        ("ABCDEF+", None),
        ("-Bold", None),
        ("MT", Some("MT")),
    ] {
        assert_eq!(word_font(base).as_deref(), family, "{base}");
    }
    assert!(word_font(&"VeryLongFamilyName".repeat(5)).is_some_and(|f| f.chars().count() <= 31));

    let mut p = form_page();
    p.blocks[0].font = "BFPKJL+ArialMT".into();
    p.blocks[1].font = "Bad\"<Name>".into();
    let xml = part(&docx(&[p.clone()], "Fonts"), "word/document.xml");
    assert!(
        xml.contains(
            "<w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\" w:eastAsia=\"Arial\" w:cs=\"Arial\"/><w:sz w:val=\"16\"/></w:rPr><w:t xml:space=\"preserve\">Name</w:t>"
        ),
        "{xml}"
    );
    // A family keeps only letters, digits, spaces and `-_.`: nothing in it needs escaping.
    assert!(xml.contains("w:ascii=\"BadName\""), "{xml}");
    assert!(!xml.contains("Bad&quot;") && !xml.contains("&lt;Name"), "{xml}");
    // Flowing pages and table cells carry it too.
    let mut flowing = page();
    flowing.blocks[0].font = "TimesNewRomanPSMT".into();
    assert!(part(&docx(&[flowing], "Report"), "word/document.xml").contains("<w:rFonts w:ascii=\"Times New Roman\""));
    let mut table = table_page();
    for b in &mut table.blocks {
        b.font = "Courier".into();
    }
    let xml = part(&docx(&[table], "Invoice"), "word/document.xml");
    assert!(xml.contains("<w:tbl>") && xml.contains("<w:tc><w:tcPr>") && xml.contains("<w:rFonts w:ascii=\"Courier New\""), "{xml}");
}

/// A form whose last page holds only notes keeps that page's layout too, when most of its pages
/// keep theirs: flowing it pushed its footer onto a page of its own (seven pages became eight).
/// A report with one form page among flowing pages still flows.
#[test]
fn word_keeps_every_page_of_a_mostly_positioned_document_in_place() {
    let mut notes = page();
    notes.images.clear();
    let positioned = |pages: &[Page]| part(&docx(pages, "Claim"), "word/document.xml").contains(">Annual Report</w:t></w:r></w:p></w:txbxContent>");
    assert!(positioned(&[form_page(), form_page(), notes.clone()]), "two form pages of three");
    assert!(!positioned(&[form_page(), notes.clone(), notes.clone()]), "one form page of three");
    assert!(!positioned(&[form_page(), notes.clone()]), "half is not most");
    // A flowing page keeps its one-inch margins.
    let xml = part(&docx(&[form_page(), notes.clone()], "Claim"), "word/document.xml");
    assert!(xml.contains("<w:pgMar w:top=\"360\"") && xml.contains("<w:pgMar w:top=\"1440\""), "{xml}");
}

/// Every shape Word draws counts against the page's limit, not every path: one path of many
/// subpaths wrote a shape per subpath past the limit, without a word. A page that has to
/// leave shapes, points or fields out is listed; one that fits is not.
#[test]
fn word_counts_every_subpath_against_the_page_limit() {
    let tick = |i: usize| (vec![[1.0, 100.0 + i as f64 / 100.0], [2.0, 110.0]], false);
    let with = |subpaths: usize| {
        let mut p = form_page();
        p.shapes.push(Shape { paths: (0..subpaths).map(tick).collect(), ..boxed([0.0; 4]) });
        p
    };
    // The form page draws 11 shapes of its own.
    let fits = with(MAX_DOCX_PAGE_SHAPES - 11);
    let (zip, dropped) = docx_checked(&[fits], "Fits");
    assert!(dropped.is_empty());
    assert_eq!(drawn(&word(&zip)).len(), MAX_DOCX_PAGE_SHAPES);
    let (zip, dropped) = docx_checked(&[form_page(), with(MAX_DOCX_PAGE_SHAPES)], "Too many");
    assert_eq!(dropped, [1], "the second page is cut short");
    let doc = word(&zip);
    // Each page's shapes hang from its own anchoring paragraph.
    let pages: Vec<usize> =
        body_paragraphs(&doc).into_iter().map(|p| drawn_of(&p.objects.iter().collect::<Vec<_>>()).len()).filter(|shapes| *shapes > 0).collect();
    assert_eq!(pages, [11, MAX_DOCX_PAGE_SHAPES], "it keeps the first ones");
    // One polyline with more points than the page holds is left out, and the page listed.
    let mut long = form_page();
    long.shapes.push(Shape {
        paths: vec![((0..=MAX_DOCX_PAGE_POINTS).map(|i| [i as f64 % 500.0, (i / 500) as f64 % 800.0]).collect(), false)],
        ..boxed([0.0; 4])
    });
    assert_eq!(docx_checked(&[long], "Long").1, [0]);
    // More fields than a page holds.
    let mut fields = form_page();
    let field = fields.fields[0].clone();
    fields.fields = vec![field; MAX_DOCX_PAGE_SHAPES + 1];
    assert_eq!(docx_checked(&[fields], "Fields").1, [0]);
    // A flowing page draws no shapes, so it is never listed.
    let mut text = page();
    text.shapes = vec![Shape { paths: (0..MAX_DOCX_PAGE_SHAPES + 1).map(tick).collect(), ..boxed([0.0; 4]) }];
    assert!(docx_checked(&[text], "Report").1.is_empty());
}

/// A picture the PDF draws mirrored (a print driver's bottom-up bitmap, drawn with a negative
/// height) is flipped back in Word and PowerPoint: a scanned form drawn in 70 such strips came
/// out in Word with every strip upside down, as if cut apart and pasted back wrongly. The HTML
/// writer embeds a picture's bytes as they are: the engine bakes the flip into its pixels.
#[test]
fn a_mirrored_picture_is_flipped_back_in_every_format() {
    let flipped = |mut p: Page| {
        p.images.iter_mut().for_each(|im| im.flip = true);
        p
    };
    let flips = |p: Page| {
        let doc = word(&docx(&[p], "Scan"));
        pictures(&doc).iter().map(|f| (f.wrap == Wrap::Inline, f.flip_h, f.flip_v, f.rot)).collect::<Vec<_>>()
    };
    // Flowing Word: an inline picture.
    assert_eq!(flips(flipped(page())), [(true, false, true, 0.0)]);
    assert_eq!(flips(page()), [(true, false, false, 0.0)], "an upright picture stays as it is");
    // Positioned Word: an anchored picture, flipped in its frame before it is rotated.
    let mut form = form_page();
    form.images = page().images;
    assert_eq!(flips(flipped(form.clone())), [(false, false, true, 0.0)]);
    assert_eq!(flips(form), [(false, false, false, 0.0)], "an upright picture stays as it is");
    // PowerPoint.
    assert!(part(&pptx(&[flipped(page())], "Scan"), "ppt/slides/slide1.xml").contains("flipV=\"1\""));
    assert!(!part(&pptx(&[page()], "Scan"), "ppt/slides/slide1.xml").contains("flipV"));
    // HTML shows the bytes it is given, with no transform of its own in either mode.
    for out in [html(&[flipped(page())], "Scan"), html_simple(&[flipped(page())], "Scan")] {
        assert!(out.contains("<img ") && !out.contains("transform"), "{out}");
    }
}

/// The writers that place pictures upright (flowing Word pages, PowerPoint) compose a mirror
/// with a half turn: a left-right mirror (`-w 0 0 h`, mirrored and turned 180°) flips
/// horizontally, not vertically, and an upright picture turned half round flips both ways. A
/// positioned Word page rotates the picture itself, so it keeps the vertical flip and the turn.
#[test]
fn a_mirror_and_a_half_turn_compose_in_the_upright_writers() {
    let turned = |flip: bool| {
        let mut p = page();
        p.images.iter_mut().for_each(|im| {
            im.flip = flip;
            im.rotation = 180.0;
        });
        p
    };
    let flips = |p: Page| {
        let doc = word(&docx(&[p], "Scan"));
        pictures(&doc).iter().map(|f| (f.flip_h, f.flip_v, f.rot)).collect::<Vec<_>>()
    };
    let slide = |p: Page| part(&pptx(&[p], "Scan"), "ppt/slides/slide1.xml");
    // A left-right mirror.
    assert_eq!(flips(turned(true)), [(true, false, 0.0)]);
    assert!(slide(turned(true)).contains("flipH=\"1\"") && !slide(turned(true)).contains("flipV"));
    // Upright, turned half round.
    assert_eq!(flips(turned(false)), [(true, true, 0.0)]);
    assert!(slide(turned(false)).contains("flipH=\"1\"") && slide(turned(false)).contains("flipV=\"1\""));
    // A positioned page turns the picture: a vertical flip before the half turn.
    let mut form = form_page();
    form.images = turned(true).images;
    assert_eq!(flips(form), [(false, true, 180.0)]);
}

/// A filled multi-line text field keeps its lines in Word: a line ends at a break element, as
/// CR LF, CR or LF end one in the PDF's value; a one-line field's value is written as it is.
#[test]
fn a_multiline_fields_value_keeps_its_lines() {
    let mut p = form_page();
    p.fields[2].kind = FieldKind::Text { value: "first\r\nsecond\rthird\nfourth & more".into(), multiline: true, comb: None, max_len: None };
    let xml = part(&docx(&[p], "Claim"), "word/document.xml");
    assert!(
        xml.contains("<w:t xml:space=\"preserve\">first</w:t><w:br/><w:t xml:space=\"preserve\">second</w:t><w:br/><w:t xml:space=\"preserve\">third</w:t><w:br/><w:t xml:space=\"preserve\">fourth &amp; more</w:t>"),
        "{xml}"
    );
    assert!(xml.contains("<w:t xml:space=\"preserve\">10115</w:t>"), "one-line fields are unchanged");
}

/// A field's own border and fill (what a PDF viewer draws for its widget) are drawn behind the
/// text but above the page's pictures: a scanned form's check boxes, whose thin scanned lines
/// the fields' solid boxes cover in the PDF, lost their top line in Word without them. They
/// stack above every picture and path, with the border inside the field's box.
#[test]
fn word_draws_a_fields_border_and_fill_over_the_pictures() {
    let mut p = form_page();
    p.images = page().images;
    p.fields[3].border = Some(Stroke { color: [0.0; 3], width: 1.0, dashed: false });
    p.fields[3].background = Some([1.0; 3]);
    p.fields[0].border = Some(Stroke { color: [1.0, 0.0, 0.0], width: 2.0, dashed: true });
    let doc = word(&docx(&[p], "Scan"));
    let art = drawn(&doc);
    // Only fields with a border or fill get a box: two after the page's own eleven shapes.
    assert_eq!(art.len(), 13, "{art:?}");
    let (dashed, check) = (&art[11], &art[12]);
    // The check box [60, 150, 70, 160]: its 1 pt border half a point inside, on an 842 pt page.
    assert!(close(check.rect, [60.5, 682.5, 9.0, 9.0]), "{check:?}");
    assert!(check.fill == Some(Rgb(255, 255, 255)) && check.stroke == Some(Rgb(0, 0, 0)) && (check.width - 1.0).abs() < 0.001, "{check:?}");
    assert!(dashed.fill.is_none() && dashed.stroke == Some(Rgb(255, 0, 0)) && (dashed.width - 2.0).abs() < 0.001 && dashed.dashed, "{dashed:?}");
    // Stacked above the picture and the page's paths, behind the text.
    let picture = pictures(&doc)[0].z;
    assert!([dashed, check].iter().all(|look| look.behind && look.z > picture && art[..11].iter().all(|s| s.z < look.z)), "{art:?}");
    assert!(text_boxes(&doc).iter().all(|(_, _, z)| *z > check.z));
}

/// Paths the reader left unread make a page cut short only where Word keeps its layout: a
/// flowing page draws no shapes, so it lost nothing and is not listed (it was, and Office export
/// then warned that its text and images were missing).
#[test]
fn unread_paths_cut_a_page_short_only_where_word_keeps_its_layout() {
    let form = Page { art_truncated: true, ..form_page() };
    assert_eq!(docx_checked(&[form], "Form").1, [0]);
    let mut text = page();
    text.images.clear();
    let text = Page { art_truncated: true, ..text };
    assert!(docx_checked(&[text], "Report").1.is_empty());
}

/// A list field with more items than a Word drop-down holds keeps the first ones, and its page
/// is listed as cut short: the rest were left out without a word.
#[test]
fn a_list_past_the_drop_down_limit_cuts_its_page_short() {
    let list = |n: usize| {
        let mut p = form_page();
        p.fields.retain(|f| matches!(f.kind, FieldKind::Choice { .. }));
        let options: Vec<String> = (0..n).map(|i| format!("Option {i}")).collect();
        if let Some(f) = p.fields.first_mut() {
            f.kind = FieldKind::Choice { value: String::new(), options, editable: false };
        }
        p
    };
    let (zip, cut) = docx_checked(&[list(MAX_DOCX_LIST_ITEMS)], "List");
    assert!(cut.is_empty());
    assert_eq!(part(&zip, "word/document.xml").matches("<w:listItem ").count(), MAX_DOCX_LIST_ITEMS);
    let (zip, cut) = docx_checked(&[list(MAX_DOCX_LIST_ITEMS + 1)], "List");
    assert_eq!(cut, [0]);
    assert_eq!(part(&zip, "word/document.xml").matches("<w:listItem ").count(), MAX_DOCX_LIST_ITEMS);
}

/// Word has no control that lets only one of several boxes, each in its own place, be on, so a
/// radio button is a locked check box at its place whose title explains its group; a
/// group of one button, or a check box, keeps its ordinary name.
#[test]
fn word_titles_a_radio_button_with_its_groups_one_choice() {
    let button = |name: &str, x: f64, checked: bool, buttons: usize| Field {
        name: name.into(),
        rect: [x, 150.0, x + 10.0, 160.0],
        size: 0.0,
        font: "ZapfDingbats".into(),
        read_only: false,
        color: [0.0; 3],
        alignment: 0,
        round: false,
        kind: FieldKind::Radio { checked, buttons },
        border: None,
        background: None,
    };
    let mut p = form_page();
    p.fields = vec![button("Pay", 60.0, true, 2), button("Pay", 90.0, false, 2), button("Alone", 120.0, false, 1)];
    let xml = part(&docx(&[p], "Radio"), "word/document.xml");
    assert_eq!(xml.matches("<w:alias w:val=\"Pay (read-only radio group, 2 options)\"/><w:tag w:val=\"Pay\"/>").count(), 2, "{xml}");
    assert!(xml.contains("<w:alias w:val=\"Alone\"/><w:tag w:val=\"Alone\"/>"), "{xml}");
    assert!(xml.contains("<w14:checked w14:val=\"1\"/>") && xml.contains("<w14:checked w14:val=\"0\"/>"), "{xml}");
    // The X is Arial's, not the ZapfDingbats the PDF's buttons name.
    assert!(!xml.contains("Zapf"), "{xml}");
    let check = part(&docx(&[form_page()], "Check"), "word/document.xml");
    assert!(check.contains("<w:alias w:val=\"Yes\"/>") && !check.contains("choose 1"), "{check}");
}

/// A text or list field's text is set in the family of the font its `/DA` names (it was always
/// Arial, with other widths and maybe missing glyphs); a field naming none, or a symbol font,
/// keeps Arial.
#[test]
fn word_sets_field_text_in_the_fields_font() {
    let mut p = form_page();
    let font = |p: &mut Page, name: &str, font: &str| {
        if let Some(f) = p.fields.iter_mut().find(|f| f.name == name) {
            f.font = font.into();
        }
    };
    font(&mut p, "Name", "CourierNewPSMT");
    font(&mut p, "Postcode", "ZapfDingbats");
    font(&mut p, "Fund", "Times-Roman");
    font(&mut p, "Yes", "Courier");
    let xml = part(&docx(&[p], "Fonts"), "word/document.xml");
    let family = |name: &str| {
        let at = xml.find(&format!("<w:tag w:val=\"{name}\"/>")).unwrap_or_else(|| panic!("{name}: {xml}"));
        let start = xml[..at].rfind("<w:sdtPr>").unwrap();
        xml[start..].split("w:ascii=\"").nth(1).unwrap().split('"').next().unwrap().to_string()
    };
    assert_eq!(family("Name"), "Courier New");
    assert_eq!(family("Postcode"), "Courier New", "comb values use fixed advances, including replacements");
    assert_eq!(family("Fund"), "Times New Roman");
    assert_eq!(family("Notes"), "Arial", "a field naming no font");
    assert_eq!(family("Yes"), "Arial", "a check box's X is Arial's");
}

/// A read-only field's control is locked, so Word lets its content be read and copied, not
/// changed; a combo box that takes typed text is a Word combo box, which does too, and a plain
/// list stays a drop-down.
#[test]
fn word_locks_read_only_fields_and_lets_an_editable_combo_take_text() {
    let mut p = form_page();
    for f in p.fields.iter_mut() {
        match f.name.as_str() {
            "Name" | "Yes" => f.read_only = true,
            "Fund" => {
                if let FieldKind::Choice { editable, .. } = &mut f.kind {
                    *editable = true;
                }
            }
            _ => {}
        }
    }
    let xml = part(&docx(&[p], "Locks"), "word/document.xml");
    assert_eq!(xml.matches("<w:lock w:val=\"contentLocked\"/>").count(), 2, "{xml}");
    assert!(xml.contains("<w:tag w:val=\"Name\"/><w:lock w:val=\"contentLocked\"/>"), "{xml}");
    let check = xml.split("<w:tag w:val=\"Yes\"/>").nth(1).unwrap().split("</w:sdtPr>").next().unwrap();
    assert!(check.contains("<w:lock w:val=\"contentLocked\"/>") && check.contains("<w14:checkbox>"), "{check}");
    assert!(xml.contains("<w:comboBox><w:listItem w:displayText=\"Health\"") && !xml.contains("dropDownList"), "{xml}");
    let plain = part(&docx(&[form_page()], "Plain"), "word/document.xml");
    assert!(!plain.contains("<w:lock ") && plain.contains("<w:dropDownList>") && !plain.contains("comboBox"), "{plain}");
}

/// #3657: two-column text finishes the left column before starting the right column across
/// Word, HTML, RTF, Markdown and PowerPoint, and text turned 90° exports in reading order rather
/// than last line first.
#[test]
fn two_column_and_turned_text_export_in_reading_order() {
    let para = |t: &str, x: f64, y0: f64, y1: f64, size: f64| Block {
        text: t.into(),
        rect: [x, y0, x + 130.0, y1],
        size,
        bold: false,
        italic: false,
        color: [0.0; 3],
        rotation: 0.0,
        space_around: [false; 2],
        font: String::new(),
    };
    let columns = Page {
        width: 595.0,
        height: 842.0,
        blocks: vec![
            para("Two columns", 50.0, 790.0, 808.0, 18.0),
            para("A1 first paragraph A2 first paragraph A3 first paragraph", 50.0, 720.0, 762.0, 12.0),
            para("B1 second paragraph B2 second paragraph", 50.0, 675.0, 702.0, 12.0),
            para("B3 second paragraph B4 second paragraph", 320.0, 735.0, 762.0, 12.0),
            para("C1 third paragraph C2 third paragraph", 320.0, 695.0, 722.0, 12.0),
        ],
        images: Vec::new(),
        rules: Vec::new(),
        background: None,
        ..Page::default()
    };
    let expected_cols = ["Two columns", "A1 first paragraph", "B1 second paragraph", "B3 second paragraph", "C1 third paragraph"];
    for (fmt, out) in [
        ("html", html(std::slice::from_ref(&columns), "Columns")),
        ("docx", part(&docx(std::slice::from_ref(&columns), "Columns"), "word/document.xml")),
        ("rtf", rtf(std::slice::from_ref(&columns))),
        ("md", markdown(std::slice::from_ref(&columns))),
        ("pptx", part(&pptx(std::slice::from_ref(&columns), "Columns"), "ppt/slides/slide1.xml")),
    ] {
        let positions: Vec<usize> = expected_cols.iter().map(|s| out.find(s).unwrap_or_else(|| panic!("{s} missing in {fmt}: {out}"))).collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{fmt} out of order: {positions:?}\n{out}");
    }

    // Text turned 90 degrees clockwise (rotation = -90°): lines run top to bottom (y = 540
    // down to 468) and follow each other right to left (x = 676, 654.4, 632.8, 611.2).
    let turned_line = |t: &str, x: f64, size: f64| Block {
        text: t.into(),
        rect: [x - size, 468.0, x, 540.0],
        size,
        bold: false,
        italic: false,
        color: [0.0; 3],
        rotation: -90.0,
        space_around: [false; 2],
        font: String::new(),
    };
    let rotated = Page {
        width: 792.0,
        height: 612.0,
        blocks: vec![
            turned_line("Line one", 676.0, 20.0),
            turned_line("Line two", 654.4, 18.0),
            turned_line("Line three", 632.8, 18.0),
            turned_line("Line four", 611.2, 18.0),
        ],
        images: Vec::new(),
        rules: Vec::new(),
        background: None,
        ..Page::default()
    };
    let expected_lines = ["Line one", "Line two", "Line three", "Line four"];
    for (fmt, out) in [
        ("html", html(std::slice::from_ref(&rotated), "Rotated")),
        ("docx", part(&docx(std::slice::from_ref(&rotated), "Rotated"), "word/document.xml")),
        ("rtf", rtf(std::slice::from_ref(&rotated))),
        ("md", markdown(std::slice::from_ref(&rotated))),
        ("pptx", part(&pptx(std::slice::from_ref(&rotated), "Rotated"), "ppt/slides/slide1.xml")),
    ] {
        let positions: Vec<usize> = expected_lines.iter().map(|s| out.find(s).unwrap_or_else(|| panic!("{s} missing in {fmt}: {out}"))).collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{fmt} out of order: {positions:?}\n{out}");
    }

    // The same lines, stored bottom-to-top. Rotated lines do not join one paragraph, and
    // Word's positioned boxes must follow reading order rather than the file's block order.
    let mut shuffled = rotated.clone();
    shuffled.blocks.reverse();
    for (fmt, out) in [
        ("html", html(std::slice::from_ref(&shuffled), "Rotated")),
        ("docx", part(&docx(std::slice::from_ref(&shuffled), "Rotated"), "word/document.xml")),
        ("rtf", rtf(std::slice::from_ref(&shuffled))),
        ("md", markdown(std::slice::from_ref(&shuffled))),
        ("pptx", part(&pptx(std::slice::from_ref(&shuffled), "Rotated"), "ppt/slides/slide1.xml")),
    ] {
        let positions: Vec<usize> = expected_lines.iter().map(|s| out.find(s).unwrap_or_else(|| panic!("{s} missing in {fmt}: {out}"))).collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{fmt} shuffled out of order: {positions:?}\n{out}");
    }
}

/// #5149: two columns of two rows are a table when ruled lines draw its grid, and the docx keeps
/// its borders; the same text without rules (or with rules that only underline it) stays paragraphs.
#[test]
fn a_ruled_two_by_two_grid_is_a_table() {
    let blocks = vec![
        block("Name", 25.0, 250.0, 55.0, 261.0),
        block("Value", 165.0, 250.0, 195.0, 261.0),
        block("Alpha", 25.0, 225.0, 55.0, 236.0),
        block("One", 165.0, 225.0, 185.0, 236.0),
    ];
    let grid = vec![
        [20.0, 270.0, 300.0, 270.0],
        [20.0, 245.0, 300.0, 245.0],
        [20.0, 220.0, 300.0, 220.0],
        [20.0, 220.0, 20.0, 270.0],
        [160.0, 220.0, 160.0, 270.0],
        [300.0, 220.0, 300.0, 270.0],
    ];
    let (found, consumed) = tables(&blocks, &grid);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].ruled, Some([true; 4]), "the grid's frame is ruled on every side");
    assert_eq!(found[0].rows.len(), 2);
    assert!(consumed.iter().all(|used| *used));
    let p = Page { width: 320.0, height: 300.0, blocks: blocks.clone(), rules: grid, ..Page::default() };
    let xml = part(&docx(std::slice::from_ref(&p), "Grid"), "word/document.xml");
    assert_eq!(xml.matches("<w:tbl>").count(), 1, "{xml}");
    assert!(xml.contains("w:val=\"single\"") && !xml.contains("w:val=\"nil\""), "{xml}");

    assert!(tables(&blocks, &[]).0.is_empty(), "alignment alone does not make two by two a table");
    let underlines = [[20.0, 245.0, 300.0, 245.0]];
    assert!(tables(&blocks, &underlines).0.is_empty(), "a rule between the rows alone is no grid");
}

/// A grid drawn with inside lines only gets inside borders in Word, not a box the page never
/// draws; an outer line far from the table (a page rule) is not its frame.
#[test]
fn a_ruled_table_gets_outer_borders_only_where_the_page_draws_them() {
    let blocks = vec![
        block("Name", 25.0, 250.0, 55.0, 261.0),
        block("Value", 165.0, 250.0, 195.0, 261.0),
        block("Alpha", 25.0, 225.0, 55.0, 236.0),
        block("One", 165.0, 225.0, 185.0, 236.0),
    ];
    let inside = vec![[20.0, 245.0, 300.0, 245.0], [160.0, 220.0, 160.0, 270.0]];
    let (found, _) = tables(&blocks, &inside);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].ruled, Some([false; 4]));
    let borders = crate::as_word::table(&found[0], true).props.borders.unwrap();
    let single = |b: Option<craft_wordmodel::props::Border>| b.is_some_and(|b| b.style == craft_wordmodel::props::BorderStyle::Single);
    assert!(single(borders.between) && single(borders.inside_v), "{borders:?}");
    for edge in [borders.top, borders.left, borders.bottom, borders.right] {
        assert!(!single(edge), "{borders:?}");
    }

    let mut ruled_top = inside.clone();
    ruled_top.push([20.0, 270.0, 300.0, 270.0]);
    ruled_top.push([0.0, 500.0, 320.0, 500.0]);
    ruled_top.push([0.0, 0.0, 320.0, 0.0]);
    assert_eq!(tables(&blocks, &ruled_top).0[0].ruled, Some([true, false, false, false]), "only the top line near the table frames it");
}

/// Word accepts one family per script, not the two CSS fallback candidates. Preserve known
/// joined words in both ordinary text and field runs, including the East Asian script slot.
#[test]
fn word_preserves_joined_font_families_for_all_scripts() {
    for (base, family) in [
        ("ABCDEF+SimSun", "SimSun"),
        ("MicrosoftYaHei-Bold", "Microsoft YaHei"),
        ("DejaVuSans", "DejaVu Sans"),
        ("DejaVuSansMono-Oblique", "DejaVu Sans Mono"),
        ("DejaVuSerif", "DejaVu Serif"),
        ("MS-Gothic", "MS Gothic"),
    ] {
        let mut p = form_page();
        p.blocks[0].font = base.into();
        p.blocks[0].text = "文字".into();
        p.fields[0].font = base.into();
        let xml = part(&docx(&[p], "Fonts"), "word/document.xml");
        let fonts = format!("<w:rFonts w:ascii=\"{family}\" w:hAnsi=\"{family}\" w:eastAsia=\"{family}\" w:cs=\"{family}\"/>");
        assert!(xml.matches(&fonts).count() >= 3, "text, field default and field value keep {family}: {xml}");
    }
}

/// Exhausting a document's point budget must not turn a flattened form into flowing text or
/// hide its omission warning; a flowing page still does not report artwork it never draws.
#[test]
fn word_shape_budget_keeps_layout_and_reports_only_positioned_art() {
    let mut form = Page { fields: Vec::new(), ..form_page() };
    let mut budget = 4;
    form.limit_shape_points(&mut budget);
    assert!(form.art_framed && form.art_truncated);
    assert_eq!(form.shapes.len(), 1, "whole shapes only: the first 5-point box does not fit");
    assert_eq!(budget, 0);
    let mut second = Page { fields: Vec::new(), ..form_page() };
    second.limit_shape_points(&mut budget);
    assert!(second.shapes.is_empty());
    assert_eq!(second.shapes.capacity(), 0, "empty pages must not retain their old shape allocations");
    let (bytes, dropped) = docx_checked_with_language(&[form, second], "Forms", Some("ja-JP"));
    assert_eq!(dropped, vec![0, 1]);
    assert!(part(&bytes, "word/document.xml").contains("<v:textbox"));
    assert!(part(&bytes, "word/styles.xml").contains("<w:lang w:eastAsia=\"ja-JP\"/>"));

    let mut flowing = page();
    flowing.shapes = vec![boxed([0.0, 0.0, 10.0, 10.0])];
    flowing.limit_shape_points(&mut budget);
    assert!(flowing.art_truncated && !flowing.art_framed);
    assert!(docx_checked(&[flowing], "Flow").1.is_empty());
}

#[test]
fn office_regression_grouped_boxes_select_positioned_layout() {
    let mut p = form_page();
    p.fields.clear();
    let paths = p.shapes.iter().take(FRAMING_SHAPES).flat_map(|s| s.paths.clone()).collect();
    p.shapes = vec![Shape { paths, ..boxed([0.0; 4]) }];
    assert!(!text_boxes(&word(&docx(&[p], "Grouped boxes"))).is_empty(), "the labels keep their places");
}

#[test]
fn office_regression_retraced_rules_are_not_rectangles() {
    let mut p = form_page();
    p.shapes = vec![Shape { paths: vec![(vec![[10.0, 10.0], [30.0, 10.0], [10.0, 10.0], [10.0, 30.0]], true)], ..boxed([0.0; 4]) }];
    let art = drawn(&word(&docx(&[p], "Retraced rules")));
    assert_eq!(art.iter().map(|s| s.kind).collect::<Vec<_>>(), [ShapeKind::Freeform], "{art:?}");
}

#[test]
fn office_regression_open_paths_fill_closed_but_stroke_open() {
    let mut p = form_page();
    p.shapes = vec![Shape { paths: vec![(vec![[10.0, 10.0], [30.0, 10.0], [20.0, 30.0]], false)], fill: Some([0.5; 3]), ..boxed([0.0; 4]) }];
    let art = drawn(&word(&docx(&[p], "Open triangle")));
    assert_eq!(art.len(), 2, "{art:?}");
    let triangle = vec![[0.0, 20.0], [20.0, 20.0], [10.0, 0.0]];
    assert!(art[0].stroke.is_none() && art[0].fill.is_some());
    assert_eq!(art[0].paths, [(triangle.clone(), true)]);
    assert!(art[1].stroke.is_some() && art[1].fill.is_none());
    assert_eq!(art[1].paths, [(triangle, false)]);
    assert!(art.iter().all(|s| close(s.rect, [10.0, 812.0, 20.0, 20.0])), "{art:?}");
}

#[test]
fn office_regression_positioned_mixed_styles_use_each_line_piece_once() {
    let mut p = form_page();
    p.fields.clear();
    p.lines = vec![
        Block { size: 10.0, ..block("First", 60.0, 700.0, 85.0, 710.0) },
        Block { size: 10.0, bold: true, ..block("Bold", 87.0, 700.0, 110.0, 710.0) },
        Block { size: 10.0, italic: true, ..block("Italic", 112.0, 700.0, 140.0, 710.0) },
    ];
    let doc = word(&docx(&[p], "Pieces"));
    // One text box holds the row, each piece once, in its own style.
    assert_eq!(text_boxes(&doc).iter().map(|(text, ..)| text.as_str()).collect::<Vec<_>>(), ["First Bold Italic"]);
    let row = doc
        .parts
        .values()
        .flat_map(|part| part.blocks.iter())
        .find_map(|b| if let craft_wordmodel::Block::Para(p) = &**b { Some(p) } else { None })
        .unwrap();
    let styles: Vec<(String, bool, bool)> =
        row.run_ranges().map(|(range, props)| (row.text[range].trim().to_string(), props.bold == Some(true), props.italic == Some(true))).collect();
    assert_eq!(styles, [("First".to_string(), false, false), ("Bold".to_string(), true, false), ("Italic".to_string(), false, true)]);
}

#[test]
fn office_regression_field_formatting_applies_to_field_text() {
    let mut p = form_page();
    p.fields.truncate(1);
    p.fields[0].font = "Helvetica-BoldOblique".into();
    p.fields[0].color = [1.0, 0.0, 0.0];
    p.fields[0].alignment = 2;
    let doc = word(&docx(&[p], "Styled field"));
    let (_, paragraph) = &controls(&doc)[0];
    let run = run_of(paragraph);
    assert_eq!((run.bold, run.italic, run.color), (Some(true), Some(true), Some(craft_wordmodel::TextColor::Rgb(Rgb(255, 0, 0)))));
    assert_eq!(paragraph.props.align, Some(Align::Right));
}

#[test]
fn office_regression_round_buttons_retain_one_visible_boundary() {
    let mut p = form_page();
    p.shapes.clear();
    p.fields.retain(|f| f.name == "No");
    p.fields[0].round = true;
    p.fields[0].border = Some(Stroke { color: [0.0; 3], width: 1.0, dashed: false });
    let doc = word(&docx(&[p.clone()], "Round"));
    assert_eq!(drawn(&doc).iter().map(|s| s.kind).collect::<Vec<_>>(), [ShapeKind::Ellipse]);
    assert!(matches!(controls(&doc)[0].0.kind, ControlKind::CheckBox { checked: false, .. }));
    p.shapes = vec![Shape {
        paths: vec![(
            (0..32)
                .map(|i| {
                    let t = f64::from(i) * std::f64::consts::TAU / 32.0;
                    [95.0 + 5.0 * t.cos(), 155.0 + 5.0 * t.sin()]
                })
                .collect(),
            true,
        )],
        ..boxed([0.0; 4])
    }];
    p.fields[0].border = Some(Stroke { color: [0.0; 3], width: 1.0, dashed: false });
    let art = drawn(&word(&docx(&[p], "Already outlined")));
    assert_eq!(art.iter().map(|s| s.kind).collect::<Vec<_>>(), [ShapeKind::Freeform], "no duplicate border: {art:?}");
}

#[test]
fn review_invalid_fields_do_not_embed_hidden_values() {
    for rect in [[f64::NAN, 0.0, 100.0, 20.0], [0.0, 0.0, f64::INFINITY, 20.0], [20.0, 0.0, 10.0, 20.0]] {
        let mut p = form_page();
        p.fields.truncate(2);
        p.fields[0].rect = rect;
        p.fields[0].kind = FieldKind::Text { value: "PRIVATE OMITTED VALUE".into(), multiline: false, comb: None, max_len: None };
        let (zip, dropped) = docx_checked(&[p], "Invalid field");
        assert!(!package_text(&zip).contains("PRIVATE OMITTED VALUE"), "in no part of the package");
        assert_eq!(controls(&word(&zip)).len(), 1, "the other field is kept");
        assert_eq!(dropped, [0]);
    }
}

#[test]
fn review_direct_field_auto_size_fits_width_and_multiline_height() {
    let size = |p: &Page| run_of(&controls(&word(&docx(std::slice::from_ref(p), "Sized")))[0].1).size;
    for auto in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut p = form_page();
        p.fields.truncate(1);
        let f = &mut p.fields[0];
        f.size = auto;
        f.font = "Courier".into();
        f.rect = [20.0, 20.0, 82.0, 40.0];
        f.kind = FieldKind::Text { value: "WWWWWWWWWWWWWWWWWWWW".into(), multiline: false, comb: None, max_len: None };
        assert_eq!(size(&p), Some(5.0), "20 Courier glyphs fit at 5 pt");
        p.fields[0].rect = [20.0, 20.0, 200.0, 80.0];
        p.fields[0].kind = FieldKind::Text { value: "one\ntwo\nthree".into(), multiline: true, comb: None, max_len: None };
        assert_eq!(size(&p), Some(12.0), "three lines fit the box at the most an automatic size goes to");
        p.fields[0].size = 9.0;
        assert_eq!(size(&p), Some(9.0), "an explicit size is kept");
    }
}

#[test]
fn review_regular_family_names_do_not_imply_bold() {
    for (font, bold) in [("BlackadderITC-Regular", false), ("HeavyMetal-Regular", false), ("Arial-Black", true), ("Helvetica-BoldOblique", true)] {
        let mut p = form_page();
        p.fields.truncate(1);
        p.fields[0].font = font.into();
        let doc = word(&docx(&[p], "Weight"));
        assert_eq!(run_of(&controls(&doc)[0].1).bold == Some(true), bold, "{font}");
    }
}

#[test]
fn review_paths_and_pictures_keep_both_paint_orders() {
    for image_first in [true, false] {
        let mut p = form_page();
        p.shapes.truncate(1);
        p.fields.clear();
        p.images = page().images;
        p.art_framed = true;
        p.paint_order = if image_first { vec![Paint::Image(0), Paint::Shape(0)] } else { vec![Paint::Shape(0), Paint::Image(0)] };
        let doc = word(&docx(&[p], "Paint order"));
        let (image, path) = (pictures(&doc)[0], &drawn(&doc)[0]);
        assert_eq!(image.z < path.z, image_first, "what the PDF paints first is furthest back");
        assert!(image.wrap == Wrap::BehindText && path.behind, "both behind the text");
    }
}

#[test]
fn review_point_limits_remap_shared_paint_order() {
    let mut p = form_page();
    p.shapes = vec![boxed([0.0, 0.0, 10.0, 10.0]), Shape { paths: vec![(vec![[0.0, 0.0], [10.0, 10.0]], false)], ..boxed([0.0; 4]) }];
    p.paint_order = vec![Paint::Shape(0), Paint::Image(0), Paint::Shape(1)];
    p.limit_shape_points(&mut 2);
    assert_eq!(p.paint_order, [Paint::Image(0), Paint::Shape(0)]);
    assert_eq!(p.shapes.len(), 1);
    assert!(p.art_truncated);
}

#[test]
fn review_repeated_choices_and_checks_keep_their_values() {
    for kind in [
        FieldKind::Check { checked: true, on_state: "Yes".into() },
        FieldKind::Choice { value: "First".into(), options: vec!["First".into(), "Second".into()], editable: false },
    ] {
        let mut p = form_page();
        p.fields.truncate(1);
        p.fields[0].kind = kind;
        p.fields.push(p.fields[0].clone());
        p.fields[1].rect = [20.0, 20.0, 100.0, 40.0];
        let doc = word(&docx(&[p], "Repeated controls"));
        // Each widget of the field is a control of its own, tagged alike and showing the value.
        let both = controls(&doc);
        assert_eq!(both.len(), 2);
        assert_eq!((both[0].0.tag.as_str(), &both[0].0.kind, text_of(&both[0].1)), (both[1].0.tag.as_str(), &both[1].0.kind, text_of(&both[1].1)));
        assert!(matches!(text_of(&both[0].1).as_str(), "X" | "First"));
    }
}

#[test]
fn review_radio_groups_keep_their_state_locked_and_report_lost_editability() {
    let mut p = form_page();
    p.fields.truncate(2);
    p.fields[0].kind = FieldKind::Radio { checked: true, buttons: 2 };
    p.fields[1].kind = FieldKind::Radio { checked: false, buttons: 2 };
    let (zip, dropped) = docx_checked(&[p.clone()], "Radio group");
    let xml = part(&zip, "word/document.xml");
    assert_eq!(dropped, [0]);
    assert_eq!(xml.matches("<w:lock w:val=\"contentLocked\"/>").count(), 2);
    assert_eq!(xml.matches("<w14:checked w14:val=\"1\"/>").count(), 1);
    assert_eq!(xml.matches("<w14:checked w14:val=\"0\"/>").count(), 1);
    assert!(xml.contains("read-only radio group"));
    for f in &mut p.fields {
        f.read_only = true;
    }
    assert!(docx_checked(&[p], "Already read-only").1.is_empty());
}

#[test]
fn review_only_emitted_closed_artwork_suppresses_a_field_outline() {
    for (closed, omitted) in [(false, false), (true, false), (true, true)] {
        let mut p = form_page();
        p.fields.retain(|f| f.name == "No");
        p.fields[0].round = true;
        p.fields[0].border = Some(Stroke { color: [0.0; 3], width: 1.0, dashed: false });
        let points = vec![[95.0, 150.0], [100.0, 155.0], [95.0, 160.0], [90.0, 155.0]];
        p.shapes = vec![Shape { paths: vec![(points, closed)], ..boxed([0.0; 4]) }];
        if omitted {
            p.shapes.insert(0, Shape { paths: vec![(vec![[0.0, 0.0], [1.0, 1.0]], false); MAX_DOCX_PAGE_SHAPES], ..boxed([0.0; 4]) });
        }
        let xml = part(&docx(&[p], "Outline"), "word/document.xml");
        assert!(xml.contains("<a:prstGeom prst=\"ellipse\">"));
    }
}

#[test]
fn office_regression_repeated_text_widgets_each_keep_the_value() {
    let mut p = form_page();
    p.fields.truncate(1);
    p.fields[0].name = format!("{}A", "x".repeat(MAX_FIELD_NAME_CHARS));
    p.fields[0].kind = FieldKind::Text { value: "a<&\rb".into(), multiline: true, comb: None, max_len: None };
    let mut second = p.clone();
    second.fields.push(Field { name: format!("{}B", "x".repeat(MAX_FIELD_NAME_CHARS)), ..p.fields[0].clone() });
    let doc = word(&docx(&[p, second], "Repeated"));
    let all = controls(&doc);
    // Three widgets on two pages: each is a control showing the value, its line ending a break.
    assert_eq!(all.len(), 3);
    assert!(all.iter().all(|(_, paragraph)| text_of(paragraph) == "a<&\nb"), "{all:?}");
    assert!(all.iter().all(|(control, _)| control.tag == "x".repeat(MAX_FIELD_NAME_CHARS)), "the displayed tag is cut to its limit");
}

#[test]
fn office_regression_comb_values_use_constant_width_advances() {
    let mut p = form_page();
    p.fields.retain(|f| f.name == "Postcode");
    p.fields[0].font = "Helvetica".into();
    p.fields[0].kind = FieldKind::Text { value: "WiWiW".into(), multiline: false, comb: Some(5), max_len: Some(5) };
    let doc = word(&docx(&[p], "Comb"));
    let (_, paragraph) = &controls(&doc)[0];
    let run = run_of(paragraph);
    // Each 20 pt cell = Courier's 6 pt advance + 14 pt character spacing, the text starting
    // half that spacing in; the frame reaches one cell past the 100 pt box.
    assert_eq!((run.font.as_deref(), run.spacing), (Some("Courier New"), Some(14.0)));
    assert_eq!(paragraph.props.indent_left, Some(7.0));
    assert_eq!(paragraph.props.frame.as_ref().and_then(|f| f.w), Some(120.0));
}

/// A line of 10 pt text from `x0` to `x1` whose box starts at `y0`: 12 pt tall, so lines 14 pt
/// apart leave 2 pt between their boxes.
fn line(t: &str, x0: f64, x1: f64, y0: f64) -> Block {
    Block { size: 10.0, ..block(t, x0, y0, x1, y0 + 12.0) }
}

/// The text of each flowing paragraph Word gets for a page of `blocks`.
fn word_texts(blocks: Vec<Block>) -> Vec<String> {
    let p = Page { width: 612.0, height: 792.0, blocks, ..Page::default() };
    let xml = part(&docx(&[p], "Lines"), "word/document.xml");
    word_paragraphs(&xml).into_iter().map(|runs| runs.into_iter().map(|(t, _)| t).collect()).collect()
}

/// A sentence over three lines, as the reading blocks give it: split at each style change, a
/// bold name in the first line and a slanted name in the second whose box leans 2.5 pt into the
/// comma after it (#4158). The next paragraph follows after paragraph space.
fn wrapped_sentence() -> Vec<Block> {
    vec![
        line("The parties to this lease are the owner,", 72.0, 262.0, 700.0),
        Block { bold: true, ..line("Harbour Lane Properties", 265.0, 400.0, 700.0) },
        line(", of 4 Quay Street, and the", 400.5, 540.0, 700.0),
        line("tenant named in the schedule,", 72.0, 215.0, 686.0),
        Block { italic: true, ..line("Blue Finch Studio", 218.0, 312.0, 686.0) },
        line(", who agree as follows to rent the upper", 309.5, 538.0, 686.0),
        line("rooms for the whole of the term.", 72.0, 230.0, 672.0),
        line("Rent is paid monthly in advance.", 72.0, 230.0, 640.0),
    ]
}

/// #4158: a sentence that wraps onto the next line after a change of style is one paragraph in
/// every format, its lines joined by a word space.
#[test]
fn a_sentence_that_wraps_after_a_style_change_is_one_paragraph() {
    let sentence = "The parties to this lease are the owner, Harbour Lane Properties, of 4 Quay Street, and the tenant named in the \
schedule, Blue Finch Studio, who agree as follows to rent the upper rooms for the whole of the term.";
    assert_eq!(word_texts(wrapped_sentence()), [sentence, "Rent is paid monthly in advance."]);
    let p = Page { width: 612.0, height: 792.0, blocks: wrapped_sentence(), ..Page::default() };
    let h = html(std::slice::from_ref(&p), "Lease");
    assert_eq!(h.matches("<p>").count(), 2, "{h}");
    assert!(
        h.contains(
            "<p>The parties to this lease are the owner, <strong>Harbour Lane Properties</strong>, of 4 Quay Street, and the tenant \
named in the schedule, <em>Blue Finch Studio</em>, who agree as follows to rent the upper rooms for the whole of the term.</p>"
        ),
        "{h}"
    );
    let r = rtf(std::slice::from_ref(&p));
    assert_eq!(rtf_paras(&r).len(), 2, "{r}");
    let md = markdown(std::slice::from_ref(&p));
    assert_eq!(md.trim().split("\n\n").count(), 2, "{md}");
    assert!(md.contains("of 4 Quay Street, and the tenant named in the schedule, "), "{md}");
    // On a slide the last of the paragraphs set largest is the title; the sentence is one paragraph.
    let s = part(&pptx(std::slice::from_ref(&p), "Lease"), "ppt/slides/slide1.xml");
    let slide: Vec<String> =
        s.split("<a:p>").skip(1).map(|p| p.split("<a:t>").skip(1).filter_map(|t| t.split_once("</a:t>").map(|(t, _)| t)).collect()).collect();
    assert!(slide.iter().any(|p| p == sentence), "{s}");
}

/// A slanted word's box leans into the word after it by about a quarter of its size, so that
/// word still runs on from it; text that starts further back is drawn over the text before it,
/// not after it.
#[test]
fn a_slanted_word_that_leans_into_the_next_stays_in_its_line() {
    let leaning = vec![
        line("named in the schedule,", 72.0, 180.0, 700.0),
        Block { italic: true, ..line("Blue Finch Studio", 183.0, 276.0, 700.0) },
        line(", who agree", 273.5, 330.0, 700.0),
    ];
    assert_eq!(word_texts(leaning), ["named in the schedule, Blue Finch Studio, who agree"]);
    let over = vec![line("A label", 72.0, 120.0, 700.0), line("over another", 115.0, 180.0, 700.0)];
    assert_eq!(word_texts(over), ["A label", "over another"]);
}

/// A line runs on into the next only when it reaches the right edge of the text and the next
/// starts just below at the same left edge, in the same size. A line that ends a sentence or ends
/// short, and a next line that is a heading, a list item, indented or after paragraph space, keep
/// their paragraph break. Each case is a page of its own.
#[test]
fn a_paragraph_breaks_where_its_line_does_not_run_on() {
    let full = "The tenant keeps the rooms, the stairs and the shared yard clean and in good repair";
    let next = "throughout the term of the lease";
    let first = |t: &str| line(t, 72.0, 540.0, 700.0);
    let second = |t: &str| line(t, 72.0, 240.0, 686.0);
    assert_eq!(word_texts(vec![first(full), second(next)]), [format!("{full} {next}")]);
    assert_eq!(word_texts(vec![first("Изпълнителят поддържа сървърите и мрежата на"), second("възложителя в работно състояние")]).len(), 1);
    // Chinese and Japanese wrap between characters: their lines join without a space. Korean
    // spaces its words, so its lines join with one.
    let zh = ("本合同自双方签字之日起生效，有效期", "为一年，期满前一个月双方协商续签事宜");
    assert_eq!(word_texts(vec![first(zh.0), second(zh.1)]), [format!("{}{}", zh.0, zh.1)]);
    assert_eq!(
        word_texts(vec![first("この契約は双方が署名した日から"), second("一年間有効とする")]),
        ["この契約は双方が署名した日から一年間有効とする"]
    );
    assert_eq!(word_texts(vec![first("임차인은 방과 계단을 깨끗하게"), second("유지합니다")]), ["임차인은 방과 계단을 깨끗하게 유지합니다"]);
    let heading = |b: Block| Block { bold: true, ..b };
    for (why, blocks) in [
        ("a sentence ends", vec![first(&format!("{full}.")), second(next)]),
        ("a sentence ends in quotes", vec![first("Страните се договориха „за срок от една година.“"), second(next)]),
        ("a Chinese sentence ends", vec![first("本合同自双方签字之日起生效。"), second(zh.1)]),
        ("a Japanese sentence ends in quotes", vec![first("「この契約は一年間有効ですか？」"), second("双方が署名した日から")]),
        ("the line ends short", vec![line(next, 72.0, 240.0, 700.0), line(full, 72.0, 540.0, 686.0)]),
        ("paragraph space", vec![first(full), line(next, 72.0, 240.0, 676.0)]),
        ("an indent", vec![first(full), line(next, 90.0, 258.0, 686.0)]),
        ("a larger size", vec![first(full), Block { size: 12.0, ..second(next) }]),
        ("a numbered item", vec![first(full), second("2. The tenant pays the rent")]),
        ("a lettered item", vec![first(full), second("а) поддръжка на сървърите")]),
        ("a bulleted item", vec![first(full), second("• the stairs")]),
        ("a heading below", vec![first(full), heading(second("Rent and deposit"))]),
        ("a heading above", vec![heading(first("Obligations of the tenant")), second(next)]),
    ] {
        assert_eq!(word_texts(blocks).len(), 2, "{why}");
    }
}

/// Two columns of text, the right one set half a line lower: a full line in one column never runs
/// on into the line of the other just below it, though the right column's lines reach the page's
/// right edge and the left column's start at its left.
#[test]
fn a_line_never_runs_on_into_the_other_column() {
    let left = ["The tenant keeps the rooms and the", "stairs clean and in good repair for", "the whole of the term of the lease"];
    let right = ["The landlord keeps the roof and the", "outer walls sound and dry for the", "whole of the term of the lease too"];
    let mut blocks = Vec::new();
    for (n, (l, r)) in left.iter().zip(right).enumerate() {
        let y = 700.0 - 14.0 * n as f64;
        blocks.push(line(l, 72.0, 290.0, y));
        blocks.push(line(r, 322.0, 540.0, y - 7.0));
    }
    let paragraphs = word_texts(blocks);
    for line in left.iter().chain(&right) {
        assert!(paragraphs.iter().any(|p| p.contains(line)), "{line}: {paragraphs:?}");
    }
    for p in &paragraphs {
        assert!(!(left.iter().any(|l| p.contains(l)) && right.iter().any(|r| p.contains(r))), "{p:?} mixes the columns");
    }
}

/// A poem quoted between paragraphs keeps its lines: they stop well short of the right edge the
/// prose around it reaches, so none of them is a full line that wraps.
#[test]
fn a_poem_keeps_its_lines() {
    let poem = [
        "The kettle sings beside the door",
        "the cat is curled upon the chair",
        "and rain runs down the window glass",
        "while evening settles on the stair",
    ];
    let prose = "The tenant may read aloud in the shared garden on summer evenings, as in the verse below";
    let mut blocks = vec![line(prose, 72.0, 540.0, 720.0)];
    for (n, (t, end)) in poem.iter().zip([236.0, 230.0, 248.0, 232.0]).enumerate() {
        blocks.push(line(t, 90.0, end, 696.0 - 14.0 * n as f64));
    }
    let mut expected = vec![prose];
    expected.extend(poem);
    assert_eq!(word_texts(blocks), expected);
}

#[test]
fn list_items_are_told_from_text() {
    for item in ["• item", "\u{F0B7} item", "- item", "\u{2013} item", "* item", "1. Term", "12) Rent", "3.", "a) rooms", "а) стаи"] {
        assert!(list_item(item), "{item}");
    }
    for text in ["1.5 kg of", "2026. Year", "i.e. this", "т.е. така", "38 rooms", "Rent 12)", "1234) many", ""] {
        assert!(!list_item(text), "{text}");
    }
}

/// Wrapped lines join on hostile boxes and sizes without a panic, and never lose a line's text.
#[test]
fn joining_lines_survives_hostile_boxes() {
    let mut blocks = Vec::new();
    for (i, v) in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 0.0, 1e300].into_iter().enumerate() {
        let y = 700.0 - 14.0 * i as f64;
        blocks.push(Block { size: v, ..line(&format!("size {i}"), 72.0, 540.0, y) });
        blocks.push(Block { rect: [v, y, 540.0, y + 12.0], ..line(&format!("left {i}"), 72.0, 540.0, y) });
        blocks.push(Block { rect: [72.0, v, v, y + 12.0], ..line(&format!("box {i}"), 72.0, 540.0, y) });
        blocks.push(Block { rotation: v, ..line(&format!("turn {i}"), 72.0, 540.0, y) });
    }
    let p = Page { width: 612.0, height: 792.0, blocks, ..Page::default() };
    let outputs = [
        html(std::slice::from_ref(&p), "Hostile"),
        rtf(std::slice::from_ref(&p)),
        part(&pptx(std::slice::from_ref(&p), "Hostile"), "ppt/slides/slide1.xml"),
        word_texts(p.blocks.clone()).concat(),
    ];
    for output in outputs {
        for b in &p.blocks {
            assert!(output.contains(&b.text), "{} was lost", b.text);
        }
    }
}

/// A bulleted list as Word and LibreOffice draw one: each bullet a block of its own in a symbol
/// font, a tab stop before its item's text, between a lead-in line and the next paragraph.
fn bulleted_list(bullet: &str) -> Vec<Block> {
    let mut blocks = vec![line("The tenant looks after:", 72.0, 180.0, 700.0)];
    for (n, (text, end)) in
        [("the stairs and the landing;", 230.0), ("the shared yard and its gate;", 240.0), ("the bins, every week.", 200.0)].into_iter().enumerate()
    {
        let y = 686.0 - 14.0 * n as f64;
        blocks.push(line(bullet, 90.0, 94.6, y));
        blocks.push(line(text, 108.0, end, y));
    }
    blocks.push(line("Rent is paid monthly in advance.", 72.0, 230.0, 630.0));
    blocks
}

/// A bulleted list whose bullets are blocks of their own is one paragraph per item, each item's
/// text after its bullet, in every format: not a two-column table of bullets and text. The
/// Symbol and Wingdings bullets Word writes count, and so does a dash that opens its line.
#[test]
fn a_bulleted_list_is_paragraphs_not_a_table() {
    for bullet in ["•", "\u{F0B7}", "\u{F0A7}", "\u{2013}"] {
        let p = Page { width: 612.0, height: 792.0, blocks: bulleted_list(bullet), ..Page::default() };
        let (found, _) = tables(&p.blocks, &[]);
        assert!(found.is_empty(), "{bullet}: {found:?}");
        let items = ["the stairs and the landing;", "the shared yard and its gate;", "the bins, every week."].map(|t| format!("{bullet} {t}"));
        let mut expected = vec!["The tenant looks after:".to_string()];
        expected.extend(items.iter().cloned());
        expected.push("Rent is paid monthly in advance.".into());
        assert_eq!(word_texts(p.blocks.clone()), expected, "{bullet}");
        let h = html(std::slice::from_ref(&p), "List");
        assert!(!h.contains("<table>") && items.iter().all(|i| h.contains(&format!("<p>{i}</p>"))), "{bullet}: {h}");
        assert!(!rtf(std::slice::from_ref(&p)).contains("\\trowd"), "{bullet}");
        let s = part(&pptx(std::slice::from_ref(&p), "List"), "ppt/slides/slide1.xml");
        assert!(!s.contains("<a:tbl>"), "{bullet}: {s}");
    }
}

/// A dash in a table row stands for "none": it is a cell like the others, not a bullet, so the
/// table keeps all its rows.
#[test]
fn a_dash_in_a_table_row_stays_a_cell() {
    let mut blocks = Vec::new();
    for (n, (name, first, second)) in [("Apples", "\u{2013}", "5"), ("Pears", "3", "4"), ("Plums", "2", "6")].into_iter().enumerate() {
        let y = 700.0 - 14.0 * n as f64;
        blocks.push(line(name, 72.0, 110.0, y));
        blocks.push(line(first, 200.0, 205.0, y));
        blocks.push(line(second, 225.0, 230.0, y));
    }
    let (found, consumed) = tables(&blocks, &[]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].rows.len(), 3, "{found:?}");
    assert!(consumed.iter().all(|used| *used), "{consumed:?}");
}

/// A bullet is never a table cell, even when its items carry a second column such as an amount:
/// every item keeps its bullet in front of its text.
#[test]
fn a_bullet_is_never_a_table_cell() {
    let mut blocks = Vec::new();
    for (n, (item, amount)) in [("apples", "5 kg"), ("pears", "4 kg"), ("plums", "2 kg")].into_iter().enumerate() {
        let y = 700.0 - 14.0 * n as f64;
        blocks.push(line("•", 90.0, 94.6, y));
        blocks.push(line(item, 108.0, 140.0, y));
        blocks.push(line(amount, 300.0, 325.0, y));
    }
    let (found, _) = tables(&blocks, &[]);
    assert!(found.iter().flat_map(|t| &t.rows).flatten().all(|cell| cell.text != "•"), "{found:?}");
    let paragraphs = word_texts(blocks);
    for item in ["• apples", "• pears", "• plums"] {
        assert!(paragraphs.iter().any(|p| p.starts_with(item)), "{item}: {paragraphs:?}");
    }
}

/// Every item of a long list keeps its bullet, though all the items' texts start at one tab stop.
#[test]
fn a_long_bulleted_list_keeps_every_bullet_with_its_item() {
    let mut blocks = Vec::new();
    for n in 0..150 {
        let y = 2200.0 - 14.0 * f64::from(n);
        blocks.push(line("\u{F0B7}", 90.0, 94.6, y));
        blocks.push(line(&format!("item {n};"), 108.0, 160.0, y));
    }
    let (found, _) = tables(&blocks, &[]);
    assert!(found.is_empty(), "{found:?}");
    let paragraphs = word_texts(blocks);
    assert_eq!(paragraphs.len(), 150, "{paragraphs:?}");
    assert!(paragraphs.iter().enumerate().all(|(n, p)| *p == format!("\u{F0B7} item {n};")), "{paragraphs:?}");
}

#[test]
fn review_followup_explicit_field_sizes_and_scaled_borders_survive() {
    let mut p = form_page();
    p.fields.truncate(1);
    p.fields[0].size = 96.0;
    p.fields[0].rect = [30.0, 300.0, 500.0, 500.0];
    p.fields[0].border = Some(Stroke { color: [0.0; 3], width: 20.0, dashed: false });
    let xml = part(&docx(&[p], "Large field"), "word/document.xml");
    assert!(xml.contains("<w:sz w:val=\"192\"/>"));
    assert!(xml.contains("<a:ln w=\"254000\">"));
}

#[test]
fn review_followup_positioned_rows_follow_reading_order() {
    let mut p = form_page();
    p.blocks.clear();
    p.fields.clear();
    p.art_framed = true;
    p.lines = vec![
        line("Footer", 72.0, 150.0, 40.0),
        line("Heading", 72.0, 150.0, 700.0),
        line("Body", 72.0, 100.0, 500.0),
        Block { bold: true, ..line("bold", 104.0, 132.0, 500.0) },
    ];
    let xml = part(&docx(&[p], "Reading order"), "word/document.xml");
    assert!(xml.find(">Heading</w:t>").unwrap() < xml.find(">Body</w:t>").unwrap());
    assert!(xml.find("> bold</w:t>").unwrap() < xml.find(">Footer</w:t>").unwrap());
    assert_eq!(xml.matches("<v:textbox ").count(), 3, "mixed styles still share one row");
}

#[test]
fn review_followup_separate_font_style_suffixes() {
    for font in ["Arial,Bold-Italic", "Arial-Bold-Oblique", "Segoe-UI-Heavy-ItalicMT"] {
        assert_eq!(font_weight_and_slant(font), (true, true), "{font}");
    }
    for font in ["BlackadderITC-Regular", "Boldness-Sans-Regular", "Heavy-Family-Italic"] {
        assert!(!font_weight_and_slant(font).0, "{font}");
    }
}

#[test]
fn review_followup_repeated_checkboxes_keep_their_own_states() {
    let mut p = form_page();
    p.fields.truncate(1);
    let check = p.fields[0].clone();
    p.fields = [("Yes", true), ("Other", false), ("Yes", true)]
        .into_iter()
        .map(|(on_state, checked)| Field { kind: FieldKind::Check { checked, on_state: on_state.into() }, ..check.clone() })
        .collect();
    let doc = word(&docx(&[p], "Checkbox states"));
    let states: Vec<bool> = controls(&doc)
        .iter()
        .map(|(control, _)| match control.kind {
            ControlKind::CheckBox { checked, .. } => checked,
            _ => panic!("not a check box: {control:?}"),
        })
        .collect();
    assert_eq!(states, [true, false, true], "widgets with different on-states show their own");
}

#[test]
fn review_followup_cropped_fields_keep_their_full_offset_in_fixed_textboxes() {
    let mut p = form_page();
    p.shapes.clear();
    p.blocks.clear();
    p.fields.truncate(1);
    p.fields[0].rect = [-60.0, 200.0, 180.0, 250.0];
    p.fields[0].kind = FieldKind::Text { value: "ABCDEFGH".into(), multiline: false, comb: None, max_len: None };
    let doc = word(&docx(&[p.clone()], "Crop"));
    // The control stands in a text box of the field's own size, left of the page's edge, which
    // doesn't resize to its text; it has no frame for a reader to move onto the page.
    let boxes = text_boxes(&doc);
    assert_eq!(boxes.len(), 1);
    assert!(boxes[0].0.contains("ABCDEFGH") && close(boxes[0].1, [-60.0, 592.0, 240.0, 50.0]), "{boxes:?}");
    assert!(doc.parts.values().all(|part| !part.body.auto_fit));
    let all = controls(&doc);
    assert!(all.len() == 1 && all[0].1.props.frame.is_none(), "{all:?}");
    p.fields[0].kind = FieldKind::Text { value: "ABCDEFGH".into(), multiline: false, comb: Some(8), max_len: Some(8) };
    let boxes = text_boxes(&word(&docx(&[p], "Cropped comb")));
    assert!(close(boxes[0].1, [-60.0, 592.0, 270.0, 50.0]), "the last-cell allowance remains inside the text box: {boxes:?}");
}

#[test]
fn review_followup_partial_page_artwork_keeps_its_place() {
    let p = Page {
        width: 300.0,
        height: 400.0,
        art_framed: true,
        shapes: vec![boxed([50.0, 100.0, 150.0, 200.0])],
        images: vec![Image { ext: "png", bytes: vec![0].into(), rect: [200.0, 300.0, 210.0, 310.0], size: [10.0; 2], rotation: 0.0, flip: false }],
        ..Default::default()
    };
    let (bytes, dropped) = docx_checked(&[p], "Partial artwork");
    assert!(dropped.is_empty());
    let doc = word(&bytes);
    // Artwork that covers part of the page keeps its size and its offset from the page's corner.
    assert!(close(drawn(&doc)[0].rect, [50.0, 200.0, 100.0, 100.0]), "{:?}", drawn(&doc));
    let picture = pictures(&doc)[0];
    assert!((picture.x - 200.0).abs() < 0.01 && (picture.y - 90.0).abs() < 0.01, "the image keeps its original offset: {picture:?}");
}

#[test]
fn review_followup_compound_fill_work_exhaustion_is_reported() {
    let mut shape = boxed([10.0, 10.0, 100.0, 100.0]);
    shape.paths = (0..1000).map(|_| shape.paths[0].clone()).collect();
    shape.fill = Some([1.0, 0.0, 0.0]);
    shape.stroke = None;
    shape.even_odd = true;
    let page = Page { width: 200.0, height: 200.0, art_framed: true, shapes: vec![shape], ..Default::default() };
    let (bytes, dropped) = docx_checked(&[page], "Complex fill");
    assert_eq!(dropped, [0]);
    assert!(drawn(&word(&bytes)).is_empty(), "the fill is left out, not half drawn");
}

#[test]
fn review_followup_compound_fills_are_trapezoids_that_keep_the_hole() {
    for image in [false, true] {
        let mut p = form_page();
        p.fields.clear();
        p.blocks.clear();
        p.art_framed = true;
        let mut shape = boxed([10.0, 10.0, 100.0, 100.0]);
        shape.paths.extend(boxed([40.0, 40.0, 70.0, 70.0]).paths);
        shape.fill = Some([1.0, 0.0, 0.0]);
        shape.even_odd = true;
        shape.stroke = None;
        p.shapes = vec![shape];
        p.images.clear();
        if image {
            p.images.push(Image { ext: "png", bytes: vec![0].into(), rect: [0.0, 0.0, 10.0, 10.0], size: [10.0; 2], rotation: 0.0, flip: false });
        }
        let (zip, dropped) = docx_checked(&[p], "Hole");
        assert!(dropped.is_empty());
        let art = drawn(&word(&zip));
        // One freeform over the square: a band below the hole, one on each side of it, one above.
        assert_eq!(art.len(), 1, "{art:?}");
        assert!(art[0].fill == Some(Rgb(255, 0, 0)) && art[0].stroke.is_none() && close(art[0].rect, [10.0, 742.0, 90.0, 90.0]), "{art:?}");
        assert_eq!(art[0].paths.len(), 4, "{art:?}");
        assert!(art[0].paths.iter().all(|(points, closed)| *closed && points.len() == 4));
        // No band covers the hole's middle, (45, 45) from the square's corner.
        let covers = |points: &[[f32; 2]]| {
            let (x0, x1) = points.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[0]), b.max(p[0])));
            let (y0, y1) = points.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
            x0 < 45.0 && 45.0 < x1 && y0 < 45.0 && 45.0 < y1
        };
        assert!(!art[0].paths.iter().any(|(points, _)| covers(points)), "{art:?}");
    }
}

#[test]
fn shared_base64_keeps_padded_unwrapped_export_bytes() {
    for (input, expected) in
        [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")]
    {
        assert_eq!(base64(input.as_bytes()), expected);
    }
    assert_eq!(base64(&[251, 255, 255]), "+///");
    let encoded = base64(&[255; 256]);
    assert_eq!(encoded.len(), 344);
    assert!(!encoded.contains(['\r', '\n']));
    assert!(encoded.ends_with("/w=="));
}

/// Each place a picture is drawn embeds it whole in the HTML, so the places count against the
/// media limits: past them, pictures are left out instead of multiplying one into gigabytes.
#[test]
fn html_bounds_the_pictures_it_embeds() {
    let mut p = page();
    let one = p.images[0].clone();
    p.images = std::iter::repeat_n(one, MAX_MEDIA + 10).collect();
    for h in [html(std::slice::from_ref(&p), "Report"), html_simple(std::slice::from_ref(&p), "Report")] {
        assert_eq!(h.matches("<img").count(), MAX_MEDIA);
    }
}

/// RTF keeps a page's pictures, inline where they sit in the reading order (it left them out).
#[test]
fn rtf_keeps_pictures() {
    let r = rtf(&[page()]);
    assert!(r.contains(r"\pngblip"), "{r}");
    let doc = craft_rtf::doc::import(r.as_bytes()).unwrap();
    let pictures = doc
        .body
        .iter()
        .filter_map(|b| if let craft_wordmodel::Block::Para(p) = &**b { Some(p) } else { None })
        .flat_map(|p| &p.objects)
        .filter(|o| matches!(o, craft_wordmodel::InlineObject::Image { .. }))
        .count();
    assert_eq!(pictures, 1, "{r}");
}

/// Headings are Word's Heading 1 and 2 styles, which a word processor shows in its outline.
#[test]
fn rtf_headings_have_heading_styles() {
    let r = rtf(&[page()]);
    let doc = craft_rtf::doc::import(r.as_bytes()).unwrap();
    let heading = doc.body.iter().find_map(|b| match &**b {
        craft_wordmodel::Block::Para(p) if p.plain_text() == "Annual Report" => Some(p.props.style.clone()),
        _ => None,
    });
    let style = heading.flatten().and_then(|id| doc.styles.styles.iter().find(|s| s.id == id).map(|s| s.para.outline_level));
    assert_eq!(style, Some(Some(0)), "Annual Report is a level 1 heading: {r}");
}

/// The document's title goes into the RTF's information group, and PdfCraft is named as the
/// application that wrote it.
#[test]
fn rtf_names_the_title_and_pdfcraft() {
    let r = crate::rtf_with_title(&[page()], "Annual Report");
    assert_eq!(craft_rtf::doc::import(r.as_bytes()).unwrap().core.title, "Annual Report", "{r}");
    assert!(r.contains(r"{\*\generator PdfCraft;}") && !r.contains("WordCraft"), "{r}");
}

/// Two tables one under the other stay two tables: in RTF, rows that follow each other are one
/// table, so a paragraph keeps them apart.
#[test]
fn rtf_keeps_tables_that_touch_apart() {
    // Two grids on one page with nothing between them.
    let mut one = table_page();
    one.blocks.retain(|b| !b.text.starts_with("Notes"));
    let mut lower = one.blocks.clone();
    for b in &mut lower {
        b.rect[1] -= 300.0;
        b.rect[3] -= 300.0;
    }
    one.blocks.extend(lower);
    let r = rtf(std::slice::from_ref(&one));
    let (found, _) = tables(&one.blocks, &one.rules);
    assert_eq!(found.len(), 2, "the page has two grids");
    let read = craft_rtf::doc::import(r.as_bytes()).unwrap().body.iter().filter(|b| matches!(&***b, craft_wordmodel::Block::Table(_))).count();
    assert_eq!(read, 2, "{r}");
}

/// A hard line ending in the text (LF, CR or CRLF) is a line break inside its paragraph or table
/// cell, which RTF readers keep (storytold/pdfcraft#4723, by KyleKassen): they ignore a bare line
/// ending as whitespace.
#[test]
fn rtf_keeps_hard_line_breaks_in_paragraphs_and_cells() {
    let mut paragraph = page();
    paragraph.blocks.truncate(1);
    paragraph.blocks[0].text = "First\nSecond\r\nThird\rFourth".into();
    paragraph.blocks[0].italic = true;
    paragraph.images.clear();
    let r = rtf(&[paragraph]);
    let texts: Vec<String> = rtf_sections(&r).into_iter().flat_map(|s| s.1).collect();
    assert_eq!(texts, ["First\nSecond\nThird\nFourth"], "one paragraph, four lines: {r}");

    let mut table = table_page();
    table.blocks[0].text = "Meter\nnumber".into();
    table.blocks[4].text = "kWh\r\nunit".into();
    let r = rtf(&[table]);
    let back = craft_rtf::read(r.as_bytes()).unwrap();
    let cells: Vec<String> = back
        .blocks
        .iter()
        .filter_map(|b| if let craft_rtf::Block::Table(t) = b { Some(t) } else { None })
        .flat_map(|t| t.rows.iter().flatten())
        .map(|c| c.paras.iter().map(|p| p.text()).collect())
        .collect();
    assert!(cells.contains(&"Meter\nnumber".to_string()) && cells.contains(&"kWh\nunit".to_string()), "{cells:?}");
}

/// The Word export names PdfCraft as its application, its body font (Arial) in a font table that
/// says it is a sans-serif, so a reader without it draws a like face.
#[test]
fn word_names_pdfcraft_and_its_fonts() {
    let zip = docx(&[page()], "Report");
    assert!(part(&zip, "docProps/app.xml").contains("<Application>PdfCraft</Application>"));
    let fonts = part(&zip, "word/fontTable.xml");
    assert!(fonts.contains(r#"<w:font w:name="Arial"><w:family w:val="swiss"/>"#), "{fonts}");
    assert_eq!(craft_docx::read(&zip).unwrap().styles.default_chr.font.as_deref(), Some("Arial"));
}

/// A positioned page's text boxes have a VML fallback, which Word 2007 reads (it refuses a Word
/// 2010 shape on its own).
#[test]
fn word_text_boxes_have_a_fallback_for_word_2007() {
    let mut p = table_page();
    p.blocks[0].rotation = 45.0;
    let xml = part(&docx(&[p], "Rotated"), "word/document.xml");
    let boxes = xml.matches("<wps:txbx>").count();
    assert!(boxes > 0, "{xml}");
    assert_eq!(xml.matches("<mc:Fallback>").count(), boxes, "{xml}");
    assert!(xml.contains("<v:textbox"), "{xml}");
}

/// Rotated text is one text box turned with it, as wide as the text's run, not a column of
/// letters: the W-4 draft's "DRAFT — DO NOT FILE" down its margin.
#[test]
fn word_rotated_text_is_a_turned_line() {
    let mut p = page();
    p.blocks.push(Block {
        text: "DRAFT — DO NOT FILE".into(),
        // Running up the left margin: 11 pt text turned a quarter, so its box is tall and narrow.
        rect: [20.0, 200.0, 31.0, 330.0],
        size: 11.0,
        font: String::new(),
        bold: false,
        italic: false,
        color: [0.0; 3],
        rotation: 90.0,
        space_around: [false; 2],
    });
    let doc = craft_docx::read(&docx(&[p], "Draft")).unwrap();
    let objects = body_objects(&doc);
    let draft = objects.iter().copied().find(|o| shape_text(&doc, o) == "DRAFT — DO NOT FILE").expect("the margin text");
    let craft_wordmodel::InlineObject::Shape { w, h, float, .. } = draft else { panic!("not a shape") };
    assert!((float.rot - 270.0).abs() < 0.01, "{}", float.rot);
    assert!((*w - 130.0).abs() < 0.5 && (*h - 11.0).abs() < 0.5, "unturned, it is a line: {w} x {h}");
}

/// A line break or a tab inside a block's text stays one in its Word paragraph, in a table cell
/// too (storytold/pdfcraft#5112 by KyleKassen made Word export write them).
#[test]
fn word_keeps_line_breaks_and_tabs_inside_paragraphs_and_cells() {
    let mut p = page();
    p.blocks.truncate(1);
    p.blocks[0].text = "First\nSecond\r\nThird\tafter a tab".into();
    p.images.clear();
    let doc = craft_docx::read(&docx(&[p], "Breaks")).unwrap();
    let texts: Vec<String> = body_paragraphs(&doc).iter().map(|p| p.plain_text()).filter(|t| !t.is_empty()).collect();
    assert_eq!(texts, ["First\nSecond\nThird\tafter a tab"]);

    let mut t = table_page();
    t.blocks[0].text = "Meter\nnumber".into();
    t.blocks[4].text = "kWh\tunit".into();
    let tables = docx_tables(&docx(&[t], "Breaks"));
    let cells: Vec<String> = tables
        .iter()
        .flat_map(|t| t.rows.iter().flat_map(|r| &r.cells))
        .map(|c| c.blocks.iter().filter_map(|b| if let craft_wordmodel::Block::Para(p) = &**b { Some(p.plain_text()) } else { None }).collect())
        .collect();
    assert!(cells.contains(&"Meter\nnumber".to_string()) && cells.contains(&"kWh\tunit".to_string()), "{cells:?}");
}

#[test]
fn checked_rtf_reports_omitted_pictures_once_per_page_without_dropping_text() {
    let mut first = page();
    first.images[0].rotation = f64::NAN;
    first.images.push(first.images[0].clone());
    let clean = page();
    let mut third = first.clone();
    third.blocks[0].text = "Surviving last-page text".into();
    let pages = [first, clean, third];
    let (output, omitted) = crate::rtf_with_title_checked(&pages, "Partial");
    assert_eq!(omitted, [0, 2]);
    assert!(output.contains("Surviving last-page text"));
    assert_eq!(output, crate::rtf_with_title(&pages, "Partial"));
}
