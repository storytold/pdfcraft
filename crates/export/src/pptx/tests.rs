use super::*;
use crate::Block;
use crate::tests::part;

fn sample_page() -> Page {
    Page {
        width: 720.0,
        height: 540.0,
        blocks: vec![
            Block {
                text: "Q3 Business Update".into(),
                rect: [72.0, 460.0, 500.0, 500.0],
                size: 28.0,
                bold: true,
                italic: false,
                color: [0.1, 0.2, 0.3],
                rotation: 0.0,
                space_around: [false; 2],
            },
            Block {
                text: "Revenue grew 25% year over year across all regions.".into(),
                rect: [72.0, 380.0, 500.0, 410.0],
                size: 14.0,
                bold: false,
                italic: false,
                color: [0.0; 3],
                rotation: 0.0,
                space_around: [false; 2],
            },
            Block {
                text: "Operational expenses remained flat.".into(),
                rect: [72.0, 340.0, 500.0, 370.0],
                size: 14.0,
                bold: false,
                italic: true,
                color: [0.0; 3],
                rotation: 0.0,
                space_around: [false; 2],
            },
        ],
        images: vec![Image {
            ext: "png",
            bytes: b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x02\0\0\0\x03".as_slice().into(),
            rect: [72.0, 150.0, 300.0, 300.0],
            size: [228.0, 150.0],
            rotation: 0.0,
        }],
        background: None,
    }
}

#[test]
pub fn pptx_is_a_presentationml_package() {
    let p = sample_page();
    let bytes = pptx(&[p], "Executive Deck");
    assert!(bytes.starts_with(b"PK"));
    for part_name in [
        "[Content_Types].xml",
        "_rels/.rels",
        "docProps/core.xml",
        "ppt/presentation.xml",
        "ppt/_rels/presentation.xml.rels",
        "ppt/slideMasters/slideMaster1.xml",
        "ppt/slideMasters/_rels/slideMaster1.xml.rels",
        "ppt/slideLayouts/slideLayout1.xml",
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
        "ppt/theme/theme1.xml",
        "ppt/slides/slide1.xml",
        "ppt/slides/_rels/slide1.xml.rels",
        "ppt/media/image1.png",
    ] {
        assert!(bytes.windows(part_name.len()).any(|w| w == part_name.as_bytes()), "missing {part_name}");
    }
}

#[test]
pub fn pptx_slide_has_title_body_and_images_in_order() {
    let p = sample_page();
    let bytes = pptx(&[p], "Deck");
    let slide_xml = part(&bytes, "ppt/slides/slide1.xml");
    assert!(slide_xml.contains("name=\"Title 2\""));
    assert!(slide_xml.contains("txBox=\"1\""));
    assert!(slide_xml.contains("Q3 Business Update"));
    assert!(slide_xml.contains("name=\"TextBox 3\""));
    assert!(slide_xml.contains("Revenue grew 25%"));
    assert!(slide_xml.contains("<p:pic>"));
    assert!(slide_xml.contains("<a:blip r:embed=\"rIdImg1\"/>"));
    assert!(!slide_xml.contains("<p:ph"), "pure text boxes avoid placeholder inheritance repair warnings: {slide_xml}");

    let rels_xml = part(&bytes, "ppt/slides/_rels/slide1.xml.rels");
    assert!(rels_xml.contains("Target=\"../media/image1.png\""));
    assert!(rels_xml.contains("Target=\"../slideLayouts/slideLayout1.xml\""));
}

#[test]
pub fn pptx_text_colour_and_formatting() {
    let p = sample_page();
    let bytes = pptx(&[p], "Deck");
    let slide_xml = part(&bytes, "ppt/slides/slide1.xml");
    assert!(slide_xml.contains("sz=\"2800\""));
    assert!(slide_xml.contains("b=\"1\""));
    assert!(slide_xml.contains("<a:srgbClr val=\"1A334D\"/>"));
    assert!(slide_xml.contains("i=\"1\""));
}

