use super::*;

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

#[test]
fn many_distinct_colours_take_constant_steps_each() {
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
    let xml = part(&docx(&[table_page()], "Invoice"), "word/document.xml");
    assert!(xml.contains("<w:tbl>"));
    assert!(!xml.contains("w:val=\"single\""), "text alignment is not evidence of a border: {xml}");
    assert!(xml.contains("<w:insideH w:val=\"nil\"/>"));
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
    let xml = docx_table(&table);
    assert!(xml.contains("<w:gridCol w:w=\"1000\"/><w:gridCol w:w=\"2000\"/><w:gridCol w:w=\"3000\"/>"), "{xml}");
    let widths: Vec<Vec<&str>> =
        xml.split("<w:tr>").skip(1).map(|row| row.split("<w:tcW w:w=\"").skip(1).map(|cell| cell.split('"').next().unwrap()).collect()).collect();
    assert_eq!(widths, vec![vec!["1000", "2000", "3000"], vec!["1000", "5000"], vec!["3000", "3000"], vec!["6000"], vec!["1000", "0", "0"]]);
}

#[test]
fn word_keeps_a_page_with_a_clear_image_and_big_title_in_flow() {
    // page() has a picture between paragraphs and a 24 pt title over 11 pt body text, but
    // nothing overlaps: it must stay flowing paragraphs with the image inline.
    let xml = part(&docx(&[page()], "Report"), "word/document.xml");
    assert!(!xml.contains("position:absolute"), "nothing overlaps, so nothing floats: {xml}");
    assert!(xml.contains("<wp:inline>"), "the picture stays inline: {xml}");
    assert!(xml.contains("<w:pStyle w:val=\"Heading1\"/>"), "the title stays a heading: {xml}");
    let (title, body) = (xml.find(">Annual Report</w:t>"), xml.find(">Body text after the picture"));
    assert!(title.is_some() && body.is_some() && title < body, "reading order kept: {xml}");
    // The same page with the picture moved over the body text floats.
    let mut over = page();
    over.images[0].rect = [72.0, 380.0, 272.0, 530.0];
    let xml = part(&docx(&[over], "Report"), "word/document.xml");
    assert!(xml.contains("position:absolute"), "an image over text floats: {xml}");
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
    let xml = part(&docx(&[p], "Invoice"), "word/document.xml");
    assert!(xml.contains("position:absolute"), "overlapping text must float: {xml}");
    assert!(!xml.contains("<wp:inline>"), "images must not displace invoice text");
    assert!(!xml.contains("<w:tbl>"), "the source text boxes must keep their positions");
    assert_eq!(xml.matches(">DRAFT</w:t>").count(), 1);
    assert_eq!(xml.matches("w:type=\"page\"").count(), 0);
    assert!(xml.contains("margin-left:72.000pt;margin-top:160.000pt"), "Meter field keeps its source position: {xml}");
    let watermark = xml.split(">DRAFT</w:t>").next().unwrap().rsplit("<v:rect ").next().unwrap();
    assert!(watermark.contains("z-index:-2"), "watermark is behind foreground graphics");
    // The picture is a DrawingML anchor behind the text (a VML rectangle shows no picture in Word).
    assert!(!xml.contains("<v:imagedata"), "{xml}");
    assert!(xml.contains("<wp:anchor ") && xml.contains("behindDoc=\"1\"") && xml.contains("<a:blip r:embed=\"rIdImg1\"/>"), "{xml}");
}

#[test]
fn word_rotated_pages_keep_separate_page_sizes_and_unique_shapes() {
    let mut p = table_page();
    p.blocks[0].rotation = 45.0;
    let mut second = p.clone();
    second.width = 400.0;
    second.height = 500.0;
    let xml = part(&docx(&[p, second], "Two invoices"), "word/document.xml");
    assert!(xml.contains("rotation:315.000"), "PDF y-up becomes Word y-down");
    assert_eq!(xml.matches("<w:sectPr>").count(), 2);
    assert!(xml.contains("<w:pgSz w:w=\"8000\" w:h=\"10000\"/>"));
    assert_eq!(xml.matches("id=\"Text1\"").count(), 1);
    assert_eq!(xml.matches(">Meter</w:t>").count(), 2);
}

