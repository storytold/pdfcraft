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
        })
        .collect();
    Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), background: None }
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
    let cell = |text: &str, span| Cell { text: text.into(), size: 11.0, bold: false, italic: false, color: [0.0; 3], span };
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
        assert!(!zip.windows(b"word/settings.xml".len()).any(|w| w == b"word/settings.xml"), "{why}");
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
            },
        ],
        images: Vec::new(),
        background: None,
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
                    }
                })
            })
            .collect(),
        images: Vec::new(),
        background: None,
    };
    let grid_cols = |p: &Page| part(&docx(std::slice::from_ref(p), "Grid"), "word/document.xml").matches("<w:gridCol ").count();
    assert_eq!(grid_cols(&grid(63)), 63, "63 columns is still a table");
    // Word refuses to open a document with a 64-column table, so that grid stays text.
    assert_eq!(grid_cols(&grid(64)), 0);
}

fn block(t: &str, x0: f64, y0: f64, x1: f64, y1: f64) -> Block {
    Block { text: t.into(), rect: [x0, y0, x1, y1], size: 12.0, bold: false, italic: false, color: [0.0; 3], rotation: 0.0, space_around: [false; 2] }
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
    let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), background: None };
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
    let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), background: None };
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
    let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), background: None };
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
    let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), background: None };
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
    let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), background: None };
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
    let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), background: None };
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
    let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), background: None };
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
    let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), background: None };
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
    let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), background: None };
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
        let (found, _) = tables(&p.blocks);
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
    let (found, consumed) = tables(&blocks);
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
    let (found, _) = tables(&blocks);
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
    let (found, _) = tables(&blocks);
    assert!(found.is_empty(), "{found:?}");
    let paragraphs = word_texts(blocks);
    assert_eq!(paragraphs.len(), 150, "{paragraphs:?}");
    assert!(paragraphs.iter().enumerate().all(|(n, p)| *p == format!("\u{F0B7} item {n};")), "{paragraphs:?}");
}