/// Every entry of the package in order: (name, uncompressed bytes), read from the local
/// headers the way an unzipper walks them, so a broken structure fails here.
fn entries(zip: &[u8]) -> Vec<(String, Vec<u8>)> {
    let at = |i: usize, n: usize| zip[i..i + n].iter().rev().fold(0usize, |v, b| v << 8 | *b as usize);
    let mut out = Vec::new();
    let mut i = 0;
    while zip[i..].starts_with(b"PK\x03\x04") {
        let (method, size, raw, name_len, extra) = (at(i + 8, 2), at(i + 18, 4), at(i + 22, 4), at(i + 26, 2), at(i + 28, 2));
        let data = i + 30 + name_len + extra;
        let name = String::from_utf8(zip[i + 30..i + 30 + name_len].to_vec()).expect("UTF-8 name");
        let body = &zip[data..data + size];
        let bytes = match method {
            0 => body.to_vec(),
            8 => {
                let mut v = Vec::new();
                std::io::Read::read_to_end(&mut flate2::read::DeflateDecoder::new(body), &mut v).expect("inflates");
                v
            }
            m => panic!("{name}: unexpected method {m}"),
        };
        assert_eq!(bytes.len(), raw, "{name}: uncompressed size");
        out.push((name, bytes));
        i = data + size;
    }
    assert!(zip[i..].starts_with(b"PK\x01\x02"), "the central directory follows the entries");
    out
}

/// Checks that every start tag is closed by the matching end tag (text is escaped, so a
/// `<` always starts markup). Catches a typo such as `<a:lnR>…</a:lnL>`.
fn assert_tags_balance(name: &str, xml: &str) {
    let mut stack: Vec<&str> = Vec::new();
    for chunk in xml.split('<').skip(1) {
        let tag = chunk.split('>').next().unwrap_or_default();
        if tag.starts_with('?') {
            continue;
        }
        if let Some(close) = tag.strip_prefix('/') {
            assert_eq!(stack.pop(), Some(close.trim()), "{name}: </{close}> closes the wrong element");
        } else if !tag.ends_with('/') {
            stack.push(tag.split_whitespace().next().unwrap_or_default());
        }
    }
    assert!(stack.is_empty(), "{name}: unclosed {stack:?}");
}

#[test]
fn pptx_package_parts_are_complete_and_consistent() {
    let cell = |t: &str, x: f64, y: f64| Block {
        text: t.into(),
        rect: [x, y, x + 100.0, y + 14.0],
        size: 12.0,
        bold: false,
        italic: false,
        color: [0.0; 3],
        rotation: 0.0,
        space_around: [false; 2],
    };
    let table_page = Page {
        width: 612.0,
        height: 792.0,
        blocks: vec![
            cell("Metric", 72.0, 400.0),
            cell("Value", 200.0, 400.0),
            cell("Growth", 72.0, 360.0),
            cell("+9%", 200.0, 360.0),
            cell("Target", 72.0, 320.0),
            cell("100%", 200.0, 320.0),
        ],
        images: Vec::new(),
        background: None,
    };
    let bytes = pptx(&[sample_page(), table_page], "Two");
    let parts = entries(&bytes);
    assert_eq!(parts.first().map(|p| p.0.as_str()), Some("[Content_Types].xml"), "content types come first");
    let names: std::collections::HashSet<&str> = parts.iter().map(|p| p.0.as_str()).collect();
    assert_eq!(names.len(), parts.len(), "no duplicate parts");
    for required in [
        "[Content_Types].xml",
        "_rels/.rels",
        "ppt/presentation.xml",
        "ppt/_rels/presentation.xml.rels",
        "ppt/slideMasters/slideMaster1.xml",
        "ppt/slideMasters/_rels/slideMaster1.xml.rels",
        "ppt/slideLayouts/slideLayout1.xml",
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
        "ppt/theme/theme1.xml",
        "ppt/slides/slide1.xml",
        "ppt/slides/_rels/slide1.xml.rels",
        "ppt/slides/slide2.xml",
        "ppt/slides/_rels/slide2.xml.rels",
        "ppt/media/image1.png",
    ] {
        assert!(names.contains(required), "missing {required}");
    }
    let text = |n: &str| String::from_utf8(parts.iter().find(|p| p.0 == n).map(|p| p.1.clone()).unwrap_or_default()).expect("UTF-8 XML");
    for (name, body) in &parts {
        if name.ends_with(".xml") || name.ends_with(".rels") {
            assert!(body.starts_with(b"<?xml"), "{name} is XML");
            assert_tags_balance(name, &text(name));
        }
    }
    assert!(text("ppt/slides/slide2.xml").contains("<a:tbl>"), "the second slide carries the table");
    // Text boxes, not placeholders: the blank layout defines none to inherit from.
    for slide in ["ppt/slides/slide1.xml", "ppt/slides/slide2.xml"] {
        assert!(!text(slide).contains("<p:ph"), "{slide} uses no placeholders");
    }
    // Every override names a part that exists, and every XML part has a content type.
    let types = text("[Content_Types].xml");
    for chunk in types.split("PartName=\"/").skip(1) {
        let part = chunk.split('"').next().unwrap_or_default();
        assert!(names.contains(part), "override for missing part {part}");
    }
    for slide in ["ppt/slides/slide1.xml", "ppt/slides/slide2.xml", "ppt/presentation.xml", "ppt/theme/theme1.xml"] {
        assert!(types.contains(&format!("PartName=\"/{slide}\"")), "{slide} has a content type");
    }
    // Relationship targets resolve to parts in the package.
    for (rels, base) in [
        ("_rels/.rels", ""),
        ("ppt/_rels/presentation.xml.rels", "ppt/"),
        ("ppt/slides/_rels/slide1.xml.rels", "ppt/slides/"),
        ("ppt/slides/_rels/slide2.xml.rels", "ppt/slides/"),
        ("ppt/slideMasters/_rels/slideMaster1.xml.rels", "ppt/slideMasters/"),
        ("ppt/slideLayouts/_rels/slideLayout1.xml.rels", "ppt/slideLayouts/"),
    ] {
        for chunk in text(rels).split("Target=\"").skip(1) {
            let target = chunk.split('"').next().unwrap_or_default();
            let mut path: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
            for seg in target.split('/') {
                if seg == ".." {
                    path.pop();
                } else {
                    path.push(seg);
                }
            }
            let resolved = path.join("/");
            assert!(names.contains(resolved.as_str()), "{rels}: target {target} is missing");
        }
    }
    // The theme's font scheme carries the schema-required latin, ea and cs faces.
    let theme = text("ppt/theme/theme1.xml");
    for font in ["majorFont", "minorFont"] {
        let inner = theme.split(&format!("<a:{font}>")).nth(1).and_then(|r| r.split(&format!("</a:{font}>")).next()).unwrap_or_default();
        assert!(inner.contains("<a:latin ") && inner.contains("<a:ea ") && inner.contains("<a:cs "), "{font}: {inner}");
    }
    assert!(text("ppt/presentation.xml").contains("r:id=\"rIdSld2\""));
}