/// #2992: pages that share a background colour get it as the Word document's page colour
/// (with the setting that makes Word show it); differing or white backgrounds don't.
#[test]
fn docx_keeps_a_shared_page_background() {
    let teal = Page { background: Some([0.0, 0.5, 0.5]), ..page() };
    let zip = docx(&[teal.clone(), teal.clone()], "Teal");
    let xml = part(&zip, "word/document.xml");
    assert!(xml.contains("<w:background w:color=\"008080\"/><w:body>"), "{xml}");
    assert!(part(&zip, "word/settings.xml").contains("<w:displayBackgroundShape/>"));
    assert!(part(&zip, "[Content_Types].xml").contains("/word/settings.xml"));
    assert!(part(&zip, "word/_rels/document.xml.rels").contains("Target=\"settings.xml\""));
    let other = Page { background: Some([1.0, 0.0, 0.0]), ..page() };
    for (pages, why) in [
        (vec![teal.clone(), other], "different colours"),
        (vec![teal.clone(), page()], "a page without one"),
        (vec![Page { background: Some([1.0; 3]), ..page() }], "white"),
        (vec![page()], "none"),
    ] {
        let zip = docx(&pages, "Plain");
        assert!(!part(&zip, "word/document.xml").contains("<w:background"), "{why}");
        assert!(!part(&zip, "word/settings.xml").contains("<w:displayBackgroundShape/>"), "{why}");
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
    let styles = |lang| part(&docx_with_language(&pages, "Report", lang), "word/styles.xml");
    let defaults = |lang: &str| format!("<w:docDefaults><w:rPrDefault><w:rPr>{lang}</w:rPr></w:rPrDefault></w:docDefaults>");
    assert!(styles(Some("bg-BG")).contains(&defaults("<w:lang w:val=\"bg-BG\"/>")), "{}", styles(Some("bg-BG")));
    assert!(styles(Some(" en-GB ")).contains("<w:lang w:val=\"en-GB\"/>"), "the tag is trimmed");
    assert!(styles(Some("zh-Hant-TW")).contains(&defaults("<w:lang w:eastAsia=\"zh-Hant-TW\"/>")), "East Asian text");
    assert!(styles(Some("ja")).contains("<w:lang w:eastAsia=\"ja\"/>"), "East Asian text");
    assert!(styles(Some("ar-EG")).contains(&defaults("<w:lang w:bidi=\"ar-EG\"/>")), "right-to-left text");
    assert!(styles(Some("HE")).contains("<w:lang w:bidi=\"HE\"/>"), "right-to-left text");
    // A script subtag decides over the language; private use names no script.
    assert!(styles(Some("pa-Arab-PK")).contains("<w:lang w:bidi=\"pa-Arab-PK\"/>"), "Punjabi in Arabic script");
    assert!(styles(Some("sr-Latn")).contains("<w:lang w:val=\"sr-Latn\"/>"), "Serbian in Latin script");
    assert!(styles(Some("zh-Latn")).contains("<w:lang w:val=\"zh-Latn\"/>"), "Chinese in Latin script");
    assert!(styles(Some("en-x-arab")).contains("<w:lang w:val=\"en-x-arab\"/>"), "private use");
    assert!(styles(Some("az-Latn-x-arab")).contains("<w:lang w:val=\"az-Latn-x-arab\"/>"), "private use");
    assert!(html_with_language(&pages, "Report", Some("bg-BG")).starts_with("<!DOCTYPE html>\n<html lang=\"bg-BG\">\n"));
    // A tag longer than 35 characters is still a tag.
    let long = "en-US-u-ca-gregory-nu-latn-co-phonebk";
    assert!(styles(Some(long)).contains(&format!("<w:lang w:val=\"{long}\"/>")), "{long}");
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
        assert!(docx_with_language(&pages, "Report", lang) == docx(&pages, "Report"), "{lang:?}");
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
    let xml = part(&docx(&[p.clone()], "Receipt\u{1}"), "word/document.xml");
    assert!(xml.contains(">Total 4.50\tGBP</w:t>"), "{xml}");
    assert!(!xml.chars().any(|c| !allowed(c)));
    assert!(xml.contains("<w:pgSz w:w=\"12240\" w:h=\"31680\"/>"), "clamped to 22 in: {xml}");
    assert!(xml.contains("<w:sz w:val=\"24\"/>"), "a non-finite size falls back to 12 pt");
    assert!(rtf(&[p]).contains("Total 4.50\tGBP"));
    // A narrow page keeps room for text: margins shrink.
    let narrow = Page { width: 100.0, height: 200.0, ..page() };
    let xml = part(&docx(&[narrow], "Narrow"), "word/document.xml");
    assert!(xml.contains("<w:pgMar w:top=\"500\" w:right=\"250\" w:bottom=\"500\" w:left=\"250\""), "{xml}");
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
    assert!(r.starts_with("{\\rtf1") && r.ends_with('}'));
    assert!(r.contains("\\fs48\\b Annual Report"));
    assert!(r.contains("\\fs20\\i Caf\\u233? \\{x\\}"), "{r}");
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
    let table_xml = docx_table(&tables[0]);
    assert!(table_xml.contains("<w:tblGrid>"));
    assert!(table_xml.contains("<w:gridSpan w:val=\"2\"/>"), "{table_xml}");
    assert_eq!(table_xml.matches("<w:tr>").count(), 4);

    let r = rtf(std::slice::from_ref(&p));
    assert!(r.contains("\\trowd"), "{r}");
    assert_eq!(r.matches("\\row").count(), 4);
    // Boundaries precede the cells; "Totals" spans two columns, so it ends at the third edge.
    assert!(r.contains("\\trowd\\trgaph108\\cellx4640\\cellx7840\\cellx9920{"), "{r}");
    assert!(r.contains("\\trowd\\trgaph108\\cellx7840{\\intbl\\fs22 Totals}\\cell\\row"), "{r}");

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
        let xml = part(&zip, "word/document.xml");
        assert_eq!(xml.contains("<wp:anchor "), placed, "{xml}");
        let rels = part(&zip, "word/_rels/document.xml.rels");
        let embed = xml.split("r:embed=\"").nth(1).and_then(|r| r.split('"').next()).unwrap();
        let rel = rels.split(&format!("Id=\"{embed}\"")).nth(1).unwrap();
        assert!(rel.contains("relationships/image\""), "{rels}");
        let target = rel.split("Target=\"").nth(1).and_then(|t| t.split('"').next()).unwrap();
        assert_eq!(target, "media/image1.jpg");
        assert!(zip.windows(21).any(|w| w == b"word/media/image1.jpg"));
        assert!(part(&zip, "[Content_Types].xml").contains("Extension=\"jpg\" ContentType=\"image/jpeg\""));
        assert!(xml.matches("<wp:docPr ").count() == 1 && !xml.contains("<v:imagedata"));
    }
}

