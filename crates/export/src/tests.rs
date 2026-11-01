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
    let (tables, _) = tables(&p.blocks);
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
    let (tables, _) = tables(&blocks);
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
    let (tables, consumed) = tables(&p.blocks);
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
    let (tables, consumed) = tables(&p.blocks);
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
        stroke: Some(Stroke { color: [1.0, 0.0, 0.0], width: 0.0, dashed: true }),
        paths: vec![(vec![[60.0, 380.0], [300.0, 380.0]], false)],
        fill: None,
    });
    shapes.push(Shape { paths: vec![(vec![[100.0, 300.0], [120.0, 320.0], [140.0, 300.0]], true)], stroke: None, fill: Some([0.5; 3]) });
    shapes.push(Shape { paths: vec![(vec![[60.0, 500.0], [70.0, 510.0]], false)], ..boxed([0.0; 4]) });
    let fields = vec![
        Field {
            name: "Name".into(),
            rect: [60.0, 700.0, 300.0, 712.0],
            size: 0.0,
            kind: FieldKind::Text { value: String::new(), multiline: false, comb: None, max_len: None },
        },
        Field {
            name: "Postcode".into(),
            rect: [60.0, 520.0, 160.0, 532.0],
            size: 10.0,
            kind: FieldKind::Text { value: "10115".into(), multiline: false, comb: Some(5), max_len: Some(5) },
        },
        Field {
            name: "Notes".into(),
            rect: [60.0, 200.0, 540.0, 260.0],
            size: 9.0,
            kind: FieldKind::Text { value: "a & b".into(), multiline: true, comb: None, max_len: None },
        },
        Field { name: "Yes".into(), rect: [60.0, 150.0, 70.0, 160.0], size: 0.0, kind: FieldKind::Check { checked: true } },
        Field { name: "No".into(), rect: [90.0, 150.0, 100.0, 160.0], size: 0.0, kind: FieldKind::Check { checked: false } },
        Field {
            name: "Fund".into(),
            rect: [120.0, 150.0, 300.0, 162.0],
            size: 0.0,
            kind: FieldKind::Choice { value: "Health".into(), options: vec!["Health".into(), "Pension".into()] },
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

/// A form's fields become fields a Word user can fill in, over their boxes, limited to the
/// PDF's maximum length, in a document protected for filling in forms.
#[test]
fn word_makes_a_forms_fields_fillable_within_their_limits() {
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
    // Text fields are Word form fields named after the field; one with a maximum length in the
    // PDF takes no more characters in Word either.
    assert_eq!(xml.matches(" FORMTEXT ").count(), 3);
    assert!(
        xml.contains(
            "<w:name w:val=\"Name\"/><w:enabled/><w:calcOnExit w:val=\"0\"/><w:statusText w:type=\"text\" w:val=\"Name\"/><w:textInput></w:textInput>"
        ),
        "{xml}"
    );
    assert!(xml.contains("<w:name w:val=\"Postcode\"/>") && xml.contains("<w:textInput><w:maxLength w:val=\"5\"/></w:textInput>"), "{xml}");
    // A comb field spaces its characters one cell apart: 100 pt over 5 cells, 10 pt text.
    assert!(xml.contains("<w:spacing w:val=\"290\"/><w:sz w:val=\"20\"/></w:rPr><w:t xml:space=\"preserve\">10115</w:t>"), "{xml}");
    assert!(xml.contains(">a &amp; b</w:t>"), "{xml}");
    // Check boxes and lists are content controls.
    assert_eq!(xml.matches("<w:sdt>").count(), 3);
    assert!(xml.contains("<w14:checked w14:val=\"1\"/>") && xml.contains("<w14:checked w14:val=\"0\"/>"), "{xml}");
    assert!(xml.contains("<w:dropDownList><w:listItem w:displayText=\"Health\" w:value=\"Health\"/>"), "{xml}");
    // Word reads the check boxes through the Word 2010 namespace, which older readers may skip.
    assert!(xml.contains("mc:Ignorable=\"w14\""), "{xml}");
    // The document is protected for filling in forms, without a password, so the limits hold
    // and only the fields can be changed.
    let zip = docx(&[form_page()], "Claim");
    assert_eq!(
        part(&zip, "word/settings.xml"),
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:settings xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:documentProtection w:edit=\"forms\" w:enforcement=\"1\"/><w:compat><w:compatSetting w:name=\"compatibilityMode\" w:uri=\"http://schemas.microsoft.com/office/word\" w:val=\"15\"/></w:compat></w:settings>"
    );
    assert!(part(&zip, "[Content_Types].xml").contains("/word/settings.xml"));
    assert!(part(&zip, "word/_rels/document.xml.rels").contains("Target=\"settings.xml\""));
}

/// Word's form field names name bookmarks: letters, digits and `_`, starting with a letter, at
/// most 20 characters and unique; a PDF's dotted, numbered, accented or repeated names become such.
#[test]
fn word_form_field_names_are_valid_and_unique() {
    let mut p = form_page();
    let text = |name: &str| Field {
        name: name.into(),
        rect: [60.0, 50.0, 200.0, 62.0],
        size: 0.0,
        kind: FieldKind::Text { value: String::new(), multiline: false, comb: None, max_len: Some(8) },
    };
    p.fields =
        vec![text("Arbeitnehmeranteil.Betrag.1"), text("Arbeitnehmeranteil.Betrag.2"), text("1. Betrag (€)"), text("Name"), text("Name"), text("")];
    let xml = part(&docx(&[p], "Names"), "word/document.xml");
    let names: Vec<&str> = xml.split("<w:name w:val=\"").skip(1).map(|s| s.split('"').next().unwrap()).collect();
    assert_eq!(names.len(), 6, "{xml}");
    assert_eq!(names[0], "Arbeitnehmeranteil_B");
    assert!(names[1].starts_with("Arbeitnehmeran") && names[1].len() <= 20 && names[1] != names[0], "{names:?}");
    assert_eq!(names[2], "F1__Betrag____");
    assert_eq!(names[3], "Name");
    assert!(names[4].starts_with("Name_"));
    assert_eq!(names[5], "F");
    assert!(
        names
            .iter()
            .all(|n| n.len() <= 20 && n.starts_with(|c: char| c.is_ascii_alphabetic()) && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    );
    let unique: std::collections::HashSet<_> = names.iter().collect();
    assert_eq!(unique.len(), names.len());
    assert_eq!(xml.matches("<w:maxLength w:val=\"8\"/>").count(), 6);
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
    p.fields.push(Field { name: "<&>\u{0}".into(), rect: [f64::NAN, 0.0, 1.0, 1.0], size: f64::NAN, kind: FieldKind::Check { checked: true } });
    p.fields.push(Field {
        name: "x".into(),
        rect: [0.0, 0.0, 1e300, 1e300],
        size: -5.0,
        kind: FieldKind::Text { value: "1".into(), multiline: false, comb: Some(0), max_len: Some(0) },
    });
    for i in 0..MAX_PAGE_SHAPES + 50 {
        p.shapes.push(Shape { paths: vec![(vec![[1.0, i as f64 / 100.0], [2.0, 3.0]], false)], ..boxed([0.0; 4]) });
    }
    let xml = part(&docx(&[p], "Hostile"), "word/document.xml");
    assert!(xml.matches("id=\"Shape").count() <= MAX_PAGE_SHAPES);
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
            "<w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\" w:cs=\"Arial\"/><w:sz w:val=\"16\"/></w:rPr><w:t xml:space=\"preserve\">Name</w:t>"
        ),
        "{xml}"
    );
    assert!(xml.contains("w:ascii=\"Bad&quot;&lt;Name&gt;\""), "names are escaped: {xml}");
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