#[test]
fn hostile_sizes_do_not_overflow_the_slide_position() {
    let huge = |text: &str, size: f64, y: f64| Block {
        text: text.into(),
        rect: [72.0, y, 500.0, y + 10.0],
        size,
        bold: false,
        italic: false,
        color: [0.0; 3],
        rotation: 0.0,
        space_around: [false; 2],
    };
    let mut blocks = vec![huge("Title", f64::MAX, 500.0)];
    for k in 0..40 {
        blocks.push(huge(&"word ".repeat(10_000), 1e300, 400.0 - k as f64));
        blocks.push(huge("nan", f64::NAN, 300.0 - k as f64));
        blocks.push(huge("inf", f64::INFINITY, 200.0 - k as f64));
    }
    let images = (0..40)
        .map(|k| Image {
            ext: "png",
            bytes: [0; 4].as_slice().into(),
            rect: [0.0, -1e300, 1e-300, 1e300 - k as f64],
            size: [1.0, 1.0],
            rotation: 0.0,
        })
        .collect();
    let p = Page { width: f64::MAX, height: f64::NAN, blocks, images, background: None };
    let bytes = pptx(&[p], "Hostile");
    let slide = part(&bytes, "ppt/slides/slide1.xml");
    assert!(slide.contains("Title") && slide.contains("<p:pic>"), "the slide is still written");
    assert!(!slide.contains("y=\"-"), "positions never wrap negative");
    assert!(!slide.contains("<p:ph"), "text boxes, not placeholders");
}

#[test]
pub fn pptx_empty_pages_produce_valid_slide() {
    let bytes = pptx(&[], "Blank");
    assert!(bytes.starts_with(b"PK"));
    let pres_xml = part(&bytes, "ppt/presentation.xml");
    assert!(pres_xml.contains("<p:sldId id=\"256\" r:id=\"rIdSld1\"/>"));
    let slide_xml = part(&bytes, "ppt/slides/slide1.xml");
    assert!(slide_xml.contains("<p:spTree>"));
}