#[test]
fn word_export_bounds_the_number_of_pictures_and_skips_empty_ones() {
    let mut p = page();
    let one = p.images[0].clone();
    p.images = vec![Image { bytes: [].as_slice().into(), ..one.clone() }];
    p.images.extend(std::iter::repeat_n(one, MAX_MEDIA + 10));
    let zip = docx(&[p], "T");
    let rels = part(&zip, "word/_rels/document.xml.rels");
    assert_eq!(rels.matches("relationships/image\"").count(), MAX_MEDIA);
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
    assert!(r.contains("{\\pard\\fs24 Before {\\fs24\\b0\\i\\cf0 Chevron }{\\fs24\\b0\\i0\\cf0 after.}\\par}"), "{r}");
    // The longest line is the slide's title; the sentence is one paragraph of the body.
    let pxml = part(&pptx(std::slice::from_ref(&p), "Opinion"), "ppt/slides/slide1.xml");
    assert!(
        pxml.contains("<a:p><a:r><a:rPr sz=\"1200\"></a:rPr><a:t>Before </a:t></a:r><a:r><a:rPr sz=\"1200\" i=\"1\"></a:rPr><a:t>Chevron </a:t></a:r><a:r><a:rPr sz=\"1200\"></a:rPr><a:t>after.</a:t></a:r></a:p>"),
        "{pxml}"
    );
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

/// A form exported to Word keeps its layout: its boxes, rules and comb cells are VML shapes
/// behind the text, in painting order, and its labels stay where they were (gap 8: the boxes
/// and fields were all lost, and the labels reflowed onto twice as many pages).
#[test]
fn word_keeps_a_forms_boxes_rules_and_labels_in_place() {
    let xml = part(&docx(&[form_page()], "Claim"), "word/document.xml");
    assert!(!xml.contains("<w:tbl>") && !xml.contains("<wp:inline>"), "nothing flows: {xml}");
    // Boxes, closed or drawn back to their corner: an axis-aligned rectangle each, measured from
    // the page's top-left corner (a free-form path showed a box as a diagonal in LibreOffice).
    assert_eq!(xml.matches("<v:rect id=\"Shape").count(), 7, "{xml}");
    assert!(xml.contains("margin-left:56.000pt;margin-top:118.000pt;width:484.000pt;height:24.000pt"), "{xml}");
    assert!(xml.contains("stroked=\"t\" strokecolor=\"#000000\" strokeweight=\"0.70pt\" filled=\"f\""), "{xml}");
    // A straight segment is a line, y down, written from its top end (LibreOffice drew nothing
    // for the comb ticks this form draws upwards); a rising diagonal is a two-point polyline.
    assert!(xml.contains("from=\"80.000pt,438.000pt\" to=\"80.000pt,442.000pt\""), "{xml}");
    assert!(xml.contains("<v:polyline id=\"Shape11\" style=\"position:absolute;z-index:-4;mso-position-horizontal-relative:page;mso-position-vertical-relative:page\" points=\"60.000pt,342.000pt,70.000pt,332.000pt\""), "{xml}");
    // A 0 pt line is drawn at the thinnest visible width.
    assert!(xml.contains("strokecolor=\"#FF0000\" strokeweight=\"0.25pt\""), "{xml}");
    assert!(xml.contains("<v:stroke dashstyle=\"dash\"/>"), "{xml}");
    // Anything else is a polyline, closed back to its first point when it is closed or filled.
    assert!(xml.contains("points=\"100.000pt,542.000pt,120.000pt,522.000pt,140.000pt,542.000pt,100.000pt,542.000pt\""), "{xml}");
    assert!(xml.contains("filled=\"t\" fillcolor=\"#808080\""), "{xml}");
    // Painting order: the first shape is furthest back, and every shape is behind the text.
    let z: Vec<i64> =
        xml.split("id=\"Shape").skip(1).map(|s| s.split("z-index:").nth(1).unwrap().split(';').next().unwrap().parse().unwrap()).collect();
    assert_eq!(z.len(), 11);
    assert!(z.windows(2).all(|w| w[0] < w[1]) && z.iter().all(|z| *z < -2), "{z:?}");
    // The labels keep their places, one line each that never wraps (and has room to spare for
    // readers that wrap anyway).
    assert!(xml.contains("margin-left:60.000pt;margin-top:120.000pt;width:77.000pt;height:8.000pt;rotation:0.000;z-index:1;"), "{xml}");
    // A long line's spare room ends at the page's edge: 60 + 480 × 1.25 + 2 would pass 595.
    let mut long = form_page();
    long.blocks.push(Block { size: 10.0, ..block("A line nearly as wide as the page", 60.0, 30.0, 540.0, 40.0) });
    assert!(part(&docx(&[long], "Long"), "word/document.xml").contains("margin-left:60.000pt;margin-top:802.000pt;width:535.000pt;"));
    // The width is fixed: Word caps a box that sizes itself at the width between the margins.
    assert!(!xml.contains("mso-wrap-style"), "{xml}");
}

/// A form's fields become content controls a Word user fills in over their boxes, without
/// protecting the document, so it can still be edited, cut and copied. A control's own run
/// properties give the text typed over its placeholder the field's font, size and comb spacing
/// (typed digits ran out of a comb's cells in Word); Word doesn't limit a control's length, so
/// the PDF's maximum length is named in the control's title.
#[test]
fn word_makes_a_forms_fields_content_controls_that_keep_their_look() {
    let xml = part(&docx(&[form_page()], "Claim"), "word/document.xml");
    // Fields: framed paragraphs over the fields' boxes, which Word places out of the text flow.
    assert_eq!(xml.matches("<w:framePr ").count(), 6, "{xml}");
    // The comb field's frame reaches one cell (20 pt) past its box: Word spaces the last
    // character too, and in Word the last digit of a postcode wrapped out of sight.
    // Its line of 10 pt text (11.5 pt high) sits in the middle of the 12 pt box: the frame
    // starts a quarter point lower, rather than its paragraph having space before it.
    assert!(xml.contains("<w:framePr w:w=\"2400\" w:h=\"235\" w:hRule=\"exact\" w:wrap=\"around\" w:vAnchor=\"page\" w:hAnchor=\"page\" w:x=\"1200\" w:y=\"6205\"/><w:spacing w:before=\"0\""), "{xml}");
    // The page places everything itself: a quarter-inch top margin, so that Word doesn't move
    // a field framed near the top of the page down to a one-inch margin.
    assert!(xml.contains("<w:pgMar w:top=\"360\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"180\""), "{xml}");
    // Every field is a content control tagged with its name; none is a legacy form field.
    assert_eq!(xml.matches("<w:sdt>").count(), 6, "{xml}");
    assert!(!xml.contains("FORMTEXT") && !xml.contains("<w:ffData>"), "{xml}");
    let arial = "<w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\" w:eastAsia=\"Arial\" w:cs=\"Arial\"/>";
    // An empty field shows a placeholder, and its typed text takes its own properties.
    let name = xml.split("<w:sdtPr>").nth(1).unwrap().split("</w:sdtPr>").next().unwrap();
    assert!(name.starts_with(&format!("<w:rPr>{arial}<w:color w:val=\"000000\"/><w:sz w:val=\"16\"/></w:rPr>")), "{name}");
    assert!(name.contains("<w:alias w:val=\"Name\"/>") && name.contains("<w:showingPlcHdr/>") && name.contains("<w:text/>"));
    // The comb's 6 pt Courier advance plus 14 pt spacing is one 20 pt cell.
    let courier = arial.replace("Arial", "Courier New");
    let comb = format!("<w:rPr>{courier}<w:color w:val=\"000000\"/><w:spacing w:val=\"280\"/><w:sz w:val=\"20\"/></w:rPr>");
    assert_eq!(xml.matches(&comb).count(), 2, "{xml}");
    assert!(xml.contains("<w:alias w:val=\"Postcode (max. 5)\"/>"), "{xml}");
    assert!(xml.contains(&format!("<w:r>{comb}<w:t xml:space=\"preserve\">10115</w:t>")), "{xml}");
    assert!(xml.contains("<w:tag w:val=\"Notes\"/>") && xml.contains("<w:text w:multiLine=\"1\"/>") && xml.contains(">a &amp; b</w:t>"), "{xml}");
    // Check boxes and lists keep their look the same way.
    assert!(
        xml.contains(&format!("<w:sdtPr><w:rPr>{arial}<w:color w:val=\"000000\"/><w:sz w:val=\"16\"/></w:rPr><w:alias w:val=\"Yes\"/>")),
        "{xml}"
    );
    assert!(xml.contains("<w14:checked w14:val=\"1\"/>") && xml.contains("<w14:checked w14:val=\"0\"/>"), "{xml}");
    assert!(xml.contains("<w:dropDownList><w:listItem w:displayText=\"Health\" w:value=\"Health\"/>"), "{xml}");
    // Word reads the check boxes through the Word 2010 namespace, which older readers may skip.
    assert!(xml.contains("mc:Ignorable=\"w14 wps\""), "{xml}");
    // Not protected: a protected form could not be copied or cut in Word (it only lets the
    // fields be changed), which is how a reader reuses an exported form.
    let zip = docx(&[form_page()], "Claim");
    assert_eq!(
        part(&zip, "word/settings.xml"),
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:settings xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:compat><w:compatSetting w:name=\"compatibilityMode\" w:uri=\"http://schemas.microsoft.com/office/word\" w:val=\"15\"/></w:compat></w:settings>"
    );
    assert!(part(&zip, "[Content_Types].xml").contains("/word/settings.xml"));
    assert!(part(&zip, "word/_rels/document.xml.rels").contains("Target=\"settings.xml\""));
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
    let ruled = Page { fields: Vec::new(), ..form_page() };
    assert!(part(&docx(&[ruled], "Ruled"), "word/document.xml").contains("<v:rect id=\"Shape"));

    let mut text = page();
    text.images.clear();
    for y in [395.0, 500.0, 600.0, 690.0] {
        text.shapes.push(Shape { paths: vec![(vec![[72.0, y], [500.0, y]], false)], ..boxed([0.0; 4]) });
    }
    let xml = part(&docx(&[text.clone()], "Report"), "word/document.xml");
    assert!(!xml.contains("position:absolute"), "underlines alone don't hold a page in place: {xml}");
    // Five upright boxes around text are not yet a form either; six are.
    let mut framed = text.clone();
    for i in 0..FRAMING_SHAPES - 1 {
        framed.shapes.push(boxed([70.0, 398.0 - i as f64, 502.0, 413.0 + i as f64]));
    }
    assert!(!part(&docx(&[framed.clone()], "Report"), "word/document.xml").contains("position:absolute"));
    framed.shapes.push(boxed([70.0, 690.0, 502.0, 726.0]));
    assert!(part(&docx(&[framed], "Report"), "word/document.xml").contains("position:absolute"));
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
/// subpaths wrote a VML shape per subpath past the limit, without a word. A page that has to
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
    assert_eq!(part(&zip, "word/document.xml").matches("id=\"Shape").count(), MAX_DOCX_PAGE_SHAPES);
    let (zip, dropped) = docx_checked(&[form_page(), with(MAX_DOCX_PAGE_SHAPES)], "Too many");
    assert_eq!(dropped, [1], "the second page is cut short");
    let xml = part(&zip, "word/document.xml");
    let second = xml.split("<w:sectPr>").nth(1).unwrap();
    assert_eq!(second.matches("id=\"Shape").count(), MAX_DOCX_PAGE_SHAPES, "it keeps the first ones");
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
/// height) is flipped back in every format that shows pictures: a scanned form drawn in 70 such
/// strips came out in Word with every strip upside down, as if cut apart and pasted back wrongly.
/// Simple HTML, which has no styles, shows it as stored.
#[test]
fn a_mirrored_picture_is_flipped_back_in_every_format() {
    let flipped = |mut p: Page| {
        p.images.iter_mut().for_each(|im| im.flip = true);
        p
    };
    // Flowing Word: an inline picture.
    let xml = part(&docx(&[flipped(page())], "Scan"), "word/document.xml");
    assert!(xml.contains("<wp:inline>") && xml.contains("<a:xfrm flipV=\"1\">"), "{xml}");
    // Positioned Word: an anchored picture, flipped in its frame before it is rotated.
    let mut form = form_page();
    form.images = page().images;
    let xml = part(&docx(&[flipped(form.clone())], "Scan"), "word/document.xml");
    assert!(xml.contains("<wp:anchor ") && xml.contains("<a:xfrm rot=\"0\" flipV=\"1\">"), "{xml}");
    assert!(!part(&docx(&[form], "Scan"), "word/document.xml").contains("flipV"), "an upright picture stays as it is");
    // PowerPoint and HTML.
    assert!(part(&pptx(&[flipped(page())], "Scan"), "ppt/slides/slide1.xml").contains("<a:xfrm flipV=\"1\">"));
    assert!(!part(&pptx(&[page()], "Scan"), "ppt/slides/slide1.xml").contains("flipV"));
    assert!(html(&[flipped(page())], "Scan").contains("<img alt=\"\" style=\"transform:scale(1,-1)\" src="));
    assert!(!html(&[page()], "Scan").contains("transform"));
    let simple = html_simple(&[flipped(page())], "Scan");
    assert!(simple.contains("<img alt=\"\" src=") && !simple.contains("style="), "{simple}");
}

/// The writers that place pictures upright (flowing Word pages, PowerPoint, HTML) compose a mirror
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
    let docx_xml = |p: Page| part(&docx(&[p], "Scan"), "word/document.xml");
    let slide = |p: Page| part(&pptx(&[p], "Scan"), "ppt/slides/slide1.xml");
    // A left-right mirror.
    assert!(docx_xml(turned(true)).contains("<a:xfrm flipH=\"1\">"));
    assert!(slide(turned(true)).contains("<a:xfrm flipH=\"1\">"));
    assert!(html(&[turned(true)], "Scan").contains("transform:scale(-1,1)"));
    // Upright, turned half round.
    assert!(docx_xml(turned(false)).contains("<a:xfrm flipH=\"1\" flipV=\"1\">"));
    assert!(slide(turned(false)).contains("<a:xfrm flipH=\"1\" flipV=\"1\">"));
    assert!(html(&[turned(false)], "Scan").contains("transform:scale(-1,-1)"));
    // A positioned page turns the picture: a vertical flip before the half turn.
    let mut form = form_page();
    form.images = turned(true).images;
    assert!(docx_xml(form).contains("<a:xfrm rot=\"10800000\" flipV=\"1\">"));
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
/// stack above every picture (a VML shape went behind a picture in LibreOffice), with the border
/// inside the field's box.
#[test]
fn word_draws_a_fields_border_and_fill_over_the_pictures() {
    let mut p = form_page();
    p.images = page().images;
    p.fields[3].border = Some(Stroke { color: [0.0; 3], width: 1.0, dashed: false });
    p.fields[3].background = Some([1.0; 3]);
    p.fields[0].border = Some(Stroke { color: [1.0, 0.0, 0.0], width: 2.0, dashed: true });
    let xml = part(&docx(&[p], "Scan"), "word/document.xml");
    assert_eq!(xml.matches("name=\"Field box ").count(), 2, "only fields with a border or fill: {xml}");
    // The check box [60, 150, 70, 160]: its 1 pt border half a point inside, on an 842 pt page.
    assert!(
        xml.contains("<wp:posOffset>768350</wp:posOffset></wp:positionH><wp:positionV relativeFrom=\"page\"><wp:posOffset>8667750</wp:posOffset>"),
        "{xml}"
    );
    assert!(xml.contains("<wp:extent cx=\"114300\" cy=\"114300\"/>"), "{xml}");
    assert!(
        xml.contains(
            "<a:solidFill><a:srgbClr val=\"FFFFFF\"/></a:solidFill><a:ln w=\"12700\"><a:solidFill><a:srgbClr val=\"000000\"/></a:solidFill></a:ln>"
        ),
        "{xml}"
    );
    assert!(
        xml.contains("<a:noFill/><a:ln w=\"25400\"><a:solidFill><a:srgbClr val=\"FF0000\"/></a:solidFill><a:prstDash val=\"dash\"/></a:ln>"),
        "{xml}"
    );
    // Stacked above the picture, behind the text.
    let height = |marker: &str| -> u64 {
        let at = xml.find(marker).unwrap();
        xml[..at].rsplit("relativeHeight=\"").next().unwrap().split('"').next().unwrap().parse().unwrap()
    };
    assert!(height("name=\"Field box ") > height("<pic:pic "), "{xml}");
    assert!(!xml.contains("behindDoc=\"0\""));
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
    let xml = docx_table(&found[0]);
    for edge in ["insideH", "insideV"] {
        assert!(xml.contains(&format!("<w:{edge} w:val=\"single\"")), "{xml}");
    }
    for edge in ["top", "left", "bottom", "right"] {
        assert!(xml.contains(&format!("<w:{edge} w:val=\"nil\"/>")), "{xml}");
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
    let xml = part(&docx(&[p], "Grouped boxes"), "word/document.xml");
    assert!(xml.contains("<v:rect id=\"Text"), "{xml}");
}

#[test]
fn office_regression_retraced_rules_are_not_rectangles() {
    let mut p = form_page();
    p.shapes = vec![Shape { paths: vec![(vec![[10.0, 10.0], [30.0, 10.0], [10.0, 10.0], [10.0, 30.0]], true)], ..boxed([0.0; 4]) }];
    let xml = part(&docx(&[p], "Retraced rules"), "word/document.xml");
    assert!(xml.contains("<v:polyline id=\"Shape"));
    assert!(!xml.contains("<v:rect id=\"Shape"));
}

#[test]
fn office_regression_open_paths_fill_closed_but_stroke_open() {
    let mut p = form_page();
    p.shapes = vec![Shape { paths: vec![(vec![[10.0, 10.0], [30.0, 10.0], [20.0, 30.0]], false)], fill: Some([0.5; 3]), ..boxed([0.0; 4]) }];
    let xml = part(&docx(&[p], "Open triangle"), "word/document.xml");
    let paths: Vec<_> = xml.split("<v:polyline ").skip(1).map(|s| s.split("</v:polyline>").next().unwrap()).collect();
    assert_eq!(paths.len(), 2);
    assert!(paths[0].contains("stroked=\"f\"") && paths[0].contains("filled=\"t\""));
    assert!(paths[0].contains("points=\"10.000pt,832.000pt,30.000pt,832.000pt,20.000pt,812.000pt,10.000pt,832.000pt\""));
    assert!(paths[1].contains("stroked=\"t\"") && paths[1].contains("filled=\"f\""));
    assert!(paths[1].contains("points=\"10.000pt,832.000pt,30.000pt,832.000pt,20.000pt,812.000pt\""));
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
    let xml = part(&docx(&[p], "Pieces"), "word/document.xml");
    for text in ["First", "Bold", "Italic"] {
        let count = xml.split("<w:t xml:space=\"preserve\">").skip(1).filter(|s| s.split("</w:t>").next().unwrap().trim() == text).count();
        assert_eq!(count, 1, "{xml}");
    }
    assert_eq!(xml.matches("<v:rect id=\"Text").count(), 1);
    assert!(xml.contains("<w:b/>") && xml.contains("<w:i/>"));
}

#[test]
fn office_regression_field_formatting_applies_to_values_and_replacements() {
    let mut p = form_page();
    p.fields.truncate(1);
    p.fields[0].font = "Helvetica-BoldOblique".into();
    p.fields[0].color = [1.0, 0.0, 0.0];
    p.fields[0].alignment = 2;
    let xml = part(&docx(&[p], "Styled field"), "word/document.xml");
    assert_eq!(xml.matches("<w:b/><w:bCs/><w:i/><w:iCs/><w:color w:val=\"FF0000\"/>").count(), 2);
    assert!(xml.contains("<w:jc w:val=\"right\"/>"));
}

#[test]
fn office_regression_round_buttons_retain_one_visible_boundary() {
    let mut p = form_page();
    p.shapes.clear();
    p.fields.retain(|f| f.name == "No");
    p.fields[0].round = true;
    p.fields[0].border = Some(Stroke { color: [0.0; 3], width: 1.0, dashed: false });
    let xml = part(&docx(&[p.clone()], "Round"), "word/document.xml");
    assert_eq!(xml.matches("<a:prstGeom prst=\"ellipse\">").count(), 1, "{xml}");
    assert!(xml.contains("<w14:checked w14:val=\"0\"/>"));
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
    let xml = part(&docx(&[p], "Already outlined"), "word/document.xml");
    assert_eq!(xml.matches("<v:polyline id=\"Shape").count(), 1);
    assert!(!xml.contains("<a:prstGeom prst=\"ellipse\">"), "no duplicate border");
}

#[test]
fn review_invalid_fields_do_not_embed_hidden_values() {
    for rect in [[f64::NAN, 0.0, 100.0, 20.0], [0.0, 0.0, f64::INFINITY, 20.0], [20.0, 0.0, 10.0, 20.0]] {
        let mut p = form_page();
        p.fields.truncate(2);
        p.fields[0].rect = rect;
        p.fields[0].kind = FieldKind::Text { value: "PRIVATE OMITTED VALUE".into(), multiline: false, comb: None, max_len: None };
        let (zip, dropped) = docx_checked(&[p], "Invalid field");
        assert!(!part(&zip, "word/document.xml").contains("PRIVATE OMITTED VALUE"));
        assert!(!part(&zip, "customXml/item1.xml").contains("PRIVATE OMITTED VALUE"));
        assert_eq!(dropped, [0]);
    }
}

#[test]
fn review_direct_field_auto_size_fits_width_and_multiline_height() {
    for size in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut p = form_page();
        p.fields.truncate(1);
        let f = &mut p.fields[0];
        f.size = size;
        f.font = "Courier".into();
        f.rect = [20.0, 20.0, 82.0, 40.0];
        f.kind = FieldKind::Text { value: "WWWWWWWWWWWWWWWWWWWW".into(), multiline: false, comb: None, max_len: None };
        let xml = part(&docx(&[p.clone()], "Single line"), "word/document.xml");
        assert!(xml.contains("<w:sz w:val=\"10\"/>"), "20 Courier glyphs fit at 5 pt");
        p.fields[0].rect = [20.0, 20.0, 200.0, 80.0];
        p.fields[0].kind = FieldKind::Text { value: "one\ntwo\nthree".into(), multiline: true, comb: None, max_len: None };
        let xml = part(&docx(&[p.clone()], "Multiline"), "word/document.xml");
        assert_eq!(xml.matches("<w:sz w:val=\"24\"/>").count(), 2);
        p.fields[0].size = 9.0;
        let xml = part(&docx(&[p], "Explicit"), "word/document.xml");
        assert_eq!(xml.matches("<w:sz w:val=\"18\"/>").count(), 2);
    }
}

#[test]
fn review_regular_family_names_do_not_imply_bold() {
    for (font, bold) in [("BlackadderITC-Regular", false), ("HeavyMetal-Regular", false), ("Arial-Black", true), ("Helvetica-BoldOblique", true)] {
        let mut p = form_page();
        p.fields.truncate(1);
        p.fields[0].font = font.into();
        let xml = part(&docx(&[p], "Weight"), "word/document.xml");
        assert_eq!(xml.contains("<w:bCs/>"), bold, "{font}");
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
        let xml = part(&docx(&[p], "Paint order"), "word/document.xml");
        let image = xml.find("<pic:pic ").unwrap();
        let path = xml.find("<a:custGeom>").unwrap();
        assert_eq!(image < path, image_first);
        let layers: Vec<_> = xml.split("relativeHeight=\"").skip(1).map(|s| s.split('"').next().unwrap()).collect();
        assert_eq!(layers, ["1"]);
        assert!(xml.contains("<wpg:wgp "));
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
fn review_repeated_choices_and_checks_share_values() {
    for kind in [
        FieldKind::Check { checked: true, on_state: "Yes".into() },
        FieldKind::Choice { value: "First".into(), options: vec!["First".into(), "Second".into()], editable: false },
    ] {
        let mut p = form_page();
        p.fields.truncate(1);
        p.fields[0].kind = kind;
        p.fields.push(p.fields[0].clone());
        p.fields[1].rect = [20.0, 20.0, 100.0, 40.0];
        let zip = docx(&[p], "Repeated controls");
        assert_eq!(part(&zip, "word/document.xml").matches("<w:dataBinding ").count(), 2);
        assert_eq!(part(&zip, "customXml/item1.xml").matches("<pc:field ").count(), 1);
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
fn office_regression_repeated_text_widgets_share_a_custom_xml_value() {
    let mut p = form_page();
    p.fields.truncate(1);
    p.fields[0].name = format!("{}A", "x".repeat(MAX_FIELD_NAME_CHARS));
    p.fields[0].kind = FieldKind::Text { value: "a<&\rb".into(), multiline: true, comb: None, max_len: None };
    let mut second = p.clone();
    second.fields.push(Field { name: format!("{}B", "x".repeat(MAX_FIELD_NAME_CHARS)), ..p.fields[0].clone() });
    let zip = docx(&[p, second], "Repeated");
    let xml = part(&zip, "word/document.xml");
    assert_eq!(xml.matches("w:xpath=\"/pc:fields/pc:field[@id='f1']\"").count(), 2);
    assert_eq!(xml.matches("w:xpath=\"/pc:fields/pc:field[@id='f2']\"").count(), 1, "full names distinguish identical displayed tags");
    let data = part(&zip, "customXml/item1.xml");
    assert_eq!(data.matches("<pc:field id=").count(), 2);
    assert!(data.contains(">a&lt;&amp;&#13;b</pc:field>"));
    assert!(part(&zip, "word/_rels/document.xml.rels").contains("Target=\"../customXml/item1.xml\""));
    assert!(part(&zip, "customXml/_rels/item1.xml.rels").contains("Target=\"itemProps1.xml\""));
    assert!(part(&zip, "[Content_Types].xml").contains("/customXml/itemProps1.xml"));
    let properties = part(&zip, "customXml/itemProps1.xml");
    let store = properties.split("ds:itemID=\"").nth(1).unwrap().split('"').next().unwrap();
    assert_eq!(xml.matches(&format!("w:storeItemID=\"{store}\"")).count(), 3);
}

#[test]
fn office_regression_comb_values_use_constant_width_advances() {
    let mut p = form_page();
    p.fields.retain(|f| f.name == "Postcode");
    p.fields[0].font = "Helvetica".into();
    p.fields[0].kind = FieldKind::Text { value: "WiWiW".into(), multiline: false, comb: Some(5), max_len: Some(5) };
    let xml = part(&docx(&[p], "Comb"), "word/document.xml");
    assert_eq!(xml.matches("w:ascii=\"Courier New\"").count(), 2);
    // Each 20 pt cell = Courier's 6 pt advance + 14 pt character spacing.
    assert_eq!(xml.matches("<w:spacing w:val=\"280\"/>").count(), 2);
    assert!(xml.contains("<w:ind w:left=\"140\"/>"));
    assert!(xml.contains("<w:framePr w:w=\"2400\""));
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
    assert_eq!(r.matches("{\\pard").count(), 2, "{r}");
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
fn review_followup_checkbox_bindings_distinguish_on_states() {
    let mut p = form_page();
    p.fields.truncate(1);
    let check = p.fields[0].clone();
    p.fields = [("Yes", true), ("Other", false), ("Yes", true)]
        .into_iter()
        .map(|(on_state, checked)| Field { kind: FieldKind::Check { checked, on_state: on_state.into() }, ..check.clone() })
        .collect();
    let zip = docx(&[p], "Checkbox states");
    let xml = part(&zip, "word/document.xml");
    assert_eq!(xml.matches("w:xpath=\"/pc:fields/pc:field[@id='f1']\"").count(), 2);
    assert_eq!(xml.matches("w:xpath=\"/pc:fields/pc:field[@id='f2']\"").count(), 1);
    let data = part(&zip, "customXml/item1.xml");
    assert!(data.contains("<pc:field id=\"f1\">true</pc:field>"));
    assert!(data.contains("<pc:field id=\"f2\">false</pc:field>"));
}

#[test]
fn review_followup_cropped_fields_keep_their_full_offset_in_fixed_textboxes() {
    let mut p = form_page();
    p.fields.truncate(1);
    p.fields[0].rect = [-60.0, 200.0, 180.0, 250.0];
    p.fields[0].kind = FieldKind::Text { value: "ABCDEFGH".into(), multiline: false, comb: None, max_len: None };
    let xml = part(&docx(&[p.clone()], "Crop"), "word/document.xml");
    assert!(xml.contains("margin-left:-60.000pt"));
    assert!(xml.contains("width:240.000pt;height:50.000pt;z-index:1"));
    assert!(xml.contains("mso-fit-shape-to-text:f"));
    assert!(!xml.contains("<w:framePr"));
    p.fields[0].kind = FieldKind::Text { value: "ABCDEFGH".into(), multiline: false, comb: Some(8), max_len: Some(8) };
    let xml = part(&docx(&[p], "Cropped comb"), "word/document.xml");
    assert!(xml.contains("width:270.000pt;height:50.000pt;z-index:1"), "last-cell allowance remains inside the textbox");
}

#[test]
fn review_followup_partial_page_artwork_keeps_its_group_scale() {
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
    let xml = part(&bytes, "word/document.xml");
    let extent = xml.split("name=\"Page extent\"").nth(1).unwrap().split("</wps:wsp>").next().unwrap();
    assert!(extent.contains("<a:off x=\"0\" y=\"0\"/><a:ext cx=\"3810000\" cy=\"5080000\"/>"));
    assert!(extent.contains("<a:noFill/><a:ln><a:noFill/></a:ln>"));
    assert!(xml.contains("<a:off x=\"2540000\" y=\"1143000\"/>"), "the image keeps its original offset");
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
    assert!(!part(&bytes, "word/document.xml").contains("<v:shape "));
}

#[test]
fn review_followup_compound_fills_use_one_geometry_in_both_writers() {
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
        let xml = part(&zip, "word/document.xml");
        if image {
            assert_eq!(xml.matches("<a:path ").count(), 1);
            assert_eq!(xml.matches("<a:moveTo>").count(), 4);
        } else {
            assert_eq!(xml.matches("<v:shape ").count(), 1);
            assert!(xml.contains("coordsize=\"21600,21600\""));
        }
    }
}