#[test]
fn pptx_slide_dimensions_and_clamping() {
    // Known dimensions: 720x540 pt (10x7.5 in) = 9144000x6858000 EMU
    let p = Page { width: 720.0, height: 540.0, ..Page::default() };
    let bytes = pptx(&[p], "Known Dimensions");
    let pres_xml = part(&bytes, "ppt/presentation.xml");
    assert!(pres_xml.contains("<p:sldSz cx=\"9144000\" cy=\"6858000\"/>"), "{pres_xml}");

    // Tiny page (10x10 pt) clamps to PowerPoint minimum of 1 inch (914400 EMU)
    let tiny = Page { width: 10.0, height: 10.0, ..Page::default() };
    let tiny_bytes = pptx(&[tiny], "Tiny");
    let tiny_pres = part(&tiny_bytes, "ppt/presentation.xml");
    assert!(tiny_pres.contains("<p:sldSz cx=\"914400\" cy=\"914400\"/>"), "{tiny_pres}");

    // Poster page (10000x10000 pt) clamps to PowerPoint maximum of 56 inches (51206400 EMU)
    let poster = Page { width: 10_000.0, height: 10_000.0, ..Page::default() };
    let poster_bytes = pptx(&[poster], "Poster");
    let poster_pres = part(&poster_bytes, "ppt/presentation.xml");
    assert!(poster_pres.contains("<p:sldSz cx=\"51206400\" cy=\"51206400\"/>"), "{poster_pres}");

    // Non-finite dimensions fall back to default
    let nan_page = Page { width: f64::NAN, height: -1.0, ..Page::default() };
    let nan_bytes = pptx(&[nan_page], "NaN");
    let nan_pres = part(&nan_bytes, "ppt/presentation.xml");
    assert!(nan_pres.contains("<p:sldSz cx=\"9144000\" cy=\"6858000\"/>"), "{nan_pres}");
}

#[test]
fn pptx_table_cell_with_largest_font_stays_in_table_and_not_in_title() {
    let cell = |t: &str, x: f64, y: f64, w: f64, sz: f64| Block {
        text: t.into(),
        rect: [x, y, x + w, y + 14.0],
        size: sz,
        bold: false,
        italic: false,
        color: [0.0; 3],
        rotation: 0.0,
        space_around: [false; 2],
    };
    let p = Page {
        width: 720.0,
        height: 540.0,
        blocks: vec![
            // Heading outside the table (short width, does not span table columns)
            Block {
                text: "Monthly Summary".into(),
                rect: [50.0, 480.0, 150.0, 500.0],
                size: 20.0,
                bold: true,
                italic: false,
                color: [0.0; 3],
                rotation: 0.0,
                space_around: [false; 2],
            },
            // Table where one cell has a very large number (e.g. 36 pt callout in the table)
            cell("Metric", 72.0, 400.0, 100.0, 12.0),
            cell("Value", 200.0, 400.0, 100.0, 12.0),
            cell("Growth", 72.0, 360.0, 100.0, 12.0),
            cell("+99%", 200.0, 360.0, 100.0, 36.0),
            cell("Target", 72.0, 320.0, 100.0, 12.0),
            cell("100%", 200.0, 320.0, 100.0, 12.0),
        ],
        images: Vec::new(),
        background: None,
    };
    let bytes = pptx(&[p], "Table Check");
    let slide_xml = part(&bytes, "ppt/slides/slide1.xml");
    // Title should be the block outside the table ("Monthly Summary"), not the "+99%" cell
    assert!(slide_xml.contains("Monthly Summary"));
    assert!(slide_xml.contains("name=\"Title 2\""));
    // The table must contain both the cell and explicit borders
    assert!(slide_xml.contains("<a:tbl>"));
    assert!(slide_xml.contains("+99%"));
    assert!(slide_xml.contains("<a:tcPr><a:lnL w=\"12700\" cmpd=\"s\">"));
}

#[test]
fn pptx_opens_in_libreoffice() {
    let p = sample_page();
    let bytes = pptx(&[p], "LibreOffice Check");
    let dir = std::env::temp_dir().join(format!("pdfcraft-pptx-lo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    let pptx_path = dir.join("test.pptx");
    std::fs::write(&pptx_path, &bytes).unwrap();

    for cmd in ["libreoffice", "soffice"] {
        let status = std::process::Command::new(cmd).args(["--headless", "--convert-to", "pdf", "--outdir"]).arg(&dir).arg(&pptx_path).status();
        if status.is_ok_and(|st| st.success()) {
            assert!(dir.join("test.pdf").is_file(), "LibreOffice produced test.pdf");
            break;
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
